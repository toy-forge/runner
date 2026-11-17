//! The set of available themes: the built-ins plus the `.tmTheme` files in the
//! user's themes directory, with the Themes tab's operations (new, duplicate,
//! rename, invert, delete/reset, import, export).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::builtin::{DEFAULT_DARK, DEFAULT_LIGHT, builtin_themes};
use crate::error::ThemeError;
use crate::theme::{Theme, ThemeOrigin};

/// One row of the theme list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    /// The theme's name.
    pub name: String,
    /// Dark theme (background luminance < 0.5).
    pub is_dark: bool,
    /// A built-in theme exists with this name (it may be overridden by a
    /// user copy; "Reset" removes the copy).
    pub is_builtin: bool,
    /// A user file exists for this name.
    pub is_user: bool,
}

/// Built-in and user themes. Cheap to clone out of: themes are `Arc`s.
#[derive(Debug)]
pub struct ThemeLibrary {
    user_dir: Option<PathBuf>,
    builtin: Vec<Arc<Theme>>,
    user: Vec<Arc<Theme>>,
    warnings: Vec<String>,
}

fn is_theme_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|e| ["tmtheme", "crtheme"].contains(&e.to_ascii_lowercase().as_str()))
}

fn file_name_for(name: &str) -> Result<String, ThemeError> {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' || c.is_control() { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim_start_matches('.').trim().to_owned();
    if cleaned.is_empty() || cleaned.len() > 120 {
        return Err(ThemeError::InvalidName(name.to_owned()));
    }
    Ok(format!("{cleaned}.tmTheme"))
}

impl ThemeLibrary {
    /// A library of the built-ins and the themes found in `user_dir`
    /// (`None`: built-ins only). Unreadable files are skipped and reported by
    /// [`ThemeLibrary::warnings`]. A missing directory is not an error.
    pub fn new(user_dir: Option<PathBuf>) -> ThemeLibrary {
        let mut library = ThemeLibrary {
            user_dir,
            builtin: builtin_themes().into_iter().map(Arc::new).collect(),
            user: Vec::new(),
            warnings: Vec::new(),
        };
        library.reload();
        library
    }

