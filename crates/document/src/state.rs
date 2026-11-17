//! Per-document state that is not text: where the caret is, what is folded,
//! the breakpoints, the per-file run settings.
//!
//! Positions follow the NSH editor: lines and columns count from 0 and a column
//! counts UTF-16 code units, so [`Selection`], [`ViewState`] and
//! [`FoldRange`] convert one to one to `EditorSelection`, `set_scroll_offset`
//! and `set_folds` without this crate depending on NSH.

use serde::{Deserialize, Serialize};

/// A position in the text.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Position {
    /// The line, from 0.
    pub line: u32,
    /// The UTF-16 code units before the position in its line.
    pub column: u32,
}

impl Position {
    /// Line `line`, column `column`.
    pub const fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A selection: from `anchor` (where it started) to `head` (where the caret
/// is). Equal ends are a caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Selection {
    /// Where the selection started.
    pub anchor: Position,
    /// Where the caret is.
    pub head: Position,
}

impl Selection {
    /// A caret at `position`.
    pub const fn caret(position: Position) -> Self {
        Self {
            anchor: position,
            head: position,
        }
    }

    /// From `anchor` to `head`.
    pub const fn new(anchor: Position, head: Position) -> Self {
        Self { anchor, head }
    }

    /// The selection is empty.
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
}

/// A range of lines that is folded: `start_line` stays visible, the lines
/// after it up to and including `end_line` are hidden. Both from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FoldRange {
    /// The line that shows the fold.
    pub start_line: u32,
    /// The last hidden line.
    pub end_line: u32,
}

/// The scroll position of an editor.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ScrollState {
    /// Horizontal offset in points.
    pub x: f32,
    /// Vertical offset in points.
    pub y: f32,
}

/// What the editor remembers about a document between sessions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    /// The selections; the first is the primary one. Empty means a caret at
    /// the start.
    pub selections: Vec<Selection>,
    /// The scroll offset.
    pub scroll: ScrollState,
    /// The folded ranges.
    pub folds: Vec<FoldRange>,
}

impl ViewState {
    /// The primary caret (head of the first selection), the start of the
    /// document when there is none.
    pub fn caret(&self) -> Position {
        self.selections
            .first()
            .map_or_else(Position::default, |s| s.head)
    }

    /// Sets a single caret.
    pub fn set_caret(&mut self, position: Position) {
        self.selections = vec![Selection::caret(position)];
    }
}

/// The position of UTF-16 offset `offset` in `text` (clamped to the end).
/// `text` is expected to use `\n` line endings.
pub fn position_at_utf16_offset(text: &str, offset: usize) -> Position {
    let (mut line, mut column, mut seen) = (0u32, 0u32, 0usize);
    for c in text.chars() {
        if seen >= offset {
            break;
        }
        let units = c.len_utf16();
        seen += units;
        if c == '\n' {
            line += 1;
            column = 0;
        } else {
            column += units as u32;
        }
    }
    Position { line, column }
}

/// The position of the UTF-8 byte `offset` in `text` (clamped to the end and
/// to a character boundary).
pub fn position_at_byte_offset(text: &str, offset: usize) -> Position {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let line = before.matches('\n').count() as u32;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .encode_utf16()
        .count() as u32;
    Position { line, column }
}

/// The UTF-16 offset of `position` in `text` (clamped to the line and the end).
pub fn utf16_offset_of(text: &str, position: Position) -> usize {
    let mut offset = 0usize;
    let mut lines = text.split('\n');
    for _ in 0..position.line {
        match lines.next() {
            Some(line) => offset += line.encode_utf16().count() + 1,
            None => return offset.saturating_sub(1),
        }
    }
    let line = lines.next().unwrap_or_default();
    offset + (position.column as usize).min(line.encode_utf16().count())
}

/// A breakpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Breakpoint {
    /// The line, from 0.
    pub line: u32,
    /// Installed in a debug session only when enabled (CodeRunner's disabled
    /// breakpoints keep their place but do not stop).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Hit only when this expression is true (CodeRunner's Edit Breakpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Removed when hit (`Stopped at Temporary Breakpoint`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub temporary: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

const fn yes() -> bool {
    true
}

impl Breakpoint {
    /// A permanent breakpoint on `line`.
    pub const fn at(line: u32) -> Self {
        Self {
            line,
            enabled: true,
            condition: None,
            temporary: false,
        }
    }
}

/// The breakpoints of a document, sorted by line with at most one per line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Breakpoints(Vec<Breakpoint>);

