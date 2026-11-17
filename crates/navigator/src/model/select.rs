//! Selection and keyboard navigation over a flattened row list.

use std::collections::HashSet;

use super::{entry::EntryId, tree::Row};

/// Multi-selection with an anchor (for Shift ranges) and a cursor (the row
/// the arrow keys move).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    ids: HashSet<EntryId>,
    /// Where a Shift-range starts.
    pub anchor: Option<EntryId>,
    /// The row keys act on.
    pub cursor: Option<EntryId>,
}

/// Modifier keys of a click.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickMods {
    /// Shift: extend a range.
    pub shift: bool,
    /// Command: toggle one row.
    pub cmd: bool,
}

/// The index of `id` in `rows`.
pub fn index_of(rows: &[Row], id: EntryId) -> Option<usize> {
    rows.iter().position(|r| r.entry.id == id)
}

impl Selection {
    /// Whether `id` is selected.
    pub fn contains(&self, id: EntryId) -> bool {
        self.ids.contains(&id)
    }

    /// Number of selected rows.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The selected ids in row order.
    pub fn in_order(&self, rows: &[Row]) -> Vec<EntryId> {
        rows.iter()
            .filter(|r| self.ids.contains(&r.entry.id))
            .map(|r| r.entry.id)
            .collect()
    }

    /// Selects only `id`.
    pub fn select_only(&mut self, id: EntryId) {
        self.ids.clear();
        self.ids.insert(id);
        self.anchor = Some(id);
        self.cursor = Some(id);
    }

    /// Clears the selection.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
        self.cursor = None;
    }

    /// A click on row `id`.
    pub fn click(&mut self, rows: &[Row], id: EntryId, mods: ClickMods) {
        if mods.shift {
            self.extend_to(rows, id);
        } else if mods.cmd {
            if !self.ids.remove(&id) {
                self.ids.insert(id);
            }
            self.anchor = Some(id);
            self.cursor = Some(id);
        } else {
            self.select_only(id);
        }
    }

    /// Selects the range from the anchor to `id`.
    pub fn extend_to(&mut self, rows: &[Row], id: EntryId) {
        let anchor = self.anchor.or(self.cursor).unwrap_or(id);
        let (Some(a), Some(b)) = (index_of(rows, anchor), index_of(rows, id)) else {
            self.select_only(id);
            return;
        };
        let (lo, hi) = (a.min(b), a.max(b));
        self.ids = rows[lo..=hi].iter().map(|r| r.entry.id).collect();
        self.anchor = Some(anchor);
        self.cursor = Some(id);
    }

    /// Moves the cursor by `delta` rows (clamped); `extend` keeps a range.
    pub fn step(&mut self, rows: &[Row], delta: isize, extend: bool) {
        if rows.is_empty() {
            return;
        }
        let at = self.cursor.and_then(|c| index_of(rows, c));
        let next = match at {
            Some(i) => (i as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None if delta >= 0 => 0,
            None => rows.len() - 1,
        };
        let id = rows[next].entry.id;
        if extend {
            self.extend_to(rows, id);
        } else {
            self.select_only(id);
        }
    }

    /// Cursor to the first or last row.
    pub fn jump(&mut self, rows: &[Row], last: bool, extend: bool) {
        if rows.is_empty() {
            return;
        }
        let id = rows[if last { rows.len() - 1 } else { 0 }].entry.id;
        if extend {
            self.extend_to(rows, id);
        } else {
            self.select_only(id);
        }
    }

    /// Keeps only ids still present in `rows` (after a refresh).
    pub fn retain_visible(&mut self, rows: &[Row]) {
        let present: HashSet<EntryId> = rows.iter().map(|r| r.entry.id).collect();
        self.ids.retain(|i| present.contains(i));
        if self.anchor.is_some_and(|a| !present.contains(&a)) {
            self.anchor = None;
        }
        if self.cursor.is_some_and(|c| !present.contains(&c)) {
            self.cursor = self.ids.iter().next().copied();
        }
    }
}

/// A command of the Finder-like keyboard model (report 10, G30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCommand {
    /// Up arrow (`extend`: Shift).
    Up {
        /// Extend the selection.
        extend: bool,
    },
    /// Down arrow.
    Down {
        /// Extend the selection.
        extend: bool,
    },
    /// Home.
    First,
    /// End.
    Last,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Right: expand a folder, else step into it.
    Expand,
    /// Left: collapse a folder, else go to the parent row.
    Collapse,
    /// Return: begin inline rename.
    Rename,
    /// Cmd+Down: open the file / enter the folder.
    Open,
    /// Cmd+Up: re-root at the parent folder.
    GoParent,
    /// Cmd+Backspace: move to Trash.
    Trash,
    /// Shift+Cmd+N: new folder.
    NewFolder,
    /// Escape: clear the filter.
    Escape,
    /// A printable character: type-to-filter.
    Type(char),
    /// Backspace: delete one filter character.
    Backspace,
    /// Cmd+A: select all.
    SelectAll,
}

