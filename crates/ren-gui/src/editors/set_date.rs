//! Set Date & Time.
//!
//! Laid out as the dialog has it: the Source dropdown, the date and
//! time boxes it feeds, the interval controls beside them, and the `Change:`
//! grid of six components and three targets.

use ren_core::datetime::{self, DateComponents, IntervalUnit};
use ren_core::ops::{DateSource, DateTargets, SetDate, WallClock};
use ren_platform::{Capability, Platform};

use crate::theme::width::{FIELD_DATE, FIELD_TOKEN};
use crate::widgets::form::{After, Form};

pub fn ui(ui: &mut egui::Ui, op: &mut SetDate, platform: &dyn Platform) -> bool {
    let mut changed = false;
    let error = ui.visuals().error_fg_color;

    Form::new("set_date").show(ui, |form| {
        // --- Source ---
        changed |= form
            .row("Source:", After::Nothing, |row| {
                let width = row.field_width();
                let mut changed = false;
                egui::ComboBox::from_id_salt("set_date_source")
                    .width(width)
                    .selected_text(op.source.label())
                    .show_ui(row.ui(), |ui| {
                        for candidate in DateSource::ALL {
                            changed |= ui
                                .selectable_value(&mut op.source, candidate, candidate.label())
                                .changed();
                        }
                    });
                changed
            })
            .inner;

        // --- The date and time boxes ---
        //
        // Text fields with a fixed ISO format rather than a calendar popup:
        // `egui_extras`' date picker is behind a feature that pulls in a second
        // date library beside chrono, and D30 already rejects locale-dependent
        // formats — the same preset has to mean the same thing on every machine.
        // The crate itself is no longer a dependency: this comment was the only
        // thing in the tree that mentioned it.
        //
        // Under the Source they feed rather than beside a label of their own,
        // which is what the form's unlabelled row is for.
        changed |= form
            .unlabelled(After::Nothing, |row| {
                let live = op.source.uses_wall_clock();
                row.ui()
                    .add_enabled_ui(live, |ui| {
                        let mut changed = false;
                        let mut date = format!(
                            "{:04}-{:02}-{:02}",
                            op.date.year, op.date.month, op.date.day
                        );
                        let mut time = format!(
                            "{:02}:{:02}:{:02}",
                            op.date.hour, op.date.minute, op.date.second
                        );
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut date)
                                    .desired_width(FIELD_DATE)
                                    .hint_text("yyyy-mm-dd")
                                    .id_salt("set_date_date"),
                            )
                            .changed()
                            && let Some(parsed) = parse_date(&date)
                        {
                            op.date.year = parsed.0;
                            op.date.month = parsed.1;
                            op.date.day = parsed.2;
                            changed = true;
                        }
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut time)
                                    .desired_width(FIELD_TOKEN)
                                    .hint_text("hh:mm:ss")
                                    .id_salt("set_date_time"),
                            )
                            .changed()
                            && let Some(parsed) = parse_time(&time)
                        {
                            op.date.hour = parsed.0;
                            op.date.minute = parsed.1;
                            op.date.second = parsed.2;
                            changed = true;
                        }
                        if ui
                            .button("Now")
                            .on_hover_text("Fill in this moment")
                            .clicked()
                        {
                            op.date = WallClock::from_naive(chrono::Local::now().naive_local());
                            changed = true;
                        }
                        changed
                    })
                    .inner
            })
            .inner;
        if op.date.to_naive().is_none() {
            form.note(egui::RichText::new("That is not a real date.").color(error));
        } else if !op.source.uses_wall_clock() {
            form.note("These boxes are only used by 'Enter new date (set below)'.");
        }

        // --- The interval, dead unless the source is one ---
        changed |= form
            .unlabelled(After::Nothing, |row| {
                row.ui()
                    .add_enabled_ui(op.source.uses_interval(), |ui| {
                        let mut changed = crate::widgets::number::add(
                            ui,
                            egui::DragValue::new(&mut op.interval).range(0..=9_999),
                        )
                        .changed();
                        egui::ComboBox::from_id_salt("set_date_unit")
                            .width(110.0)
                            .selected_text(op.unit.label())
                            .show_ui(ui, |ui| {
                                for candidate in IntervalUnit::ALL {
                                    changed |= ui
                                        .selectable_value(
                                            &mut op.unit,
                                            candidate,
                                            candidate.label(),
                                        )
                                        .changed();
                                }
                            });
                        changed
                    })
                    .inner
            })
            .inner;
    });

    ui.add_space(6.0);
    ui.label(egui::RichText::new("Change:").strong());

    // --- The Change grid: six components, then the three targets ---
    //
    // The dialog reads *down* each column, not across: Year/Month/Day, then
    // Hour/Min./Sec., then the three stamps.
    let mut change = op.change;
    let mut targets = op.targets;
    egui::Grid::new("set_date_change")
        .num_columns(3)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            let row =
                |ui: &mut egui::Ui,
                 (a_label, a): (&str, &mut bool),
                 (b_label, b): (&str, &mut bool),
                 (t_label, t, capability): (&str, &mut bool, Capability)| {
                    let mut touched = ui.checkbox(a, a_label).changed();
                    touched |= ui.checkbox(b, b_label).changed();
                    // D40 extended to dates: a Windows preset stays legible here,
                    // and the planner blocks the run with the reason rather than
                    // letting it fail on the first file.
                    touched |= ui
                        .add_enabled_ui(platform.supports(capability), |ui| {
                            ui.checkbox(t, t_label).changed()
                        })
                        .inner;
                    ui.end_row();
                    touched
                };

            changed |= row(
                ui,
                (DateComponents::LABELS[0], &mut change.year),
                (DateComponents::LABELS[3], &mut change.hour),
                (
                    DateTargets::LABELS[0],
                    &mut targets.created,
                    Capability::CreatedTime,
                ),
            );
            changed |= row(
                ui,
                (DateComponents::LABELS[1], &mut change.month),
                (DateComponents::LABELS[4], &mut change.minute),
                (
                    DateTargets::LABELS[1],
                    &mut targets.accessed,
                    Capability::AccessedTime,
                ),
            );
            changed |= row(
                ui,
                (DateComponents::LABELS[2], &mut change.day),
                (DateComponents::LABELS[5], &mut change.second),
                (
                    DateTargets::LABELS[2],
                    &mut targets.modified,
                    Capability::ModifiedTime,
                ),
            );
        });
    op.change = change;
    op.targets = targets;

    ui.add_space(4.0);
    if !platform.supports(Capability::CreatedTime) {
        ui.label(
            egui::RichText::new(format!(
                "The created date cannot be written on {} — it is shown so a preset built \
                 on Windows still reads here, and the run is blocked with that reason \
                 rather than failing halfway.",
                platform.name()
            ))
            .weak()
            .small(),
        );
    }
    if op.targets.none() {
        ui.label(
            egui::RichText::new("No date is selected, so this operation does nothing.")
                .weak()
                .small(),
        );
    } else if op.change.none() {
        ui.label(
            egui::RichText::new("No part of the date is selected, so this operation does nothing.")
                .weak()
                .small(),
        );
    }
    ui.label(
        egui::RichText::new(format!(
            "Dates must be between {} and {}. The limit applies to the result, not the source.",
            datetime::YEAR_MIN,
            datetime::YEAR_MAX
        ))
        .weak()
        .small(),
    );

    changed
}

