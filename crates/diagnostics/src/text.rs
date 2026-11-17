//! Text helpers: line index, UTF-16 columns, applying edits.

use std::ops::Range;

/// A position in a text: 0-based line and 0-based column in UTF-16 code
/// units (what NSH's `EditorPosition` uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPos {
    /// 0-based line.
    pub line: usize,
    /// 0-based column in UTF-16 code units.
    pub column: usize,
}

/// Line starts of a text, for conversions between byte offsets and
/// line/column positions. Lines end at `\n` (a preceding `\r` is not part of
/// the line content).
#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    /// Indexes `text`.
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    /// The indexed text.
    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Number of lines (a trailing newline starts a last, empty line).
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Byte range of the content of `line` (0-based), without the line
    /// terminator.
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        let start = *self.starts.get(line)?;
        let mut end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        if end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        Some(start..end)
    }

    /// Text of `line` (0-based) without the terminator.
    pub fn line_text(&self, line: usize) -> Option<&'a str> {
        self.line_range(line).map(|r| &self.text[r])
    }

    /// The 0-based line containing byte offset `offset` (clamped).
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset.min(self.text.len())) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Position (line, UTF-16 column) of a byte offset. Offsets inside a
    /// character are rounded down to its start.
    pub fn position(&self, offset: usize) -> TextPos {
        let offset = floor_boundary(self.text, offset.min(self.text.len()));
        let line = self.line_of(offset);
        let start = self.starts[line];
        let column = self.text[start..offset].encode_utf16().count();
        TextPos { line, column }
    }

    /// Byte offset of a position; columns past the end of the line clamp to
    /// the line end, lines past the end to the text end.
    pub fn offset(&self, pos: TextPos) -> usize {
        let Some(range) = self.line_range(pos.line) else {
            return self.text.len();
        };
        let line = &self.text[range.clone()];
        let mut units = 0;
        for (i, ch) in line.char_indices() {
            if units >= pos.column {
                return range.start + i;
            }
            units += ch.len_utf16();
        }
        range.end
    }

    /// Byte offset of a 1-based line and 1-based column counted in
    /// characters (`bytes == false`) or bytes (`bytes == true`). Out-of-range
    /// columns clamp to the line end; `None` when the line does not exist.
    pub fn offset_of(&self, line1: usize, column1: usize, bytes: bool) -> Option<usize> {
        let range = self.line_range(line1.checked_sub(1)?)?;
        let line = &self.text[range.clone()];
        let col0 = column1.saturating_sub(1);
        if bytes {
            return Some(range.start + floor_boundary(line, col0.min(line.len())));
        }
        Some(
            line.char_indices()
                .nth(col0)
                .map_or(range.end, |(i, _)| range.start + i),
        )
    }
}

/// The largest char boundary `<= index`.
pub fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The byte offset of the `n`-th character (0-based); the text length when
/// `n` is past the end.
pub fn char_to_byte(text: &str, n: usize) -> usize {
    text.char_indices().nth(n).map_or(text.len(), |(i, _)| i)
}

/// One replacement in a text: `range` (bytes) becomes `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSpan {
    /// Replaced byte range.
    pub range: Range<usize>,
    /// Replacement text.
    pub text: String,
}

/// Applies non-overlapping edits to `source`, in any order. Overlapping or
/// out-of-range edits are skipped (the later one in position order loses).
/// Returns the new text.
pub fn apply_edits(source: &str, edits: &[EditSpan]) -> String {
    let mut sorted: Vec<&EditSpan> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.range.start, e.range.end));
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for edit in sorted {
        let (start, end) = (edit.range.start, edit.range.end);
        if start < cursor
            || end > source.len()
            || start > end
            || !source.is_char_boundary(start)
            || !source.is_char_boundary(end)
        {
            continue;
        }
        out.push_str(&source[cursor..start]);
        out.push_str(&edit.text);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Removes ANSI escape sequences (CSI and OSC) from console text.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for n in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_positions() {
        let text = "ab\r\ncd\n\u{1F600}x";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.line_text(0), Some("ab"));
        assert_eq!(idx.line_text(1), Some("cd"));
        assert_eq!(idx.line_text(2), Some("\u{1F600}x"));
        let x = text.find('x').unwrap();
        assert_eq!(idx.position(x), TextPos { line: 2, column: 2 });
        assert_eq!(idx.offset(TextPos { line: 2, column: 2 }), x);
        assert_eq!(idx.offset(TextPos { line: 1, column: 99 }), 6);
        assert_eq!(idx.offset(TextPos { line: 9, column: 0 }), text.len());
        assert_eq!(idx.line_of(3), 0);
        assert_eq!(idx.line_of(4), 1);
    }

    #[test]
    fn offset_of_columns() {
        let text = "h\u{e9}llo\nx";
        let idx = LineIndex::new(text);
        assert_eq!(idx.offset_of(1, 3, false), Some(3));
        assert_eq!(idx.offset_of(1, 3, true), Some(1), "byte column inside a char rounds down");
        assert_eq!(idx.offset_of(1, 99, false), Some(6));
        assert_eq!(idx.offset_of(3, 1, false), None);
        assert_eq!(idx.offset_of(0, 1, false), None);
    }

    #[test]
    fn edits_apply_in_any_order() {
        let edits = [
            EditSpan {
                range: 4..5,
                text: "Y".into(),
            },
            EditSpan {
                range: 0..1,
                text: "XX".into(),
            },
            EditSpan {
                range: 0..2,
                text: "overlap".into(),
            },
        ];
        assert_eq!(apply_edits("abcdefg", &edits), "XXbcdYfg");
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m ok"), "red ok");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}x"), "x");
    }
}
