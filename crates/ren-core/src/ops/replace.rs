//! Find & Replace, and Batch Replace.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::cache::Cached;
use crate::matcher::MatchSpec;
use crate::regex_flavor::{Pattern, PatternOptions, RegexError};
use crate::template::{Template, TextTemplate};
use crate::wildcard;

/// Find & Replace: every occurrence of one string in the name becomes another.
///
/// Case Sensitive, Swap Mode and Regular Expression all start unchecked, and
/// Skip and Max both start at 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Replace {
    /// Compiled on first use. Without this the regex is rebuilt once per file
    /// per keystroke, which alone blew the M1 planning budget three times over.
    #[serde(skip)]
    compiled: Cached<Result<Pattern, RegexError>>,
    /// What to look for. Plain text or a wildcard mask; a regular expression
    /// when [`Self::regex`] is on.
    pub find: String,
    /// What each occurrence becomes. Empty deletes it.
    ///
    /// A template, so it takes tags. `$1`–`$9` are capture references in
    /// regex mode and plain text otherwise, because only a pattern has
    /// captures to refer to. Serialises transparently, so every preset and job
    /// file written before it became a template still parses.
    pub replace: TextTemplate,
    /// Match case exactly.
    pub case_sensitive: bool,
    /// Exchange the two strings in one pass: `find` becomes the replacement and
    /// the replacement becomes `find`. See [`Self::swap_applies`] for when it
    /// is ignored.
    pub swap: bool,
    /// Read `find` as a regular expression rather than text or a wildcard mask.
    pub regex: bool,
    /// Leave the first this-many occurrences alone.
    pub skip: usize,
    /// Replace at most this many occurrences per name; 0 is unlimited. The
    /// card labels it **Max**.
    pub max: usize,
}

impl Replace {
    pub fn new(find: impl Into<String>, replace: impl Into<TextTemplate>) -> Self {
        Self {
            find: find.into(),
            replace: replace.into(),
            ..Default::default()
        }
    }

    pub fn regex(mut self, yes: bool) -> Self {
        self.regex = yes;
        self
    }

    pub fn case_sensitive(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self
    }

    pub fn swap(mut self, yes: bool) -> Self {
        self.swap = yes;
        self
    }

    pub fn skip(mut self, n: usize) -> Self {
        self.skip = n;
        self
    }

    pub fn max(mut self, n: usize) -> Self {
        self.max = n;
        self
    }

    /// The replacement as a fixed string, when it is one.
    ///
    /// `None` means it holds tags — or does not compile, in which case `apply`
    /// is the right place for the error and this is not.
    fn literal_replacement(&self) -> Option<&str> {
        self.replace
            .compiled()
            .ok()
            .and_then(Template::literal_text)
    }

    /// Whether Swap Mode takes effect for this configuration.
    ///
    /// Not with a wildcard in the find box, nor in regex mode (P17): a mask or
    /// a pattern cannot be turned back into the literal text it matched, so
    /// there is nothing to swap with.
    /// And to a replacement holding tags, for a mechanical reason rather than a
    /// logical one: swap compiles an alternation of *both* literals into one
    /// pattern cached for the whole operation, and a value that differs per
    /// file cannot live in that cache — while a per-file pattern would thrash
    /// the process-wide regex cache P40 exists to protect (D116). Nor with an
    /// empty replacement, which has nothing to swap with. The editor draws
    /// "swap ignored here" whenever this returns false.
    pub fn swap_applies(&self) -> bool {
        self.swap_applies_with(self.literal_replacement())
    }

    /// The same, given an answer already in hand — `apply` has one and asking
    /// again per file is work with a known result.
    ///
    /// An empty replacement has nothing to swap with, so it does not count:
    /// the card then deletes, which is what its summary says, rather than
    /// silently doing nothing.
    fn swap_applies_with(&self, literal: Option<&str>) -> bool {
        self.swap
            && !self.regex
            && !wildcard::has_wildcards(&self.find)
            && literal.is_some_and(|text| !text.is_empty())
    }

    fn spec(&self) -> MatchSpec {
        if self.regex {
            MatchSpec::Regex(self.find.clone())
        } else {
            MatchSpec::auto(self.find.clone())
        }
    }

    fn options(&self) -> PatternOptions {
        PatternOptions {
            case_sensitive: self.case_sensitive,
            ..Default::default()
        }
    }

