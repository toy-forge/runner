//! Text selection and copy over the (filtered) console lines.
//!
//! Positions are `(LineId, byte offset)`, so a selection survives appends and
//! eviction (endpoints in evicted lines clamp to the first retained line). All
//! operations work on the *visible* lines: copying with a filter active copies
//! what the user sees.

use std::ops::Range;

use crate::console::Console;
use crate::line::LineId;
use crate::textutil::{floor_char_boundary, word_range};

/// A position in the console text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Pos {
    /// The line.
    pub line: LineId,
    /// Byte offset in the line text (clamped to a char boundary when used).
    pub offset: usize,
}

impl Pos {
    /// Offset `offset` of `line`.
    pub fn new(line: LineId, offset: usize) -> Pos {
        Pos { line, offset }
    }
}

/// An anchor and a head (the head moves while dragging).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConsoleSelection {
    /// Where the selection started.
    pub anchor: Pos,
    /// Where it currently ends.
    pub head: Pos,
}

impl ConsoleSelection {
    /// An empty selection at `pos`.
    pub fn caret(pos: Pos) -> ConsoleSelection {
        ConsoleSelection { anchor: pos, head: pos }
    }

    /// True when anchor and head coincide.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in document order.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The selection with the head moved to `head`.
    pub fn extend_to(self, head: Pos) -> ConsoleSelection {
        ConsoleSelection { anchor: self.anchor, head }
    }
}

/// The part of one line a selection covers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LineSelection {
    /// Selected byte range in the line.
    pub range: Range<usize>,
    /// The selection continues past the end of the line (draw the highlight
    /// to the row's edge / include the newline).
    pub to_end_of_line: bool,
}

impl Console {
    /// The selected text of the visible lines, lines joined with `\n` (soft
    /// wrapped continuations are joined without one).
    pub fn selection_text(&self, sel: &ConsoleSelection) -> String {
        let (a, b) = sel.ordered();
        let start = self.visible_lower_bound(a.line);
        let end = self.visible_lower_bound(LineId(b.line.0.saturating_add(1)));
        let mut out = String::new();
        let mut prev: Option<LineId> = None;
        for i in start..end {
            let Some(v) = self.visible_line(i) else { break };
            let t = v.text();
            let from = if v.id() == a.line { floor_char_boundary(t, a.offset) } else { 0 };
            let to = if v.id() == b.line { floor_char_boundary(t, b.offset) } else { t.len() };
            if let Some(p) = prev && !(v.is_continued() && p.0 + 1 == v.id().0) {
                out.push('\n');
            }
            if to > from {
                out.push_str(&t[from..to]);
            }
            prev = Some(v.id());
        }
        out
    }

    /// The part of line `line` that `sel` covers, if any.
    pub fn selection_in_line(&self, sel: &ConsoleSelection, line: LineId) -> Option<LineSelection> {
        let (a, b) = sel.ordered();
        if line < a.line || line > b.line {
            return None;
        }
        let v = self.line_by_id(line)?;
        let t = v.text();
        let start = if line == a.line { floor_char_boundary(t, a.offset) } else { 0 };
        let end = if line == b.line { floor_char_boundary(t, b.offset) } else { t.len() };
        let to_eol = line != b.line;
        if start >= end && !to_eol {
            return None;
        }
        Some(LineSelection {
            range: start..end.max(start),
            to_end_of_line: to_eol,
        })
    }

    /// A selection of all visible text.
    pub fn select_all(&self) -> Option<ConsoleSelection> {
        let n = self.visible_count();
        let first = self.visible_line(0)?;
        let last = self.visible_line(n - 1)?;
        Some(ConsoleSelection {
            anchor: Pos::new(first.id(), 0),
            head: Pos::new(last.id(), last.len()),
        })
    }

    /// All visible text, one line per row.
    pub fn visible_text(&self) -> String {
        self.select_all().map(|s| self.selection_text(&s)).unwrap_or_default()
    }

    /// The word around `pos` (double click).
    pub fn word_selection(&self, pos: Pos) -> Option<ConsoleSelection> {
        let v = self.line_by_id(pos.line)?;
        let r = word_range(v.text(), pos.offset);
        Some(ConsoleSelection {
            anchor: Pos::new(pos.line, r.start),
            head: Pos::new(pos.line, r.end),
        })
    }

