//! CSV List Rename — new names from a list: one column holds current names,
//! another the new ones.
//!
//! The operation is thin on purpose: [`crate::csv_table`] does the reading, and
//! the *scoping* is done by the engine rather than here. Under the default
//! [`Scope::Name`](crate::Scope) the subject handed to `apply` is already the
//! stem, so `Lorem` in the list renames `lorem.txt` on disk to `Some.txt`
//! without this file mentioning extensions once. A list whose names carry
//! their extensions needs the card scoped to the whole name (Process
//! Extension on), and that too falls out of the scoping with no code here.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{EvalCx, NameTransform, OpError};
use crate::cache::Cached;
use crate::csv_table::{self, CsvError, CsvOptions, CsvTable};
use crate::run::AskSpec;
use crate::template::TagNeeds;

/// The `Separator:` combo.
///
/// Five separators. See D51 for how the list settled. The only choice worth
/// noting is ordering: this puts Tab
/// fourth. The `{TAB}` token still works in the Other box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvSeparator {
    #[default]
    Comma,
    Colon,
    Semicolon,
    Tab,
    Other,
}

impl CsvSeparator {
    pub const ALL: [Self; 5] = [
        Self::Comma,
        Self::Colon,
        Self::Semicolon,
        Self::Tab,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Comma => "Comma",
            Self::Colon => "Colon",
            Self::Semicolon => "Semicolon",
            Self::Tab => "Tab",
            Self::Other => "Other:",
        }
    }

    /// `None` for `Other`, whose character comes from the text box beside it.
    pub fn byte(self) -> Option<u8> {
        match self {
            Self::Comma => Some(b','),
            Self::Colon => Some(b':'),
            Self::Semicolon => Some(b';'),
            Self::Tab => Some(b'\t'),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CsvList {
    /// The list. Stored in presets exactly as written (D39) — the card says
    /// plainly that an absolute path will not resolve elsewhere.
    pub file: PathBuf,
    pub separator: CsvSeparator,
    /// The box beside the combo, live only when `separator` is `Other`.
    /// Accepts the `{TAB}` and `{ENTER}` tokens.
    pub separator_char: String,
    /// The column holding current names — 1-based, as the card numbers them.
    pub old_column: usize,
    /// The column holding new names, 1-based.
    pub new_column: usize,
    /// Off by default, so `Lorem` in a list matches `lorem.txt` on disk: a
    /// list typed by hand rarely matches the case of every file.
    pub case_sensitive: bool,
    /// D21: a clone starts empty, which is what makes editing the path re-read.
    /// The *parse* is shared process-wide, so the clone costs a `stat` rather
    /// than a re-read — see [`crate::csv_table::load`].
    #[serde(skip)]
    table: Cached<Arc<Result<CsvTable, CsvError>>>,
}

impl Default for CsvList {
    fn default() -> Self {
        Self {
            file: PathBuf::new(),
            separator: CsvSeparator::Comma,
            separator_char: String::new(),
            old_column: 1,
            new_column: 2,
            case_sensitive: false,
            table: Cached::new(),
        }
    }
}

impl CsvList {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            ..Self::default()
        }
    }

    /// The separator byte, resolving `Other` through the text box.
    pub fn separator_byte(&self) -> Result<u8, CsvError> {
        if let Some(byte) = self.separator.byte() {
            return Ok(byte);
        }
        let text = match self.separator_char.as_str() {
            "{TAB}" => "\t",
            "{ENTER}" => "\n",
            other => other,
        };
        let mut bytes = text.bytes();
        match (bytes.next(), bytes.next()) {
            (Some(byte), None) => Ok(byte),
            _ => Err(CsvError::BadSeparator),
        }
    }

    fn options(&self) -> Result<CsvOptions, CsvError> {
        Ok(CsvOptions {
            separator: self.separator_byte()?,
            old_column: self.old_column,
            new_column: self.new_column,
            case_sensitive: self.case_sensitive,
        })
    }

    /// The parsed list, read at most once per operation instance.
    pub fn table(&self) -> &Result<CsvTable, CsvError> {
        self.table.get_or_init(|| match self.options() {
            Ok(options) => csv_table::load(&self.file, options),
            Err(e) => Arc::new(Err(e)),
        })
    }

    /// How many rows the list has, or why it could not be read — for the card.
    pub fn status(&self) -> Result<usize, CsvError> {
        match self.table() {
            Ok(table) => Ok(table.len()),
            Err(e) => Err(e.clone()),
        }
    }

    /// A card with no file chosen yet does nothing rather than failing.
    ///
    /// P34's reading: a freshly added card must not shout before the user has
    /// touched it. A file that *was* named and cannot be read is a different
    /// thing entirely, and does block the run.
    fn dormant(&self) -> bool {
        self.file.as_os_str().is_empty()
    }
}

