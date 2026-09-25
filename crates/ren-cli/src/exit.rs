//! What the process exits with, and why each code means what it does.
//!
//! There were two codes before M7 — success and failure — and M7 asks
//! for a richer set. Richer is only worth having if it stays *stable*, so the
//! numbers are fixed here with the rule each one answers, and two of them are
//! constrained by decisions that already exist:
//!
//! * **P4** — conflicts hard-block a run rather than being skipped silently, so
//!   a blocked plan must not exit zero. A script that pipes `preview` into a
//!   deploy has to be able to tell "nothing needed renaming" from "the rename
//!   was refused".
//! * **D54 / D77** — an undo whose only shortfall is a tag write it could
//!   never take back still exits `SUCCESS`: saying so is not a failure, and an
//!   undo that reports "still applied: …" did its whole job. A *skipped* item,
//!   or an undo the journal could not record, is a shortfall, and exits
//!   [`Exit::Failed`].
//!
//! Anything added later has to keep both.

use std::process::ExitCode;

/// The process's answer, one variant per reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The command did what it was asked.
    Success,
    /// The command line was wrong, or something failed before any work
    /// started — nothing was changed. Also every unexpected error, so that a
    /// caller checking only for zero is never misled.
    ///
    /// Never used once files have moved: a list file that will not delete
    /// after a successful run is a warning, and an error part-way through a
    /// rollback is [`Self::Failed`].
    Usage,
    /// Refused before anything was touched.
    ///
    /// The plan has conflicts or row errors (P4); or a folder is one the
    /// operating system needs left alone (D127); or a change cannot be undone
    /// and `--allow-irreversible` was not given (P2); or the plan names a path
    /// that is not absolute; or another run is writing the journal an `undo`
    /// wanted.
    ///
    /// Distinct from [`Self::Failed`] on purpose: nothing was touched. A
    /// caller can retry after fixing the cause, and does not need to inspect
    /// the filesystem first.
    Blocked,
    /// The run started and some items did not make it, or an undo or a
    /// rollback could not put everything back.
    ///
    /// The filesystem is in whatever state the journal describes, so the next
    /// step for a caller is `recover`, not a retry.
    Failed,
}

impl Exit {
    /// The number a shell sees.
    ///
    /// Written out rather than derived from declaration order, because these
    /// numbers *are* the interface: a script matching on `2` must keep
    /// matching on it after somebody reorders the enum.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Usage => 1,
            Self::Blocked => 2,
            Self::Failed => 3,
        }
    }

    pub fn code(self) -> ExitCode {
        ExitCode::from(self.as_u8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are the interface. Changing one silently breaks every
    /// caller that matches on it, so they are pinned rather than derived from
    /// declaration order.
    #[test]
    fn the_codes_are_the_ones_documented() {
        assert_eq!(Exit::Success.as_u8(), 0);
        assert_eq!(Exit::Usage.as_u8(), 1);
        assert_eq!(Exit::Blocked.as_u8(), 2);
        assert_eq!(Exit::Failed.as_u8(), 3);
    }

    /// Zero means success and nothing else does, which is the only property
    /// most callers actually rely on.
    #[test]
    fn only_success_is_zero() {
        for exit in [Exit::Usage, Exit::Blocked, Exit::Failed] {
            assert_ne!(exit.as_u8(), 0, "{exit:?} must not look like success");
        }
    }
}
