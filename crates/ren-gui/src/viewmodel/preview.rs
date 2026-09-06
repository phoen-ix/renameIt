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
//!
//! # A panic is an answer, not the end of the session
//!
//! `plan()` runs user-shaped input through a regex engine, a script
//! interpreter and half a dozen file-format readers, and a panic anywhere in
//! that path used to kill this thread outright. Nothing noticed: `request`
//! sends into a closed channel and ignores the error, `is_stale` stays true
//! forever, and P35 makes every run wait for a preview that will never come —
//! the app was wedged for the rest of the session with "updating…" in the
//! status bar and no way to find out why. So the work runs under
//! `catch_unwind`, the same containment the thumbnail decoder has (P75), and a
//! panic comes back as a *failed generation*: answered, so nothing waits on it,
//! with the message where the status bar can show it, and with no plan, so
//! nothing can run against it.

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
    /// Which listing rows `entries` are, in order — the plan's `items[k]`
    /// is about listing row `scoped[k]`. Carried with the request so the
    /// answer comes back with the scope it was computed over, not whatever
    /// the app has selected since (P22 scopes the plan to the selection).
    scoped: Vec<usize>,
}

struct Response {
    generation: u64,
    /// The plan, or what the engine panicked with.
    outcome: Result<Plan, String>,
    scoped: Vec<usize>,
    elapsed: Duration,
}

/// The most recent plan the worker has delivered.
#[derive(Debug)]
pub struct Ready {
    pub generation: u64,
    pub plan: Plan,
    /// The listing rows the plan was built over: `plan.items[k]` describes
    /// row `scoped[k]`. A plan does not know which rows it is about — its
    /// items index the entries it was handed — so this is what turns it back
    /// into something the table can address by row.
    pub scoped: Vec<usize>,
    /// How long the worker took, for the status bar.
    pub elapsed: Duration,
}

pub struct PreviewWorker {
    requests: Sender<Request>,
    responses: Receiver<Response>,
    /// Bumped on every request; a response carrying anything older is dropped.
    generation: u64,
    /// The newest generation the worker has answered, with a plan or with a
    /// failure. Kept apart from `ready` so a failed generation counts as
    /// answered without there being a plan to show for it.
    answered: u64,
    ready: Option<Ready>,
    /// Why the latest answered generation produced no plan. Cleared by the
    /// next plan that lands.
    failure: Option<String>,
}

