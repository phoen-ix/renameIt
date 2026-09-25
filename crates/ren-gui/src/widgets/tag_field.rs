//! A text field that accepts `<tags>`, with the tag menu beside it.
//!
//! Every tag-bearing field carries a tag-picker button that opens a menu of
//! all tags. Since D29 makes an unknown tag a compile error, the field also
//! has somewhere to say so — the
//! message appears under the box as you type, rather than after the rename.

use ren_core::template::{TagError, TextTemplate};

/// One entry in the tag menu.
struct Entry {
    tag: &'static str,
    what: &'static str,
}

/// The menu, grouped by subject. Every entry is a tag the engine renders —
/// `the_tag_menu_offers_only_tags_that_compile` holds that — and the hover
/// text says what it gives, in the words `docs/tags.md` uses.
const GROUPS: [(&str, &[Entry]); 8] = [
    (
        "Name",
        &[
            Entry {
                tag: "<Name>",
                what: "Current filename",
            },
            Entry {
                tag: "<Ext>",
                what: "Current extension",
            },
            Entry {
                tag: "<FullName>",
                what: "Current full filename",
            },
            Entry {
                tag: "<Parent>",
                what: "Parent folder name",
            },
            Entry {
                tag: "<Parent-2>",
                what: "N:th parent folder name",
            },
            Entry {
                tag: "<Left-3>",
                what: "Left # characters",
            },
            Entry {
                tag: "<Right-3>",
                what: "Right # characters",
            },
            Entry {
                tag: "<Mid-2>",
                what: "Chars from pos # to end",
            },
            Entry {
                tag: "<Mid-2-5>",
                what: "Chars from pos #1 to #2",
            },
            Entry {
                tag: "<MidRev-3>",
                what: "Chars from pos # counting backwards",
            },
            Entry {
                tag: "<FLetter>",
                what: "First letter in name",
            },
            Entry {
                tag: "<FLetterN1>",
                what: "First letter incl. numbers",
            },
            Entry {
                tag: "<FLetterN2>",
                what: "First letter incl. numbers subst.",
            },
        ],
    ),
    (
        "Size",
        &[
            Entry {
                tag: "<Size>",
                what: "File size (auto)",
            },
            Entry {
                tag: "<SizeB>",
                what: "File size (bytes)",
            },
            Entry {
                tag: "<SizeB-8>",
                what: "File size padded with # zeros",
            },
        ],
    ),
    // The folder a row is in — or, for a folder row, the folder itself. `S`
    // means "and subfolders".
    (
        "Folder",
        &[
            Entry {
                tag: "<DirFiles>",
                what: "Number of files in the folder",
            },
            Entry {
                tag: "<DirDirs>",
                what: "Number of subfolders in the folder",
            },
            Entry {
                tag: "<SDirFiles>",
                what: "Number of files, through subfolders",
            },
            Entry {
                tag: "<SDirDirs>",
                what: "Number of folders, through subfolders",
            },
            Entry {
                tag: "<DirSize>",
                what: "Size of the files in the folder (auto)",
            },
            Entry {
                tag: "<DirSizeB>",
                what: "Size of the files in the folder (bytes)",
            },
            Entry {
                tag: "<SDirSize>",
                what: "Size through subfolders (auto)",
            },
            Entry {
                tag: "<FirstFileInFolder>",
                what: "Name of the first file in the folder",
            },
            Entry {
                tag: "<FirstFileInFolder-Ext>",
                what: "The same, with its extension",
            },
            Entry {
                tag: "<HtmlTitle>",
                what: "The <title> of an HTML page",
            },
        ],
    ),
    (
        "Date",
        &[
            Entry {
                tag: "<Date>",
                what: "Modified date",
            },
            Entry {
                tag: "<Time>",
                what: "Modified time",
            },
            Entry {
                tag: "<CDate>",
                what: "Created date",
            },
            Entry {
                tag: "<CTime>",
                what: "Created time",
            },
            Entry {
                tag: "<ADate>",
                what: "Accessed date",
            },
            Entry {
                tag: "<ATime>",
                what: "Accessed time",
            },
            Entry {
                tag: "<Date-yyyy-mm-dd>",
                what: "Custom formatted date",
            },
            Entry {
                tag: "<NowDate>",
                what: "Date at start of rename",
            },
            Entry {
                tag: "<NowTime>",
                what: "Time at start of rename",
            },
        ],
    ),
    (
        "Numbers",
        &[
            Entry {
                tag: "<Counter>",
                what: "Counter",
            },
            Entry {
                tag: "<NumFiles>",
                what: "Number of items to be renamed",
            },
            Entry {
                tag: "<Rnd>",
                what: "Random character (A-Z)",
            },
            Entry {
                tag: "<Rnd3>",
                what: "Random number, # digits",
            },
            Entry {
                tag: "<Rnd3-1-100>",
                what: "Random number (custom range)",
            },
            Entry {
                tag: "<RndZ3-1-100>",
                what: "Random number, zero padded",
            },
        ],
    ),
    (
        "Misc",
        &[
            Entry {
                tag: "<%1>",
                what: "Part 1 of the filename",
            },
            Entry {
                tag: "<%2>",
                what: "Part 2 of the filename",
            },
            Entry {
                tag: "<Ask>",
                what: "Ask for user input",
            },
            Entry {
                tag: "<Ask-1>",
                what: "Additional user input",
            },
            Entry {
                tag: "<Clipboard>",
                what: "Clipboard contents",
            },
            Entry {
                tag: "<Crc32>",
                what: "File CRC32 checksum",
            },
            Entry {
                tag: "<DetectedExt>",
                what: "Detected filetype extension",
            },
            Entry {
                tag: "<FileMax-64>",
                what: "Cut the text this field produces to # characters",
            },
            Entry {
                tag: "<PathMax-260>",
                what: "Cut this field's text so the folder, a separator and the text fit in # characters",
            },
            Entry {
                tag: "<\\>",
                what: "Move to subfolder",
            },
        ],
    ),
    // The music family. Case-insensitive like every other tag, but the menu is
    // still the reliable way to get the spelling — `<TrackN>` and `<FreqS>` are
    // not names anyone guesses.
    (
        "Music",
        &[
            Entry {
                tag: "<Artist>",
                what: "Artist",
            },
            Entry {
                tag: "<Title>",
                what: "Title",
            },
            Entry {
                tag: "<Album>",
                what: "Album",
            },
            Entry {
                tag: "<Year>",
                what: "Year",
            },
            Entry {
                tag: "<Genre>",
                what: "Genre",
            },
            Entry {
                tag: "<Comment>",
                what: "Comment",
            },
            Entry {
                tag: "<Track>",
                what: "Track Nr., zero-padded",
            },
            Entry {
                tag: "<TrackN>",
                what: "Track Nr., no zero padding",
            },
            Entry {
                tag: "<Length>",
                what: "Length (long)",
            },
            Entry {
                tag: "<LengthS>",
                what: "Length (short)",
            },
            Entry {
                tag: "<Bitrate>",
                what: "Bitrate (kbps)",
            },
            Entry {
                tag: "<Stereo>",
                what: "Stereo or Mono",
            },
            Entry {
                tag: "<Freq>",
                what: "Frequency (Hz)",
            },
            Entry {
                tag: "<FreqS>",
                what: "Frequency (kHz)",
            },
            Entry {
                tag: "<ID3-AlbumArtist>",
                what: "Album artist (ID3v2)",
            },
            Entry {
                tag: "<ID3-Composer>",
                what: "Composer (ID3v2)",
            },
            Entry {
                tag: "<ID3-DiscNumber>",
                what: "Disc number (ID3v2)",
            },
            Entry {
                tag: "<ID3-Publisher>",
                what: "Publisher (ID3v2)",
            },
            Entry {
                tag: "<ID3-Lyrics>",
                what: "Lyrics (ID3v2)",
            },
        ],
    ),
    // Exif is a passthrough over whatever the image carries, so the menu lists
    // the handful people actually reach for rather than all 161.
    (
        "Image",
        &[
            Entry {
                tag: "<ExifDate>",
                what: "Date the photograph was taken",
            },
            Entry {
                tag: "<ExifTime>",
                what: "Time the photograph was taken",
            },
            Entry {
                tag: "<ExifDate-yyyy-mm-dd>",
                what: "Exif date, custom format",
            },
            Entry {
                tag: "<Exif-Make>",
                what: "Camera manufacturer",
            },
            Entry {
                tag: "<Exif-Model>",
                what: "Camera model",
            },
            Entry {
                tag: "<Exif-PixelXDimension>",
                what: "Image width",
            },
            Entry {
                tag: "<Exif-PixelYDimension>",
                what: "Image height",
            },
            Entry {
                tag: "<Exif-FNumber>",
                what: "Aperture",
            },
            Entry {
                tag: "<Exif-ExposureTime>",
                what: "Shutter speed",
            },
            Entry {
                tag: "<Exif-PhotographicSensitivity>",
                what: "ISO speed",
            },
            Entry {
                tag: "<Exif-FocalLength>",
                what: "Focal length",
            },
            Entry {
                tag: "<Exif-Orientation>",
                what: "Orientation",
            },
            // The file's own header rather than the camera's Exif block, so
            // these answer for a PNG or a BMP where every `<Exif-…>` above
            // returns nothing.
            Entry {
                tag: "<Width>",
                what: "Width in pixels, from the image header",
            },
            Entry {
                tag: "<Height>",
                what: "Height in pixels, from the image header",
            },
            Entry {
                tag: "<Depth>",
                what: "Colour depth, in colours",
            },
            Entry {
                tag: "<Depthb>",
                what: "Colour depth, in bits",
            },
            Entry {
                tag: "<JpgComment>",
                what: "The comment stored inside a JPEG",
            },
        ],
    ),
];

