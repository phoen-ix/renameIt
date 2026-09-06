//! The pipeline panel — an ordered stack of operation cards.
//!
//! `docs/DESIGN.md` S1: *"Vertical stack of operation cards: drag-handle to
//! reorder, checkbox to enable/disable (instant preview reflects it), name,
//! one-line summary, overflow menu (duplicate, delete, per-op scope). Selecting
//! a card opens its editor inline (card expands) — no separate inspector to
//! keep spatial locality."*
//!
//! D25 promised M4 would add "only the stack chrome (cards, reorder, enable)
//! and rewrite none of the editors". It kept: every `editors::*` function is
//! called here unchanged. The one thing that makes that work is
//! [`egui::Ui::push_id`] around each card body — every editor uses hard-coded
//! widget ids, and duplicating a card is a feature, so two cards of one kind is
//! the normal case rather than the edge case.

use ren_core::model::Scope;

use crate::editors::{self, EditorCx};
use crate::viewmodel::{Card, CardId, CardStack};
use crate::widgets::filter_editor::FilterForm;

/// What a card asked the panel to do, applied after the loop so the list is not
/// mutated while it is being drawn.
enum Command {
    Move { from: usize, to: usize },
    Duplicate(usize),
    Delete(usize),
    Expand(Option<CardId>),
}

/// The payload a dragged card carries. A newtype, not a bare index: the file
/// table will want row dragging too, and `DragAndDrop` matches on the type.
#[derive(Debug, Clone, Copy)]
struct CardDrag(usize);

/// Draws the stack. Returns true if anything changed the preview.
pub fn ui(
    ui: &mut egui::Ui,
    stack: &mut CardStack,
    expanded: &mut Option<CardId>,
    cx: &EditorCx<'_>,
    assist: Option<&crate::panels::visual_assist::VisualAssist>,
) -> bool {
    let mut changed = false;
    let mut commands: Vec<Command> = Vec::new();

    egui::ScrollArea::vertical()
        .id_salt("pipeline")
        // Vertically shrink-to-fit, capped. `[false, false]` reserved the full
        // 55 % whatever was in it, so a single collapsed card left 338 pt of
        // empty panel between itself and "+ Add operation" — which then sat
        // level with nothing, a third of the way down a column the user reads
        // top to bottom. Horizontally it still fills, so cards are panel-wide.
        .auto_shrink([false, true])
        .max_height(ui.available_height() * 0.55)
        .show(ui, |ui| {
            if stack.is_empty() {
                empty_state(ui);
                return;
            }
            for index in 0..stack.len() {
                changed |= card_ui(ui, stack, index, expanded, &mut commands, cx, assist);
                ui.add_space(4.0);
            }
        });

    for command in commands {
        match command {
            Command::Move { from, to } => changed |= stack.move_card(from, to),
            Command::Duplicate(index) => {
                if let Some(id) = stack.duplicate(index) {
                    *expanded = Some(id);
                    changed = true;
                }
            }
            Command::Delete(index) => {
                if stack.remove(index).is_some() {
                    if expanded.is_some_and(|id| stack.index_of(id).is_none()) {
                        *expanded = stack.cards().first().map(|c| c.id);
                    }
                    changed = true;
                }
            }
            Command::Expand(id) => *expanded = id,
        }
    }

    changed
}

fn empty_state(ui: &mut egui::Ui) {
    egui::Frame::new()
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(4.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No operations yet")
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.label(
                    egui::RichText::new("Add one to start renaming.")
                        .weak()
                        .small(),
                );
            });
        });
}

/// Turns what the strip reported into what the app should do.
fn request_for(outcome: crate::panels::visual_assist::Outcome) -> Option<editors::AssistRequest> {
    use crate::panels::visual_assist::Outcome;
    match outcome {
        Outcome::Open => None,
        Outcome::Show(path) => Some(editors::AssistRequest::Show(path)),
        Outcome::Commit(span) => Some(editors::AssistRequest::Commit(span)),
        Outcome::AnchorToEnd(span) => Some(editors::AssistRequest::AnchorToEnd(span)),
        Outcome::Close => Some(editors::AssistRequest::Close),
    }
}