    /// The regex text this operation matches with, before compilation.
    ///
    /// Swap Mode needs a different pattern (an alternation of both strings), so
    /// it is folded in here rather than compiled separately.
    ///
    /// The longer string goes first. The engines take the leftmost alternative
    /// that matches, so with `Art|Artist` the `Art` inside `Artist` won and
    /// `Artist - Art` became `Artistist - Artist`. Which string matched is
    /// decided afterwards by comparing it with `find`, so the order of the
    /// alternatives changes nothing else.
    fn pattern_source(&self) -> String {
        if self.swap_applies() {
            let find = self.find.as_str();
            let replacement = self.literal_replacement().unwrap_or_default();
            let (first, second) = if replacement.len() > find.len() {
                (replacement, find)
            } else {
                (find, replacement)
            };
            format!("{}|{}", regex_escape(first), regex_escape(second))
        } else {
            match self.spec() {
                MatchSpec::Substring(s) => regex_escape(&s),
                MatchSpec::Wildcard(s) => wildcard::to_regex(&s),
                MatchSpec::Regex(s) => s,
            }
        }
    }

    /// The pattern this operation matches with, compiled once.
    fn pattern(&self) -> Result<&Pattern, RegexError> {
        self.compiled
            .get_or_init(|| Pattern::compile(&self.pattern_source(), self.options()))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Swap Mode: one simultaneous pass, so text written by one direction is
    /// never re-read by the other. A sequential two-pass replace would turn
    /// every `find` into `replace` and then straight back again.
    fn swap_both_ways(&self, subject: &str, replacement: &str) -> Result<String, OpError> {
        let pattern = self.pattern().map_err(|e| OpError::new("replace", e))?;

        let mut out = String::with_capacity(subject.len());
        let mut copied_to = 0usize;
        let mut seen = 0usize;
        let mut replaced = 0usize;

        for hit in pattern
            .find_all(subject)
            .map_err(|e| OpError::new("replace", e))?
        {
            seen += 1;
            if seen <= self.skip {
                continue;
            }
            if self.max != 0 && replaced >= self.max {
                break;
            }
            out.push_str(&subject[copied_to..hit.start]);
            let matched = &subject[hit.clone()];
            out.push_str(if equals(matched, &self.find, self.case_sensitive) {
                replacement
            } else {
                &self.find
            });
            copied_to = hit.end;
            replaced += 1;
        }
        out.push_str(&subject[copied_to..]);
        Ok(out)
    }
}

fn equals(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

/// Escapes every regex metacharacter so a literal string matches itself.
///
/// Public because Visual Assist lifts a span of a real filename into the *Look
/// For* box, and that box is an interpreter when *Regular expression* is
/// ticked: `(1)` selected from `photo (1).jpg` would otherwise become a capture
/// group matching the bare `1`. **D115**'s rule decides it — what the data said
/// is sanitised, what the user wrote is obeyed — and `\(1\)` is precisely the
/// pattern that means the literal the user pointed at.
pub fn regex_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len() + 8);
    for c in literal.chars() {
        if "\\.+*?()|[]{}^$#&~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl NameTransform for Replace {
    fn id(&self) -> &'static str {
        "replace"
    }

    fn summary(&self) -> String {
        // A card the user has only just added has an empty *find* box, and
        // `Delete ""` is a summary that describes a deletion of nothing rather
        // than a card waiting to be filled in. It is also the first thing a
        // new user reads in the pipeline panel.
        if self.find.is_empty() {
            return "Nothing to find yet".to_owned();
        }
        if self.replace.is_empty() {
            format!("Delete {:?}", self.find)
        } else {
            format!("Replace {:?} -> {:?}", self.find, self.replace.as_str())
        }
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.replace.needs()
    }