/// How many strings a field with a drop-down history remembers.
///
/// A constant, not a setting (P71). How many past patterns a box offers
/// changes nothing about any rename, so it has no page to live on — the same
/// reasoning that made Visual Assist's picker length a constant
/// (`visual_assist::MAX_ITEMS`).
pub const HISTORY_ITEMS: usize = 12;

/// Draws a tag-accepting field. Returns true if the text changed.
pub fn tag_field(ui: &mut egui::Ui, id: &str, field: &mut TextTemplate, width: f32) -> bool {
    edit(ui, id, field, width, "", true, &[])
}

/// The same, with the strings this field has been **run** with, offered from
/// a chevron beside its tag button — Free Format's pattern, *Replace with*
/// and Insert. (Find is [`text_with_history`]: it takes no tags.)
pub fn tag_field_with_history(
    ui: &mut egui::Ui,
    id: &str,
    field: &mut TextTemplate,
    width: f32,
    history: &[String],
) -> bool {
    edit(ui, id, field, width, "", true, history)
}

/// The same again, with a placeholder shown while the box is empty.
pub fn tag_field_hinted_with_history(
    ui: &mut egui::Ui,
    id: &str,
    field: &mut TextTemplate,
    width: f32,
    hint: &str,
    history: &[String],
) -> bool {
    edit(ui, id, field, width, hint, true, history)
}

