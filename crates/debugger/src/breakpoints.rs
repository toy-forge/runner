//! Breakpoints, persistence and line tracking across edits.
//!
//! A breakpoint has four presentation states beyond file/line:
//!
//! * **condition** - an expression the debugger evaluates before stopping;
//! * **temporary** - a one-shot breakpoint (CodeRunner's "temporary
//!   breakpoint", used by run-to-line); it is removed after the first hit;
//! * **disabled** - kept in the document's list but never installed;
//! * **ghost** - a UI hover preview only; never installed and never persisted
//!   by the session.
//!
//! [`BreakpointStore`] persists `file`, `line`, `condition`, `enabled` and the
//! temporary flag as TOML (or JSON for the per-document extended attribute).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::DebugError;

/// Identity of a breakpoint's effect at the debugger level: two breakpoints
/// with the same key are interchangeable and need not be re-sent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpointKey {
    /// Source file.
    pub file: PathBuf,
    /// One-based line.
    pub line: u32,
    /// Condition expression.
    pub condition: Option<String>,
    /// One-shot breakpoint.
    pub temporary: bool,
}

/// A breakpoint in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// Stable local id (assigned by [`BreakpointStore`]).
    pub id: u64,
    /// Source file.
    pub file: PathBuf,
    /// One-based line.
    pub line: u32,
    /// Optional condition expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Installed when true.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// One-shot breakpoint.
    #[serde(default)]
    pub temporary: bool,
    /// UI hover preview; never installed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ghost: bool,
    /// The debugger acknowledged the breakpoint.
    #[serde(default, skip_serializing_if = "is_false")]
    pub verified: bool,
    /// Debugger-side id (LLDB's "Breakpoint 1", pdb's id) once accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debugger_id: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Breakpoint {
    /// Create an enabled breakpoint.
    pub fn new(file: impl Into<PathBuf>, line: u32) -> Self {
        Self {
            id: 0,
            file: file.into(),
            line,
            condition: None,
            enabled: true,
            temporary: false,
            ghost: false,
            verified: false,
            debugger_id: None,
        }
    }

    /// Set a condition.
    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    /// Make this a one-shot breakpoint.
    pub fn temporary(mut self) -> Self {
        self.temporary = true;
        self
    }

    /// Mark the breakpoint disabled.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Mark the breakpoint as a UI ghost.
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }

    /// True when this breakpoint should be installed in the debugger.
    pub fn is_effective(&self) -> bool {
        self.enabled && !self.ghost && self.line > 0
    }

    /// The debugger-level identity used for diffing.
    pub fn key(&self) -> BreakpointKey {
        BreakpointKey {
            file: self.file.clone(),
            line: self.line,
            condition: self.condition.clone(),
            temporary: self.temporary,
        }
    }
}

/// Persisted breakpoint document.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BreakpointFile {
    #[serde(default = "current_version")]
    version: u32,
    #[serde(default)]
    breakpoints: Vec<Breakpoint>,
}

fn current_version() -> u32 {
    1
}

/// A collection of breakpoints with stable ids.
///
/// The store is a plain value: the session owns one while debugging and the
/// document layer can keep one per file for persistence.
#[derive(Debug, Clone, Default)]
pub struct BreakpointStore {
    breakpoints: Vec<Breakpoint>,
    next_id: u64,
}

impl BreakpointStore {
    /// Empty store.
    pub fn new() -> Self {
        Self {
            breakpoints: Vec::new(),
            next_id: 1,
        }
    }

    /// Build from existing breakpoints, preserving their ids.
    pub fn with_breakpoints(breakpoints: Vec<Breakpoint>) -> Self {
        let next_id = breakpoints.iter().map(|b| b.id).max().unwrap_or(0) + 1;
        Self {
            breakpoints,
            next_id,
        }
    }

    /// All breakpoints, including disabled and ghost ones.
    pub fn all(&self) -> &[Breakpoint] {
        &self.breakpoints
    }

    /// Breakpoints that should be installed.
    pub fn effective(&self) -> impl Iterator<Item = &Breakpoint> {
        self.breakpoints.iter().filter(|b| b.is_effective())
    }

    /// Breakpoints in one file.
    pub fn for_file(&self, file: &Path) -> impl Iterator<Item = &Breakpoint> {
        let file = file.to_path_buf();
        self.breakpoints.iter().filter(move |b| b.file == file)
    }

    /// Add a breakpoint, assigning an id.  Returns the id.
    pub fn add(&mut self, mut breakpoint: Breakpoint) -> u64 {
        if breakpoint.id == 0 {
            breakpoint.id = self.next_id;
            self.next_id += 1;
        } else {
            self.next_id = self.next_id.max(breakpoint.id + 1);
        }
        let id = breakpoint.id;
        self.breakpoints.push(breakpoint);
        id
    }