    fn asks(&self) -> Vec<crate::run::AskSpec> {
        self.replace.asks()
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.find.is_empty() {
            return Ok(Cow::Borrowed(subject));
        }

        // Asked once. This runs per file per keystroke, and the answer cannot
        // change between the swap check and the replacement — reading it twice
        // was doing the same cached lookups and the same `literal_text` match
        // twice for every file in the listing.
        let literal = self.literal_replacement();

        if self.swap_applies_with(literal) {
            let out = self.swap_both_ways(subject, literal.unwrap_or_default())?;
            return Ok(borrow_if_unchanged(subject, out));
        }

        // The common case by far: a replacement with no tags in it is the text
        // the user typed, and there is nothing to render.
        let replacement = if self.regex {
            match literal {
                Some(text) => Cow::Borrowed(text),
                // `$1`–`$9` in the replacement are capture references, so a
                // `$` that arrived *in a tag value* is escaped and a `$` the
                // user typed is not (D115).
                None => match cx.render_with(&self.replace, escape_dollars_into)? {
                    Some(text) => text,
                    // Only rename if all tags are available.
                    None => return Ok(Cow::Borrowed(subject)),
                },
            }
        } else {
            // Without the regex box there are no capture groups to refer to —
            // the find is escaped, or a wildcard mask translated without any —
            // so every `$` is a dollar sign, typed or not. Left to the regex
            // engine, `$10` expanded to an empty group and vanished, and the
            // same card with Swap ticked (which writes the text as-is) kept it.
            let text = match literal {
                Some(text) => Cow::Borrowed(text),
                None => match cx.render(&self.replace)? {
                    Some(text) => text,
                    None => return Ok(Cow::Borrowed(subject)),
                },
            };
            if text.contains('$') {
                let mut escaped = String::with_capacity(text.len() + 4);
                escape_dollars_into(&text, &mut escaped);
                Cow::Owned(escaped)
            } else {
                text
            }
        };

        let pattern = self.pattern().map_err(|e| OpError::new("replace", e))?;
        // Nothing to replace is the common answer, and it is answered before
        // anything is built: a Batch Replace card runs fifty-one rules over
        // every file on every keystroke, and almost none of them match almost
        // any name. `find` is the engine's cheapest question — no capture
        // groups, no allocation — and `replace_skipping` used to pay for the
        // output string and the translated replacement on every miss.
        if pattern
            .find(subject)
            .map_err(|e| OpError::new("replace", e))?
            .is_none()
        {
            return Ok(Cow::Borrowed(subject));
        }
        let out = pattern
            .replace_skipping(subject, &replacement, self.skip, self.max)
            .map_err(|e| OpError::new("replace", e))?;
        Ok(borrow_if_unchanged(subject, out))
    }
}

/// Appends `text` with every `$` doubled — `$$` is the regex crate's literal
/// dollar in a replacement.
fn escape_dollars_into(text: &str, out: &mut String) {
    for c in text.chars() {
        if c == '$' {
            out.push('$');
        }
        out.push(c);
    }
}

fn borrow_if_unchanged(subject: &str, produced: String) -> Cow<'_, str> {
    if produced == subject {
        Cow::Borrowed(subject)
    } else {
        Cow::Owned(produced)
    }
}

/// Batch Replace: a list of Replace rules run top to bottom in one card.
///
/// Ships pre-loaded with 51 rules. An empty Batch Replace would be an
/// operation the user can select and that then does nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BatchReplace {
    /// Executed top-down, each seeing the previous rule's output.
    pub rules: Vec<Replace>,
    /// Which rules can match a given name, asked of all of them at once.
    /// Looked up on first use and reset by the clone `to_step` makes per
    /// preview (D21), like `Replace::compiled` beside it; the set itself is
    /// shared process-wide (P40), because building one compiles fifty-one
    /// patterns and the clone happens on every keystroke.
    #[serde(skip)]
    prefilter: Cached<Arc<RuleSet>>,
    /// The first rule whose replacement does not compile, as the error every
    /// row reports. Checked once per operation rather than left to the rules
    /// that happen to run: see [`Self::template_error`].
    #[serde(skip)]
    broken: Cached<Option<OpError>>,
}

impl Default for BatchReplace {
    fn default() -> Self {
        Self::new(shipped_rules().clone())
    }
}

/// Every rule's pattern in one `RegexSet`, so "which of these fifty-one could
/// possibly match this name" is one scan rather than fifty-one.
///
/// **Why it is exact.** A pattern without a fancy feature — lookaround, a
/// backreference — is one the `regex` crate accepts, and `fancy-regex` runs
/// such a pattern on the `regex` crate too, after parsing it with its own
/// parser and handing over the tree it built. So the set and the rule are
/// two parses of the same text by parsers that are expected to agree on
/// every non-fancy construct; `the_batch_replace_prefilter_agrees_with_running_every_rule`
/// and its generated-rules sibling in `tests/properties.rs` hold them to it.
/// A pattern the `regex` crate refuses is left out of the set and treated as
/// *always* possibly matching, which costs its full run per file and never a
/// missed match. The set is asked again after every rule that changed the
/// name, because the rules are sequential and a later rule sees the earlier
/// one's output.
///
/// **What it does not decide.** Whether a rule's replacement compiles: a set
/// that skips a rule would also skip that rule's template error, which D29
/// makes an error on every row. `BatchReplace` checks the templates itself.
///
/// **Why it exists.** A Batch Replace card runs its rules over every file on
/// every keystroke, and the shipped fifty-one are English contractions that
/// match almost no name. Fifty-one `find`s per file were most of the cost of
/// the whole preview; one set scan is a fraction of one.
struct RuleSet {
    set: Option<regex::RegexSet>,
    /// `set`'s pattern `i` is rule `members[i]`.
    members: Vec<usize>,
    /// Rules the set cannot speak for: always run.
    unfiltered: Vec<usize>,
    /// How many rules the set was built for. An operation edited in place
    /// keeps its cached set (D21), and a set for a longer list hands back
    /// indices the list no longer has — so a length that disagrees means the
    /// set is rebuilt rather than trusted. See `BatchReplace::apply`.
    len: usize,
}

