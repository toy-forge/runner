//! Back/Forward navigation history of a window.
//!
//! CodeRunner keeps, per window, a back stack and a forward stack of
//! *locations*: a caret position in a document. Jumps (Go to Line, a symbol,
//! a search result, a diagnostic) and switches between documents record the
//! place left behind; **Go Back** (⌃⌘←) returns there and puts the place it
//! came from on the forward stack, **Go Forward** (⌃⌘→) undoes that. A new
//! jump clears the forward stack. Entries of a closed document are dropped.
//!
//! The menu and toolbar tooltips are derived from the entry about to be
//! visited ([`NavigationHistory::back_title`]): `Go Back to Previous Caret
//! Location`, `Go Back to Search Results` or `Go Back to Tab or Document "name"`.
//!
//! ```
//! use scratch_document::{DocumentId, NavigationEntry, NavigationHistory, Position};
//!
//! let doc = DocumentId::new();
//! let mut history = NavigationHistory::default();
//! let before = NavigationEntry::caret(doc, "a.py", Position::new(3, 0));
//! let after = NavigationEntry::caret(doc, "a.py", Position::new(40, 0));
//! history.record_jump(before.clone());            // about to jump from line 3 ...
//! assert_eq!(history.go_back(after.clone()), Some(before)); // Back returns to line 3
//! assert_eq!(history.go_forward(NavigationEntry::caret(doc, "a.py", Position::new(3, 0))), Some(after));
//! ```

use serde::{Deserialize, Serialize};

use crate::DocumentId;
use crate::state::Position;

/// What a history entry stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationKind {
    /// A caret location in a document.
    #[default]
    Caret,
    /// The search results of the sidebar.
    SearchResults,
}

/// A place the user was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationEntry {
    /// The document.
    pub document: DocumentId,
    /// The document's name, for titles (kept so closed-then-reopened names
    /// stay readable).
    pub title: String,
    /// The caret.
    pub position: Position,
    /// What the entry is.
    pub kind: NavigationKind,
}

impl NavigationEntry {
    /// A caret location.
    pub fn caret(document: DocumentId, title: impl Into<String>, position: Position) -> Self {
        Self {
            document,
            title: title.into(),
            position,
            kind: NavigationKind::Caret,
        }
    }

    /// The search results view (`document` is the document that was active).
    pub fn search_results(
        document: DocumentId,
        title: impl Into<String>,
        position: Position,
    ) -> Self {
        Self {
            document,
            title: title.into(),
            position,
            kind: NavigationKind::SearchResults,
        }
    }

    fn same_place(&self, other: &Self) -> bool {
        self.document == other.document
            && self.kind == other.kind
            && self.position.line == other.position.line
    }
}

/// The default number of entries kept per stack.
pub const DEFAULT_HISTORY_LIMIT: usize = 200;

/// The back and forward stacks of one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NavigationHistory {
    back: Vec<NavigationEntry>,
    forward: Vec<NavigationEntry>,
    limit: usize,
}

impl Default for NavigationHistory {
    fn default() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            limit: DEFAULT_HISTORY_LIMIT,
        }
    }
}

impl NavigationHistory {
    /// A history keeping at most `limit` entries per stack.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            ..Self::default()
        }
    }

    /// Records the place the user is leaving, just before a jump or a switch
    /// to another document. Consecutive entries for the same line are merged,
    /// and the forward stack is cleared.
    pub fn record_jump(&mut self, from: NavigationEntry) {
        self.forward.clear();
        if self.back.last().is_some_and(|last| last.same_place(&from)) {
            if let Some(last) = self.back.last_mut() {
                *last = from; // keep the latest column
            }
            return;
        }
        self.back.push(from);
        if self.back.len() > self.limit {
            self.back.remove(0);
        }
    }

    /// Goes back from `current` (where the user is now). Returns the entry to
    /// visit, or `None` when there is none. `current` moves to the forward
    /// stack.
    pub fn go_back(&mut self, current: NavigationEntry) -> Option<NavigationEntry> {
        let target = self.back.pop()?;
        Self::push_limited(&mut self.forward, current, self.limit);
        Some(target)
    }

    /// Goes forward from `current`; the mirror of [`NavigationHistory::go_back`].
    pub fn go_forward(&mut self, current: NavigationEntry) -> Option<NavigationEntry> {
        let target = self.forward.pop()?;
        Self::push_limited(&mut self.back, current, self.limit);
        Some(target)
    }

    fn push_limited(stack: &mut Vec<NavigationEntry>, entry: NavigationEntry, limit: usize) {
        if stack.last().is_some_and(|last| last.same_place(&entry)) {
            return;
        }
        stack.push(entry);
        if stack.len() > limit {
            stack.remove(0);
        }
    }

    /// Whether Go Back is available (enables the toolbar segment and menu item).
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether Go Forward is available.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The entry Go Back would visit.
    pub fn peek_back(&self) -> Option<&NavigationEntry> {
        self.back.last()
    }

    /// The entry Go Forward would visit.
    pub fn peek_forward(&self) -> Option<&NavigationEntry> {
        self.forward.last()
    }

    /// The title of the Go Back menu item and tooltip, given the document
    /// that is active now.
    pub fn back_title(&self, active: DocumentId) -> String {
        title(
            "Go Back",
            "Previous Caret Location",
            self.peek_back(),
            active,
        )
    }

    /// The title of the Go Forward menu item and tooltip.
    pub fn forward_title(&self, active: DocumentId) -> String {
        title(
            "Go Forward",
            "Next Caret Location",
            self.peek_forward(),
            active,
        )
    }

    /// Drops every entry of a closed document.
    pub fn forget_document(&mut self, id: DocumentId) {
        self.back.retain(|e| e.document != id);
        self.forward.retain(|e| e.document != id);
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }

    /// The back stack, oldest first.
    pub fn back_entries(&self) -> &[NavigationEntry] {
        &self.back
    }

    /// The forward stack, next to visit last.
    pub fn forward_entries(&self) -> &[NavigationEntry] {
        &self.forward
    }
}

