//! The decode threads.
//!
//! `viewmodel::preview`'s shape, deliberately, down to the generation counter
//! and the injected `repaint` closure — with one difference that is the whole
//! design. The preview worker **coalesces** to the newest request because only
//! the newest plan matters. Thumbnails are independent per file, so this is a
//! *set*: the view sends the whole visible set, and anything from an older set
//! is dropped unread.
//!
//! ```text
//! scroll ──► generation += 1 ──► one job per visible key
//!                                        │
//!                    N threads take jobs from one queue;
//!                    a job older than the newest generation
//!                    is thrown away without being decoded
//!                                        │
//!  cache ◄── recv ── (key, pixels or a named refusal)
//! ```
//!
//! **One job per key, not one job per set.** A single message carrying four
//! hundred keys would be taken by a single thread and decoded serially while
//! the other three sat idle behind the same mutex — a pool in name only. Per
//! key, whichever thread is free takes the next file, and cancellation stays
//! free because a superseded job is discarded for the price of an atomic load.
//!
//! # Why not rayon
//!
//! `ren_core::pipeline::evaluate_all` is a `par_iter` on the **global** rayon
//! pool, on every keystroke, against a 50 ms budget CI enforces. `image` is
//! built `default-features = false`, so its own rayon feature is off and one
//! decode is one long single-threaded task — precisely the shape that starves a
//! work-stealing pool of the small latency-sensitive work the preview is made
//! of. Type one character while four hundred tiles decode and a 4 ms keystroke
//! becomes a stutter the user cannot explain.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use ren_core::meta::thumb::{NoThumbnail, Thumbnail, thumbnail};

/// What identifies one thumbnail.
///
/// Built from the `FileEntry` the listing already holds — never a fresh
/// `fs::metadata`, which at three hundred visible tiles would be eighteen
/// thousand syscalls a second on the UI thread and invisible to every test.
///
/// `len` and `modified` are in the key, so a file edited on disk is a different
/// thumbnail rather than a stale one. `edge` is in it too: a cache keyed on the
/// path alone hands a 96 px tile back for a 256 px request, and the only
/// symptom is a blurry picture — which is exactly what a harness that cannot
/// see pixels cannot catch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<Duration>,
    pub edge: u32,
}

impl ThumbKey {
    pub fn of(entry: &ren_core::model::FileEntry, edge: u32) -> Self {
        Self {
            path: entry.path.clone(),
            len: entry.size,
            modified: entry
                .modified
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
            edge,
        }
    }

    /// The same file under a different name — what a finished rename produces.
    ///
    /// Everything else is carried over unchanged, which is what makes the move
    /// self-verifying: the next lookup is built from the *new* listing, so if
    /// the run rewrote the bytes as well as the name, the length or the mtime
    /// differs, the lookup misses, and it decodes again. A wrong picture under
    /// a right name cannot happen.
    pub fn renamed_to(&self, path: PathBuf) -> Self {
        Self {
            path,
            ..self.clone()
        }
    }
}

/// One tile's answer: pixels, or a named reason there are none.
pub type Decoded = Result<Arc<Thumbnail>, NoThumbnail>;

struct Job {
    generation: u64,
    key: ThumbKey,
}

struct Answer {
    generation: u64,
    key: ThumbKey,
    decoded: Decoded,
}

pub struct ThumbWorker {
    jobs: Sender<Job>,
    answers: Receiver<Answer>,
    /// Bumped by [`Self::request`]; the threads read it to know a job is stale.
    /// Shared rather than sent, so a thread learns about a newer set without
    /// having to reach the messages that carry it.
    newest: Arc<AtomicU64>,
    generation: u64,
    /// Asked for and not yet delivered. `settle()` waits on this, so it must
    /// reach zero even when the threads die — see [`Self::poll`].
    pending: usize,
    threads: usize,
}

impl std::fmt::Debug for ThumbWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbWorker")
            .field("generation", &self.generation)
            .field("pending", &self.pending)
            .field("threads", &self.threads)
            .finish()
    }
}

