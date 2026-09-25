//! The include filter: deciding whether a file takes part in a step at all.
//!
//! Two boxes, include and exclude, each read as plain text, a wildcard mask or
//! a regular expression. A file takes part when it matches the include (or
//! there is none) and does not match the exclude — the exclude is applied
//! second, so it carves files back out of what the include let in.
//!
//! **What is tested.** The stem is always a subject. The extension option
//! adds the bare extension *and the whole name* (`stem.ext`), so a mask across
//! the dot — `*.bak`, `*.mp3` — means what it says; without it only the stem
//! is tested (P19 decides how a mask reads; this decides what it reads). The
//! whole-path option adds the full path.
//!
//! The filter is per-step, and it sees the *current* name: a step earlier in
//! the pipeline can rename a file into or out of a later step's filter.

use serde::{Deserialize, Serialize};

use crate::cache::Cached;
use crate::matcher::{MatchSpec, Matcher};
use crate::model::split_file_name;
use crate::regex_flavor::RegexError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IncludeFilter {
    /// Only files matching this take part.
    pub include: Option<MatchSpec>,
    /// Applied after `include`.
    pub exclude: Option<MatchSpec>,
    /// Also test the full path, not just the name.
    pub whole_path: bool,
    /// Also test the extension, and the whole name with it. The stem is
    /// always tested.
    pub extension: bool,
    pub case_sensitive: bool,

    /// Both matchers, compiled together on first use — this runs once per file
    /// per keystroke.
    ///
    /// Deliberately **private**, which makes struct-update syntax
    /// (`IncludeFilter { case_sensitive: true, ..old }`) a compile error
    /// outside this module. That syntax would move the already-populated cache
    /// onto the new configuration and keep matching by the old rules — a silent
    /// wrong answer. Use the builder methods instead; each returns a value with
    /// a fresh cache.
    #[serde(skip)]
    compiled: Cached<Result<Compiled, RegexError>>,
}

#[derive(Debug)]
struct Compiled {
    include: Option<Matcher>,
    exclude: Option<Matcher>,
}

