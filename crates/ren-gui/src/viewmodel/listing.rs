//! The listing worker.
//!
//! The walk that turns a folder into rows used to run inside the frame:
//! `Session::refresh` was a call, the call walked the tree and `stat`-ed
//! every entry, and the window was frozen until it came back. Ten thousand
//! files on a warm disk is a stutter; a network share, a spinning disk, or
//! Subfolders over a deep tree is seconds of a window that does not repaint —
//! and Windows names it "not responding" after five of them. D128 already
//! knew the walk could be slow enough to want a setting for avoiding it.
//!
//! So the walk runs here, on the same shape as the preview worker:
//!
//! ```text
//! UI asks ──► generation += 1 ──► send (generation, source, options)
//!                                          │
//!                        worker: coalesce, walk, abandon if superseded
//!                                          │
//! UI installs the newest ◄── recv ── drop anything stale
//! ```
//!
//! While a walk is in flight the old listing stays on screen and the source
//! bar says "listing…" — a static label, never a spinner (D26). A response
//! carrying an older generation than the latest request is dropped, and a
//! walk still running when a newer request arrives abandons itself between
//! entries, so changing folder twice quickly costs one walk and a half rather
//! than two.
//!
//! The synchronous `Session::refresh_now` still exists, for the session's own
//! tests and for anything that has no frame to wait in. The app never calls
//! it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use ren_core::ListOptions;
use ren_core::listing::ListProblem;
use ren_core::model::FileEntry;

/// What to list: the browser's folder, or the free-select set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Browser { dir: PathBuf, options: ListOptions },
    FreeSelect { paths: Vec<PathBuf> },
}

impl Source {
    /// The walk, on whatever thread calls it.
    ///
    /// `stop` is polled between entries so an abandoned walk gives up early;
    /// it answers `Interrupted`, which the worker discards and nothing else
    /// ever sees.
    pub fn list(
        &self,
        stop: &dyn Fn() -> bool,
    ) -> std::io::Result<(Vec<FileEntry>, Vec<ListProblem>)> {
        match self {
            Self::Browser { dir, options } => {
                ren_core::listing::list_reporting_with(dir, options.clone(), stop)
            }
            Self::FreeSelect { paths } => paths
                .iter()
                .map(FileEntry::from_path)
                .collect::<std::io::Result<Vec<_>>>()
                .map(|entries| (entries, Vec::new())),
        }
    }
}

struct Request {
    generation: u64,
    source: Source,
}

/// A listing the worker delivered, with the generation it answers.
pub struct Listed {
    pub generation: u64,
    pub outcome: std::io::Result<(Vec<FileEntry>, Vec<ListProblem>)>,
}

pub struct ListingWorker {
    requests: Sender<Request>,
    responses: Receiver<Listed>,
    /// The newest request. Shared with the worker so a walk can see it has
    /// been superseded and stop.
    generation: Arc<AtomicU64>,
    /// The newest generation a listing has been taken for.
    answered: u64,
}

impl std::fmt::Debug for ListingWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListingWorker")
            .field("generation", &self.generation.load(Ordering::Relaxed))
            .field("answered", &self.answered)
            .finish()
    }
}

