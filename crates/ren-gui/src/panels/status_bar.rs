//! Counts on the left, actions on the right — the bottom zone of screen S1.
//!
//! The Rename button is the one place P4 becomes visible: conflicts *block* a
//! run rather than being skipped one by one, so a disabled button that does not
//! say why would be the app's worst moment.

use std::path::Path;

use ren_core::{Counts, Plan};

use crate::viewmodel::{History, RowFilter, Session};

/// What the status bar wants the app to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StatusBarOutput {
    pub run: bool,
    pub undo: bool,
    pub row_filter: Option<RowFilter>,
    pub toggle_log: bool,
    /// Stop the run that is out before its next op.
    pub cancel: bool,
}

/// What the preview worker currently has to show, read once per frame.
#[derive(Debug, Clone, Copy)]
pub struct PreviewState<'a> {
    pub plan: Option<&'a Plan>,
    /// A newer request is still in flight.
    pub stale: bool,
    /// The latest preview produced no plan because the engine panicked.
    pub failure: Option<&'a str>,
    /// A run or an undo is out on its thread, and how far it has got.
    pub job: Option<crate::viewmodel::InFlight>,
    /// The run out has been asked to stop and has not answered yet.
    pub cancelling: bool,
}

pub fn ui(
    ui: &mut egui::Ui,
    session: &Session,
    preview: PreviewState<'_>,
    history: &History,
    simulate: &mut bool,
    pipeline_empty: bool,
) -> StatusBarOutput {
    let mut out = StatusBarOutput::default();
    let PreviewState {
        plan,
        stale,
        failure,
        job,
        cancelling,
    } = preview;

    // One pass over the plan for every number this bar shows, rather than
    // one pass per number per place it is shown.
    let counts = plan.map(Plan::counts);

    ui.horizontal(|ui| {
        ui.label(summary(session, counts));
        if stale {
            // Deliberately not a spinner: an animated widget asks egui to
            // repaint forever, which never settles in a headless test and
            // burns a core in the real app for a preview that lands in
            // milliseconds.
            ui.label(egui::RichText::new("updating…").weak().italics());
        }
        if let Some(job) = job {
            // Static, like "updating…": the worker wakes the UI once when the
            // job lands, and the count moves on the frames the user's own
            // input causes. A spinner here would never settle (D26).
            ui.separator();
            // Undo reports no progress of its own — it has to finish to be
            // exact, so there is no count worth watching — and a "0 of N"
            // that never moves would read as stuck.
            let text = match job.kind {
                crate::viewmodel::JobKind::Run => {
                    format!("Renaming {} of {}…", job.done, job.total)
                }
                crate::viewmodel::JobKind::Undo => "Undoing…".to_owned(),
            };
            ui.label(egui::RichText::new(text).italics());
            if job.kind == crate::viewmodel::JobKind::Run {
                if cancelling {
                    ui.label(egui::RichText::new("stopping…").weak().italics());
                } else if ui
                    .button("Cancel")
                    .on_hover_text(
                        "Stop before the next file. What has been renamed stays renamed, and \
                         Undo takes it back.",
                    )
                    .clicked()
                {
                    out.cancel = true;
                }
            }
        }
        if let Some(failure) = failure {
            // The engine panicked on this listing. Said here rather than
            // nowhere: without it the table shows "—" on every row and the
            // Rename button is disabled, which looks like a run that was
            // never asked for.
            ui.label(
                egui::RichText::new(format!("preview failed: {failure}"))
                    .color(ui.visuals().error_fg_color),
            )
            .on_hover_text(
                "The preview engine hit a bug on one of these files. Change the pipeline or \
                 the listing to try again; if it keeps happening, please report it.",
            );
        }

        ui.separator();
        for filter in RowFilter::ALL {
            if ui
                .selectable_label(session.settings.row_filter == filter, filter.label())
                .clicked()
            {
                out.row_filter = Some(filter);
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let blocked = if job.is_some() {
                Some("A run is still going.".to_owned())
            } else {
                blocked_reason(counts, pipeline_empty, session.guarded.as_deref(), failure)
            };
            // "Run simulation", not "Simulate": the checkbox beside it is
            // already called Simulate, and two adjacent controls with the same
            // word on them do not say which is the verb.
            let label = if *simulate {
                "Run simulation"
            } else {
                "Rename"
            };
            // The one primary action in the window, and the only irreversible
            // one. `RichText::strong()` was its whole emphasis, which left it
            // dimmer than the *Simulate* label beside it and indistinguishable
            // from disabled *Undo* in the far corner. A simulation is not
            // irreversible, so it does not get the accent.
            let accent = crate::theme::Accents::from_ui(ui);
            let mut run =
                egui::Button::new(egui::RichText::new(label).strong().color(if *simulate {
                    ui.visuals().widgets.inactive.fg_stroke.color
                } else {
                    accent.on_primary
                }))
                .min_size(egui::vec2(84.0, 0.0));
            if !*simulate {
                run = run.fill(accent.primary);
            }
            let button = ui.add_enabled(blocked.is_none(), run);
            if let Some(reason) = &blocked {
                button.on_disabled_hover_text(reason);
            } else if button.on_hover_text("F5").clicked() {
                out.run = true;
            }

            ui.checkbox(simulate, "Simulate")
                .on_hover_text("Run the whole plan without touching the disk");

            let can_undo = history.can_undo() && job.is_none();
            let undo = ui.add_enabled(can_undo, egui::Button::new("Undo"));
            if undo.clicked() {
                out.undo = true;
            }
            if !can_undo {
                undo.on_disabled_hover_text(if job.is_some() {
                    "A run is still going"
                } else {
                    "Nothing to undo yet"
                });
            }

            if !history.log.is_empty() && ui.button("Log").clicked() {
                out.toggle_log = true;
            }
        });
    });

    out
}

