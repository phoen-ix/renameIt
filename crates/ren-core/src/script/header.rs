//! The two lines at the top of a script that are not code.
//!
//! Legacy `.frs` files open with exactly two `key=value` lines:
//!
//! ```text
//! language=vbscript
//! description=This script will replace non English characters with a base …
//! ```
//!
//! `language=` chose between the scripting engines installed on the machine,
//! by naming one on the first line. We run one language, so that line has
//! nothing to select and is
//! dropped; `.koto` says it instead, and editors and the koto LSP get to work
//! without being told (D96).
//!
//! `description=` survives, because it is what the picker shows. It moves into a
//! comment so that a script file is still a valid script file:
//!
//! ```koto
//! # description: Replace non-English characters with a safe base version.
//! # args: none
//! rename = ||
//!   fr.filename
//! ```
//!
//! # `# args:` has no legacy counterpart
//!
//! A `.frs` file has **no** structured argument field — it documents its
//! argument syntax in free prose inside `description=`, leaving the Arguments
//! box an untyped string with its hint buried in a paragraph.
//!
//! `# args:` pulls that hint out where the operation card can put it beside the
//! field instead of making the user read the description to find it. It is an
//! addition, and recorded as one.

/// What the picker and the operation card display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// One line of prose. Empty when the script does not carry one — which is
    /// allowed: a script with no description still runs, it just reads as
    /// nothing but its filename in the picker.
    pub description: String,
    /// The hint shown beside the Arguments field. `None` when the script does
    /// not take arguments, which is different from an empty hint.
    pub args: Option<String>,
}

impl Header {
    /// Read the header off the front of a script.
    ///
    /// Scans only the leading run of comments and blank lines, so a `# args:`
    /// written halfway down the file next to the code that reads it is a
    /// comment and nothing more. That is deliberate: the header is a property
    /// of the file, and a header that could be anywhere is one that has to be
    /// searched for.
    pub fn parse(source: &str) -> Self {
        let mut header = Self::default();
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(comment) = line.strip_prefix('#') else {
                break; // The first real code ends the header.
            };
            let comment = comment.trim();
            if let Some(rest) = strip_key(comment, "description") {
                // First one wins, so a `# description:` mentioned in a later
                // comment cannot overwrite the real one.
                if header.description.is_empty() {
                    header.description = rest.to_owned();
                }
            } else if let Some(rest) = strip_key(comment, "args")
                && header.args.is_none()
            {
                header.args = Some(rest.to_owned());
            }
        }
        header
    }
}

/// `key:` at the start of a comment, case-insensitively, colon required.
///
/// Case-insensitive: a case-sensitive key table is a trap for the user and D62
/// records that as worth not repeating.
fn strip_key<'a>(comment: &'a str, key: &str) -> Option<&'a str> {
    let (head, rest) = comment.split_once(':')?;
    head.trim().eq_ignore_ascii_case(key).then(|| rest.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_the_two_lines() {
        let header = Header::parse(
            "# description: Swap two parts of a filename around a separator.\n\
             # args: the separator, e.g. ' - '\n\
             rename = || fr.filename\n",
        );
        assert_eq!(
            header.description,
            "Swap two parts of a filename around a separator."
        );
        assert_eq!(header.args.as_deref(), Some("the separator, e.g. ' - '"));
    }

    /// A script with no header still runs: the header lines are descriptive,
    /// and nothing requires them.
    #[test]
    fn a_script_without_a_header_is_fine() {
        assert_eq!(Header::parse("rename = || fr.filename"), Header::default());
    }

    /// No arguments at all is a different statement from an empty hint, and the
    /// card renders them differently.
    #[test]
    fn no_args_line_is_not_the_same_as_an_empty_one() {
        assert_eq!(Header::parse("# description: x").args, None);
        assert_eq!(Header::parse("# args:").args.as_deref(), Some(""));
    }

    /// The header is the *front* of the file. A comment further down that
    /// happens to say `description:` is a comment.
    #[test]
    fn the_header_stops_at_the_first_line_of_code() {
        let header = Header::parse(
            "# description: the real one\n\
             rename = ||\n\
             # description: not this one\n\
             \x20 fr.filename\n",
        );
        assert_eq!(header.description, "the real one");
    }

    #[test]
    fn blank_lines_and_spacing_do_not_end_the_header() {
        let header =
            Header::parse("\n  #  Description :  spaced out \n\n#args:x\n\nrename = || ''");
        assert_eq!(header.description, "spaced out");
        assert_eq!(header.args.as_deref(), Some("x"));
    }

    /// A `#` comment that is not a header key is left alone — the ported
    /// scripts carry explanatory comments above the code and none of them
    /// should be mistaken for metadata.
    #[test]
    fn ordinary_comments_are_ignored() {
        let header =
            Header::parse("# Do not modify this script.\n# description: x\nrename = || ''");
        assert_eq!(header.description, "x");
    }
}
