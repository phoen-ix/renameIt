//! Reading metadata out of files.
//!
//! `docs/DESIGN.md` §1 names this module; M5 fills in the Exif date Set Date
//! needs, and M6 adds the music tags beside it.
//!
//! Everything here opens a file. That is allowed from the parallel evaluation
//! pass — `<Crc32>` has always done it — but only behind a process-wide,
//! mtime-keyed cache (P44), because the pass runs once per file per keystroke.
//! M5's note here said the readers must never be reached from that pass, which
//! was true while the only caller was an action describing its intent; M6's
//! `<Artist>` is a *tag*, so it is resolved inside `Template::render` like any
//! other, and the cache is what makes that affordable.

/// How many files a metadata cache holds before it drops everything.
///
/// Above the listing sizes the app is designed for, and that is the whole
/// requirement. These caches clear *wholesale* when full rather than evicting
/// least-recently-used, so a capacity below the working set is worse than
/// useless: a 10 000-file folder against a 1 024-entry cache clears it ten
/// times per pass and serves almost no hits, while still paying to build every
/// key. The preview budget is written for 10 000 files, so the number has to
/// be comfortably past that.
pub const CACHE_CAPACITY: usize = 16_384;

pub mod audio;
pub mod exif;
pub mod folder;
pub mod html;
pub mod image;
pub mod lyrics3;
pub mod names;
pub mod thumb;
pub mod write;

/// Fixture builders, compiled into the library rather than hidden behind
/// `#[cfg(test)]`.
///
/// M5 made `jpeg_with_exif` test-only, and the consequence was that no
/// integration test could reach it — so the Exif reader has had unit coverage
/// and nothing end to end since. A ~200-line byte builder with no dependencies
/// is cheap to carry, and being able to construct a tagged file is useful to
/// anything testing against this crate, not only to this crate's own tests.
#[doc(hidden)]
pub mod testing;
