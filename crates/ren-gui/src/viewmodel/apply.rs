//! The run worker: apply, undo and rollback, off the frame.
//!
//! A run is one `fdatasync` per file (D168) and a rename syscall each, and
//! it used to happen inside the frame: ten thousand files on a spinning disk
//! or a share is minutes of a window that does not repaint, which Windows
//! names "not responding" after five seconds. Undo is the same work
//! backwards, and so is the recovery banner's Roll back.
//!
//! So both happen here, on the shape the preview and listing workers share —
//! one job at a time, because a second run while the first is going would be
//! two batches over one listing:
//!
//! ```text
//! UI starts a job ──► send (plan, options) or (journal)
//!                               │
//!                    worker: apply / undo, bumping `progress`
//!                               │
//! UI polls the outcome ◄── recv ── then records it exactly as before
//! ```
//!
//! While a job is out the app draws a static progress line — "Renaming 4 213
//! of 10 000" and a Cancel button — never a spinner (D26); the worker wakes
//! the UI once when the job lands (D135) and the progress line repaints on
//! the frames the user's own input causes. Cancel pulls a flag the engine
//! reads between ops, so a cancelled run is a shorter run: confirmed,
//! committed, and undone exactly like any other.
//!
//! The work runs under `catch_unwind`, as the preview does: a panic in the
//! engine mid-batch comes back as [`Outcome::Panicked`], naming the kind of
//! job it interrupted, with the journal already written ahead — which is what
//! recovery is for — not as a dead thread and a button that never
//! re-enables. Its own variant rather than an `ExecError`, because the app
//! has to treat it as "files may have moved", and no error the engine
//! returns before starting means that.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use ren_core::exec::{ApplyOptions, ApplyReport, ExecError, UndoReport, Unfinished};
use ren_core::{Plan, apply};
use ren_platform::Platform;

/// What the worker is asked to do.
pub enum Job {
    Run {
        plan: Plan,
        options: ApplyOptions,
    },
    Undo {
        /// Where the batch sat on the history stack, handed back with the
        /// report so the app can record the undo against the right batch.
        position: usize,
        journal: PathBuf,
    },
    /// The recovery banner's Roll back: each unfinished transaction in turn,
    /// stopping at the first that fails.
    Rollback(Vec<Unfinished>),
}

impl Job {
    fn kind(&self) -> JobKind {
        match self {
            Self::Run { .. } => JobKind::Run,
            Self::Undo { .. } => JobKind::Undo,
            Self::Rollback(_) => JobKind::Rollback,
        }
    }
}

/// What came back.
pub enum Outcome {
    Run(Result<ApplyReport, ExecError>),
    Undo {
        position: usize,
        report: Result<UndoReport, ExecError>,
    },
    /// One result per transaction attempted, in order.
    Rollback(Vec<(Unfinished, Result<UndoReport, ExecError>)>),
    /// The engine panicked part-way through a job of this kind.
    Panicked {
        kind: JobKind,
        message: String,
    },
}

/// The job that is out, as the frame sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlight {
    pub kind: JobKind,
    /// Ops performed so far.
    pub done: usize,
    /// Ops the job has to perform.
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Run,
    Undo,
    Rollback,
}

pub struct ApplyWorker {
    requests: Sender<Job>,
    responses: Receiver<Outcome>,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    /// What is out, if anything. One at a time.
    busy: Option<(JobKind, usize)>,
}

impl std::fmt::Debug for ApplyWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplyWorker")
            .field("busy", &self.busy)
            .field("progress", &self.progress.load(Ordering::Relaxed))
            .finish()
    }
}

