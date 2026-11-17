//! [`LanguageRegistry`]: the built-in languages plus the user's overlay.
//!
//! ```no_run
//! use std::path::Path;
//! use scratch_language::{DetectInput, LanguageRegistry};
//!
//! let mut registry = LanguageRegistry::builtin();
//! let report = registry.load_user_dir("/path/to/Languages").unwrap();
//! for (path, error) in &report.errors {
//!     eprintln!("skipped {}: {error}", path.display());
//! }
//! let found = registry.detect(&DetectInput::new().path(Path::new("main.rs")));
//! assert_eq!(found.language.id, "rust");
//! ```
//!
//! # The overlay model
//!
//! Every `*.toml` file of the user directory is one language. A file whose
//! `id` is a built-in's id **replaces** that built-in ("override"; this is how
//! editing a bundled language works, like CodeRunner's per-user copies);
//! [`LanguageRegistry::reset`] drops the override and restores the shipped
//! definition. Files with new ids are custom languages. Toggling `enabled` of a
//! built-in is an edit like any other (it is stored as an override).
//!
//! Use statistics are separate ([`LanguageRegistry::usage`]) and not persisted
//! by the registry.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::builtin::parse_builtins;
use crate::detect::{
    DetectInput, DetectReason, Detection, Guess, extension_candidates, interpreter_matches,
    modeline_hints, parse_content_pattern, shebang_interpreter,
};
use crate::error::LanguageError;
use crate::import::{import_cr_language_path, slugify};
use crate::model::{LanguageDef, Severity};
use crate::template::{
    RenderedTemplate, TemplateContext, TemplateDef, TemplateGroup, group_templates, render_template,
};
use crate::usage::LanguageUsage;

/// The id of the plain text language (always present, cannot be disabled or
/// deleted).
pub const PLAIN_TEXT_ID: &str = "plain-text";

/// What [`LanguageRegistry::load_user_dir`] found.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Ids of custom languages that were added.
    pub added: Vec<String>,
    /// Ids of built-ins that a file replaced.
    pub overridden: Vec<String>,
    /// Files that were skipped and why.
    pub errors: Vec<(PathBuf, LanguageError)>,
}

/// How menus order languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOrder {
    /// By name, case-insensitive.
    Alphabetical,
    /// Most recently used first, then alphabetical.
    MostRecentlyUsed,
    /// Most used first (ties by recency), then alphabetical.
    MostUsed,
}

/// An entry of a language menu.
#[derive(Debug, Clone)]
pub enum MenuEntry<'a> {
    /// A language shown directly.
    Language(&'a LanguageDef),
    /// A submenu of similar languages (`JavaScript` with Browser, jQuery,
    /// Node.js).
    Group {
        /// The group title.
        name: String,
        /// The members, in menu order. Use [`LanguageDef::variant_name`] for
        /// their titles inside the submenu.
        languages: Vec<&'a LanguageDef>,
    },
}

impl MenuEntry<'_> {
    /// The title of the entry.
    pub fn title(&self) -> &str {
        match self {
            MenuEntry::Language(language) => &language.name,
            MenuEntry::Group { name, .. } => name,
        }
    }
}

#[derive(Debug, Default)]
struct Index {
    by_id: HashMap<String, usize>,
    by_uuid: HashMap<String, usize>,
    by_extension: HashMap<String, Vec<usize>>,
    by_filename: HashMap<String, Vec<usize>>,
    patterns: Vec<Vec<(u32, Regex)>>,
}

/// The built-in languages plus the user's overlay directory. See the module
/// documentation.
#[derive(Debug)]
pub struct LanguageRegistry {
    langs: Vec<LanguageDef>,
    pristine: Vec<LanguageDef>,
    overridden: BTreeSet<String>,
    user_dir: Option<PathBuf>,
    usage: LanguageUsage,
    default_id: String,
    index: Index,
}

impl LanguageRegistry {
    /// A registry with the built-in definitions only.
    pub fn builtin() -> Self {
        let mut pristine = Vec::new();
        for parsed in parse_builtins() {
            match parsed {
                Ok(def) => pristine.push(def),
                Err(error) => tracing::error!("built-in language skipped: {error}"),
            }
        }
        if !pristine.iter().any(|d| d.id == PLAIN_TEXT_ID) {
            let mut plain = LanguageDef::new(PLAIN_TEXT_ID, "Plain Text");
            plain.builtin = true;
            plain.extensions = vec!["txt".to_owned()];
            pristine.insert(0, plain);
        }
        let mut registry = Self {
            langs: pristine.clone(),
            pristine,
            overridden: BTreeSet::new(),
            user_dir: None,
            usage: LanguageUsage::default(),
            default_id: PLAIN_TEXT_ID.to_owned(),
            index: Index::default(),
        };
        registry.rebuild_index();
        registry
    }

    // ---------------------------------------------------------------- lookup

    /// Every language (enabled or not), built-ins first in their menu order,
    /// then custom languages in load order.
    pub fn all(&self) -> &[LanguageDef] {
        &self.langs
    }

    /// The enabled languages.
    pub fn enabled(&self) -> impl Iterator<Item = &LanguageDef> {
        self.langs.iter().filter(|l| l.enabled)
    }

    /// The language with this id.
    pub fn get(&self, id: &str) -> Option<&LanguageDef> {
        self.index.by_id.get(id).map(|&i| &self.langs[i])
    }

