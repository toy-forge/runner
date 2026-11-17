//! Key binding sets: the data model, file formats and the resolved [`KeyMap`].
//!
//! A [`BindingSet`] is what the Key Bindings pane edits: a titled list of [`Binding`]s, each
//! mapping a key equivalent to a menu path, an editor command name or a compound command.
//! Sets load from and save to
//!
//! * CodeRunner's `.crBindings` XML property lists ([`BindingSet::from_plist_bytes`], [`BindingSet::to_plist_bytes`]),
//! * our own JSON or TOML ([`BindingSet::from_json`], [`BindingSet::to_toml`] ...),
//! * macOS `KeyBindings.dict` (import only, [`import_key_bindings_dict`]).
//!
//! [`default_set`], [`vim_set`] and [`global_set`] provide the shipped sets; [`SetStore`] manages
//! the user's sets directory; [`KeyMap`] resolves active set + globals + menu defaults into
//! shortcut lookups, menu shortcuts and conflict reports.

mod builtin;
mod dict;
mod files;
mod keymap;
mod plist_io;
mod store;

use std::sync::Arc;

use thiserror::Error;
use uuid::Uuid;

pub use builtin::{builtin_set, builtin_sets, default_set, global_set, vim_set, DEFAULT_SET_NAME, GLOBAL_SET_NAME, VIM_SET_NAME};
pub use dict::{import_key_bindings_dict, import_key_bindings_dict_str, DictImport};
pub use keymap::{Conflict, ConflictKind, KeyMap, KeyMapEntry, KeyMapIssue, Lookup, Source, Target};
pub use store::{SetStore, StoreError};

use crate::compound::{CommandLibrary, CompoundCommand, CompoundError};
use crate::shortcut::{Shortcut, ShortcutError};

/// Why a binding set could not be read, written or used.
#[derive(Debug, Error)]
pub enum BindingError {
    /// The property list is malformed.
    #[error("invalid property list: {0}")]
    Plist(String),
    /// The structure of a file is wrong.
    #[error("invalid binding set: {0}")]
    Format(String),
    /// JSON text could not be read or written.
    #[error("JSON error: {0}")]
    Json(String),
    /// TOML text could not be read or written.
    #[error("TOML error: {0}")]
    Toml(String),
    /// A key equivalent is invalid.
    #[error("invalid key equivalent {key:?}: {source}")]
    Key {
        /// The text.
        key: String,
        /// Why.
        source: ShortcutError,
    },
    /// A compound command is invalid.
    #[error("invalid compound command in {title:?}: {source}")]
    Compound {
        /// The binding title.
        title: String,
        /// Why.
        source: CompoundError,
    },
    /// The file extension is not `.crBindings`, `.json` or `.toml`.
    #[error("unsupported key binding file {0:?} (use .crBindings, .json or .toml)")]
    UnsupportedExtension(String),
    /// Reading or writing the file failed.
    #[error("I/O error: {0}")]
    Io(String),
    /// A macOS `KeyBindings.dict` could not be parsed.
    #[error("invalid KeyBindings.dict: {0}")]
    Dict(String),
}

/// One key binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Stable identifier (an upper-case UUID).
    pub identifier: String,
    /// The CodeRunner key equivalent; `None` or empty means "callable by name only"
    /// (or, for a key-table compound command, "keyed by the table").
    pub key_equivalent: Option<String>,
    /// Title shown in the UI and used as the name for `callCompoundCommand:`.
    pub title: Option<String>,
    /// A menu path (`MainMenu/Find/Find Mode/Regular Expression`, wildcards allowed) or an
    /// editor command name (`insertNewlineBelow`, `selectTab3`).
    pub command: Option<String>,
    /// Source text of a compound command (JSON with comments and trailing commas).
    pub compound_code: Option<String>,
    /// Whether the binding is active (the switch in the first table column).
    pub enabled: bool,
}

impl Binding {
    /// A binding of `shortcut` to a menu path or command name.
    pub fn command(title: Option<&str>, key_equivalent: &str, command: &str) -> Binding {
        Binding {
            identifier: new_identifier(),
            key_equivalent: Some(key_equivalent.to_owned()),
            title: title.map(str::to_owned),
            command: Some(command.to_owned()),
            compound_code: None,
            enabled: true,
        }
    }

    /// A binding of `key_equivalent` (may be empty) to a compound command.
    pub fn compound(title: &str, key_equivalent: &str, code: &str) -> Binding {
        Binding {
            identifier: new_identifier(),
            key_equivalent: Some(key_equivalent.to_owned()),
            title: Some(title.to_owned()),
            command: None,
            compound_code: Some(code.to_owned()),
            enabled: true,
        }
    }

    /// The parsed shortcut (`None` when the key equivalent is absent or empty).
    pub fn shortcut(&self) -> Result<Option<Shortcut>, ShortcutError> {
        match self.key_equivalent.as_deref() {
            None | Some("") => Ok(None),
            Some(k) => Shortcut::parse_crbindings(k).map(Some),
        }
    }

    /// Sets the shortcut (stored as a CodeRunner key equivalent).
    pub fn set_shortcut(&mut self, shortcut: Option<&Shortcut>) {
        self.key_equivalent = shortcut.map(Shortcut::to_crbindings);
    }

    /// The parsed compound command, if the binding has one.
    pub fn parse_compound(&self) -> Option<Result<CompoundCommand, CompoundError>> {
        self.compound_code.as_deref().map(CompoundCommand::parse)
    }

    /// Whether the binding is a compound command.
    pub fn is_compound(&self) -> bool {
        self.compound_code.is_some()
    }

