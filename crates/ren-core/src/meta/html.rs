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

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::cache::{MetaCache, Stamp};

/// How much of the file to read.
///
/// `<title>` lives in `<head>`, and a head that has not finished inside 64 KiB
/// is one carrying a megabyte of inline CSS before it — at which point the
/// title is not what that file is for. Bounded because this runs once per file
/// per keystroke (P44) and because the file is untrusted.
const HEAD_BYTES: usize = 64 * 1024;

/// A title longer than this is not a filename anybody wants.
const MAX_TITLE: usize = 512;

/// How far past an `&` the closing `;` of an entity is looked for. The longest
/// form this decoder accepts is `&#x10FFFF;`, ten characters.
const ENTITY_SCAN_CHARS: usize = 12;

/// Extensions worth opening.
///
/// The gate every other reader has and this one lacked: without it
/// `<HtmlTitle>` opened *every* file in the listing — 64 KiB of each
/// photograph and each track — on the first preview, and the extension is
/// the only thing that says a file might be a page at all.
pub const HTML_EXTENSIONS: [&str; 5] = ["html", "htm", "xhtml", "shtml", "xht"];

/// The smallest thing that could possibly be a page with a title (P50):
/// `<title>x</title>`.
const MIN_HTML_BYTES: u64 = 16;

/// The document's title, or `None`.
///
/// `None` covers every reason at once — not HTML, no `<title>`, an empty one.
/// To a renamer they are the same fact, and none of them is an error.
pub fn title_of(path: &Path) -> Option<String> {
    title_at(path, Stamp::stat(path)?)
}

/// The same, for a listed entry — the parallel pass's way in, keyed on the
/// entry's own stamp so it costs no syscall (see `meta::cache`).
pub fn title_of_entry(entry: &crate::model::FileEntry) -> Option<String> {
    title_at(&entry.path, Stamp::of_entry(entry))
}

fn title_at(path: &Path, stamp: Stamp) -> Option<String> {
    if stamp.is_dir
        || stamp.len < MIN_HTML_BYTES
        || !super::folder::has_extension(path, &HTML_EXTENSIONS)
    {
        return None;
    }
    READ.get_or_init(Default::default)
        .get_or_read(path, stamp, || read_uncached(path).map(Arc::from))
        .map(|title| title.to_string())
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
        // The `;` is looked for within the first twelve **characters**, never
        // by slicing at byte twelve: `tail` starts at an ASCII `&`, but what
        // follows is a stranger's title, and slicing a `str` inside a
        // multi-byte character panics. Found by a title of the shape
        // `& Tom and Jé…`, where the `é` straddles byte twelve.
        let end = tail
            .char_indices()
            .take(ENTITY_SCAN_CHARS)
            .find(|(_, c)| *c == ';')
            .map(|(i, _)| i);
        let Some(end) = end else {
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

/// Shared with every other reader through [`super::cache::MetaCache`]. The
/// title is an `Arc<str>` so a hit is a refcount bump rather than a copy.
static READ: OnceLock<MetaCache<Option<Arc<str>>>> = OnceLock::new();

/// Drops every cached read. Tests only: two tempdirs can reuse a path.
pub fn forget_all() {
    if let Some(cache) = READ.get() {
        cache.clear();
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

    /// The entity scan is bounded in characters, not bytes. Slicing at byte
    /// twelve panics whenever a multi-byte character straddles it, and this
    /// title puts the `é` exactly there: `&` plus ten ASCII characters is
    /// eleven bytes, so the two-byte `é` occupies bytes eleven and twelve.
    #[test]
    fn an_ampersand_followed_by_non_ascii_text_does_not_panic() {
        assert_eq!(
            extract("<title>Q & Tom and Jérôme</title>").as_deref(),
            Some("Q & Tom and Jérôme")
        );
        // Three-byte and four-byte characters at every offset in the window.
        for filler in 0..ENTITY_SCAN_CHARS + 2 {
            let title = format!("&{}—😀;x", "a".repeat(filler));
            let _ = decode_entities(&title);
        }
        // A real entity still decodes when non-ASCII text follows it.
        assert_eq!(decode_entities("&amp;Jérôme"), "&Jérôme");
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

    /// Only a file that could be a page is opened at all: a `.jpg` with a
    /// `<title>` inside is a picture, and reading 64 KiB of every photograph
    /// on the first preview was the cost this gate removes.
    #[test]
    fn only_a_file_with_a_page_extension_is_opened() {
        forget_all();
        let dir = tempfile::TempDir::new().unwrap();
        let body = b"<html><head><title>Saved page</title></head>";
        let page = dir.path().join("page.HTM");
        let not = dir.path().join("photo.jpg");
        std::fs::write(&page, body).unwrap();
        std::fs::write(&not, body).unwrap();
        assert_eq!(title_of(&page).as_deref(), Some("Saved page"));
        assert_eq!(title_of(&not), None);
    }

    /// A folder is not a document, and asking is not a failure.
    #[test]
    fn a_folder_has_no_title() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(title_of(dir.path()), None);
    }
}
