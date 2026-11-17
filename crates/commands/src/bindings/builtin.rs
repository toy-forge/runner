//! The shipped binding sets, authored as data in `crates/commands/data/`.
//!
//! * `Default.json` - CodeRunner 4.6's Default set: additions and overrides on top of the
//!   menu-bar shortcuts, including the ⌥⌘E chords and the four "move to character" compound
//!   commands.
//! * `Vim.json` - a Vim modal set in our compound-command language (see the report summary;
//!   the behaviour is re-implemented here, not copied).
//! * `Global.json` - always-active compound commands (auto-pairing braces/brackets, HTML tag
//!   closing, Markdown emphasis, shell `[  ]`).
//!
//! All three are read-only (the global set is editable through the user's copy).

use std::sync::OnceLock;

use super::BindingSet;

/// Name of the shipped default set.
pub const DEFAULT_SET_NAME: &str = "Default";
/// Name of the shipped Vim set.
pub const VIM_SET_NAME: &str = "Vim";
/// Name of the global compound-command set.
pub const GLOBAL_SET_NAME: &str = "_Global_";

const DEFAULT_DATA: &str = include_str!("../../data/default.json");
const VIM_DATA: &str = include_str!("../../data/vim.json");
const GLOBAL_DATA: &str = include_str!("../../data/global.json");

fn parse_cached(slot: &'static OnceLock<BindingSet>, data: &str, title: &str, read_only: bool) -> &'static BindingSet {
    slot.get_or_init(|| match BindingSet::from_json(data) {
        Ok(mut set) => {
            set.title = title.to_owned();
            set.read_only = read_only;
            set
        }
        Err(e) => {
            eprintln!("scratch-commands: shipped key binding set {title:?} is invalid: {e}");
            BindingSet { title: title.to_owned(), read_only, bindings: Vec::new() }
        }
    })
}

/// The shipped Default set.
pub fn default_set() -> BindingSet {
    static SET: OnceLock<BindingSet> = OnceLock::new();
    parse_cached(&SET, DEFAULT_DATA, DEFAULT_SET_NAME, true).clone()
}

/// The shipped Vim set.
pub fn vim_set() -> BindingSet {
    static SET: OnceLock<BindingSet> = OnceLock::new();
    parse_cached(&SET, VIM_DATA, VIM_SET_NAME, true).clone()
}

/// The shipped global compound commands.
pub fn global_set() -> BindingSet {
    static SET: OnceLock<BindingSet> = OnceLock::new();
    parse_cached(&SET, GLOBAL_DATA, GLOBAL_SET_NAME, true).clone()
}

/// `Default` and `Vim`, in popup order.
pub fn builtin_sets() -> Vec<BindingSet> {
    vec![default_set(), vim_set()]
}

/// A shipped set by name (`Default`, `Vim`, `_Global_`).
pub fn builtin_set(name: &str) -> Option<BindingSet> {
    match name {
        DEFAULT_SET_NAME => Some(default_set()),
        VIM_SET_NAME => Some(vim_set()),
        GLOBAL_SET_NAME => Some(global_set()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_sets_are_valid_and_read_only() {
        for set in [default_set(), vim_set(), global_set()] {
            assert!(set.read_only, "{}", set.title);
            assert!(!set.bindings.is_empty(), "{}", set.title);
            let problems = set.validate();
            assert!(problems.is_empty(), "{}: {problems:?}", set.title);
        }
        assert_eq!(default_set().bindings.len(), 30);
        assert_eq!(global_set().bindings.len(), 9);
    }

    #[test]
    fn vim_set_has_helpers_and_modes() {
        let vim = vim_set();
        assert!(vim.bindings.len() >= 60, "{}", vim.bindings.len());
        for helper in ["Load", "Unload", "Helper: Insert", "Helper: Exit Modes", "Helper: Recursive Repeat"] {
            assert!(vim.compound_titled(helper).is_some(), "missing {helper}");
        }
    }
}
