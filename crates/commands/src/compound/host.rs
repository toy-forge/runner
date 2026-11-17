//! The [`Host`] trait: everything the compound-command interpreter needs from an editor.

use std::ops::Range;

use super::ast::Arg;
use crate::shortcut::{Key, Stroke};

/// Which end of a selection moves when it is extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    /// The start (the anchor is the end).
    Start,
    /// The end (the anchor is the start).
    #[default]
    End,
}

/// A selection in the document, `start <= end`, in byte offsets on character boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    /// Start offset.
    pub start: usize,
    /// End offset (exclusive).
    pub end: usize,
    /// The moving edge.
    pub edge: Edge,
}

impl Selection {
    /// A selection of `start..end` (ordered automatically), moving edge at the end.
    pub fn new(start: usize, end: usize) -> Selection {
        Selection { start: start.min(end), end: start.max(end), edge: Edge::End }
    }

    /// An empty selection (a caret).
    pub fn caret(at: usize) -> Selection {
        Selection { start: at, end: at, edge: Edge::End }
    }

    /// Whether the selection is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The selected range.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// The offset of the moving edge.
    pub fn moving_offset(&self) -> usize {
        match self.edge {
            Edge::Start => self.start,
            Edge::End => self.end,
        }
    }
}

/// A key press delivered to a waiting compound command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    /// The stroke.
    pub stroke: Stroke,
    /// The character the key types, if it types one.
    pub character: Option<char>,
}

impl KeyInput {
    /// Input for a stroke; the typed character is derived from it.
    pub fn from_stroke(stroke: Stroke) -> KeyInput {
        KeyInput { stroke, character: stroke.typed_char() }
    }

    /// Input for typing the character `c` (`'G'` is Shift+g, `'\r'` is Return).
    pub fn typed(c: char) -> KeyInput {
        let (key, shift) = Key::from_char(c);
        let mut stroke = Stroke::plain(key);
        stroke.modifiers.shift = shift;
        KeyInput { stroke, character: Some(if c == '\r' { '\n' } else { c }) }
    }
}

/// Result of a host-executed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The command ran.
    Done,
    /// The command ran but failed or did not apply (acts like a failed test).
    Failed,
    /// The host does not know the command (aborts the compound command with an error).
    Unknown,
}

/// A menu item as seen by `selectMenuItem:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMenuItem {
    /// Menu path without `MainMenu/`: `Find/Find Mode/Regular Expression`.
    pub path: String,
    /// Whether the item can be chosen now.
    pub enabled: bool,
}

/// Everything the interpreter needs from the editor it runs against.
///
/// All offsets are byte offsets into the document text on character boundaries. Only the
/// required methods must be implemented; the defaults suit simple hosts and tests.
pub trait Host {
    /// A saved document state (text and selection). Taken at the start of every command
    /// list, so keep it cheap (a rope clone, an undo-group marker).
    type Snapshot;

    /// Captures the state to return to when a command list fails.
    fn snapshot(&mut self) -> Self::Snapshot;
    /// Returns to a captured state.
    fn restore(&mut self, snapshot: Self::Snapshot);

    /// Length of the document in bytes.
    fn text_len(&self) -> usize;
    /// The text in `range`.
    fn text(&self, range: Range<usize>) -> String;
    /// The (primary) selection.
    fn selection(&self) -> Selection;
    /// Replaces the selection.
    fn set_selection(&mut self, selection: Selection);
    /// Replaces the selected text with `text` as if typed; the caret ends after it.
    fn insert_text(&mut self, text: &str);
    /// A counter that grows with every document edit (`testDocumentEditedSinceSelectionWithName:`).
    fn revision(&self) -> u64;

    /// Number of selections (more than one means multiple selections).
    fn selection_count(&self) -> usize {
        1
    }
    /// Whether typing edits the document (false in Vim normal mode).
    fn is_editable(&self) -> bool {
        true
    }
    /// Switches editing on or off (`makeEditable` / `makeNotEditable`).
    fn set_editable(&mut self, _editable: bool) {}
    /// Whether this is a source-code editor (as opposed to a plain text field).
    fn is_source_editor(&self) -> bool {
        true
    }
    /// The pasteboard text.
    fn clipboard(&self) -> String {
        String::new()
    }
    /// Sets the pasteboard text.
    fn set_clipboard(&mut self, _text: &str) {}

    /// The range of the paragraph (logical line, including its newline) containing `offset`.
    /// The default scans the whole text; real editors should override.
    fn paragraph_range(&self, offset: usize) -> Range<usize> {
        let all = self.text(0..self.text_len());
        let offset = offset.min(all.len());
        let start = all[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = all[offset..].find('\n').map_or(all.len(), |i| offset + i + 1);
        start..end
    }
    /// The 0-based line containing `offset`.
    fn line_of_offset(&self, offset: usize) -> usize {
        let all = self.text(0..self.text_len());
        let offset = offset.min(all.len());
        all[..offset].bytes().filter(|b| *b == b'\n').count()
    }
    /// The offset where the 0-based `line` starts (`None` past the end).
    fn offset_of_line(&self, line: usize) -> Option<usize> {
        let all = self.text(0..self.text_len());
        if line == 0 {
            return Some(0);
        }
        all.match_indices('\n').nth(line - 1).map(|(i, _)| i + 1).filter(|&o| o <= all.len())
    }

    /// Whether the TextMate scope selector matches at `offset` (`testSelectedScope:` ...).
    fn scope_matches(&self, _selector: &str, _offset: usize) -> bool {
        false
    }
    /// `testActiveSyntax:`: whether the syntax at the caret matches the top-level scope
    /// selector and the caret is not inside a string or comment.
    fn active_syntax_matches(&self, selector: &str) -> bool {
        self.scope_matches(selector, self.selection().start)
    }

    /// The menu bar's items (static and dynamic) for `selectMenuItem:`.
    fn menu_items(&self) -> Vec<HostMenuItem> {
        Vec::new()
    }
    /// Chooses the menu item with the exact `path` returned by [`Host::menu_items`].
    fn perform_menu_item(&mut self, _path: &str) -> bool {
        false
    }

    /// Runs an editor command the interpreter does not implement itself
    /// (`moveLeft`, `balanceDelimiters`, `undo`, `testIsSourceEditor` ...).
    /// `name` is the command name as written, including colons.
    fn execute(&mut self, _name: &str, _args: &[Arg]) -> CommandOutcome {
        CommandOutcome::Unknown
    }
    /// `beep`.
    fn beep(&mut self) {}
    /// A message from the `debug` command machinery.
    fn debug(&mut self, _message: &str) {}
}