impl RuleSet {
    /// The set for `rules`, shared with every other `BatchReplace` holding
    /// the same rules.
    ///
    /// P40's reasoning, one level up: `to_step` clones the operation on every
    /// keystroke and a clone resets `prefilter`, so without this the fifty-one
    /// patterns were compiled — twice, once to probe and once into the set —
    /// per keystroke, which cost more than the fifty-one finds it replaced.
    /// Keyed by the pattern texts, so two cards with the same rules share one
    /// set and an edited rule gets a fresh one.
    fn shared(rules: &[Replace]) -> Arc<Self> {
        static SETS: OnceLock<Mutex<HashMap<Vec<String>, Arc<RuleSet>>>> = OnceLock::new();
        const CAPACITY: usize = 64;

        let key: Vec<String> = rules
            .iter()
            .map(|rule| {
                let mut source = rule.pattern_source();
                if !rule.case_sensitive {
                    source.insert_str(0, "(?i)");
                }
                source
            })
            .collect();
        let sets = SETS.get_or_init(Default::default);
        if let Ok(map) = sets.lock()
            && let Some(set) = map.get(&key)
        {
            return set.clone();
        }
        let set = Arc::new(Self::build(rules));
        if let Ok(mut map) = sets.lock() {
            // A user typing into a rule produces a new key per keystroke;
            // cleared wholesale when full, as a cost cache may be.
            if map.len() >= CAPACITY {
                map.clear();
            }
            map.insert(key, set.clone());
        }
        set
    }

    fn build(rules: &[Replace]) -> Self {
        let mut sources = Vec::new();
        let mut members = Vec::new();
        let mut unfiltered = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            if rule.find.is_empty() {
                continue; // `Replace::apply` answers a borrow at once.
            }
            let mut source = rule.pattern_source();
            if !rule.case_sensitive {
                source.insert_str(0, "(?i)");
            }
            match regex::Regex::new(&source) {
                Ok(_) => {
                    sources.push(source);
                    members.push(index);
                }
                Err(_) => unfiltered.push(index),
            }
        }
        // A set that fails to build — a size limit on the union — makes every
        // member unfiltered rather than a failed operation: the prefilter is
        // a cost cache, never a correctness one.
        let set = match regex::RegexSet::new(&sources) {
            Ok(set) => Some(set),
            Err(_) => {
                unfiltered.append(&mut members);
                unfiltered.sort_unstable();
                None
            }
        };
        Self {
            set,
            members,
            unfiltered,
            len: rules.len(),
        }
    }

    /// The rules that may match `name`, in rule order, starting at `from`.
    fn candidates(&self, name: &str, from: usize, out: &mut Vec<usize>) {
        out.clear();
        if let Some(set) = &self.set {
            out.extend(
                set.matches(name)
                    .iter()
                    .map(|i| self.members[i])
                    .filter(|&rule| rule >= from),
            );
        }
        out.extend(self.unfiltered.iter().copied().filter(|&rule| rule >= from));
        out.sort_unstable();
    }
}

