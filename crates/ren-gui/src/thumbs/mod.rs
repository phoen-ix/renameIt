//! Thumbnails: decoded off the UI thread, held under a byte budget, drawn as
//! textures.
//!
//! Three pieces, split by what each is allowed to know:
//!
//! * [`cache`] — a least-recently-used cache bounded by cost. Generic, so its
//!   eviction rules are tested with integers and no egui.
//! * [`worker`] — the decode threads. No egui either, so the protocol is tested
//!   as plain Rust.
//! * [`Thumbs`] — the two joined together, and the only place that touches an
//!   `egui::Context`, because a `TextureHandle` needs one to make and its
//!   `Drop` is what gives the memory back.
//!
//! The decoding itself is `ren_core::meta::thumb`, which is where the `image`
//! dependency lives — this crate never learns what a `DynamicImage` is.

pub mod cache;
pub mod worker;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use cache::CostCache;
use ren_core::meta::thumb::NoThumbnail;
pub use worker::{Decoded, ThumbKey, ThumbWorker};

/// What the cache holds for one file.
///
/// **A refusal is an entry**, and that is the whole reason this is an enum
/// rather than an `Option<TextureHandle>`. Not caching a failure is not
/// "slightly wasteful": the next frame finds no entry, asks again, fails again,
/// and does so forever. It presents as `Harness::run` panicking — it panics
/// above four immediate repaints, it does not warn — and as `settle()` burning
/// its full ten-second deadline in every test whose fixture holds one file that
/// is not a picture.
#[derive(Clone)]
pub enum Tile {
    Ready {
        texture: egui::TextureHandle,
        /// The decoded size, which is what the tile is drawn at. Read from the
        /// handle would mean reaching into the texture manager every frame.
        size: [u32; 2],
    },
    /// Why there is no picture, in the vocabulary the placeholder uses.
    None(NoThumbnail),
}

impl Tile {
    /// What this costs the budget.
    ///
    /// RGBA on the GPU, which is what `load_texture` uploads. A refusal costs
    /// nothing here and is still charged the cache's own per-entry floor.
    pub fn bytes(&self) -> usize {
        match self {
            Self::Ready { size, .. } => size[0] as usize * size[1] as usize * 4,
            Self::None(_) => 0,
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        match self {
            Self::Ready { texture, .. } => Some(texture),
            Self::None(_) => None,
        }
    }
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready { size, .. } => write!(f, "Ready({}×{})", size[0], size[1]),
            Self::None(why) => write!(f, "None({why:?})"),
        }
    }
}

/// The sizes anything is ever decoded at.
///
/// The slider is continuous and the decode is not, deliberately. Decoding at
/// whatever the slider currently reads would re-decode the whole folder once
/// per pixel of travel while the user drags it — hundreds of decodes to answer
/// a question the user is still asking.
pub const BUCKETS: [u32; 3] = [96, 160, 256];

/// The bucket to decode at for a tile of `pixels` across.
///
/// Always the smallest bucket **at least** as large as the tile, so the draw is
/// a downscale rather than a magnification of something smaller. That is also
/// what makes it right on a 2× display, where a 96-point tile is 192 pixels and
/// asking for 96 would show a picture visibly softer than the one beside it.
///
/// Above the largest bucket it saturates: a 256-pixel thumbnail scaled up is
/// still better than holding a screenful of half-megabyte textures.
pub fn bucket(pixels: u32) -> u32 {
    BUCKETS
        .iter()
        .copied()
        .find(|&edge| edge >= pixels)
        .unwrap_or(BUCKETS[BUCKETS.len() - 1])
}

/// The texture budget.
///
/// Derived rather than picked round: three screenfuls at the largest decode
/// bucket on a 1440p window is a little under 60 MiB, so 64 MiB is one
/// screenful either side of what is on show.
pub const BUDGET: usize = 64 * 1024 * 1024;

/// The most the budget may be raised to for a viewport that cannot fit inside
/// it. Past this the machine is better served by re-decoding than by holding.
pub const CEILING: usize = 256 * 1024 * 1024;