fn title(
    verb: &str,
    caret_phrase: &str,
    entry: Option<&NavigationEntry>,
    active: DocumentId,
) -> String {
    match entry {
        None => verb.to_owned(),
        Some(e) if e.kind == NavigationKind::SearchResults => format!("{verb} to Search Results"),
        Some(e) if e.document == active => format!("{verb} to {caret_phrase}"),
        Some(e) => format!("{verb} to Tab or Document \"{}\"", e.title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(doc: DocumentId, line: u32) -> NavigationEntry {
        NavigationEntry::caret(doc, "a.py", Position::new(line, 0))
    }

    #[test]
    fn back_and_forward_walk_the_jumps() {
        let doc = DocumentId::new();
        let mut h = NavigationHistory::default();
        assert!(!h.can_go_back() && !h.can_go_forward());
        h.record_jump(at(doc, 1)); // jump 1 -> 10
        h.record_jump(at(doc, 10)); // jump 10 -> 50
        assert!(h.can_go_back());
        assert_eq!(h.go_back(at(doc, 50)), Some(at(doc, 10)));
        assert_eq!(h.go_back(at(doc, 10)), Some(at(doc, 1)));
        assert_eq!(h.go_back(at(doc, 1)), None);
        assert_eq!(h.go_forward(at(doc, 1)), Some(at(doc, 10)));
        assert_eq!(h.go_forward(at(doc, 10)), Some(at(doc, 50)));
        assert_eq!(h.go_forward(at(doc, 50)), None);
        // and back again
        assert_eq!(h.go_back(at(doc, 50)), Some(at(doc, 10)));
    }

    #[test]
    fn a_new_jump_clears_forward() {
        let doc = DocumentId::new();
        let mut h = NavigationHistory::default();
        h.record_jump(at(doc, 1));
        h.go_back(at(doc, 20));
        assert!(h.can_go_forward());
        h.record_jump(at(doc, 1));
        assert!(!h.can_go_forward());
    }

    #[test]
    fn jumps_within_one_line_are_merged() {
        let doc = DocumentId::new();
        let mut h = NavigationHistory::default();
        h.record_jump(NavigationEntry::caret(doc, "a.py", Position::new(5, 1)));
        h.record_jump(NavigationEntry::caret(doc, "a.py", Position::new(5, 9)));
        assert_eq!(h.back_entries().len(), 1);
        assert_eq!(
            h.peek_back().unwrap().position,
            Position::new(5, 9),
            "latest column kept"
        );
        h.record_jump(at(doc, 6));
        assert_eq!(h.back_entries().len(), 2);
    }

    #[test]
    fn switching_documents_is_an_entry_too() {
        let (a, b) = (DocumentId::new(), DocumentId::new());
        let mut h = NavigationHistory::default();
        h.record_jump(NavigationEntry::caret(a, "a.py", Position::new(3, 0)));
        let target = h
            .go_back(NavigationEntry::caret(b, "b.py", Position::new(0, 0)))
            .unwrap();
        assert_eq!(target.document, a);
        assert_eq!(h.peek_forward().unwrap().document, b);
    }

    #[test]
    fn titles_follow_codrunner() {
        let (a, b) = (DocumentId::new(), DocumentId::new());
        let mut h = NavigationHistory::default();
        assert_eq!(h.back_title(a), "Go Back");
        assert_eq!(h.forward_title(a), "Go Forward");
        h.record_jump(NavigationEntry::caret(a, "a.py", Position::new(3, 0)));
        assert_eq!(h.back_title(a), "Go Back to Previous Caret Location");
        assert_eq!(h.back_title(b), "Go Back to Tab or Document \"a.py\"");
        h.record_jump(NavigationEntry::search_results(
            a,
            "a.py",
            Position::new(9, 0),
        ));
        assert_eq!(h.back_title(a), "Go Back to Search Results");
        h.go_back(at(a, 1));
        assert_eq!(h.forward_title(a), "Go Forward to Next Caret Location");
        assert_eq!(h.forward_title(b), "Go Forward to Tab or Document \"a.py\"");
    }

    #[test]
    fn closing_a_document_prunes_both_stacks() {
        let (a, b) = (DocumentId::new(), DocumentId::new());
        let mut h = NavigationHistory::default();
        h.record_jump(at(a, 1));
        h.record_jump(at(b, 2));
        h.record_jump(at(a, 3));
        h.go_back(at(b, 9));
        h.forget_document(a);
        assert!(h.back_entries().iter().all(|e| e.document == b));
        assert!(h.forward_entries().iter().all(|e| e.document == b));
    }

    #[test]
    fn stacks_are_bounded() {
        let doc = DocumentId::new();
        let mut h = NavigationHistory::with_limit(3);
        for line in 0..10 {
            h.record_jump(at(doc, line));
        }
        let lines: Vec<u32> = h.back_entries().iter().map(|e| e.position.line).collect();
        assert_eq!(lines, [7, 8, 9]);
    }

    #[test]
    fn serializes_for_the_session() {
        let doc = DocumentId::new();
        let mut h = NavigationHistory::default();
        h.record_jump(at(doc, 4));
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<NavigationHistory>(&json).unwrap(), h);
        assert_eq!(
            serde_json::from_str::<NavigationHistory>("{}").unwrap(),
            NavigationHistory::default()
        );
    }
}
