//! The tags themselves: what they are called, and what each one needs.
//!
//! The Format Tag Reference, plus the
//! tag-menu strings recovered from the binary, which spell several of them more
//! precisely (`<PathMax-260>`, not `<PathMax-260#>`).
//!
//! **D29: an unrecognised tag is a compile error.** Silently rendering nothing
//! is how a whole folder ends up named `.mp3` across a batch. Here a typo stops
//! the rename and the editor says which tag it was —
//! and a tag from a family we have not built yet says *that*, rather than
//! pretending never to have heard of it.

use std::fmt;

use super::dates::{DEFAULT_DATE, DEFAULT_TIME, DateFormat};

/// Which timestamp a date tag reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    Modified,
    Created,
    Accessed,
    /// *"Time at start of rename"* — one instant for the whole run.
    Now,
    /// The date the photograph was taken.
    ///
    /// A fifth source rather than a sixth `Tag` variant, so `<ExifDate-yyyy>`
    /// gets the whole VB6 date mini-language for nothing — which is what the
    /// manual promises: *"`<ExifDate-fmt>`"* sits in the same list as
    /// `<Date-fmt>` and `<CDate-fmt>`.
    Exif,
}

impl TimeSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Created => "created",
            Self::Accessed => "accessed",
            Self::Now => "now",
            Self::Exif => "Exif",
        }
    }
}

/// Which of a music file's named tags a tag reads.
///
/// One enum rather than eight `Tag` variants, for the reason `Stamp` collapses
/// the eight date spellings into one: the resolver then has a single arm, and
/// adding the ninth thing a music file can say is a row rather than a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioField {
    Artist,
    Title,
    Album,
    Year,
    Comment,
    Genre,
    /// `<Track>` — *"Track Nr., zero-padded"*.
    Track,
    /// `<TrackN>` — *"Track Nr., no zero padding"*.
    TrackNoPad,
    /// `<Length>` — *"Long"*, `h:mm:ss`.
    Length,
    /// `<LengthS>` — *"Short"*, `m:ss`.
    LengthShort,
    Bitrate,
    /// `<Stereo>` — *"Stereo or Mono"*, from the channel count.
    Stereo,
    /// `<Freq>` — Hz.
    Frequency,
    /// `<FreqS>` — kHz.
    FrequencyShort,
}

impl AudioField {
    /// The canonical spelling, which is what the tag menu inserts.
    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Title => "Title",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Comment => "Comment",
            Self::Genre => "Genre",
            Self::Track => "Track",
            Self::TrackNoPad => "TrackN",
            Self::Length => "Length",
            Self::LengthShort => "LengthS",
            Self::Bitrate => "Bitrate",
            Self::Stereo => "Stereo",
            Self::Frequency => "Freq",
            Self::FrequencyShort => "FreqS",
        }
    }

    pub const ALL: [Self; 14] = [
        Self::Artist,
        Self::Title,
        Self::Album,
        Self::Year,
        Self::Comment,
        Self::Genre,
        Self::Track,
        Self::TrackNoPad,
        Self::Length,
        Self::LengthShort,
        Self::Bitrate,
        Self::Stereo,
        Self::Frequency,
        Self::FrequencyShort,
    ];

    fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|field| field.label().eq_ignore_ascii_case(key))
    }
}

/// What `<FLetter>` and its two variants count as a first letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetterMode {
    /// `<FLetter>` — letters only.
    Letters,
    /// `<FLetterN1>` — *"First letter incl. numbers"*.
    WithNumbers,
    /// `<FLetterN2>` — *"First letter incl. numbers subst."*
    NumbersSubstituted,
}

/// One resolved tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Name,
    Ext,
    FullName,
    /// *"Left # characters in filename"*.
    Left(usize),
    Right(usize),
    /// *"Chars from pos # to end"* / *"from pos #1 to #2"*.
    Mid {
        from: usize,
        to: Option<usize>,
    },
    /// The same, *"counting backwards"*.
    MidRev {
        from: usize,
        to: Option<usize>,
    },
    FirstLetter(LetterMode),
    /// `<Parent>` is `<Parent-1>`: *"N:th parent folder name (set # to 1-9)"*.
    Parent(usize),
    /// *"File size (Auto)"*.
    Size,
    /// *"File size (Bytes)"*, optionally *"padded with # zeros"*.
    SizeBytes {
        pad: usize,
    },
    Stamp {
        source: TimeSource,
        format: DateFormat,
    },
    Counter,
    /// *"Random Character (A-Z)"*.
    RandomLetter,
    /// `<Rnd#>`, `<Rnd#-Low-High>`, `<RndZ#-Low-High>`.
    RandomNumber {
        digits: usize,
        range: Option<(i64, i64)>,
        pad: bool,
    },
    NumFiles,
    /// Slot 0 is `<Ask>`; 1–9 are `<Ask-1>`…`<Ask-9>`.
    Ask(u8),
    Clipboard,
    Crc32,
    DetectedExt,
    /// `<%1>`…`<%9>`.
    Part(u8),
    /// `<\>` — *"Move To SubFolder"* (D31).
    SubFolder,
    /// *"Limit filename length to # chars"*.
    FileMax(usize),
    /// *"Limit path and filename length to # chars"*.
    PathMax(usize),
    /// `<Artist>`, `<Title>`, `<Bitrate>` … — a music file's own account of
    /// itself.
    Audio(AudioField),
    /// `<ID3-AlbumArtist>` and the rest of the ID3v2 frames.
    ///
    /// The name is carried rather than resolved at parse time so the table in
    /// [`crate::meta::names`] stays the single place that knows them.
    Id3(String),
    /// `<Exif-ImageWidth>` and anything else the image carries.
    Exif(String),
    /// `<Width> <Height> <Depth> <Depthb> <JpgComment>` — the image's own
    /// header, as opposed to the Exif block a camera wrote into it.
    Image(ImageField),
    /// `<HtmlTitle>` — the `<title>` of an HTML document.
    HtmlTitle,
    /// `<DirSize> <DirFiles> <DirDirs>` and the rest of the Folders group.
    Folder {
        field: FolderField,
        /// The `S` prefix: *"in Folder && Subfolders"*.
        recursive: bool,
        /// `<DirSizeB-#>` — *"padded to # zeros"*.
        pad: usize,
    },
}

