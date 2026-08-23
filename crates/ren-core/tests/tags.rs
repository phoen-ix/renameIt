//! One table, every tag M3 implements.
//!
//! M3's acceptance criterion is *"a table-driven test per tag"*. The
//! per-family tests live next to the code; this is the
//! flat list, run against one real file on disk so the tags that need a
//! timestamp or the file's contents resolve for real rather than through a
//! synthetic entry.
//!
//! It is also the completeness guard: a tag that parses but was never wired
//! into the resolver, or one whose meaning drifts, shows up here as a single
//! failing row.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ren_core::model::FileEntry;
use ren_core::run::{Answers, RunContext, RunSettings};
use ren_core::{CounterSetup, EvalCx, PartsSpec, Template};
use tempfile::TempDir;

/// 1977-05-09 10:18:05 UTC — the example timestamp.
const STAMP: u64 = 231_934_685;

struct Fixture {
    _dir: TempDir,
    entry: FileEntry,
    run: RunContext,
}

impl Fixture {
    /// A PNG, 1536 bytes, in `…/music/rock`, with every timestamp set and a
    /// name the Parts pattern below can split.
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let folder = dir.path().join("music").join("rock");
        std::fs::create_dir_all(&folder).expect("mkdir");

        let path = folder.join("01. Metallica (S&M) Nothing Else Matters.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.resize(1536, 0);
        std::fs::write(&path, &bytes).expect("write");

        let mut entry = FileEntry::from_path(&path).expect("metadata");
        let stamp = UNIX_EPOCH + Duration::from_secs(STAMP);
        entry.modified = Some(stamp);
        entry.created = Some(stamp);
        entry.accessed = Some(stamp);

        let settings = RunSettings {
            counter: CounterSetup {
                start: 7,
                auto_pad: false,
                pad: 3,
                ..Default::default()
            },
            parts: PartsSpec::new("<%1>. <%2> (<%3>) <%4>"),
            ..Default::default()
        };
        let mut answers = Answers::default();
        answers.asks.insert(0, "asked".into());
        answers.asks.insert(4, "asked four".into());
        answers.clipboard = Some("pasted".into());

        // Five entries so <NumFiles> has something to say and the counter has
        // somewhere to go.
        let listing: Vec<FileEntry> = (0..5)
            .map(|i| FileEntry::synthetic(folder.join(format!("{i}.png"))))
            .collect();
        let run = RunContext::build(&listing, &settings, answers);

        Self {
            _dir: dir,
            entry,
            run,
        }
    }

    fn render(&self, template: &str) -> String {
        Template::compile(template)
            .unwrap_or_else(|e| panic!("{template} should compile: {e}"))
            .render(&EvalCx::new(&self.entry, 0, 5, &self.run))
            .text
    }
}