/// Every keystroke the navigator binds while it has the keyboard focus, in
/// `nsh_core::keymap` syntax.
pub fn key_bindings() -> Vec<(String, NavCommand)> {
    let mut v: Vec<(String, NavCommand)> = vec![
        ("up".into(), NavCommand::Up { extend: false }),
        ("down".into(), NavCommand::Down { extend: false }),
        ("shift+up".into(), NavCommand::Up { extend: true }),
        ("shift+down".into(), NavCommand::Down { extend: true }),
        ("home".into(), NavCommand::First),
        ("end".into(), NavCommand::Last),
        ("pageup".into(), NavCommand::PageUp),
        ("pagedown".into(), NavCommand::PageDown),
        ("right".into(), NavCommand::Expand),
        ("left".into(), NavCommand::Collapse),
        ("enter".into(), NavCommand::Rename),
        ("mod+down".into(), NavCommand::Open),
        ("mod+up".into(), NavCommand::GoParent),
        ("mod+backspace".into(), NavCommand::Trash),
        ("mod+shift+n".into(), NavCommand::NewFolder),
        ("escape".into(), NavCommand::Escape),
        ("backspace".into(), NavCommand::Backspace),
        ("mod+a".into(), NavCommand::SelectAll),
    ];
    // `on_key` panics on a keystroke written as the character it types with Shift ("_"):
    // underscore is "shift+-".
    v.push(("shift+-".into(), NavCommand::Type('_')));
    for c in ('a'..='z').chain('0'..='9').chain(['-', '.', ' ']) {
        let key = if c == ' ' {
            "space".to_owned()
        } else {
            c.to_string()
        };
        v.push((key.clone(), NavCommand::Type(c)));
        if c.is_ascii_lowercase() {
            v.push((
                format!("shift+{key}"),
                NavCommand::Type(c.to_ascii_uppercase()),
            ));
        }
    }
    v
}

/// The filter text after a typed character / backspace.
pub fn edit_filter(filter: &str, cmd: NavCommand) -> Option<String> {
    match cmd {
        NavCommand::Type(c) => {
            // A leading space is not a filter (Space is not "Quick Look" here).
            if c == ' ' && filter.is_empty() {
                return None;
            }
            Some(format!("{filter}{c}"))
        }
        NavCommand::Backspace => {
            let mut s = filter.to_owned();
            s.pop().map(|_| s)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::model::{
        entry::{Entry, Kind},
        tree::Disclosure,
    };

    fn rows(n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| Row {
                entry: Entry::new(Arc::<Path>::from(Path::new(&format!("/r/{i}"))), Kind::File),
                depth: 0,
                disclosure: Disclosure::None,
                draft: false,
            })
            .collect()
    }

    #[test]
    fn arrows_clamp_and_shift_extends() {
        let r = rows(5);
        let mut s = Selection::default();
        s.step(&r, 1, false);
        assert_eq!(
            s.cursor,
            Some(r[0].entry.id),
            "first Down selects the first row"
        );
        s.step(&r, 1, false);
        s.step(&r, 1, true);
        s.step(&r, 1, true);
        assert_eq!(
            s.in_order(&r),
            vec![r[1].entry.id, r[2].entry.id, r[3].entry.id]
        );
        s.step(&r, 1, false);
        s.step(&r, 10, false);
        assert_eq!(s.cursor, Some(r[4].entry.id));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn cmd_toggles_and_shift_ranges_from_the_anchor() {
        let r = rows(6);
        let mut s = Selection::default();
        s.click(&r, r[1].entry.id, ClickMods::default());
        s.click(
            &r,
            r[3].entry.id,
            ClickMods {
                cmd: true,
                shift: false,
            },
        );
        assert_eq!(s.len(), 2);
        s.click(
            &r,
            r[3].entry.id,
            ClickMods {
                cmd: true,
                shift: false,
            },
        );
        assert_eq!(s.len(), 1);
        s.click(
            &r,
            r[4].entry.id,
            ClickMods {
                shift: true,
                cmd: false,
            },
        );
        assert_eq!(s.len(), 2, "range from the anchor (row 3) to row 4");
    }

    #[test]
    fn type_to_filter_edits() {
        assert_eq!(edit_filter("", NavCommand::Type('a')), Some("a".into()));
        assert_eq!(edit_filter("ab", NavCommand::Backspace), Some("a".into()));
        assert_eq!(edit_filter("", NavCommand::Backspace), None);
        assert_eq!(edit_filter("", NavCommand::Type(' ')), None);
    }

    #[test]
    fn bindings_cover_the_g30_keys() {
        let b = key_bindings();
        for k in [
            "enter",
            "mod+down",
            "mod+up",
            "mod+backspace",
            "mod+shift+n",
            "escape",
            "a",
            "shift+a",
        ] {
            assert!(b.iter().any(|(s, _)| s == k), "{k}");
        }
    }
}