    /// Add a simple breakpoint at `file:line`.
    pub fn add_at(&mut self, file: impl Into<PathBuf>, line: u32) -> u64 {
        self.add(Breakpoint::new(file, line))
    }

    /// Remove by id.  Returns true when something was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.breakpoints.len();
        self.breakpoints.retain(|b| b.id != id);
        self.breakpoints.len() != before
    }

    /// Remove every breakpoint in a file, optionally at one line.
    pub fn clear(&mut self, file: &Path, line: Option<u32>) {
        self.breakpoints
            .retain(|b| b.file != file || line.is_some_and(|l| b.line != l));
    }

    /// Remove every breakpoint.
    pub fn clear_all(&mut self) {
        self.breakpoints.clear();
    }

    /// CodeRunner's "Toggle Breakpoint at Current Line": remove all
    /// breakpoints on a line, or add one when none exists.  Returns true when
    /// a breakpoint now exists.
    pub fn toggle(&mut self, file: &Path, line: u32) -> bool {
        let exists = self
            .breakpoints
            .iter()
            .any(|b| b.file == file && b.line == line && !b.ghost);
        if exists {
            self.breakpoints
                .retain(|b| !(b.file == file && b.line == line && !b.ghost));
            false
        } else {
            self.add_at(file.to_path_buf(), line);
            true
        }
    }

    /// Change the enabled flag.  Returns false when the id is unknown.
    pub fn set_enabled(&mut self, id: u64, enabled: bool) -> bool {
        match self.breakpoints.iter_mut().find(|b| b.id == id) {
            Some(bp) => {
                bp.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Replace a breakpoint's whole value (matched by id).
    pub fn update(&mut self, breakpoint: Breakpoint) -> bool {
        match self
            .breakpoints
            .iter_mut()
            .find(|b| b.id == breakpoint.id)
        {
            Some(slot) => {
                *slot = breakpoint;
                true
            }
            None => false,
        }
    }

    /// Find by id.
    pub fn get(&self, id: u64) -> Option<&Breakpoint> {
        self.breakpoints.iter().find(|b| b.id == id)
    }

    /// Find the first effective breakpoint at a location (used to label a
    /// breakpoint hit and to consume temporary breakpoints).
    pub fn find_at(&self, file: &Path, line: u32) -> Option<&Breakpoint> {
        self.breakpoints
            .iter()
            .find(|b| b.is_effective() && b.file == file && b.line == line)
    }

    /// Remove temporary breakpoints that were hit at `file:line`.
    ///
    /// Returns the removed breakpoint, if any.
    pub fn consume_temporary_at(&mut self, file: &Path, line: u32) -> Option<Breakpoint> {
        let index = self.breakpoints.iter().position(|b| {
            b.is_effective() && b.temporary && b.file == file && b.line == line
        })?;
        Some(self.breakpoints.remove(index))
    }

    /// Remap lines for one file after an edit.  See [`LineTracker`].
    pub fn apply_edit(
        &mut self,
        file: &Path,
        before: &str,
        edit: &TextEdit,
        after: &str,
    ) {
        let indices: Vec<usize> = self
            .breakpoints
            .iter()
            .enumerate()
            .filter(|(_, b)| b.file == file)
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return;
        }
        let lines: Vec<u32> = indices.iter().map(|&i| self.breakpoints[i].line).collect();
        let mut tracker = LineTracker::new(before, lines);
        tracker.apply_edit(after, edit);
        for (&index, &line) in indices.iter().zip(tracker.markers()) {
            self.breakpoints[index].line = line;
        }
        self.dedupe_file(file);
    }

    /// Collapse multiple breakpoints that ended up on the same line after an
    /// edit, keeping the first (lowest id).
    pub fn dedupe_file(&mut self, file: &Path) {
        let mut seen = std::collections::BTreeMap::new();
        self.breakpoints
            .retain(|b| {
                if b.file != file {
                    return true;
                }
                let key = (b.line, b.ghost);
                seen.insert(key, ()).is_none()
            });
    }

    /// Serialise to the persistence format (TOML).
    pub fn to_toml(&self) -> Result<String, DebugError> {
        let doc = BreakpointFile {
            version: 1,
            breakpoints: self.breakpoints.clone(),
        };
        toml::to_string_pretty(&doc)
            .map_err(|e| DebugError::Message(format!("serialise breakpoints: {e}")))
    }

    /// Parse the persistence format (TOML).
    pub fn from_toml(text: &str) -> Result<Self, DebugError> {
        let doc: BreakpointFile = toml::from_str(text)
            .map_err(|e| DebugError::Message(format!("invalid breakpoints file: {e}")))?;
        Ok(Self::with_breakpoints(doc.breakpoints))
    }

    /// JSON form used for the per-document extended attribute fallback.
    pub fn to_json(&self) -> Result<String, DebugError> {
        let doc = BreakpointFile {
            version: 1,
            breakpoints: self.breakpoints.clone(),
        };
        serde_json::to_string_pretty(&doc)
            .map_err(|e| DebugError::Message(format!("serialise breakpoints: {e}")))
    }

    /// Parse JSON breakpoint data.
    pub fn from_json(text: &str) -> Result<Self, DebugError> {
        let doc: BreakpointFile = serde_json::from_str(text)
            .map_err(|e| DebugError::Message(format!("parse breakpoints: {e}")))?;
        Ok(Self::with_breakpoints(doc.breakpoints))
    }

    /// Write the persistence file atomically.
    pub fn save(&self, path: &Path) -> Result<(), DebugError> {
        let text = self.to_toml()?;
        let tmp = path.with_extension("tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load the persistence file.  A missing file is an empty store.
    pub fn load(path: &Path) -> Result<Self, DebugError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(err.into()),
        }
    }
}

/// A replacement of the byte range `start..end` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// Start byte offset in the old text.
    pub start: usize,
    /// End byte offset in the old text (exclusive).
    pub end: usize,
    /// Replacement text.
    pub replacement: String,
}

