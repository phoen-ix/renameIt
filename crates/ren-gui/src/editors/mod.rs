//! One editor per operation.
//!
//! Each is a plain `fn(&mut Ui, &mut TheOp) -> bool` returning whether anything
//! changed, so it can be dropped into M2's single-operation panel *and* into
//! M4's pipeline cards without alteration (D25).
//!
//! M5's two list-driven operations needed more than the operation itself — a
//! file picker, and the listing their lines pair with — so that arrives as an
//! [`EditorCx`] the dispatcher holds and only the editors that want it read.
//!
//! **Ten of the seventeen take it now**, and the promise D25 made is the one
//! that survives: an editor that needs nothing beyond its operation is not made
//! to carry an argument for one that does. This line used to say "the nine
//! older editors keep their signature untouched", which was already wrong
//! before Visual Assist widened two more — a stated rule the code broke is
//! worse than no rule, so it is a count that can be checked rather than a claim
//! that goes stale.

use std::cell::{Cell, RefCell};

use ren_core::model::{FileEntry, Scope};
use ren_core::ops::OpKind;

use crate::dialogs::FileDialogs;

mod add_counter;
mod add_remove;
pub mod assist;
mod casing;
mod csv_list;
mod filename_editor;
mod free_format;
mod move_section;
mod music_rename;
mod music_tagger;
mod remove_tags;
mod renumber;
mod replace;
mod script;
mod set_attributes;
mod set_date;
mod space_trim;
mod zero_pad;

/// Things an editor can ask the app for that are not edits to its own operation.
///
/// A `Cell` rather than a return value because an editor's contract is
/// `-> bool` and D25 is the reason to keep it that way: widening the signature
/// would touch all fourteen. Read once, after the whole stack has been drawn,
/// so a card cannot open a window in the middle of the frame that drew it.
#[derive(Debug, Default)]
pub struct EditorRequests {
    /// Music Rename's *(edit styles)* link — Settings ▸ Music Styles.
    pub edit_music_styles: Cell<bool>,
    /// Music Tagger's Quick Setup, which configures the run-wide Setup Parts
    /// pattern as well as its own fields — *"a 'quick setup' button that will
    /// automatically perform the above actions for you"*.
    pub set_parts: RefCell<Option<String>>,
    /// Find & Replace's *"add the current Replace function settings to the
    /// list"* — the rule to append to the Batch Replace defaults.
    pub add_batch_rule: RefCell<Option<ren_core::ops::Replace>>,
    /// A ⌖ was clicked, or the strip it opened asked for something.
    ///
    /// `RefCell` rather than `Cell` because the request carries a `String` and
    /// a `PathBuf`. Drained beside the other three, after the whole stack has
    /// been drawn — the strip lives *inside* a card, so acting on it mid-frame
    /// would mutate a card while it is being drawn.
    pub assist: RefCell<Option<AssistRequest>>,
}

/// What a card, or the strip inside it, asked Visual Assist to do.
#[derive(Debug, Clone, PartialEq)]
pub enum AssistRequest {
    /// A ⌖ click. On the one already lit, this means close.
    Point {
        card: crate::viewmodel::CardId,
        target: assist::AssistTarget,
    },
    /// The picker moved.
    Show(std::path::PathBuf),
    /// **Select**: write this and close.
    Commit(assist::AssistSpan),
    /// Re-express the position it just wrote as a distance from the end.
    AnchorToEnd(assist::AssistSpan),
    /// **Cancel**, or the lit ⌖ clicked again.
    Close,
}

/// What an editor may need beyond the operation it is editing.
#[derive(Clone, Copy)]
pub struct EditorCx<'a> {
    /// The listing, in the order it is showing.
    pub entries: &'a [FileEntry],
    /// The indices the preview is actually computed over — the selection, or
    /// everything when nothing is selected (P22). The Filename Editor's lines
    /// pair with *this*, so its copy link has to produce exactly this list.
    pub scoped: &'a [usize],
    pub dialogs: &'a dyn FileDialogs,
    /// Greying what this machine cannot do, and saying why (D40).
    pub platform: &'a dyn ren_platform::Platform,
    /// The scope of the card being drawn. The Filename Editor's copy link has
    /// to write the slice its operation will be *handed* — writing whole names
    /// into a card scoped to the stem would append every extension twice on the
    /// first run.
    pub scope: Scope,
    /// Settings ▸ Music Styles: the list Music Rename's radios offer. Shared
    /// rather than copied into the operation, the same bargain `batch_replace`
    /// struck — a card stores the *pattern* it ended up with, so a preset stays
    /// self-contained (D35) even on a machine whose styles have been edited.
    pub music_styles: &'a [String],
    /// What each drop-down field has been run with, newest first.
    ///
    /// The whole map rather than one field's list, because a card does not know
    /// which of its boxes have a history until it draws them.
    pub field_history: &'a std::collections::BTreeMap<String, Vec<String>>,
    pub requests: &'a EditorRequests,
    /// The card being drawn. The one thing an editor cannot work out for
    /// itself: all eighteen are drawn by the same function, and two cards of a
    /// kind is the normal case.
    pub card: crate::viewmodel::CardId,
    /// Visual Assist, when it is open **on this card** — `None` everywhere
    /// else, so no editor ever compares a `CardId` itself.
    pub assist: Option<&'a crate::panels::visual_assist::VisualAssist>,
}