impl ApplyWorker {
    /// Spawns the worker. `repaint` is called when a job lands.
    pub fn spawn(platform: Arc<dyn Platform>, repaint: impl Fn() + Send + 'static) -> Self {
        let (request_tx, request_rx) = channel::<Job>();
        let (response_tx, response_rx) = channel::<Outcome>();

        std::thread::Builder::new()
            .name("renameit-apply".into())
            .spawn(move || {
                while let Ok(job) = request_rx.recv() {
                    // Read before the job moves into the closure: a panic
                    // has to come back as the kind of job it interrupted.
                    let kind = job.kind();
                    let outcome =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                            Job::Run { plan, options } => {
                                Outcome::Run(apply(&plan, platform.as_ref(), &options))
                            }
                            Job::Undo { position, journal } => Outcome::Undo {
                                position,
                                report: ren_core::exec::undo_transaction(
                                    &journal,
                                    platform.as_ref(),
                                ),
                            },
                            Job::Rollback(items) => {
                                let mut results = Vec::with_capacity(items.len());
                                for item in items {
                                    let result = ren_core::exec::rollback(&item, platform.as_ref());
                                    let failed = result.is_err();
                                    results.push((item, result));
                                    if failed {
                                        break;
                                    }
                                }
                                Outcome::Rollback(results)
                            }
                        }))
                        .unwrap_or_else(|payload| Outcome::Panicked {
                            kind,
                            message: crate::viewmodel::preview::panic_message(payload.as_ref()),
                        });
                    if response_tx.send(outcome).is_err() {
                        return; // The UI is gone.
                    }
                    repaint();
                }
            })
            .expect("the run worker thread should start");

        Self {
            requests: request_tx,
            responses: response_rx,
            progress: Arc::new(AtomicUsize::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: None,
        }
    }

    /// Starts a run. `options` gets this worker's cancel flag and progress
    /// counter; the caller's own are ignored.
    ///
    /// Refused, and the plan handed back, while another job is out.
    pub fn run(&mut self, plan: Plan, mut options: ApplyOptions) -> Result<(), Plan> {
        if self.busy.is_some() {
            return Err(plan);
        }
        self.progress.store(0, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Relaxed);
        options.cancel = Some(self.cancel.clone());
        options.progress = Some(self.progress.clone());
        self.busy = Some((JobKind::Run, plan.ops.len()));
        // A closed channel means the worker died; the outcome that never
        // comes is reported by `poll`'s caller as a job that is still out.
        let _ = self.requests.send(Job::Run { plan, options });
        Ok(())
    }

    /// Starts an undo of the journal at `journal`, which sits at `position`
    /// on the history stack. Refused while another job is out.
    pub fn undo(&mut self, position: usize, journal: PathBuf, ops: usize) -> bool {
        self.start(Job::Undo { position, journal }, ops)
    }

    /// Starts rolling back `items`, as the recovery banner asks. Refused
    /// while another job is out — a rollback beside a run over the same files
    /// is two batches over one listing.
    pub fn rollback(&mut self, items: Vec<Unfinished>) -> bool {
        let total = items.len();
        self.start(Job::Rollback(items), total)
    }

    fn start(&mut self, job: Job, total: usize) -> bool {
        if self.busy.is_some() {
            return false;
        }
        self.progress.store(0, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Relaxed);
        self.busy = Some((job.kind(), total));
        let _ = self.requests.send(job);
        true
    }

    /// Asks the run out to stop before its next op. Nothing for an undo or a
    /// rollback, which have to finish to be exact.
    pub fn cancel(&self) {
        if matches!(self.busy, Some((JobKind::Run, _))) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a cancel has been asked for and not yet answered.
    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The job that is out, with how far it has got.
    pub fn in_flight(&self) -> Option<InFlight> {
        self.busy.map(|(kind, total)| InFlight {
            kind,
            done: self.progress.load(Ordering::Relaxed).min(total),
            total,
        })
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The outcome of the job that was out, if it has landed.
    pub fn poll(&mut self) -> Option<Outcome> {
        let outcome = self.responses.try_recv().ok()?;
        self.busy = None;
        Some(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(worker: &mut ApplyWorker) -> Outcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(outcome) = worker.poll() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "the worker never answered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_run_lands_with_its_report_and_frees_the_worker() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a_b.txt"), b"x").unwrap();
        let journal = tempfile::TempDir::new().unwrap();
        let platform = ren_platform::host();
        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = ren_core::Pipeline::new().then(ren_core::ops::Replace::new("_", "-"));
        let plan = ren_core::plan(&entries, &pipeline, platform.as_ref());

        let mut worker = ApplyWorker::spawn(platform, || {});
        let options = ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        };
        assert!(worker.run(plan, options).is_ok());
        assert!(worker.is_busy());
        assert_eq!(worker.in_flight().map(|f| f.total), Some(1));

        let Outcome::Run(report) = wait(&mut worker) else {
            panic!("a run answers with a run report");
        };
        assert_eq!(report.unwrap().renamed.len(), 1);
        assert!(!worker.is_busy());
        assert!(dir.path().join("a-b.txt").exists());
    }

    /// A platform whose renames panic, the way a bug in a reader might.
    #[derive(Debug)]
    struct Panics;

    impl Platform for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn capabilities(&self) -> &'static [ren_platform::Capability] {
            ren_platform::host().capabilities()
        }
        fn rename(
            &self,
            _from: &std::path::Path,
            _to: &std::path::Path,
        ) -> ren_platform::Result<()> {
            panic!("a rename that panics");
        }
        fn replace_file(
            &self,
            temp: &std::path::Path,
            target: &std::path::Path,
        ) -> ren_platform::Result<()> {
            ren_platform::host().replace_file(temp, target)
        }
        fn get_attributes(
            &self,
            path: &std::path::Path,
        ) -> ren_platform::Result<ren_platform::FileAttributes> {
            ren_platform::host().get_attributes(path)
        }
        fn set_attributes(
            &self,
            path: &std::path::Path,
            change: ren_platform::AttributeChange,
        ) -> ren_platform::Result<()> {
            ren_platform::host().set_attributes(path, change)
        }
        fn get_times(
            &self,
            path: &std::path::Path,
        ) -> ren_platform::Result<ren_platform::FileTimes> {
            ren_platform::host().get_times(path)
        }
        fn set_times(
            &self,
            path: &std::path::Path,
            change: ren_platform::TimeChange,
        ) -> ren_platform::Result<()> {
            ren_platform::host().set_times(path, change)
        }
        fn naming_rules(&self, path: &std::path::Path) -> &'static ren_platform::NamingRules {
            ren_platform::host().naming_rules(path)
        }
        fn case_sensitivity(&self, dir: &std::path::Path) -> ren_platform::CaseSensitivity {
            ren_platform::host().case_sensitivity(dir)
        }
        fn reveal_in_file_manager(&self, path: &std::path::Path) -> ren_platform::Result<()> {
            ren_platform::host().reveal_in_file_manager(path)
        }
        fn notify_shell_changed(&self, path: &std::path::Path) {
            ren_platform::host().notify_shell_changed(path);
        }
    }

    /// A panic in an undo comes back as an undo. It used to come back as a
    /// run — which the app, with no run out, dropped without a word.
    #[test]
    fn a_panic_names_the_kind_of_job_it_interrupted() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a_b.txt"), b"x").unwrap();
        let journal = tempfile::TempDir::new().unwrap();
        let host = ren_platform::host();
        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = ren_core::Pipeline::new().then(ren_core::ops::Replace::new("_", "-"));
        let plan = ren_core::plan(&entries, &pipeline, host.as_ref());
        let options = ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        };
        let report = apply(&plan, host.as_ref(), &options).unwrap();

        let mut worker = ApplyWorker::spawn(Arc::new(Panics), || {});
        assert!(worker.undo(0, report.journal.unwrap(), 1));
        let Outcome::Panicked { kind, message } = wait(&mut worker) else {
            panic!("a panic is its own outcome");
        };
        assert_eq!(kind, JobKind::Undo);
        assert!(message.contains("a rename that panics"), "{message}");
        assert!(!worker.is_busy(), "and the worker is free again");
    }

    /// One job at a time: a second run while the first is out is refused
    /// and its plan handed back, rather than queued behind a batch that
    /// changes the listing it was planned over.
    #[test]
    fn a_second_job_while_one_is_out_is_refused() {
        let platform = ren_platform::host();
        let mut worker = ApplyWorker::spawn(platform, || {});
        let journal = tempfile::TempDir::new().unwrap();
        let options = ApplyOptions {
            journal_dir: journal.path().to_path_buf(),
            ..Default::default()
        };
        assert!(worker.run(Plan::default(), options.clone()).is_ok());
        assert!(worker.run(Plan::default(), options).is_err());
        wait(&mut worker);
        assert!(!worker.is_busy());
    }
}