/// Every tag that renders the same thing every time it runs.
#[test]
fn every_tag_renders_what_the_reference_says_it_does() {
    let f = Fixture::new();
    let name = "01. Metallica (S&M) Nothing Else Matters";

    let cases: &[(&str, String)] = &[
        // Name
        ("<Name>", name.to_owned()),
        ("<Ext>", "png".to_owned()),
        ("<FullName>", format!("{name}.png")),
        ("<Left-2>", "01".to_owned()),
        ("<Right-7>", "Matters".to_owned()),
        ("<Mid-4>", name[4..].to_owned()),
        ("<Mid-4-13>", "Metallica".to_owned()),
        ("<MidRev-7>", "Matters".to_owned()),
        ("<MidRev-7-0>", "Matters".to_owned()),
        ("<FLetter>", String::new()),
        ("<FLetterN1>", "0".to_owned()),
        ("<FLetterN2>", "#".to_owned()),
        ("<Parent>", "rock".to_owned()),
        ("<Parent-1>", "rock".to_owned()),
        ("<Parent-2>", "music".to_owned()),
        // Size
        ("<Size>", "1.5KB".to_owned()),
        ("<SizeB>", "1536".to_owned()),
        ("<SizeB-8>", "00001536".to_owned()),
        // Dates. Rendered in local time, so the expectation is computed the
        // same way rather than hard-coded to a timezone the CI runner may not
        // be in.
        ("<Date>", local("yyyy-mm-dd")),
        ("<Time>", local("Hh.Mm.Ss")),
        ("<CDate>", local("yyyy-mm-dd")),
        ("<CTime>", local("Hh.Mm.Ss")),
        ("<ADate>", local("yyyy-mm-dd")),
        ("<ATime>", local("Hh.Mm.Ss")),
        ("<Date-yyyy>", local("yyyy")),
        ("<Date-dddd d mmmm yyyy>", local("dddd d mmmm yyyy")),
        ("<CDate-yy>", local("yy")),
        ("<ADate-Short Date>", local("yyyy-mm-dd")),
        // Numbers
        ("<Counter>", "007".to_owned()),
        ("<NumFiles>", "5".to_owned()),
        // Misc
        ("<Ask>", "asked".to_owned()),
        ("<Ask-4>", "asked four".to_owned()),
        ("<Clipboard>", "pasted".to_owned()),
        ("<DetectedExt>", "png".to_owned()),
        ("<\\>", "/".to_owned()),
        // Parts, from the worked example.
        ("<%1>", "01".to_owned()),
        ("<%2>", "Metallica".to_owned()),
        ("<%3>", "S&M".to_owned()),
        ("<%4>", "Nothing Else Matters".to_owned()),
        // Composition, and the Free Format worked example shape.
        ("<Parent>_<FullName>", format!("rock_{name}.png")),
        (
            "<%2> - <%1> - <%4>",
            "Metallica - 01 - Nothing Else Matters".to_owned(),
        ),
    ];

    for (template, expected) in cases {
        assert_eq!(&f.render(template), expected, "tag {template}");
    }
}

/// The tags whose value is not fixed, checked by shape.
#[test]
fn the_remaining_tags_render_something_of_the_right_shape() {
    let f = Fixture::new();

    // 1536 zero-ish bytes with a PNG header — the value does not matter, the
    // shape does.
    let crc = f.render("<Crc32>");
    assert_eq!(crc.len(), 8, "<Crc32> is eight hex digits: {crc}");
    assert!(
        crc.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_lowercase())
    );
    assert_eq!(crc, f.render("<Crc32>"), "and it is stable");

    assert_eq!(f.render("<NowDate>").len(), 10, "yyyy-mm-dd");
    assert_eq!(f.render("<NowTime>").len(), 8, "Hh.Mm.Ss");

    let letter = f.render("<Rnd>");
    assert_eq!(letter.len(), 1);
    assert!(letter.chars().all(|c| c.is_ascii_uppercase()), "{letter}");

    assert_eq!(f.render("<Rnd4>").len(), 4);
    assert!(f.render("<Rnd4>").chars().all(|c| c.is_ascii_digit()));

    let ranged: i64 = f.render("<Rnd2-10-20>").parse().expect("a number");
    assert!((10..=20).contains(&ranged), "{ranged}");
    let padded = f.render("<RndZ4-1-9>");
    assert_eq!(padded.len(), 4, "{padded}");
    assert!(padded.starts_with("000"), "{padded}");

    // The limits truncate whatever the template produced.
    assert_eq!(f.render("<Name><FileMax-2>"), "01");
    assert!(f.render("<Name><PathMax-40>").len() < f.render("<Name>").len());
}

/// D29: nothing renders as a silent empty string.
#[test]
fn a_tag_that_is_not_ours_refuses_to_compile() {
    for body in ["<Nmae>", "<>", "<Left>", "<Parent-0>", "<%0>"] {
        assert!(
            Template::compile(body).is_err(),
            "{body} should not compile"
        );
    }
    // A name inside an implemented family that the family does not know: D29
    // still applies, or a typo becomes an empty string in a filename.
    for body in ["<ID3-Compsoer>", "<ID3-Nonsense>"] {
        assert!(
            Template::compile(body).is_err(),
            "{body} should not compile"
        );
    }
    for (body, needle) in [
        ("<BrMode>", "music"),
        ("<Iptc-Keywords>", "IPTC"),
        ("<ExifTool-ImageWidth>", "ExifTool"),
        ("<PdfPages>", "PDF"),
        ("<DirLength>", "length of the music"),
    ] {
        let err = Template::compile(body).unwrap_err().to_string();
        assert!(err.contains(needle), "{body} said {err:?}");
    }
}