/// The Folders group — *"Size of Files in Folder"* and its neighbours.
///
/// **Which folder**: for a folder row, itself; for a file row, the folder it is
/// in. Both readings are what the tag name says out loud, and a file answering
/// about its own parent is the only thing `<DirFiles>` could usefully mean when
/// the row *is* a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FolderField {
    /// *"Size of Files in Folder (Auto)"*, formatted like `<Size>`.
    Size,
    /// *"(Bytes)"*, optionally zero padded.
    SizeBytes,
    /// *"Number of Files in Folder"*.
    Files,
    /// *"Number of Folders in Folder"*.
    Dirs,
    /// *"First file in folder"*, without its extension.
    FirstFile,
    /// *"First file in folder with extension"*.
    FirstFileWithExtension,
}

/// The five image-header tags.
///
/// Four of these names would mean the same things about a video frame. We
/// answer for images; the movie half stays deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageField {
    /// *"Image Width"*.
    Width,
    /// *"Image Height"*.
    Height,
    /// *"Image Color Depth (Colors)"* — the palette the bit depth implies.
    Colours,
    /// *"Image Color Depth (Bits)"*.
    Bits,
    /// *"Jpeg Comment"*.
    JpegComment,
}

impl Tag {
    /// What this tag costs to resolve, so a template that never mentions
    /// `<Crc32>` never opens a file.
    pub fn needs(&self) -> TagNeeds {
        match self {
            Self::Counter => TagNeeds::COUNTER,
            Self::Ask(_) => TagNeeds::ASK,
            Self::Clipboard => TagNeeds::CLIPBOARD,
            Self::Part(_) => TagNeeds::PARTS,
            Self::RandomLetter | Self::RandomNumber { .. } => TagNeeds::RANDOM,
            Self::Stamp { source, .. } => match source {
                TimeSource::Now => TagNeeds::NONE,
                // The one date source that opens the file.
                TimeSource::Exif => TagNeeds::FILE_CONTENT,
                TimeSource::Modified | TimeSource::Created | TimeSource::Accessed => {
                    TagNeeds::TIMESTAMPS
                }
            },
            // Everything that opens the file. `<Crc32>` has always been here;
            // M6's readers join it, which is what the bit is for.
            Self::Crc32
            | Self::DetectedExt
            | Self::Audio(_)
            | Self::Id3(_)
            | Self::Exif(_)
            | Self::Image(_)
            | Self::HtmlTitle
            // Not the file's content, but a directory walk — which is the same
            // "this costs I/O" answer, and the bit exists to say so.
            | Self::Folder { .. } => TagNeeds::FILE_CONTENT,
            Self::SubFolder => TagNeeds::SUBFOLDER,
            Self::FileMax(_) | Self::PathMax(_) => TagNeeds::LIMITS,
            // Exhaustive from here on purpose. The catch-all this replaced
            // would have answered `NONE` for all three of M6's families —
            // silently, so nothing would have declared that music tags open
            // files and no test would have noticed.
            Self::Name
            | Self::Ext
            | Self::FullName
            | Self::Left(_)
            | Self::Right(_)
            | Self::Mid { .. }
            | Self::MidRev { .. }
            | Self::FirstLetter(_)
            | Self::Parent(_)
            | Self::Size
            | Self::SizeBytes { .. }
            | Self::NumFiles => TagNeeds::NONE,
        }
    }
}

/// The costs a whole template adds up to.
///
/// A hand-rolled bit set rather than a dependency: nine flags do not justify
/// one, and D2 makes every added crate a licence decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagNeeds(u16);

