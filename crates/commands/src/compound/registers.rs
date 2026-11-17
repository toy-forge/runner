//! Named selections, texts and numbers shared by the commands of a compound-command run
//! (and, because the owner keeps the [`Registers`] between keystrokes, between runs: Vim's
//! `repeat` count and marks live here).

use std::collections::HashMap;

use super::host::Selection;

/// A saved selection and the document revision it was saved at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedSelection {
    /// The selection.
    pub selection: Selection,
    /// [`crate::compound::Host::revision`] when it was saved.
    pub revision: u64,
}

/// The named state of compound commands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registers {
    /// Named selections (`saveSelectionWithName:`).
    pub selections: HashMap<String, SavedSelection>,
    /// Named texts (`copyWithName:`, `setText:withName:`).
    pub texts: HashMap<String, String>,
    /// Named numbers (`setNumber:withName:`).
    pub numbers: HashMap<String, f64>,
}

impl Registers {
    /// Empty registers.
    pub fn new() -> Registers {
        Registers::default()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Registers::default();
    }
}
