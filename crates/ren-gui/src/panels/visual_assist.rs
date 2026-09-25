//! The Visual Assist strip — picking a field's text, position or span by
//! selecting it in the name the card is handed.
//!
//! Five controls — a combo box of filenames, a prompt, a live `Position: N`
//! readout, and buttons captioned **Select** and **Cancel** — drawn inside the
//! card whose field they fill rather than in a window of their own (D143) — which is what
//! `docs/DESIGN.md` §S4 asked for when it said *"instead of a separate
//! window"*, and not what it asked for in the next clause.
//!
//! # Why the text is not the file name
//!
//! A position field is measured on the operation's **input**. With one
//! operation at a time that is always the file name; this app has a card stack
//! (D8), and the input to card N is the output of cards 1..N−1 — a
//! string that appears in neither column of the file table.
//! `Pipeline::subject_at` is the engine answering that, scope and pre-processor
//! already applied, so the offsets this strip reports are the offsets the
//! fields mean and nothing on the path converts.
//!
//! # Why the field is a read-only `TextEdit`
//!
//! `impl TextBuffer for &str` has `is_mutable() == false`, which gives real
//! mouse-drag and keyboard selection with no editing and no custom hit-testing,
//! and `CCursorRange` is already in **characters** — the unit the position
//! fields use. Three consequences worth knowing:
//!
//! * `.interactive(false)` is *not* the same thing. It downgrades the sense to
//!   hover-only and kills selection outright.
//! * The caret-blink block is gated on `is_mutable()`, so a read-only field
//!   never requests a repaint. That is what keeps `Harness::run`, which panics
//!   rather than warns when it cannot reach a still frame, happy (D26).
//! * It also paints **no caret**, so a click that selects nothing shows
//!   nothing. Hence `Position: -` as the empty state, and a **Select** that
//!   refuses.
//!
//! # Why the selection is cached
//!
//! `TextEditOutput::cursor_range` is `Some` only while the field has focus, and
//! egui *collapses the stored selection to a bare caret* the first time the
//! field is drawn without it (`builder.rs:538`). Reading it when Select is
//! clicked returns a zero-length range — `delete = 0`, `find = ""`, both silent
//! no-ops. It happens to work by the accident that `AtomLayout` measures before
//! it interacts, which egui documents nowhere. So the range is read from
//! `state.cursor.range(&galley)` on every frame it is live and kept here, and
//! `range(&galley)` clamps — which is what saves us when the picker switches to
//! a shorter name with a longer selection stored.

use std::path::{Path, PathBuf};

use ren_core::NoSubject;

use crate::editors::assist::{AssistSpan, AssistTarget};
use crate::viewmodel::CardId;

/// How many names the picker offers.
///
/// A fixed constant rather than a setting: which name you mark up changes
/// nothing about the result, so the picker is a convenience rather than a
/// parameter of the run.
pub const MAX_ITEMS: usize = 200;

/// One name the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub path: PathBuf,
}

/// Visual Assist, while the strip is open.
///
/// An `Option<VisualAssist>` on `RenameItApp` beside `palette`, `asking`,
/// `confirming` and `drawer` — the shape every other mode has. Unlike those it
/// is not drawn at the top of `show()`: it lives inside a card, so the card
/// draws it and this holds only what the card cannot work out for itself.
/// Session state; never persisted.
#[derive(Debug)]
pub struct VisualAssist {
    /// **Identity, never an index.** The card can be dragged, duplicated or
    /// deleted while this is open.
    pub card: CardId,
    pub target: AssistTarget,
    /// **A path, never a row.** F9, a re-sort, a reorder and a selection change
    /// all renumber the rows underneath; the path does not move. Storing a
    /// `usize` here is the bug this field exists to prevent.
    pub file: PathBuf,
    /// The names on offer, rebuilt when the listing or the selection changes.
    pub choices: Vec<Choice>,
    /// How many names the cap dropped, for the "first N of M" line.
    pub capped: usize,
    /// The text the card's operation is handed for [`Self::file`], or why it is
    /// handed none. Recomputed once per preview generation, never per frame.
    pub subject: Result<String, NoSubject>,
    /// True when an enabled step above this card is order-sensitive, so its
    /// contribution is missing from `subject` and the strip has to say so.
    pub unreplayed_script: bool,
}