impl TagNeeds {
    pub const NONE: Self = Self(0);
    pub const COUNTER: Self = Self(1 << 0);
    pub const ASK: Self = Self(1 << 1);
    pub const CLIPBOARD: Self = Self(1 << 2);
    pub const PARTS: Self = Self(1 << 3);
    pub const RANDOM: Self = Self(1 << 4);
    /// Reads a timestamp captured with the listing — free, but absent on some
    /// filesystems, which is what makes a date tag *unavailable*.
    pub const TIMESTAMPS: Self = Self(1 << 5);
    /// Opens the file. The expensive one.
    pub const FILE_CONTENT: Self = Self(1 << 6);
    /// Produces a path separator, so the planner must create directories.
    pub const SUBFOLDER: Self = Self(1 << 7);
    pub const LIMITS: Self = Self(1 << 8);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Why a tag would not compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    #[error("<> is not a tag")]
    Empty,
    #[error("<{0}> is not a tag")]
    Unknown(String),
    /// A tag we recognise from a family that is not built yet. Saying so beats
    /// "unknown": the user typed something real.
    #[error("<{tag}> is not supported yet ({feature})")]
    Deferred { tag: String, feature: &'static str },
    #[error("<{tag}>: {message}")]
    BadArgument { tag: String, message: String },
}

impl Tag {
    /// Parses the text between the angle brackets.
    ///
    /// *"Most tags are not case sensitive, so you can write e.g. `<Date>`
    /// `<date>` `<DATE>` with the same result."*
    pub fn parse(body: &str) -> Result<Self, TagError> {
        if body.is_empty() {
            return Err(TagError::Empty);
        }
        // The one tag that is punctuation.
        if body == "\\" {
            return Ok(Self::SubFolder);
        }
        // Parts: `<%1>` … `<%9>`.
        if let Some(digits) = body.strip_prefix('%') {
            return match digits.parse::<u8>() {
                Ok(slot @ 1..=9) if digits.len() == 1 => Ok(Self::Part(slot)),
                _ => Err(TagError::BadArgument {
                    tag: body.to_owned(),
                    message: "parts are numbered 1 to 9".to_owned(),
                }),
            };
        }

        // Everything else is `Head` or `Head-argument…`, where the argument may
        // itself contain hyphens (`<Date-yyyy-mm-dd>`).
        let (head, arg) = match body.split_once('-') {
            Some((head, arg)) => (head, Some(arg)),
            None => (body, None),
        };
        let key = head.to_ascii_lowercase();

        // The metadata families, before the deferred gate: a prefix entry
        // there would swallow them whole, which is what kept `<ExifDate>`
        // unreachable behind the old `exif` row.
        if let Some(name) = strip_family(&key, "id3", arg) {
            if crate::meta::names::id3_is_unsupported(name) {
                return Err(TagError::Deferred {
                    tag: body.to_owned(),
                    feature: "this ID3 frame has no cross-format equivalent",
                });
            }
            if crate::meta::names::id3_key(name).is_none() {
                return Err(TagError::Unknown(body.to_owned()));
            }
            return Ok(Self::Id3(name.to_owned()));
        }
        if let Some(name) = strip_family(&key, "exif", arg) {
            return Ok(Self::Exif(name.to_owned()));
        }
        if arg.is_none()
            && let Some(field) = AudioField::parse(&key)
        {
            return Ok(Self::Audio(field));
        }
        // Before the deferred gate, which still lists the folder family for the
        // tags M8 did not build (`<DirLength>`).
        if key == "htmltitle" && arg.is_none() {
            return Ok(Self::HtmlTitle);
        }
        if let Some(tag) = folder_tag(&key, arg) {
            return tag.map_err(|_| TagError::Unknown(body.to_owned()));
        }
        // Before the deferred gate, which still lists these names for the
        // *movie* half of the family they share (`deferred_feature`).
        if arg.is_none() {
            let image = match key.as_str() {
                "width" => Some(ImageField::Width),
                "height" => Some(ImageField::Height),
                "depth" => Some(ImageField::Colours),
                "depthb" => Some(ImageField::Bits),
                "jpgcomment" => Some(ImageField::JpegComment),
                _ => None,
            };
            if let Some(field) = image {
                return Ok(Self::Image(field));
            }
        }

        if let Some(feature) = deferred_feature(&key) {
            return Err(TagError::Deferred {
                tag: body.to_owned(),
                feature,
            });
        }

        // `<Rnd>`, `<Rnd3>`, `<Rnd3-1-100>`, `<RndZ3-1-100>` all start here.
        if let Some(rest) = key.strip_prefix("rnd") {
            return random(body, rest, arg);
        }

        match (key.as_str(), arg) {
            ("name", None) => Ok(Self::Name),
            ("ext", None) => Ok(Self::Ext),
            ("fullname", None) => Ok(Self::FullName),
            ("fletter", None) => Ok(Self::FirstLetter(LetterMode::Letters)),
            ("flettern1", None) => Ok(Self::FirstLetter(LetterMode::WithNumbers)),
            ("flettern2", None) => Ok(Self::FirstLetter(LetterMode::NumbersSubstituted)),
            ("parent", None) => Ok(Self::Parent(1)),
            ("parent", Some(n)) => Ok(Self::Parent(index(body, n, 1, 9)? as usize)),
            ("left", Some(n)) => Ok(Self::Left(count(body, n)?)),
            ("right", Some(n)) => Ok(Self::Right(count(body, n)?)),
            ("mid", Some(a)) => {
                let (from, to) = span(body, a)?;
                Ok(Self::Mid { from, to })
            }
            ("midrev", Some(a)) => {
                let (from, to) = span(body, a)?;
                Ok(Self::MidRev { from, to })
            }
            ("size", None) => Ok(Self::Size),
            ("sizeb", None) => Ok(Self::SizeBytes { pad: 0 }),
            ("sizeb", Some(n)) => Ok(Self::SizeBytes {
                pad: count(body, n)?,
            }),
            ("date", None) => Ok(stamp(TimeSource::Modified, DEFAULT_DATE)),
            ("time", None) => Ok(stamp(TimeSource::Modified, DEFAULT_TIME)),
            ("cdate", None) => Ok(stamp(TimeSource::Created, DEFAULT_DATE)),
            ("ctime", None) => Ok(stamp(TimeSource::Created, DEFAULT_TIME)),
            ("adate", None) => Ok(stamp(TimeSource::Accessed, DEFAULT_DATE)),
            ("atime", None) => Ok(stamp(TimeSource::Accessed, DEFAULT_TIME)),
            ("nowdate", None) => Ok(stamp(TimeSource::Now, DEFAULT_DATE)),
            ("nowtime", None) => Ok(stamp(TimeSource::Now, DEFAULT_TIME)),
            // *"`<ExifDate>`/`<ExifTime>` are 'smart' (find date across the
            // varying Exif date fields)"* — the smartness lives in
            // `meta::exif`, which already tries DateTimeOriginal, then
            // DateTimeDigitized, then DateTime.
            ("exifdate", None) => Ok(stamp(TimeSource::Exif, DEFAULT_DATE)),
            ("exiftime", None) => Ok(stamp(TimeSource::Exif, DEFAULT_TIME)),
            ("date", Some(f)) => Ok(stamp(TimeSource::Modified, f)),
            ("cdate", Some(f)) => Ok(stamp(TimeSource::Created, f)),
            ("adate", Some(f)) => Ok(stamp(TimeSource::Accessed, f)),
            ("exifdate", Some(f)) => Ok(stamp(TimeSource::Exif, f)),
            ("counter", None) => Ok(Self::Counter),
            ("numfiles", None) => Ok(Self::NumFiles),
            ("ask", None) => Ok(Self::Ask(0)),
            ("ask", Some(n)) => Ok(Self::Ask(index(body, n, 1, 9)?)),
            ("clipboard", None) => Ok(Self::Clipboard),
            ("crc32", None) => Ok(Self::Crc32),
            ("detectedext", None) => Ok(Self::DetectedExt),
            ("filemax", Some(n)) => Ok(Self::FileMax(count(body, n)?)),
            ("pathmax", Some(n)) => Ok(Self::PathMax(count(body, n)?)),
            // A known head with the wrong shape deserves better than "unknown".
            ("left" | "right" | "mid" | "midrev" | "filemax" | "pathmax", None) => {
                Err(TagError::BadArgument {
                    tag: body.to_owned(),
                    message: "needs a number, as in <Left-3>".to_owned(),
                })
            }
            _ => Err(TagError::Unknown(body.to_owned())),
        }
    }
}

fn stamp(source: TimeSource, format: &str) -> Tag {
    Tag::Stamp {
        source,
        format: DateFormat::parse(format),
    }
}

/// A plain count, as in `<Left-3>`.
fn count(tag: &str, text: &str) -> Result<usize, TagError> {
    text.trim()
        .parse::<usize>()
        .map_err(|_| TagError::BadArgument {
            tag: tag.to_owned(),
            message: format!("{text:?} is not a number"),
        })
}

/// A 1-based selector with a documented range, as in `<Parent-2>`.
fn index(tag: &str, text: &str, low: u8, high: u8) -> Result<u8, TagError> {
    match text.trim().parse::<u8>() {
        Ok(n) if (low..=high).contains(&n) => Ok(n),
        _ => Err(TagError::BadArgument {
            tag: tag.to_owned(),
            message: format!("set # to {low}-{high}"),
        }),
    }
}

/// `<Mid-#>` or `<Mid-#-#>`.
fn span(tag: &str, arg: &str) -> Result<(usize, Option<usize>), TagError> {
    match arg.split_once('-') {
        Some((from, to)) => Ok((count(tag, from)?, Some(count(tag, to)?))),
        None => Ok((count(tag, arg)?, None)),
    }
}

/// The `<Rnd…>` family. `rest` is whatever followed `rnd` in the head.
fn random(body: &str, rest: &str, arg: Option<&str>) -> Result<Tag, TagError> {
    let bad = |message: &str| TagError::BadArgument {
        tag: body.to_owned(),
        message: message.to_owned(),
    };

    // `<Rnd>` on its own is the random *letter*.
    if rest.is_empty() && arg.is_none() {
        return Ok(Tag::RandomLetter);
    }
    let (pad, digits_text) = match rest.strip_prefix('z') {
        Some(digits) => (true, digits),
        None => (false, rest),
    };
    if digits_text.is_empty() {
        return Err(bad("needs a digit count, as in <Rnd3>"));
    }
    let digits = count(body, digits_text)?;
    if digits == 0 || digits > 18 {
        return Err(bad("use 1 to 18 digits"));
    }

    let range = match arg {
        None => None,
        Some(text) => {
            let Some((low, high)) = text.split_once('-') else {
                return Err(bad("a range needs both ends, as in <Rnd3-1-100>"));
            };
            let parse = |s: &str| {
                s.trim()
                    .parse::<i64>()
                    .map_err(|_| bad("range must be whole numbers"))
            };
            let (low, high) = (parse(low)?, parse(high)?);
            if low > high {
                return Err(bad("the low end must not exceed the high end"));
            }
            Some((low, high))
        }
    };
    Ok(Tag::RandomNumber { digits, range, pad })
}

/// The argument of `<Family-Name>`, when `key` is that family.
///
/// The argument is handed back with its **original case**, because `Tag::parse`
/// folds only the head — which is what makes `<Exif-CameraSettings:MacroMode>`
/// and `<Exif-Capture Mode>` work without the lexer knowing anything about
/// them.
fn strip_family<'a>(key: &str, family: &str, arg: Option<&'a str>) -> Option<&'a str> {
    (key == family).then_some(arg?).filter(|a| !a.is_empty())
}