/// How many decode threads to run.
///
/// Half the machine, capped. One on a two-core CI runner, which cannot starve
/// the preview; four on a developer box, which fills a screen of 24-megapixel
/// photographs in about a second. The cap matters more than the ratio: a decode
/// may hold 256 MiB while it runs, so four is also the memory ceiling.
fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, 4)
}

impl ThumbWorker {
    /// Spawns the pool. `repaint` is called when a tile lands, so an idle UI
    /// wakes up instead of sitting on a placeholder.
    ///
    /// The view itself must never call `request_repaint` (D26): a frame that
    /// asks to be drawn again while any decode is outstanding never settles,
    /// and `egui_kittest` *panics* rather than warns when it cannot reach a
    /// still frame. Calling `repaint` per result looks wasteful and is not — it
    /// is idempotent within a frame, so four hundred results cost one.
    pub fn spawn(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (answer_tx, answer_rx) = channel::<Answer>();

        let threads = thread_count();
        // One queue read by the whole pool: whichever thread is free takes the
        // next file. `Arc<Mutex<Receiver>>` rather than a channel per thread,
        // because a per-thread channel deals the work out in advance, and then
        // one slow photograph holds up a queue nobody else is allowed to drain.
        let jobs = Arc::new(std::sync::Mutex::new(job_rx));
        let newest = Arc::new(AtomicU64::new(0));
        let repaint = Arc::new(repaint);

        for index in 0..threads {
            let jobs = jobs.clone();
            let answers = answer_tx.clone();
            let newest = newest.clone();
            let repaint = repaint.clone();
            std::thread::Builder::new()
                .name(format!("renameit-thumbs-{index}"))
                .spawn(move || {
                    loop {
                        let job = {
                            // The lock is held across `recv`, which is what
                            // makes the threads take turns, and released before
                            // the decode, which is the part that takes time.
                            let Ok(queue) = jobs.lock() else { return };
                            match queue.recv() {
                                Ok(job) => job,
                                Err(_) => return, // The UI is gone.
                            }
                        };
                        // The user has scrolled since this was asked for. An
                        // abandoned job costs an atomic load rather than a
                        // decode, which is what lets a fast scroll past three
                        // thousand pictures cost nothing.
                        if job.generation < newest.load(Ordering::Acquire) {
                            continue;
                        }

                        let decoded = thumbnail(&job.key.path, job.key.edge).map(Arc::new);
                        let answer = Answer {
                            generation: job.generation,
                            key: job.key,
                            decoded,
                        };
                        if answers.send(answer).is_err() {
                            return;
                        }
                        repaint();
                    }
                })
                .expect("a thumbnail decode thread should start");
        }

        Self {
            jobs: job_tx,
            answers: answer_rx,
            newest,
            generation: 0,
            pending: 0,
            threads,
        }
    }

    /// Asks for a set of thumbnails, abandoning anything older.
    ///
    /// The caller sends what is **visible and not already held**. An empty set
    /// is not a request: a folder of text files would otherwise bump the
    /// generation on every frame and cancel work that nothing replaced.
    pub fn request(&mut self, wanted: Vec<ThumbKey>) {
        if wanted.is_empty() {
            return;
        }
        self.generation += 1;
        // Published *before* the jobs are queued, so a thread that is between
        // files sees the new generation at once rather than after draining
        // whatever is ahead of it in the queue.
        self.newest.store(self.generation, Ordering::Release);
        self.pending = wanted.len();
        for key in wanted {
            if self
                .jobs
                .send(Job {
                    generation: self.generation,
                    key,
                })
                .is_err()
            {
                // The pool is gone. Nothing will arrive, so nothing may be
                // waited on — otherwise every later `settle()` costs its full
                // ten-second deadline.
                self.pending = 0;
                return;
            }
        }
    }

