//! M6's acceptance, over every format the engine dispatches on differently.
//!
//! > *"golden tests: rename-from-tags over the corpus; tagger write→lofty
//! > re-read round-trip per format; untagger leaves audio stream intact
//! > (byte-compare past tag blocks)"*
//!
//! One row per **tag type**, not per container: `meta::write` chooses what to do
//! from `FileType::primary_tag_type()`, so a second Vorbis-comment format tests
//! the same branch twice. Ogg Vorbis is in anyway, because its *container* has
//! behaviour FLAC's does not — lofty hands back a comment tag whether or not one
//! was written, so "there is no tag here" is not a state an Ogg file can be in.
//!
//! Opus, Speex and Musepack reuse a tag type already covered and are recorded as
//! untested-but-supported rather than half-built.

use ren_core::meta::testing::{Flac, M4a, Mp3, OggVorbis, WavPack};
use ren_core::meta::write::{FieldWrite, MusicField, TagKind, remove_tags, write_fields};
use ren_platform::host;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// One entry in the corpus: how to build it, and where its audio sits.
struct Format {
    /// The extension decides whether the operations look at it at all (P56).
    name: &'static str,
    /// A file tagged `artist`/`title`, plus an album to leave alone.
    build: fn(&Path, &str, &str, &str) -> PathBuf,
}

fn mp3(dir: &Path, artist: &str, title: &str, album: &str) -> PathBuf {
    Mp3::tagged(artist, title)
        .frame("TALB", album)
        .write(dir, "track.mp3")
}

fn flac(dir: &Path, artist: &str, title: &str, album: &str) -> PathBuf {
    Flac::tagged(artist, title)
        .field("ALBUM", album)
        .write(dir, "track.flac")
}

fn ogg(dir: &Path, artist: &str, title: &str, album: &str) -> PathBuf {
    OggVorbis::tagged(artist, title)
        .field("ALBUM", album)
        .write(dir, "track.ogg")
}

fn m4a(dir: &Path, artist: &str, title: &str, album: &str) -> PathBuf {
    M4a::tagged(artist, title)
        .field(b"\xa9alb", album)
        .write(dir, "track.m4a")
}

fn wavpack(dir: &Path, artist: &str, title: &str, album: &str) -> PathBuf {
    WavPack::tagged(artist, title)
        .field("Album", album)
        .write(dir, "track.wv")
}

/// Every dispatch path, once each.
const CORPUS: &[Format] = &[
    Format {
        name: "mp3",
        build: mp3,
    },
    Format {
        name: "flac",
        build: flac,
    },
    Format {
        name: "ogg",
        build: ogg,
    },
    Format {
        name: "m4a",
        build: m4a,
    },
    Format {
        name: "wv",
        build: wavpack,
    },
];

fn read(path: &Path) -> std::sync::Arc<ren_core::meta::audio::AudioTags> {
    ren_core::meta::audio::forget_all();
    ren_core::meta::audio::tags_of(path).expect("readable audio")
}

/// The first acceptance criterion: a name built from what is inside the file,
/// on every format.
#[test]
fn a_name_is_built_from_the_tags_of_every_format() {
    for format in CORPUS {
        let dir = TempDir::new().unwrap();
        let path = (format.build)(dir.path(), "Metallica", "One", "And Justice For All");
        ren_core::meta::audio::forget_all();

        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = ren_core::Pipeline::new().with(
            ren_core::Step::Name(Box::new(ren_core::ops::MusicRename::new(
                "<Artist> - <Title>",
            ))),
            ren_core::StepConfig::for_op(&ren_core::OpKind::MusicRename(Default::default())),
        );
        let plan = ren_core::plan(&entries, &pipeline, host().as_ref());

        assert_eq!(plan.items.len(), 1, "{}", format.name);
        assert_eq!(
            plan.items[0].new_name,
            format!("Metallica - One.{}", format.name),
            "{} did not rename from its tags",
            format.name
        );
        let _ = path;
    }
}

