//! The confirmation a run that cannot be undone has to pass.
//!
//! **P2**: tag writes and tag removal have no undo in 1.0, so both are
//! confirmation-gated with an explicit "cannot be undone" warning.
//!
//! A static warning in the panel would not be enough — a gate in front of the
//! run is. P2 is also what keeps the scope honest in the other direction:
//! sidecar backups are a post-1.0 idea, so the job here is to make the cost
//! visible, not
//! to make it recoverable.
//!
//! Shown **only** when the plan contains something irreversible. An ordinary
//! rename stays one click; adding a step to every run would train the habit of
//! clicking through, which is the one thing a confirmation cannot survive. The
//! summary wording is borrowed from the status bar (DESIGN S7's *"N renames, M
//! skipped, K conflicts"*), so widening the trigger later is a change of
//! condition rather than a rewrite.

use ren_core::Plan;

/// The dialog while it is up — a **snapshot** of the plan it was opened for.
///
/// Everything is computed once, in [`Confirm::new`]. Two reasons, and the
/// second is the important one: walking `plan.items` per frame is a full scan
/// of up to ten thousand rows on every repaint, and — precisely because it
/// cannot change while the dialog is open — what the user approves is what they
/// were shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    /// The preview generation these numbers came from.
    ///
    /// Consent is for one plan, not for the app: `app.rs` only honours it while
    /// the plan in hand is still this one, and every edit bumps the generation.
    pub generation: u64,
    /// Rows changed irreversibly — `Plan::irreversible()`, which counts files.
    irreversible: usize,
    /// The pre-run clauses, in the status bar's own words.
    clauses: Vec<String>,
    /// `("song.mp3", "Title → One, Artist → Metallica")`, capped.
    lines: Vec<(String, String)>,
    /// Irreversible rows that did not fit in `lines`.
    more: usize,
    /// The sentence `Undoability` itself supplies.
    verdict: &'static str,
}

/// Enough to recognise the run, few enough that the modal never grows past the
/// window. An unbounded list over a ten-thousand-row plan would also resize the
/// modal every frame, which is the settling hazard D26 forbids.
const SHOWN: usize = 6;

