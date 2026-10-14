//! The `.env` file format: a lossless parser and a writer.
//!
//! The parser keeps every line it reads, including comments, blank lines,
//! and the exact text of each entry, so a file can be loaded, changed, and
//! saved without disturbing anything the user wrote by hand. Only entries
//! whose value changed are re-rendered.
//!
//! Accepted syntax, one entry per line:
//!
//! ```text
//! # comment
//! NAME=value                 unquoted; ends at end of line or ` #`
//! NAME = value               spaces around `=` are allowed
//! export NAME=value          `export ` is accepted and kept
//! NAME="a\nb \"q\" \$x"      double quotes: \n \r \t \" \\ \$ escapes, may span lines
//! NAME='literal $x'          single quotes: no escapes, may span lines
//! NAME=                      empty value
//! ```
//!
//! Variable interpolation (`${OTHER}`) is deliberately not performed: a
//! value is exactly what the file says, so a file cannot pull arbitrary
//! environment state into a variable.

use std::path::Path;

use crate::error::{Error, Result};
use crate::key;

/// One line (or, for multi-line quoted values, one group of lines) of a
/// `.env` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line {
    /// A `NAME=value` entry.
    Entry {
        key: String,
        value: String,
        /// The original text, without line terminators.
        raw: String,
        /// Whether the line started with `export `.
        export: bool,
        /// An inline comment after the value, with the whitespace before
        /// it (`"  # note"`), or empty. Kept when the value is rewritten.
        comment: String,
    },
    /// A comment or blank line, verbatim.
    Other(String),
}

/// A parsed `.env` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Document {
    pub lines: Vec<Line>,
    /// Whether the file used `\r\n` line endings.
    pub crlf: bool,
}

impl Document {
    /// Renders the document, ending with a line terminator when non-empty.
    pub(crate) fn render(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        for line in &self.lines {
            let text = match line {
                Line::Entry { raw, .. } => raw,
                Line::Other(text) => text,
            };
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    out.push_str(eol);
                }
                out.push_str(part);
            }
            out.push_str(eol);
        }
        out
    }
}

/// Parses `.env` text. `path` is only used for error messages.
pub(crate) fn parse(text: &str, path: Option<&Path>) -> Result<Document> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let crlf = text.contains("\r\n");
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    if raw_lines.last() == Some(&"") {
        let _trailing = raw_lines.pop();
    }
    let raw_lines: Vec<&str> = raw_lines
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let mut doc = Document {
        lines: Vec::new(),
        crlf,
    };
    let mut i = 0;
    while i < raw_lines.len() {
        let start = i;
        let line = raw_lines[i];
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            doc.lines.push(Line::Other(line.to_owned()));
            i += 1;
            continue;
        }
        let syntax = |reason| Error::Syntax {
            path: path.map(Path::to_path_buf),
            line: start + 1,
            reason,
        };
        let (export, body) = match trimmed.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => (true, rest.trim_start()),
            _ => (false, trimmed),
        };
        let (name, rest) = body
            .split_once('=')
            .ok_or_else(|| syntax("expected NAME=value"))?;
        let name = name.trim_end();
        key::check_name(name).map_err(|_| syntax("invalid variable name"))?;
        let rest = rest.trim_start_matches([' ', '\t']);

        let (value, consumed) = match rest.chars().next() {
            Some('"') => quoted(&raw_lines, i, line.len() - rest.len() + 1, '"')
                .ok_or_else(|| syntax("unterminated double-quoted value"))?,
            Some('\'') => quoted(&raw_lines, i, line.len() - rest.len() + 1, '\'')
                .ok_or_else(|| syntax("unterminated single-quoted value"))?,
            _ => (unquoted(rest), 1),
        };
        let comment = match value.after {
            Some(after) => {
                let trimmed = after.trim_start();
                if trimmed.is_empty() {
                    String::new()
                } else if trimmed.starts_with('#') {
                    after.trim_end().to_owned()
                } else {
                    return Err(syntax("unexpected text after the closing quote"));
                }
            }
            None => String::new(),
        };
        key::check_value(name, &value.text)
            .map_err(|_| syntax("value contains NUL or is too long"))?;
        let raw = raw_lines[start..start + consumed].join("\n");
        doc.lines.push(Line::Entry {
            key: name.to_owned(),
            value: value.text,
            raw,
            export,
            comment,
        });
        i += consumed;
    }
    Ok(doc)
}

