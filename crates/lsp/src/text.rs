//! UTF-16 positions over UTF-8 text, text edits, and the minimal change between two texts.
//!
//! The protocol counts characters in UTF-16 code units and ends lines with `\n`, `\r\n` or `\r`.
//! Rust strings are UTF-8, so every conversion goes through a [`LineIndex`].

use crate::error::{LspError, Result};
use crate::types::{Position, Range, TextEdit};

/// The number of UTF-16 code units in `text`.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte index in `line` (a line without its terminator) of UTF-16 column `column`.
/// A column past the end is the end; a column in the middle of a surrogate pair is the start of
/// that character.
pub fn column_to_byte(line: &str, column: u32) -> usize {
    let mut units = 0u32;
    for (index, ch) in line.char_indices() {
        if units >= column {
            return index;
        }
        let next = units + ch.len_utf16() as u32;
        if next > column {
            return index;
        }
        units = next;
    }
    line.len()
}

/// The start offsets of the lines of one text, for converting between byte offsets and
/// positions. Build it once per text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    /// Indexes `text`.
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut starts = vec![0];
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => starts.push(i + 1),
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    starts.push(i + 1);
                }
                _ => {}
            }
            i += 1;
        }
        Self { starts }
    }

    /// The number of lines. A text that ends with a line break has an empty last line.
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The byte range of line `line` without its terminator.
    pub fn line_range(&self, text: &str, line: usize) -> Option<(usize, usize)> {
        let start = *self.starts.get(line)?;
        let mut end = self.starts.get(line + 1).copied().unwrap_or(text.len());
        let bytes = text.as_bytes();
        if end > start && matches!(bytes[end - 1], b'\n' | b'\r') {
            end -= 1;
            if end > start && bytes[end] == b'\n' && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        }
        Some((start, end))
    }

    /// The byte offset of `position`, or `None` if its line does not exist. A character past the
    /// end of the line is the end of the line.
    pub fn offset(&self, text: &str, position: Position) -> Option<usize> {
        let (start, end) = self.line_range(text, position.line as usize)?;
        Some(start + column_to_byte(&text[start..end], position.character))
    }

    /// Like [`offset`](Self::offset), but a line past the end clamps to the end of the text.
    pub fn offset_clamped(&self, text: &str, position: Position) -> usize {
        self.offset(text, position).unwrap_or(text.len())
    }

    /// The position of byte offset `offset` (clamped to the text, and to a character boundary).
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = match self.starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        };
        let (start, end) = self.line_range(text, line).unwrap_or((0, 0));
        let upto = offset.min(end).max(start);
        Position::new(line as u32, utf16_len(&text[start..upto]) as u32)
    }

    /// The range covering the byte range `start..end`.
    pub fn range(&self, text: &str, start: usize, end: usize) -> Range {
        Range {
            start: self.position(text, start),
            end: self.position(text, end),
        }
    }

    /// The position of the end of the text.
    pub fn end_position(&self, text: &str) -> Position {
        self.position(text, text.len())
    }
}

/// The position of byte offset `offset` in `text`. Indexes the text on every call; use a
/// [`LineIndex`] for several conversions.
pub fn position_at(text: &str, offset: usize) -> Position {
    LineIndex::new(text).position(text, offset)
}

/// The byte offset of `position` in `text` (clamped, see [`LineIndex::offset_clamped`]).
pub fn offset_at(text: &str, position: Position) -> usize {
    LineIndex::new(text).offset_clamped(text, position)
}

/// The UTF-16 offset from the start of `text` of byte offset `offset`.
pub fn utf16_offset(text: &str, offset: usize) -> u32 {
    utf16_len(&text[..offset.min(text.len())]) as u32
}

/// The byte offset of UTF-16 offset `units` from the start of `text` (clamped).
pub fn byte_offset_of_utf16(text: &str, units: u32) -> usize {
    let mut seen = 0u32;
    for (index, ch) in text.char_indices() {
        if seen >= units {
            return index;
        }
        seen += ch.len_utf16() as u32;
    }
    text.len()
}