/// The decode threads and the texture cache, joined.
///
/// The protocol is one call each way per frame:
///
/// ```text
/// begin_frame()  ─►  poll()  ─►  tile(ctx, key) …  ─►  want(visible)
/// ```
///
/// `poll` before `want` is not arbitrary. A tile that has just arrived must
/// reach the cache before the visible set is recomputed, or it is still
/// "missing", is asked for again, and the same picture is decoded twice.
pub struct Thumbs {
    worker: ThumbWorker,
    cache: CostCache<ThumbKey, Tile>,
    /// Decoded, not yet uploaded. Pixels cannot become a texture without an
    /// `egui::Context`, and `poll` deliberately has none — that is what lets
    /// `RenameItApp::settle()`, which has no context either, wait for a decode.
    ///
    /// Bounded twice over: only keys of the current request can land here, and
    /// [`Self::begin_frame`] drops anything a frame went by without using.
    staged: HashMap<ThumbKey, (u64, Decoded)>,
    /// Asked for and not yet answered. Without it a static viewport re-asks for
    /// the same files on every frame, which cancels the work in flight and
    /// starts it again sixty times a second — a decode that never finishes.
    outstanding: HashSet<ThumbKey>,
    /// Keys handed to the threads, ever.
    ///
    /// The instrument for "did this view ask for the whole folder or only the
    /// part of it on screen". `len()` cannot answer that — it counts *uploaded*
    /// tiles, and only what is drawn is uploaded, so a view that decoded ten
    /// thousand pictures and drew eight looks exactly like one that decoded
    /// eight. `ren_core::meta::thumb::decodes_so_far` cannot either: it is
    /// process-wide, and `cargo test` runs these in parallel.
    asked: usize,
    frame: u64,
}

impl std::fmt::Debug for Thumbs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbs")
            .field("worker", &self.worker)
            .field("cache", &self.cache)
            .field("staged", &self.staged.len())
            .field("outstanding", &self.outstanding.len())
            .finish()
    }
}