    /// The name to show in a list: title, else the command, else the key equivalent.
    pub fn display_name(&self) -> String {
        self.title.clone().or_else(|| self.command.clone()).or_else(|| self.key_equivalent.clone()).unwrap_or_default()
    }
}

/// A new upper-case UUID string for a binding.
pub fn new_identifier() -> String {
    // v5 over a counter-free random source is unavailable without the `v4` feature; derive from time and address.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{nanos}-{n}-{}", std::process::id());
    Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes()).to_string().to_uppercase()
}

/// A stable identifier derived from a set name and binding slug (for built-in sets).
pub(crate) fn stable_identifier(set: &str, slug: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("scratchrunner:binding:{set}:{slug}").as_bytes()).to_string().to_uppercase()
}

/// A titled set of bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingSet {
    /// The set's name.
    pub title: String,
    /// Shipped sets are read-only: edit a duplicate.
    pub read_only: bool,
    /// The bindings.
    pub bindings: Vec<Binding>,
}

impl BindingSet {
    /// An empty, editable set.
    pub fn new(title: &str) -> BindingSet {
        BindingSet { title: title.to_owned(), read_only: false, bindings: Vec::new() }
    }

    /// A copy under a new name, editable.
    pub fn duplicate(&self, title: &str) -> BindingSet {
        BindingSet { title: title.to_owned(), read_only: false, bindings: self.bindings.clone() }
    }

    /// The binding with this identifier.
    pub fn find(&self, identifier: &str) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.identifier == identifier)
    }

    /// The compound command titled `title` (for `Load` / `Unload`).
    pub fn compound_titled(&self, title: &str) -> Option<Result<CompoundCommand, CompoundError>> {
        self.bindings.iter().find(|b| b.title.as_deref() == Some(title) && b.is_compound()).and_then(Binding::parse_compound)
    }

    /// All compound commands as a [`CommandLibrary`] for `callCompoundCommand:`; commands that
    /// fail to parse are skipped (see [`BindingSet::validate`]).
    pub fn library(&self) -> CommandLibrary {
        let mut lib = CommandLibrary::new();
        self.add_to_library(&mut lib);
        lib
    }

    /// Adds this set's compound commands to `lib`.
    pub fn add_to_library(&self, lib: &mut CommandLibrary) {
        for b in self.bindings.iter().filter(|b| b.enabled) {
            if let Some(Ok(c)) = b.parse_compound() {
                lib.add_command(b.title.as_deref().unwrap_or(""), &c);
            }
        }
    }

    /// Library of one or several sets; earlier sets win when a compound command name repeats.
    pub fn library_of(sets: &[&BindingSet]) -> Arc<CommandLibrary> {
        let mut lib = CommandLibrary::new();
        for set in sets {
            set.add_missing_to_library(&mut lib);
        }
        Arc::new(lib)
    }

    fn add_missing_to_library(&self, lib: &mut CommandLibrary) {
        for b in self.bindings.iter().filter(|b| b.enabled) {
            let Some(Ok(c)) = b.parse_compound() else { continue };
            let title = b.title.as_deref().unwrap_or("");
            match &c.root {
                crate::compound::Root::List(l) => {
                    if lib.get(title).is_none() {
                        lib.insert(title, l.clone());
                    }
                }
                crate::compound::Root::Entries(t) => {
                    for e in &t.entries {
                        if lib.get(&e.key).is_none() {
                            lib.insert(e.key.clone(), e.list.clone());
                        }
                    }
                }
            }
        }
    }

    /// Checks every binding: key equivalents parse, compound commands parse, exactly one of
    /// command / compound code is present (helpers have compound code and no key).
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        for b in &self.bindings {
            let name = b.display_name();
            if let Err(e) = b.shortcut() {
                out.push(format!("{name}: {e}"));
            }
            match (&b.command, &b.compound_code) {
                (Some(_), Some(_)) => out.push(format!("{name}: has both a command and compound code")),
                (None, None) => out.push(format!("{name}: has neither a command nor compound code")),
                _ => {}
            }
            if let Some(Err(e)) = b.parse_compound() {
                out.push(format!("{name}: {e}"));
            }
        }
        out
    }

    /// Reads a set from a `.crBindings` property list (XML; tolerates raw control characters).
    pub fn from_plist_bytes(bytes: &[u8]) -> Result<BindingSet, BindingError> {
        plist_io::read(bytes)
    }

    /// Writes the set as a `.crBindings` XML property list (CodeRunner-compatible, including the
    /// regenerated `compoundCommandArray` / `compoundCommandDictionary` caches).
    pub fn to_plist_bytes(&self) -> Result<Vec<u8>, BindingError> {
        plist_io::write(self)
    }

    /// Reads a set from our JSON format.
    pub fn from_json(text: &str) -> Result<BindingSet, BindingError> {
        files::from_json(text)
    }

    /// Writes our JSON format.
    pub fn to_json(&self) -> Result<String, BindingError> {
        files::to_json(self)
    }

    /// Reads a set from our TOML format.
    pub fn from_toml(text: &str) -> Result<BindingSet, BindingError> {
        files::from_toml(text)
    }

    /// Writes our TOML format.
    pub fn to_toml(&self) -> Result<String, BindingError> {
        files::to_toml(self)
    }

    /// Loads a set from a file, by extension (`.crBindings`, `.json`, `.toml`).
    pub fn load_file(path: &std::path::Path) -> Result<BindingSet, BindingError> {
        files::load_file(path)
    }

    /// Saves the set to a file, by extension.
    pub fn save_file(&self, path: &std::path::Path) -> Result<(), BindingError> {
        files::save_file(self, path)
    }
}
