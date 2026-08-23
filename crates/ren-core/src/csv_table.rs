//! Reading a CSV into an old-name → new-name lookup.
//!
//! Standard CSV formatting rules apply, except for newlines within data. The
//! order of the names does not matter; where the old-name column repeats, the
//! first row found wins; and every line is processed, including the first.
//!
//! Split out from the operation for one reason: `CsvList::apply` runs once per
//! file under rayon and must stay pure, but the data lives on disk. The parse
//! happens here, once, behind a process-wide cache (P40).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::run::AskSpec;
use crate::template::{TagNeeds, TextTemplate};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvError {
    #[error("no CSV file chosen")]
    NoFile,
    #[error("{path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("the separator must be a single character")]
    BadSeparator,
    #[error("the two columns must be different")]
    SameColumn,
    /// The one documented exclusion.
    ///
    /// The whole file is rejected rather than the row, because the usual cause
    /// is a single unbalanced quote — which swallows every remaining row into
    /// one field. Skipping quietly would leave the user staring at "nothing was
    /// renamed" with no way to find out why.
    #[error("line {line}: a value contains a line break, which CSV List Rename does not support")]
    NewlineInData { line: u64 },
    #[error("line {line}: {message}")]
    Malformed { line: u64, message: String },
}

/// How to read one CSV. Part of the cache key, so changing any of it re-reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CsvOptions {
    pub separator: u8,
    /// 1-based, as the dialog numbers them.
    pub old_column: usize,
    pub new_column: usize,
    pub case_sensitive: bool,
}

/// One CSV, parsed and ready to answer lookups.
#[derive(Debug)]
pub struct CsvTable {
    /// Keys are folded to lower case when the lookup is case-insensitive, so
    /// the fold happens once at load rather than once per file.
    rows: HashMap<String, TextTemplate>,
    case_sensitive: bool,
}

impl CsvTable {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The replacement for `subject`, if the list names it.
    pub fn get(&self, subject: &str) -> Option<&TextTemplate> {
        if self.case_sensitive {
            self.rows.get(subject)
        } else {
            self.rows.get(&fold(subject))
        }
    }

    /// What every new-name value between them costs.
    pub fn needs(&self) -> TagNeeds {
        self.rows
            .values()
            .fold(TagNeeds::NONE, |acc, t| acc.union(t.needs()))
    }

    /// Every `<Ask>` slot in the file, so the run can collect them before
    /// evaluating anything (D28).
    pub fn asks(&self) -> Vec<AskSpec> {
        let mut out: Vec<AskSpec> = self.rows.values().flat_map(TextTemplate::asks).collect();
        out.sort_by_key(|a| a.slot);
        out.dedup_by_key(|a| a.slot);
        out
    }
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Everything up to and including the last separator, gone.
///
/// The current-name column may contain paths, full or relative — but only
/// files in the listing are processed.
///
/// The path is tolerated, not resolved: the listing decides what is processed,
/// so `C:\photos\lorem`, `photos/lorem` and `lorem` all key identically. Both
/// separators on both platforms — a CSV written on Windows is an ordinary thing
/// to open on Linux.
fn file_part(value: &str) -> &str {
    match value.rfind(['/', '\\']) {
        Some(at) => &value[at + 1..],
        None => value,
    }
}

/// The same strip for the *new* column, blind to separators inside a `<tag>`.
///
/// > *"Paths the the column with new names are ignored."*
///
/// Not a convenience: D31 makes `/` in a produced name a subfolder move, so
/// without this a spreadsheet value of `2024/Some` silently relocates the file.
///
/// The tag-awareness is what keeps that from over-reaching. `<\>` is the tag
/// that *means* a separator, and it contains a literal backslash — so a plain
/// `rfind` would cut `2024<\>Some` down to `>Some` and quietly destroy the one
/// way a user has of asking for a move. Depth tracking draws the line where it
/// belongs: between what the data said and what the user wrote.
///
/// An unclosed `<` swallows the rest, so nothing is stripped. That is the safe
/// direction, and the template will refuse to compile anyway (D29).
fn new_file_part(value: &str) -> &str {
    let mut depth = 0usize;
    let mut cut = None;
    for (at, ch) in value.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            '/' | '\\' if depth == 0 => cut = Some(at + ch.len_utf8()),
            _ => {}
        }
    }
    match cut {
        Some(at) => &value[at..],
        None => value,
    }
}