impl TextEdit {
    /// A replacement edit.
    pub fn replace(start: usize, end: usize, replacement: impl Into<String>) -> Self {
        Self {
            start,
            end,
            replacement: replacement.into(),
        }
    }

    /// An insertion at `offset`.
    pub fn insert(offset: usize, text: impl Into<String>) -> Self {
        Self::replace(offset, offset, text)
    }

    /// A deletion.
    pub fn delete(start: usize, end: usize) -> Self {
        Self::replace(start, end, "")
    }
}

/// Maps marked line numbers through text edits.
///
/// This is a small pure model: the caller supplies the text before and after
/// the edit, and the tracker updates both its line index and the marker lines.
/// Markers whose line is deleted clamp to the first line of the replaced
/// range; markers after the edit shift by the difference in newline counts.
#[derive(Debug, Clone)]
pub struct LineTracker {
    line_starts: Vec<usize>,
    markers: Vec<u32>,
}

impl LineTracker {
    /// Build a tracker for `text` with one-based marker lines.
    pub fn new(text: &str, markers: impl IntoIterator<Item = u32>) -> Self {
        Self {
            line_starts: line_starts(text),
            markers: markers.into_iter().collect(),
        }
    }

    /// Current line starts (byte offsets), for tests and diagnostics.
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// Marker lines.
    pub fn markers(&self) -> &[u32] {
        &self.markers
    }

    /// Replace all markers.
    pub fn set_markers(&mut self, markers: impl IntoIterator<Item = u32>) {
        self.markers = markers.into_iter().collect();
    }

    /// Number of lines in the current text.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Zero-based line containing a byte offset.
    pub fn line_of_offset(&self, offset: usize) -> usize {
        match self.line_starts.partition_point(|&start| start <= offset) {
            0 => 0,
            n => n - 1,
        }
    }

    /// Apply an edit whose offsets refer to the current text, producing
    /// `new_text`.
    pub fn apply_edit(&mut self, new_text: &str, edit: &TextEdit) {
        if edit.end < edit.start {
            return;
        }
        let start_line = self.line_of_offset(edit.start);
        let end_line = self.line_of_offset(edit.end);
        let removed_newlines = end_line.saturating_sub(start_line);
        let inserted_newlines = edit.replacement.bytes().filter(|b| *b == b'\n').count();
        let delta = inserted_newlines as i64 - removed_newlines as i64;
        let new_line_count = self.line_starts.len() as i64 + delta;

        for marker in &mut self.markers {
            let index = (*marker as usize).saturating_sub(1);
            let Some(&line_start) = self.line_starts.get(index) else {
                continue;
            };
            let new_line = if line_start < edit.start {
                // The line begins before the edit: no renumbering.
                i64::from(*marker)
            } else if line_start >= edit.end {
                // The line begins after the edit: shift.
                i64::from(*marker) + delta
            } else {
                // The line's start offset was inside the replaced range: the
                // line was deleted or merged; clamp to the first affected line.
                start_line as i64 + 1
            };
            *marker = new_line.clamp(1, new_line_count.max(1)) as u32;
        }
        self.line_starts = line_starts(new_text);
        let count = self.line_starts.len() as i64;
        for marker in &mut self.markers {
            *marker = (*marker as i64).clamp(1, count.max(1)) as u32;
        }
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' && index + 1 < text.len() {
            starts.push(index + 1);
        }
    }
    starts
}

