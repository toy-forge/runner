//! [`FakeHost`]: a [`Host`] over a plain `String`, for tests of compound commands and of code
//! that drives the interpreter. It implements a small set of editor commands (`moveLeft`,
//! `moveRight`, `deleteForward`, `insertNewline`, `balanceDelimiters` ...) and records
//! everything else.

use std::ops::Range;

use super::ast::Arg;
use super::host::{CommandOutcome, Edge, Host, HostMenuItem, Selection};

/// A test host.
#[derive(Debug, Clone, Default)]
pub struct FakeHost {
    text: String,
    selection: Selection,
    editable: bool,
    revision: u64,
    clipboard: String,
    /// `(selector, range)`: [`Host::scope_matches`] is true for `selector` at offsets in `range`.
    pub scopes: Vec<(String, Range<usize>)>,
    /// Menu items offered to `selectMenuItem:`.
    pub menu: Vec<HostMenuItem>,
    /// Paths chosen through `selectMenuItem:`.
    pub performed_menu_items: Vec<String>,
    /// Names of commands the fake does not implement, in the order they were executed.
    pub executed: Vec<String>,
    /// Number of `beep` calls.
    pub beeps: usize,
    /// Number of selections reported by [`Host::selection_count`].
    pub selection_count: usize,
}

impl FakeHost {
    /// A host with `text`, caret at 0, editable.
    pub fn new(text: &str) -> FakeHost {
        FakeHost { text: text.to_owned(), editable: true, selection_count: 1, ..FakeHost::default() }
    }

    /// The document text.
    pub fn text_string(&self) -> &str {
        &self.text
    }

    /// The caret (moving edge) offset.
    pub fn caret(&self) -> usize {
        self.selection.moving_offset()
    }

    /// Places a caret.
    pub fn set_caret(&mut self, at: usize) {
        self.selection = Selection::caret(at);
    }

    /// The selection.
    pub fn sel(&self) -> Selection {
        self.selection
    }

    /// Sets the pasteboard.
    pub fn with_clipboard(mut self, text: &str) -> FakeHost {
        self.clipboard = text.to_owned();
        self
    }

    /// Adds a menu item.
    pub fn with_menu_item(mut self, path: &str, enabled: bool) -> FakeHost {
        self.menu.push(HostMenuItem { path: path.to_owned(), enabled });
        self
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }

    fn clamp_line_col(&self, line_range: Range<usize>, col: usize) -> usize {
        let line = &self.text[line_range.clone()];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let mut end = col.min(line.len());
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        line_range.start + end
    }

    fn line_move_up(&self, at: usize, col: usize) -> Option<usize> {
        let p = self.paragraph_range(at);
        if p.start == 0 {
            return None;
        }
        let q = self.paragraph_range(p.start - 1);
        Some(self.clamp_line_col(q, col))
    }

