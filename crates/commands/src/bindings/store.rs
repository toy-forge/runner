//! The user's key-binding sets directory.
//!
//! Layout (matching CodeRunner's, under our application-support folder):
//!
//! ```text
//! ~/Library/Application Support/ScratchRunner/KeyBindings/
//!     Default.crBindings      (optional user override of the built-in set)
//!     My Set.crBindings
//!     _Global_.crBindings     (global compound commands)
//! ```
//!
//! Sets are written as `.crBindings` for CodeRunner compatibility; `.json` and `.toml` files
//! in the directory are read too. Shipped sets are read-only; duplicating is the way to edit.

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::{BindingError, BindingSet, builtin, dict};

/// File name of the global set.
pub const GLOBAL_FILE: &str = "_Global_.crBindings";

/// Why a set store operation failed.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The directory or a file could not be read or written.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A set file could not be parsed.
    #[error("invalid key binding set {file:?}: {source}")]
    Bindings {
        /// The file path.
        file: PathBuf,
        /// Why.
        source: BindingError,
    },
    /// A name is empty or contains path separators.
    #[error("invalid set name {0:?}")]
    InvalidName(String),
    /// The set is read-only (a shipped set): duplicate it first.
    #[error("the set {0:?} is read-only; duplicate it to edit")]
    ReadOnly(String),
    /// No such user set exists.
    #[error("no key binding set named {0:?}")]
    NotFound(String),
}

/// A directory of key-binding sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetStore {
    dir: PathBuf,
}

impl SetStore {
    /// A store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> SetStore {
        SetStore { dir: dir.into() }
    }

    /// `~/Library/Application Support/ScratchRunner/KeyBindings`, if `HOME` is set.
    pub fn default_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/ScratchRunner/KeyBindings"))
    }

    /// The store under the application-support directory, if `HOME` is set.
    pub fn application_default() -> Option<SetStore> {
        SetStore::default_dir().map(SetStore::new)
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Creates the directory when missing.
    pub fn ensure_dir(&self) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    fn validate_name(name: &str) -> Result<(), StoreError> {
        if name.trim().is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
            Err(StoreError::InvalidName(name.to_owned()))
        } else {
            Ok(())
        }
    }

    fn path_for(&self, name: &str) -> Result<PathBuf, StoreError> {
        SetStore::validate_name(name)?;
        Ok(self.dir.join(format!("{name}.crBindings")))
    }

    fn candidates(&self, name: &str) -> Result<Vec<PathBuf>, StoreError> {
        SetStore::validate_name(name)?;
        Ok(vec![self.dir.join(format!("{name}.crBindings")), self.dir.join(format!("{name}.json")), self.dir.join(format!("{name}.toml"))])
    }

    fn read_set(&self, path: &Path) -> Result<BindingSet, StoreError> {
        BindingSet::load_file(path).map_err(|source| StoreError::Bindings { file: path.to_path_buf(), source })
    }

    /// Loads a set by name: a user file if present, else a built-in set.
    pub fn load_set(&self, name: &str) -> Result<BindingSet, StoreError> {
        if name == builtin::GLOBAL_SET_NAME {
            return self.load_global();
        }
        for path in self.candidates(name)? {
            if path.is_file() {
                return self.read_set(&path);
            }
        }
        builtin::builtin_set(name).ok_or_else(|| StoreError::NotFound(name.to_owned()))
    }

    /// The names of the user's set files (without extension), sorted.
    pub fn user_set_names(&self) -> Result<Vec<String>, StoreError> {
        let mut names = Vec::new();
        if !self.dir.exists() {
            return Ok(names);
        }
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
            if !matches!(ext.as_str(), "crbindings" | "json" | "toml") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|n| n.to_str()) else { continue };
            if name == builtin::GLOBAL_SET_NAME {
                continue;
            }
            names.push(name.to_owned());
        }
        names.sort_by_key(|a| a.to_lowercase());
        names.dedup();
        Ok(names)
    }

    /// All sets for the "Key Bindings Set" popup: shipped `Default` and `Vim` first (a user set
    /// with the same name replaces the shipped one), then the other user sets.
    pub fn list_sets(&self) -> Result<Vec<BindingSet>, StoreError> {
        let mut sets = builtin::builtin_sets();
        for name in self.user_set_names()? {
            // Prefer the .crBindings file when several formats share a name.
            let path = self
                .candidates(&name)?
                .into_iter()
                .find(|p| p.is_file())
                .ok_or_else(|| StoreError::NotFound(name.clone()))?;
            let set = self.read_set(&path)?;
            match sets.iter_mut().find(|s| s.title == set.title) {
                Some(slot) => *slot = set,
                None => sets.push(set),
            }
        }
        Ok(sets)
    }

    /// Saves an editable set as `<title>.crBindings`; returns the path.
    pub fn save_set(&self, set: &BindingSet) -> Result<PathBuf, StoreError> {
        if set.read_only {
            return Err(StoreError::ReadOnly(set.title.clone()));
        }
        self.ensure_dir()?;
        let path = self.path_for(&set.title)?;
        cleanup_other_formats(&self.dir, &set.title, &path);
        set.save_file(&path).map_err(|source| StoreError::Bindings { file: path.clone(), source })?;
        Ok(path)
    }

    /// Deletes a user set (all formats sharing the name). Shipped sets cannot be deleted.
    pub fn delete_set(&self, name: &str) -> Result<(), StoreError> {
        if builtin::builtin_set(name).is_some() && !self.candidates(name)?.iter().any(|p| p.is_file()) {
            return Err(StoreError::ReadOnly(name.to_owned()));
        }
        let mut removed = false;
        for path in self.candidates(name)? {
            if path.is_file() {
                std::fs::remove_file(&path)?;
                removed = true;
            }
        }
        if removed { Ok(()) } else { Err(StoreError::NotFound(name.to_owned())) }
    }

    /// The global set: the user's `_Global_.crBindings` when present, else the shipped one.
    pub fn load_global(&self) -> Result<BindingSet, StoreError> {
        let path = self.dir.join(GLOBAL_FILE);
        if path.is_file() {
            let mut set = self.read_set(&path)?;
            set.read_only = false;
            Ok(set)
        } else {
            let mut set = builtin::global_set();
            set.read_only = false;
            Ok(set)
        }
    }

    /// Saves the global set (never read-only).
    pub fn save_global(&self, set: &BindingSet) -> Result<PathBuf, StoreError> {
        self.ensure_dir()?;
        let mut set = set.clone();
        set.title = builtin::GLOBAL_SET_NAME.to_owned();
        set.read_only = false;
        let path = self.dir.join(GLOBAL_FILE);
        set.save_file(&path).map_err(|source| StoreError::Bindings { file: path.clone(), source })?;
        Ok(path)
    }

    /// Removes the user's global file and returns the shipped global commands.
    pub fn reset_global(&self) -> Result<BindingSet, StoreError> {
        let path = self.dir.join(GLOBAL_FILE);
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        let mut set = builtin::global_set();
        set.read_only = false;
        Ok(set)
    }

    /// Imports a `.crBindings`, `.json`, `.toml` or macOS `KeyBindings.dict` file into the store.
    /// Returns the imported set and, for a dict, the skipped-entry warnings.
    pub fn import(&self, path: &Path) -> Result<(BindingSet, Vec<String>), StoreError> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let (mut set, warnings) = if ext == "dict" {
            let bytes = std::fs::read(path)?;
            let import = dict::import_key_bindings_dict(&bytes)
                .map_err(|source| StoreError::Bindings { file: path.to_path_buf(), source })?;
            (import.set, import.warnings)
        } else {
            (self.read_set(path)?, Vec::new())
        };
        set.read_only = false;
        self.save_set(&set)?;
        Ok((set, warnings))
    }

    /// Exports a set to `path`, choosing the format by extension.
    pub fn export(&self, set: &BindingSet, path: &Path) -> Result<(), StoreError> {
        set.save_file(path).map_err(|source| StoreError::Bindings { file: path.to_path_buf(), source })
    }
}