impl BatchReplace {
    pub fn new(rules: Vec<Replace>) -> Self {
        Self {
            rules,
            prefilter: Cached::new(),
            broken: Cached::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// The error every row reports when a rule's replacement does not compile.
    ///
    /// Running every rule used to find it on every row, because
    /// `Replace::apply` renders its replacement before it looks for a match.
    /// The prefilter runs only the rules whose find can match, so a mistyped
    /// tag in any other rule went unreported until a file matching its find
    /// was listed — D29's hard error quietly became a rule that did nothing.
    /// A rule with an empty find is skipped, as `Replace::apply` skips it
    /// before rendering anything.
    fn template_error(&self) -> Option<&OpError> {
        self.broken
            .get_or_init(|| {
                self.rules
                    .iter()
                    .filter(|rule| !rule.find.is_empty())
                    .find_map(|rule| {
                        rule.replace
                            .compiled()
                            .err()
                            .map(|e| OpError::new("tags", e.to_string()))
                    })
            })
            .as_ref()
    }
}

// --- The shipped default list ------------------------------------------------

#[derive(Debug, Deserialize)]
struct ShippedFile {
    #[allow(dead_code)]
    version: u32,
    rule: Vec<LiteralRule>,
    contractions: Contractions,
}

#[derive(Debug, Deserialize)]
struct LiteralRule {
    find: String,
    replace: String,
    regex: bool,
    #[allow(dead_code)]
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct Contractions {
    stand_ins: Vec<String>,
    boundary: String,
    pairs: Vec<(String, String)>,
}

/// The defaults we ship (D6/D20), parsed once.
///
/// The contraction rules are *generated* from linguistic pairs rather than
/// stored as fifty near-identical regexes — see
/// `crates/ren-core/data/batch_replace_default.toml`.
pub fn shipped_rules() -> &'static Vec<Replace> {
    static RULES: std::sync::OnceLock<Vec<Replace>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let text = include_str!("../../data/batch_replace_default.toml");
        let parsed: ShippedFile =
            toml::from_str(text).expect("the shipped batch-replace defaults must parse");

        let mut rules: Vec<Replace> = parsed
            .rule
            .iter()
            .map(|r| Replace::new(&r.find, r.replace.as_str()).regex(r.regex))
            .collect();

        let class: String = parsed.contractions.stand_ins.concat();
        let boundary = &parsed.contractions.boundary;
        for (head, tail) in &parsed.contractions.pairs {
            rules.push(
                Replace::new(
                    format!("( {head})[{class}]?({tail}){boundary}"),
                    format!("$1'$2{boundary}"),
                )
                .regex(true),
            );
        }
        rules
    })
}