fn card_ui(
    ui: &mut egui::Ui,
    stack: &mut CardStack,
    index: usize,
    expanded: &Option<CardId>,
    commands: &mut Vec<Command>,
    cx: &EditorCx<'_>,
    assist: Option<&crate::panels::visual_assist::VisualAssist>,
) -> bool {
    let mut changed = false;
    let Some(card) = stack.get(index) else {
        return false;
    };
    let id = card.id;
    let is_expanded = *expanded == Some(id);
    let last = index + 1 == stack.len();

    // Everything inside is salted with the card's identity, which is what lets
    // two cards of the same kind keep their own text cursors and popups.
    let frame = egui::Frame::group(ui.style());
    let (_, dropped) = ui.dnd_drop_zone::<CardDrag, _>(frame, |ui| {
        ui.push_id(id, |ui| {
            ui.horizontal(|ui| {
                // The handle alone is the drag source: `dnd_drag_source` senses
                // dragging on everything inside it, which would swallow the
                // checkbox and the menu.
                ui.dnd_drag_source(egui::Id::new(("card_handle", id)), CardDrag(index), |ui| {
                    let _ = ui.add(
                        crate::widgets::icons::IconButton::new(
                            crate::widgets::icons::Icon::Grip,
                            "Drag to reorder",
                        )
                        .frameless(),
                    );
                })
                .response
                .on_hover_text("Drag to reorder");

                let card = stack.get_mut(index).expect("index checked above");
                changed |= ui
                    .checkbox(&mut card.enabled, card.op.label())
                    .on_hover_text("Include this operation in the run")
                    .changed();

                if summary_toggle(ui, is_expanded, &card.summary(), card.enabled).clicked() {
                    commands.push(Command::Expand((!is_expanded).then_some(id)));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    menu(ui, index, last, commands);
                });
            });

            // A card that will not compile says so while it is collapsed: with
            // eight of them, "which one is broken" must not need eight clicks.
            if let Some(problem) = trouble(stack.get(index).expect("index checked")) {
                ui.label(
                    egui::RichText::new(problem)
                        .color(ui.visuals().error_fg_color)
                        .small(),
                );
            }

            if is_expanded {
                ui.separator();
                let card = stack.get_mut(index).expect("index checked above");
                // The card's own identity and scope: the scope so the Filename
                // Editor's copy link writes the slice its operation will
                // actually be handed, the identity so a ⌖ can say which card it
                // was clicked on. `assist` is `Some` on at most one card.
                // Belt and braces: the accordion draws one card's body at a
                // time and the app closes the strip the moment its card
                // collapses, so this filter is not reachable today. It is the
                // correct statement of what `assist` means, and it costs a
                // comparison — but no test covers it, and pretending otherwise
                // would be worse than saying so.
                let mine = assist.filter(|state| state.card == card.id);
                let card_cx = cx.for_card(card.id, card.scope, mine);
                changed |= editors::ui(ui, &mut card.op, &card_cx);
                ui.add_space(6.0);
                changed |= scope_ui(ui, card);

                // **Last in the body, deliberately.** `Ui::next_auto_id_salt`
                // increments per widget and `editors::number` uses a bare
                // `DragValue` with no salt, so a strip appearing *above* the
                // spinners would re-id every one of them — dropping focus and
                // any half-typed number the moment it opened.
                if let Some(state) = mine {
                    ui.add_space(6.0);
                    let outcome = crate::panels::visual_assist::ui(ui, state);
                    if let Some(request) = request_for(outcome) {
                        cx.requests.ask_assist(request);
                    }
                }
            }
        });
    });

    if let Some(drag) = dropped {
        commands.push(Command::Move {
            from: drag.0,
            to: index,
        });
    }
    changed
}

/// The card's summary, and the control that opens it.
///
/// Was a `selectable_label`, which egui fills with `visuals.selection.bg_fill`
/// when selected — so an expanded card wore the same accent slab as the source
/// mode, the Files/Folders chips, the List/Grid toggle, the row filter and the
/// settings tabs, and `Delete ""` read as a badge saying something rather than
/// a control you could press. The text was `RichText::weak()` *on* that fill,
/// which made it the worst contrast in the window at 1.57:1.
///
/// A disclosure caret and plain text instead. One widget rather than a caret
/// button beside a text button, because two adjacent nodes that do the same
/// thing is two things for a screen reader to read out and one of them is a
/// triangle.
fn summary_toggle(
    ui: &mut egui::Ui,
    expanded: bool,
    summary: &str,
    enabled: bool,
) -> egui::Response {
    use crate::widgets::icons::{Icon, MARK};

    let caret = if expanded {
        Icon::CaretUp
    } else {
        Icon::CaretDown
    };
    let gap = ui.spacing().icon_spacing;
    let padding = ui.spacing().button_padding;
    // The kebab that follows this on the header row, with the spacing before
    // it. Without this the truncated text took the whole row, the kebab was
    // placed one `item_spacing` past the card's edge, and egui widened the card
    // to include it — which widened the panel, which un-truncated the text a
    // little, which moved the kebab again: five frames of panel growth every
    // time a long summary appeared, and a `run()` in the headless harness that
    // never settled. The old fixed-width editors hid it by keeping the panel
    // too wide for a summary to ever truncate.
    let menu_room = crate::widgets::icons::button_size(ui).x + ui.spacing().item_spacing.x;

    let mut text = egui::RichText::new(summary);
    if !enabled {
        text = text.strikethrough();
    }
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (ui.available_width() - MARK - gap - 2.0 * padding.x - menu_room).max(0.0),
        egui::TextStyle::Button,
    );

    let size = egui::vec2(
        MARK + gap + galley.size().x + 2.0 * padding.x,
        galley.size().y.max(MARK) + 2.0 * padding.y,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        // Only the hovered and pressed states get a frame: at rest this is a
        // line of text with a caret, not a slab.
        if response.hovered() || expanded {
            ui.painter().rect(
                rect,
                visuals.corner_radius,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        let colour = visuals.fg_stroke.color;
        let inner = rect.shrink2(padding);
        caret.paint(
            ui.painter(),
            egui::Rect::from_min_size(
                egui::pos2(inner.left(), inner.center().y - MARK / 2.0),
                egui::Vec2::splat(MARK),
            ),
            colour,
        );
        ui.painter().galley(
            egui::pos2(
                inner.left() + MARK + gap,
                inner.center().y - galley.size().y / 2.0,
            ),
            galley,
            colour,
        );
    }

    // Inside the closure: it runs only when the accessibility tree is being
    // built, and a copy made outside it was paid on every frame.
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, expanded, summary.to_owned())
    });
    response
}