/// A parsed value and whatever followed its closing quote on the last
/// line, if it was quoted.
struct Value<'a> {
    text: String,
    after: Option<&'a str>,
}

/// An unquoted value: up to an inline comment (` #`), trimmed.
fn unquoted(rest: &str) -> Value<'_> {
    let bytes = rest.as_bytes();
    let mut end = rest.len();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            end = i;
            break;
        }
    }
    let text = rest[..end].trim_end();
    let tail = &rest[text.len()..];
    Value {
        text: text.to_owned(),
        // Only an actual comment is kept; bare trailing spaces are not.
        after: tail.contains('#').then_some(tail),
    }
}

/// A quoted value starting at byte `from` of line `first` (just after the
/// opening quote), possibly spanning following lines. Returns the value and
/// the number of lines consumed, or `None` if the quote never closes.
fn quoted<'a>(
    lines: &[&'a str],
    first: usize,
    from: usize,
    quote: char,
) -> Option<(Value<'a>, usize)> {
    let mut text = String::new();
    let mut line_no = first;
    let mut segment = &lines[first][from..];
    loop {
        let mut chars = segment.char_indices();
        while let Some((idx, c)) = chars.next() {
            if c == quote {
                let after = &segment[idx + c.len_utf8()..];
                return Some((
                    Value {
                        text,
                        after: Some(after),
                    },
                    line_no - first + 1,
                ));
            }
            if quote == '"' && c == '\\' {
                match chars.next() {
                    Some((_, 'n')) => text.push('\n'),
                    Some((_, 'r')) => text.push('\r'),
                    Some((_, 't')) => text.push('\t'),
                    Some((_, e @ ('"' | '\\' | '$'))) => text.push(e),
                    Some((_, other)) => {
                        text.push('\\');
                        text.push(other);
                    }
                    None => text.push('\\'),
                }
                continue;
            }
            text.push(c);
        }
        line_no += 1;
        segment = lines.get(line_no)?;
        text.push('\n');
    }
}

/// Renders one entry line for `key` and `value`.
pub(crate) fn render_entry(key: &str, value: &str, export: bool) -> String {
    let mut out = String::with_capacity(key.len() + value.len() + 10);
    if export {
        out.push_str("export ");
    }
    out.push_str(key);
    out.push('=');
    if value.bytes().all(is_plain) {
        out.push_str(value);
    } else {
        out.push('"');
        for c in value.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '$' => out.push_str("\\$"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other => out.push(other),
            }
        }
        out.push('"');
    }
    out
}