/// Applies `edits`, all relative to `text`, and returns the new text. Edits must not overlap;
/// those at the same place apply in the order given. Positions past the end of a line clamp to
/// the line's end, but a line that does not exist is an error, so a stale edit is refused rather
/// than applied somewhere unrelated.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> Result<String> {
    let index = LineIndex::new(text);
    let mut resolved: Vec<(usize, usize, usize, &str)> = Vec::with_capacity(edits.len());
    for (order, edit) in edits.iter().enumerate() {
        let start = index.offset(text, edit.range.start).ok_or_else(|| {
            LspError::InvalidEdit(format!("line {} does not exist", edit.range.start.line))
        })?;
        let end = index.offset(text, edit.range.end).ok_or_else(|| {
            LspError::InvalidEdit(format!("line {} does not exist", edit.range.end.line))
        })?;
        if end < start {
            return Err(LspError::InvalidEdit(
                "a range ends before it starts".to_owned(),
            ));
        }
        resolved.push((start, end, order, edit.new_text.as_str()));
    }
    resolved.sort_by_key(|&(start, end, order, _)| (start, end, order));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end, _, new_text) in resolved {
        if start < cursor {
            return Err(LspError::InvalidEdit("edits overlap".to_owned()));
        }
        out.push_str(&text[cursor..start]);
        out.push_str(new_text);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

/// One incremental change of a document (`TextDocumentContentChangeEvent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentChange {
    /// The replaced range in the old text.
    pub range: Range,
    /// The length of the replaced range in UTF-16 units.
    pub range_length: u32,
    /// The text that replaces it.
    pub text: String,
}