/// `<ExifDate>` and `<ExifTime>`, over a real image.
///
/// Also the first end-to-end coverage the Exif reader has had: M5 built it
/// behind `#[cfg(test)]`, so no integration test could reach the fixture
/// builder and the whole family went untested outside its own module.
#[test]
fn the_exif_date_tags_render_the_date_the_photograph_was_taken() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("photo.jpg");
    std::fs::write(
        &path,
        ren_core::meta::testing::jpeg_with_exif(Some("2008:02:17 11:23:50"), None, None),
    )
    .unwrap();

    let entry = FileEntry::synthetic(&path);
    let run = RunContext::default();
    let render = |template: &str| {
        Template::compile(template)
            .unwrap_or_else(|e| panic!("{template}: {e}"))
            .render(&EvalCx::new(&entry, 0, 1, &run))
            .text
    };

    assert_eq!(render("<ExifDate>"), "2008-02-17");
    assert_eq!(render("<ExifTime>"), "11.23.50");
    // The whole VB6 date mini-language, for free, because it is a `TimeSource`
    // rather than a tag of its own.
    assert_eq!(render("<ExifDate-yyyy>"), "2008");
    assert_eq!(render("<ExifDate-yyyy-mm>"), "2008-02");

    // An image with no Exif date leaves the tag missing rather than empty, so
    // "only rename if all tags are available" can act on it.
    let plain = dir.path().join("plain.jpg");
    std::fs::write(
        &plain,
        ren_core::meta::testing::jpeg_with_exif(None, None, None),
    )
    .unwrap();
    let entry = FileEntry::synthetic(&plain);
    let out = Template::compile("<ExifDate>")
        .unwrap()
        .render(&EvalCx::new(&entry, 0, 1, &run));
    assert!(!out.available());
    assert_eq!(out.missing, ["<ExifDate>"]);
}

/// The music tags, over a real tagged file — the completeness guard extended
/// to the family M6 adds. A tag that parses but was never wired into the
/// resolver shows up here as a single failing row.
#[test]
fn every_music_tag_renders_what_the_reference_says_it_does() {
    let dir = TempDir::new().unwrap();
    let path = ren_core::meta::testing::Mp3::tagged("Metallica", "Nothing Else Matters")
        .frame("TALB", "Metallica")
        .frame("TRCK", "8")
        .frame("TDRC", "1991")
        .frame("TCON", "Metal")
        .frame("COMM", "a comment")
        .frame("TPE2", "Various Artists")
        .frame("TCOM", "Ennio Morricone")
        .write(dir.path(), "track.mp3");
    ren_core::meta::audio::forget_all();

    let entry = FileEntry::synthetic(&path);
    let run = RunContext::default();
    let render = |template: &str| {
        Template::compile(template)
            .unwrap_or_else(|e| panic!("{template}: {e}"))
            .render(&EvalCx::new(&entry, 0, 1, &run))
            .text
    };

    for (template, expected) in [
        ("<Artist>", "Metallica"),
        ("<Title>", "Nothing Else Matters"),
        ("<Album>", "Metallica"),
        ("<Year>", "1991"),
        ("<Genre>", "Metal"),
        ("<Comment>", "a comment"),
        // *"Track Nr., zero-padded"* and *"Track Nr., no zero padding"*.
        ("<Track>", "08"),
        ("<TrackN>", "8"),
        ("<Bitrate>", "128"),
        ("<Freq>", "44100"),
        ("<FreqS>", "44"),
        ("<Stereo>", "Stereo"),
        // The `<ID3-*>` family, by the canonical spelling.
        ("<ID3-AlbumArtist>", "Various Artists"),
        ("<ID3-Composer>", "Ennio Morricone"),
        // And the whole documented style, which is the point of all of it.
        ("<Artist> - <Title>", "Metallica - Nothing Else Matters"),
    ] {
        assert_eq!(render(template), expected, "{template}");
    }

    // The durations come from the audio, so they say something rather than
    // nothing — the exact value depends on how many frames the fixture has.
    assert!(render("<Length>").contains(':'), "{}", render("<Length>"));
    assert!(render("<LengthS>").contains(':'));
}