    fn line_move_down(&self, at: usize, col: usize) -> Option<usize> {
        let p = self.paragraph_range(at);
        if p.end >= self.text.len() {
            return None;
        }
        let q = self.paragraph_range(p.end);
        Some(self.clamp_line_col(q, col))
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn word_forward(&self, at: usize) -> usize {
        let start = at.min(self.text.len());
        for (rel, ch) in self.text[start..].char_indices() {
            if Self::is_word_char(ch) {
                let word_start = start + rel;
                for (wrel, wch) in self.text[word_start..].char_indices() {
                    if !Self::is_word_char(wch) {
                        return word_start + wrel;
                    }
                }
                return self.text.len();
            }
        }
        self.text.len()
    }

    fn word_backward(&self, at: usize) -> usize {
        let end = at.min(self.text.len());
        for (idx, ch) in self.text[..end].char_indices().rev() {
            if Self::is_word_char(ch) {
                let mut start = idx;
                for (widx, wch) in self.text[..idx].char_indices().rev() {
                    if !Self::is_word_char(wch) {
                        return start;
                    }
                    start = widx;
                }
                return 0;
            }
        }
        0
    }

    fn replace(&mut self, range: Range<usize>, with: &str) {
        self.text.replace_range(range, with);
        self.revision += 1;
    }

    fn modify(&mut self, to: impl Fn(&FakeHost, usize) -> usize) {
        let s = self.selection;
        let moved = to(self, s.moving_offset());
        let anchor = if s.edge == Edge::End { s.start } else { s.end };
        let mut n = Selection::new(anchor.min(moved), anchor.max(moved));
        n.edge = if moved < anchor { Edge::Start } else if moved > anchor { Edge::End } else { s.edge };
        self.selection = n;
    }

    fn matching(&self, at: usize) -> Option<(usize, usize)> {
        let bytes = self.text.as_bytes();
        for (open, close) in [(b'(', b')'), (b'[', b']'), (b'{', b'}')] {
            let mut depth = 0i32;
            let mut i = at;
            while i > 0 {
                i -= 1;
                if bytes[i] == close {
                    depth += 1;
                } else if bytes[i] == open {
                    if depth == 0 {
                        let mut d = 0i32;
                        for (j, &b) in bytes.iter().enumerate().skip(i + 1) {
                            if b == open {
                                d += 1;
                            } else if b == close {
                                if d == 0 {
                                    return (j >= at).then_some((i, j + 1));
                                }
                                d -= 1;
                            }
                        }
                        break;
                    }
                    depth -= 1;
                }
            }
        }
        None
    }
}

impl Host for FakeHost {
    type Snapshot = (String, Selection);

    fn snapshot(&mut self) -> Self::Snapshot {
        (self.text.clone(), self.selection)
    }

    fn restore(&mut self, (text, selection): Self::Snapshot) {
        if text != self.text {
            self.revision += 1;
        }
        self.text = text;
        self.selection = selection;
    }

    fn text_len(&self) -> usize {
        self.text.len()
    }

    fn text(&self, range: Range<usize>) -> String {
        self.text.get(range).unwrap_or_default().to_owned()
    }

    fn selection(&self) -> Selection {
        self.selection
    }

    fn set_selection(&mut self, selection: Selection) {
        let len = self.text.len();
        self.selection = Selection { start: selection.start.min(len), end: selection.end.min(len), edge: selection.edge };
    }

    fn insert_text(&mut self, text: &str) {
        let r = self.selection.range();
        self.replace(r.clone(), text);
        self.selection = Selection::caret(r.start + text.len());
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn selection_count(&self) -> usize {
        self.selection_count.max(1)
    }

    fn is_editable(&self) -> bool {
        self.editable
    }

    fn set_editable(&mut self, editable: bool) {
        self.editable = editable;
    }

    fn clipboard(&self) -> String {
        self.clipboard.clone()
    }

    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.to_owned();
    }

    fn scope_matches(&self, selector: &str, offset: usize) -> bool {
        self.scopes.iter().any(|(s, r)| s == selector && r.contains(&offset))
    }

    fn menu_items(&self) -> Vec<HostMenuItem> {
        self.menu.clone()
    }

    fn perform_menu_item(&mut self, path: &str) -> bool {
        self.performed_menu_items.push(path.to_owned());
        true
    }