impl IncludeFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Only files matching `spec` take part.
    pub fn including(mut self, spec: MatchSpec) -> Self {
        self.include = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// Files matching `spec` are left out, applied after the include.
    pub fn excluding(mut self, spec: MatchSpec) -> Self {
        self.exclude = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// Also test the whole path, not just the name.
    pub fn testing_whole_path(mut self, yes: bool) -> Self {
        self.whole_path = yes;
        self
    }

    /// Also test the extension and the whole name. The stem is always tested.
    pub fn testing_extension(mut self, yes: bool) -> Self {
        self.extension = yes;
        self
    }

    pub fn matching_case(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self.compiled = Cached::new();
        self
    }

    pub fn is_identity(&self) -> bool {
        self.include.is_none() && self.exclude.is_none()
    }

    fn compiled(&self) -> Result<&Compiled, RegexError> {
        self.compiled
            .get_or_init(|| {
                // Filter semantics: a plain string is "contains", a wildcard
                // string is a mask over the whole subject. See
                // MatchSpec::compile_for_filter.
                let compile = |spec: &Option<MatchSpec>| -> Result<Option<Matcher>, RegexError> {
                    spec.as_ref()
                        .map(|s| s.compile_for_filter(self.case_sensitive))
                        .transpose()
                };
                Ok(Compiled {
                    include: compile(&self.include)?,
                    exclude: compile(&self.exclude)?,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Does this file take part?
    ///
    /// `path` is the full path; `file_name` is the *current* name, which may
    /// already differ from the path's last component because an earlier step
    /// changed it.
    pub fn accepts(&self, path: &str, file_name: &str) -> Result<bool, RegexError> {
        let compiled = self.compiled()?;
        if let Some(include) = &compiled.include
            && !self.matches_any(include, path, file_name)?
        {
            return Ok(false);
        }
        if let Some(exclude) = &compiled.exclude
            && self.matches_any(exclude, path, file_name)?
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// True if any of the enabled subjects matches.
    fn matches_any(
        &self,
        matcher: &Matcher,
        path: &str,
        file_name: &str,
    ) -> Result<bool, RegexError> {
        let (stem, ext) = split_file_name(file_name);

        // The stem is always tested.
        if matcher.is_match(stem)? {
            return Ok(true);
        }
        if self.extension {
            if let Some(ext) = ext
                && matcher.is_match(ext)?
            {
                return Ok(true);
            }
            // The whole name as well. A wildcard is an anchored mask here
            // (P19), so `*.bak` can match neither `old` nor `bak` on its own —
            // without this subject the classic file mask matched nothing, and
            // an exclude of `*.bak` let every backup through.
            if ext.is_some() && matcher.is_match(file_name)? {
                return Ok(true);
            }
        }
        if self.whole_path && matcher.is_match(path)? {
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(filter: &IncludeFilter, path: &str, name: &str) -> bool {
        filter.accepts(path, name).unwrap()
    }

    #[test]
    fn the_default_filter_accepts_everything() {
        let f = IncludeFilter::default();
        assert!(f.is_identity());
        assert!(accepts(&f, "/music/song.mp3", "song.mp3"));
    }

    #[test]
    fn include_keeps_only_matching_files() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("live".into()));
        assert!(accepts(&f, "/m/live set.mp3", "live set.mp3"));
        assert!(!accepts(&f, "/m/studio.mp3", "studio.mp3"));
    }

    /// The exclude is applied after the include, so it removes files from the
    /// set the include let in.
    #[test]
    fn exclude_runs_after_include() {
        let f = IncludeFilter::new()
            .including(MatchSpec::Substring("track".into()))
            .excluding(MatchSpec::Substring("demo".into()));
        assert!(accepts(&f, "/m/track 1.mp3", "track 1.mp3"));
        assert!(!accepts(&f, "/m/track demo.mp3", "track demo.mp3"));
        assert!(!accepts(&f, "/m/other.mp3", "other.mp3"));
    }

    #[test]
    fn exclude_alone_works_without_an_include() {
        let f = IncludeFilter::new().excluding(MatchSpec::Substring("tmp".into()));
        assert!(accepts(&f, "/a/keep.txt", "keep.txt"));
        assert!(!accepts(&f, "/a/tmp.txt", "tmp.txt"));
    }

    /// The stem is always tested; the extension only when asked.
    #[test]
    fn the_extension_is_only_tested_when_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("mp3".into()));
        assert!(!accepts(&f, "/m/song.mp3", "song.mp3"));

        let f = f.testing_extension(true);
        assert!(accepts(&f, "/m/song.mp3", "song.mp3"));
    }

    #[test]
    fn the_whole_path_is_only_tested_when_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("Bootlegs".into()));
        assert!(!accepts(&f, "/music/Bootlegs/song.mp3", "song.mp3"));

        let f = f.testing_whole_path(true);
        assert!(accepts(&f, "/music/Bootlegs/song.mp3", "song.mp3"));
    }

    /// A wildcard is a mask over the whole subject (P19): `hello*` is only a
    /// sharper tool than the plain string `hello` if it is anchored.
    #[test]
    fn a_wildcard_filter_is_a_mask_over_the_whole_name() {
        let f = IncludeFilter::new().including(MatchSpec::auto("track ?"));
        assert!(accepts(&f, "/m/track 1.mp3", "track 1.mp3"));
        assert!(!accepts(&f, "/m/track 10.mp3", "track 10.mp3"));
        assert!(!accepts(&f, "/m/my track 1.mp3", "my track 1.mp3"));
    }

    #[test]
    fn the_classic_file_mask_works_when_the_extension_is_tested() {
        let f = IncludeFilter::new()
            .including(MatchSpec::auto("*.mp3"))
            .testing_extension(true);
        assert!(accepts(&f, "/m/song.mp3", "song.mp3"));
        assert!(!accepts(&f, "/m/song.flac", "song.flac"));
    }

    /// The exclude box is where a mask across the dot matters most: `*.bak`
    /// with the extension option on must keep the backup out of the run.
    #[test]
    fn an_exclude_mask_across_the_dot_works_when_the_extension_is_tested() {
        let f = IncludeFilter::new()
            .excluding(MatchSpec::auto("*.bak"))
            .testing_extension(true);
        assert!(!accepts(&f, "/a/old.bak", "old.bak"));
        assert!(accepts(&f, "/a/keep.txt", "keep.txt"));
        // The name is the *current* one: a step that already renamed the file
        // is judged by what it is called now.
        assert!(!accepts(&f, "/a/old.txt", "old.txt.bak"));
    }

    /// A regex gets the whole name too, so `\.bak$` means what it says.
    #[test]
    fn a_regex_sees_the_whole_name_when_the_extension_is_tested() {
        let f = IncludeFilter::new()
            .including(MatchSpec::Regex(r"\.mp3$".into()))
            .testing_extension(true);
        assert!(accepts(&f, "/m/song.mp3", "song.mp3"));
        assert!(!accepts(&f, "/m/song.flac", "song.flac"));
    }

    /// With the option off only the stem is tested, so a mask that spans the
    /// dot matches nothing. Pinned, because it is the behaviour the option
    /// exists to change.
    #[test]
    fn without_the_extension_option_a_mask_across_the_dot_matches_nothing() {
        let f = IncludeFilter::new().including(MatchSpec::auto("*.mp3"));
        assert!(!accepts(&f, "/m/song.mp3", "song.mp3"));
        let f = IncludeFilter::new().excluding(MatchSpec::auto("*.bak"));
        assert!(accepts(&f, "/a/old.bak", "old.bak"));
    }

    #[test]
    fn filtering_is_case_insensitive_unless_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("LIVE".into()));
        assert!(accepts(&f, "/m/live.mp3", "live.mp3"));

        // The builder resets the compiled cache, so the new setting takes
        // effect even though the old filter had already matched something.
        let f = f.matching_case(true);
        assert!(!accepts(&f, "/m/live.mp3", "live.mp3"));
    }
}