/// The second: write, read back through **our** reader, and check that the
/// fields nobody enabled are exactly where they were.
///
/// This is D67's guarantee — lofty's save replaces the whole tag, so the
/// obvious implementation of per-field checkboxes destroys everything the user
/// did not tick. It was proved on MP3 alone until now.
#[test]
fn writing_one_field_leaves_the_others_alone_on_every_format() {
    for format in CORPUS {
        let dir = TempDir::new().unwrap();
        let path = (format.build)(dir.path(), "OldArtist", "OldTitle", "OldAlbum");

        let written = write_fields(
            &path,
            &[FieldWrite {
                field: MusicField::Artist,
                value: "NewArtist".into(),
            }],
            host().as_ref(),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", format.name));
        assert_eq!(written, [MusicField::Artist], "{}", format.name);

        let after = read(&path);
        assert_eq!(
            after.artist.as_deref(),
            Some("NewArtist"),
            "{}",
            format.name
        );
        assert_eq!(
            after.title.as_deref(),
            Some("OldTitle"),
            "{} lost its title",
            format.name
        );
        assert_eq!(
            after.album.as_deref(),
            Some("OldAlbum"),
            "{} lost its album",
            format.name
        );
    }
}

/// And the round trip through every field the tagger offers.
#[test]
fn every_field_round_trips_on_every_format() {
    for format in CORPUS {
        let dir = TempDir::new().unwrap();
        let path = (format.build)(dir.path(), "A", "B", "C");

        write_fields(
            &path,
            &[
                FieldWrite {
                    field: MusicField::Track,
                    value: "4".into(),
                },
                FieldWrite {
                    field: MusicField::Title,
                    value: "One".into(),
                },
                FieldWrite {
                    field: MusicField::Artist,
                    value: "Metallica".into(),
                },
                FieldWrite {
                    field: MusicField::Album,
                    value: "And Justice For All".into(),
                },
                FieldWrite {
                    field: MusicField::Genre,
                    value: "Metal".into(),
                },
                FieldWrite {
                    field: MusicField::Comment,
                    value: "a comment".into(),
                },
            ],
            host().as_ref(),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", format.name));

        let after = read(&path);
        let what = format.name;
        assert_eq!(after.track.as_deref(), Some("4"), "{what}");
        assert_eq!(after.title.as_deref(), Some("One"), "{what}");
        assert_eq!(after.artist.as_deref(), Some("Metallica"), "{what}");
        assert_eq!(
            after.album.as_deref(),
            Some("And Justice For All"),
            "{what}"
        );
        assert_eq!(after.genre.as_deref(), Some("Metal"), "{what}");
        assert_eq!(after.comment.as_deref(), Some("a comment"), "{what}");
    }
}

// --- The hazards step 5 found and only documented ---------------------------
//
// All three are stopped by the *pair* of guards in `remove_tags` — a
// `tag_support` check and a presence check — and either alone is enough. That
// is deliberate for a change nobody can take back, and it is also why each of
// these tests only fails when **both** are removed. Verified by doing exactly
// that: FLAC then fails with lofty's `unreachable!` verbatim, the M4A changes
// on its second pass, and the Ogg is rewritten.

/// Remove Tags offers ID3v1, ID3v2 and Lyrics3, and an M4A carries none of
/// them — so the honest statement is that **an M4A is untouched by anything
/// this operation can be asked for**.
///
/// That matters because of what lofty does if the request gets through: a
/// second empty-tag save deletes the mandatory `hdlr` box and leaves a file
/// lofty itself can no longer parse, every write returning `Ok` the whole way
/// down. The guards are what make that unreachable rather than merely unlikely.
#[test]
fn an_m4a_is_untouched_by_every_kind_remove_tags_offers() {
    let dir = TempDir::new().unwrap();
    let path = M4a::tagged("A", "B").write(dir.path(), "t.m4a");

    let before = std::fs::read(&path).unwrap();
    for _ in 0..2 {
        assert!(
            remove_tags(&path, &TagKind::ALL, host().as_ref())
                .unwrap()
                .is_empty(),
            "an M4A carries none of the three kinds on offer"
        );
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file was rewritten anyway"
    );

    ren_core::meta::audio::forget_all();
    assert!(
        ren_core::meta::audio::tags_of(&path).is_some(),
        "the file is no longer parseable audio"
    );
}

/// Asking to remove an ID3v2 from an Ogg would **wipe its Vorbis comments** and
/// return `Ok(())`: lofty's Ogg writer ignores the tag *type* it is handed and
/// rebuilds the comment header from whatever it was given, so an empty ID3v2
/// tag empties the real one. Nothing about the return value or the file size
/// says so.
#[test]
fn removing_a_tag_an_ogg_cannot_carry_leaves_its_comments_alone() {
    let dir = TempDir::new().unwrap();
    let path = OggVorbis::tagged("Metallica", "One").write(dir.path(), "t.ogg");
    let before = std::fs::read(&path).unwrap();

    let removed = remove_tags(
        &path,
        &[TagKind::Id3v1, TagKind::Id3v2, TagKind::Lyrics3],
        host().as_ref(),
    )
    .unwrap();
    assert!(
        removed.is_empty(),
        "an Ogg carries none of those, so none can be removed"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file was rewritten anyway"
    );
    assert_eq!(read(&path).artist.as_deref(), Some("Metallica"));
}

/// The same request, on the format where it is a **panic** rather than a wipe:
/// an empty tag skips lofty's own writability check — that check is
/// `!is_writable() && !is_empty()`, so an empty one sails past — and reaches a
/// writer that dispatches on the file's format. FLAC's ends in `unreachable!`,
/// which takes the process with it, mid-batch, in the operation with no undo.
#[test]
fn removing_a_tag_a_flac_cannot_carry_does_not_panic() {
    let dir = TempDir::new().unwrap();
    let path = Flac::tagged("Metallica", "One").write(dir.path(), "t.flac");
    let before = std::fs::read(&path).unwrap();

    assert!(
        remove_tags(&path, &[TagKind::Id3v1], host().as_ref())
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A tag write must not reach the audio. On MP3 the whole file past the tag is
/// comparable; on FLAC lofty writes a padding block, so the *file* changes size
/// while the audio region does not — which is why this compares the region
/// rather than the file, and why the fixture knows where its audio starts.
#[test]
fn a_tag_write_never_reaches_the_audio() {
    let dir = TempDir::new().unwrap();

    let flac = Flac::tagged("OldArtist", "OldTitle");
    let path = flac.write(dir.path(), "t.flac");
    let audio = flac.bytes()[flac.audio_offset()..].to_vec();

    write_fields(
        &path,
        &[FieldWrite {
            field: MusicField::Artist,
            value: "NewArtist".into(),
        }],
        host().as_ref(),
    )
    .unwrap();

    let after = std::fs::read(&path).unwrap();
    assert!(
        after.ends_with(&audio),
        "the audio region is not what it was"
    );
    assert_eq!(read(&path).artist.as_deref(), Some("NewArtist"));
}

/// Whether `needle` occurs anywhere in `path`'s bytes.
fn contains(path: &Path, needle: &[u8]) -> bool {
    std::fs::read(path)
        .unwrap()
        .windows(needle.len())
        .any(|w| w == needle)
}

/// D67's guarantee, for what the generic `Tag` has **no name for**.
///
/// lofty turns a Vorbis comment or APE tag into a generic `Tag` by splitting
/// it, and throws away the half its `ItemKey` table cannot map: a rip's
/// `CUESHEET`, a user's own key, an APE cover stored as a binary item. Saving
/// the generic tag back then rebuilds the whole block from what is left. So
/// the write has to keep that remainder itself; checked here with a byte scan,
/// because reading back through the same generic API cannot see a field it
/// never had a name for.
#[test]
fn a_one_field_write_keeps_the_fields_lofty_has_no_name_for() {
    let dir = TempDir::new().unwrap();
    let artist = [FieldWrite {
        field: MusicField::Artist,
        value: "NewArtist".into(),
    }];

    let cover = b"\xFF\xD8\xFF\xE0 not really a jpeg";
    let flac = Flac::tagged("OldArtist", "OldTitle")
        .field("CUESHEET", "FILE rip.wav WAVE")
        .field("MY_OWN_KEY", "kept")
        .cover(cover)
        .write(dir.path(), "rip.flac");
    write_fields(&flac, &artist, host().as_ref()).unwrap();
    assert!(contains(&flac, b"CUESHEET=FILE rip.wav WAVE"), "CUESHEET");
    assert!(contains(&flac, b"MY_OWN_KEY=kept"), "a user's own key");
    assert!(contains(&flac, cover), "the FLAC's picture block");
    assert_eq!(read(&flac).artist.as_deref(), Some("NewArtist"));
    assert_eq!(read(&flac).title.as_deref(), Some("OldTitle"));

    let cover = b"front.jpg\0\xFF\xD8\xFF\xE0 not really a jpeg";
    let wv = WavPack::tagged("OldArtist", "OldTitle")
        .binary("Cover Art (Front)", cover)
        .field("MY_OWN_KEY", "kept")
        .write(dir.path(), "track.wv");
    write_fields(&wv, &artist, host().as_ref()).unwrap();
    assert!(contains(&wv, b"Cover Art (Front)"), "the cover's key");
    assert!(contains(&wv, cover), "the cover itself");
    assert!(contains(&wv, b"MY_OWN_KEY"), "a user's own key");
    assert_eq!(read(&wv).artist.as_deref(), Some("NewArtist"));
    assert_eq!(read(&wv).title.as_deref(), Some("OldTitle"));
}