impl<'a> EditorCx<'a> {
    /// The entries the preview will hand to an operation, in order.
    pub(crate) fn listed(&self) -> Vec<FileEntry> {
        self.scoped
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .cloned()
            .collect()
    }

    /// The same context, for a card with its own identity, scope and strip.
    /// One field's remembered strings.
    pub(crate) fn history(&self, id: &str) -> &[String] {
        self.field_history.get(id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn for_card(
        self,
        card: crate::viewmodel::CardId,
        scope: Scope,
        assist: Option<&'a crate::panels::visual_assist::VisualAssist>,
    ) -> Self {
        Self {
            card,
            scope,
            assist,
            ..self
        }
    }

    /// The strip's state, when it is open on this card for `target`.
    pub(crate) fn assist_for(
        &self,
        target: assist::AssistTarget,
    ) -> Option<&crate::panels::visual_assist::VisualAssist> {
        self.assist.filter(|state| state.target == target)
    }
}

/// The ⌖ beside a field Visual Assist can fill.
///
/// Lit while the strip is open on **this** card for **this** target, and a
/// second click closes it — the same toggle the card summary is. It never names
/// a `CardId` itself: `cx` already knows which card is being drawn, which is
/// what stops a button on one card opening a strip on another.
///
/// Labelled per target rather than all "⌖", because `kittest`'s `get_by_label`
/// panics on two matches and an Add & Remove card in *Both* mode has two.
pub(crate) fn assist_button(ui: &mut egui::Ui, cx: &EditorCx<'_>, target: assist::AssistTarget) {
    let lit = cx.assist_for(target).is_some();
    let label = match target {
        assist::AssistTarget::ReplaceFind => "⌖ find",
        assist::AssistTarget::AddPos => "⌖ position",
        assist::AssistTarget::RemoveSection => "⌖ section",
        assist::AssistTarget::MoveCut => "⌖ cut",
    };
    if ui
        .selectable_label(lit, label)
        .on_hover_text(format!("{} (F3)", target.prompt()))
        .clicked()
    {
        *cx.requests.assist.borrow_mut() = Some(AssistRequest::Point {
            card: cx.card,
            target,
        });
    }
}

/// Draws the editor for whichever operation this is. Returns true if the user
/// changed something, which is the app's cue to recompute the preview.
pub fn ui(ui: &mut egui::Ui, op: &mut OpKind, cx: &EditorCx<'_>) -> bool {
    match op {
        OpKind::Replace(inner) => replace::ui(ui, inner, cx),
        OpKind::BatchReplace(inner) => replace::batch_ui(ui, inner),
        OpKind::Casing(inner) => casing::ui(ui, inner),
        OpKind::AddRemove(inner) => add_remove::ui(ui, inner, cx),
        OpKind::MoveSection(inner) => move_section::ui(ui, inner, cx),
        OpKind::SpaceTrim(inner) => space_trim::ui(ui, inner),
        OpKind::AddCounter(inner) => add_counter::ui(ui, inner),
        OpKind::ReNumber(inner) => renumber::ui(ui, inner),
        OpKind::ZeroPadding(inner) => zero_pad::ui(ui, inner),
        OpKind::FreeFormat(inner) => free_format::ui(ui, inner, cx),
        OpKind::MusicRename(inner) => music_rename::ui(ui, inner, cx),
        OpKind::MusicTagger(inner) => music_tagger::ui(ui, inner, cx),
        OpKind::RemoveTags(inner) => remove_tags::ui(ui, inner),
        OpKind::CsvList(inner) => csv_list::ui(ui, inner, cx.dialogs),
        OpKind::FilenameEditor(inner) => filename_editor::ui(ui, inner, cx),
        OpKind::SetAttributes(inner) => set_attributes::ui(ui, inner, cx.platform),
        OpKind::SetDate(inner) => set_date::ui(ui, inner, cx.platform),
        OpKind::Script(inner) => script::ui(ui, inner, cx),
    }
}

/// A labelled integer box, which every position-based operation needs.
pub(crate) fn number(ui: &mut egui::Ui, label: &str, value: &mut usize) -> bool {
    ui.label(label);
    crate::widgets::number::add(ui, egui::DragValue::new(value).range(0..=99_999).speed(0.2))
        .changed()
}