impl Confirm {
    pub fn new(plan: &Plan, generation: u64) -> Self {
        let mut lines = Vec::new();
        let mut more = 0;
        for item in &plan.items {
            let irreversible: Vec<&str> = item
                .actions
                .iter()
                .filter(|a| !a.undoability.is_reversible())
                .map(|a| a.describe.as_str())
                .collect();
            if irreversible.is_empty() {
                continue;
            }
            if lines.len() < SHOWN {
                lines.push((
                    item.source
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    irreversible.join("; "),
                ));
            } else {
                more += 1;
            }
        }
        // A file a script is about to overwrite is not a row, so it has no
        // entry above — and it is exactly the kind of thing this modal exists
        // to name. Listed by path, because "1 item" would tell the user nothing
        // about which of their files is about to be replaced.
        for path in plan.overwrites() {
            if lines.len() < SHOWN {
                lines.push((
                    path.display().to_string(),
                    "replace this file's contents".to_owned(),
                ));
            } else {
                more += 1;
            }
        }
        Self {
            generation,
            irreversible: plan.irreversible(),
            clauses: crate::panels::status_bar::plan_clauses(plan),
            lines,
            more,
            verdict: plan.undoability().describe(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Confirmed,
    Cancelled,
}

/// The label of the button that goes ahead.
///
/// Deliberately not "Rename", "OK" or "Yes". It restates the commitment, so the
/// muscle memory that dismisses dialogs has nothing to grab — and it collides
/// with no other label in the tree, which matters because the panels behind a
/// modal are still in the accessibility tree and a duplicate label makes a test
/// ambiguous rather than wrong.
pub fn go_ahead_label(irreversible: usize) -> String {
    format!("Change {irreversible} item(s)")
}

pub fn ui(ctx: &egui::Context, state: &Confirm) -> Outcome {
    let mut outcome = Outcome::Open;

    let modal = egui::Modal::new(egui::Id::new("confirm_modal")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(format!("This run {}", state.verdict));
        ui.add_space(6.0);

        if !state.clauses.is_empty() {
            ui.label(state.clauses.join(" • "));
            ui.add_space(4.0);
        }

        // The engine's own sentence, one tense earlier. A user who ever sees
        // `ExecError::Irreversible` should recognise it.
        ui.label(
            egui::RichText::new(format!(
                "⚠ {} item(s) will be changed in a way that {}. Undo puts names back; \
                 it cannot put file contents back, because the old ones are not kept \
                 anywhere.",
                state.irreversible, state.verdict
            ))
            .color(ui.visuals().warn_fg_color),
        );

        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .id_salt("confirm_detail")
            .show(ui, |ui| {
                for (file, what) in &state.lines {
                    ui.label(
                        egui::RichText::new(format!("{file}  —  {what}"))
                            .weak()
                            .small(),
                    );
                }
                if state.more > 0 {
                    ui.label(
                        egui::RichText::new(format!("+ {} more", state.more))
                            .weak()
                            .small(),
                    );
                }
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            // Safe answer first, on the left, and it is also what Escape and a
            // click on the backdrop do.
            if ui.button("Leave everything alone").clicked() {
                outcome = Outcome::Cancelled;
            }
            if ui.button(go_ahead_label(state.irreversible)).clicked() {
                outcome = Outcome::Confirmed;
            }
        });
    });

    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Cancelled;
    }
    outcome
}

/// The other irreversible thing this app does, and the only one that is not a
/// rename.
///
/// **P2 does not reach here**: that policy is about tag writes, and D52 made it
/// an engine invariant on `ApplyOptions::allow_irreversible` — a settings reset
/// never touches `apply`. **D156** carries this confirmation instead, and the
/// wording follows the conventions [`ui`] above established: the safe answer on
/// the left, and a go-ahead label that restates the commitment rather than
/// saying "OK", so muscle memory has nothing to grab.
///
/// It names what it **spares**, because a button called "reset settings" sitting
/// ten lines under three folder shortcuts invites exactly the wrong guess.
pub fn reset_ui(ctx: &egui::Context) -> Outcome {
    let mut outcome = Outcome::Open;

    let modal = egui::Modal::new(egui::Id::new("reset_modal")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading("Restore every setting to its defaults?");
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "⚠ This cannot be undone. Your Batch Replace rules, casing exception words, \
                 music styles, columns, theme, counter and every switch in this window go \
                 back to what the app ships with.",
            )
            .color(ui.visuals().warn_fg_color),
        );
        ui.add_space(6.0);
        ui.label(
            "It does not touch your files, your presets, your scripts or the undo journal — \
             those are files on disk, and the folders are listed on the page behind this. The \
             pipeline you have built and the folder you are looking at are left alone too.",
        );

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Leave my settings alone").clicked() {
                outcome = Outcome::Cancelled;
            }
            if ui.button("Restore every setting").clicked() {
                outcome = Outcome::Confirmed;
            }
        });
    });

    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Cancelled;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::model::FileEntry;
    use ren_core::ops::{MusicTagger, RemoveTags};
    use ren_core::pipeline::{Pipeline, Step, StepConfig};
    use ren_core::template::TextTemplate;
    use tempfile::TempDir;

    /// A plan over one MP3 with two irreversible cards on it.
    fn two_card_plan(dir: &TempDir) -> Plan {
        ren_core::meta::testing::Mp3::tagged("A", "B").write(dir.path(), "song.mp3");
        ren_core::meta::audio::forget_all();
        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new()
            .with(
                Step::Action(Box::new(MusicTagger {
                    artist: Some(TextTemplate::new("Written")),
                    ..Default::default()
                })),
                StepConfig::default(),
            )
            .with(
                Step::Action(Box::new(RemoveTags {
                    id3v1: true,
                    ..Default::default()
                })),
                StepConfig::default(),
            );
        ren_core::plan(&entries, &pipeline, ren_platform::host().as_ref())
    }

    /// The number the dialog quotes has to be the number the engine's refusal
    /// quotes, or the user is told two different things about one run.
    #[test]
    fn the_dialog_counts_files_not_actions() {
        let dir = TempDir::new().unwrap();
        let plan = two_card_plan(&dir);
        let confirm = Confirm::new(&plan, 7);

        assert_eq!(plan.irreversible(), 1, "one file");
        assert_eq!(confirm.irreversible, 1);
        assert_eq!(confirm.lines.len(), 1, "one line per file, not per card");
        // Both cards named on that one line, so nothing is hidden by the
        // collapsing.
        assert!(confirm.lines[0].1.contains("Artist"), "{:?}", confirm.lines);
        assert!(confirm.lines[0].1.contains("ID3v1"), "{:?}", confirm.lines);
        assert_eq!(confirm.generation, 7);
    }

    /// The dialog's verdict comes from `Undoability` rather than from a string
    /// typed here, so a future level cannot leave the sentence behind.
    #[test]
    fn the_verdict_is_the_engines_own_word() {
        let dir = TempDir::new().unwrap();
        let confirm = Confirm::new(&two_card_plan(&dir), 0);
        assert_eq!(
            confirm.verdict,
            ren_core::Undoability::None.describe(),
            "the dialog must not carry its own copy of this sentence"
        );
        assert_eq!(confirm.verdict, "cannot be undone");
    }

    /// And its summary is the status bar's, for the same reason.
    #[test]
    fn the_summary_is_the_status_bars_own_clauses() {
        let dir = TempDir::new().unwrap();
        let plan = two_card_plan(&dir);
        assert_eq!(
            Confirm::new(&plan, 0).clauses,
            crate::panels::status_bar::plan_clauses(&plan)
        );
    }

    /// The list is capped, or a ten-thousand-row plan resizes the modal on
    /// every frame — the settling hazard D26 exists for.
    #[test]
    fn a_long_list_is_capped_and_says_how_many_it_left_out() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            ren_core::meta::testing::Mp3::tagged("A", "B")
                .write(dir.path(), &format!("song{i}.mp3"));
        }
        ren_core::meta::audio::forget_all();
        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new().with(
            Step::Action(Box::new(RemoveTags {
                id3v2: true,
                ..Default::default()
            })),
            StepConfig::default(),
        );
        let plan = ren_core::plan(&entries, &pipeline, ren_platform::host().as_ref());
        let confirm = Confirm::new(&plan, 0);

        assert_eq!(confirm.irreversible, 10, "all of them are counted");
        assert_eq!(confirm.lines.len(), SHOWN, "but not all of them are listed");
        assert_eq!(confirm.more, 10 - SHOWN);
    }

    /// A reversible plan never reaches the dialog, and if it somehow did it
    /// would have nothing to say.
    #[test]
    fn a_reversible_plan_has_nothing_to_confirm() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let entries = ren_core::list(dir.path(), Default::default()).unwrap();
        let pipeline = Pipeline::new().with(
            Step::Name(Box::new(ren_core::ops::Replace::new("a", "b"))),
            StepConfig::default(),
        );
        let plan = ren_core::plan(&entries, &pipeline, ren_platform::host().as_ref());
        let confirm = Confirm::new(&plan, 0);
        assert_eq!(confirm.irreversible, 0);
        assert!(confirm.lines.is_empty());
        assert_eq!(confirm.verdict, "can be undone");
    }

    /// The affirmative label must collide with nothing else on screen: the
    /// panels behind a modal stay in the accessibility tree, so a duplicate
    /// makes a test ambiguous rather than merely wrong.
    #[test]
    fn the_go_ahead_button_is_not_called_rename_or_ok() {
        let label = go_ahead_label(3);
        assert_eq!(label, "Change 3 item(s)");
        for taken in ["Rename", "Cancel", "OK", "Yes", "Simulate", "Close"] {
            assert_ne!(label, taken);
        }
    }

    #[test]
    fn the_entry_is_the_file_name_not_the_whole_path() {
        let dir = TempDir::new().unwrap();
        let confirm = Confirm::new(&two_card_plan(&dir), 0);
        assert_eq!(confirm.lines[0].0, "song.mp3");
        let _ = FileEntry::synthetic("/unused");
    }
}