/// Tags we recognise but have not built, and what is missing.
///
/// Keeping this list means a user who types `<AVLength>` is told it is coming,
/// not that it does not exist. The example used to be `<Artist>`, which M6
/// built, and then `<Width>`, which M8 built — a list like this is only useful
/// while its example is still on it.
fn deferred_feature(key: &str) -> Option<&'static str> {
    const AUDIO: &str = "music tags arrive with the Mp3 function";
    const MEDIA: &str = "movie and audio property tags are not implemented";
    const PDF: &str = "PDF tags are not implemented";
    const FOLDER: &str = "the length of the music in a folder is not implemented (D129)";

    // `id3` and `exif` are gone from this list: M6 implements them, and a
    // *prefix* entry would have swallowed them whole. `exiftool` has to be
    // tested before `xif` would have matched it — the old list had `exif`
    // ahead of `exiftool`, so the longer name was unreachable and every
    // `<ExifTool-…>` reported the wrong feature. Longest first, always.
    let prefixed = [
        ("exiftool", "the ExifTool bridge is not implemented (P8)"),
        // Not IMAGE: `<Xif-*>` is a legacy INI-driven Exif reader, waived by
        // P8. Reusing the image-dimension message told a
        // user Exif had not arrived, which has been false since M6.
        (
            "xif",
            "the legacy INI-driven Exif reader is not implemented (P8) — use <Exif-…>",
        ),
        ("iptc", "IPTC tags are not implemented"),
        ("geotiff", "GeoTIFF tags are not implemented"),
        ("pdf", PDF),
    ];
    if let Some((_, feature)) = prefixed.iter().find(|(p, _)| key.starts_with(p)) {
        return Some(feature);
    }

    let exact = [
        // The three MPEG header tags. lofty drops the MPEG-specific fields on
        // the way out of `MpegFile`, so reading them means a second open per
        // file per keystroke for three rarely-used tags — deferred rather than
        // paid for. `<BrMode>` needs a Xing/VBRI probe lofty does not expose at
        // all.
        ("brmode", AUDIO),
        ("mpeg", AUDIO),
        ("layer", AUDIO),
        ("smode", AUDIO),
        ("avlength", MEDIA),
        ("avlengths", MEDIA),
        ("fps", MEDIA),
        ("avbitrate", MEDIA),
        ("codec", MEDIA),
        ("sound", MEDIA),
        ("avstereo", MEDIA),
        ("avfreq", MEDIA),
        ("avfreqs", MEDIA),
        ("bits", MEDIA),
        ("sndbitrate", MEDIA),
        ("sndformat", MEDIA),
        // The eleven names beside it are built (D129); this one is not, and
        // the message says why rather than naming a family.
        ("dirlength", FOLDER),
    ];
    exact
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, feature)| *feature)
}