/// The selection, cached where the widget that makes it can reach it.
///
/// Not on `VisualAssist`, because that is the *app's* state and the strip is
/// drawn from a shared reference deep inside a card. This is per-widget UI
/// state — the same bargain `rule_table` and `music_tagger` already strike with
/// `ui.data_mut` — and it is keyed off the field's own fixed id, so there is
/// exactly one.
fn selection_of(ctx: &egui::Context) -> Option<(usize, usize)> {
    ctx.data(|d| d.get_temp(field_id().with("selection")))
}

pub fn remember_selection(ctx: &egui::Context, selection: Option<(usize, usize)>) {
    ctx.data_mut(|d| match selection {
        Some(range) => {
            d.insert_temp(field_id().with("selection"), range);
        }
        None => {
            d.remove_temp::<(usize, usize)>(field_id().with("selection"));
            // Focus is re-requested when the strip next opens.
            d.remove_temp::<bool>(field_id().with("focused"));
        }
    });
}

/// Puts a selection into the field itself.
///
/// The **widget's** cursor, not a cache beside it: a seeded selection and one
/// made by dragging then take exactly the same path, so the test seam exercises
/// what a user exercises rather than a parallel road that can rot.
pub fn set_selection(ctx: &egui::Context, start: usize, len: usize) {
    let id = field_id();
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(start + len),
        )));
    egui::TextEdit::store_state(ctx, id, state);
    remember_selection(ctx, Some((start, len)));
}

/// Drops a selection made in a string that is no longer on screen.
///
/// A range into text that has changed is worse than no range at all: it still
/// points somewhere, and where it points is wrong.
pub fn forget_selection(ctx: &egui::Context) {
    remember_selection(ctx, None);
    let id = field_id();
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        state.cursor.set_char_range(None);
        egui::TextEdit::store_state(ctx, id, state);
    }
}

impl VisualAssist {
    pub fn new(card: CardId, target: AssistTarget, file: PathBuf) -> Self {
        Self {
            card,
            target,
            file,
            choices: Vec::new(),
            capped: 0,
            subject: Err(NoSubject::NoSuchStep),
            unreplayed_script: false,
        }
    }

    /// The text on screen, when there is any.
    pub fn text(&self) -> Option<&str> {
        self.subject.as_deref().ok()
    }

    /// The selection as a span of the subject, if one has been made.
    pub fn span(&self, selection: Option<(usize, usize)>) -> Option<AssistSpan> {
        let text = self.text()?;
        let (start, len) = selection?;
        Some(AssistSpan::cut(text, start, len))
    }

    /// Whether **Select** would do anything.
    pub fn can_commit(&self, selection: Option<(usize, usize)>) -> bool {
        self.span(selection)
            .is_some_and(|span| self.target.accepts_empty() || !span.is_empty())
    }

    /// Points the strip at a different target on the same card.
    ///
    /// The file and the picker are kept. The selection is dropped by the
    /// caller, because the prompt changed and so has what a span would mean.
    pub fn point_at(&mut self, target: AssistTarget) {
        self.target = target;
    }

    /// Shows a different file.
    pub fn show(&mut self, file: PathBuf) {
        self.file = file;
    }
}

/// What the strip asked the app to do this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Still open, nothing asked.
    Open,
    /// The picker moved.
    Show(PathBuf),
    /// **Select** — write this and close.
    Commit(AssistSpan),
    /// The selection touched the end and the user took the offer.
    AnchorToEnd(AssistSpan),
    /// **Cancel**.
    Close,
}

/// Why there is nothing to select in, in words the user can act on.
fn refusal(why: NoSubject) -> &'static str {
    match why {
        NoSubject::NoSuchStep => "This card is no longer in the pipeline.",
        NoSubject::Disabled => "This card is switched off, so nothing is handed to it.",
        NoSubject::FilteredOut => "This card's include filter skips this file.",
        NoSubject::NotANameTransform => "This operation changes the file, not its name.",
        NoSubject::ScopeDoesNotApply => "This file has no extension, so this card skips it.",
        NoSubject::PreProcessorRejected => "The pre-processor matches nothing in this file.",
    }
}