impl Breakpoints {
    /// No breakpoints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the set from any list (sorted, one per line; a permanent entry
    /// wins over a temporary one on the same line).
    pub fn from_list(list: impl IntoIterator<Item = Breakpoint>) -> Self {
        let mut all: Vec<Breakpoint> = list.into_iter().collect();
        all.sort_by_key(|b| (b.line, b.temporary));
        all.dedup_by_key(|b| b.line);
        Self(all)
    }

    /// The breakpoints in line order.
    pub fn as_slice(&self) -> &[Breakpoint] {
        &self.0
    }

    /// The number of breakpoints.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// There are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `line` has one.
    pub fn contains(&self, line: u32) -> bool {
        self.0.binary_search_by_key(&line, |b| b.line).is_ok()
    }

    /// The breakpoint on `line`, mutably (enable/disable, edit condition).
    pub fn get_mut(&mut self, line: u32) -> Option<&mut Breakpoint> {
        self.0
            .binary_search_by_key(&line, |b| b.line)
            .ok()
            .map(|index| &mut self.0[index])
    }

    /// Sets a breakpoint on `line` if there is none, else removes it. Returns
    /// whether the line has one afterwards (`Toggle Breakpoint at Current Line`).
    pub fn toggle(&mut self, line: u32) -> bool {
        match self.0.binary_search_by_key(&line, |b| b.line) {
            Ok(index) => {
                self.0.remove(index);
                false
            }
            Err(index) => {
                self.0.insert(index, Breakpoint::at(line));
                true
            }
        }
    }

    /// Sets a breakpoint (replacing one on the same line).
    pub fn set(&mut self, breakpoint: Breakpoint) {
        match self.0.binary_search_by_key(&breakpoint.line, |b| b.line) {
            Ok(index) => self.0[index] = breakpoint,
            Err(index) => self.0.insert(index, breakpoint),
        }
    }

    /// Removes the breakpoint on `line`; returns whether there was one.
    pub fn remove(&mut self, line: u32) -> bool {
        match self.0.binary_search_by_key(&line, |b| b.line) {
            Ok(index) => {
                self.0.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// Removes all (`Clear Breakpoints`).
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Keeps the breakpoints attached to their code across an edit: `removed`
    /// lines starting at `first_line` were replaced by `inserted` lines.
    /// Breakpoints before the edit stay, later ones move by the difference,
    /// and those on lines the edit made disappear are dropped. An edit that
    /// keeps the line count (typing inside a line) changes nothing.
    pub fn adjust_for_edit(&mut self, first_line: u32, removed: u32, inserted: u32) {
        if removed == inserted {
            return;
        }
        let kept_end = first_line.saturating_add(removed.min(inserted));
        let old_end = first_line.saturating_add(removed);
        let shift = i64::from(inserted) - i64::from(removed);
        let moved = self.0.iter().filter_map(|b| {
            if b.line < kept_end {
                Some(b.clone())
            } else if b.line < old_end {
                None
            } else {
                let line = u32::try_from((i64::from(b.line) + shift).max(0)).unwrap_or(u32::MAX);
                Some(Breakpoint {
                    line,
                    ..b.clone()
                })
            }
        });
        *self = Self::from_list(moved);
    }
}

/// The per-file run settings (what the Run Settings sheet edits). `None` and
/// empty mean "the language default".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunSettings {
    /// A run command specific to this file (`Run command is specific to this file.`).
    pub run_command: Option<String>,
    /// Command-line arguments, appended to the run command unexpanded.
    pub arguments: String,
    /// Compile flags specific to this file.
    pub compiler_flags: Option<String>,
    /// Program input, piped to stdin (and offered as `$CR_INPUT`).
    pub input: String,
    /// Variables added to the run's environment.
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    /// The input goes to the program as the file named by `$CR_INPUT`, not on stdin.
    #[serde(default)]
    pub input_via_environment: bool,
    /// The folder the program runs in.
    #[serde(default)]
    pub working_folder: RunFolder,
}

/// The folder a run starts in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RunFolder {
    /// The directory of the file.
    #[default]
    File,
    /// The user's home folder.
    Home,
    /// A folder the user chose, absolute.
    Custom(std::path::PathBuf),
}

impl RunSettings {
    /// Any setting differs from the default: the toolbar wrench is highlighted
    /// (`hasCustomRunSettings`).
    pub fn is_custom(&self) -> bool {
        self.run_command.is_some()
            || self.compiler_flags.is_some()
            || !self.arguments.is_empty()
            || !self.input.is_empty()
            || !self.environment.is_empty()
            || self.input_via_environment
            || self.working_folder != RunFolder::File
    }

