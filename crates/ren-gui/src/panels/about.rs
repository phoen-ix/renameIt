//! Settings ▸ About.
//!
//! It shows the version, the licence and where the code lives, because this
//! is an MIT program shipping a binary and those are the three things somebody
//! opening About is entitled to. The counts are **this session's**, from the
//! runs that actually happened; there is no lifetime tally, and P67 says why.

use crate::viewmodel::{Batch, describe_counts};

/// What one session did, as About reports it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionTally {
    pub runs: usize,
    pub renamed: usize,
    pub modified: usize,
}

impl SessionTally {
    /// A simulated batch touched nothing, so it is not something this ran.
    pub fn of(batches: &[Batch]) -> Self {
        batches
            .iter()
            .filter(|batch| !batch.simulated)
            .fold(Self::default(), |mut tally, batch| {
                tally.runs += 1;
                tally.renamed += batch.renamed;
                tally.modified += batch.modified;
                tally
            })
    }

    pub fn summary(&self) -> String {
        if self.runs == 0 {
            return "Nothing renamed yet this session.".to_owned();
        }
        let what = describe_counts(self.renamed, self.modified);
        let runs = if self.runs == 1 { "run" } else { "runs" };
        format!("{what}, over {} {runs} this session.", self.runs)
    }
}

/// Settings ▸ About, the last page of the rail (D255).
///
/// `newer` is a release the updater found, said beside the version so the
/// page that answers "which version is this" also answers "is it the latest"
/// when that is known.
pub fn body(ui: &mut egui::Ui, batches: &[Batch], newer: Option<crate::viewmodel::Version>) {
    ui.horizontal(|ui| {
        ui.heading("RenameIt");
        ui.label(
            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                .weak()
                .monospace(),
        );
        if let Some(version) = newer {
            ui.label(egui::RichText::new(format!("{version} is available — see Updates")).small());
        }
    });
    ui.label("A batch file renamer.");
    ui.add_space(8.0);

    ui.label(SessionTally::of(batches).summary());
    ui.label(
        egui::RichText::new(
            "No lifetime total is kept: it would count one machine and one install, and \
             reset without saying so.",
        )
        .weak()
        .small(),
    );

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("MIT licensed").small());
        ui.label(egui::RichText::new("·").weak().small());
        ui.hyperlink_to(
            egui::RichText::new(env!("CARGO_PKG_REPOSITORY")).small(),
            env!("CARGO_PKG_REPOSITORY"),
        );
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Built with egui, lofty, kamadak-exif, image, koto, regex and clap — all under \
             permissive licences.",
        )
        .weak()
        .small(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn batch(renamed: usize, modified: usize, simulated: bool) -> Batch {
        Batch {
            txn: "t".into(),
            journal: PathBuf::from("/tmp"),
            renamed,
            modified,
            simulated,
        }
    }

    #[test]
    fn a_fresh_session_says_so_rather_than_reporting_zero() {
        assert_eq!(
            SessionTally::of(&[]).summary(),
            "Nothing renamed yet this session."
        );
    }

    /// A simulation is the one run that deliberately did nothing, and counting
    /// it would make the number mean "times you pressed a button".
    #[test]
    fn a_simulated_run_is_not_something_that_happened() {
        let tally = SessionTally::of(&[batch(3, 0, true), batch(2, 1, false)]);
        assert_eq!(
            tally,
            SessionTally {
                runs: 1,
                renamed: 2,
                modified: 1
            }
        );
        assert_eq!(
            tally.summary(),
            "Renamed 2 and modified 1 item, over 1 run this session."
        );
    }

    #[test]
    fn several_runs_are_summed_and_pluralised() {
        let tally = SessionTally::of(&[batch(2, 0, false), batch(3, 0, false)]);
        assert_eq!(
            tally.summary(),
            "Renamed 5 items, over 2 runs this session."
        );
    }
}