/// A plain text box with the same history menu — for **Find**, which is drawn
/// as a combo and which takes no tags.
pub fn text_with_history(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    width: f32,
    hint: &str,
    history: &[String],
) -> bool {
    let mut changed = ui
        .add(
            egui::TextEdit::singleline(text)
                .desired_width(width)
                .id_salt(id)
                .hint_text(hint),
        )
        .changed();
    if !history.is_empty() {
        let mut picked = None;
        let history_button = crate::widgets::icons::icon_button(
            ui,
            crate::widgets::icons::Icon::Chevron,
            "Recent values",
        );
        egui::Popup::menu(&history_button).show(|ui| {
            for previous in history {
                if ui.button(previous).clicked() {
                    picked = Some(previous.clone());
                    ui.close();
                }
            }
        });
        history_button.on_hover_text("Text you have searched for before");
        if let Some(previous) = picked {
            *text = previous;
            changed = true;
        }
    }
    changed
}

/// The same again, without the `<tags>` button — for a field that sits in a
/// row of a table, where one menu button per row would not fit. The field still
/// takes tags and still names a bad one; only the picker is gone.
pub fn tag_text_edit(
    ui: &mut egui::Ui,
    id: &str,
    field: &mut TextTemplate,
    width: f32,
    hint: &str,
) -> bool {
    edit(ui, id, field, width, hint, false, &[])
}

