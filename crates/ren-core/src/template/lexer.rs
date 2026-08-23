//! Splitting template text into literals and `<tag>` spans.
//!
//! The grammar is deliberately tiny: everything between a `<` and the next `>`
//! is a tag, everything else is literal text. Two consequences worth
//! stating, because they are what a user actually runs into:
//!
//! * A `<` with no `>` after it is literal, so `a < b` is a perfectly good
//!   template. Only a *closed* pair is a tag, and a closed pair that names
//!   nothing we know is an error (D29).
//! * The tag body is taken verbatim, which is what lets `<\>` and
//!   `<Date-yyyy-mm-dd>` — a backslash and a string full of hyphens — pass
//!   through untouched for [`super::tag`] to interpret.

use std::ops::Range;

/// One piece of template source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'a> {
    Literal(&'a str),
    /// The text between the angle brackets, plus where the whole `<…>` sat, so
    /// the editor can underline exactly what is wrong.
    Tag {
        body: &'a str,
        span: Range<usize>,
    },
}

/// Splits `input`. Never fails: an unrecognised *body* is the tag parser's
/// problem, not the scanner's.
pub fn scan(input: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut literal_from = 0;
    let mut at = 0;

    while let Some(open) = input[at..].find('<') {
        let open = at + open;
        let Some(close) = input[open + 1..].find('>') else {
            // No closing bracket anywhere after this one, so nothing left can be
            // a tag.
            break;
        };
        let close = open + 1 + close;

        if literal_from < open {
            pieces.push(Piece::Literal(&input[literal_from..open]));
        }
        pieces.push(Piece::Tag {
            body: &input[open + 1..close],
            span: open..close + 1,
        });
        at = close + 1;
        literal_from = at;
    }

    if literal_from < input.len() {
        pieces.push(Piece::Literal(&input[literal_from..]));
    }
    pieces
}

/// Whether `input` contains anything the template engine would treat as a tag.
///
/// Lets a caller keep the fast path for the overwhelmingly common case of a
/// field holding plain text.
pub fn has_tags(input: &str) -> bool {
    scan(input)
        .iter()
        .any(|piece| matches!(piece, Piece::Tag { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(input: &str) -> Vec<&str> {
        scan(input)
            .into_iter()
            .filter_map(|p| match p {
                Piece::Tag { body, .. } => Some(body),
                Piece::Literal(_) => None,
            })
            .collect()
    }

    fn literals(input: &str) -> Vec<&str> {
        scan(input)
            .into_iter()
            .filter_map(|p| match p {
                Piece::Literal(text) => Some(text),
                Piece::Tag { .. } => None,
            })
            .collect()
    }

    #[test]
    fn plain_text_is_one_literal() {
        assert_eq!(scan("hello"), vec![Piece::Literal("hello")]);
        assert!(scan("").is_empty());
    }

    /// the Free Format worked example: `<PARENT>_<FULLNAME>`.
    #[test]
    fn tags_and_literals_alternate() {
        assert_eq!(tags("<PARENT>_<FULLNAME>"), ["PARENT", "FULLNAME"]);
        assert_eq!(literals("<PARENT>_<FULLNAME>"), ["_"]);
    }

    #[test]
    fn a_tag_body_is_taken_verbatim() {
        assert_eq!(tags("<Date-yyyy-mm-dd>"), ["Date-yyyy-mm-dd"]);
        assert_eq!(tags(r"<\>"), [r"\"]);
        assert_eq!(tags("<%1>. <%2>"), ["%1", "%2"]);
    }

    #[test]
    fn an_unclosed_bracket_is_literal_text() {
        assert_eq!(scan("a < b"), vec![Piece::Literal("a < b")]);
        assert_eq!(literals("2 <Name"), ["2 <Name"]);
    }

    #[test]
    fn a_stray_closing_bracket_is_literal_text() {
        assert_eq!(scan("a > b"), vec![Piece::Literal("a > b")]);
    }

    /// The first `>` closes: `<a<b>` is one tag whose body is `a<b`, which the
    /// tag parser will reject by name rather than the scanner by shape.
    #[test]
    fn the_first_closing_bracket_wins() {
        assert_eq!(tags("<a<b>"), ["a<b"]);
    }

    #[test]
    fn an_empty_tag_is_reported_as_an_empty_body() {
        assert_eq!(tags("<>"), [""]);
    }

    #[test]
    fn spans_cover_the_brackets() {
        let pieces = scan("ab<Name>cd");
        assert_eq!(pieces[0], Piece::Literal("ab"));
        match &pieces[1] {
            Piece::Tag { body, span } => {
                assert_eq!(*body, "Name");
                assert_eq!(*span, 2..8);
                assert_eq!(&"ab<Name>cd"[span.clone()], "<Name>");
            }
            other => panic!("expected a tag, got {other:?}"),
        }
        assert_eq!(pieces[2], Piece::Literal("cd"));
    }

    #[test]
    fn has_tags_answers_the_cheap_question() {
        assert!(has_tags("<Name>"));
        assert!(has_tags("x<>y"));
        assert!(!has_tags("plain text"));
        assert!(!has_tags("a < b"));
    }
}