fn cleanup_other_formats(dir: &Path, title: &str, keep: &Path) {
    for ext in ["json", "toml"] {
        let path = dir.join(format!("{title}.{ext}"));
        if path != keep && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::Binding;

    fn store() -> (tempfile::TempDir, SetStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SetStore::new(dir.path().join("KeyBindings"));
        (dir, store)
    }

    fn custom() -> BindingSet {
        BindingSet { title: "Mine".into(), read_only: false, bindings: vec![Binding::compound("Hi", "~@h", r#"["insertText:", "hi"]"#)] }
    }

    #[test]
    fn save_list_load_delete() {
        let (_t, store) = store();
        let path = store.save_set(&custom()).unwrap();
        assert!(path.ends_with("Mine.crBindings"));
        let sets = store.list_sets().unwrap();
        assert_eq!(sets.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Default", "Vim", "Mine"]);
        let loaded = store.load_set("Mine").unwrap();
        assert_eq!(loaded.title, "Mine");
        assert_eq!(loaded.bindings.len(), 1);
        assert_eq!(loaded.bindings[0].compound_code.as_deref(), Some("[\"insertText:\", \"hi\"]"));
        store.delete_set("Mine").unwrap();
        assert!(matches!(store.load_set("Mine"), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn builtins_are_read_only_and_global_resets() {
        let (_t, store) = store();
        assert!(matches!(store.save_set(&crate::bindings::builtin::default_set()), Err(StoreError::ReadOnly(_))));
        let g = store.load_global().unwrap();
        assert_eq!(g.title, builtin::GLOBAL_SET_NAME);
        assert!(!g.bindings.is_empty());
        store.save_global(&g).unwrap();
        assert!(store.dir().join(GLOBAL_FILE).is_file());
        let reset = store.reset_global().unwrap();
        assert!(!reset.read_only);
        assert_eq!(reset.bindings.len(), crate::bindings::builtin::global_set().bindings.len());
        assert!(!store.dir().join(GLOBAL_FILE).is_file());
    }

    #[test]
    fn import_dict_and_export_json() {
        let (t, store) = store();
        let dict = t.path().join("mine.dict");
        std::fs::write(&dict, "{ \"^w\" = \"deleteWordBackward:\"; }").unwrap();
        let (set, warnings) = store.import(&dict).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(set.title, "macOS Key Bindings");
        assert_eq!(store.load_set("macOS Key Bindings").unwrap().bindings.len(), 1);
        let out = t.path().join("export.json");
        store.export(&set, &out).unwrap();
        assert_eq!(BindingSet::load_file(&out).unwrap(), set);
    }

    #[test]
    fn invalid_names() {
        let (_t, store) = store();
        assert!(matches!(store.load_set("../x"), Err(StoreError::InvalidName(_))));
    }
}