    fn execute(&mut self, name: &str, _args: &[Arg]) -> CommandOutcome {
        let s = self.selection;
        match name {
            "moveLeft" => {
                let at = if s.is_empty() { self.prev(s.start) } else { s.start };
                self.selection = Selection::caret(at);
            }
            "moveRight" => {
                let at = if s.is_empty() { self.next(s.end) } else { s.end };
                self.selection = Selection::caret(at);
            }
            "moveLeftAndModifySelection" => self.modify(|h, at| h.prev(at)),
            "moveRightAndModifySelection" => self.modify(|h, at| h.next(at)),
            "moveToBeginningOfParagraph" => self.selection = Selection::caret(self.paragraph_range(s.start).start),
            "moveToBeginningOfParagraphAndModifySelection" => self.modify(|h, at| h.paragraph_range(at).start),
            "moveToEndOfParagraph" => {
                let p = self.paragraph_range(s.end);
                let end = if self.text[p.clone()].ends_with('\n') { p.end - 1 } else { p.end };
                self.selection = Selection::caret(end);
            }
            "moveToEndOfParagraphAndModifySelection" => {
                self.modify(|h, at| {
                    let p = h.paragraph_range(at);
                    if h.text[p.clone()].ends_with('\n') { p.end - 1 } else { p.end }
                });
            }
            "selectParagraph" => {
                let start = self.paragraph_range(s.start).start;
                let end = self.paragraph_range(s.end).end;
                self.selection = Selection::new(start, end);
            }
            "moveToBeginningOfDocument" => self.selection = Selection::caret(0),
            "moveToEndOfDocument" => self.selection = Selection::caret(self.text.len()),
            "moveUp" => {
                let col = s.start - self.paragraph_range(s.start).start;
                self.selection = Selection::caret(self.line_move_up(s.start, col).unwrap_or(0));
            }
            "moveDown" => {
                let col = s.end - self.paragraph_range(s.end).start;
                self.selection = Selection::caret(self.line_move_down(s.end, col).unwrap_or(self.text.len()));
            }
            "moveUpAndModifySelection" => {
                self.modify(|h, at| {
                    let col = at - h.paragraph_range(at).start;
                    h.line_move_up(at, col).unwrap_or(0)
                });
            }
            "moveDownAndModifySelection" => {
                self.modify(|h, at| {
                    let col = at - h.paragraph_range(at).start;
                    h.line_move_down(at, col).unwrap_or(h.text.len())
                });
            }
            "moveParagraphUp" => {
                let col = s.start - self.paragraph_range(s.start).start;
                let Some(pos) = self.line_move_up(s.start, col) else {
                    return CommandOutcome::Failed;
                };
                self.selection = Selection::caret(pos);
            }
            "moveParagraphDown" => {
                let col = s.end - self.paragraph_range(s.start).start;
                let Some(pos) = self.line_move_down(s.end, col) else {
                    return CommandOutcome::Failed;
                };
                self.selection = Selection::caret(pos);
            }
            "moveParagraphUpAndModifySelection" => {
                self.modify(|h, at| {
                    let col = at - h.paragraph_range(at).start;
                    h.line_move_up(at, col).unwrap_or(0)
                });
            }
            "moveParagraphDownAndModifySelection" => {
                self.modify(|h, at| {
                    let col = at - h.paragraph_range(at).start;
                    h.line_move_down(at, col).unwrap_or(h.text.len())
                });
            }
            "moveWordForward" | "moveWordRight" => self.selection = Selection::caret(self.word_forward(s.end)),
            "moveWordBackward" | "moveWordLeft" => self.selection = Selection::caret(self.word_backward(s.start)),
            "moveWordForwardAndModifySelection" | "moveWordRightAndModifySelection" => {
                self.modify(|h, at| h.word_forward(at));
            }
            "moveWordBackwardAndModifySelection" | "moveWordLeftAndModifySelection" => {
                self.modify(|h, at| h.word_backward(at));
            }
            "deleteForward" => {
                let r = if s.is_empty() { s.start..self.next(s.start) } else { s.range() };
                self.replace(r.clone(), "");
                self.selection = Selection::caret(r.start);
            }
            "deleteBackward" => {
                let r = if s.is_empty() { self.prev(s.start)..s.start } else { s.range() };
                self.replace(r.clone(), "");
                self.selection = Selection::caret(r.start);
            }
            "insertNewline" => self.insert_text("\n"),
            "balanceDelimiters" => match self.matching(s.start) {
                Some((a, b)) => self.selection = Selection::new(a, b),
                None => return CommandOutcome::Failed,
            },
            "testSelectedCharacterIsBalanced" => {
                return if self.matching(s.start).is_some() { CommandOutcome::Done } else { CommandOutcome::Failed };
            }
            "unknownForTests" => return CommandOutcome::Unknown,
            other => self.executed.push(other.to_owned()),
        }
        CommandOutcome::Done
    }

    fn beep(&mut self) {
        self.beeps += 1;
    }
}