impl fmt::Display for Tag {
    /// The canonical spelling, which is what the tag picker inserts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio(field) => write!(f, "<{}>", field.label()),
            Self::Id3(name) => write!(f, "<ID3-{name}>"),
            Self::Exif(name) => write!(f, "<Exif-{name}>"),
            Self::HtmlTitle => f.write_str("<HtmlTitle>"),
            Self::Folder {
                field,
                recursive,
                pad,
            } => {
                let s = if *recursive { "S" } else { "" };
                match field {
                    FolderField::Size => write!(f, "<{s}DirSize>"),
                    FolderField::SizeBytes if *pad == 0 => write!(f, "<{s}DirSizeB>"),
                    FolderField::SizeBytes => write!(f, "<{s}DirSizeB-{pad}>"),
                    FolderField::Files => write!(f, "<{s}DirFiles>"),
                    FolderField::Dirs => write!(f, "<{s}DirDirs>"),
                    FolderField::FirstFile => f.write_str("<FirstFileInFolder>"),
                    FolderField::FirstFileWithExtension => f.write_str("<FirstFileInFolder-Ext>"),
                }
            }
            Self::Image(field) => f.write_str(match field {
                ImageField::Width => "<Width>",
                ImageField::Height => "<Height>",
                ImageField::Colours => "<Depth>",
                ImageField::Bits => "<Depthb>",
                ImageField::JpegComment => "<JpgComment>",
            }),
            Self::Name => f.write_str("<Name>"),
            Self::Ext => f.write_str("<Ext>"),
            Self::FullName => f.write_str("<FullName>"),
            Self::Left(n) => write!(f, "<Left-{n}>"),
            Self::Right(n) => write!(f, "<Right-{n}>"),
            Self::Mid { from, to: None } => write!(f, "<Mid-{from}>"),
            Self::Mid { from, to: Some(to) } => write!(f, "<Mid-{from}-{to}>"),
            Self::MidRev { from, to: None } => write!(f, "<MidRev-{from}>"),
            Self::MidRev { from, to: Some(to) } => write!(f, "<MidRev-{from}-{to}>"),
            Self::FirstLetter(LetterMode::Letters) => f.write_str("<FLetter>"),
            Self::FirstLetter(LetterMode::WithNumbers) => f.write_str("<FLetterN1>"),
            Self::FirstLetter(LetterMode::NumbersSubstituted) => f.write_str("<FLetterN2>"),
            Self::Parent(1) => f.write_str("<Parent>"),
            Self::Parent(n) => write!(f, "<Parent-{n}>"),
            Self::Size => f.write_str("<Size>"),
            Self::SizeBytes { pad: 0 } => f.write_str("<SizeB>"),
            Self::SizeBytes { pad } => write!(f, "<SizeB-{pad}>"),
            Self::Stamp { source, format } => {
                // The two default formats have tags of their own — `<Time>` is
                // just `<Date-Hh.Mm.Ss>` with a nicer name.
                let (date, time) = match source {
                    TimeSource::Modified => ("Date", "Time"),
                    TimeSource::Created => ("CDate", "CTime"),
                    TimeSource::Accessed => ("ADate", "ATime"),
                    TimeSource::Now => ("NowDate", "NowTime"),
                    TimeSource::Exif => ("ExifDate", "ExifTime"),
                };
                if format.source() == DEFAULT_DATE {
                    write!(f, "<{date}>")
                } else if format.source() == DEFAULT_TIME {
                    write!(f, "<{time}>")
                } else {
                    write!(f, "<{date}-{}>", format.source())
                }
            }
            Self::Counter => f.write_str("<Counter>"),
            Self::RandomLetter => f.write_str("<Rnd>"),
            Self::RandomNumber { digits, range, pad } => {
                let z = if *pad { "Z" } else { "" };
                match range {
                    Some((low, high)) => write!(f, "<Rnd{z}{digits}-{low}-{high}>"),
                    None => write!(f, "<Rnd{z}{digits}>"),
                }
            }
            Self::NumFiles => f.write_str("<NumFiles>"),
            Self::Ask(0) => f.write_str("<Ask>"),
            Self::Ask(n) => write!(f, "<Ask-{n}>"),
            Self::Clipboard => f.write_str("<Clipboard>"),
            Self::Crc32 => f.write_str("<Crc32>"),
            Self::DetectedExt => f.write_str("<DetectedExt>"),
            Self::Part(n) => write!(f, "<%{n}>"),
            Self::SubFolder => f.write_str("<\\>"),
            Self::FileMax(n) => write!(f, "<FileMax-{n}>"),
            Self::PathMax(n) => write!(f, "<PathMax-{n}>"),
        }
    }
}