/// CP1252 for the 0x80–0x9F range; everything else is Latin-1, i.e. identity.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

/// UTF-8 if it is valid, CP1252 if it is not.
///
/// Excel writes CP1252, and P10 already records that a CSV in the wild is as
/// likely to be ISO-8859-1/CP1252 as UTF-8. `String::from_utf8_lossy` is the
/// wrong fallback here:
/// it would turn `Björk` into `Bj<?>rk` and rename a file to it.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => CP1252_HIGH[(b - 0x80) as usize],
                other => char::from(other),
            })
            .collect(),
    }
}

fn parse(path: &Path, options: CsvOptions) -> Result<CsvTable, CsvError> {
    if options.old_column == 0 || options.new_column == 0 {
        return Err(CsvError::Malformed {
            line: 0,
            message: "columns are numbered from 1".to_owned(),
        });
    }
    if options.old_column == options.new_column {
        return Err(CsvError::SameColumn);
    }

    let bytes = std::fs::read(path).map_err(|e| CsvError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    // Excel prefixes a BOM. Without this the first key is "\u{FEFF}Lorem" and
    // row 1 silently never matches — which looks exactly like a typo in the
    // spreadsheet, and is the single most common way a CSV import "just does
    // nothing".
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(options.separator)
        // "will read and process all lines in the CSV file, including the
        // first one" — so the header row, if there is one, is just a row that
        // happens to match nothing.
        .has_headers(false)
        // A short row is a row that names no replacement, not a broken file.
        .flexible(true)
        .quoting(true)
        .double_quote(true)
        .trim(csv::Trim::None)
        .from_reader(bytes);

    let mut rows: HashMap<String, TextTemplate> = HashMap::new();
    for record in reader.byte_records() {
        let record = record.map_err(|e| {
            let line = e.position().map_or(0, csv::Position::line);
            CsvError::Malformed {
                line,
                message: e.to_string(),
            }
        })?;
        let line = record.position().map_or(0, csv::Position::line);

        if record
            .iter()
            .any(|f| f.contains(&b'\n') || f.contains(&b'\r'))
        {
            return Err(CsvError::NewlineInData { line });
        }

        let (Some(old), Some(new)) = (
            record.get(options.old_column - 1),
            record.get(options.new_column - 1),
        ) else {
            continue;
        };

        let old = file_part(&decode(old)).to_owned();
        let new = new_file_part(&decode(new)).to_owned();
        // An empty key would match every extensionless file; an empty value
        // would erase a name. Neither is a thing a row can mean.
        if old.is_empty() || new.is_empty() {
            continue;
        }

        let key = if options.case_sensitive {
            old
        } else {
            fold(&old)
        };
        // "If the CSV file contains duplicate filenames in the old column, the
        // first one found is used."
        rows.entry(key).or_insert_with(|| TextTemplate::new(new));
    }

    Ok(CsvTable {
        rows,
        case_sensitive: options.case_sensitive,
    })
}

/// Parsed tables, shared process-wide (P40).
///
/// `Cached` alone cannot do this job: `OpKind::to_step` clones, and a clone
/// resets `Cached` by design (D21) — which is exactly what makes editing the
/// path take effect. Without a second layer the file would be re-read and
/// re-parsed on *every keystroke*, for every card.
///
/// Keyed on length and mtime as well as path, so saving the CSV in Excel and
/// pressing refresh does re-read it while typing in an unrelated box does not.
type Cache = Mutex<HashMap<CacheKey, Arc<Result<CsvTable, CsvError>>>>;
static PARSED: OnceLock<Cache> = OnceLock::new();

/// Bounded for the same reason the regex cache is: a user typing a path
/// produces a new key per keystroke and nothing would evict them.
const PARSED_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::Duration>,
    options: CsvOptions,
}