/// Missing values are reported rather than rendered as nothing, which is what
/// *"only rename if all tags are available"* reads.
#[test]
fn a_tag_with_no_value_is_reported_as_missing() {
    let entry = FileEntry::synthetic("/nowhere/README");
    let run = RunContext::default();
    let out = Template::compile("<Ext><Date><Crc32><Ask><%1>")
        .unwrap()
        .render(&EvalCx::new(&entry, 0, 1, &run));

    assert_eq!(out.text, "");
    assert_eq!(out.missing, ["<Ext>", "<Date>", "<Crc32>", "<Ask>", "<%1>"]);
    assert!(!out.available());
}

fn local(format: &str) -> String {
    ren_core::template::dates::DateFormat::parse(format)
        .render(UNIX_EPOCH + Duration::from_secs(STAMP))
}

/// A sanity check on the fixture itself: if the stamp ever stops resolving,
/// half the table above would silently compare two empty strings.
#[test]
fn the_fixture_timestamp_is_the_manuals_example() {
    let stamp: SystemTime = UNIX_EPOCH + Duration::from_secs(STAMP);
    let utc = ren_core::template::dates::DateFormat::parse("yyyy-mm-dd").render(stamp);
    assert!(utc.starts_with("1977-05-0"), "{utc}");
    assert!(!local("yyyy-mm-dd").is_empty());
}

// --- M8: the image header tags -----------------------------------------------

/// `<Width> <Height> <Depth> <Depthb>` read the *file's own header*, not the
/// Exif block a camera wrote into it — which is why they answer for a PNG,
/// where every `<Exif-…>` returns nothing.
#[test]
fn the_image_header_tags_read_a_png() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("shot.png");
    // 4x7, 8-bit greyscale: 256 colours.
    image::GrayImage::new(4, 7).save(&path).unwrap();
    ren_core::meta::image::forget_all();

    let entry = FileEntry::from_path(&path).unwrap();
    let run = RunContext::build(
        std::slice::from_ref(&entry),
        &RunSettings::default(),
        Answers::default(),
    );
    let cx = EvalCx::new(&entry, 0, 1, &run);

    for (source, expected) in [
        ("<Width>", "4"),
        ("<Height>", "7"),
        ("<Depth>", "256"),
        ("<Depthb>", "8"),
    ] {
        let rendered = Template::compile(source).expect("compiles").render(&cx);
        assert_eq!(rendered.text, expected, "{source}");
        assert!(rendered.missing.is_empty(), "{source}: {rendered:?}");
    }

    // A PNG has no JPEG comment, and that is a missing tag rather than an
    // empty string — the same rule every other absent value follows.
    let rendered = Template::compile("<JpgComment>").unwrap().render(&cx);
    assert_eq!(rendered.missing, ["<JpgComment>"]);
}

/// The names are matched case-insensitively and render back the way the tag
/// picker spells them.
#[test]
fn the_image_tags_round_trip_through_their_canonical_spelling() {
    for (typed, canonical) in [
        ("width", "<Width>"),
        ("HEIGHT", "<Height>"),
        ("Depth", "<Depth>"),
        ("depthb", "<Depthb>"),
        ("JPGCOMMENT", "<JpgComment>"),
    ] {
        let tag = ren_core::Tag::parse(typed).expect("a real tag");
        assert_eq!(tag.to_string(), canonical);
    }
}

// --- M8: the Folders group ---------------------------------------------------

