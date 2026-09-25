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

pub(crate) mod cache;

pub mod audio;
pub mod exif;
pub mod folder;
pub mod html;
pub mod image;
pub mod lyrics3;
pub mod names;
pub mod thumb;
pub mod write;

/// Fixture builders: tagged music files, Exif JPEGs and hostile TIFFs, written
/// byte by byte.
///
/// Behind the `testing` feature rather than `#[cfg(test)]`, because
/// `#[cfg(test)]` is true only for this crate's own unit tests — the
/// integration tests, the bench and the two front ends' tests could not reach
/// them, and the Exif reader once went without an end-to-end test for exactly
/// that reason. The feature is switched on only by dev-dependencies (this
/// crate's own, and ren-cli's and ren-gui's), so a shipped build never
/// compiles the thousand-odd lines of fixture writers here, nor the `image`
/// encoder call one of them makes.
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