    /// Takes delivery of whatever the threads finished.
    ///
    /// Needs no `egui::Context` — decoded pixels are handed straight back and
    /// the *upload* happens where there is a `Ui`. That is what lets
    /// `RenameItApp::settle()`, which has no context, wait for this.
    pub fn poll(&mut self) -> Vec<(ThumbKey, Decoded)> {
        let mut out = Vec::new();
        loop {
            match self.answers.try_recv() {
                Ok(answer) => {
                    if answer.generation == self.generation {
                        self.pending = self.pending.saturating_sub(1);
                    }
                    // An answer from an older generation is not wrong, only
                    // unasked-for: it is keyed by identity, so if the file is
                    // still listed it is exactly the picture wanted, and if it
                    // is not, nothing will ever look it up.
                    out.push((answer.key, answer.decoded));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Every thread died. Draining `pending` here is what stops
                    // that from becoming a ten-second stall per `settle()`.
                    self.pending = 0;
                    break;
                }
            }
        }
        out
    }

    /// How many tiles of the current request have not arrived.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// How many times the pool has been asked for a set.
    ///
    /// Every bump abandons whatever the last one asked for, so this is the
    /// count of cancellations as much as of requests — which is what makes it
    /// the thing to assert on when the question is "did a still viewport ask
    /// twice".
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Blocks until the current request is answered.
    ///
    /// Only for tests. The UI must never wait on a decode — that is what the
    /// placeholder is for.
    #[cfg(test)]
    fn drain(&mut self, timeout: Duration) -> Vec<(ThumbKey, Decoded)> {
        let deadline = std::time::Instant::now() + timeout;
        let mut out = Vec::new();
        while self.pending() > 0 && std::time::Instant::now() < deadline {
            out.extend(self.poll());
            std::thread::sleep(Duration::from_millis(1));
        }
        out.extend(self.poll());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn key(path: &std::path::Path, edge: u32) -> ThumbKey {
        ThumbKey {
            path: path.to_path_buf(),
            len: 0,
            modified: None,
            edge,
        }
    }

    /// A real, decodable JPEG of exactly these dimensions.
    ///
    /// `ren_core::meta::testing` is compiled into the library rather than
    /// hidden behind `#[cfg(test)]` for precisely this — so a crate testing
    /// *against* `ren-core` can build a picture without taking on `image` as a
    /// dev-dependency it would otherwise have no reason to carry.
    fn picture(dir: &std::path::Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        let bytes = ren_core::meta::testing::image::jpeg_rotated(width, height, 1);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_request_comes_back_as_pixels() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg", 80, 40);

        let mut worker = ThumbWorker::spawn(|| {});
        worker.request(vec![key(&path, 32)]);
        let landed = worker.drain(Duration::from_secs(10));

        assert_eq!(landed.len(), 1);
        let thumb = landed[0].1.as_ref().expect("a readable png");
        assert_eq!((thumb.width, thumb.height), (32, 16));
        assert_eq!(worker.pending(), 0);
    }

    /// A file that cannot be decoded still answers. If it did not, the tile
    /// that asked for it would wait forever and `settle()` would wait with it —
    /// which is a ten-second stall in every test whose fixture holds one bad
    /// file, not a missing picture.
    #[test]
    fn an_unreadable_file_answers_rather_than_hanging_the_batch() {
        let dir = tempfile::TempDir::new().unwrap();
        let good = picture(dir.path(), "good.jpg", 8, 8);
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"not a picture at all").unwrap();

        let mut worker = ThumbWorker::spawn(|| {});
        worker.request(vec![key(&bad, 32), key(&good, 32)]);
        let landed = worker.drain(Duration::from_secs(10));

        assert_eq!(landed.len(), 2, "both answered");
        assert_eq!(worker.pending(), 0);
        assert!(landed.iter().any(|(_, d)| d.is_err()));
        assert!(landed.iter().any(|(_, d)| d.is_ok()));
    }

    /// The repaint hook is how an idle UI learns a tile arrived. Without it the
    /// picture appears only when something else happens to cause a frame — a
    /// mouse moved across the window, say.
    #[test]
    fn the_repaint_hook_fires_when_a_tile_lands() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg", 8, 8);

        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let mut worker = ThumbWorker::spawn(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        worker.request(vec![key(&path, 32)]);
        worker.drain(Duration::from_secs(10));

        assert!(count.load(Ordering::Relaxed) >= 1);
    }

