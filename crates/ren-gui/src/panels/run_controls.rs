//! The run's numbers and its controls, in the two places D251 put them.
//!
//! * [`results_bar`], above the file table: what the preview will do — the
//!   counts — and the All / Changed / Conflicts filter over it.
//! * [`footer`], under the pipeline: Undo, Simulate, the log, and the Rename
//!   button, which says how many files it will touch. They used to share a
//!   status bar at the bottom of the window, where the one control the app
//!   exists for sat in the corner furthest from where the pipeline is edited.
//!
//! The Rename button is the one place P4 becomes visible: conflicts *block* a
//! run rather than being skipped one by one, so a disabled button that does not
//! say why would be the app's worst moment — which is why the reason is now
//! printed above it, not only in its hover text.

use std::path::Path;

use ren_core::{Counts, Plan, PlannedOp};

use crate::viewmodel::{History, RowFilter, Session};

/// What the run controls want the app to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunOutput {
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
    /// A relist is out, so the plan on screen is over rows that may be gone
    /// — after a run, over the names it just replaced.
    pub listing: bool,
}

/// The bar above the table: the counts, whether the preview is catching up,
/// and the row filter.
pub fn results_bar(ui: &mut egui::Ui, session: &Session, preview: PreviewState<'_>) -> RunOutput {
    let mut out = RunOutput::default();
    let counts = preview.plan.map(Plan::counts);
    let writes = preview.plan.map_or(0, writes);

    ui.horizontal(|ui| {
        ui.label(summary(session, counts, writes));
        if preview.stale {
            // Deliberately not a spinner: an animated widget asks egui to
            // repaint forever, which never settles in a headless test and
            // burns a core in the real app for a preview that lands in
            // milliseconds.
            ui.label(egui::RichText::new("updating…").weak().italics());
        }
        if let Some(failure) = preview.failure {
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for filter in RowFilter::ALL.iter().rev() {
                if ui
                    .selectable_label(session.settings.row_filter == *filter, filter.label())
                    .clicked()
                {
                    out.row_filter = Some(*filter);
                }
            }
        });
    });
    out
}

/// The foot of the pipeline panel: progress while a job is out, the reason
/// Rename is unavailable, Undo / Simulate / Log, the Rename button, and the
/// last thing the app had to say.
pub fn footer(
    ui: &mut egui::Ui,
    session: &Session,
    preview: PreviewState<'_>,
    history: &History,
    simulate: &mut bool,
    pipeline_empty: bool,
    status: Option<&str>,
) -> RunOutput {
    let mut out = RunOutput::default();
    let PreviewState {
        plan,
        failure,
        job,
        cancelling,
        listing,
        ..
    } = preview;
    let counts = plan.map(Plan::counts);
    let writes = plan.map_or(0, writes);

    let blocked = if job.is_some() {
        Some("A run is still going.".to_owned())
    } else if listing {
        Some("Listing… the preview is for the rows as they were.".to_owned())
    } else {
        blocked_reason(
            counts,
            writes,
            plan.map_or(&[][..], |plan| &plan.blockers[..]),
            pipeline_empty,
            session.guarded.as_deref(),
            failure,
        )
    };

    // One row, whatever is happening, so the panel keeps its height and the
    // buttons stay under the pointer: a run swaps Undo / Simulate / Log for
    // its progress and Cancel.
    ui.horizontal(|ui| {
        if let Some(job) = job {
            // Static, like "updating…": the worker wakes the UI once when the
            // job lands, and the count moves on the frames the user's own
            // input causes. A spinner here would never settle (D26). Undo
            // reports no progress of its own — it has to finish to be exact —
            // and a "0 of N" that never moves would read as stuck.
            let text = match job.kind {
                crate::viewmodel::JobKind::Run => {
                    format!("Renaming {} of {}…", job.done, job.total)
                }
                crate::viewmodel::JobKind::Undo => "Undoing…".to_owned(),
                crate::viewmodel::JobKind::Rollback => "Rolling back…".to_owned(),
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
            return;
        }
        let undo = ui.add_enabled(history.can_undo(), egui::Button::new("Undo"));
        if undo.clicked() {
            out.undo = true;
        }
        if history.can_undo() {
            undo.on_hover_text("F4");
        } else {
            undo.on_disabled_hover_text("Nothing to undo yet");
        }
        ui.checkbox(simulate, "Simulate")
            .on_hover_text("Run the whole plan without touching the disk");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !history.log.is_empty() && ui.button("Log").clicked() {
                out.toggle_log = true;
            }
        });
    });

    // The one primary action in the window, and the only irreversible one:
    // the accent, the full width of the panel and a height of its own. A
    // simulation is not irreversible, so it does not get the accent.
    let accent = crate::theme::Accents::from_ui(ui);
    let label = run_label(*simulate, counts, writes, plan, session);
    let text = egui::RichText::new(label)
        .strong()
        .size(15.0)
        .color(if *simulate {
            ui.visuals().widgets.inactive.fg_stroke.color
        } else {
            accent.on_primary
        });
    let mut run = egui::Button::new(text);
    if !*simulate {
        run = run.fill(accent.primary);
    }
    let button = ui.add_enabled_ui(blocked.is_none(), |ui| {
        ui.add_sized(egui::vec2(ui.available_width(), RUN_HEIGHT), run)
    });
    let button = button.inner;
    if let Some(reason) = &blocked {
        button.on_disabled_hover_text(reason);
    } else if button.on_hover_text("F5").clicked() {
        out.run = true;
    }

    // Printed, not only hovered: a disabled button's hover text is the last
    // place anybody looks, and it never reaches a screen reader. Not during a
    // run, which the row above already explains.
    if job.is_none()
        && let Some(reason) = &blocked
    {
        ui.label(egui::RichText::new(reason).weak().small());
    }
    if let Some(status) = status {
        ui.label(egui::RichText::new(status).weak().small());
    }
    out
}