fn menu(ui: &mut egui::Ui, index: usize, last: bool, commands: &mut Vec<Command>) {
    let button = crate::widgets::icons::icon_button(
        ui,
        crate::widgets::icons::Icon::Kebab,
        "Reorder, duplicate or delete",
    );
    egui::Popup::menu(&button).show(|ui| {
        if ui
            .add_enabled(index > 0, egui::Button::new("Move up"))
            .clicked()
        {
            commands.push(Command::Move {
                from: index,
                to: index - 1,
            });
            ui.close();
        }
        if ui
            .add_enabled(!last, egui::Button::new("Move down"))
            .clicked()
        {
            commands.push(Command::Move {
                from: index,
                to: index + 1,
            });
            ui.close();
        }
        ui.separator();
        if ui.button("Duplicate").clicked() {
            commands.push(Command::Duplicate(index));
            ui.close();
        }
        if ui.button("Delete").clicked() {
            commands.push(Command::Delete(index));
            ui.close();
        }
    });
    button.on_hover_text("Reorder, duplicate or delete");
}

/// The Process Name / Process Extension switches (D19), plus the
/// per-card include filter — the two things a preset stores per operation.
fn scope_ui(ui: &mut egui::Ui, card: &mut Card) -> bool {
    let mut changed = false;

    // The header says what the pre-processor does, because Scope is collapsed
    // by default and a card that silently narrows what it sees is the thing
    // hardest to notice from the preview alone. `id_salt` is already set, so a
    // changing label does not change the widget id.
    let heading = match &card.preproc {
        Some(preproc) => format!(
            "Scope — pre-processor: {}",
            crate::widgets::preproc_editor::summary(preproc)
        ),
        None => "Scope".to_owned(),
    };
    egui::CollapsingHeader::new(heading)
        .id_salt("card_scope")
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Apply to").strong());
            ui.horizontal(|ui| {
                for (scope, label) in [
                    (Scope::Name, "Name"),
                    (Scope::Extension, "Extension"),
                    (Scope::Both, "Both"),
                ] {
                    changed |= ui.radio_value(&mut card.scope, scope, label).changed();
                }
            });
            ui.label(
                egui::RichText::new("The extension is everything after the last period.")
                    .weak()
                    .small(),
            );

            ui.add_space(6.0);
            changed |= filter_ui(ui, card);

            ui.add_space(6.0);
            changed |= preproc_ui(ui, card);
        });

    changed
}

/// D32: the source bar's filter is the default; a card may override it.
fn filter_ui(ui: &mut egui::Ui, card: &mut Card) -> bool {
    let mut changed = false;

    let mut own = card.filter.is_some();
    if ui
        .checkbox(&mut own, "Use a filter for this operation only")
        .on_hover_text(
            "Otherwise this operation uses the include filter in the source bar, \
             like every other card that has none of its own.",
        )
        .changed()
    {
        card.filter = own.then(FilterForm::default);
        changed = true;
    }

    match &mut card.filter {
        Some(filter) => changed |= filter.ui(ui),
        None => {
            ui.label(
                egui::RichText::new("Using the include filter from the source bar.")
                    .weak()
                    .small(),
            );
        }
    }

    changed
}

/// The pre-processor a card carries.
///
/// > *"The pre-processor filters out a section of the filename. Only this
/// > section is then processed by the actual rename function."*
///
/// The layout lives in `widgets::preproc_editor`, beside `widgets::filter_editor`
/// which owns the other control on this expander. Until M8 this drew a
/// read-only summary and said editing was not built — a card could only acquire
/// a pre-processor from a hand-written job file.
fn preproc_ui(ui: &mut egui::Ui, card: &mut Card) -> bool {
    crate::widgets::preproc_editor::ui(ui, &mut card.preproc)
}

/// The one-line reason a card cannot run, if it has one.
///
/// Drawn **every frame for every card**, collapsed or expanded, so it must stay
/// cheap and must never touch the disk.
///
/// Deliberately delegates to the engine rather than dry-running `apply` here.
/// The old version called `apply` with a synthetic entry and `total = 1`, which
/// the Filename Editor would read as "1 file is listed" — so a three-line
/// editor showed a permanent, wrong error on a collapsed card forever. Anything
/// that depends on the *listing* rather than on the card's own configuration
/// belongs in the plan, where the listing is.
fn trouble(card: &Card) -> Option<String> {
    card.op.problem()
}