fn edit(
    ui: &mut egui::Ui,
    id: &str,
    field: &mut TextTemplate,
    width: f32,
    hint: &str,
    with_menu: bool,
    history: &[String],
) -> bool {
    let mut changed = false;
    let mut text = field.as_str().to_owned();

    ui.horizontal(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width)
                    .hint_text(hint)
                    .id_salt(id),
            )
            .changed()
        {
            changed = true;
        }
        if with_menu && let Some(tag) = menu(ui, id) {
            text.push_str(tag);
            changed = true;
        }
        // Hidden while there is nothing in it, so a fresh install grows no dead
        // control — and it **replaces** the box rather than appending to it,
        // because that is what a combo box does and appending is what the
        // `<tags>` menu beside it is for.
        if !history.is_empty() {
            let mut picked = None;
            let history_button = crate::widgets::icons::icon_button(
                ui,
                crate::widgets::icons::Icon::Chevron,
                "Recent values",
            );
            egui::Popup::menu(&history_button).show(|ui| {
                for previous in history {
                    if ui.button(previous).clicked() {
                        picked = Some(previous.clone());
                        ui.close();
                    }
                }
            });
            history_button.on_hover_text("Patterns you have run before");
            if let Some(previous) = picked {
                text = previous;
                changed = true;
            }
        }
    });

    if changed {
        *field = TextTemplate::new(text);
    }
    problem(ui, field);
    changed
}

/// The tag button and its menu. Returns the tag the user picked.
pub(crate) fn menu(ui: &mut egui::Ui, id: &str) -> Option<&'static str> {
    let mut picked = None;
    ui.menu_button("<tags>", |ui| {
        ui.set_max_height(360.0);
        egui::ScrollArea::vertical()
            .id_salt(format!("{id}_tags"))
            .show(ui, |ui| {
                for (group, entries) in GROUPS {
                    ui.label(egui::RichText::new(group).strong().small());
                    for entry in entries {
                        if ui.button(entry.tag).on_hover_text(entry.what).clicked() {
                            picked = Some(entry.tag);
                            ui.close();
                        }
                    }
                    ui.separator();
                }
            });
    })
    .response
    .on_hover_text("Insert a tag");
    picked
}

/// D29's payoff: the bad tag is named while it is being typed.
fn problem(ui: &mut egui::Ui, field: &TextTemplate) {
    let Err(error) = field.compiled() else {
        return;
    };
    let colour = match error.error {
        TagError::Deferred { .. } => ui.visuals().warn_fg_color,
        _ => ui.visuals().error_fg_color,
    };
    ui.label(egui::RichText::new(error.to_string()).color(colour).small());
}

/// Every tag the menu offers, for the test that they all still compile.
pub fn menu_tags() -> Vec<&'static str> {
    GROUPS
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|e| e.tag))
        .collect()
}