    /// The language with this CodeRunner uuid (case-insensitive).
    pub fn by_uuid(&self, uuid: &str) -> Option<&LanguageDef> {
        self.index
            .by_uuid
            .get(&uuid.to_ascii_lowercase())
            .map(|&i| &self.langs[i])
    }

    /// The language with this display name (case-insensitive).
    pub fn by_name(&self, name: &str) -> Option<&LanguageDef> {
        self.langs
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Finds a language by id, then uuid, then display name.
    pub fn find(&self, query: &str) -> Option<&LanguageDef> {
        let query = query.trim();
        self.get(query)
            .or_else(|| self.by_uuid(query))
            .or_else(|| self.by_name(query))
    }

    /// The plain text language.
    pub fn plain_text(&self) -> &LanguageDef {
        self.default_language_or_plain(PLAIN_TEXT_ID)
    }

    /// The language detection falls back to (plain text unless changed).
    pub fn default_language(&self) -> &LanguageDef {
        self.default_language_or_plain(&self.default_id)
    }

    fn default_language_or_plain(&self, id: &str) -> &LanguageDef {
        self.get(id)
            .or_else(|| self.get(PLAIN_TEXT_ID))
            .unwrap_or_else(|| &self.langs[0])
    }

    /// Changes the language detection falls back to.
    ///
    /// # Errors
    ///
    /// [`LanguageError::NotFound`] for an unknown id.
    pub fn set_default_language(&mut self, id: &str) -> Result<(), LanguageError> {
        if self.get(id).is_none() {
            return Err(LanguageError::NotFound(id.to_owned()));
        }
        self.default_id = id.to_owned();
        Ok(())
    }

    /// Whether the language is a built-in (possibly overridden).
    pub fn is_builtin(&self, id: &str) -> bool {
        self.pristine.iter().any(|d| d.id == id)
    }

    /// Whether a user file replaces this built-in.
    pub fn is_overridden(&self, id: &str) -> bool {
        self.overridden.contains(id)
    }

    /// The shipped definition of a built-in, ignoring overrides.
    pub fn pristine(&self, id: &str) -> Option<&LanguageDef> {
        self.pristine.iter().find(|d| d.id == id)
    }

    // ----------------------------------------------------------------- usage

    /// The use statistics.
    pub fn usage(&self) -> &LanguageUsage {
        &self.usage
    }

    /// Replaces the use statistics (load them from your settings at startup).
    pub fn set_usage(&mut self, usage: LanguageUsage) {
        self.usage = usage;
    }

    /// Records that `id` was used (a document set to it, a run).
    pub fn record_use(&mut self, id: &str) {
        self.usage.record(id);
    }

    // ----------------------------------------------------------------- menus

    /// The language menu: enabled languages ordered by `order`. With `group`
    /// languages that share a group name (`JavaScript (Browser)`,
    /// `JavaScript (Node.js)`) become one submenu entry (CodeRunner's "Group
    /// similar languages"); a group with a single member stays a plain entry.
    /// A group is placed where its first member would be.
    pub fn menu_entries(&self, group: bool, order: MenuOrder) -> Vec<MenuEntry<'_>> {
        let mut sorted: Vec<&LanguageDef> = self.enabled().collect();
        sorted.sort_by(|a, b| {
            let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
            match order {
                MenuOrder::Alphabetical => by_name,
                MenuOrder::MostRecentlyUsed => self.usage.most_recent(&b.id, &a.id).then(by_name),
                MenuOrder::MostUsed => self.usage.most_used(&b.id, &a.id).then(by_name),
            }
        });
        if !group {
            return sorted.into_iter().map(MenuEntry::Language).collect();
        }
        let mut slots: Vec<(&str, Vec<&LanguageDef>)> = Vec::new();
        for language in sorted {
            let name = language.group_name();
            match slots.iter_mut().find(|(group, _)| *group == name) {
                Some((_, members)) => members.push(language),
                None => slots.push((name, vec![language])),
            }
        }
        slots
            .into_iter()
            .map(|(name, mut members)| {
                if members.len() == 1 {
                    MenuEntry::Language(members.remove(0))
                } else {
                    MenuEntry::Group {
                        name: name.to_owned(),
                        languages: members,
                    }
                }
            })
            .collect()
    }

    // ------------------------------------------------------------- detection

    /// Detects the language of a document. See [`crate::detect`] for the
    /// precedence. Never fails: the default language is the fallback.
    pub fn detect(&self, input: &DetectInput<'_>) -> Detection<'_> {
        if let Some(query) = input.explicit
            && let Some(language) = self.find(query)
        {
            return Detection {
                language,
                reason: DetectReason::Explicit,
                candidates: Vec::new(),
            };
        }
        if let Some(path) = input.path
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            for ext in extension_candidates(name) {
                let claimants: Vec<&LanguageDef> = self
                    .index
                    .by_extension
                    .get(&ext)
                    .into_iter()
                    .flatten()
                    .map(|&i| &self.langs[i])
                    .filter(|l| l.enabled)
                    .collect();
                if let Some(winner) = claimants
                    .iter()
                    .find(|l| l.default_for.contains(&ext))
                    .or_else(|| claimants.first())
                {
                    let winner: &LanguageDef = winner;
                    return Detection {
                        language: winner,
                        reason: DetectReason::Extension,
                        candidates: claimants
                            .iter()
                            .copied()
                            .filter(|l| l.id != winner.id)
                            .collect(),
                    };
                }
            }
            if let Some(&i) = self
                .index
                .by_filename
                .get(name)
                .into_iter()
                .flatten()
                .find(|&&i| self.langs[i].enabled)
            {
                return Detection {
                    language: &self.langs[i],
                    reason: DetectReason::FileName,
                    candidates: Vec::new(),
                };
            }
        }
        if let Some(text) = input.text {
            if let Some(language) = self.by_shebang(text) {
                return Detection {
                    language,
                    reason: DetectReason::Shebang,
                    candidates: Vec::new(),
                };
            }
            if let Some(language) = self.by_modeline(text) {
                return Detection {
                    language,
                    reason: DetectReason::Modeline,
                    candidates: Vec::new(),
                };
            }
        }
        Detection {
            language: self.default_language(),
            reason: DetectReason::Fallback,
            candidates: Vec::new(),
        }
    }

