//! Text positions: byte offsets, lines and UTF-16 columns.
//!
//! All engines in this crate work on `&str` and report **byte offsets**. The
//! NSH editor speaks `(line, UTF-16 column)` (0-based, the same as LSP), so
//! [`LineIndex`] converts between the two. The editor normalises `\r\n` to
//! `\n` in `value()`; the conversions here also work on text that still has
//! `\r\n` (the `\r` is not counted as part of the line when clamping).
//!
//! Only `\n` separates lines. A lone `\r` is an ordinary character.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// A 0-based line and UTF-16 column, as the NSH editor and LSP use.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Position {
    /// 0-based line.
    pub line: u32,
    /// UTF-16 code units from the start of the line.
    pub column: u32,
}

impl Position {
    /// Creates a position.
    pub const fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A half-open range of [`Position`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PositionRange {
    /// Start (inclusive).
    pub start: Position,
    /// End (exclusive).
    pub end: Position,
}

/// Number of UTF-16 code units `s` needs.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Rounds `offset` down to a char boundary of `text` (and clamps to its length).
pub fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while o > 0 && !text.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// Rounds `offset` up to a char boundary of `text` (and clamps to its length).
pub fn ceil_char_boundary(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while o < text.len() && !text.is_char_boundary(o) {
        o += 1;
    }
    o
}

/// Converts a byte offset inside `line_text` to a UTF-16 column.
pub fn byte_to_utf16(line_text: &str, byte: usize) -> usize {
    utf16_len(&line_text[..floor_char_boundary(line_text, byte)])
}

/// Converts a UTF-16 column to a byte offset inside `line_text`. A column in
/// the middle of a surrogate pair rounds down; a column past the end clamps.
pub fn utf16_to_byte(line_text: &str, column: usize) -> usize {
    let mut units = 0usize;
    for (i, c) in line_text.char_indices() {
        let next = units + c.len_utf16();
        if next > column {
            return i;
        }
        units = next;
    }
    line_text.len()
}

/// Line starts of one text. Build once per text revision, then convert freely.
///
/// The index stores offsets only; pass the same text to every call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    /// Indexes `text`.
    pub fn new(text: &str) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 32 + 1);
        starts.push(0);
        starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|i| i + 1));
        Self {
            starts,
            len: text.len(),
        }
    }

    /// Number of lines (an empty text has one empty line; a trailing `\n`
    /// starts one more).
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Byte length of the indexed text.
    pub fn text_len(&self) -> usize {
        self.len
    }

    /// 0-based line containing the byte `offset` (clamped to the last line).
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Byte offset where `line` starts (clamped to the text length).
    pub fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.len)
    }

    /// Byte range of `line` without its `\n` or `\r\n` terminator.
    pub fn line_range(&self, text: &str, line: usize) -> Range<usize> {
        let start = self.line_start(line);
        let Some(next) = self.starts.get(line + 1) else {
            // Last line: no terminator, a trailing `\r` is content.
            return start..self.len;
        };
        let mut end = next - 1; // the `\n`
        if end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        start..end
    }

    /// The text of `line` without its terminator.
    pub fn line_text<'t>(&self, text: &'t str, line: usize) -> &'t str {
        &text[self.line_range(text, line)]
    }

    /// Byte offset to `(line, UTF-16 column)`. Offsets inside a char round down.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = floor_char_boundary(text, offset);
        let line = self.line_of(offset);
        let start = self.starts[line];
        Position {
            line: line as u32,
            column: utf16_len(&text[start..offset]) as u32,
        }
    }

    /// `(line, UTF-16 column)` to a byte offset. Lines past the end clamp to
    /// the end of the text; columns past the end of the line clamp to the end
    /// of the line (before its terminator).
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.starts.len() {
            return text.len();
        }
        let range = self.line_range(text, line);
        range.start + utf16_to_byte(&text[range.clone()], pos.column as usize)
    }

    /// Converts a byte range to a [`PositionRange`].
    pub fn position_range(&self, text: &str, range: &Range<usize>) -> PositionRange {
        PositionRange {
            start: self.position(text, range.start),
            end: self.position(text, range.end),
        }
    }

    /// Converts a [`PositionRange`] to a byte range.
    pub fn byte_range(&self, text: &str, range: &PositionRange) -> Range<usize> {
        self.offset(text, range.start)..self.offset(text, range.end)
    }

    /// UTF-16 offset from the start of the text for a byte offset (what the
    /// editor's `offset_to_position`/`position_to_offset` use).
    pub fn byte_to_utf16_offset(&self, text: &str, byte: usize) -> usize {
        utf16_len(&text[..floor_char_boundary(text, byte)])
    }

    /// Byte offset for a UTF-16 offset from the start of the text.
    pub fn utf16_offset_to_byte(&self, text: &str, utf16: usize) -> usize {
        utf16_to_byte(text, utf16)
    }
}