    /// An empty set must not bump the generation: a folder of text files would
    /// otherwise cancel, on every frame, work that nothing replaced.
    #[test]
    fn asking_for_nothing_is_not_a_request() {
        let mut worker = ThumbWorker::spawn(|| {});
        worker.request(Vec::new());
        assert_eq!(worker.pending(), 0);
        assert_eq!(worker.generation, 0);
    }

    /// The property that keeps a fast scroll from decoding everything it went
    /// past: only the newest set is waited on, and the older set's jobs are
    /// thrown away rather than decoded.
    #[test]
    fn a_newer_request_supersedes_the_one_before_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg", 400, 400);

        let mut worker = ThumbWorker::spawn(|| {});
        worker.request((0..200).map(|_| key(&path, 32)).collect());
        worker.request(vec![key(&path, 64)]);
        assert_eq!(worker.pending(), 1, "the older set is no longer waited on");

        let landed = worker.drain(Duration::from_secs(10));
        assert_eq!(worker.pending(), 0);
        assert!(
            landed.iter().any(|(k, _)| k.edge == 64),
            "the newest set is the one that answered"
        );
        assert!(
            landed.len() < 200,
            "{} of 201 jobs ran — the abandoned set was decoded anyway",
            landed.len()
        );
    }

    /// Four hundred tiles must not queue behind one thread. Whichever thread is
    /// free takes the next file, so twelve pictures reach twelve answers even
    /// when some of them are slow.
    #[test]
    fn every_thread_draws_from_the_same_queue() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths: Vec<_> = (0..12)
            .map(|i| picture(dir.path(), &format!("p{i}.jpg"), 600, 600))
            .collect();

        let mut worker = ThumbWorker::spawn(|| {});
        worker.request(paths.iter().map(|p| key(p, 96)).collect());
        let landed = worker.drain(Duration::from_secs(30));

        assert_eq!(landed.len(), 12);
        assert!(landed.iter().all(|(_, d)| d.is_ok()));
        assert_eq!(worker.pending(), 0);
    }

    #[test]
    fn the_pool_is_at_least_one_thread_and_never_more_than_four() {
        let worker = ThumbWorker::spawn(|| {});
        assert!((1..=4).contains(&worker.threads()));
    }

    /// `settle()` waits on `pending`, so a pool that has gone away must drain it
    /// rather than leave every later call to burn its full deadline.
    #[test]
    fn a_pool_that_has_gone_away_does_not_stall_the_caller() {
        let mut worker = ThumbWorker::spawn(|| {});
        // Replacing the sending end drops the one the threads are reading, so
        // the send below has nowhere to go.
        let (dead_jobs, _) = channel::<Job>();
        worker.jobs = dead_jobs;
        worker.request(vec![key(std::path::Path::new("/nowhere.png"), 32)]);

        assert_eq!(worker.pending(), 0, "nothing is coming, so nothing is due");
    }

    /// A rename gives the same picture a new name.
    #[test]
    fn a_key_can_follow_a_file_to_its_new_name() {
        let original = key(std::path::Path::new("/a/one.png"), 96);
        let renamed = original.renamed_to("/a/two.png".into());
        assert_eq!(renamed.path, std::path::Path::new("/a/two.png"));
        assert_eq!(renamed.len, original.len);
        assert_eq!(renamed.edge, original.edge);
        assert_ne!(renamed, original);
    }

    /// The listing already holds everything the key needs, and reaching for the
    /// disk again would be three hundred `stat` calls a frame.
    #[test]
    fn a_key_is_built_from_the_listing_not_from_the_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = picture(dir.path(), "a.jpg", 8, 8);
        let entry = ren_core::model::FileEntry::from_path(&path).unwrap();

        let small = ThumbKey::of(&entry, 96);
        let large = ThumbKey::of(&entry, 256);
        assert_eq!(small.len, entry.size);
        assert!(small.modified.is_some());
        assert_ne!(small, large, "two sizes are two thumbnails");
    }
}