/// Reads `path`, or hands back the copy already parsed for the same file and
/// options.
pub fn load(path: &Path, options: CsvOptions) -> Arc<Result<CsvTable, CsvError>> {
    if path.as_os_str().is_empty() {
        return Arc::new(Err(CsvError::NoFile));
    }

    let stamp = std::fs::metadata(path).ok();
    let key = CacheKey {
        path: path.to_path_buf(),
        len: stamp.as_ref().map_or(0, std::fs::Metadata::len),
        modified: stamp
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
        options,
    };

    let cache = PARSED.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(&key)
    {
        return hit.clone();
    }

    let table = Arc::new(parse(path, options));
    if let Ok(mut map) = cache.lock() {
        if map.len() >= PARSED_CAPACITY {
            map.clear();
        }
        map.insert(key, table.clone());
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn table(body: &str, options: CsvOptions) -> CsvTable {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("list.csv");
        std::fs::write(&path, body).unwrap();
        parse(&path, options).expect("parses")
    }

    fn comma() -> CsvOptions {
        CsvOptions {
            separator: b',',
            old_column: 1,
            new_column: 2,
            case_sensitive: false,
        }
    }

    fn value(t: &CsvTable, key: &str) -> Option<String> {
        t.get(key).map(|v| v.as_str().to_owned())
    }

    /// The documented list, read as a table.
    #[test]
    fn the_first_line_is_processed_like_any_other() {
        let t = table(
            "Old,New\nLorem,Some\nipsum,example\ndolor,text\nsit,I just made\namet,up\n",
            comma(),
        );
        assert_eq!(t.len(), 6, "the header row is a row");
        assert_eq!(value(&t, "Old").as_deref(), Some("New"));
        assert_eq!(value(&t, "sit").as_deref(), Some("I just made"));
    }

    /// `Lorem` in the file, `lorem.txt` on disk — the example only works
    /// because the default is case-insensitive.
    #[test]
    fn lorem_matches_lorem_because_matching_is_case_insensitive_by_default() {
        let t = table("Lorem,Some\n", comma());
        assert_eq!(value(&t, "lorem").as_deref(), Some("Some"));
        assert_eq!(value(&t, "LOREM").as_deref(), Some("Some"));

        let strict = table(
            "Lorem,Some\n",
            CsvOptions {
                case_sensitive: true,
                ..comma()
            },
        );
        assert_eq!(value(&strict, "lorem"), None);
        assert_eq!(value(&strict, "Lorem").as_deref(), Some("Some"));
    }

    #[test]
    fn if_the_old_column_has_duplicates_the_first_one_wins() {
        let t = table("a,first\na,second\n", comma());
        assert_eq!(value(&t, "a").as_deref(), Some("first"));
    }

    #[test]
    fn the_order_of_the_rows_does_not_matter() {
        let forwards = table("a,1\nb,2\n", comma());
        let backwards = table("b,2\na,1\n", comma());
        for key in ["a", "b"] {
            assert_eq!(value(&forwards, key), value(&backwards, key));
        }
    }

    /// *"The column with current names may contain paths, both full and
    /// relative"* — and only the file name is used to match.
    #[test]
    fn a_path_in_the_old_column_is_reduced_to_its_file_name() {
        let t = table("C:\\photos\\lorem,Some\nphotos/ipsum,example\n", comma());
        assert_eq!(value(&t, "lorem").as_deref(), Some("Some"));
        assert_eq!(value(&t, "ipsum").as_deref(), Some("example"));
    }

    /// *"Paths the the column with new names are ignored."* D31 makes `/` a
    /// subfolder move, so this is what stops a spreadsheet relocating files.
    #[test]
    fn a_path_in_the_new_column_cannot_smuggle_a_subfolder_move() {
        let t = table("lorem,2024/Some\nipsum,C:\\elsewhere\\example\n", comma());
        assert_eq!(value(&t, "lorem").as_deref(), Some("Some"));
        assert_eq!(value(&t, "ipsum").as_deref(), Some("example"));
    }

    /// The strip is on literal text, so the tag that means "a separator" still
    /// works — a user asking to move files can still say so.
    /// The strip must not eat the tag that *means* a separator: `<\>` carries
    /// a literal backslash, and a plain `rfind` would cut the value to
    /// `>Some` — destroying the only way a user has of asking for a move.
    #[test]
    fn a_backslash_tag_in_the_new_column_survives_the_strip() {
        let t = table("lorem,2024<\\>Some\nipsum,<Year><\\>x\n", comma());
        assert_eq!(value(&t, "lorem").as_deref(), Some("2024<\\>Some"));
        assert_eq!(value(&t, "ipsum").as_deref(), Some("<Year><\\>x"));
    }

    /// A literal path and a tag in the same value: the literal half goes, the
    /// tag half stays.
    #[test]
    fn a_literal_path_is_stripped_even_when_a_tag_follows_it() {
        let t = table("a,2024/<Year><\\>x\n", comma());
        assert_eq!(value(&t, "a").as_deref(), Some("<Year><\\>x"));
    }

    #[test]
    fn quoted_values_may_contain_the_separator() {
        let t = table("\"a,b\",\"Smith, John\"\n", comma());
        assert_eq!(value(&t, "a,b").as_deref(), Some("Smith, John"));
    }

    #[test]
    fn a_value_containing_a_line_break_is_rejected_with_its_line_number() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("list.csv");
        std::fs::write(&path, "a,1\nb,\"two\nlines\"\n").unwrap();
        assert_eq!(
            parse(&path, comma()).unwrap_err(),
            CsvError::NewlineInData { line: 2 }
        );
    }

    #[test]
    fn a_utf8_bom_is_stripped_so_the_first_row_still_matches() {
        let t = table("\u{FEFF}Lorem,Some\n", comma());
        assert_eq!(
            value(&t, "lorem").as_deref(),
            Some("Some"),
            "the BOM must not become part of the key"
        );
    }

    #[test]
    fn a_csv_written_by_excel_in_cp1252_is_decoded_not_mangled() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("list.csv");
        // "Björk,Jóga" in CP1252.
        std::fs::write(&path, b"Bj\xF6rk,J\xF3ga\n").unwrap();
        let t = parse(&path, comma()).unwrap();
        assert_eq!(value(&t, "björk").as_deref(), Some("Jóga"));
    }

    #[test]
    fn a_short_row_names_no_replacement_and_is_skipped() {
        let t = table("a,1\njust-one-column\nb,2\n", comma());
        assert_eq!(t.len(), 2);
        assert_eq!(value(&t, "b").as_deref(), Some("2"));
    }

    #[test]
    fn an_empty_key_or_value_is_not_a_row() {
        let t = table("a,1\n,2\nb,\n", comma());
        assert_eq!(t.len(), 1);
        assert_eq!(value(&t, "a").as_deref(), Some("1"));
    }

    #[test]
    fn other_separators_and_other_columns() {
        let t = table(
            "ignored;a;1\nignored;b;2\n",
            CsvOptions {
                separator: b';',
                old_column: 2,
                new_column: 3,
                case_sensitive: false,
            },
        );
        assert_eq!(value(&t, "b").as_deref(), Some("2"));
    }

    #[test]
    fn the_two_columns_must_differ() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("list.csv");
        std::fs::write(&path, "a,1\n").unwrap();
        assert_eq!(
            parse(
                &path,
                CsvOptions {
                    new_column: 1,
                    ..comma()
                }
            )
            .unwrap_err(),
            CsvError::SameColumn
        );
    }

    #[test]
    fn a_missing_file_is_reported_with_its_path() {
        let result = load(Path::new("/nowhere/at/all.csv"), comma());
        match &*result {
            Err(CsvError::Io { path, .. }) => assert_eq!(path, Path::new("/nowhere/at/all.csv")),
            other => panic!("expected Io, got {other:?}"),
        }
        assert!(matches!(
            &*load(Path::new(""), comma()),
            Err(CsvError::NoFile)
        ));
    }

    /// The point of the process-wide cache: the same file and options give the
    /// same `Arc`, so a keystroke elsewhere costs a `stat`, not a reparse.
    #[test]
    fn the_same_file_and_options_are_parsed_once() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("list.csv");
        std::fs::write(&path, "a,1\n").unwrap();

        let first = load(&path, comma());
        let second = load(&path, comma());
        assert!(Arc::ptr_eq(&first, &second), "the parse was repeated");

        // Different options are a different table.
        let other = load(
            &path,
            CsvOptions {
                case_sensitive: true,
                ..comma()
            },
        );
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn tags_in_the_new_column_are_kept_for_rendering() {
        let t = table("a,<Counter> - a\n", comma());
        assert!(t.needs().contains(TagNeeds::COUNTER));
        assert!(t.asks().is_empty());

        let asking = table("a,<Ask-2>\nb,<Ask-2>\n", comma());
        assert_eq!(
            asking.asks().len(),
            1,
            "the same slot twice is one question"
        );
        assert_eq!(asking.asks()[0].slot, 2);
    }
}