/// The id of the read-only field.
///
/// Fixed rather than salted, the `source_bar::address_box()` pattern: only one
/// strip is ever open, and the selection this module caches is keyed off it.
/// F3 does not need it — it is handled above the guard that stands the other
/// shortcuts down while a text field has focus.
pub fn field_id() -> egui::Id {
    egui::Id::new("renameit_visual_assist_subject")
}

/// Draws the strip. Returns what the user asked for, if anything.
///
/// Takes the state by shared reference: everything mutable about the strip is
/// either the app's (recomputed once per preview generation) or the widget's
/// own (the selection, in egui's store). That is what lets a card deep inside
/// the pipeline panel draw it without reaching the app.
///
/// `find_reads_wildcards` is true when the target is a Find box that is *not*
/// a regex: there `*`, `:` and `?` are wildcards, and a selection holding one
/// would be written into it verbatim — see [`wildcard_note`].
pub fn ui(ui: &mut egui::Ui, state: &VisualAssist, find_reads_wildcards: bool) -> Outcome {
    let mut outcome = Outcome::Open;
    let mut selection = selection_of(ui.ctx());

    ui.group(|ui| {
        ui.label(egui::RichText::new("Visual Assist").strong().small());

        // The picker exists because the first file in a folder is often the
        // least representative one to measure against.
        if state.choices.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("File:");
                let showing = state
                    .choices
                    .iter()
                    .find(|c| c.path == state.file)
                    .map_or("—", |c| c.name.as_str())
                    .to_owned();
                egui::ComboBox::from_id_salt("visual_assist_file")
                    .selected_text(showing)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for choice in &state.choices {
                            let picked = choice.path == state.file;
                            if ui.selectable_label(picked, &choice.name).clicked() && !picked {
                                outcome = Outcome::Show(choice.path.clone());
                            }
                        }
                    });
            });
            if state.capped > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "The first {} of {} — the positions apply to every file.",
                        state.choices.len(),
                        state.choices.len() + state.capped
                    ))
                    .weak()
                    .small(),
                );
            }
        }

        ui.label(state.target.prompt());

        match &state.subject {
            Err(why) => {
                ui.label(egui::RichText::new(refusal(*why)).weak());
            }
            Ok(subject) => {
                let mut shown: &str = subject.as_str();
                // `multiline`, deliberately. It reaches the accessibility tree
                // as `MultilineTextInput`, so this field can never renumber the
                // `Role::TextInput` ordinals the test suite indexes by; and
                // `singleline` sets `clip_text`, which would hide the end of a
                // long name with no way to reach it.
                let output = egui::TextEdit::multiline(&mut shown)
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .id(field_id())
                    .font(egui::TextStyle::Monospace)
                    // The caret starts at the beginning, not at the end.
                    // `TextEdit`'s default suits a box you are about to type
                    // into; this one is a ruler, and "position 0" is where
                    // reading a name starts.
                    .cursor_at_end(false)
                    .show(ui);

                // Once, not every frame: a frame that keeps asking for focus
                // never settles, and `Harness::run` panics rather than warns.
                if ui.ctx().data_mut(|d| {
                    !d.get_temp::<bool>(field_id().with("focused"))
                        .unwrap_or(false)
                }) {
                    output.response.request_focus();
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(field_id().with("focused"), true));
                }
                // Only while it is live — see the module docs.
                if let Some(range) = output.state.cursor.range(&output.galley) {
                    let chars = range.as_sorted_char_range();
                    selection = Some((chars.start.0, chars.end.0 - chars.start.0));
                    remember_selection(ui.ctx(), selection);
                }
            }
        }

        // The readout, as two separate labels, so a failure names which
        // number is wrong.
        ui.horizontal(|ui| match selection.filter(|_| state.text().is_some()) {
            Some((start, len)) => {
                ui.label(format!("Position: {start}"));
                if !matches!(state.target, AssistTarget::AddPos) {
                    ui.label(format!("Selection length: {len}"));
                }
            }
            None => {
                ui.label("Position: -");
            }
        });

        if find_reads_wildcards
            && state.target == AssistTarget::ReplaceFind
            && let Some(note) = state
                .span(selection)
                .and_then(|span| wildcard_note(&span.text))
        {
            ui.label(
                egui::RichText::new(note)
                    .color(ui.visuals().warn_fg_color)
                    .small(),
            );
        }

        if state.unreplayed_script {
            ui.label(
                egui::RichText::new("A script above this card is not reflected here.")
                    .weak()
                    .small(),
            );
        }

        ui.horizontal(|ui| {
            let commit = ui.add_enabled(state.can_commit(selection), egui::Button::new("Select"));
            if commit.clicked()
                && let Some(span) = state.span(selection)
            {
                outcome = Outcome::Commit(span);
            }
            if !state.can_commit(selection) {
                commit.on_disabled_hover_text(if state.text().is_none() {
                    "There is nothing to select in"
                } else {
                    "Select some text first"
                });
            }
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Close;
            }

            // The one place backwards is the better answer, offered rather than
            // done: `pos 0 from ending` appends to every name in the listing,
            // whatever its length.
            if let (Some(span), Some(text)) = (state.span(selection), state.text())
                && span.touches_the_end(text.chars().count())
                && !matches!(state.target, AssistTarget::ReplaceFind)
                && ui
                    .button("Anchor to the end")
                    .on_hover_text(
                        "Measure this from the end of each name instead, so it lands in the \
                         right place whatever a file is called",
                    )
                    .clicked()
            {
                outcome = Outcome::AnchorToEnd(span);
            }
        });
    });

    outcome
}

