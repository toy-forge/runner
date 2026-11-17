//! History for the debugger REPL line in the console.
//!
//! Up/down arrows browse previously typed debugger commands, exactly like
//! CodeRunner's console (`getHistoricalUserCommandOlderCommand:`).  The
//! session sends the command text verbatim to the debugger; only the editing
//! state lives here.

/// Bounded command history with a browse cursor.
#[derive(Debug, Clone)]
pub struct ReplHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    limit: usize,
}

impl Default for ReplHistory {
    fn default() -> Self {
        Self::new(100)
    }
}

impl ReplHistory {
    /// New history with a maximum number of entries.
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            cursor: None,
            limit: limit.max(1),
        }
    }

    /// Push a command.  Empty commands and immediate duplicates are ignored.
    pub fn push(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        if self.entries.last().is_some_and(|last| last == command) {
            self.cursor = None;
            return;
        }
        self.entries.push(command.to_string());
        if self.entries.len() > self.limit {
            let overflow = self.entries.len() - self.limit;
            self.entries.drain(0..overflow);
        }
        self.cursor = None;
    }

    /// All entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing was typed yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Move to the previous (older) entry, returning it.  Repeated calls walk
    /// further back; at the oldest entry the result stays there.
    pub fn older(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let index = match self.cursor {
            None => self.entries.len() - 1,
            Some(0) => 0,
            Some(index) => index - 1,
        };
        self.cursor = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    /// Move to the next (newer) entry.  Past the newest entry the cursor
    /// clears and `None` is returned (the line becomes empty again).
    pub fn newer(&mut self) -> Option<&str> {
        let index = self.cursor?;
        if index + 1 >= self.entries.len() {
            self.cursor = None;
            return None;
        }
        self.cursor = Some(index + 1);
        self.entries.get(index + 1).map(String::as_str)
    }

    /// Forget the browse cursor (e.g. after pressing Return).
    pub fn reset_cursor(&mut self) {
        self.cursor = None;
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browses_oldest_to_newest() {
        let mut history = ReplHistory::new(10);
        history.push("bt");
        history.push("frame variable");
        assert_eq!(history.older(), Some("frame variable"));
        assert_eq!(history.older(), Some("bt"));
        assert_eq!(history.older(), Some("bt"));
        assert_eq!(history.newer(), Some("frame variable"));
        assert_eq!(history.newer(), None);
        assert_eq!(history.older(), Some("frame variable"));
    }

    #[test]
    fn drops_duplicates_and_respects_limit() {
        let mut history = ReplHistory::new(2);
        history.push("a");
        history.push("a");
        history.push("b");
        history.push("c");
        assert_eq!(history.entries(), ["b", "c"]);
    }

    #[test]
    fn ignores_empty_lines() {
        let mut history = ReplHistory::new(4);
        history.push("  ");
        history.push("");
        assert!(history.is_empty());
    }
}