    fn by_shebang(&self, text: &str) -> Option<&LanguageDef> {
        let interpreter = shebang_interpreter(text.lines().next()?)?;
        let enabled = || self.langs.iter().filter(|l| l.enabled);
        enabled()
            .find(|l| l.shebangs.contains(&interpreter))
            .or_else(|| {
                enabled().find(|l| {
                    l.shebangs
                        .iter()
                        .any(|s| interpreter_matches(&interpreter, s))
                })
            })
    }

    fn by_modeline(&self, text: &str) -> Option<&LanguageDef> {
        modeline_hints(text).into_iter().find_map(|hint| {
            self.enabled().find(|l| {
                l.modelines.iter().any(|m| m.eq_ignore_ascii_case(&hint))
                    || l.id == hint
                    || l.name.eq_ignore_ascii_case(&hint)
            })
        })
    }

    /// Guesses the language of pasted text ("New from Clipboard"): shebang,
    /// then modeline, then the languages' content patterns. `None` when
    /// nothing convincing was found (the caller then uses its default).
    pub fn guess_from_text(&self, text: &str) -> Option<Guess<'_>> {
        if let Some(language) = self.by_shebang(text) {
            return Some(Guess {
                language,
                reason: DetectReason::Shebang,
                score: 0,
            });
        }
        if let Some(language) = self.by_modeline(text) {
            return Some(Guess {
                language,
                reason: DetectReason::Modeline,
                score: 0,
            });
        }
        let mut scores: Vec<(usize, u32)> = self
            .index
            .patterns
            .iter()
            .enumerate()
            .filter(|(i, _)| self.langs[*i].enabled)
            .map(|(i, patterns)| {
                let score = patterns
                    .iter()
                    .filter(|(_, regex)| regex.is_match(text))
                    .map(|(weight, _)| *weight)
                    .sum();
                (i, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect();
        scores.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let (best, best_score) = *scores.first()?;
        let second = scores.get(1).map_or(0, |s| s.1);
        (best_score >= 2 && best_score > second).then(|| Guess {
            language: &self.langs[best],
            reason: DetectReason::Content,
            score: best_score,
        })
    }

    // ------------------------------------------------------------- templates

    /// The templates of a language, grouped by name (multi-file templates).
    pub fn templates_for(&self, id: &str) -> Vec<TemplateGroup<'_>> {
        self.get(id)
            .map(|l| group_templates(&l.templates))
            .unwrap_or_default()
    }

    /// The default template item of a language, if it has one.
    pub fn default_template(&self, id: &str) -> Option<&TemplateDef> {
        self.get(id)?.templates.iter().find(|t| t.default)
    }

    /// The initial content of a new document of the language: its default
    /// template rendered, or empty text when it has none.
    pub fn new_document(&self, id: &str, ctx: &TemplateContext) -> RenderedTemplate {
        match self.default_template(id) {
            Some(template) => render_template(&template.text, ctx),
            None => render_template("", ctx),
        }
    }

    // ------------------------------------------------------------ user files

    /// The user directory, if one is set.
    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    /// Sets the user directory (created on first save) without loading it.
    pub fn set_user_dir(&mut self, dir: impl Into<PathBuf>) {
        self.user_dir = Some(dir.into());
    }

    /// Sets the user directory and loads every `*.toml` in it. A missing
    /// directory is fine (nothing to load). Files that fail to parse or
    /// validate are skipped and listed in the report.
    ///
    /// # Errors
    ///
    /// [`LanguageError::Io`] when the directory exists but cannot be listed.
    pub fn load_user_dir(&mut self, dir: impl Into<PathBuf>) -> Result<LoadReport, LanguageError> {
        self.user_dir = Some(dir.into());
        self.reload_user_dir()
    }

    /// Discards all overrides and custom languages and loads the user
    /// directory again.
    ///
    /// # Errors
    ///
    /// [`LanguageError::NoUserDirectory`] without a directory;
    /// [`LanguageError::Io`] when it cannot be listed.
    pub fn reload_user_dir(&mut self) -> Result<LoadReport, LanguageError> {
        let dir = self
            .user_dir
            .clone()
            .ok_or(LanguageError::NoUserDirectory)?;
        self.langs = self.pristine.clone();
        self.overridden.clear();
        let mut report = LoadReport::default();
        if !dir.exists() {
            self.rebuild_index();
            return Ok(report);
        }
        let entries =
            std::fs::read_dir(&dir).map_err(|e| LanguageError::io("read directory", &dir, e))?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            let result = std::fs::read_to_string(&path)
                .map_err(|e| LanguageError::io("read", &path, e))
                .and_then(|text| {
                    LanguageDef::from_toml_with_origin(&text, Some(&path.display().to_string()))
                })
                .and_then(|def| self.apply_user_def(def));
            match result {
                Ok((id, was_override)) => {
                    if was_override {
                        report.overridden.push(id);
                    } else {
                        report.added.push(id);
                    }
                }
                Err(error) => report.errors.push((path, error)),
            }
        }
        self.rebuild_index();
        Ok(report)
    }