/// What a selection holding wildcard characters would mean in a Find box that
/// reads wildcards, or `None` when it holds none.
///
/// Said *before* Select rather than after: the wildcard language has no escape
/// ([`wildcards_in`](crate::editors::assist::wildcards_in)), so the text would
/// go into the box verbatim and match more than it says — and the one box that
/// can hold it literally is a tick away. Only reachable where a name can hold
/// `*`, `:` or `?`, which is everywhere but Windows.
fn wildcard_note(text: &str) -> Option<String> {
    let found = crate::editors::assist::wildcards_in(text);
    if found.is_empty() {
        return None;
    }
    let listed: Vec<String> = found.iter().map(char::to_string).collect();
    Some(format!(
        "The selection contains {} — a wildcard in the Find box. To match it as it is, tick \
         Regular expression before Select.",
        listed.join(" ")
    ))
}

/// The names the picker offers, and how many the cap dropped.
pub fn choices(entries: &[ren_core::model::FileEntry], scoped: &[usize]) -> (Vec<Choice>, usize) {
    // `scoped`, not the whole listing: `EvalCx::index`/`total` are positions in
    // the scoped list, so a name from outside it would render an earlier
    // `<Counter>` as a number the run will never produce.
    let all: Vec<Choice> = scoped
        .iter()
        .filter_map(|&i| entries.get(i))
        .map(|entry| Choice {
            name: entry.file_name.clone(),
            path: entry.path.clone(),
        })
        .collect();
    let capped = all.len().saturating_sub(MAX_ITEMS);
    (all.into_iter().take(MAX_ITEMS).collect(), capped)
}