/// The Rename button's height: the window's one primary action, bigger than
/// every other button in it (D251).
const RUN_HEIGHT: f32 = 36.0;

/// What the run button says: the verb and how many rows it touches, so the
/// button states what will happen before it is pressed (D251).
///
/// "Rename" for a plan that renames anything; "Run on" for one that only
/// modifies (an action-only pipeline renames nothing, and "Rename 3 files"
/// would be a lie). "items" as soon as a folder is among them. A simulation
/// keeps its own verb, because the checkbox beside the button is already
/// called Simulate and the two must not say the same thing.
fn run_label(
    simulate: bool,
    counts: Option<Counts>,
    writes: usize,
    plan: Option<&Plan>,
    session: &Session,
) -> String {
    let counts = counts.unwrap_or_default();
    let touched = counts.affected;
    if touched == 0 {
        return if simulate {
            "Run simulation".to_owned()
        } else if writes > 0 {
            "Run".to_owned()
        } else {
            "Rename".to_owned()
        };
    }
    let folders = plan.is_some_and(|plan| {
        plan.items.iter().any(|item| {
            (item.state.is_changed() || item.acts())
                && session.entries().get(item.index).is_some_and(|e| e.is_dir)
        })
    });
    let what = ren_core::plural(touched, if folders { "item" } else { "file" });
    if simulate {
        format!("Simulate {what}")
    } else if counts.changed > 0 {
        format!("Rename {what}")
    } else {
        format!("Run on {what}")
    }
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
///
/// A file a script writes is its own clause: it is not a row, so neither of
/// the other two counts it, and it is the one thing a script does beyond
/// renaming.
pub(crate) fn plan_clauses(plan: &Plan) -> Vec<String> {
    clauses(plan.counts(), writes(plan))
}

/// How many files the plan asks to write — a script's `done()`, M7.
pub(crate) fn writes(plan: &Plan) -> usize {
    plan.ops
        .iter()
        .filter(|op| matches!(op, PlannedOp::WriteFile { .. }))
        .count()
}

fn clauses(counts: Counts, writes: usize) -> Vec<String> {
    let mut parts = Vec::new();
    if counts.changed > 0 || (counts.acted == 0 && writes == 0) {
        parts.push(format!("{} will be renamed", counts.changed));
    }
    if counts.acted > 0 {
        parts.push(format!("{} will be modified", counts.acted));
    }
    if writes > 0 {
        parts.push(format!(
            "{} will be written",
            ren_core::plural(writes, "file")
        ));
    }
    parts
}

fn summary(session: &Session, counts: Option<Counts>, writes: usize) -> String {
    let total = session.entries().len();
    let Some(counts) = counts else {
        return ren_core::plural(total, "item");
    };
    let mut parts = vec![ren_core::plural(total, "item")];
    parts.extend(clauses(counts, writes));
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
    blocked_reason(
        plan.map(Plan::counts),
        plan.map_or(0, writes),
        plan.map_or(&[][..], |plan| &plan.blockers[..]),
        pipeline_empty,
        guarded,
        failure,
    )
}

fn blocked_reason(
    counts: Option<Counts>,
    writes: usize,
    blockers: &[String],
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
    // A reason that belongs to no row — a script write outside the listed
    // folders. The engine refuses the plan for it (`Plan::is_executable`), so
    // the button has to say it before the user presses.
    if let Some(first) = blockers.first() {
        return Some(first.clone());
    }
    // `affected`, not `changed`: an action-only pipeline changes no *names*,
    // and reporting that as "nothing would change" disabled the button with a
    // message that was simply false. A script that only writes a file is the
    // same case again.
    if counts.affected == 0 && writes == 0 {
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
            blockers: Vec::new(),
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
            blocked_reason(Some(acted_plan(3).counts()), 0, &[], false, None, None),
            None
        );
    }

    /// A preview that panicked has no plan, and the button has to say so
    /// rather than look like a folder with nothing listed.
    #[test]
    fn a_failed_preview_names_the_failure() {
        let reason = blocked_reason(None, 0, &[], false, None, Some("boom")).expect("should block");
        assert!(reason.contains("boom"), "{reason}");
    }

    #[test]
    fn a_plan_that_neither_renames_nor_acts_still_says_nothing_would_change() {
        let plan = plan_with(&[RowState::Unchanged, RowState::Unchanged]);
        assert_eq!(
            blocked_reason(Some(plan.counts()), 0, &[], false, None, None).as_deref(),
            Some("Nothing would change.")
        );
    }

    /// "5 will change" for a run that renames three files and re-dates two says
    /// nothing useful about either.
    #[test]
    fn the_counts_name_renames_and_modifications_separately() {
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
        assert_eq!(
            blocked_reason(Some(plan.counts()), 0, &[], false, None, None),
            None
        );
    }

    #[test]
    fn a_plan_with_nothing_to_do_says_so() {
        let plan = plan_with(&[RowState::Unchanged]);
        let reason =
            blocked_reason(Some(plan.counts()), 0, &[], false, None, None).expect("should block");
        assert!(reason.contains("Nothing"), "{reason}");
    }

    /// P4: conflicts hard-block, so the button owes an explanation.
    #[test]
    fn conflicts_block_the_run_with_a_reason_that_names_the_risk() {
        let plan = plan_with(&[
            RowState::Changed,
            RowState::Conflict(ren_core::ConflictKind::TargetExists),
        ]);
        let reason =
            blocked_reason(Some(plan.counts()), 0, &[], false, None, None).expect("should block");
        assert!(reason.contains("collide"), "{reason}");
        assert!(reason.contains("overwrite"), "{reason}");
    }

    #[test]
    fn errors_block_the_run_too() {
        let plan = plan_with(&[RowState::Error("bad regex".into())]);
        let reason =
            blocked_reason(Some(plan.counts()), 0, &[], false, None, None).expect("should block");
        assert!(reason.contains("previewed"), "{reason}");
    }

    /// An empty pipeline is the identity, so every row comes back unchanged.
    /// "Nothing would change" is true and useless; say what to do instead.
    #[test]
    fn an_empty_pipeline_blocks_the_run_and_says_what_is_missing() {
        let plan = plan_with(&[RowState::Unchanged]);
        let reason =
            blocked_reason(Some(plan.counts()), 0, &[], true, None, None).expect("should block");
        assert!(reason.contains("empty"), "{reason}");
        assert!(reason.contains("add an operation"), "{reason}");
        // And it wins over the vaguer reason underneath it.
        assert!(!reason.contains("Nothing would change"), "{reason}");
    }

    /// A plan the engine refuses for a reason no row carries — a script write
    /// outside the listed folders — must not leave the button enabled, only
    /// for the run to be refused after the press.
    #[test]
    fn a_plan_blocker_disables_the_button_with_its_reason() {
        let mut plan = plan_with(&[RowState::Changed]);
        plan.blockers
            .push("a script asked to write outside the listed folders".into());
        let reason = blocked_reason_for(Some(&plan), false, None, None).expect("should block");
        assert!(reason.contains("outside the listed folders"), "{reason}");
    }

    /// A file a script writes is not a row, so the row counts cannot say it —
    /// and a run that only writes one is not a run that changes nothing.
    #[test]
    fn a_script_write_is_its_own_clause() {
        let mut plan = plan_with(&[RowState::Unchanged]);
        plan.ops.push(PlannedOp::WriteFile {
            path: "/tmp/list.m3u".into(),
            contents: String::new(),
            undoability: ren_core::Undoability::Journaled,
        });
        assert_eq!(plan_clauses(&plan), ["1 file will be written"]);
        assert_eq!(blocked_reason_for(Some(&plan), false, None, None), None);
    }

    #[test]
    fn without_a_plan_the_button_is_simply_unavailable() {
        assert_eq!(
            blocked_reason(None, 0, &[], false, None, None),
            None,
            "no plan yet is not a conflict"
        );
    }
}
