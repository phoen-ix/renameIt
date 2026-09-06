//! The `<tag>` template engine.
//!
//! A template is compiled once — when
//! the user stops typing, not once per file — into literals and [`Tag`]s, and
//! then rendered against one [`EvalCx`]. Compiling up front buys two things
//! beyond speed: the editor can report a bad tag while it is being typed (D29),
//! and the [`TagNeeds`] set tells the engine what this template will cost, so a
//! template that never mentions `<Crc32>` never opens a file.
//!
//! Rendering never fails. A tag whose value is not available — a filesystem
//! with no creation time, an `<Ask>` nobody has answered yet — renders as
//! nothing and is reported in [`Rendered::missing`], which is what the Format
//! tab's *"Only rename if all tags are available"* checkbox reads.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cache::Cached;
use crate::model::split_file_name;
use crate::ops::EvalCx;
use crate::run::AskSpec;

pub mod dates;
pub mod lexer;
pub mod tag;

pub use tag::{ImageField, LetterMode, Tag, TagError, TagNeeds, TimeSource};

/// A compiled template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    source: String,
    nodes: Vec<Node>,
    needs: TagNeeds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Literal(String),
    /// The tag, plus the text the user typed, so messages quote them back.
    Tag {
        tag: Tag,
        text: String,
    },
}

/// A tag that would not compile, and where it sat in the source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct TemplateError {
    #[source]
    pub error: TagError,
    /// Byte range of the offending `<…>` — the editor underlines this.
    pub at: std::ops::Range<usize>,
}

/// The result of rendering one template against one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Tags that had no value for this file, in the order they appeared.
    pub missing: Vec<String>,
    /// Whether one of those was a value the *user* still owes us — `<Ask>` or
    /// `<Clipboard>`, collected once per run before evaluation (D28).
    ///
    /// Worth separating from the rest: a filesystem with no creation date will
    /// never have one, but an unanswered `<Ask>` is simply a question not yet
    /// asked, and rendering it as nothing during the preview is how a
    /// `Format: <Ask> <Counter>` preset would rename every file to " 1".
    pub awaiting_input: bool,
}

impl Rendered {
    /// *"Only rename if all tags are available"* reads exactly this.
    pub fn available(&self) -> bool {
        self.missing.is_empty()
    }
}