impl NameTransform for CsvList {
    fn id(&self) -> &'static str {
        "csv_list"
    }

    fn summary(&self) -> String {
        if self.dormant() {
            return "CSV List Rename (no file yet)".to_owned();
        }
        let name = self
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file.display().to_string());
        match self.status() {
            Ok(1) => format!("Rename from {name} (1 row)"),
            Ok(rows) => format!("Rename from {name} ({rows} rows)"),
            Err(e) => format!("Rename from {name} — {e}"),
        }
    }

    fn apply<'a>(&self, subject: &'a str, cx: &EvalCx<'_>) -> Result<Cow<'a, str>, OpError> {
        if self.dormant() {
            return Ok(Cow::Borrowed(subject));
        }
        let table = match self.table() {
            Ok(table) => table,
            Err(e) => return Err(OpError::new("CSV List Rename", e.to_string())),
        };
        // A file the list does not name is left alone — as is a row that names
        // no file. Both silent: a list is usually a superset or a subset of the
        // folder, so `not in list.txt` survives untouched and a row naming no
        // listed file does nothing.
        let Some(value) = table.get(subject) else {
            return Ok(Cow::Borrowed(subject));
        };
        match cx.render(value)? {
            None => Ok(Cow::Borrowed(subject)),
            Some(text) if text == subject => Ok(Cow::Borrowed(subject)),
            Some(text) => Ok(Cow::Owned(text.into_owned())),
        }
    }

    fn needs(&self) -> TagNeeds {
        match self.table() {
            Ok(table) => table.needs(),
            Err(_) => TagNeeds::NONE,
        }
    }

    fn asks(&self) -> Vec<AskSpec> {
        match self.table() {
            Ok(table) => table.asks(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Scope};
    use crate::pipeline::{Pipeline, Step, StepConfig};
    use crate::{OpKind, evaluate_all};
    use tempfile::TempDir;

    /// A list with a header row, one row for a file that is not listed
    /// (`amet`), and names that match only case-insensitively (`Lorem`).
    const EXAMPLE: &str =
        "Old,New\nLorem,Some\nipsum,example\ndolor,text\nsit,I just made\namet,up\n";

    fn write(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("list.csv");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Runs the op as a pipeline step, so the engine's scoping is part of the
    /// test rather than bypassed by it — which is the whole point here.
    fn rename_all(op: CsvList, scope: Scope, names: &[&str]) -> Vec<String> {
        let entries: Vec<FileEntry> = names
            .iter()
            .map(|n| FileEntry::synthetic(format!("/files/{n}")))
            .collect();
        let pipeline = Pipeline::new().with(Step::Name(Box::new(op)), StepConfig::scoped(scope));
        evaluate_all(&entries, &pipeline)
            .into_iter()
            .map(|r| r.unwrap().name)
            .collect()
    }

    /// Three things at once: matching is case-insensitive by default, the
    /// match is against the stem so the extension survives, and an unmatched
    /// row and an unmatched file are both silent.
    #[test]
    fn the_example_list_renames_by_stem_and_leaves_the_rest_alone() {
        let dir = TempDir::new().unwrap();
        let op = CsvList::new(write(&dir, EXAMPLE));
        assert_eq!(
            rename_all(
                op,
                Scope::Name,
                &[
                    "dolor.txt",
                    "ipsum.txt",
                    "lorem.txt",
                    "not in list.txt",
                    "sit.txt",
                ]
            ),
            [
                "text.txt",
                "example.txt",
                "Some.txt",
                "not in list.txt",
                "I just made.txt",
            ]
        );
    }

    /// A list whose names carry extensions matches only when the extension is
    /// in scope.
    #[test]
    fn a_list_whose_names_carry_extensions_needs_the_extension_in_scope() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "lorem.txt,Some.md\n");

        assert_eq!(
            rename_all(CsvList::new(&path), Scope::Name, &["lorem.txt"]),
            ["lorem.txt"],
            "scoped to the stem, `lorem.txt` in the list matches nothing"
        );
        assert_eq!(
            rename_all(CsvList::new(&path), Scope::Both, &["lorem.txt"]),
            ["Some.md"]
        );
    }

    /// The default is the one a hand-typed list of stems needs.
    #[test]
    fn a_fresh_card_is_scoped_to_the_name_with_two_columns_and_no_case() {
        let op = CsvList::default();
        assert_eq!(op.old_column, 1);
        assert_eq!(op.new_column, 2);
        assert!(!op.case_sensitive);
        assert_eq!(op.separator, CsvSeparator::Comma);
        assert_eq!(
            OpKind::CsvList(CsvList::default()).default_scope(),
            Scope::Name
        );
    }

    /// A card with no file chosen must not report anything (P34) — but a file
    /// that was named and cannot be read must block the run (P4).
    #[test]
    fn no_file_is_dormant_but_a_missing_file_is_an_error() {
        assert_eq!(
            rename_all(CsvList::default(), Scope::Name, &["a.txt"]),
            ["a.txt"]
        );
        assert_eq!(
            CsvList::default().summary(),
            "CSV List Rename (no file yet)"
        );

        let op = CsvList::new("/nowhere/at/all.csv");
        let entry = FileEntry::synthetic("/files/a.txt");
        let cx = EvalCx::simple(&entry, 0, 1);
        let err = op
            .apply("a", &cx)
            .expect_err("a named file that is missing");
        assert!(err.to_string().contains("all.csv"), "{err}");
    }

    #[test]
    fn tags_in_the_new_column_are_rendered_per_file() {
        let dir = TempDir::new().unwrap();
        let op = CsvList::new(write(&dir, "a,<Counter> - a\nb,<Counter> - b\n"));
        assert!(op.needs().contains(TagNeeds::COUNTER));
        assert_eq!(
            rename_all(op, Scope::Name, &["a.txt", "b.txt"]),
            ["1 - a.txt", "2 - b.txt"]
        );
    }

    #[test]
    fn the_separator_resolves_through_the_combo_or_the_box() {
        assert_eq!(CsvList::default().separator_byte(), Ok(b','));
        for (separator, byte) in [
            (CsvSeparator::Colon, b':'),
            (CsvSeparator::Semicolon, b';'),
            (CsvSeparator::Tab, b'\t'),
        ] {
            let op = CsvList {
                separator,
                ..Default::default()
            };
            assert_eq!(op.separator_byte(), Ok(byte));
        }

        let custom = |text: &str| {
            CsvList {
                separator: CsvSeparator::Other,
                separator_char: text.to_owned(),
                ..Default::default()
            }
            .separator_byte()
        };
        assert_eq!(custom("|"), Ok(b'|'));
        assert_eq!(custom("{TAB}"), Ok(b'\t'), "the token for a tab");
        assert_eq!(custom(""), Err(CsvError::BadSeparator));
        assert_eq!(custom("ab"), Err(CsvError::BadSeparator));
    }

    #[test]
    fn it_round_trips_through_a_job_file() {
        let op = CsvList {
            file: "/lists/names.csv".into(),
            separator: CsvSeparator::Semicolon,
            old_column: 3,
            new_column: 4,
            case_sensitive: true,
            ..Default::default()
        };
        let back: CsvList = toml::from_str(&toml::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op);
    }
}