    /// Back to the language defaults.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_toggle_and_stay_sorted() {
        let mut set = Breakpoints::new();
        assert!(set.toggle(10));
        assert!(set.toggle(2));
        assert!(set.toggle(5));
        assert_eq!(
            set.as_slice().iter().map(|b| b.line).collect::<Vec<_>>(),
            [2, 5, 10]
        );
        assert!(!set.toggle(5));
        assert!(set.contains(2) && !set.contains(5));
        assert!(set.remove(2) && !set.remove(2));
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn breakpoints_carry_enabled_and_condition() {
        let mut set = Breakpoints::new();
        set.toggle(4);
        let bp = set.get_mut(4).expect("breakpoint");
        assert!(bp.enabled && bp.condition.is_none());
        bp.enabled = false;
        bp.condition = Some("x > 1".into());
        let bp = &set.as_slice()[0];
        assert!(!bp.enabled && bp.condition.as_deref() == Some("x > 1"));
        // Edits keep the flags when the line survives the shift.
        set.adjust_for_edit(0, 0, 1);
        let bp = &set.as_slice()[0];
        assert_eq!(bp.line, 5);
        assert!(!bp.enabled && bp.condition.as_deref() == Some("x > 1"));
        // Serialization round-trips, defaulting old files to enabled.
        let json = serde_json::to_string(&set).unwrap();
        let back: Breakpoints = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_slice()[0].condition.as_deref(), Some("x > 1"));
        let old: Breakpoints = serde_json::from_str(r#"[{"line":2}]"#).unwrap();
        assert!(old.as_slice()[0].enabled);
    }

    #[test]
    fn from_list_dedups_by_line() {
        let set = Breakpoints::from_list([
            Breakpoint {
                line: 3,
                temporary: true,
                ..Breakpoint::at(0)
            },
            Breakpoint::at(3),
            Breakpoint::at(1),
        ]);
        assert_eq!(set.as_slice(), [Breakpoint::at(1), Breakpoint::at(3)]);
    }

    #[test]
    fn breakpoints_follow_inserted_and_deleted_lines() {
        let mut set = Breakpoints::from_list([2, 5, 9].map(Breakpoint::at));
        // two lines inserted at line 4
        set.adjust_for_edit(4, 0, 2);
        assert_eq!(
            set.as_slice().iter().map(|b| b.line).collect::<Vec<_>>(),
            [2, 7, 11]
        );
        // lines 6..=7 (two lines) removed: the breakpoint on 7 goes
        set.adjust_for_edit(6, 2, 0);
        assert_eq!(
            set.as_slice().iter().map(|b| b.line).collect::<Vec<_>>(),
            [2, 9]
        );
        // typing inside a line changes nothing
        set.adjust_for_edit(2, 1, 1);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn converts_between_utf16_offsets_and_positions() {
        let text = "ab\n😀x\nlast";
        assert_eq!(position_at_utf16_offset(text, 0), Position::new(0, 0));
        assert_eq!(position_at_utf16_offset(text, 2), Position::new(0, 2));
        assert_eq!(position_at_utf16_offset(text, 3), Position::new(1, 0));
        assert_eq!(position_at_utf16_offset(text, 5), Position::new(1, 2)); // after the emoji
        assert_eq!(position_at_utf16_offset(text, 1000), Position::new(2, 4));
        for position in [
            Position::new(0, 1),
            Position::new(1, 2),
            Position::new(2, 4),
        ] {
            let offset = utf16_offset_of(text, position);
            assert_eq!(position_at_utf16_offset(text, offset), position);
        }
        assert_eq!(utf16_offset_of(text, Position::new(0, 99)), 2);
    }

    #[test]
    fn byte_offsets_become_utf16_columns() {
        assert_eq!(position_at_byte_offset("a😀b\ncd", 5), Position::new(0, 3));
        assert_eq!(position_at_byte_offset("a😀b\ncd", 7), Position::new(1, 0));
        assert_eq!(position_at_byte_offset("a😀b\ncd", 8), Position::new(1, 1));
        assert_eq!(
            position_at_byte_offset("é", 1),
            Position::new(0, 0),
            "inside a character"
        );
        assert_eq!(position_at_byte_offset("ab", 99), Position::new(0, 2));
    }

    #[test]
    fn run_settings_report_custom() {
        let mut settings = RunSettings::default();
        assert!(!settings.is_custom());
        settings.arguments = "--fast".into();
        assert!(settings.is_custom());
        settings.reset();
        assert!(!settings.is_custom());
    }

    #[test]
    fn view_state_defaults_and_caret() {
        let mut view = ViewState::default();
        assert_eq!(view.caret(), Position::new(0, 0));
        view.set_caret(Position::new(3, 4));
        assert_eq!(view.caret(), Position::new(3, 4));
        assert!(view.selections[0].is_caret());
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<ViewState>(&json).unwrap(), view);
        assert_eq!(
            serde_json::from_str::<ViewState>("{}").unwrap(),
            ViewState::default()
        );
    }
}