impl ListingWorker {
    /// Spawns the worker. `repaint` is called when a listing lands, so an idle
    /// UI wakes up to install it (D135: once per result, never per frame).
    pub fn spawn(repaint: impl Fn() + Send + 'static) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (response_tx, response_rx) = channel::<Listed>();
        let generation = Arc::new(AtomicU64::new(0));
        let latest = generation.clone();

        std::thread::Builder::new()
            .name("renameit-listing".into())
            .spawn(move || {
                while let Ok(mut request) = request_rx.recv() {
                    // Coalesce: a folder changed twice while the walk was
                    // queued is one walk.
                    loop {
                        match request_rx.try_recv() {
                            Ok(newer) => request = newer,
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                    let mine = request.generation;
                    let superseded = || latest.load(Ordering::Relaxed) != mine;
                    let outcome = request.source.list(&superseded);
                    // An abandoned walk is not an answer: the newer request
                    // is what the UI wants, and it is already queued.
                    if superseded() {
                        continue;
                    }
                    if response_tx
                        .send(Listed {
                            generation: mine,
                            outcome,
                        })
                        .is_err()
                    {
                        return; // The UI is gone.
                    }
                    repaint();
                }
            })
            .expect("the listing worker thread should start");

        Self {
            requests: request_tx,
            responses: response_rx,
            generation,
            answered: 0,
        }
    }

    /// Asks for `source` to be listed. Cheap — the walk happens on the worker.
    pub fn request(&mut self, source: Source) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        // A closed channel means the worker died; the UI keeps the listing it
        // has rather than panicking mid-frame.
        let _ = self.requests.send(Request { generation, source });
    }

    /// The newest listing the worker has delivered since the last poll, if
    /// any. Older ones are dropped on the way.
    pub fn poll(&mut self) -> Option<Listed> {
        let mut newest: Option<Listed> = None;
        while let Ok(listed) = self.responses.try_recv() {
            if listed.generation > self.answered
                && newest
                    .as_ref()
                    .is_none_or(|n| listed.generation > n.generation)
            {
                newest = Some(listed);
            }
        }
        if let Some(listed) = &newest {
            self.answered = listed.generation;
        }
        newest
    }

    /// True while a request is in flight, so the source bar can say so.
    pub fn is_listing(&self) -> bool {
        self.answered < self.generation.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(worker: &mut ListingWorker) -> Listed {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(listed) = worker.poll() {
                return listed;
            }
            assert!(Instant::now() < deadline, "the worker never answered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn browser(dir: &std::path::Path) -> Source {
        Source::Browser {
            dir: dir.to_path_buf(),
            options: ListOptions::default(),
        }
    }

    #[test]
    fn a_request_lists_the_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let mut worker = ListingWorker::spawn(|| {});
        worker.request(browser(dir.path()));
        assert!(worker.is_listing());
        let listed = wait(&mut worker);
        let (entries, problems) = listed.outcome.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(problems.is_empty());
        assert!(!worker.is_listing());
    }

    /// Two folders asked for in quick succession: the second wins, and the
    /// first is never delivered — not even if it finished.
    #[test]
    fn the_newest_request_wins() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        std::fs::write(first.path().join("first.txt"), b"x").unwrap();
        std::fs::write(second.path().join("second.txt"), b"x").unwrap();
        let mut worker = ListingWorker::spawn(|| {});
        worker.request(browser(first.path()));
        worker.request(browser(second.path()));
        let listed = wait(&mut worker);
        assert_eq!(listed.generation, 2);
        let names: Vec<String> = listed
            .outcome
            .unwrap()
            .0
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        assert_eq!(names, ["second.txt"]);
        assert!(worker.poll().is_none(), "nothing older arrives afterwards");
    }

    /// A folder that cannot be read is an error the UI can show, delivered
    /// like any other answer.
    #[test]
    fn an_unreadable_root_is_delivered_as_an_error() {
        let mut worker = ListingWorker::spawn(|| {});
        worker.request(browser(std::path::Path::new("/definitely/not/here")));
        let listed = wait(&mut worker);
        assert!(listed.outcome.is_err());
    }

    /// A walk that has been superseded stops between entries rather than
    /// finishing a listing nobody wants.
    #[test]
    fn a_superseded_walk_is_abandoned() {
        let dir = tempfile::TempDir::new().unwrap();
        for i in 0..50 {
            std::fs::write(dir.path().join(format!("{i}.txt")), b"x").unwrap();
        }
        let stopped = std::sync::atomic::AtomicBool::new(true);
        let result = browser(dir.path()).list(&|| stopped.load(Ordering::Relaxed));
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    }
}