fn parse_date(text: &str) -> Option<(i32, u32, u32)> {
    let mut parts = text.trim().split('-');
    let year = parts.next()?.trim().parse().ok()?;
    let month = parts.next()?.trim().parse().ok()?;
    let day = parts.next()?.trim().parse().ok()?;
    parts.next().is_none().then_some((year, month, day))
}

fn parse_time(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split(':');
    let hour = parts.next()?.trim().parse().ok()?;
    let minute = parts.next()?.trim().parse().ok()?;
    // Seconds are optional, so "09:30" is a perfectly good thing to type.
    let second = match parts.next() {
        Some(text) => text.trim().parse().ok()?,
        None => 0,
    };
    parts.next().is_none().then_some((hour, minute, second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boxes_read_the_iso_format_they_ask_for() {
        assert_eq!(parse_date("2008-02-17"), Some((2008, 2, 17)));
        assert_eq!(parse_date(" 2008-2-7 "), Some((2008, 2, 7)));
        assert_eq!(parse_date("17/02/2008"), None, "one format, deliberately");
        assert_eq!(parse_date("2008-02"), None);
        assert_eq!(parse_date("2008-02-17-1"), None);

        assert_eq!(parse_time("11:23:50"), Some((11, 23, 50)));
        assert_eq!(
            parse_time("09:30"),
            Some((9, 30, 0)),
            "seconds are optional"
        );
        assert_eq!(parse_time("nope"), None);
    }
}