impl Thumbs {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            worker: ThumbWorker::spawn(repaint),
            cache: CostCache::new(BUDGET, Tile::bytes),
            staged: HashMap::new(),
            outstanding: HashSet::new(),
            asked: 0,
            frame: 0,
        }
    }

    /// Starts a frame: protects what is about to be drawn from eviction, and
    /// drops pixels a whole frame went by without wanting.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.cache.begin_frame();
        // One frame of grace, not none: the results of a decode land between
        // frames, so anything staged during frame N is drawn in frame N+1.
        let frame = self.frame;
        self.staged
            .retain(|_, (staged_at, _)| *staged_at + 1 >= frame);
    }

    /// Takes delivery of anything the threads finished. Needs no context.
    pub fn poll(&mut self) {
        for (key, decoded) in self.worker.poll() {
            self.outstanding.remove(&key);
            self.staged.insert(key, (self.frame, decoded));
        }
    }

    /// Asks for whatever is visible and not already held.
    ///
    /// The set is sent whole, so scrolling away cancels rather than queues. It
    /// is sent only when it names something not already coming: re-sending an
    /// identical set every frame would cancel the work in flight and restart
    /// it, and nothing would ever finish.
    pub fn want(&mut self, visible: impl IntoIterator<Item = ThumbKey>) {
        let visible: Vec<ThumbKey> = visible.into_iter().collect();
        // Room for everything on screen at once, up to the ceiling. Without it,
        // a window whose visible tiles alone exceed the budget evicts the tiles
        // it has not drawn yet in order to make room for the ones it has — and
        // re-decodes them next frame, forever. It only ever grows, so a window
        // maximised once keeps the larger budget for the session; that is a
        // bounded 256 MiB, and the alternative is a budget that shrinks under a
        // viewport still using it.
        let needed: usize = visible
            .iter()
            .map(|key| (key.edge as usize).pow(2) * 4 + 256)
            .sum();
        self.cache.reserve_for(needed, CEILING);

        let missing: Vec<ThumbKey> = visible
            .into_iter()
            .filter(|key| !self.cache.contains(key) && !self.staged.contains_key(key))
            .collect();
        if missing.iter().all(|key| self.outstanding.contains(key)) {
            return; // Everything wanted is already on its way.
        }
        self.outstanding = missing.iter().cloned().collect();
        self.asked += missing.len();
        self.worker.request(missing);
    }

    /// The tile for a key, uploading it if this is the frame it arrived on.
    ///
    /// `&mut` because a read is what marks an entry recently used — a cache
    /// that did not would evict whatever was inserted longest ago, regardless
    /// of what is on screen.
    pub fn tile(&mut self, ctx: &egui::Context, key: &ThumbKey) -> Option<&Tile> {
        if !self.cache.contains(key)
            && let Some((_, decoded)) = self.staged.remove(key)
        {
            let tile = match decoded {
                Ok(thumb) => {
                    let size = [thumb.width as usize, thumb.height as usize];
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, &thumb.rgba);
                    Tile::Ready {
                        texture: ctx.load_texture(
                            format!("thumb:{}@{}", key.path.display(), key.edge),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                        size: [thumb.width, thumb.height],
                    }
                }
                Err(why) => Tile::None(why),
            };
            self.cache.insert(key.clone(), tile);
        }
        self.cache.get(key)
    }

    /// How many tiles of the current request have not arrived. `settle()` waits
    /// on this.
    pub fn pending(&self) -> usize {
        self.worker.pending()
    }

    /// How many times the threads have been asked for a set.
    ///
    /// Each one abandons the last, so a viewport that has not moved must not
    /// raise it — see [`Self::want`].
    pub fn requests(&self) -> u64 {
        self.worker.generation()
    }

    /// How many pictures have been asked for, in total.
    ///
    /// What bounds the work: a view that hands over its whole listing rather
    /// than its viewport shows up here and nowhere else.
    pub fn asked(&self) -> usize {
        self.asked
    }

    /// F9's half of D140: forget every picture and read them all again.
    ///
    /// The listing's own refresh must **not** do this. It runs after every
    /// rename and after every undo, where clearing would undo the rekey below
    /// and blank the whole view on the app's commonest workflow.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.staged.clear();
        self.outstanding.clear();
    }

    /// Follows the files a run or an undo moved.
    ///
    /// Renaming four hundred photographs must not re-decode four hundred
    /// photographs. The move is self-verifying: the next lookup is built from
    /// the new listing, so a run that rewrote the bytes as well as the name
    /// gives a key with a different length or mtime, misses, and decodes — see
    /// [`ThumbKey::renamed_to`].
    pub fn renamed(&mut self, moves: &[(PathBuf, PathBuf)]) {
        if moves.is_empty() {
            return;
        }
        let destination: HashMap<&PathBuf, &PathBuf> =
            moves.iter().map(|(from, to)| (from, to)).collect();
        for key in self.cache.keys() {
            if let Some(to) = destination.get(&key.path) {
                let moved = key.renamed_to((*to).clone());
                self.cache.rekey(&key, moved);
            }
        }
        // Anything in flight would answer under a name that no longer exists,
        // and nothing would ever look it up. Forgetting the request is what
        // lets the next frame ask again under the new one.
        self.outstanding.clear();
    }

    /// What the cache holds, in bytes — for the tests and the spike harness.
    pub fn bytes(&self) -> usize {
        self.cache.bytes()
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Entries dropped to stay under budget, for the performance harness.
    pub fn evictions(&self) -> u64 {
        self.cache.evictions()
    }

    /// What the cache is currently allowed to hold.
    pub fn budget(&self) -> usize {
        self.cache.budget()
    }

    /// Blocks until the current request is answered.
    ///
    /// Only for tests and for `RenameItApp::settle()`. The UI itself must never
    /// wait on a decode — the placeholder is what it does instead.
    pub fn wait(&mut self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while self.pending() > 0 && std::time::Instant::now() < deadline {
            self.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn picture(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let bytes = ren_core::meta::testing::image::jpeg_rotated(40, 20, 1);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn key(path: &std::path::Path) -> ThumbKey {
        ThumbKey {
            path: path.to_path_buf(),
            len: 0,
            modified: None,
            edge: 96,
        }
    }

    fn thumbs() -> Thumbs {
        Thumbs::new(|| {})
    }

    #[test]
    fn a_wanted_picture_becomes_a_tile() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg");
        let ctx = egui::Context::default();

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([key(&path)]);
        thumbs.wait(Duration::from_secs(10));

        let tile = thumbs.tile(&ctx, &key(&path)).expect("a decoded tile");
        assert!(matches!(tile, Tile::Ready { size, .. } if *size == [40, 20]));
        assert_eq!(thumbs.bytes(), 40 * 20 * 4 + 256);
    }

    /// The one that keeps `Harness::run` from panicking: a file that is not a
    /// picture must occupy an entry, or every frame asks again and the frame
    /// after that asks again.
    #[test]
    fn a_file_that_is_not_a_picture_is_remembered_as_one_that_is_not() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"plain text").unwrap();
        let ctx = egui::Context::default();

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([key(&path)]);
        thumbs.wait(Duration::from_secs(10));
        assert!(matches!(
            thumbs.tile(&ctx, &key(&path)),
            Some(Tile::None(_))
        ));

        // The next frame asks for the same thing and must ask the workers for
        // nothing at all.
        thumbs.begin_frame();
        thumbs.want([key(&path)]);
        assert_eq!(thumbs.pending(), 0, "a refusal is an answer, not a retry");
        assert_eq!(thumbs.len(), 1);
    }

    /// A viewport that has not moved re-sends the same set on every frame. Each
    /// request abandons the one before it, so counting those as new requests
    /// would cancel the work in flight and start it over sixty times a second —
    /// and the tiles would arrive a frame at a time, or on a slow disk not at
    /// all. Asserted as the request count rather than as elapsed time, because
    /// a timing assertion here would only mean "this machine was fast today".
    #[test]
    fn asking_twice_for_what_is_already_coming_does_not_restart_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths: Vec<_> = (0..4)
            .map(|i| picture(dir.path(), &format!("p{i}.jpg")))
            .collect();
        let wanted: Vec<_> = paths.iter().map(|p| key(p)).collect();

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want(wanted.clone());
        assert_eq!(thumbs.pending(), 4);
        assert_eq!(thumbs.requests(), 1);

        // Ten frames with nothing polled, so nothing has arrived and every key
        // is still missing — the exact state a re-request would be made in.
        for _ in 0..10 {
            thumbs.begin_frame();
            thumbs.want(wanted.clone());
        }
        assert_eq!(thumbs.requests(), 1, "a still viewport asked once");

        thumbs.wait(Duration::from_secs(10));
        thumbs.begin_frame();
        thumbs.want(wanted.clone());
        assert_eq!(thumbs.requests(), 1, "and does not ask again once held");

        let ctx = egui::Context::default();
        for key in &wanted {
            assert!(thumbs.tile(&ctx, key).is_some(), "{key:?} never arrived");
        }
    }

    /// Renaming four hundred photographs must not re-decode four hundred
    /// photographs.
    #[test]
    fn a_rename_moves_the_tile_rather_than_decoding_it_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let before = picture(dir.path(), "before.jpg");
        let after = dir.path().join("after.jpg");
        let ctx = egui::Context::default();

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([key(&before)]);
        thumbs.wait(Duration::from_secs(10));
        thumbs.tile(&ctx, &key(&before)).expect("decoded once");
        let bytes = thumbs.bytes();

        std::fs::rename(&before, &after).unwrap();
        thumbs.renamed(&[(before.clone(), after.clone())]);

        thumbs.begin_frame();
        assert!(
            thumbs.tile(&ctx, &key(&after)).is_some(),
            "the picture followed its file"
        );
        assert!(thumbs.tile(&ctx, &key(&before)).is_none());
        assert_eq!(thumbs.bytes(), bytes, "a move, not a second decode");
        assert_eq!(thumbs.len(), 1);
    }

    /// A run that rewrote the bytes as well as the name must not show the old
    /// picture under the new one. Nothing checks for that — the key does.
    #[test]
    fn a_rename_that_also_changed_the_file_misses_and_decodes_again() {
        let moved = ThumbKey {
            path: "/a/one.jpg".into(),
            len: 100,
            modified: None,
            edge: 96,
        }
        .renamed_to("/a/two.jpg".into());
        let looked_up = ThumbKey {
            path: "/a/two.jpg".into(),
            len: 120, // The run rewrote it.
            modified: None,
            edge: 96,
        };
        assert_ne!(moved, looked_up);
    }

    /// F9 means "read the disk again". Keeping the pictures would make it the
    /// one refresh that refreshes everything except what the user is looking at.
    #[test]
    fn clearing_forgets_every_picture() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg");
        let ctx = egui::Context::default();

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([key(&path)]);
        thumbs.wait(Duration::from_secs(10));
        thumbs.tile(&ctx, &key(&path)).unwrap();
        assert!(!thumbs.is_empty());

        thumbs.clear();
        assert!(thumbs.is_empty());
        assert_eq!(thumbs.bytes(), 0);
    }

    /// Pixels that arrived for a tile nobody drew are megabytes with no owner.
    #[test]
    fn pixels_nothing_drew_are_dropped_rather_than_held() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg");

        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([key(&path)]);
        thumbs.wait(Duration::from_secs(10));
        assert_eq!(thumbs.staged.len(), 1);

        thumbs.begin_frame(); // The frame that could have drawn it.
        assert_eq!(thumbs.staged.len(), 1, "one frame of grace");
        thumbs.begin_frame();
        assert!(thumbs.staged.is_empty(), "and no more than one");
    }

    /// A window big enough that its own tiles will not fit in the budget must
    /// raise the budget, not evict the tiles it has yet to draw in order to
    /// make room for the ones it has — which it would then re-decode next
    /// frame, and the frame after that.
    #[test]
    fn a_viewport_larger_than_the_budget_raises_it() {
        let mut thumbs = thumbs();
        assert_eq!(thumbs.budget(), BUDGET);

        // 300 tiles at the largest bucket is a little over 75 MiB of RGBA —
        // more than the budget, less than the ceiling. The files need not
        // exist: what is being asserted is the arithmetic on the way in.
        let wanted: Vec<_> = (0..300)
            .map(|i| ThumbKey {
                path: format!("/nowhere/{i}.jpg").into(),
                len: 0,
                modified: None,
                edge: 256,
            })
            .collect();
        thumbs.begin_frame();
        thumbs.want(wanted);

        assert!(thumbs.budget() > BUDGET, "{}", thumbs.budget());
        assert!(thumbs.budget() <= CEILING);
    }

    /// Dragging the slider must not re-decode the folder once per pixel of
    /// travel, and the picture drawn must never be smaller than the tile.
    #[test]
    fn a_tile_is_decoded_at_the_bucket_above_its_size() {
        assert_eq!(bucket(1), 96);
        assert_eq!(bucket(96), 96, "exactly a bucket is that bucket");
        assert_eq!(bucket(97), 160);
        assert_eq!(bucket(256), 256);
        assert_eq!(bucket(4000), 256, "and it saturates rather than growing");

        // The whole of the slider's range costs three decodes, not two hundred.
        let asked: std::collections::BTreeSet<u32> = (crate::viewmodel::THUMB_MIN
            ..=crate::viewmodel::THUMB_MAX)
            .map(bucket)
            .collect();
        assert_eq!(asked.len(), BUCKETS.len());
    }

    /// A folder with no pictures in it must not keep the threads busy.
    #[test]
    fn a_folder_of_text_files_asks_for_nothing() {
        let mut thumbs = thumbs();
        thumbs.begin_frame();
        thumbs.want([]);
        assert_eq!(thumbs.pending(), 0);
    }
}
