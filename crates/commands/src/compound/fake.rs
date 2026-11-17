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
            "moveToBeginningOfDocument" => self.selection = Selection::caret(0),
            "moveToEndOfDocument" => self.selection = Selection::caret(self.text.len()),
            "moveParagraphDown" => {
                let p = self.paragraph_range(s.end);
                let col = s.end - self.paragraph_range(s.start).start;
                if p.end >= self.text.len() {
                    return CommandOutcome::Failed;
                }
                let q = self.paragraph_range(p.end);
                let line = &self.text[q.clone()];
                let line = line.strip_suffix('\n').unwrap_or(line);
                self.selection = Selection::caret(q.start + col.min(line.len()));
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