/// The file the strip should open on: the one already showing if it is still
/// listed, else the first.
pub fn opening_file(choices: &[Choice], showing: Option<&Path>) -> Option<PathBuf> {
    showing
        .filter(|path| choices.iter().any(|c| c.path == *path))
        .map(Path::to_path_buf)
        .or_else(|| choices.first().map(|c| c.path.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ren_core::model::FileEntry;

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry::synthetic(PathBuf::from("/tmp").join(n)))
            .collect()
    }

    fn a_card() -> CardId {
        let mut stack = crate::viewmodel::CardStack::default();
        stack.push(ren_core::ops::OpKind::default())
    }

    fn open(subject: &str) -> VisualAssist {
        let mut state = VisualAssist::new(
            a_card(),
            AssistTarget::RemoveSection,
            PathBuf::from("/tmp/a.txt"),
        );
        state.subject = Ok(subject.to_owned());
        state
    }

    /// The picker offers what the run will actually touch, and the cap says so
    /// rather than silently showing the first two hundred.
    #[test]
    fn the_picker_offers_the_scoped_rows_and_says_when_it_capped() {
        let listing = entries(&["a.txt", "b.txt", "c.txt"]);
        let (some, capped) = choices(&listing, &[1, 2]);
        assert_eq!(
            some.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["b.txt", "c.txt"],
            "the selection, not the listing"
        );
        assert_eq!(capped, 0);

        let many = entries(
            &(0..MAX_ITEMS + 7)
                .map(|i| format!("f{i:04}.txt"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let all: Vec<usize> = (0..many.len()).collect();
        let (shown, capped) = choices(&many, &all);
        assert_eq!(shown.len(), MAX_ITEMS);
        assert_eq!(capped, 7);
    }

    /// A file that left the listing must not leave the strip pointed at
    /// nothing — F9 and a selection change both do this.
    #[test]
    fn a_file_that_is_no_longer_listed_falls_back_to_the_first() {
        let listing = entries(&["a.txt", "b.txt"]);
        let (choices, _) = choices(&listing, &[0, 1]);

        let still_there = PathBuf::from("/tmp/b.txt");
        assert_eq!(
            opening_file(&choices, Some(&still_there)),
            Some(still_there.clone())
        );
        assert_eq!(
            opening_file(&choices, Some(Path::new("/tmp/gone.txt"))),
            Some(PathBuf::from("/tmp/a.txt"))
        );
        assert_eq!(opening_file(&[], Some(&still_there)), None);
    }

    /// Three of the four targets do nothing with an empty selection, so Select
    /// has to refuse rather than appear to work.
    #[test]
    fn select_refuses_an_empty_selection_except_for_a_caret() {
        let mut state = open("holiday");
        assert!(!state.can_commit(None), "nothing selected yet");

        assert!(
            !state.can_commit(Some((2, 0))),
            "a span target needs a span"
        );

        state.target = AssistTarget::AddPos;
        assert!(state.can_commit(Some((2, 0))), "a caret is the point");

        state.target = AssistTarget::RemoveSection;
        assert!(state.can_commit(Some((2, 3))));
        assert_eq!(state.span(Some((2, 3))).unwrap().text, "lid");
    }

    /// There is nothing to select in, and the reason is the user's own
    /// configuration in four cases out of six.
    #[test]
    fn every_reason_there_is_no_text_says_something_actionable() {
        for why in [
            NoSubject::NoSuchStep,
            NoSubject::Disabled,
            NoSubject::FilteredOut,
            NoSubject::NotANameTransform,
            NoSubject::ScopeDoesNotApply,
            NoSubject::PreProcessorRejected,
        ] {
            assert!(refusal(why).len() > 20, "{why:?}");
        }

        let mut state = open("x");
        state.subject = Err(NoSubject::Disabled);
        assert!(state.text().is_none());
        assert!(!state.can_commit(Some((0, 3))), "nothing to select in");
    }

    /// Re-pointing keeps the file; the caller drops the selection, because the
    /// prompt changed and so has what a span would mean.
    #[test]
    fn re_pointing_keeps_the_file() {
        let mut state = open("holiday");
        state.point_at(AssistTarget::AddPos);
        assert_eq!(state.target, AssistTarget::AddPos);
        assert_eq!(state.file, PathBuf::from("/tmp/a.txt"));
    }

    /// A range into a string that no longer exists is worse than no range —
    /// it still points somewhere, and where it points is wrong.
    #[test]
    fn a_selection_is_forgotten_rather_than_carried_to_another_string() {
        let ctx = egui::Context::default();
        remember_selection(&ctx, Some((0, 3)));
        assert_eq!(selection_of(&ctx), Some((0, 3)));
        forget_selection(&ctx);
        assert_eq!(selection_of(&ctx), None);
    }
}
