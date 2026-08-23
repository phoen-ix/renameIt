//! The preview worker.
//!
//! `docs/DESIGN.md` Part 2 §2 calls this "an architectural commitment, not an
//! optimization to defer": a naive on-UI-thread recompute visibly stutters, and
//! by then the whole app is built around it.
//!
//! ```text
//! UI edit ──► generation += 1 ──► send (generation, entries, pipeline)
//!                                             │
//!                            worker: coalesce, then plan() using rayon
//!                                             │
//! UI paints the latest cache ◄── recv ── drop anything stale
//! ```
//!
//! The UI never blocks: it asks for a plan and keeps painting whatever it has.
//! Spike B measured the work itself at 4 ms for 10 000 rows
//! (`docs/spikes/preview-perf.md`), so this is about *never* stuttering rather
//! than about the average case.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ren_core::model::FileEntry;
use ren_core::{Pipeline, Plan, plan};
use ren_platform::Platform;

struct Request {
    generation: u64,
    entries: Arc<Vec<FileEntry>>,
    pipeline: Arc<Pipeline>,
}

struct Response {
    generation: u64,
    plan: Plan,
    elapsed: Duration,
}

/// The most recent plan the worker has delivered.
#[derive(Debug)]
pub struct Ready {
    pub generation: u64,
    pub plan: Plan,
    /// How long the worker took, for the status bar.
    pub elapsed: Duration,
}

pub struct PreviewWorker {
    requests: Sender<Request>,
    responses: Receiver<Response>,
    /// Bumped on every request; a response carrying anything older is dropped.
    generation: u64,
    ready: Option<Ready>,
}

impl std::fmt::Debug for PreviewWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewWorker")
            .field("generation", &self.generation)
            .field("ready", &self.ready.as_ref().map(|r| r.generation))
            .finish()
    }
}

impl PreviewWorker {
    /// Spawns the worker. `repaint` is called when a result lands, so an idle
    /// UI wakes up instead of sleeping on a stale preview.
    pub fn spawn(platform: Arc<dyn Platform>, repaint: impl Fn() + Send + 'static) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (response_tx, response_rx) = channel::<Response>();

        std::thread::Builder::new()
            .name("renameit-preview".into())
            .spawn(move || {
                while let Ok(mut request) = request_rx.recv() {
                    // Coalesce: while the user was typing, several requests may
                    // have queued. Only the last one is worth computing.
                    loop {
                        match request_rx.try_recv() {
                            Ok(newer) => request = newer,
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }

                    let started = Instant::now();
                    let computed = plan(&request.entries, &request.pipeline, platform.as_ref());
                    let response = Response {
                        generation: request.generation,
                        plan: computed,
                        elapsed: started.elapsed(),
                    };
                    if response_tx.send(response).is_err() {
                        return; // The UI is gone.
                    }
                    repaint();
                }
            })
            .expect("the preview worker thread should start");

        Self {
            requests: request_tx,
            responses: response_rx,
            generation: 0,
            ready: None,
        }
    }

    /// Asks for a fresh plan. Cheap — the work happens on the worker.
    pub fn request(&mut self, entries: Arc<Vec<FileEntry>>, pipeline: Arc<Pipeline>) {
        self.generation += 1;
        // A closed channel means the worker died; the UI keeps its last plan
        // rather than panicking mid-frame.
        let _ = self.requests.send(Request {
            generation: self.generation,
            entries,
            pipeline,
        });
    }

    /// Takes delivery of anything the worker has finished.
    ///
    /// Returns true if the cached plan changed, which is the UI's cue to
    /// recompute derived state such as diff spans.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(response) = self.responses.try_recv() {
            // A response older than one we already have is stale — the user has
            // typed since.
            let newer = self
                .ready
                .as_ref()
                .is_none_or(|r| response.generation > r.generation);
            if newer {
                self.ready = Some(Ready {
                    generation: response.generation,
                    plan: response.plan,
                    elapsed: response.elapsed,
                });
                changed = true;
            }
        }
        changed
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.ready.as_ref().map(|r| &r.plan)
    }

    pub fn ready(&self) -> Option<&Ready> {
        self.ready.as_ref()
    }

    /// True when a newer request is still in flight, so the visible preview is
    /// one or more keystrokes behind.
    pub fn is_stale(&self) -> bool {
        self.ready
            .as_ref()
            .is_none_or(|r| r.generation < self.generation)
    }

    /// Blocks until the plan for the latest request has arrived.
    ///
    /// Only for tests: the UI must never block on the worker.
    #[cfg(test)]
    pub fn wait(&mut self, timeout: Duration) -> Option<&Plan> {
        let deadline = Instant::now() + timeout;
        while self.is_stale() && Instant::now() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.poll();
        self.plan()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::ops::{CaseMode, Casing, Replace};

    fn entries(names: &[&str]) -> Arc<Vec<FileEntry>> {
        Arc::new(
            names
                .iter()
                .map(|n| FileEntry::synthetic(std::path::PathBuf::from("/tmp").join(n)))
                .collect(),
        )
    }

    fn worker() -> PreviewWorker {
        PreviewWorker::spawn(ren_platform::host(), || {})
    }

    #[test]
    fn a_request_produces_a_plan() {
        let mut worker = worker();
        assert!(worker.plan().is_none());

        worker.request(
            entries(&["a_b.txt"]),
            Arc::new(Pipeline::new().then(Replace::new("_", "-"))),
        );
        let plan = worker
            .wait(Duration::from_secs(5))
            .expect("the worker should deliver a plan");
        assert_eq!(plan.items[0].new_name, "a-b.txt");
    }

    #[test]
    fn the_preview_is_stale_until_the_worker_answers() {
        let mut worker = worker();
        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()));
        assert!(worker.is_stale(), "nothing has come back yet");
        worker.wait(Duration::from_secs(5));
        assert!(!worker.is_stale());
    }

    /// Typing fast queues several requests; only the newest answer may win.
    #[test]
    fn a_late_answer_never_overwrites_a_newer_one() {
        let mut worker = worker();
        let files = entries(&["song.txt"]);
        for mode in [CaseMode::Upper, CaseMode::Lower, CaseMode::Title] {
            worker.request(
                files.clone(),
                Arc::new(Pipeline::new().then(Casing::new(mode))),
            );
        }
        let plan = worker.wait(Duration::from_secs(5)).unwrap();
        // The last request wins, whatever order the answers arrived in.
        assert_eq!(plan.items[0].new_name, "Song.txt");
        assert_eq!(worker.ready().unwrap().generation, 3);
    }

    #[test]
    fn polling_reports_whether_anything_changed() {
        let mut worker = worker();
        assert!(!worker.poll(), "nothing requested yet");

        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()));
        worker.wait(Duration::from_secs(5));
        assert!(!worker.poll(), "already delivered");
    }

    #[test]
    fn the_repaint_hook_fires_when_a_result_lands() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let repaints = Arc::new(AtomicUsize::new(0));
        let counter = repaints.clone();
        let mut worker = PreviewWorker::spawn(ren_platform::host(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()));
        worker.wait(Duration::from_secs(5));
        assert!(
            repaints.load(Ordering::SeqCst) >= 1,
            "an idle UI must be woken when the preview is ready"
        );
    }
}