impl std::fmt::Debug for PreviewWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewWorker")
            .field("generation", &self.generation)
            .field("answered", &self.answered)
            .field("ready", &self.ready.as_ref().map(|r| r.generation))
            .field("failure", &self.failure)
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
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        plan(&request.entries, &request.pipeline, platform.as_ref())
                    }))
                    .map_err(|payload| panic_message(payload.as_ref()));
                    let response = Response {
                        generation: request.generation,
                        outcome,
                        scoped: request.scoped,
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
            answered: 0,
            ready: None,
            failure: None,
        }
    }

    /// Asks for a fresh plan over `entries`, which are listing rows `scoped`.
    /// Cheap — the work happens on the worker.
    pub fn request(
        &mut self,
        entries: Arc<Vec<FileEntry>>,
        pipeline: Arc<Pipeline>,
        scoped: Vec<usize>,
    ) {
        self.generation += 1;
        // A closed channel means the worker died; the UI keeps its last plan
        // rather than panicking mid-frame.
        let _ = self.requests.send(Request {
            generation: self.generation,
            entries,
            pipeline,
            scoped,
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
            if response.generation <= self.answered {
                continue;
            }
            self.answered = response.generation;
            match response.outcome {
                Ok(plan) => {
                    self.ready = Some(Ready {
                        generation: response.generation,
                        plan,
                        scoped: response.scoped,
                        elapsed: response.elapsed,
                    });
                    self.failure = None;
                }
                // No plan at all rather than the previous one: a stale plan
                // under a fresh pipeline is exactly what P35 forbids running,
                // and `run()` reads `ready()`.
                Err(message) => {
                    self.ready = None;
                    self.failure = Some(message);
                }
            }
            changed = true;
        }
        changed
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.ready.as_ref().map(|r| &r.plan)
    }

    pub fn ready(&self) -> Option<&Ready> {
        self.ready.as_ref()
    }

    /// What the engine panicked with, when the latest preview produced no plan.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// True when a newer request is still in flight, so the visible preview is
    /// one or more keystrokes behind.
    pub fn is_stale(&self) -> bool {
        self.answered < self.generation
    }

    /// Installs a plan as if the worker had just delivered it for `scoped`.
    ///
    /// Only for tests, which need a plan whose scope disagrees with the
    /// listing on screen — the shape a race produces and a worker cannot be
    /// made to produce on cue.
    #[cfg(test)]
    pub(crate) fn install_ready(&mut self, plan: Plan, scoped: Vec<usize>) {
        self.generation += 1;
        self.answered = self.generation;
        self.ready = Some(Ready {
            generation: self.generation,
            plan,
            scoped,
            elapsed: Duration::ZERO,
        });
        self.failure = None;
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

/// The text of a panic, for the status bar.
///
/// A `panic!` with a literal carries a `&str`; one with a format string
/// carries a `String`; anything else is somebody's custom payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "the preview engine panicked".to_owned()
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
            vec![0],
        );
        let plan = worker
            .wait(Duration::from_secs(5))
            .expect("the worker should deliver a plan");
        assert_eq!(plan.items[0].new_name, "a-b.txt");
    }

    #[test]
    fn the_preview_is_stale_until_the_worker_answers() {
        let mut worker = worker();
        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()), vec![0]);
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
                vec![0],
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

        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()), vec![0]);
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

        worker.request(entries(&["a.txt"]), Arc::new(Pipeline::new()), vec![0]);
        worker.wait(Duration::from_secs(5));
        assert!(
            repaints.load(Ordering::SeqCst) >= 1,
            "an idle UI must be woken when the preview is ready"
        );
    }

    /// A transform that panics on one name, the way a reader given a hostile
    /// file might.
    #[derive(Debug)]
    struct Explodes;

    impl ren_core::ops::NameTransform for Explodes {
        fn id(&self) -> &'static str {
            "explodes"
        }
        fn summary(&self) -> String {
            "explodes".into()
        }
        fn apply<'a>(
            &self,
            subject: &'a str,
            _cx: &ren_core::ops::EvalCx<'_>,
        ) -> Result<std::borrow::Cow<'a, str>, ren_core::ops::OpError> {
            if subject.contains("boom") {
                panic!("byte index 12 is not a char boundary");
            }
            Ok(std::borrow::Cow::Borrowed(subject))
        }
    }

    /// A panic inside the engine is a failed generation, not a dead worker:
    /// the request is answered, the message is reported, there is no plan to
    /// run, and the next request still gets a plan.
    #[test]
    fn a_panic_in_the_engine_is_reported_and_the_worker_survives() {
        // The default hook prints the panic to stderr, which is noise here and
        // nothing else; it is left in place because a test that swapped the
        // process-wide hook would race every other test in the binary.
        let mut worker = worker();
        let files = entries(&["boom.txt"]);

        worker.request(
            files.clone(),
            Arc::new(Pipeline::new().then(Explodes)),
            vec![0],
        );
        assert!(worker.wait(Duration::from_secs(5)).is_none(), "no plan");
        assert!(
            !worker.is_stale(),
            "the failed generation counts as answered"
        );
        assert_eq!(
            worker.failure(),
            Some("byte index 12 is not a char boundary")
        );

        // The thread is still there and a sound pipeline still gets a plan.
        worker.request(
            files,
            Arc::new(Pipeline::new().then(Replace::new("boom", "ok"))),
            vec![0],
        );
        let plan = worker
            .wait(Duration::from_secs(5))
            .expect("the worker survived");
        assert_eq!(plan.items[0].new_name, "ok.txt");
        assert_eq!(worker.failure(), None, "cleared by the next plan");
    }
}