    fn check_def(&self, def: &LanguageDef) -> Result<(), LanguageError> {
        let problems: Vec<String> = def
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.message)
            .collect();
        if !problems.is_empty() {
            return Err(LanguageError::Invalid {
                id: def.id.clone(),
                problems,
            });
        }
        if let Some(uuid) = &def.uuid {
            let clash = self.langs.iter().find(|l| {
                l.id != def.id
                    && l.uuid
                        .as_deref()
                        .is_some_and(|u| u.eq_ignore_ascii_case(uuid))
            });
            if let Some(other) = clash {
                return Err(LanguageError::Invalid {
                    id: def.id.clone(),
                    problems: vec![format!("uuid {uuid} is already used by `{}`", other.id)],
                });
            }
        }
        Ok(())
    }

    /// Validates and inserts a definition read from a user file. Returns the
    /// id and whether it replaced a built-in. The index is not rebuilt.
    fn apply_user_def(&mut self, mut def: LanguageDef) -> Result<(String, bool), LanguageError> {
        self.check_def(&def)?;
        let is_builtin = self.is_builtin(&def.id);
        let existing = self.langs.iter().position(|l| l.id == def.id);
        match (existing, is_builtin) {
            (Some(pos), true) => {
                def.builtin = true;
                self.langs[pos] = def;
                self.overridden.insert(self.langs[pos].id.clone());
                Ok((self.langs[pos].id.clone(), true))
            }
            (Some(_), false) => Err(LanguageError::Duplicate(def.id)),
            (None, _) => {
                def.builtin = false;
                let id = def.id.clone();
                self.langs.push(def);
                Ok((id, false))
            }
        }
    }

    fn file_for(&self, id: &str) -> Result<PathBuf, LanguageError> {
        let dir = self
            .user_dir
            .as_ref()
            .ok_or(LanguageError::NoUserDirectory)?;
        Ok(dir.join(format!("{id}.toml")))
    }

    fn write_user_file(&self, def: &LanguageDef) -> Result<(), LanguageError> {
        let path = self.file_for(&def.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| LanguageError::io("create directory", parent, e))?;
        }
        let text = def.to_toml_string()?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| LanguageError::io("write", &tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| LanguageError::io("rename to", &path, e))
    }

    fn remove_user_file(&self, id: &str) -> Result<(), LanguageError> {
        let Ok(path) = self.file_for(id) else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LanguageError::io("remove", path, e)),
        }
    }

    /// Adds or replaces a language and, when a user directory is set, writes
    /// it there. Saving a built-in stores an override; saving a built-in
    /// that equals its shipped definition removes the override instead.
    ///
    /// # Errors
    ///
    /// [`LanguageError::Invalid`] for an invalid definition, I/O errors.
    pub fn save(&mut self, mut def: LanguageDef) -> Result<(), LanguageError> {
        self.check_def(&def)?;
        let pristine = self.pristine(&def.id).cloned();
        def.builtin = pristine.is_some();
        let unchanged = pristine.as_ref() == Some(&def);
        if unchanged {
            self.remove_user_file(&def.id)?;
            self.overridden.remove(&def.id);
        } else {
            if self.user_dir.is_some() {
                self.write_user_file(&def)?;
            }
            if def.builtin {
                self.overridden.insert(def.id.clone());
            }
        }
        match self.langs.iter().position(|l| l.id == def.id) {
            Some(pos) => self.langs[pos] = def,
            None => self.langs.push(def),
        }
        self.rebuild_index();
        Ok(())
    }

    /// Enables or disables a language (stored like any other edit).
    ///
    /// # Errors
    ///
    /// [`LanguageError::NotFound`]; plain text cannot be disabled.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), LanguageError> {
        let mut def = self
            .get(id)
            .cloned()
            .ok_or_else(|| LanguageError::NotFound(id.to_owned()))?;
        if id == PLAIN_TEXT_ID && !enabled {
            return Err(LanguageError::Invalid {
                id: id.to_owned(),
                problems: vec!["plain text cannot be disabled".to_owned()],
            });
        }
        if def.enabled == enabled {
            return Ok(());
        }
        def.enabled = enabled;
        self.save(def)
    }

    /// Restores a built-in to its shipped definition (drops the override and
    /// deletes its file).
    ///
    /// # Errors
    ///
    /// [`LanguageError::NotBundled`] for a custom language.
    pub fn reset(&mut self, id: &str) -> Result<(), LanguageError> {
        let Some(pristine) = self.pristine(id).cloned() else {
            return Err(LanguageError::NotBundled(id.to_owned()));
        };
        self.remove_user_file(id)?;
        self.overridden.remove(id);
        if let Some(pos) = self.langs.iter().position(|l| l.id == id) {
            self.langs[pos] = pristine;
        }
        self.rebuild_index();
        Ok(())
    }

    /// Restores every built-in ("Reset All"). Custom languages are kept.
    ///
    /// # Errors
    ///
    /// I/O errors while deleting override files.
    pub fn reset_all(&mut self) -> Result<(), LanguageError> {
        let ids: Vec<String> = self.overridden.iter().cloned().collect();
        for id in ids {
            self.reset(&id)?;
        }
        Ok(())
    }

    /// Deletes a custom language (and its file).
    ///
    /// # Errors
    ///
    /// [`LanguageError::BundledLanguage`] for built-ins (disable them or
    /// [`LanguageRegistry::reset`] instead), [`LanguageError::NotFound`].
    pub fn delete(&mut self, id: &str) -> Result<(), LanguageError> {
        if self.is_builtin(id) {
            return Err(LanguageError::BundledLanguage(id.to_owned()));
        }
        let pos = self
            .langs
            .iter()
            .position(|l| l.id == id)
            .ok_or_else(|| LanguageError::NotFound(id.to_owned()))?;
        self.remove_user_file(id)?;
        self.langs.remove(pos);
        self.usage.remove(id);
        self.rebuild_index();
        Ok(())
    }

    /// Copies a language under a new name ("Duplicate"): a custom language
    /// with a fresh id, no uuid, enabled. Returns its id.
    ///
    /// # Errors
    ///
    /// [`LanguageError::NotFound`], plus whatever [`LanguageRegistry::save`] returns.
    pub fn duplicate(&mut self, id: &str, new_name: &str) -> Result<String, LanguageError> {
        let mut copy = self
            .get(id)
            .cloned()
            .ok_or_else(|| LanguageError::NotFound(id.to_owned()))?;
        copy.id = self.unique_id(&slugify(new_name));
        copy.name = new_name.to_owned();
        copy.uuid = None;
        copy.group = None;
        copy.variant = None;
        copy.builtin = false;
        copy.enabled = true;
        // A copy would otherwise compete for the extensions of its original.
        copy.default_for.clear();
        let new_id = copy.id.clone();
        self.save(copy)?;
        Ok(new_id)
    }

    /// A free id based on `base`: `base`, `base-2`, `base-3`, ...
    pub fn unique_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "language" } else { base };
        if self.get(base).is_none() {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| self.get(id).is_none())
            .unwrap_or_else(|| base.to_owned())
    }

    /// The TOML text of a language (export).
    ///
    /// # Errors
    ///
    /// [`LanguageError::NotFound`] or a serialization error.
    pub fn export_toml(&self, id: &str) -> Result<String, LanguageError> {
        self.get(id)
            .ok_or_else(|| LanguageError::NotFound(id.to_owned()))?
            .to_toml_string()
    }

    /// Imports a CodeRunner `.crLanguage` bundle directory (or a bare
    /// `Language.plist` file). A bundle whose
    /// uuid belongs to a built-in replaces that built-in (as importing over a
    /// bundled language does in CodeRunner); one whose uuid belongs to a custom
    /// language replaces it; otherwise it becomes a new custom language.
    /// Returns the id and any import warnings.
    ///
    /// # Errors
    ///
    /// Import and save errors.
    pub fn import_cr_language(
        &mut self,
        bundle: &Path,
    ) -> Result<(String, Vec<String>), LanguageError> {
        let imported = import_cr_language_path(bundle)?;
        let mut def = imported.def;
        let existing = def
            .uuid
            .as_deref()
            .and_then(|u| self.by_uuid(u))
            .map(|l| l.id.clone());
        def.id = match existing {
            Some(id) => id,
            None => self.unique_id(&def.id),
        };
        let id = def.id.clone();
        self.save(def)?;
        Ok((id, imported.warnings))
    }

    // ----------------------------------------------------------------- index

    fn rebuild_index(&mut self) {
        let mut index = Index::default();
        for (i, def) in self.langs.iter().enumerate() {
            index.by_id.insert(def.id.clone(), i);
            if let Some(uuid) = &def.uuid {
                index.by_uuid.insert(uuid.to_ascii_lowercase(), i);
            }
            for ext in &def.extensions {
                index
                    .by_extension
                    .entry(ext.to_ascii_lowercase())
                    .or_default()
                    .push(i);
            }
            for name in &def.filenames {
                index.by_filename.entry(name.clone()).or_default().push(i);
            }
            let compiled = def
                .content_patterns
                .iter()
                .filter_map(|entry| {
                    let (weight, source) = parse_content_pattern(entry);
                    match Regex::new(source) {
                        Ok(regex) => Some((weight, regex)),
                        Err(error) => {
                            tracing::warn!(
                                "language {}: bad content pattern {source:?}: {error}",
                                def.id
                            );
                            None
                        }
                    }
                })
                .collect();
            index.patterns.push(compiled);
        }
        self.index = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_path<'a>(r: &'a LanguageRegistry, path: &str) -> &'a str {
        &r.detect(&DetectInput::new().path(Path::new(path)))
            .language
            .id
    }

    fn detect_text<'a>(r: &'a LanguageRegistry, text: &str) -> &'a str {
        &r.detect(&DetectInput::new().text(text)).language.id
    }

    #[test]
    fn lookup_by_id_uuid_and_name() {
        let r = LanguageRegistry::builtin();
        assert_eq!(r.get("python").unwrap().name, "Python");
        assert_eq!(
            r.by_uuid("3cb7ee93-841d-41f6-96ee-4c8e8c6927e4")
                .unwrap()
                .id,
            "python"
        );
        assert_eq!(
            r.by_name("javascript (node.js)").unwrap().id,
            "javascript-node"
        );
        assert_eq!(r.find("Rust").unwrap().id, "rust");
        assert_eq!(r.find("rust").unwrap().id, "rust");
        assert!(r.find("cobol").is_none());
        assert_eq!(r.plain_text().id, PLAIN_TEXT_ID);
    }

    // The tests ported from the legacy `scratch_document::detect_language`.
    #[test]
    fn detects_by_extension() {
        let r = LanguageRegistry::builtin();
        for (name, expected) in [
            ("main.py", "python"),
            ("index.TSX", "typescript-node"),
            ("app.mjs", "javascript-node"),
            ("build.sh", "shell"),
            ("lib.rs", "rust"),
            ("server.go", "go"),
            ("data.json", "json"),
            ("README.md", "markdown"),
            ("notes.txt", "plain-text"),
            ("Main.java", "java"),
            ("main.swift", "swift"),
            ("a.cs", "csharp-dotnet"),
            ("a.csx", "csharp-script"),
        ] {
            assert_eq!(detect_path(&r, name), expected, "{name}");
        }
    }

    #[test]
    fn file_name_beats_nothing_and_matches_dotfiles() {
        let r = LanguageRegistry::builtin();
        assert_eq!(detect_path(&r, "/home/u/.zshrc"), "shell");
        assert_eq!(detect_path(&r, "Rakefile"), "ruby");
        let d = r.detect(&DetectInput::new().path(Path::new("/home/u/.zshrc")));
        assert_eq!(d.reason, DetectReason::FileName);
    }

    #[test]
    fn detects_by_shebang_for_unsaved_text() {
        let r = LanguageRegistry::builtin();
        assert_eq!(detect_text(&r, "#!/usr/bin/env python3"), "python");
        assert_eq!(detect_text(&r, "#!/usr/bin/python3.12\nprint(1)"), "python");
        assert_eq!(detect_text(&r, "#!/bin/zsh -e"), "shell");
        assert_eq!(
            detect_text(&r, "#!/usr/bin/env -S node --no-warnings"),
            "javascript-node"
        );
        assert_eq!(detect_text(&r, "#!/usr/bin/env bun"), "typescript-node");
        assert_eq!(detect_text(&r, "#!/usr/bin/ruby"), "ruby");
        assert_eq!(detect_text(&r, "hello"), PLAIN_TEXT_ID);
        let d = r.detect(&DetectInput::new().text("#!/bin/sh"));
        assert_eq!(d.reason, DetectReason::Shebang);
    }

    #[test]
    fn extension_beats_shebang() {
        let r = LanguageRegistry::builtin();
        let d = r.detect(
            &DetectInput::new()
                .path(Path::new("tool.py"))
                .text("#!/bin/sh"),
        );
        assert_eq!(d.language.id, "python");
        assert_eq!(d.reason, DetectReason::Extension);
    }

    #[test]
    fn explicit_beats_everything() {
        let r = LanguageRegistry::builtin();
        let d = r.detect(
            &DetectInput::new()
                .explicit("Go")
                .path(Path::new("a.py"))
                .text("#!/bin/sh"),
        );
        assert_eq!(
            (d.language.id.as_str(), d.reason),
            ("go", DetectReason::Explicit)
        );
        // an unknown explicit name falls through
        let d = r.detect(&DetectInput::new().explicit("cobol").path(Path::new("a.py")));
        assert_eq!(d.language.id, "python");
    }

    #[test]
    fn modeline_is_last_before_the_fallback() {
        let r = LanguageRegistry::builtin();
        let d = r.detect(&DetectInput::new().text("# -*- mode: ruby -*-\nputs 1"));
        assert_eq!(
            (d.language.id.as_str(), d.reason),
            ("ruby", DetectReason::Modeline)
        );
        let d = r.detect(&DetectInput::new().text("x\n// vim: set ft=go :"));
        assert_eq!(d.language.id, "go");
        // shebang beats modeline
        let d = r.detect(&DetectInput::new().text("#!/bin/sh\n# vim: ft=python"));
        assert_eq!(d.language.id, "shell");
    }

    #[test]
    fn ambiguous_extensions_use_default_for_and_report_candidates() {
        let r = LanguageRegistry::builtin();
        let d = r.detect(&DetectInput::new().path(Path::new("x.h")));
        assert_eq!(d.language.id, "c");
        let mut ids: Vec<_> = d.candidates.iter().map(|l| l.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["cpp", "objective-c", "objective-cpp"]);
        assert_eq!(detect_path(&r, "x.js"), "javascript-node");
        assert_eq!(detect_path(&r, "x.ts"), "typescript-node");
    }

    #[test]
    fn disabled_languages_are_not_detected() {
        let mut r = LanguageRegistry::builtin();
        r.set_enabled("c", false).unwrap();
        let d = r.detect(&DetectInput::new().path(Path::new("x.h")));
        assert_ne!(d.language.id, "c");
        // but an explicit choice still finds them
        assert_eq!(r.detect(&DetectInput::new().explicit("c")).language.id, "c");
        r.set_enabled("python", false).unwrap();
        assert_eq!(detect_path(&r, "a.py"), PLAIN_TEXT_ID);
    }

    #[test]
    fn plain_text_cannot_be_disabled() {
        let mut r = LanguageRegistry::builtin();
        assert!(r.set_enabled(PLAIN_TEXT_ID, false).is_err());
    }

    #[test]
    fn menu_grouping_and_ordering() {
        let mut r = LanguageRegistry::builtin();
        let flat = r.menu_entries(false, MenuOrder::Alphabetical);
        assert!(flat.iter().all(|e| matches!(e, MenuEntry::Language(_))));
        let titles: Vec<_> = flat.iter().map(MenuEntry::title).collect();
        assert!(
            titles
                .windows(2)
                .all(|w| w[0].to_lowercase() <= w[1].to_lowercase())
        );

        let grouped = r.menu_entries(true, MenuOrder::Alphabetical);
        let js = grouped.iter().find(|e| e.title() == "JavaScript").unwrap();
        let MenuEntry::Group { languages, .. } = js else {
            panic!("JavaScript should be a group")
        };
        let variants: Vec<_> = languages.iter().filter_map(|l| l.variant_name()).collect();
        assert_eq!(variants, ["Browser", "jQuery", "Node.js"]);
        let cs = grouped.iter().find(|e| e.title() == "C#").unwrap();
        assert!(matches!(cs, MenuEntry::Group { languages, .. } if languages.len() == 3));
        // single languages stay plain entries
        let py = grouped.iter().find(|e| e.title() == "Python").unwrap();
        assert!(matches!(py, MenuEntry::Language(_)));
        // a group with one enabled member collapses
        r.set_enabled("javascript-browser", false).unwrap();
        r.set_enabled("javascript-jquery", false).unwrap();
        let grouped = r.menu_entries(true, MenuOrder::Alphabetical);
        let js = grouped
            .iter()
            .find(|e| e.title() == "JavaScript (Node.js)")
            .unwrap();
        assert!(matches!(js, MenuEntry::Language(_)));
    }

    #[test]
    fn menu_by_usage() {
        let mut r = LanguageRegistry::builtin();
        r.usage.record_at("rust", 100);
        r.usage.record_at("go", 200);
        r.usage.record_at("go", 210);
        r.usage.record_at("python", 150);
        let recent: Vec<_> = r
            .menu_entries(false, MenuOrder::MostRecentlyUsed)
            .iter()
            .take(3)
            .map(|e| e.title().to_owned())
            .collect();
        assert_eq!(recent, ["Go", "Python", "Rust"]);
        let used: Vec<_> = r
            .menu_entries(false, MenuOrder::MostUsed)
            .iter()
            .take(3)
            .map(|e| e.title().to_owned())
            .collect();
        assert_eq!(used, ["Go", "Python", "Rust"]);
    }

    #[test]
    fn templates() {
        let r = LanguageRegistry::builtin();
        let groups = r.templates_for("cpp");
        let names: Vec<_> = groups.iter().map(|g| g.name).collect();
        assert!(names.contains(&"Main File") && names.contains(&"Class"));
        assert_eq!(
            groups
                .iter()
                .find(|g| g.name == "Class")
                .unwrap()
                .files
                .len(),
            2
        );
        assert!(r.default_template("go").is_some());
        assert!(r.default_template("css").is_none());

        let ctx = TemplateContext::new("Untitled.go", "1/1/26", "u", "U", Default::default());
        let doc = r.new_document("go", &ctx);
        assert_eq!(doc.text, "package main\n\nfunc main() {\n\t\n}");
        assert_eq!(doc.caret, Some((3, 1)));
        let empty = r.new_document("css", &ctx);
        assert_eq!(empty.text, "");
    }

    #[test]
    fn clipboard_guesses() {
        let r = LanguageRegistry::builtin();
        let guess = |text: &str| r.guess_from_text(text).map(|g| g.language.id.clone());
        assert_eq!(guess("#!/bin/bash\necho hi").as_deref(), Some("shell"));
        assert_eq!(
            guess("fn main() {\n    let mut x = 1;\n}").as_deref(),
            Some("rust")
        );
        assert_eq!(
            guess("package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(1)\n}")
                .as_deref(),
            Some("go")
        );
        assert_eq!(
            guess("#include <stdio.h>\n\nint main(int argc, char *argv[]) {\n  return 0;\n}")
                .as_deref(),
            Some("c")
        );
        assert_eq!(
            guess("#include <iostream>\nusing namespace std;\nint main() { std::cout << 1; }")
                .as_deref(),
            Some("cpp")
        );
        assert_eq!(
            guess("public class A {\n  public static void main(String[] args) {\n    System.out.println(1);\n  }\n}").as_deref(),
            Some("java")
        );
        assert_eq!(
            guess("def f(x):\n    return x\n\nif __name__ == \"__main__\":\n    print(f(1))")
                .as_deref(),
            Some("python")
        );
        assert_eq!(guess("<?php\necho 1;").as_deref(), Some("php"));
        assert_eq!(
            guess("<!DOCTYPE html>\n<html><body></body></html>").as_deref(),
            Some("html")
        );
        assert_eq!(guess("{\"a\": 1, \"b\": [1,2]}").as_deref(), Some("json"));
        assert_eq!(
            guess("\\documentclass{article}\n\\begin{document}\nx\n\\end{document}").as_deref(),
            Some("latex")
        );
        assert_eq!(guess("just some words"), None);
        assert_eq!(guess(""), None);
    }

    #[test]
    fn python_code_without_keywords_is_not_misdetected_as_toml() {
        // `x = […]`/`y = f(…)` lines look like TOML key/value pairs; the
        // builtin-call and call-assignment patterns must let Python win.
        let r = LanguageRegistry::builtin();
        let text = "nums = [1, 2, 3]\ntotal = sum(nums)\ninput(\"continue?\")\n";
        assert_eq!(r.guess_from_text(text).map(|g| g.language.id.as_str()), Some("python"));
    }

    #[test]
    fn clipboard_guess_needs_a_unique_winner() {
        let r = LanguageRegistry::builtin();
        // one weak hint is not enough
        assert!(r.guess_from_text("print(1)").is_none());
    }

    #[test]
    fn user_directory_overrides_and_adds() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = LanguageRegistry::builtin();
        let mut py = r.get("python").unwrap().clone();
        py.run.command = "python3 -u $filename".into();
        std::fs::write(tmp.path().join("python.toml"), py.to_toml_string().unwrap()).unwrap();
        let custom = "id = \"zx\"\nname = \"JavaScript (zx.mjs)\"\nextensions = [\"mjs\"]\neditor_syntax = \"JavaScript\"\n[run]\ncommand = \"zx $filename\"\n";
        std::fs::write(tmp.path().join("zx.toml"), custom).unwrap();
        std::fs::write(tmp.path().join("broken.toml"), "id = ").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "ignored").unwrap();

        let report = r.load_user_dir(tmp.path()).unwrap();
        assert_eq!(report.overridden, ["python"]);
        assert_eq!(report.added, ["zx"]);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].0.ends_with("broken.toml"));
        assert_eq!(r.get("python").unwrap().run.command, "python3 -u $filename");
        assert!(r.get("python").unwrap().builtin);
        assert!(r.is_overridden("python"));
        assert!(!r.get("zx").unwrap().builtin);
        assert_eq!(r.get("zx").unwrap().group_name(), "JavaScript");
        // the custom language is found by name too
        assert_eq!(r.find("JavaScript (zx.mjs)").unwrap().id, "zx");

        // reset drops the override and the file
        r.reset("python").unwrap();
        assert_eq!(r.get("python").unwrap().run.command, "python3 $filename");
        assert!(!r.is_overridden("python"));
        assert!(!tmp.path().join("python.toml").exists());
        assert!(matches!(r.reset("zx"), Err(LanguageError::NotBundled(_))));
    }

    #[test]
    fn missing_user_directory_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = LanguageRegistry::builtin();
        let report = r.load_user_dir(tmp.path().join("nope")).unwrap();
        assert!(report.added.is_empty() && report.errors.is_empty());
    }

    #[test]
    fn save_creates_files_and_equal_builtin_removes_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = LanguageRegistry::builtin();
        r.set_user_dir(tmp.path().join("Languages"));

        r.set_enabled("lua", false).unwrap();
        assert!(!r.get("lua").unwrap().enabled);
        assert!(r.is_overridden("lua"));
        let file = tmp.path().join("Languages/lua.toml");
        assert!(file.exists());
        // survives a reload
        let mut again = LanguageRegistry::builtin();
        again.load_user_dir(tmp.path().join("Languages")).unwrap();
        assert!(!again.get("lua").unwrap().enabled);

        r.set_enabled("lua", true).unwrap();
        assert!(!file.exists(), "back to the shipped definition");
        assert!(!r.is_overridden("lua"));
    }

    #[test]
    fn save_rejects_invalid_definitions() {
        let mut r = LanguageRegistry::builtin();
        let bad = LanguageDef::new("Bad Id", "x");
        assert!(matches!(r.save(bad), Err(LanguageError::Invalid { .. })));
        let mut clash = LanguageDef::new("mine", "Mine");
        clash.uuid = r.get("python").unwrap().uuid.clone();
        assert!(matches!(r.save(clash), Err(LanguageError::Invalid { .. })));
    }

    #[test]
    fn duplicate_delete_and_reset_all() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = LanguageRegistry::builtin();
        r.set_user_dir(tmp.path());
        let id = r.duplicate("python", "Python Scratch").unwrap();
        assert_eq!(id, "python-scratch");
        let copy = r.get(&id).unwrap();
        assert!(!copy.builtin && copy.uuid.is_none());
        assert_eq!(copy.run.command, "python3 $filename");
        assert!(tmp.path().join("python-scratch.toml").exists());
        assert_eq!(
            r.duplicate("python", "Python Scratch").unwrap(),
            "python-scratch-2"
        );
        // the copy does not steal "py" files
        assert_eq!(detect_path(&r, "a.py"), "python");

        r.set_enabled("go", false).unwrap();
        r.set_enabled("lua", false).unwrap();
        r.reset_all().unwrap();
        assert!(r.get("go").unwrap().enabled && r.get("lua").unwrap().enabled);
        assert!(
            r.get("python-scratch").is_some(),
            "custom languages survive Reset All"
        );

        r.delete("python-scratch").unwrap();
        assert!(r.get("python-scratch").is_none());
        assert!(!tmp.path().join("python-scratch.toml").exists());
        assert!(matches!(
            r.delete("python"),
            Err(LanguageError::BundledLanguage(_))
        ));
        assert!(matches!(r.delete("nope"), Err(LanguageError::NotFound(_))));
    }

    #[test]
    fn unique_ids() {
        let r = LanguageRegistry::builtin();
        assert_eq!(r.unique_id("fresh"), "fresh");
        assert_eq!(r.unique_id("python"), "python-2");
        assert_eq!(r.unique_id(""), "language");
    }

    #[test]
    fn export_round_trips_through_toml() {
        let r = LanguageRegistry::builtin();
        for def in r.all() {
            let text = r.export_toml(&def.id).unwrap();
            let mut parsed =
                LanguageDef::from_toml_str(&text).unwrap_or_else(|e| panic!("{}: {e}", def.id));
            parsed.builtin = def.builtin;
            assert_eq!(&parsed, def, "{} does not round-trip", def.id);
        }
    }

    #[test]
    fn default_language_can_change() {
        let mut r = LanguageRegistry::builtin();
        r.set_default_language("python").unwrap();
        assert_eq!(detect_text(&r, "nothing"), "python");
        assert!(r.set_default_language("nope").is_err());
    }
}