/// The Folders tag group, over a tree built for it.
///
/// The `S` prefix is *"in Folder && Subfolders"*, so each pair is the same
/// question asked twice at different depths.
#[test]
fn the_folder_tags_count_what_is_in_the_folder() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("album");
    std::fs::create_dir_all(root.join("disc2")).unwrap();
    std::fs::write(root.join("b.mp3"), vec![0u8; 300]).unwrap();
    std::fs::write(root.join("a.mp3"), vec![0u8; 700]).unwrap();
    std::fs::write(root.join("disc2/c.mp3"), vec![0u8; 24]).unwrap();
    ren_core::meta::folder::forget_all();

    // Asked *of a file*, the tags describe the folder it is in.
    let path = root.join("a.mp3");
    let entry = FileEntry::from_path(&path).unwrap();
    let run = RunContext::build(
        std::slice::from_ref(&entry),
        &RunSettings::default(),
        Answers::default(),
    );
    let cx = EvalCx::new(&entry, 0, 1, &run);

    for (source, expected) in [
        ("<DirFiles>", "2"),
        ("<SDirFiles>", "3"),
        ("<DirDirs>", "1"),
        ("<SDirDirs>", "1"),
        ("<DirSizeB>", "1000"),
        ("<SDirSizeB>", "1024"),
        ("<DirSizeB-6>", "001000"),
        ("<SDirSize>", "1KB"),
        ("<FirstFileInFolder>", "a"),
        ("<FirstFileInFolder-Ext>", "a.mp3"),
    ] {
        let rendered = ren_core::Template::compile(source)
            .unwrap_or_else(|e| panic!("{source}: {e}"))
            .render(&cx);
        assert_eq!(rendered.text, expected, "{source}");
        assert!(rendered.missing.is_empty(), "{source}: {rendered:?}");
    }
}

/// Asked *of a folder*, the same tags describe that folder rather than its
/// parent — which is the only reading that makes `<DirFiles>` useful on a row
/// that is itself a folder.
#[test]
fn a_folder_row_answers_about_itself() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("album");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.mp3"), vec![0u8; 10]).unwrap();
    std::fs::write(root.join("b.mp3"), vec![0u8; 10]).unwrap();
    ren_core::meta::folder::forget_all();

    let entry = FileEntry::from_path(&root).unwrap();
    assert!(entry.is_dir);
    let run = RunContext::build(
        std::slice::from_ref(&entry),
        &RunSettings::default(),
        Answers::default(),
    );
    let cx = EvalCx::new(&entry, 0, 1, &run);

    assert_eq!(
        ren_core::Template::compile("<DirFiles>")
            .unwrap()
            .render(&cx)
            .text,
        "2"
    );
}

/// The names round-trip through their canonical spelling, and `<DirLength>` is
/// still deferred with a reason rather than reported as unknown.
#[test]
fn the_folder_tags_spell_themselves_back() {
    for (typed, canonical) in [
        ("dirsize", "<DirSize>"),
        ("SDIRSIZEB", "<SDirSizeB>"),
        ("DirSizeB-4", "<DirSizeB-4>"),
        ("sdirfiles", "<SDirFiles>"),
        ("firstfileinfolder-ext", "<FirstFileInFolder-Ext>"),
    ] {
        assert_eq!(
            ren_core::Tag::parse(typed).expect("a real tag").to_string(),
            canonical
        );
    }

    let deferred = ren_core::Tag::parse("DirLength").expect_err("not built");
    assert!(
        matches!(deferred, ren_core::TagError::Deferred { .. }),
        "{deferred:?}"
    );
}

/// `<HtmlTitle>` — a folder of saved pages is a folder of `Untitled-1.html`,
/// and the one useful name for each is inside the file.
#[test]
fn the_html_title_tag_names_a_saved_page() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("Untitled-1.html");
    std::fs::write(
        &path,
        b"<!doctype html><html><head>\n  <title>Tom &amp; Jerry \xe2\x80\x93 1940</title>\n</head>",
    )
    .unwrap();
    ren_core::meta::html::forget_all();

    let entry = FileEntry::from_path(&path).unwrap();
    let run = RunContext::build(
        std::slice::from_ref(&entry),
        &RunSettings::default(),
        Answers::default(),
    );
    let cx = EvalCx::new(&entry, 0, 1, &run);

    let rendered = Template::compile("<HtmlTitle>").unwrap().render(&cx);
    assert_eq!(rendered.text, "Tom & Jerry – 1940");
    assert!(rendered.missing.is_empty());

    // Not an HTML file at all: a missing tag, not an error.
    let other = dir.path().join("notes.txt");
    std::fs::write(&other, b"just text").unwrap();
    let entry = FileEntry::from_path(&other).unwrap();
    let run = RunContext::build(
        std::slice::from_ref(&entry),
        &RunSettings::default(),
        Answers::default(),
    );
    let cx = EvalCx::new(&entry, 0, 1, &run);
    assert_eq!(
        Template::compile("<HtmlTitle>")
            .unwrap()
            .render(&cx)
            .missing,
        ["<HtmlTitle>"]
    );
}