/// What this run will do, in clauses.
///
/// Shared with the confirmation dialog so the sentence a user reads before
/// approving a run and the one under the file table cannot drift apart. This is
/// DESIGN S7's *"N renames, M skipped, K conflicts"* vocabulary, already
/// settled here.
///
/// Renames and metadata changes are counted separately: *"5 will change"* for a
/// run that renames three files and re-dates two says nothing useful about
/// either. The suppression rule is the same lesson `blocked_reason` learned —
/// an action-only run changes no *names*, and saying "0 will be renamed" next
/// to "2 will be modified" is noise.
pub(crate) fn plan_clauses(plan: &Plan) -> Vec<String> {
    clauses(plan.counts())
}

fn clauses(counts: Counts) -> Vec<String> {
    let mut parts = Vec::new();
    if counts.changed > 0 || counts.acted == 0 {
        parts.push(format!("{} will be renamed", counts.changed));
    }
    if counts.acted > 0 {
        parts.push(format!("{} will be modified", counts.acted));
    }
    parts
}

fn summary(session: &Session, counts: Option<Counts>) -> String {
    let total = session.entries().len();
    let Some(counts) = counts else {
        return ren_core::plural(total, "item");
    };
    let mut parts = vec![ren_core::plural(total, "item")];
    parts.extend(clauses(counts));
    if counts.conflicts > 0 {
        parts.push(ren_core::plural(counts.conflicts, "conflict"));
    }
    if counts.errors > 0 {
        parts.push(ren_core::plural(counts.errors, "error"));
    }
    if !session.selection.is_empty() {
        parts.push(format!("{} selected", session.selection.len()));
    }
    parts.join(" • ")
}

/// Why the run button is disabled, in words the user can act on.
///
/// Public under a longer name so the app can answer the same question outside a
/// frame — a disabled button's hover text never reaches the accessibility tree.
pub fn blocked_reason_for(
    plan: Option<&Plan>,
    pipeline_empty: bool,
    guarded: Option<&Path>,
    failure: Option<&str>,
) -> Option<String> {
    blocked_reason(plan.map(Plan::counts), pipeline_empty, guarded, failure)
}