    /// The whole line of `pos` (triple click), including soft-wrapped
    /// continuation lines.
    pub fn line_selection(&self, pos: Pos) -> Option<ConsoleSelection> {
        let v = self.line_by_id(pos.line)?;
        let mut first = v.id();
        let mut cur_continued = v.is_continued();
        while cur_continued {
            let Some(p) = self.line_by_id(LineId(first.0.wrapping_sub(1))) else { break };
            first = p.id();
            cur_continued = p.is_continued();
        }
        let mut last = self.line_by_id(pos.line)?;
        while let Some(n) = self.line_by_id(LineId(last.id().0 + 1)) {
            if !n.is_continued() {
                break;
            }
            last = n;
        }
        Some(ConsoleSelection {
            anchor: Pos::new(first, 0),
            head: Pos::new(last.id(), last.len()),
        })
    }

    /// The selection with endpoints in evicted lines moved to the first
    /// retained line.
    pub fn clamp_selection(&self, sel: &ConsoleSelection) -> ConsoleSelection {
        let first = self.first_id();
        let fix = |p: Pos| if p.line < first { Pos::new(first, 0) } else { p };
        ConsoleSelection {
            anchor: fix(sel.anchor),
            head: fix(sel.head),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::ConsoleConfig;
    use crate::filter::{Filter, OutputFilter};
    use crate::tag::Stream;

    fn sample() -> Console {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "hello world\nsecond line\nthird\n");
        c
    }

    #[test]
    fn copy_across_lines() {
        let c = sample();
        let sel = ConsoleSelection {
            anchor: Pos::new(LineId(0), 6),
            head: Pos::new(LineId(2), 3),
        };
        assert_eq!(c.selection_text(&sel), "world\nsecond line\nthi");
        // Reversed selection is the same.
        let rev = ConsoleSelection {
            anchor: sel.head,
            head: sel.anchor,
        };
        assert_eq!(c.selection_text(&rev), c.selection_text(&sel));
    }

    #[test]
    fn select_all_and_text() {
        let c = sample();
        assert_eq!(c.visible_text(), "hello world\nsecond line\nthird");
        assert!(Console::default().select_all().is_none());
        assert_eq!(Console::default().visible_text(), "");
    }

    #[test]
    fn per_line_ranges() {
        let c = sample();
        let sel = ConsoleSelection {
            anchor: Pos::new(LineId(0), 6),
            head: Pos::new(LineId(2), 3),
        };
        assert_eq!(
            c.selection_in_line(&sel, LineId(0)),
            Some(LineSelection {
                range: 6..11,
                to_end_of_line: true
            })
        );
        assert_eq!(c.selection_in_line(&sel, LineId(1)).unwrap().range, 0..11);
        assert_eq!(
            c.selection_in_line(&sel, LineId(2)),
            Some(LineSelection {
                range: 0..3,
                to_end_of_line: false
            })
        );
        assert_eq!(c.selection_in_line(&sel, LineId(3)), None);
        assert_eq!(c.selection_in_line(&ConsoleSelection::caret(Pos::new(LineId(0), 2)), LineId(0)), None);
    }

    #[test]
    fn word_and_line_selection() {
        let c = sample();
        let w = c.word_selection(Pos::new(LineId(0), 8)).unwrap();
        assert_eq!(c.selection_text(&w), "world");
        let l = c.line_selection(Pos::new(LineId(1), 3)).unwrap();
        assert_eq!(c.selection_text(&l), "second line");
    }

    #[test]
    fn soft_wrapped_lines_copy_without_newline() {
        let mut c = Console::new(ConsoleConfig {
            max_line_chars: 16,
            ..Default::default()
        });
        c.append_str(Stream::Stdout, &format!("{}\nnext\n", "a".repeat(20)));
        let all = c.visible_text();
        assert_eq!(all, format!("{}\nnext", "a".repeat(20)));
        let l = c.line_selection(Pos::new(LineId(1), 0)).unwrap();
        assert_eq!(c.selection_text(&l), "a".repeat(20));
    }

    #[test]
    fn copy_respects_filter_and_eviction() {
        let mut c = Console::new(ConsoleConfig {
            max_lines: 3,
            ..Default::default()
        });
        c.append_str(Stream::Stdout, "o1\n");
        c.append_str(Stream::Stderr, "e1\n");
        c.append_str(Stream::Stdout, "o2\n");
        c.append_str(Stream::Stderr, "e2\n");
        c.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
        assert_eq!(c.visible_text(), "e1\ne2");
        let old = ConsoleSelection {
            anchor: Pos::new(LineId(0), 1),
            head: Pos::new(LineId(3), 1),
        };
        assert_eq!(c.selection_text(&old), "e1\ne");
        assert_eq!(c.clamp_selection(&old).anchor, Pos::new(LineId(1), 0));
    }

    #[test]
    fn offsets_inside_chars_clamp() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "a\u{4e2d}b\n");
        let sel = ConsoleSelection {
            anchor: Pos::new(LineId(0), 2),
            head: Pos::new(LineId(0), 5),
        };
        assert_eq!(c.selection_text(&sel), "\u{4e2d}b");
    }
}
