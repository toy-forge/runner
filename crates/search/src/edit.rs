//! Text edits produced by replace operations.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::text::{LineIndex, PositionRange};

/// Replace `range` (byte offsets of the **original** text) with `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// The replaced byte range.
    pub range: Range<usize>,
    /// The replacement.
    pub text: String,
}

impl TextEdit {
    /// Creates an edit.
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }
}

/// A [`TextEdit`] in editor coordinates (line, UTF-16 column). This is the
/// shape of `nsh_base::editor::EditorEdit { range, text }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionEdit {
    /// The replaced range.
    pub range: PositionRange,
    /// The replacement.
    pub text: String,
}

/// Why a list of edits cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditError {
    /// An edit lies (partly) outside the text or splits a UTF-8 char.
    #[error("edit {0:?} is out of range or not on a char boundary")]
    OutOfRange(Range<usize>),
    /// Two edits overlap, or the list is not sorted by start.
    #[error("edits overlap or are not sorted: {0:?} then {1:?}")]
    Overlap(Range<usize>, Range<usize>),
}

/// Checks that `edits` are sorted, non-overlapping and inside `text`.
pub fn validate_edits(text: &str, edits: &[TextEdit]) -> Result<(), EditError> {
    let mut prev: Option<&Range<usize>> = None;
    for e in edits {
        let r = &e.range;
        if r.start > r.end
            || r.end > text.len()
            || !text.is_char_boundary(r.start)
            || !text.is_char_boundary(r.end)
        {
            return Err(EditError::OutOfRange(r.clone()));
        }
        if let Some(p) = prev
            && r.start < p.end
        {
            return Err(EditError::Overlap(p.clone(), r.clone()));
        }
        prev = Some(r);
    }
    Ok(())
}

/// Applies sorted, non-overlapping `edits` (ranges of the original `text`).
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> Result<String, EditError> {
    validate_edits(text, edits)?;
    let extra: usize = edits.iter().map(|e| e.text.len()).sum();
    let mut out = String::with_capacity(text.len() + extra);
    let mut at = 0;
    for e in edits {
        out.push_str(&text[at..e.range.start]);
        out.push_str(&e.text);
        at = e.range.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Converts byte edits of `text` to editor-coordinate edits.
pub fn to_position_edits(text: &str, edits: &[TextEdit]) -> Vec<PositionEdit> {
    let index = LineIndex::new(text);
    edits
        .iter()
        .map(|e| PositionEdit {
            range: index.position_range(text, &e.range),
            text: e.text.clone(),
        })
        .collect()
}

/// The edits that undo `edits` when applied to the **result** of
/// [`apply_edits`]. Ranges are in the new text.
pub fn invert_edits(text: &str, edits: &[TextEdit]) -> Vec<TextEdit> {
    let mut delta: isize = 0;
    let mut out = Vec::with_capacity(edits.len());
    for e in edits {
        let start = (e.range.start as isize + delta) as usize;
        out.push(TextEdit {
            range: start..start + e.text.len(),
            text: text[e.range.clone()].to_string(),
        });
        delta += e.text.len() as isize - e.range.len() as isize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_validate() {
        let text = "hello world";
        let edits = vec![TextEdit::new(0..5, "hi"), TextEdit::new(6..11, "there")];
        assert_eq!(apply_edits(text, &edits).unwrap(), "hi there");
        assert!(validate_edits(text, &edits).is_ok());
        assert_eq!(
            validate_edits(text, &[TextEdit::new(0..5, "x"), TextEdit::new(4..6, "y")]),
            Err(EditError::Overlap(0..5, 4..6))
        );
        assert!(matches!(
            validate_edits(text, &[TextEdit::new(0..100, "x")]),
            Err(EditError::OutOfRange(_))
        ));
        // A range that splits a UTF-8 char.
        assert!(matches!(
            validate_edits("é", &[TextEdit::new(0..1, "x")]),
            Err(EditError::OutOfRange(_))
        ));
    }

    #[test]
    fn invert_round_trips() {
        let text = "one two three";
        let edits = vec![
            TextEdit::new(0..3, "1"),
            TextEdit::new(4..7, "22"),
            TextEdit::new(8..13, ""),
        ];
        let applied = apply_edits(text, &edits).unwrap();
        let inverse = invert_edits(text, &edits);
        assert_eq!(apply_edits(&applied, &inverse).unwrap(), text);
    }

    #[test]
    fn position_edits_use_utf16() {
        let text = "😀 a\nb";
        let edits = vec![TextEdit::new(5..6, "x")];
        let position = to_position_edits(text, &edits);
        assert_eq!(position[0].range.start, crate::Position::new(0, 3));
        assert_eq!(position[0].range.end, crate::Position::new(0, 4));
        assert_eq!(position[0].text, "x");
    }
}