/// Group breakpoints by file, for persistence and menus.
pub fn group_by_file(store: &BreakpointStore) -> BTreeMap<PathBuf, Vec<&Breakpoint>> {
    let mut map: BTreeMap<PathBuf, Vec<&Breakpoint>> = BTreeMap::new();
    for bp in store.all() {
        map.entry(bp.file.clone()).or_default().push(bp);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_and_removes() {
        let mut store = BreakpointStore::new();
        let file = Path::new("/tmp/a.c");
        assert!(store.toggle(file, 4));
        assert_eq!(store.all().len(), 1);
        assert!(!store.toggle(file, 4));
        assert!(store.all().is_empty());
    }

    #[test]
    fn ghost_and_disabled_are_not_effective() {
        let mut store = BreakpointStore::new();
        store.add(Breakpoint::new("/tmp/a.c", 1).ghost());
        store.add(Breakpoint::new("/tmp/a.c", 2).disabled());
        store.add(Breakpoint::new("/tmp/a.c", 3).temporary());
        let lines: Vec<u32> = store.effective().map(|b| b.line).collect();
        assert_eq!(lines, [3]);
    }

    #[test]
    fn temporary_breakpoint_is_consumed() {
        let mut store = BreakpointStore::new();
        store.add(Breakpoint::new("/tmp/a.c", 7).temporary());
        let consumed = store.consume_temporary_at(Path::new("/tmp/a.c"), 7);
        assert!(consumed.is_some());
        assert!(store.all().is_empty());
    }

    #[test]
    fn persistence_round_trips() {
        let mut store = BreakpointStore::new();
        store.add(Breakpoint::new("/tmp/a.c", 3).with_condition("x > 1"));
        store.add(Breakpoint::new("/tmp/a.c", 9).disabled());
        let text = store.to_toml().unwrap();
        let back = BreakpointStore::from_toml(&text).unwrap();
        assert_eq!(back.all().len(), 2);
        assert_eq!(back.all()[0].condition.as_deref(), Some("x > 1"));
        assert!(!back.all()[1].enabled);
        let json = store.to_json().unwrap();
        assert_eq!(BreakpointStore::from_json(&json).unwrap().all().len(), 2);
    }

    #[test]
    fn edits_shift_lines_after_the_change() {
        let before = "a\nb\nc\n";
        let mut tracker = LineTracker::new(before, [1, 2, 3]);
        // Insert two lines at the top.
        let edit = TextEdit::insert(0, "x\ny\n");
        tracker.apply_edit(&format!("x\ny\n{before}"), &edit);
        assert_eq!(tracker.markers(), [3, 4, 5]);
        assert_eq!(tracker.line_count(), 5);
    }

    #[test]
    fn edit_within_a_line_keeps_the_marker() {
        let before = "one\ntwo\nthree\n";
        let mut tracker = LineTracker::new(before, [2]);
        let edit = TextEdit::replace(4, 7, "TWO");
        tracker.apply_edit("one\nTWO\nthree\n", &edit);
        assert_eq!(tracker.markers(), [2]);
    }

    #[test]
    fn deleting_lines_clamps_markers() {
        let before = "a\nb\nc\nd\n";
        let mut tracker = LineTracker::new(before, [3]);
        // Delete line 2 ("b\n").
        let edit = TextEdit::delete(2, 4);
        tracker.apply_edit("a\nc\nd\n", &edit);
        assert_eq!(tracker.markers(), [2]);
    }

    #[test]
    fn deleting_the_marked_line_clamps_to_the_first_line_of_the_range() {
        let before = "a\nb\nc\nd\n";
        let mut tracker = LineTracker::new(before, [2, 3]);
        let edit = TextEdit::delete(2, 6); // "b\nc\n"
        tracker.apply_edit("a\nd\n", &edit);
        assert_eq!(tracker.markers(), [2, 2]);
    }

    #[test]
    fn deleting_newlines_merges_markers_upward() {
        let before = "a\nb\nc\nd\n";
        let mut tracker = LineTracker::new(before, [4]);
        let edit = TextEdit::delete(1, 3); // remove "\nb"
        tracker.apply_edit("a\nc\nd\n", &edit);
        assert_eq!(tracker.markers(), [3]);
    }

    #[test]
    fn store_remaps_and_dedupes_after_an_edit() {
        let mut store = BreakpointStore::new();
        store.add_at("/tmp/a.c", 2);
        store.add_at("/tmp/a.c", 3);
        let before = "a\nb\nc\nd\n";
        // Delete line 2, merging markers 2 and 3.
        store.apply_edit(
            Path::new("/tmp/a.c"),
            before,
            &TextEdit::delete(2, 4),
            "a\nc\nd\n",
        );
        assert_eq!(store.all().len(), 1);
        assert_eq!(store.all()[0].line, 2);
    }

    #[test]
    fn ids_are_unique() {
        let mut store = BreakpointStore::new();
        let a = store.add_at("/tmp/a.c", 1);
        let b = store.add_at("/tmp/a.c", 2);
        assert_ne!(a, b);
        assert!(store.get(b).is_some());
    }
}