/// Bytes that can appear in an unquoted value without changing its meaning
/// to this parser or to other common `.env` readers.
#[inline]
const fn is_plain(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b',' | b'+' | b'%' | b'='
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<(String, String)> {
        parse(text, None)
            .unwrap()
            .lines
            .into_iter()
            .filter_map(|l| match l {
                Line::Entry { key, value, .. } => Some((key, value)),
                Line::Other(_) => None,
            })
            .collect()
    }

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn test_parse_basic_forms() {
        let text = "A=1\nB = two words \nexport C=3\nD=\nE=x#not-comment\nF=y # comment\n";
        assert_eq!(
            entries(text),
            kv(&[
                ("A", "1"),
                ("B", "two words"),
                ("C", "3"),
                ("D", ""),
                ("E", "x#not-comment"),
                ("F", "y")
            ])
        );
    }

    #[test]
    fn test_parse_double_quoted_escapes_and_multiline() {
        let text = "A=\"line1\\nline2 \\\"q\\\" \\\\ \\$HOME \\t\"\nB=\"first\nsecond\" # note\nC=\"\\x\"\n";
        assert_eq!(
            entries(text),
            kv(&[
                ("A", "line1\nline2 \"q\" \\ $HOME \t"),
                ("B", "first\nsecond"),
                ("C", "\\x")
            ])
        );
    }

    #[test]
    fn test_parse_single_quoted_is_literal() {
        assert_eq!(
            entries("A='no \\n $X escapes'\nB='multi\nline'\n"),
            kv(&[("A", "no \\n $X escapes"), ("B", "multi\nline")])
        );
    }

    #[test]
    fn test_parse_keeps_comments_blanks_and_raw_text() {
        let text = "# header\n\nA = 1   # keep me\n  # indented\n";
        let doc = parse(text, None).unwrap();
        assert_eq!(doc.lines.len(), 4);
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn test_parse_crlf_and_bom_round_trip() {
        let text = "\u{FEFF}A=1\r\n# c\r\nB=\"x\r\ny\"\r\n";
        let doc = parse(text, None).unwrap();
        assert!(doc.crlf);
        assert_eq!(entries(text), kv(&[("A", "1"), ("B", "x\ny")]));
        assert_eq!(doc.render(), text.trim_start_matches('\u{FEFF}'));
    }

    #[test]
    fn test_parse_errors_report_line_numbers() {
        let cases = [
            ("A=1\nnot an entry\n", 2, "expected NAME=value"),
            ("A=1\n\n1BAD=x\n", 3, "invalid variable name"),
            (
                "A=\"open\nstill open\n",
                1,
                "unterminated double-quoted value",
            ),
            ("A='open\n", 1, "unterminated single-quoted value"),
            (
                "A=\"x\" junk\n",
                1,
                "unexpected text after the closing quote",
            ),
            ("A=a\0b\n", 1, "value contains NUL or is too long"),
            ("export=1\nexport X\n", 2, "expected NAME=value"),
        ];
        for (text, line, reason) in cases {
            match parse(text, Some(Path::new(".env"))) {
                Err(Error::Syntax {
                    line: l,
                    reason: r,
                    path,
                }) => {
                    assert_eq!((l, r), (line, reason), "{text:?}");
                    assert_eq!(path.as_deref(), Some(Path::new(".env")));
                }
                other => panic!("{text:?} -> {other:?}"),
            }
        }
    }

    #[test]
    fn test_parse_export_as_a_name() {
        assert_eq!(
            entries("export=1\nexported=2\n"),
            kv(&[("export", "1"), ("exported", "2")])
        );
    }

    #[test]
    fn test_parse_duplicates_are_all_kept_in_document() {
        assert_eq!(entries("A=1\nA=2\n"), kv(&[("A", "1"), ("A", "2")]));
    }

    #[test]
    fn test_parse_empty_and_whitespace_only() {
        assert!(parse("", None).unwrap().lines.is_empty());
        assert_eq!(parse("\n  \n", None).unwrap().lines.len(), 2);
    }

    #[test]
    fn test_render_entry_quotes_only_when_needed() {
        assert_eq!(
            render_entry("A", "plain-value_1.2/x:y@z,+%=", false),
            "A=plain-value_1.2/x:y@z,+%="
        );
        assert_eq!(render_entry("A", "", false), "A=");
        assert_eq!(render_entry("A", "two words", false), "A=\"two words\"");
        assert_eq!(
            render_entry("A", "a\"b\\c$d\ne\rf\tg", true),
            "export A=\"a\\\"b\\\\c\\$d\\ne\\rf\\tg\""
        );
        assert_eq!(render_entry("A", "#hash", false), "A=\"#hash\"");
        assert_eq!(render_entry("A", "名前", false), "A=\"名前\"");
    }

    #[test]
    fn test_render_then_parse_round_trips() {
        for value in [
            "", "x", "a b", "\"", "\\", "$X", "#", " lead", "trail ", "a\nb", "\r\n", "'", "名前",
        ] {
            let line = render_entry("K", value, false);
            assert_eq!(
                entries(&format!("{line}\n")),
                kv(&[("K", value)]),
                "{value:?}"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod comment_tests {
    use super::*;

    fn comment_of(text: &str) -> String {
        match parse(text, None).unwrap().lines.into_iter().next() {
            Some(Line::Entry { comment, .. }) => comment,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_inline_comments_are_recorded_with_spacing() {
        assert_eq!(comment_of("A=1  # note\n"), "  # note");
        assert_eq!(comment_of("A=1\t#tab\n"), "\t#tab");
        assert_eq!(comment_of("A=\"x\"   # quoted\n"), "   # quoted");
        assert_eq!(comment_of("A='x' #c\n"), " #c");
        assert_eq!(comment_of("A=\"multi\nline\" # end\n"), " # end");
    }

    #[test]
    fn test_no_comment_and_trailing_spaces_record_nothing() {
        assert_eq!(comment_of("A=1\n"), "");
        assert_eq!(comment_of("A=1   \n"), "");
        assert_eq!(comment_of("A=\"x\"   \n"), "");
        assert_eq!(comment_of("A=x#not-a-comment\n"), "");
    }
}