    /// Reads the user directory again.
    pub fn reload(&mut self) {
        self.user.clear();
        self.warnings.clear();
        let Some(dir) = self.user_dir.clone() else {
            return;
        };
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| is_theme_file(p)).collect();
        files.sort();
        for path in files {
            match std::fs::read(&path)
                .map_err(|e| ThemeError::io(&path, e))
                .and_then(|bytes| Theme::from_plist(&bytes))
            {
                Ok(mut theme) => {
                    if let Some(stem) = path.file_stem().and_then(OsStr::to_str)
                        && (theme.name == "Untitled" || theme.name == "Imported theme")
                    {
                        theme.name = stem.to_owned();
                    }
                    theme.merge_default_items();
                    theme.origin = ThemeOrigin::User(path);
                    if self.user.iter().any(|t| t.name == theme.name) {
                        self.warnings.push(format!("duplicate theme name `{}` ignored", theme.name));
                    } else {
                        self.user.push(Arc::new(theme));
                    }
                }
                Err(error) => self.warnings.push(error.to_string()),
            }
        }
    }

    /// Problems found while reading the user directory.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The user themes directory.
    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    /// The theme rows: built-ins in their order, then user-only themes by
    /// name.
    pub fn entries(&self) -> Vec<ThemeEntry> {
        let mut out: Vec<ThemeEntry> = self
            .builtin
            .iter()
            .map(|b| {
                let user = self.user.iter().find(|u| u.name == b.name);
                ThemeEntry {
                    name: b.name.clone(),
                    is_dark: user.map_or_else(|| b.is_dark(), |u| u.is_dark()),
                    is_builtin: true,
                    is_user: user.is_some(),
                }
            })
            .collect();
        let mut extra: Vec<&Arc<Theme>> = self
            .user
            .iter()
            .filter(|u| !self.builtin.iter().any(|b| b.name == u.name))
            .collect();
        extra.sort_by_key(|t| t.name.to_lowercase());
        out.extend(extra.into_iter().map(|u| ThemeEntry {
            name: u.name.clone(),
            is_dark: u.is_dark(),
            is_builtin: false,
            is_user: true,
        }));
        out
    }

    /// The theme named `name`: a user file wins over a built-in of the same
    /// name.
    pub fn get(&self, name: &str) -> Option<Arc<Theme>> {
        self.user
            .iter()
            .chain(&self.builtin)
            .find(|t| t.name == name)
            .cloned()
    }

    /// Like [`ThemeLibrary::get`] but falls back to the default theme of the
    /// requested brightness, then to any theme (never fails).
    pub fn get_or_default(&self, name: &str, dark: bool) -> Arc<Theme> {
        self.get(name)
            .or_else(|| self.get(if dark { DEFAULT_DARK } else { DEFAULT_LIGHT }))
            .or_else(|| self.builtin.first().cloned())
            .unwrap_or_else(|| Arc::new(Theme::template(if dark { DEFAULT_DARK } else { DEFAULT_LIGHT }, dark)))
    }

    /// A name based on `base` that no theme uses (`base`, `base 2`, ...).
    pub fn unique_name(&self, base: &str) -> String {
        let mut candidate = base.to_owned();
        let mut n = 2;
        while self.get(&candidate).is_some() {
            candidate = format!("{base} {n}");
            n += 1;
        }
        candidate
    }

    fn dir(&self) -> Result<&Path, ThemeError> {
        self.user_dir.as_deref().ok_or(ThemeError::NoUserDir)
    }

    /// Writes `theme` to the user directory as `<name>.tmTheme` (creating the
    /// directory) and makes it the library's theme of that name. Saving a
    /// built-in's name creates a user copy that overrides it.
    ///
    /// # Errors
    /// [`ThemeError::InvalidName`], [`ThemeError::NoUserDir`] or
    /// [`ThemeError::Io`].
    pub fn save(&mut self, mut theme: Theme) -> Result<Arc<Theme>, ThemeError> {
        let file = file_name_for(&theme.name)?;
        let dir = self.dir()?.to_owned();
        std::fs::create_dir_all(&dir).map_err(|e| ThemeError::io(&dir, e))?;
        let path = dir.join(file);
        let xml = theme.to_plist_xml()?;
        std::fs::write(&path, xml).map_err(|e| ThemeError::io(&path, e))?;
        theme.origin = ThemeOrigin::User(path);
        let theme = Arc::new(theme);
        self.user.retain(|t| t.name != theme.name);
        self.user.push(theme.clone());
        Ok(theme)
    }

    /// "New Light Theme" / "New Dark Theme": a saved template named `name`.
    ///
    /// # Errors
    /// [`ThemeError::Exists`] or what [`ThemeLibrary::save`] returns.
    pub fn new_theme(&mut self, name: &str, dark: bool) -> Result<Arc<Theme>, ThemeError> {
        if self.get(name).is_some() {
            return Err(ThemeError::Exists(name.to_owned()));
        }
        self.save(Theme::template(name, dark))
    }

    /// "Duplicate": copies `name` to a new user theme `new_name`.
    ///
    /// # Errors
    /// [`ThemeError::NotFound`], [`ThemeError::Exists`] or save errors.
    pub fn duplicate(&mut self, name: &str, new_name: &str) -> Result<Arc<Theme>, ThemeError> {
        let source = self.get(name).ok_or_else(|| ThemeError::NotFound(name.to_owned()))?;
        if self.get(new_name).is_some() {
            return Err(ThemeError::Exists(new_name.to_owned()));
        }
        self.save(source.duplicate(new_name))
    }

    /// "Invert": a lightness-inverted copy of `name` saved as `new_name`.
    ///
    /// # Errors
    /// As [`ThemeLibrary::duplicate`].
    pub fn invert(&mut self, name: &str, new_name: &str) -> Result<Arc<Theme>, ThemeError> {
        let source = self.get(name).ok_or_else(|| ThemeError::NotFound(name.to_owned()))?;
        if self.get(new_name).is_some() {
            return Err(ThemeError::Exists(new_name.to_owned()));
        }
        self.save(source.inverted(new_name))
    }

    /// "Rename": renames a user theme (and its file).
    ///
    /// # Errors
    /// [`ThemeError::Builtin`] for a theme that is not a user file,
    /// [`ThemeError::Exists`], or I/O errors.
    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<Arc<Theme>, ThemeError> {
        let theme = self
            .user
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .ok_or_else(|| {
                if self.builtin.iter().any(|b| b.name == name) {
                    ThemeError::Builtin("renamed")
                } else {
                    ThemeError::NotFound(name.to_owned())
                }
            })?;
        if self.get(new_name).is_some() {
            return Err(ThemeError::Exists(new_name.to_owned()));
        }
        let mut renamed = (*theme).clone();
        renamed.name = new_name.to_owned();
        let saved = self.save(renamed)?;
        if let ThemeOrigin::User(old) = &theme.origin
            && ThemeOrigin::User(old.clone()) != saved.origin
        {
            std::fs::remove_file(old).map_err(|e| ThemeError::io(old, e))?;
        }
        self.user.retain(|t| t.name != name);
        Ok(saved)
    }

    /// The file backing user theme `name` (for "Show in Finder" or moving it
    /// to the Trash before [`ThemeLibrary::delete`]).
    pub fn user_path(&self, name: &str) -> Option<PathBuf> {
        match &self.user.iter().find(|t| t.name == name)?.origin {
            ThemeOrigin::User(p) if !p.as_os_str().is_empty() => Some(p.clone()),
            _ => None,
        }
    }

    /// "Delete" for a user theme, "Reset" for a built-in the user changed:
    /// removes the user file. A built-in with no user copy cannot be deleted.
    ///
    /// # Errors
    /// [`ThemeError::Builtin`], [`ThemeError::NotFound`] or I/O errors.
    pub fn delete(&mut self, name: &str) -> Result<(), ThemeError> {
        match self.user_path(name) {
            Some(path) => {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(ThemeError::io(&path, e)),
                }
                self.user.retain(|t| t.name != name);
                Ok(())
            }
            None if self.builtin.iter().any(|b| b.name == name) => Err(ThemeError::Builtin("deleted")),
            None => Err(ThemeError::NotFound(name.to_owned())),
        }
    }

    /// "Reset All": removes every user copy of a built-in theme.
    ///
    /// # Errors
    /// The first I/O error.
    pub fn reset_all(&mut self) -> Result<usize, ThemeError> {
        let names: Vec<String> = self
            .builtin
            .iter()
            .filter(|b| self.user.iter().any(|u| u.name == b.name))
            .map(|b| b.name.clone())
            .collect();
        for name in &names {
            self.delete(name)?;
        }
        Ok(names.len())
    }

    /// "Import...": reads a `.tmTheme` (or legacy `.crTheme`), merges the
    /// default console items, gives it a unique name and saves it.
    ///
    /// # Errors
    /// I/O, format and save errors.
    pub fn import(&mut self, path: &Path) -> Result<Arc<Theme>, ThemeError> {
        let bytes = std::fs::read(path).map_err(|e| ThemeError::io(path, e))?;
        let mut theme = Theme::from_plist(&bytes)?;
        if (theme.name == "Untitled" || theme.name == "Imported theme")
            && let Some(stem) = path.file_stem().and_then(OsStr::to_str)
        {
            theme.name = stem.to_owned();
        }
        theme.merge_default_items();
        theme.name = self.unique_name(&theme.name);
        self.save(theme)
    }

    /// "Export...": writes `name` as a `.tmTheme`. `destination` is a file
    /// path, or a directory (then `<name>.tmTheme` is created in it).
    /// Returns the written path.
    ///
    /// # Errors
    /// [`ThemeError::NotFound`], name or I/O errors.
    pub fn export(&self, name: &str, destination: &Path) -> Result<PathBuf, ThemeError> {
        let theme = self.get(name).ok_or_else(|| ThemeError::NotFound(name.to_owned()))?;
        let path = if destination.is_dir() {
            destination.join(file_name_for(name)?)
        } else {
            destination.to_owned()
        };
        std::fs::write(&path, theme.to_plist_xml()?).map_err(|e| ThemeError::io(&path, e))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> (tempfile::TempDir, ThemeLibrary) {
        let dir = tempfile::tempdir().expect("tempdir");
        let lib = ThemeLibrary::new(Some(dir.path().join("Themes")));
        (dir, lib)
    }

    #[test]
    fn lists_builtins_first_with_defaults() {
        let (_d, lib) = library();
        let entries = lib.entries();
        assert_eq!(entries[0].name, DEFAULT_LIGHT);
        assert_eq!(entries[1].name, DEFAULT_DARK);
        assert!(entries[1].is_dark && !entries[0].is_dark);
        assert!(entries.len() >= 10);
    }

    #[test]
    fn duplicate_rename_invert_delete() {
        let (_d, mut lib) = library();
        let copy = lib.duplicate(DEFAULT_DARK, "Mine").expect("duplicate");
        assert_eq!(copy.name, "Mine");
        assert!(lib.user_path("Mine").is_some_and(|p| p.exists()));
        assert!(matches!(lib.duplicate(DEFAULT_DARK, "Mine"), Err(ThemeError::Exists(_))));
        let inv = lib.invert("Mine", "Mine Inverted").expect("invert");
        assert!(!inv.is_dark());
        lib.rename("Mine", "Yours").expect("rename");
        assert!(lib.get("Mine").is_none());
        assert!(lib.user_path("Yours").is_some_and(|p| p.exists()));
        assert!(!lib.user_dir().map(|d| d.join("Mine.tmTheme")).is_some_and(|p| p.exists()));
        lib.delete("Yours").expect("delete");
        assert!(lib.get("Yours").is_none());
        assert!(matches!(lib.delete(DEFAULT_DARK), Err(ThemeError::Builtin(_))));
        assert!(matches!(lib.rename(DEFAULT_DARK, "x"), Err(ThemeError::Builtin(_))));
    }

    #[test]
    fn user_copy_overrides_builtin_until_reset() {
        let (_d, mut lib) = library();
        let mut edited = (*lib.get("Monokai").expect("monokai")).clone();
        edited.global.background = crate::Rgba::parse("#101010");
        lib.save(edited).expect("save");
        assert_eq!(lib.get("Monokai").map(|t| t.background()), crate::Rgba::parse("#101010"));
        assert_eq!(lib.entries().iter().filter(|e| e.name == "Monokai").count(), 1);
        lib.delete("Monokai").expect("reset");
        assert_eq!(lib.get("Monokai").map(|t| t.background()), crate::Rgba::parse("#272822"));
    }

    #[test]
    fn reload_picks_up_files_and_reports_bad_ones() {
        let (_d, mut lib) = library();
        lib.duplicate(DEFAULT_LIGHT, "Disk").expect("duplicate");
        let dir = lib.user_dir().map(Path::to_owned).expect("dir");
        std::fs::write(dir.join("broken.tmTheme"), "not a plist").expect("write");
        let fresh = ThemeLibrary::new(Some(dir));
        assert!(fresh.get("Disk").is_some());
        assert_eq!(fresh.warnings().len(), 1);
    }

    #[test]
    fn import_export_round_trip_with_unique_names() {
        let (d, mut lib) = library();
        let out = lib.export("Nord", d.path()).expect("export");
        assert!(out.exists());
        let imported = lib.import(&out).expect("import");
        assert_eq!(imported.name, "Nord 2");
        assert_eq!(imported.background(), lib.get("Nord").expect("nord").background());
        assert_eq!(imported.items.len(), lib.get("Nord").expect("nord").items.len());
    }

    #[test]
    fn no_user_dir_still_serves_builtins() {
        let lib = ThemeLibrary::new(None);
        assert!(lib.get(DEFAULT_DARK).is_some());
        let mut lib = lib;
        assert!(matches!(lib.duplicate(DEFAULT_DARK, "x"), Err(ThemeError::NoUserDir)));
    }

    #[test]
    fn bad_names_are_rejected() {
        let (_d, mut lib) = library();
        assert!(matches!(lib.new_theme("  ", true), Err(ThemeError::InvalidName(_))));
        let ok = lib.new_theme("a/b: c", false).expect("sanitised");
        assert!(lib.user_path(&ok.name).is_some_and(|p| p.ends_with("a-b- c.tmTheme")));
    }
}
