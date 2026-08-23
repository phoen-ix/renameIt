//! `<HtmlTitle>` — the `<title>` of an HTML document.
//!
//! It earns a tag of its own: a folder of
//! saved pages is a folder of `Untitled-1.html`, and the one useful name for
//! each of them is inside the file.
//!
//! Hand-written rather than through an HTML parser, and that is the whole
//! decision. A parser builds a DOM for a document we are going to read eleven
//! characters of; this scans a bounded prefix for one tag and stops. `<title>`
//! is required to be in `<head>`, so the prefix is where it is or it is nowhere
//! worth looking.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How much of the file to read.
///
/// `<title>` lives in `<head>`, and a head that has not finished inside 64 KiB
/// is one carrying a megabyte of inline CSS before it — at which point the
/// title is not what that file is for. Bounded because this runs once per file
/// per keystroke (P44) and because the file is untrusted.
const HEAD_BYTES: usize = 64 * 1024;

/// A title longer than this is not a filename anybody wants.
const MAX_TITLE: usize = 512;

/// Extensions worth opening.
pub const HTML_EXTENSIONS: [&str; 5] = ["html", "htm", "xhtml", "shtml", "xht"];

/// The document's title, or `None`.
///
/// `None` covers every reason at once — not HTML, no `<title>`, an empty one.
/// To a renamer they are the same fact, and none of them is an error.
pub fn title_of(path: &Path) -> Option<String> {
    let stamp = std::fs::metadata(path).ok()?;
    if !stamp.is_file() {
        return None;
    }
    let key = Key {
        path: path.to_path_buf(),
        len: stamp.len(),
        modified: stamp
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()),
    };

    let cache = READ.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(&key)
    {
        return hit.clone();
    }

    let value = read_uncached(path);
    if let Ok(mut map) = cache.lock() {
        if map.len() >= super::CACHE_CAPACITY {
            map.clear();
        }
        map.insert(key, value.clone());
    }
    value
}

fn read_uncached(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut head = Vec::new();
    // Lossy, deliberately: a page in Latin-1 or Shift-JIS still has an ASCII
    // `<title>` around text we can at least partly render, and a title with a
    // replacement character in it is more useful than no title at all.
    std::io::BufReader::new(file)
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    extract(&String::from_utf8_lossy(&head))
}

/// Finds `<title …>…</title>` and turns it into one line of text.
///
/// Separated from the reading so every shape below is testable without a file:
/// attributes on the tag, a missing close, entities, and the newlines a
/// hand-written page puts inside a title.
fn extract(source: &str) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    // `to_ascii_lowercase` is length-preserving, so an offset found in the
    // lowered copy is valid in the unfolded string — which is the trap D110
    // records about the general case, avoided here by not folding non-ASCII.
    //
    // Scanned rather than taken from the first hit: `<titlebar>` starts the
    // same way, and a page with one before its real `<title>` must not lose
    // the title. Found by mutation-testing the guard below and noticing it
    // could be removed without a test going red.
    let mut from = 0;
    let (start, end) = loop {
        let open = from + lower[from..].find("<title")?;
        let after = source[open + "<title".len()..].chars().next()?;
        // `<title>` or `<title lang="en">`, but not `<titlebar>`.
        if after.is_alphanumeric() {
            from = open + "<title".len();
            continue;
        }
        let start = open + lower[open..].find('>')? + 1;
        let end = start + lower[start..].find("</title>")?;
        break (start, end);
    };

    let text = decode_entities(&source[start..end]);
    // A title is one line by the time it reaches a filename; the newlines and
    // indentation a hand-written page puts in one are markup, not content.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() || collapsed.len() > MAX_TITLE {
        return None;
    }
    Some(collapsed)
}

/// The five named entities HTML defines and the numeric forms.
///
/// Not the full ~2000-name table: those five are what a title actually
/// contains, and anything else is left as it was written rather than guessed
/// at — a literal `&hellip;` in a filename is at least honest about what the
/// page said.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let body = &tail[1..end];
        let decoded = match body {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => numeric(body),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::Duration>,
}

static READ: OnceLock<Mutex<HashMap<Key, Option<String>>>> = OnceLock::new();

/// Drops every cached read. Tests only: two tempdirs can reuse a path.
pub fn forget_all() {
    if let Some(cache) = READ.get()
        && let Ok(mut map) = cache.lock()
    {
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_taken_from_the_head() {
        assert_eq!(
            extract("<html><head><title>Holiday 2009</title></head><body>x</body>").as_deref(),
            Some("Holiday 2009")
        );
    }

    /// Attributes on the tag, and the case a page actually uses.
    #[test]
    fn the_tag_may_carry_attributes_and_any_case() {
        assert_eq!(
            extract(r#"<TITLE lang="en">Notes</TITLE>"#).as_deref(),
            Some("Notes")
        );
    }

    /// `<titlebar>` is not `<title>`, and a bare prefix match takes it.
    ///
    /// The second case is the one that matters and the one the first missed:
    /// with a real title *after* the impostor, a scan that gave up on the
    /// first `<title` would return the wrong text — or nothing.
    #[test]
    fn a_longer_tag_that_starts_the_same_is_not_a_title() {
        assert_eq!(extract("<titlebar>Nope</titlebar>"), None);
        assert_eq!(
            extract("<titlebar>Nope</titlebar><title>Real</title>").as_deref(),
            Some("Real")
        );
    }

    /// The newlines and indentation a hand-written page puts inside a title are
    /// markup, not content — and a filename is one line.
    #[test]
    fn a_title_written_across_lines_becomes_one_line() {
        assert_eq!(
            extract("<title>\n  Annual\n  Report\n</title>").as_deref(),
            Some("Annual Report")
        );
    }

    #[test]
    fn the_entities_a_title_really_contains_are_decoded() {
        assert_eq!(
            extract("<title>Tom &amp; Jerry &lt;1940&gt; &#8211; &quot;classic&quot;</title>")
                .as_deref(),
            Some("Tom & Jerry <1940> – \"classic\"")
        );
    }

    /// Anything outside the five is left as written rather than guessed at: a
    /// literal `&hellip;` is at least honest about what the page said.
    #[test]
    fn an_entity_we_do_not_know_is_left_alone() {
        assert_eq!(
            extract("<title>Wait&hellip; more</title>").as_deref(),
            Some("Wait&hellip; more")
        );
        assert_eq!(
            extract("<title>Q &amp A</title>").as_deref(),
            Some("Q &amp A"),
            "an unterminated entity is text too"
        );
    }

    #[test]
    fn a_missing_or_empty_title_is_not_an_error() {
        assert_eq!(extract("<html><body>no head at all</body>"), None);
        assert_eq!(extract("<title></title>"), None);
        assert_eq!(extract("<title>   </title>"), None);
        assert_eq!(extract("<title>never closed"), None);
    }

    #[test]
    fn a_title_longer_than_a_filename_is_refused() {
        let long = format!("<title>{}</title>", "x".repeat(MAX_TITLE + 1));
        assert_eq!(extract(&long), None);
    }

    #[test]
    fn a_real_file_is_read_and_cached() {
        forget_all();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(&path, b"<html><head><title>Saved page</title></head>").unwrap();

        assert_eq!(title_of(&path).as_deref(), Some("Saved page"));
        // A second read comes from the cache and says the same thing.
        assert_eq!(title_of(&path).as_deref(), Some("Saved page"));
    }

    /// A folder is not a document, and asking is not a failure.
    #[test]
    fn a_folder_has_no_title() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(title_of(dir.path()), None);
    }
}