impl Template {
    /// Compiles template text. The first bad tag wins, since that is the one
    /// the user is looking at.
    pub fn compile(source: &str) -> Result<Self, TemplateError> {
        let mut nodes = Vec::new();
        let mut needs = TagNeeds::NONE;

        for piece in lexer::scan(source) {
            match piece {
                lexer::Piece::Literal(text) => nodes.push(Node::Literal(text.to_owned())),
                lexer::Piece::Tag { body, span } => {
                    let tag = Tag::parse(body).map_err(|error| TemplateError {
                        error,
                        at: span.clone(),
                    })?;
                    needs.insert(tag.needs());
                    nodes.push(Node::Tag {
                        tag,
                        text: source[span].to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            source: source.to_owned(),
            nodes,
            needs,
        })
    }

    /// A template that is nothing but the text it was given.
    pub fn literal(text: impl Into<String>) -> Self {
        let source = text.into();
        let nodes = if source.is_empty() {
            Vec::new()
        } else {
            vec![Node::Literal(source.clone())]
        };
        Self {
            source,
            nodes,
            needs: TagNeeds::NONE,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    pub fn needs(&self) -> TagNeeds {
        self.needs
    }

    /// How many tags it has, so a caller can tell "some were missing" from
    /// "all of them were".
    pub fn tag_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| matches!(node, Node::Tag { .. }))
            .count()
    }

    /// The whole template as fixed text, when it has no tags at all.
    ///
    /// The overwhelmingly common case for a field like Add's *"Insert:"*, and
    /// worth a borrow: rendering runs once per file per keystroke.
    pub fn literal_text(&self) -> Option<&str> {
        match self.nodes.as_slice() {
            [] => Some(""),
            [Node::Literal(text)] => Some(text),
            _ => None,
        }
    }

    /// Every tag this template uses, in order.
    pub fn tags(&self) -> impl Iterator<Item = &Tag> {
        self.nodes.iter().filter_map(|node| match node {
            Node::Tag { tag, .. } => Some(tag),
            Node::Literal(_) => None,
        })
    }

    /// The `<Ask>` slots to collect before the run, deduplicated and in slot
    /// order — the modal shows one field per entry.
    pub fn asks(&self) -> Vec<AskSpec> {
        let mut slots: Vec<u8> = self
            .tags()
            .filter_map(|tag| match tag {
                Tag::Ask(slot) => Some(*slot),
                _ => None,
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots.into_iter().map(|slot| AskSpec { slot }).collect()
    }

    /// Renders against one file.
    pub fn render(&self, cx: &EvalCx<'_>) -> Rendered {
        self.render_with(cx, |value, out| out.push_str(value))
    }

    /// The same, with every **tag** value passed through `on_tag` on its way
    /// into the text. A literal the user typed goes straight through.
    ///
    /// One field needs the distinction. Find & Replace's replacement is the
    /// only place a rendered value re-enters an interpreter — `$1`–`$9` are
    /// capture references there — so a file called `Track $1 mix` rendered
    /// through `<Name>` would have its own text silently eaten. D48 and D61
    /// already state the rule: what the data said is sanitised, what the user
    /// wrote is obeyed. This is the seam that lets one field apply it without
    /// every other field paying for it.
    pub fn render_with(
        &self,
        cx: &EvalCx<'_>,
        mut on_tag: impl FnMut(&str, &mut String),
    ) -> Rendered {
        let mut out = Rendered {
            text: String::with_capacity(self.source.len() + 16),
            missing: Vec::new(),
            awaiting_input: false,
        };
        let mut resolver = Resolver::new(cx);

        for (position, node) in self.nodes.iter().enumerate() {
            match node {
                Node::Literal(text) => out.text.push_str(text),
                Node::Tag { tag, text } => match resolver.resolve(tag, position) {
                    Some(value) => on_tag(&value, &mut out.text),
                    None => {
                        out.awaiting_input |= matches!(tag, Tag::Ask(_) | Tag::Clipboard);
                        out.missing.push(text.clone());
                    }
                },
            }
        }

        resolver.apply_limits(&mut out.text);
        out
    }
}

/// A tag-bearing text field: the raw text plus its compiled form.
///
/// Every field that accepts tags holds one of these. The text is what the user
/// typed and what serialises; the compiled template is a cache that a clone
/// drops (D21), because it is the configuration that identifies the field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextTemplate {
    text: String,
    #[serde(skip)]
    compiled: Cached<Result<Template, TemplateError>>,
}

impl TextTemplate {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            compiled: Cached::new(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Compiles on first use and keeps the result — including the error, so a
    /// broken field does not recompile once per file.
    pub fn compiled(&self) -> Result<&Template, &TemplateError> {
        self.compiled
            .get_or_init(|| Template::compile(&self.text))
            .as_ref()
    }

    pub fn needs(&self) -> TagNeeds {
        self.compiled().map_or(TagNeeds::NONE, Template::needs)
    }

    pub fn tag_count(&self) -> usize {
        self.compiled().map_or(0, Template::tag_count)
    }

    pub fn asks(&self) -> Vec<AskSpec> {
        self.compiled().map(Template::asks).unwrap_or_default()
    }

    /// Renders, or reports why the template does not compile.
    pub fn render(&self, cx: &EvalCx<'_>) -> Result<Rendered, TemplateError> {
        match self.compiled() {
            Ok(template) => Ok(template.render(cx)),
            Err(error) => Err(error.clone()),
        }
    }
}

impl From<&str> for TextTemplate {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for TextTemplate {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl std::fmt::Display for TextTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Per-file rendering state: the lazily computed things, and the length limits
/// collected on the way through.
struct Resolver<'a> {
    cx: &'a EvalCx<'a>,
    parts: Option<crate::parts::Parts>,
    /// Memoised for the same reason `parts` is: `<Artist> - <Title>` is two
    /// resolutions of one file, and the outer cache is behind a mutex.
    /// `Option<Option<…>>` because "not looked up yet" and "looked up, nothing
    /// there" are different answers.
    audio: Option<Option<std::sync::Arc<crate::meta::audio::AudioTags>>>,
    file_max: Option<usize>,
    path_max: Option<usize>,
}

impl<'a> Resolver<'a> {
    fn new(cx: &'a EvalCx<'a>) -> Self {
        Self {
            cx,
            parts: None,
            audio: None,
            file_max: None,
            path_max: None,
        }
    }

    /// The music tags of this file, read at most once per render.
    ///
    /// A folder answers with the tags of the first music file inside it.
    fn audio_tags(&mut self) -> Option<&crate::meta::audio::AudioTags> {
        if self.audio.is_none() {
            let entry = self.cx.entry;
            self.audio = Some(if entry.is_dir {
                crate::meta::audio::folder_tags(&entry.path)
            } else {
                crate::meta::audio::tags_of(&entry.path)
            });
        }
        self.audio.as_ref()?.as_deref()
    }

    fn audio_field(&mut self, field: crate::template::tag::AudioField) -> Option<String> {
        use crate::template::tag::AudioField as F;
        let tags = self.audio_tags()?;
        let properties = tags.properties;
        match field {
            F::Artist => tags.artist.clone(),
            F::Title => tags.title.clone(),
            F::Album => tags.album.clone(),
            F::Year => tags.year.clone(),
            F::Comment => tags.comment.clone(),
            F::Genre => tags.genre.clone(),
            // *"Track Nr., zero-padded"*. Anything lofty handed over that is
            // not a number renders as written rather than being padded to a
            // width it has no digits for.
            F::Track => match tags.track_number() {
                Some(n) => Some(format!("{n:02}")),
                None => tags.track.clone(),
            },
            F::TrackNoPad => tags.track.clone(),
            F::Length => properties.duration.map(|d| long_duration(d.as_secs())),
            F::LengthShort => properties.duration.map(|d| short_duration(d.as_secs())),
            F::Bitrate => properties.bitrate_kbps.map(|b| b.to_string()),
            // *"Stereo or Mono"*.
            F::Stereo => properties
                .channels
                .map(|c| if c > 1 { "Stereo" } else { "Mono" }.to_owned()),
            F::Frequency => properties.sample_rate_hz.map(|r| r.to_string()),
            // kHz, and 44100 is 44 rather than 44.1 because `<FreqS>` is the
            // short form, not a rounded one.
            F::FrequencyShort => properties.sample_rate_hz.map(|r| (r / 1000).to_string()),
        }
    }

    /// The file name as the pipeline has it so far — *"Current Filename"*.
    fn current(&self) -> &str {
        self.cx.current
    }

    fn stem(&self) -> &str {
        split_file_name(self.current()).0
    }

    fn resolve(&mut self, tag: &Tag, position: usize) -> Option<String> {
        match tag {
            Tag::Name => Some(self.stem().to_owned()),
            Tag::Ext => split_file_name(self.current()).1.map(str::to_owned),
            Tag::FullName => Some(self.current().to_owned()),

            Tag::Left(n) => Some(self.stem().chars().take(*n).collect()),
            Tag::Right(n) => {
                let stem = self.stem();
                let skip = stem.chars().count().saturating_sub(*n);
                Some(stem.chars().skip(skip).collect())
            }
            Tag::Mid { from, to } => Some(slice(self.stem(), *from, *to, false)),
            Tag::MidRev { from, to } => Some(slice(self.stem(), *from, *to, true)),

            Tag::FirstLetter(mode) => first_letter(self.stem(), *mode),

            Tag::Parent(n) => parent_name(&self.cx.entry.path, *n),

            Tag::Size => Some(auto_size(self.cx.entry.size)),
            Tag::SizeBytes { pad } => Some(zero_pad(&self.cx.entry.size.to_string(), *pad)),

            Tag::Stamp { source, format } => self.timestamp(*source).map(|at| format.render(at)),

            Tag::Counter => Some(self.cx.counter_text()),
            Tag::NumFiles => Some(self.cx.run.num_files.to_string()),
            Tag::RandomLetter => {
                let value = self.random(position);
                Some(char::from(b'A' + (value % 26) as u8).to_string())
            }
            Tag::RandomNumber { digits, range, pad } => {
                let value = self.random(position);
                Some(random_number(value, *digits, *range, *pad))
            }

            Tag::Ask(slot) => self.cx.run.answers.ask(*slot).map(str::to_owned),
            Tag::Clipboard => self.cx.run.answers.clipboard.clone(),

            Tag::Crc32 => crc32(&self.cx.entry.path),
            Tag::DetectedExt => detected_extension(&self.cx.entry.path),

            Tag::Audio(field) => self.audio_field(*field).map(|v| safe(&v)),
            Tag::Id3(name) => self
                .audio_tags()
                .and_then(|tags| tags.extended(name).map(safe)),
            Tag::Exif(name) => {
                // A folder peeks inside, exactly as `<ExifDate>` does — the
                // "This also works on folders" is written about the
                // date, but nothing in it is specific to the date, and a folder
                // that answers one Exif tag and not another is inexplicable.
                let entry = self.cx.entry;
                if entry.is_dir {
                    crate::meta::exif::folder_field(&entry.path, name).map(|v| safe(&v))
                } else {
                    crate::meta::exif::field_of(&entry.path, name).map(|v| safe(&v))
                }
            }

            Tag::HtmlTitle => {
                // A folder has no title, and no peek to make one out of: the
                // "first file inside" rule (P51) is for a folder that *is* an
                // album or a shoot, and a folder of saved pages is not one
                // page. `<FirstFileInFolder>` is the tag for that question.
                let entry = self.cx.entry;
                if entry.is_dir {
                    return None;
                }
                crate::meta::html::title_of(&entry.path).map(|title| safe(&title))
            }

            Tag::Folder {
                field,
                recursive,
                pad,
            } => {
                // A folder row asks about itself; a file row asks about the
                // folder it is in. Both are what the tag name says, and for a
                // file there is nothing else `<DirFiles>` could usefully mean.
                let entry = self.cx.entry;
                let dir = if entry.is_dir {
                    entry.path.as_path()
                } else {
                    entry.path.parent()?
                };

                use crate::template::tag::FolderField;
                match field {
                    FolderField::FirstFile => crate::meta::folder::first_file(dir, false),
                    FolderField::FirstFileWithExtension => {
                        crate::meta::folder::first_file(dir, true)
                    }
                    _ => {
                        let stats = crate::meta::folder::stats(dir, *recursive)?;
                        Some(match field {
                            FolderField::Size => auto_size(stats.bytes),
                            FolderField::SizeBytes => zero_pad(&stats.bytes.to_string(), *pad),
                            FolderField::Files => stats.files.to_string(),
                            FolderField::Dirs => stats.dirs.to_string(),
                            FolderField::FirstFile | FolderField::FirstFileWithExtension => {
                                unreachable!()
                            }
                        })
                    }
                }
                // A file name from the folder carries text the user did not
                // type, so it goes through `safe` like every other such tag.
                .map(|value| safe(&value))
            }

            Tag::Image(field) => {
                // Same folder rule (P51). `<Width>` on a folder is as
                // meaningful as `<ExifDate>` on one, and for the same reason.
                let entry = self.cx.entry;
                let info = if entry.is_dir {
                    crate::meta::image::folder_info(&entry.path)
                } else {
                    crate::meta::image::info_of(&entry.path)
                }?;
                Some(match field {
                    ImageField::Width => info.width.to_string(),
                    ImageField::Height => info.height.to_string(),
                    ImageField::Colours => info.colours().to_string(),
                    ImageField::Bits => info.bits.to_string(),
                    // The one that carries text a stranger wrote, so the one
                    // that goes through `safe` (D61).
                    ImageField::JpegComment => safe(info.comment.as_deref()?),
                })
            }

            Tag::Part(slot) => {
                if self.parts.is_none() {
                    self.parts = Some(self.cx.run.parts.split(self.stem()));
                }
                self.parts
                    .as_ref()
                    .and_then(|parts| parts.get(*slot))
                    .map(str::to_owned)
            }

            // D31: the one tag that renders a path separator.
            Tag::SubFolder => Some(crate::plan::SUBFOLDER_SEPARATOR.to_string()),

            Tag::FileMax(n) => {
                self.file_max = Some(self.file_max.map_or(*n, |old| old.min(*n)));
                Some(String::new())
            }
            Tag::PathMax(n) => {
                self.path_max = Some(self.path_max.map_or(*n, |old| old.min(*n)));
                Some(String::new())
            }
        }
    }

    fn timestamp(&self, source: TimeSource) -> Option<std::time::SystemTime> {
        match source {
            TimeSource::Modified => self.cx.entry.modified,
            TimeSource::Created => self.cx.entry.created,
            TimeSource::Accessed => self.cx.entry.accessed,
            TimeSource::Now => Some(self.cx.run.now),
            // The only source that opens the file — cached, and on a folder it
            // peeks inside for the first image, exactly as Set Date does.
            TimeSource::Exif => {
                let entry = self.cx.entry;
                let found = if entry.is_dir {
                    crate::meta::exif::folder_date(&entry.path)
                } else {
                    crate::meta::exif::date_of(&entry.path)
                }?;
                // Exif is a wall clock with no zone, and `DateFormat::render`
                // takes an instant — so it goes back through the same local
                // reading `<Date>` uses, or the two would disagree by an offset.
                crate::datetime::localise(&chrono::Local, found)
                    .ok()
                    .and_then(|stamp| stamp.to_system())
            }
        }
    }

    /// A random value that is stable for (seed, file, tag position), so the
    /// preview and the rename that follows it agree (P16).
    fn random(&self, position: usize) -> u64 {
        let mut key = self.cx.run.seed;
        key = mix(key ^ 0x9e37_79b9_7f4a_7c15u64.wrapping_mul(self.cx.index as u64 + 1));
        key = mix(key ^ (position as u64).wrapping_add(0x6a09_e667_f3bc_c908));
        key
    }

    /// *"Limit filename length to # chars"* / *"…path and filename…"*.
    ///
    /// Applied to the text this template produced, once, at the end — which is
    /// the only point at which the length is known.
    fn apply_limits(&self, text: &mut String) {
        if let Some(max) = self.file_max {
            truncate_chars(text, max);
        }
        if let Some(max) = self.path_max {
            // The separator between the folder and the name counts too.
            let prefix = self.cx.entry.parent().to_string_lossy().chars().count() + 1;
            truncate_chars(text, max.saturating_sub(prefix));
        }
    }
}

/// SplitMix64 — three lines, no dependency, and good enough for filenames.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn random_number(value: u64, digits: usize, range: Option<(i64, i64)>, pad: bool) -> String {
    match range {
        Some((low, high)) => {
            // Via i128 so a range spanning the whole of i64 cannot overflow.
            let width = ((high as i128) - (low as i128) + 1).clamp(1, u64::MAX as i128) as u64;
            let picked = ((low as i128) + (value % width) as i128)
                .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
            let text = picked.to_string();
            if pad { zero_pad(&text, digits) } else { text }
        }
        None => {
            // `<Rnd3>` is three random digits.
            let limit = 10u64.saturating_pow(digits.min(19) as u32);
            zero_pad(&(value % limit).to_string(), digits)
        }
    }
}

fn zero_pad(text: &str, width: usize) -> String {
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let padding = width.saturating_sub(digits.len());
    let mut out = String::with_capacity(sign.len() + padding + digits.len());
    out.push_str(sign);
    for _ in 0..padding {
        out.push('0');
    }
    out.push_str(digits);
    out
}

/// `<Mid-#>` and `<MidRev-#>`, sharing P15's position rules: zero based,
/// saturating, counting characters. `MidRev` counts from the end, so
/// `<MidRev-3-0>` is the last three characters.
fn slice(text: &str, from: usize, to: Option<usize>, backwards: bool) -> String {
    let len = text.chars().count();
    let forward = |pos: usize| {
        if backwards {
            len.saturating_sub(pos)
        } else {
            pos.min(len)
        }
    };
    let (start, end) = match to {
        Some(to) => {
            let (a, b) = (forward(from), forward(to));
            (a.min(b), a.max(b))
        }
        None => (forward(from), len),
    };
    text.chars().skip(start).take(end - start).collect()
}

/// The three `<FLetter…>` variants.
///
/// Only the labels are given — *"First letter in name"*, *"incl.
/// numbers"*, *"incl. numbers subst."* — so this is our reading (P28): the
/// plain form yields nothing for a name that does not start with a letter,
/// `N1` accepts a digit as itself, and `N2` folds every digit onto `#`, which
/// is the convention every music library uses for its numeric shelf.
fn first_letter(stem: &str, mode: LetterMode) -> Option<String> {
    let first = stem.chars().next()?;
    let upper = first.to_uppercase().to_string();
    match mode {
        LetterMode::Letters => first.is_alphabetic().then_some(upper),
        LetterMode::WithNumbers => first.is_alphanumeric().then_some(upper),
        LetterMode::NumbersSubstituted => {
            if first.is_numeric() {
                Some("#".to_owned())
            } else {
                first.is_alphabetic().then_some(upper)
            }
        }
    }
}

/// *"N:th parent folder name (set # to 1-9)"*, where 1 is the folder the file
/// is in.
fn parent_name(path: &Path, level: usize) -> Option<String> {
    let mut at = path.parent()?;
    for _ in 1..level {
        at = at.parent()?;
    }
    at.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// *"File size (Auto)"* — the largest binary unit that keeps the number at or
/// above 1, with one decimal (P29). Fixed rather than locale-dependent, for the
/// same reason dates are (D30).
fn auto_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[0])
    } else {
        let rounded = (value * 10.0).round() / 10.0;
        if (rounded.fract()).abs() < f64::EPSILON {
            format!("{}{}", rounded as u64, UNITS[unit])
        } else {
            format!("{rounded:.1}{}", UNITS[unit])
        }
    }
}

/// A metadata value, made safe to put in a filename.
///
/// **This is the guard for the sharpest bug M6 could have shipped.** D31 makes
/// `/` in a produced name a *subfolder move*, and `plan.rs` splits on it
/// unconditionally — so `<Artist>` on an AC/DC track would produce
/// `AC/DC - Highway to Hell.mp3`, silently create a folder called `AC`, and put
/// the file inside it. No conflict, no warning, files scattered across new
/// directories. Exif rationals (`1/125`, `f/2.8`) take the same path, as do
/// `A/B` titles and any track written `3/12`.
///
/// Nothing guarded it before because every value the engine had ever rendered
/// came from the filename or the clock, and neither can contain a separator
/// that the filesystem did not already accept.
///
/// The same reasoning D48 applied to a CSV's new-name column, and the same
/// carve-out: this runs on values that *came out of a file*, never on what the
/// user typed, so `<\>` still moves a file into a subfolder. A user asking for
/// a move gets one; a tag containing a slash does not.
fn safe(value: &str) -> String {
    value.replace(['/', '\\'], "-")
}

/// `<Length>` — *"Long"*.
fn long_duration(seconds: u64) -> String {
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

/// `<LengthS>` — *"Short"*.
fn short_duration(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// `<Crc32>` — eight uppercase hex digits, as VB6's `Hex$` produced.
fn crc32(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
    Some(format!("{:08X}", hasher.finalize()))
}

/// `<DetectedExt>` — the extension the file's *content* says it should have.
fn detected_extension(path: &Path) -> Option<String> {
    infer::get_from_path(path)
        .ok()
        .flatten()
        .map(|kind| kind.extension().to_owned())
}

/// Truncates from the end, counting characters.
fn truncate_chars(text: &mut String, max: usize) {
    if let Some((at, _)) = text.char_indices().nth(max) {
        text.truncate(at);
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    use crate::meta::testing::Mp3;
    use crate::model::FileEntry;
    use crate::ops::EvalCx;
    use tempfile::TempDir;

    /// **The sharpest bug M6 could have shipped.**
    ///
    /// D31 makes `/` in a produced name a subfolder move, and the planner
    /// splits on it unconditionally. Without a guard, `<Artist> - <Title>` on
    /// an AC/DC track produces `AC/DC - Highway to Hell.mp3`, which silently
    /// creates a folder called `AC` and puts the file inside it — no conflict,
    /// no warning, a library scattered across directories that were never asked
    /// for. Nothing guarded it before because every value the engine had ever
    /// rendered came from the filename or the clock.
    #[test]
    fn a_slash_in_a_tag_value_never_becomes_a_subfolder() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("AC/DC", "Highway to Hell").write(dir.path(), "track.mp3");
        crate::meta::audio::forget_all();

        let entry = FileEntry::synthetic(&path);
        let cx = EvalCx::simple(&entry, 0, 1);
        let rendered = Template::compile("<Artist> - <Title>").unwrap().render(&cx);

        assert!(
            !rendered.text.contains(crate::plan::SUBFOLDER_SEPARATOR),
            "{} would be read as a path",
            rendered.text
        );
        assert_eq!(rendered.text, "AC-DC - Highway to Hell");
    }

    /// A backslash too: it is legal in a POSIX name, and the naming rules
    /// reject it on Windows — so leaving it in produces a file that renames on
    /// one platform and fails on the other.
    #[test]
    fn a_backslash_in_a_tag_value_is_neutralised_too() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("AC\\DC", "T").write(dir.path(), "t.mp3");
        crate::meta::audio::forget_all();
        let entry = FileEntry::synthetic(&path);
        let cx = EvalCx::simple(&entry, 0, 1);
        let rendered = Template::compile("<Artist>").unwrap().render(&cx);
        assert_eq!(rendered.text, "AC-DC");
    }

    /// The carve-out, and it is the whole reason the guard is on *values*
    /// rather than on the finished name: a user who asks for a move gets one.
    /// D48 drew the same line for a CSV's new-name column.
    #[test]
    fn the_subfolder_tag_still_moves_a_file() {
        let dir = TempDir::new().unwrap();
        let path = Mp3::tagged("AC/DC", "Highway").write(dir.path(), "t.mp3");
        crate::meta::audio::forget_all();
        let entry = FileEntry::synthetic(&path);
        let cx = EvalCx::simple(&entry, 0, 1);
        let rendered = Template::compile("<Artist><\\><Title>")
            .unwrap()
            .render(&cx);

        assert_eq!(
            rendered.text, "AC-DC/Highway",
            "the tag the user typed still separates; the one the file supplied does not"
        );
    }

    /// `<Track>` pads to two digits, `<TrackN>` does not, and a track number
    /// that is not a number is left as it is written rather than padded to a
    /// width it has no digits for.
    #[test]
    fn track_padding_follows_the_manual() {
        let dir = TempDir::new().unwrap();
        // A non-numeric track does not survive lofty's TRCK parse, so both
        // tags are *missing* for it — the file is left alone rather than
        // renamed with a number nobody wrote.
        for (raw, padded, bare) in [("7", "07", "7"), ("12", "12", "12"), ("A1", "", "")] {
            let path = Mp3::tagged("A", "B")
                .frame("TRCK", raw)
                .write(dir.path(), "t.mp3");
            crate::meta::audio::forget_all();
            let entry = FileEntry::synthetic(&path);
            let cx = EvalCx::simple(&entry, 0, 1);
            assert_eq!(
                Template::compile("<Track>").unwrap().render(&cx).text,
                padded,
                "<Track> for {raw}"
            );
            assert_eq!(
                Template::compile("<TrackN>").unwrap().render(&cx).text,
                bare,
                "<TrackN> for {raw}"
            );
        }
    }

    /// A tag the file cannot answer is *missing*, which is what lets
    /// "only rename if all tags are available" leave the file alone (P32) —
    /// rather than rendering nothing and colliding every untagged track into
    /// one name.
    #[test]
    fn a_tag_an_untagged_file_cannot_answer_is_missing_not_empty() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x".repeat(64)).unwrap();
        let entry = FileEntry::synthetic(dir.path().join("notes.txt"));
        let cx = EvalCx::simple(&entry, 0, 1);
        let rendered = Template::compile("<Artist> - <Title>").unwrap().render(&cx);

        assert!(!rendered.available(), "both tags are missing");
        assert_eq!(rendered.missing, ["<Artist>", "<Title>"]);
    }

    /// Reading is allowed from the parallel pass, but only for a template that
    /// asks for it — the bit is what stops a rename with no music tags in it
    /// opening ten thousand files.
    #[test]
    fn a_template_with_music_tags_declares_that_it_opens_files() {
        let plain = Template::compile("<Name>-<Counter>").unwrap();
        assert!(!plain.needs().contains(TagNeeds::FILE_CONTENT));

        for source in ["<Artist>", "<ID3-Composer>", "<Exif-ImageWidth>"] {
            assert!(
                Template::compile(source)
                    .unwrap()
                    .needs()
                    .contains(TagNeeds::FILE_CONTENT),
                "{source}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter::CounterSetup;
    use crate::model::FileEntry;
    use crate::parts::PartsSpec;
    use crate::run::{Answers, RunContext, RunSettings};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// 1977-05-09 10:18:05 UTC, the worked example timestamp.
    fn stamp() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(231_934_685)
    }

    fn entry(path: &str) -> FileEntry {
        let mut e = FileEntry::synthetic(PathBuf::from(path));
        e.size = 1536;
        e.modified = Some(stamp());
        e
    }

    fn render(source: &str, entry: &FileEntry) -> Rendered {
        let run = RunContext::default();
        Template::compile(source)
            .unwrap_or_else(|e| panic!("{source} should compile: {e}"))
            .render(&EvalCx::new(entry, 0, 1, &run))
    }

    fn text(source: &str, name: &str) -> String {
        render(source, &entry(&format!("/music/rock/{name}"))).text
    }

    #[test]
    fn literals_pass_through_untouched() {
        assert_eq!(text("hello", "a.txt"), "hello");
        assert_eq!(text("", "a.txt"), "");
    }

    /// "<Name> - Current Filename", "<Ext> - Current Extension",
    /// "<FullName> - Current Full Filename"
    #[test]
    fn the_three_name_tags_render_the_documented_pieces() {
        assert_eq!(text("<Name>", "song.mp3"), "song");
        assert_eq!(text("<Ext>", "song.mp3"), "mp3");
        assert_eq!(text("<FullName>", "song.mp3"), "song.mp3");
    }

    /// the Free Format worked example: "<PARENT>_<FULLNAME>".
    #[test]
    fn the_free_format_example_renders() {
        assert_eq!(text("<PARENT>_<FULLNAME>", "0001.jpg"), "rock_0001.jpg");
    }

    #[test]
    fn a_file_with_no_extension_reports_the_ext_tag_as_missing() {
        let out = render("<Ext>", &entry("/music/rock/README"));
        assert_eq!(out.text, "");
        assert_eq!(out.missing, ["<Ext>"]);
        assert!(!out.available());
    }

    #[test]
    fn substring_tags_count_characters_and_saturate() {
        assert_eq!(text("<Left-3>", "abcdef.txt"), "abc");
        assert_eq!(text("<Left-99>", "abcdef.txt"), "abcdef");
        assert_eq!(text("<Right-2>", "abcdef.txt"), "ef");
        assert_eq!(text("<Right-99>", "abcdef.txt"), "abcdef");
        assert_eq!(text("<Left-2>", "ÜÖÄ.txt"), "ÜÖ");
    }

    /// P15's zero-based positions, applied to the tags: "<Mid-#> - Chars from
    /// pos # to end".
    #[test]
    fn mid_runs_from_a_position_to_the_end_or_to_another_position() {
        assert_eq!(text("<Mid-2>", "abcdef.txt"), "cdef");
        assert_eq!(text("<Mid-0-3>", "abcdef.txt"), "abc", "same as <Left-3>");
        assert_eq!(text("<Mid-2-4>", "abcdef.txt"), "cd");
        assert_eq!(text("<Mid-99>", "abcdef.txt"), "");
    }

    /// "counting backwards" — position 0 is the very end, as everywhere else in
    /// the app.
    #[test]
    fn midrev_counts_from_the_end() {
        assert_eq!(text("<MidRev-3>", "abcdef.txt"), "def");
        assert_eq!(text("<MidRev-3-0>", "abcdef.txt"), "def");
        assert_eq!(text("<MidRev-5-2>", "abcdef.txt"), "bcd");
        assert_eq!(text("<MidRev-2-5>", "abcdef.txt"), "bcd", "order-free");
        assert_eq!(text("<MidRev-99>", "abcdef.txt"), "abcdef");
    }

    /// P28: our reading of the three first-letter tags.
    #[test]
    fn the_first_letter_variants_differ_on_digits() {
        assert_eq!(text("<FLetter>", "abc.txt"), "A");
        assert_eq!(text("<FLetterN1>", "abc.txt"), "A");
        assert_eq!(text("<FLetterN2>", "abc.txt"), "A");

        let out = render("<FLetter>", &entry("/music/rock/7abc.txt"));
        assert_eq!(out.missing, ["<FLetter>"], "a digit is not a letter");
        assert_eq!(text("<FLetterN1>", "7abc.txt"), "7");
        assert_eq!(text("<FLetterN2>", "7abc.txt"), "#");
    }

    /// "N:th parent folder name (set # to 1-9)"
    #[test]
    fn parent_walks_up_the_path() {
        let e = entry("/music/rock/live/song.mp3");
        assert_eq!(render("<Parent>", &e).text, "live");
        assert_eq!(render("<Parent-1>", &e).text, "live");
        assert_eq!(render("<Parent-2>", &e).text, "rock");
        assert_eq!(render("<Parent-3>", &e).text, "music");
        assert!(!render("<Parent-9>", &e).available());
    }

    #[test]
    fn size_renders_in_bytes_or_in_units() {
        assert_eq!(text("<SizeB>", "a.txt"), "1536");
        assert_eq!(text("<SizeB-8>", "a.txt"), "00001536");
        assert_eq!(text("<Size>", "a.txt"), "1.5KB");
        assert_eq!(auto_size(0), "0B");
        assert_eq!(auto_size(999), "999B");
        assert_eq!(auto_size(2048), "2KB");
        assert_eq!(auto_size(1024 * 1024 * 3), "3MB");
    }

    #[test]
    fn date_tags_render_the_files_timestamps() {
        let e = entry("/music/rock/song.mp3");
        assert_eq!(
            render("<Date>", &e).text,
            dates::DateFormat::parse(dates::DEFAULT_DATE).render(stamp())
        );
        // Nothing set the created time, so it is unavailable rather than wrong.
        assert_eq!(render("<CDate>", &e).missing, ["<CDate>"]);
    }

    #[test]
    fn now_comes_from_the_run_not_from_the_file() {
        let e = entry("/music/rock/song.mp3");
        let run = RunContext::default();
        let out = Template::compile("<NowDate>")
            .unwrap()
            .render(&EvalCx::new(&e, 0, 1, &run));
        assert_eq!(
            out.text,
            dates::DateFormat::parse(dates::DEFAULT_DATE).render(run.now)
        );
    }

    #[test]
    fn the_counter_comes_from_the_run_context() {
        let entries: Vec<_> = ["/a/1.txt", "/a/2.txt", "/a/3.txt"]
            .iter()
            .map(FileEntry::synthetic)
            .collect();
        let settings = RunSettings {
            counter: CounterSetup {
                start: 8,
                ..Default::default()
            },
            ..Default::default()
        };
        let run = RunContext::build(&entries, &settings, Answers::default());
        let template = Template::compile("<Counter>-<NumFiles>").unwrap();

        // Auto padding widens to the longest value in the sequence: 8, 9, 10.
        let out = template.render(&EvalCx::new(&entries[0], 0, 3, &run));
        assert_eq!(out.text, "08-3");
        let out = template.render(&EvalCx::new(&entries[2], 2, 3, &run));
        assert_eq!(out.text, "10-3");
    }

    #[test]
    fn ask_and_clipboard_come_from_the_pre_run_answers() {
        let e = entry("/music/rock/song.mp3");
        let template = Template::compile("<Ask>-<Ask-2>-<Clipboard>").unwrap();

        let empty = RunContext::default();
        let out = template.render(&EvalCx::new(&e, 0, 1, &empty));
        assert_eq!(out.text, "--");
        assert_eq!(out.missing, ["<Ask>", "<Ask-2>", "<Clipboard>"]);

        let mut answers = Answers::default();
        answers.asks.insert(0, "first".into());
        answers.asks.insert(2, "second".into());
        answers.clipboard = Some("pasted".into());
        let run = RunContext::build(&[], &RunSettings::default(), answers);
        let out = template.render(&EvalCx::new(&e, 0, 1, &run));
        assert_eq!(out.text, "first-second-pasted");
        assert!(out.available());
    }

    #[test]
    fn the_ask_slots_are_collected_for_the_modal() {
        let template = Template::compile("<Ask-3><Ask><Ask-3>x<Ask-1>").unwrap();
        let slots: Vec<u8> = template.asks().into_iter().map(|spec| spec.slot).collect();
        assert_eq!(slots, [0, 1, 3]);
        assert!(Template::compile("<Name>").unwrap().asks().is_empty());
    }

    /// the worked Parts example, end to end through the template.
    #[test]
    fn the_parts_example_rearranges_the_name() {
        let e = FileEntry::synthetic("/music/01. Metallica (S&M) Nothing Else Matters.mp3");
        let settings = RunSettings {
            parts: PartsSpec::new("<%1>. <%2> (<%3>) <%4>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&e), &settings, Answers::default());
        let out = Template::compile("<%2> - <%1> - <%4>")
            .unwrap()
            .render(&EvalCx::new(&e, 0, 1, &run));
        assert_eq!(out.text, "Metallica - 01 - Nothing Else Matters");
    }

    #[test]
    fn a_part_the_pattern_cannot_supply_is_missing() {
        let e = FileEntry::synthetic("/music/no separators here.mp3");
        let settings = RunSettings {
            parts: PartsSpec::new("<%1>. <%2>"),
            ..Default::default()
        };
        let run = RunContext::build(std::slice::from_ref(&e), &settings, Answers::default());
        let out = Template::compile("<%2>")
            .unwrap()
            .render(&EvalCx::new(&e, 0, 1, &run));
        assert_eq!(out.missing, ["<%2>"]);
    }

    #[test]
    fn random_tags_are_stable_for_a_seed_and_vary_between_files() {
        let entries: Vec<_> = (0..8)
            .map(|i| FileEntry::synthetic(format!("/a/{i}.txt")))
            .collect();
        let settings = RunSettings {
            seed: 12345,
            ..Default::default()
        };
        let run = RunContext::build(&entries, &settings, Answers::default());
        let template = Template::compile("<Rnd><Rnd3>").unwrap();

        let once: Vec<String> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                template
                    .render(&EvalCx::new(e, i, entries.len(), &run))
                    .text
            })
            .collect();
        let twice: Vec<String> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                template
                    .render(&EvalCx::new(e, i, entries.len(), &run))
                    .text
            })
            .collect();
        assert_eq!(once, twice, "the same seed must give the same names");

        let distinct: std::collections::HashSet<_> = once.iter().collect();
        assert!(distinct.len() > 1, "every file got {once:?}");
        for value in &once {
            assert_eq!(value.chars().count(), 4, "{value}");
            assert!(
                value.chars().next().unwrap().is_ascii_uppercase(),
                "{value}"
            );
        }
    }

    #[test]
    fn a_random_range_stays_inside_its_bounds() {
        let entries: Vec<_> = (0..50)
            .map(|i| FileEntry::synthetic(format!("/a/{i}.txt")))
            .collect();
        let run = RunContext::build(&entries, &RunSettings::default(), Answers::default());
        let template = Template::compile("<RndZ3-10-20>").unwrap();
        for (i, e) in entries.iter().enumerate() {
            let text = template
                .render(&EvalCx::new(e, i, entries.len(), &run))
                .text;
            assert_eq!(text.len(), 3, "{text} should be zero padded");
            let value: i64 = text.parse().unwrap();
            assert!((10..=20).contains(&value), "{value}");
        }
    }

    /// "<FileMax-#> - Limit filename length to # chars"
    #[test]
    fn the_length_limits_truncate_what_the_template_produced() {
        assert_eq!(text("<Name><FileMax-3>", "abcdef.txt"), "abc");
        assert_eq!(text("<FileMax-3><Name>", "abcdef.txt"), "abc");
        assert_eq!(text("<Name><FileMax-99>", "abcdef.txt"), "abcdef");
        // The tightest limit wins.
        assert_eq!(text("<Name><FileMax-4><FileMax-2>", "abcdef.txt"), "ab");
    }

    #[test]
    fn pathmax_counts_the_folder_the_file_is_in() {
        // "/music/rock" is 11 characters, plus the separator.
        assert_eq!(text("<Name><PathMax-15>", "abcdef.txt"), "abc");
        assert_eq!(text("<Name><PathMax-4>", "abcdef.txt"), "");
    }

    /// D31: `<\>` is how a rename becomes a move.
    #[test]
    fn the_subfolder_tag_renders_a_separator() {
        assert_eq!(text("<Left-1><\\><Name>", "abcdef.txt"), "a/abcdef");
        assert!(
            Template::compile("<\\>")
                .unwrap()
                .needs()
                .contains(TagNeeds::SUBFOLDER)
        );
    }

    #[test]
    fn crc32_and_detected_ext_read_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"123456789").unwrap();
        let e = FileEntry::from_path(&path).unwrap();

        // The CRC-32 of "123456789" is the standard check value.
        assert_eq!(render("<Crc32>", &e).text, "CBF43926");

        let png = dir.path().join("image.dat");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        let e = FileEntry::from_path(&png).unwrap();
        assert_eq!(render("<DetectedExt>", &e).text, "png");

        let e = FileEntry::synthetic("/nowhere/at/all.bin");
        assert!(!render("<Crc32>", &e).available());
        assert!(!render("<DetectedExt>", &e).available());
    }

    #[test]
    fn a_template_declares_what_it_will_cost() {
        let plain = Template::compile("<Name>-<Parent>").unwrap();
        assert!(plain.needs().is_empty());

        let expensive = Template::compile("<Name>-<Crc32>-<Counter>").unwrap();
        assert!(expensive.needs().contains(TagNeeds::FILE_CONTENT));
        assert!(expensive.needs().contains(TagNeeds::COUNTER));
        assert!(!expensive.needs().contains(TagNeeds::ASK));
    }

    /// D29: a typo stops the rename, and says where.
    #[test]
    fn an_unknown_tag_fails_to_compile_and_points_at_itself() {
        let err = Template::compile("prefix <Nmae> suffix").unwrap_err();
        assert_eq!(err.at, 7..13);
        assert_eq!(&"prefix <Nmae> suffix"[err.at.clone()], "<Nmae>");
        assert!(err.to_string().contains("<Nmae>"), "{err}");
    }

    #[test]
    fn a_text_template_compiles_once_and_keeps_its_error() {
        let field = TextTemplate::new("<Name>.bak");
        assert_eq!(field.as_str(), "<Name>.bak");
        let e = entry("/music/rock/song.mp3");
        let run = RunContext::default();
        let cx = EvalCx::new(&e, 0, 1, &run);
        assert_eq!(field.render(&cx).unwrap().text, "song.bak");

        let broken = TextTemplate::new("<Nmae>");
        assert!(broken.render(&cx).is_err());
        assert!(broken.render(&cx).is_err(), "the error is cached, not lost");
    }

    #[test]
    fn a_text_template_serialises_as_the_text_that_was_typed() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            insert: TextTemplate,
        }
        let text = toml::to_string(&Wrapper {
            insert: TextTemplate::new("<Counter>. <Name>"),
        })
        .unwrap();
        assert!(text.contains(r#"insert = "<Counter>. <Name>""#), "{text}");
        let back: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(back.insert.as_str(), "<Counter>. <Name>");
    }

    /// Tags read the name as the pipeline has it, not as it was on disk —
    /// "<Name> - Current Filename".
    #[test]
    fn tags_see_the_name_the_earlier_steps_produced() {
        let e = entry("/music/rock/original.mp3");
        let run = RunContext::default();
        let cx = EvalCx::new(&e, 0, 1, &run).with_current("changed.mp3");
        let out = Template::compile("<Name>|<FullName>").unwrap().render(&cx);
        assert_eq!(out.text, "changed|changed.mp3");
    }
}