impl NameTransform for BatchReplace {
    fn id(&self) -> &'static str {
        "batch_replace"
    }

    fn summary(&self) -> String {
        format!("Batch replace ({} rules)", self.rules.len())
    }

    fn needs(&self) -> crate::template::TagNeeds {
        self.rules
            .iter()
            .fold(crate::template::TagNeeds::NONE, |needs, rule| {
                needs.union(rule.needs())
            })
    }

    /// Every rule's `<Ask>` slots, once each. Without this a rule holding
    /// `<Ask>` was never asked about: the run collects answers only for the
    /// slots the pipeline declares, and renders an unanswered one as "leave
    /// this name alone" (P33) — so the rule silently did nothing.
    fn asks(&self) -> Vec<crate::run::AskSpec> {
        let mut asks: Vec<crate::run::AskSpec> =
            self.rules.iter().flat_map(NameTransform::asks).collect();
        asks.sort_by_key(|a| a.slot);
        asks.dedup_by_key(|a| a.slot);
        asks
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if let Some(error) = self.template_error() {
            return Err(error.clone());
        }
        let cached = self.prefilter.get_or_init(|| RuleSet::shared(&self.rules));
        // A set built for a different number of rules is one the list was
        // edited out from under (D21 — see `OpKind::refresh`, which is the
        // real fix). Its indices cannot be trusted, and one past the end used
        // to panic on the UI thread, so it is fetched afresh for this call.
        // The shared cache keeps that cheap. A same-length edit is not caught
        // here; it can only run the wrong rules in a card's probe, never
        // index out of bounds.
        let rebuilt;
        let prefilter = if cached.len == self.rules.len() {
            cached
        } else {
            rebuilt = RuleSet::shared(&self.rules);
            &rebuilt
        };
        let mut current = Cow::Borrowed(subject);
        let mut candidates = Vec::new();
        let mut from = 0;
        // Ask the set which rules can match what the name is *now*, run those
        // in order, and ask again from the next rule whenever one of them
        // changed the name — the same top-down, each-sees-the-last semantics
        // as running all of them, minus the ones that could not have fired.
        'rescan: loop {
            prefilter.candidates(&current, from, &mut candidates);
            for &index in &candidates {
                debug_assert!(index < self.rules.len(), "a prefilter index past the rules");
                let Some(rule) = self.rules.get(index) else {
                    continue;
                };
                if let Cow::Owned(next) = rule.apply(&current, cx)? {
                    current = Cow::Owned(next);
                    from = index + 1;
                    continue 'rescan;
                }
            }
            break;
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::run;
    use super::*;

    #[test]
    fn replacing_underscores_with_spaces_tidies_up_a_filename() {
        assert_eq!(
            run(&Replace::new("_", " "), "my_holiday_photo"),
            "my holiday photo"
        );
    }

    // --- tags in the replace box (M8) --------------------------------------

    /// It once could not: `replace` was a plain `String` and `apply` took an
    /// `EvalCx` it ignored, so `<Parent>` was written into the filename as
    /// typed, angle brackets and all.
    #[test]
    fn the_replace_box_takes_tags() {
        // `run` builds the entry under /tmp, so `<Parent>` is "tmp".
        assert_eq!(run(&Replace::new("_", "<Parent>"), "a_b"), "atmpb");
    }

    /// D48 and D61's rule, in the one field where a rendered value re-enters an
    /// interpreter: what the data said is sanitised, what the user wrote is
    /// obeyed. A file whose own name holds `$1` keeps it.
    #[test]
    fn a_dollar_from_a_tag_is_text_and_a_dollar_the_user_typed_is_a_capture() {
        assert_eq!(run(&Replace::new("Q", "<Name>"), "a$1bQ"), "a$1ba$1bQ");
        assert_eq!(
            run(&Replace::new("(a)(b)", "$2$1").regex(true), "ab"),
            "ba",
            "a capture reference in the box the user typed in still means capture"
        );
    }

    /// Swap compiles an alternation of *both* literals into one pattern cached
    /// for the whole operation, so a replacement that differs per file cannot
    /// take part. Same shape as the wildcard and regex exclusions, and the
    /// editor draws "swap ignored here" for all three.
    #[test]
    fn swap_mode_is_ignored_when_the_replace_box_holds_tags() {
        assert!(Replace::new("a", "b").swap(true).swap_applies());
        assert!(!Replace::new("a", "<Name>").swap(true).swap_applies());
        // And the behaviour, not just the predicate: with a tag it replaces one
        // way only.
        assert_eq!(run(&Replace::new("a", "b").swap(true), "ab"), "ba");
        assert_eq!(run(&Replace::new("a", "<Parent>").swap(true), "ab"), "tmpb");
    }

    /// D29 makes an unrecognised closed tag a hard error rather than literal
    /// text. That now reaches this box too — the compatibility break recorded
    /// with the change.
    #[test]
    fn an_unrecognised_tag_in_the_replace_box_is_an_error() {
        let e = super::super::testing::entry("x");
        let cx = EvalCx::simple(&e, 0, 1);
        assert!(Replace::new("x", "<Nmae>").apply("x", &cx).is_err());
        // An unclosed `<` stays literal, which is what keeps the blast radius
        // narrow: only a *closed* unrecognised tag changes meaning.
        assert_eq!(run(&Replace::new("x", "<Nmae"), "x"), "<Nmae");
    }

    /// `TextTemplate` is `#[serde(transparent)]`, so a preset or job file
    /// written when this was a plain string still parses unchanged.
    #[test]
    fn a_preset_written_before_the_box_took_tags_still_parses() {
        let op: Replace = toml::from_str("find = \"_\"\nreplace = \" \"\n").unwrap();
        assert_eq!(op.replace.as_str(), " ");
        assert_eq!(run(&op, "a_b"), "a b");
    }

    #[test]
    fn an_empty_replacement_deletes_the_search_string() {
        assert_eq!(run(&Replace::new("XXX", ""), "aXXXbXXXc"), "abc");
    }

    /// Unchecked, `Photo` and `photo` are the same word to the matcher.
    #[test]
    fn matching_is_case_insensitive_unless_case_sensitive_is_checked() {
        assert_eq!(run(&Replace::new("photo", "P"), "Photo photo"), "P P");
        assert_eq!(
            run(
                &Replace::new("photo", "P").case_sensitive(true),
                "Photo photo"
            ),
            "Photo P"
        );
    }

    #[test]
    fn wildcards_work_in_the_find_box() {
        // "*" matches zero or more characters.
        assert_eq!(
            run(&Replace::new("(*)", ""), "song (live).mp3"),
            "song .mp3"
        );
        // "?" matches exactly one.
        assert_eq!(run(&Replace::new("track ?", "#"), "track 7"), "#");
    }

    #[test]
    fn regex_mode_supports_capture_groups_in_the_replacement() {
        // One of the shipped contraction rules.
        let op = Replace::new(r"( don)[ `´]?(t)", "$1'$2").regex(true);
        assert_eq!(run(&op, "I dont care"), "I don't care");
        assert_eq!(run(&op, "I don`t care"), "I don't care");
    }

    #[test]
    fn a_literal_find_string_never_acts_as_a_regex() {
        // Without the regex box ticked, "." is just a period.
        assert_eq!(run(&Replace::new(".", "-"), "a.b.c"), "a-b-c");
        assert_eq!(run(&Replace::new("a+b", "x"), "a+b ab"), "x ab");
    }

    #[test]
    fn skip_passes_over_the_first_occurrences() {
        assert_eq!(run(&Replace::new("a", "X").skip(0), "aaaa"), "XXXX");
        assert_eq!(run(&Replace::new("a", "X").skip(2), "aaaa"), "aaXX");
    }

    #[test]
    fn max_limits_the_replacements_and_zero_means_unlimited() {
        assert_eq!(run(&Replace::new("a", "X").max(1), "aaaa"), "Xaaa");
        assert_eq!(run(&Replace::new("a", "X").max(0), "aaaa"), "XXXX");
    }

    #[test]
    fn swap_mode_exchanges_the_two_strings_in_one_pass() {
        let op = Replace::new("Artist", "Title").swap(true);
        assert_eq!(run(&op, "Artist - Title"), "Title - Artist");
        // A sequential two-pass implementation would produce "Artist - Artist".
        assert_eq!(run(&op, "Artist Artist Title"), "Title Title Artist");
    }

    /// Without the regex box ticked, a `$` in the replacement is a dollar sign.
    /// `$1`–`$9` are capture references only where there are captures to
    /// refer to, and a plain-text find has none — so `$10` used to vanish.
    #[test]
    fn a_dollar_in_a_plain_text_replacement_is_text() {
        assert_eq!(run(&Replace::new("a", "$1"), "a"), "$1");
        assert_eq!(run(&Replace::new("USD", "$10"), "5 USD"), "5 $10");
        assert_eq!(run(&Replace::new("x", "$$"), "x"), "$$");
        assert_eq!(
            run(&Replace::new("a*c", "$1"), "abc"),
            "$1",
            "wildcards too"
        );
        // Beside a tag, with swap on and off: the same card, the same answer.
        assert_eq!(run(&Replace::new("Q", "$1<Parent>"), "Q"), "$1tmp");
        assert_eq!(run(&Replace::new("a", "$5").swap(true), "a"), "$5");
    }

    /// Swap compiles `find|replace` into one alternation, and the engines take
    /// the leftmost alternative that matches — so when one string begins the
    /// other, the shorter one used to win inside the longer.
    #[test]
    fn swap_mode_works_when_one_string_begins_the_other() {
        assert_eq!(
            run(&Replace::new("Art", "Artist").swap(true), "Artist - Art"),
            "Art - Artist"
        );
        assert_eq!(
            run(&Replace::new("Artist", "Art").swap(true), "Artist - Art"),
            "Art - Artist"
        );
        assert_eq!(
            run(&Replace::new("cas", "casing").swap(true), "casing_default"),
            "cas_default"
        );
    }

    /// There is nothing to swap with an empty replacement, so the card does
    /// what its summary says — deletes — and the editor says swap is ignored.
    #[test]
    fn swap_mode_with_an_empty_replacement_deletes() {
        let op = Replace::new("a", "").swap(true);
        assert!(!op.swap_applies());
        assert_eq!(run(&op, "ab"), "b");
    }

    #[test]
    fn swap_mode_is_ignored_when_the_find_box_holds_wildcards() {
        let op = Replace::new("a*b", "Z").swap(true);
        assert!(!op.swap_applies());
        assert_eq!(run(&op, "axxb Z"), "Z Z");
    }

    /// P17: the same reasoning, for a pattern.
    #[test]
    fn swap_mode_is_ignored_in_regex_mode_too() {
        let op = Replace::new("a.b", "Z").regex(true).swap(true);
        assert!(!op.swap_applies());
    }

    #[test]
    fn an_empty_find_box_is_a_no_op() {
        assert_eq!(run(&Replace::new("", "X"), "unchanged"), "unchanged");
    }

    #[test]
    fn batch_replace_runs_its_rules_top_down() {
        let batch = BatchReplace::new(vec![
            Replace::new("_", " "),
            Replace::new("  ", " "),
            Replace::new("dont", "don't"),
        ]);
        assert_eq!(run(&batch, "i_dont__care"), "i don't care");
    }

    #[test]
    fn a_batch_with_no_rules_changes_nothing() {
        assert_eq!(run(&BatchReplace::empty(), "untouched"), "untouched");
    }

    /// A rule the prefilter cannot speak for — a lookahead is a fancy
    /// feature the `regex` crate refuses — is always run, so it still fires,
    /// and a later rule still sees what it produced.
    #[test]
    fn a_rule_the_prefilter_cannot_index_still_runs_in_order() {
        let batch = BatchReplace::new(vec![
            Replace::new(r"a(?=b)", "X").regex(true),
            Replace::new("Xb", "done"),
            Replace::new("_", " "),
        ]);
        let set = RuleSet::build(&batch.rules);
        assert_eq!(set.unfiltered, [0], "the lookahead rule is outside the set");
        assert_eq!(set.members, [1, 2]);
        assert_eq!(run(&batch, "ab_c"), "done c");
    }

    /// The set is asked again after a rule changes the name, because a later
    /// rule may match only what an earlier one produced.
    #[test]
    fn a_rule_that_matches_only_an_earlier_rules_output_still_fires() {
        let batch = BatchReplace::new(vec![Replace::new("_", " "), Replace::new("don t", "don't")]);
        assert_eq!(run(&batch, "i_don_t_care"), "i don't care");
    }

    /// The rule table edits `rules` in place, and the prefilter was built for
    /// the list as it was. An index the old set hands back that the new list
    /// no longer has must not take the app down — the set is rebuilt instead.
    #[test]
    fn deleting_a_rule_after_the_prefilter_was_built_does_not_panic() {
        let mut batch = BatchReplace::new(vec![
            Replace::new("x", "y"),
            Replace::new("q", "z"),
            Replace::new("e", "E"),
        ]);
        assert_eq!(run(&batch, "name"), "namE");
        batch.rules.remove(0);
        assert_eq!(run(&batch, "name"), "namE");
        batch.rules.clear();
        assert_eq!(run(&batch, "name"), "name");
    }

    /// A rule's replacement is a template, and D29 makes a bad tag an error on
    /// every row. The prefilter decides which rules *run*, so without an
    /// up-front check a typo only surfaced once a file matching its find was
    /// listed.
    #[test]
    fn a_mistyped_tag_in_any_rule_is_an_error_whether_or_not_its_find_matches() {
        let batch = BatchReplace::new(vec![Replace::new("_", " "), Replace::new("zzz", "<Nmae>")]);
        let e = super::super::testing::entry("a_b");
        let cx = EvalCx::simple(&e, 0, 1);
        let err = batch.apply("a_b", &cx).unwrap_err();
        assert!(err.to_string().contains("<Nmae>"), "{err}");
    }

    /// Every rule's `<Ask>` and `<Clipboard>` has to reach the pipeline, or
    /// the run never collects them and the rule silently does nothing.
    #[test]
    fn a_batch_declares_what_its_rules_need_and_ask() {
        use crate::template::TagNeeds;
        let batch = BatchReplace::new(vec![
            Replace::new("a", "<Ask-2>"),
            Replace::new("b", "<Clipboard>"),
            Replace::new("c", "<Ask-2>"),
        ]);
        assert!(batch.needs().contains(TagNeeds::ASK));
        assert!(batch.needs().contains(TagNeeds::CLIPBOARD));
        let slots: Vec<u8> = batch.asks().iter().map(|a| a.slot).collect();
        assert_eq!(slots, [2], "one question for the slot two rules share");
        assert_eq!(BatchReplace::default().needs(), TagNeeds::NONE);
    }

    /// Fifty-one: the shipped data file's literal rules plus the contractions
    /// generated from it. A different count means the file or the generator
    /// changed.
    #[test]
    fn the_shipped_list_has_fifty_one_rules() {
        assert_eq!(shipped_rules().len(), 51);
        assert_eq!(BatchReplace::default().rules.len(), 51);
    }

    /// The whole point of shipping it pre-loaded: selecting Batch Replace and
    /// doing nothing else already tidies a name up.
    #[test]
    fn the_shipped_list_fixes_underscores_and_contractions_out_of_the_box() {
        let batch = BatchReplace::default();
        assert_eq!(run(&batch, "i_dont_care"), "i don't care");
        assert_eq!(run(&batch, "it_isnt_here "), "it isn't here ");
        assert_eq!(run(&batch, "nothing to fix"), "nothing to fix");
    }
}