/// The Folders group, parsed. `Ok(None)` from the outer `Option` means "not one
/// of these"; the inner `Err` means it looked like one and its argument did not.
fn folder_tag(key: &str, arg: Option<&str>) -> Option<Result<Tag, ()>> {
    if key == "firstfileinfolder" {
        return Some(match arg {
            None => Ok(Tag::Folder {
                field: FolderField::FirstFile,
                recursive: false,
                pad: 0,
            }),
            Some(a) if a.eq_ignore_ascii_case("ext") => Ok(Tag::Folder {
                field: FolderField::FirstFileWithExtension,
                recursive: false,
                pad: 0,
            }),
            Some(_) => Err(()),
        });
    }

    // The `S` prefix is *"in Folder && Subfolders"*, and `<SDirSize>` has to be
    // tested before `<DirSize>` or the shorter name never matches.
    let (recursive, rest) = match key.strip_prefix("sdir") {
        Some(rest) => (true, rest),
        None => (false, key.strip_prefix("dir")?),
    };

    let field = match rest {
        "size" => FolderField::Size,
        "sizeb" => FolderField::SizeBytes,
        "files" => FolderField::Files,
        "dirs" => FolderField::Dirs,
        // `<DirLength>` and anything else beginning `dir` falls through to the
        // deferred list, which still names it.
        _ => return None,
    };

    // Only the byte size takes a pad: `<DirSizeB-#>` and nothing else in the
    // group.
    let pad = match (arg, field) {
        (None, _) => 0,
        (Some(a), FolderField::SizeBytes) => match a.parse::<usize>() {
            Ok(n) => n,
            Err(_) => return Some(Err(())),
        },
        (Some(_), _) => return Some(Err(())),
    };

    Some(Ok(Tag::Folder {
        field,
        recursive,
        pad,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Tag {
        Tag::parse(body).unwrap_or_else(|e| panic!("<{body}> should parse: {e}"))
    }

    #[test]
    fn the_three_name_tags_parse() {
        assert_eq!(parse("Name"), Tag::Name);
        assert_eq!(parse("Ext"), Tag::Ext);
        assert_eq!(parse("FullName"), Tag::FullName);
    }

    /// "Most tags are not case sensitive, so you can write e.g. <Date> <date>
    /// <DATE> with the same result."
    #[test]
    fn tags_are_not_case_sensitive() {
        assert_eq!(parse("name"), parse("NAME"));
        assert_eq!(parse("NAME"), parse("NaMe"));
        assert_eq!(parse("fullname"), Tag::FullName);
        assert_eq!(parse("flettern2"), parse("FLetterN2"));
    }

    #[test]
    fn substring_tags_carry_their_numbers() {
        assert_eq!(parse("Left-3"), Tag::Left(3));
        assert_eq!(parse("Right-2"), Tag::Right(2));
        assert_eq!(parse("Mid-4"), Tag::Mid { from: 4, to: None });
        assert_eq!(
            parse("Mid-2-5"),
            Tag::Mid {
                from: 2,
                to: Some(5)
            }
        );
        assert_eq!(parse("MidRev-3"), Tag::MidRev { from: 3, to: None });
        assert_eq!(
            parse("MidRev-5-2"),
            Tag::MidRev {
                from: 5,
                to: Some(2)
            }
        );
    }

    /// "N:th parent folder name (set # to 1-9)"
    #[test]
    fn parent_defaults_to_the_immediate_parent_and_is_limited_to_nine() {
        assert_eq!(parse("Parent"), Tag::Parent(1));
        assert_eq!(parse("Parent-3"), Tag::Parent(3));
        assert!(Tag::parse("Parent-0").is_err());
        assert!(Tag::parse("Parent-10").is_err());
    }

    #[test]
    fn the_date_family_maps_to_its_timestamp() {
        assert!(matches!(
            parse("Date"),
            Tag::Stamp {
                source: TimeSource::Modified,
                ..
            }
        ));
        assert!(matches!(
            parse("CTime"),
            Tag::Stamp {
                source: TimeSource::Created,
                ..
            }
        ));
        assert!(matches!(
            parse("ADate"),
            Tag::Stamp {
                source: TimeSource::Accessed,
                ..
            }
        ));
        assert!(matches!(
            parse("NowDate"),
            Tag::Stamp {
                source: TimeSource::Now,
                ..
            }
        ));
    }

    /// "<Date-Short Date>, <Date-dddd m mmmm>" — the argument is taken whole,
    /// hyphens and all.
    #[test]
    fn a_custom_date_format_keeps_its_hyphens() {
        let Tag::Stamp { format, .. } = parse("Date-yyyy-mm-dd") else {
            panic!("expected a date tag");
        };
        assert_eq!(format.source(), "yyyy-mm-dd");

        let Tag::Stamp { format, .. } = parse("Date-Short Date") else {
            panic!("expected a date tag");
        };
        assert_eq!(format.source(), "Short Date");
    }

    #[test]
    fn the_random_family_covers_letter_digits_and_range() {
        assert_eq!(parse("Rnd"), Tag::RandomLetter);
        assert_eq!(
            parse("Rnd3"),
            Tag::RandomNumber {
                digits: 3,
                range: None,
                pad: false
            }
        );
        assert_eq!(
            parse("Rnd2-1-100"),
            Tag::RandomNumber {
                digits: 2,
                range: Some((1, 100)),
                pad: false
            }
        );
        assert_eq!(
            parse("RndZ4-1-100"),
            Tag::RandomNumber {
                digits: 4,
                range: Some((1, 100)),
                pad: true
            }
        );
    }

    #[test]
    fn a_backwards_range_is_rejected() {
        assert!(matches!(
            Tag::parse("Rnd3-100-1"),
            Err(TagError::BadArgument { .. })
        ));
    }

    #[test]
    fn ask_slots_run_from_zero_to_nine() {
        assert_eq!(parse("Ask"), Tag::Ask(0));
        assert_eq!(parse("Ask-7"), Tag::Ask(7));
        assert!(Tag::parse("Ask-0").is_err());
    }

    #[test]
    fn parts_are_their_own_syntax() {
        assert_eq!(parse("%1"), Tag::Part(1));
        assert_eq!(parse("%9"), Tag::Part(9));
        assert!(Tag::parse("%0").is_err());
        assert!(Tag::parse("%12").is_err());
    }

    /// "<\> - Move To SubFolder"
    #[test]
    fn the_subfolder_tag_is_a_backslash() {
        assert_eq!(parse("\\"), Tag::SubFolder);
    }

    /// The canonical spelling is `<PathMax-260>`; `<PathMax-260#>` is a
    /// typo in the help file.
    #[test]
    fn the_length_limits_take_a_character_count() {
        assert_eq!(parse("FileMax-64"), Tag::FileMax(64));
        assert_eq!(parse("PathMax-260"), Tag::PathMax(260));
    }

    /// D29: a typo stops the rename instead of quietly emptying a name.
    #[test]
    fn an_unknown_tag_is_an_error_naming_what_was_typed() {
        let err = Tag::parse("Nmae").unwrap_err();
        assert_eq!(err, TagError::Unknown("Nmae".to_owned()));
        assert!(err.to_string().contains("<Nmae>"));
        assert_eq!(Tag::parse("").unwrap_err(), TagError::Empty);
    }

    /// A real tag from a family we have not built says so.
    #[test]
    fn a_deferred_tag_reports_its_family_rather_than_being_unknown() {
        // `Artist`, `ID3-Composer`, `Exif-ImageWidth`, `Width` and `DirFiles`
        // have all left this list as they were built. `DirLength` is what is
        // left of the folder family — see D129. What is here is what genuinely
        // is not implemented.
        for body in [
            "BrMode",
            "Mpeg",
            "Layer",
            "SMode",
            "AVLength",
            "Iptc-Keywords",
            "GeoTiff-ModelType",
            "ExifTool-ImageWidth",
            "PdfPages",
            "DirLength",
        ] {
            assert!(
                matches!(Tag::parse(body), Err(TagError::Deferred { .. })),
                "<{body}> should be reported as deferred, got {:?}",
                Tag::parse(body)
            );
        }
    }

    /// The old `deferred_feature` list had `exif` ahead of `exiftool`, and it
    /// matched by prefix — so the longer name was unreachable and every
    /// `<ExifTool-…>` reported the wrong feature. Deleting the `exif` row for
    /// M6 would have left `exiftool` matching `xif`'s neighbours instead.
    #[test]
    fn exiftool_is_not_shadowed_by_a_shorter_prefix() {
        let err = Tag::parse("ExifTool-ImageWidth").unwrap_err();
        match err {
            TagError::Deferred { feature, .. } => {
                assert!(feature.contains("ExifTool"), "{feature}");
            }
            other => panic!("expected Deferred, got {other:?}"),
        }
    }

    #[test]
    fn the_music_tags_parse_to_one_variant_with_a_field() {
        assert_eq!(Tag::parse("Artist"), Ok(Tag::Audio(AudioField::Artist)));
        assert_eq!(Tag::parse("artist"), Ok(Tag::Audio(AudioField::Artist)));
        assert_eq!(Tag::parse("TrackN"), Ok(Tag::Audio(AudioField::TrackNoPad)));
        assert_eq!(
            Tag::parse("FreqS"),
            Ok(Tag::Audio(AudioField::FrequencyShort))
        );
        // Every one of them opens the file, and says so.
        for field in AudioField::ALL {
            assert!(Tag::Audio(field).needs().contains(TagNeeds::FILE_CONTENT));
        }
    }

    /// The argument keeps its case, which is what makes
    /// `<Exif-CameraSettings:MacroMode>` and `<Exif-Capture Mode>` work without
    /// the lexer knowing anything about them.
    #[test]
    fn a_metadata_family_takes_its_argument_whole_and_unfolded() {
        assert_eq!(
            Tag::parse("Exif-ImageWidth"),
            Ok(Tag::Exif("ImageWidth".to_owned()))
        );
        assert_eq!(
            Tag::parse("exif-CameraSettings:MacroMode"),
            Ok(Tag::Exif("CameraSettings:MacroMode".to_owned()))
        );
        assert_eq!(
            Tag::parse("EXIF-Capture Mode"),
            Ok(Tag::Exif("Capture Mode".to_owned()))
        );
        assert_eq!(
            Tag::parse("ID3-AlbumArtist"),
            Ok(Tag::Id3("AlbumArtist".to_owned()))
        );
    }

    /// D29 survives the generic families: a name the table does not know is an
    /// error, not an empty string inside somebody's filename.
    #[test]
    fn a_mistyped_id3_name_is_an_error_rather_than_nothing() {
        assert!(matches!(
            Tag::parse("ID3-Compsoer"),
            Err(TagError::Unknown(_))
        ));
        // And the six with no cross-format equivalent say so rather than
        // reading as unknown.
        assert!(matches!(
            Tag::parse("ID3-FileType"),
            Err(TagError::Deferred { .. })
        ));
    }

    /// The regression this test exists for: deleting `deferred_feature`'s
    /// `exif` prefix row for M6 left `<ExifDate>` matching nothing at all, so
    /// it reported *"is not a tag"* — telling the user their spelling was
    /// wrong when it was not, for a tag that exists. It has to resolve, not
    /// merely stop being deferred.
    #[test]
    fn the_exif_date_tags_are_reachable() {
        assert_eq!(
            Tag::parse("ExifDate"),
            Ok(stamp(TimeSource::Exif, DEFAULT_DATE))
        );
        assert_eq!(
            Tag::parse("ExifTime"),
            Ok(stamp(TimeSource::Exif, DEFAULT_TIME))
        );
        assert_eq!(
            Tag::parse("exifdate-yyyy"),
            Ok(stamp(TimeSource::Exif, "yyyy"))
        );
        // And it is the one date source that opens the file.
        assert!(
            Tag::parse("ExifDate")
                .unwrap()
                .needs()
                .contains(TagNeeds::FILE_CONTENT)
        );
        assert!(
            Tag::parse("Date")
                .unwrap()
                .needs()
                .contains(TagNeeds::TIMESTAMPS)
        );
    }

    /// Every tag family M6 touched, checked for the same failure in one place:
    /// a tag that exists must never report as a typo.
    #[test]
    fn no_documented_tag_reports_itself_as_unknown() {
        for body in [
            // Implemented by M6.
            "Artist",
            "Title",
            "Track",
            "TrackN",
            "FreqS",
            "ExifDate",
            "ExifTime",
            "ID3-AlbumArtist",
            "Exif-Make",
            // Documented and deliberately not implemented — deferred, which
            // names a reason, rather than unknown, which blames the speller.
            "BrMode",
            "Mpeg",
            "Width",
            "PdfPages",
            "Iptc-Keywords",
            "GeoTiff-ModelType",
            "ExifTool-ImageWidth",
            "DirLength",
            "ID3-FileType",
        ] {
            assert!(
                !matches!(Tag::parse(body), Err(TagError::Unknown(_))),
                "<{body}> is documented but reports as a typo"
            );
        }
    }

    #[test]
    fn the_new_families_round_trip_through_their_canonical_spelling() {
        for tag in [
            Tag::Audio(AudioField::Artist),
            Tag::Audio(AudioField::TrackNoPad),
            Tag::Id3("AlbumArtist".to_owned()),
            Tag::Exif("ImageWidth".to_owned()),
        ] {
            let text = tag.to_string();
            let body = text.trim_start_matches('<').trim_end_matches('>');
            assert_eq!(Tag::parse(body), Ok(tag.clone()), "{text}");
        }
    }

    #[test]
    fn a_known_tag_with_a_missing_argument_says_what_it_wants() {
        let err = Tag::parse("Left").unwrap_err();
        assert!(err.to_string().contains("<Left-3>"), "{err}");
        let err = Tag::parse("Left-x").unwrap_err();
        assert!(err.to_string().contains("not a number"), "{err}");
    }

    #[test]
    fn needs_are_only_claimed_by_the_tags_that_have_them() {
        assert!(Tag::Name.needs().is_empty());
        assert!(Tag::Counter.needs().contains(TagNeeds::COUNTER));
        assert!(Tag::Crc32.needs().contains(TagNeeds::FILE_CONTENT));
        assert!(Tag::DetectedExt.needs().contains(TagNeeds::FILE_CONTENT));
        assert!(Tag::SubFolder.needs().contains(TagNeeds::SUBFOLDER));
        assert!(parse("Date").needs().contains(TagNeeds::TIMESTAMPS));
        // The run's own clock is always available.
        assert!(parse("NowDate").needs().is_empty());
    }

    #[test]
    fn needs_combine() {
        let both = Tag::Counter.needs().union(Tag::Crc32.needs());
        assert!(both.contains(TagNeeds::COUNTER));
        assert!(both.contains(TagNeeds::FILE_CONTENT));
        assert!(!both.contains(TagNeeds::ASK));
    }

    /// Every tag prints back to something that parses to itself — which is what
    /// the tag picker relies on when it inserts one.
    #[test]
    fn tags_round_trip_through_their_canonical_spelling() {
        let tags = [
            Tag::Name,
            Tag::Ext,
            Tag::FullName,
            Tag::Left(3),
            Tag::Right(2),
            Tag::Mid { from: 1, to: None },
            Tag::Mid {
                from: 1,
                to: Some(4),
            },
            Tag::MidRev { from: 3, to: None },
            Tag::MidRev {
                from: 5,
                to: Some(2),
            },
            Tag::FirstLetter(LetterMode::Letters),
            Tag::FirstLetter(LetterMode::WithNumbers),
            Tag::FirstLetter(LetterMode::NumbersSubstituted),
            Tag::Parent(1),
            Tag::Parent(4),
            Tag::Size,
            Tag::SizeBytes { pad: 0 },
            Tag::SizeBytes { pad: 12 },
            Tag::Counter,
            Tag::RandomLetter,
            Tag::RandomNumber {
                digits: 3,
                range: None,
                pad: false,
            },
            Tag::RandomNumber {
                digits: 3,
                range: Some((1, 100)),
                pad: true,
            },
            Tag::NumFiles,
            Tag::Ask(0),
            Tag::Ask(5),
            Tag::Clipboard,
            Tag::Crc32,
            Tag::DetectedExt,
            Tag::Part(2),
            Tag::SubFolder,
            Tag::FileMax(64),
            Tag::PathMax(260),
        ];
        for tag in tags {
            let text = tag.to_string();
            let body = text.trim_start_matches('<').trim_end_matches('>');
            assert_eq!(Tag::parse(body).unwrap(), tag, "{text}");
        }
    }

    #[test]
    fn date_tags_round_trip_including_their_format() {
        for body in ["Date", "CDate-yyyy", "ADate-Short Date", "NowDate"] {
            let tag = parse(body);
            let text = tag.to_string();
            let inner = text.trim_start_matches('<').trim_end_matches('>');
            assert_eq!(Tag::parse(inner).unwrap(), tag, "{body} printed as {text}");
        }
    }
}