fn blocked_reason(
    counts: Option<Counts>,
    pipeline_empty: bool,
    guarded: Option<&Path>,
    failure: Option<&str>,
) -> Option<String> {
    // Before everything, including the empty pipeline: this is the one refusal
    // that is not about the plan being wrong, and it outranks any reason the
    // plan could give (D127).
    if let Some(folder) = guarded {
        return Some(format!(
            "{} belongs to the operating system. Renaming inside it can break this machine in \
             ways undo cannot fix. Turn the guard off in Settings ▸ File System if you mean it.",
            folder.display()
        ));
    }
    // Then the empty pipeline, because it is the identity: every row would come
    // back "unchanged" and the honest-but-useless "Nothing would change" below
    // would hide the actual reason.
    if pipeline_empty {
        return Some("The pipeline is empty — add an operation.".to_owned());
    }
    // A preview that panicked has no plan, and "no plan" alone would read as
    // "nothing listed". Name the failure so the disabled button says why.
    if let Some(failure) = failure {
        return Some(format!("The preview failed: {failure}"));
    }
    let counts = counts?;
    if counts.errors > 0 {
        return Some(format!(
            "{} item(s) could not be previewed. Hover the ✖ badges to see why.",
            counts.errors
        ));
    }
    if counts.conflicts > 0 {
        return Some(format!(
            "{} item(s) would collide. Hover the ⛔ badges to see why — renaming anyway would \
             overwrite files.",
            counts.conflicts
        ));
    }
    // `affected`, not `changed`: an action-only pipeline changes no *names*,
    // and reporting that as "nothing would change" disabled the button with a
    // message that was simply false.
    if counts.affected == 0 {
        return Some("Nothing would change.".to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::{PlanItem, RowState};

    fn plan_with(states: &[RowState]) -> Plan {
        Plan {
            items: states
                .iter()
                .enumerate()
                .map(|(index, state)| PlanItem {
                    index,
                    source: format!("/tmp/{index}").into(),
                    new_name: format!("{index}-new"),
                    target: format!("/tmp/{index}-new").into(),
                    state: state.clone(),
                    actions: Vec::new(),
                })
                .collect(),
            ops: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A plan whose rows keep their names but are acted on — every Set Date and
    /// Set Attributes pipeline.
    fn acted_plan(rows: usize) -> Plan {
        let mut plan = plan_with(&vec![RowState::Unchanged; rows]);
        for item in &mut plan.items {
            item.new_name = format!("{}", item.index);
            item.actions = vec![ren_core::PlannedAction {
                step: 0,
                op: "set_attributes",
                effect: ren_core::Effect::Attributes(ren_platform::AttributeChange {
                    read_only: Some(false),
                    ..Default::default()
                }),
                undoability: ren_core::Undoability::Journaled,
                describe: "Write Protect off".into(),
            }];
        }
        plan
    }

    /// The lie this replaced: `plan.changed()` is 0 for every action-only
    /// pipeline, so the button was disabled with a message saying nothing would
    /// change — at the very moment the run was about to change something.
    #[test]
    fn an_action_only_plan_does_not_claim_nothing_would_change() {
        assert_eq!(
            blocked_reason(Some(acted_plan(3).counts()), false, None, None),
            None
        );
    }

    /// A preview that panicked has no plan, and the button has to say so
    /// rather than look like a folder with nothing listed.
    #[test]
    fn a_failed_preview_names_the_failure() {
        let reason = blocked_reason(None, false, None, Some("boom")).expect("should block");
        assert!(reason.contains("boom"), "{reason}");
    }

    #[test]
    fn a_plan_that_neither_renames_nor_acts_still_says_nothing_would_change() {
        let plan = plan_with(&[RowState::Unchanged, RowState::Unchanged]);
        assert_eq!(
            blocked_reason(Some(plan.counts()), false, None, None).as_deref(),
            Some("Nothing would change.")
        );
    }

    /// "5 will change" for a run that renames three files and re-dates two says
    /// nothing useful about either.
    #[test]
    fn the_status_bar_counts_renames_and_modifications_separately() {
        let mut plan = plan_with(&[RowState::Changed, RowState::Unchanged]);
        plan.items[1].actions = acted_plan(1).items[0].actions.clone();

        assert_eq!(plan.changed(), 1);
        assert_eq!(plan.acted(), 1);
        assert_eq!(plan.affected(), 2);

        // A row that is both renamed and acted on is one affected row.
        plan.items[0].actions = plan.items[1].actions.clone();
        assert_eq!(plan.affected(), 2, "not double-counted");
    }

    #[test]
    fn a_clean_plan_does_not_block_the_button() {
        let plan = plan_with(&[RowState::Changed]);
        assert_eq!(blocked_reason(Some(plan.counts()), false, None, None), None);
    }

    #[test]
    fn a_plan_with_nothing_to_do_says_so() {
        let plan = plan_with(&[RowState::Unchanged]);
        let reason = blocked_reason(Some(plan.counts()), false, None, None).expect("should block");
        assert!(reason.contains("Nothing"), "{reason}");
    }

    /// P4: conflicts hard-block, so the button owes an explanation.
    #[test]
    fn conflicts_block_the_run_with_a_reason_that_names_the_risk() {
        let plan = plan_with(&[
            RowState::Changed,
            RowState::Conflict(ren_core::ConflictKind::TargetExists),
        ]);
        let reason = blocked_reason(Some(plan.counts()), false, None, None).expect("should block");
        assert!(reason.contains("collide"), "{reason}");
        assert!(reason.contains("overwrite"), "{reason}");
    }

    #[test]
    fn errors_block_the_run_too() {
        let plan = plan_with(&[RowState::Error("bad regex".into())]);
        let reason = blocked_reason(Some(plan.counts()), false, None, None).expect("should block");
        assert!(reason.contains("previewed"), "{reason}");
    }

    /// An empty pipeline is the identity, so every row comes back unchanged.
    /// "Nothing would change" is true and useless; say what to do instead.
    #[test]
    fn an_empty_pipeline_blocks_the_run_and_says_what_is_missing() {
        let plan = plan_with(&[RowState::Unchanged]);
        let reason = blocked_reason(Some(plan.counts()), true, None, None).expect("should block");
        assert!(reason.contains("empty"), "{reason}");
        assert!(reason.contains("add an operation"), "{reason}");
        // And it wins over the vaguer reason underneath it.
        assert!(!reason.contains("Nothing would change"), "{reason}");
    }

    #[test]
    fn without_a_plan_the_button_is_simply_unavailable() {
        assert_eq!(
            blocked_reason(None, false, None, None),
            None,
            "no plan yet is not a conflict"
        );
    }
}