/// The smallest single replacement that turns `old` into `new`: the common prefix and suffix are
/// kept. `None` if the texts are equal.
pub fn diff_change(old: &str, new: &str) -> Option<ContentChange> {
    if old == new {
        return None;
    }
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    let mut prefix = old_bytes
        .iter()
        .zip(new_bytes)
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    // Never split a `\r\n` pair: positions inside one are not addressable.
    let split = |bytes: &[u8], at: usize| {
        at > 0 && at < bytes.len() && bytes[at - 1] == b'\r' && bytes[at] == b'\n'
    };
    if split(old_bytes, prefix) || split(new_bytes, prefix) {
        prefix -= 1;
    }
    let max_suffix = old_bytes.len().min(new_bytes.len()) - prefix;
    let mut suffix = old_bytes
        .iter()
        .rev()
        .zip(new_bytes.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while suffix > 0
        && !(old.is_char_boundary(old.len() - suffix) && new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    if suffix > 0 && (split(old_bytes, old.len() - suffix) || split(new_bytes, new.len() - suffix))
    {
        suffix -= 1;
    }
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    let index = LineIndex::new(old);
    Some(ContentChange {
        range: index.range(old, prefix, old_end),
        range_length: utf16_len(&old[prefix..old_end]) as u32,
        text: new[prefix..new_end].to_owned(),
    })
}

/// The identifier-like word around or before `position`: its range and text. `extra` lists
/// characters (besides letters, digits and `_`) that belong to words in the language.
pub fn word_before(text: &str, position: Position, extra: &str) -> (Range, String) {
    let index = LineIndex::new(text);
    let Some((start, end)) = index.line_range(text, position.line as usize) else {
        return (Range::at(position), String::new());
    };
    let line = &text[start..end];
    let caret = column_to_byte(line, position.character);
    let word_start = line[..caret]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_word_char(*ch, extra))
        .last()
        .map_or(caret, |(i, _)| i);
    let range = Range {
        start: Position::new(position.line, utf16_len(&line[..word_start]) as u32),
        end: Position::new(position.line, utf16_len(&line[..caret]) as u32),
    };
    (range, line[word_start..caret].to_owned())
}

/// Whether `ch` belongs to a word: a letter, digit, `_`, or one of `extra`.
pub fn is_word_char(ch: char, extra: &str) -> bool {
    ch.is_alphanumeric() || ch == '_' || extra.contains(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range::new(sl, sc, el, ec),
            new_text: text.to_owned(),
        }
    }

    #[test]
    fn utf16_columns_count_surrogate_pairs() {
        let text = "a😀b\nné中";
        // 'a' 1 unit, the emoji 2, 'b' 1.
        assert_eq!(position_at(text, 1), Position::new(0, 1));
        assert_eq!(position_at(text, 5), Position::new(0, 3));
        assert_eq!(position_at(text, "a😀b".len()), Position::new(0, 4));
        assert_eq!(offset_at(text, Position::new(0, 3)), 5);
        assert_eq!(offset_at(text, Position::new(1, 3)), "a😀b\nné中".len());
        // The middle of a pair rounds to its start.
        assert_eq!(offset_at(text, Position::new(0, 2)), 1);
    }

    #[test]
    fn all_three_line_endings() {
        let text = "one\r\ntwo\rthree\nfour";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.line_range(text, 0), Some((0, 3)));
        assert_eq!(
            &text[index.line_range(text, 1).unwrap().0..index.line_range(text, 1).unwrap().1],
            "two"
        );
        assert_eq!(index.position(text, text.len()), Position::new(3, 4));
        assert_eq!(
            index.offset(text, Position::new(2, 99)),
            Some(text.find("four").unwrap() - 1)
        );
        // Inside \r\n: the end of the line.
        assert_eq!(index.position(text, 4), Position::new(0, 3));
        assert_eq!(index.offset(text, Position::new(9, 0)), None);
    }

    #[test]
    fn trailing_newline_makes_an_empty_last_line() {
        let text = "a\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 2);
        assert_eq!(index.end_position(text), Position::new(1, 0));
        assert_eq!(index.offset(text, Position::new(1, 0)), Some(2));
    }

    #[test]
    fn applies_edits_in_any_order_and_rejects_overlaps() {
        let text = "hello 😀 world";
        let out =
            apply_edits(text, &[edit(0, 9, 0, 14, "there"), edit(0, 0, 0, 5, "bye")]).unwrap();
        assert_eq!(out, "bye 😀 there");
        assert!(apply_edits(text, &[edit(0, 0, 0, 5, "a"), edit(0, 3, 0, 6, "b")]).is_err());
        assert!(apply_edits(text, &[edit(4, 0, 4, 0, "a")]).is_err());
        // Inserts at the same place keep the given order.
        let out = apply_edits("x", &[edit(0, 0, 0, 0, "1"), edit(0, 0, 0, 0, "2")]).unwrap();
        assert_eq!(out, "12x");
    }

    #[test]
    fn diff_is_minimal_and_applies_back() {
        let change = diff_change("let x = 1;", "let xy = 1;").unwrap();
        assert_eq!(change.range, Range::new(0, 5, 0, 5));
        assert_eq!(change.text, "y");
        assert_eq!(change.range_length, 0);
        assert!(diff_change("same", "same").is_none());
        let change = diff_change("a😀c", "a😁c").unwrap();
        assert_eq!(change.range, Range::new(0, 1, 0, 3));
        assert_eq!(change.range_length, 2);
        assert_eq!(change.text, "😁");
    }

    #[test]
    fn diff_round_trips_on_awkward_texts() {
        let samples = [
            "",
            "a",
            "a\r\nb",
            "a\nb",
            "a\rb",
            "😀😀",
            "x\r\n\r\ny",
            "\n",
            "\r\n",
            "é\né",
            "ab\r\n",
            "a\r\nb\r\n",
            "tab\t中文\r\nline2",
            "😀\r\n😁",
        ];
        for old in samples {
            for new in samples {
                let Some(change) = diff_change(old, new) else {
                    assert_eq!(old, new);
                    continue;
                };
                let applied = apply_edits(
                    old,
                    &[TextEdit {
                        range: change.range,
                        new_text: change.text.clone(),
                    }],
                )
                .unwrap();
                assert_eq!(applied, new, "{old:?} -> {new:?} via {change:?}");
                let removed =
                    &old[offset_at(old, change.range.start)..offset_at(old, change.range.end)];
                assert_eq!(utf16_len(removed) as u32, change.range_length);
            }
        }
    }

    #[test]
    fn word_before_the_caret() {
        let (range, word) = word_before("let foo_bar = 1", Position::new(0, 11), "");
        assert_eq!(word, "foo_bar");
        assert_eq!(range, Range::new(0, 4, 0, 11));
        let (range, word) = word_before("x = os.", Position::new(0, 7), "");
        assert_eq!(word, "");
        assert_eq!(range, Range::new(0, 7, 0, 7));
        let (_, word) = word_before("echo $HO", Position::new(0, 8), "$");
        assert_eq!(word, "$HO");
        let (range, word) = word_before("éüab", Position::new(0, 4), "");
        assert_eq!(word, "éüab");
        assert_eq!(range.start.character, 0);
    }

    #[test]
    fn byte_offset_of_utf16_units() {
        assert_eq!(byte_offset_of_utf16("a😀b", 3), 5);
        assert_eq!(utf16_offset("a😀b", 5), 3);
        assert_eq!(byte_offset_of_utf16("ab", 9), 2);
    }
}
