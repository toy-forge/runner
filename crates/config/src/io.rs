//! Loading, migrating and saving `settings.toml`.
//!
//! Loading never fails: a file that does not parse gives the defaults and a
//! warning, an invalid single value gives that preference's default and a
//! warning, and unknown top-level tables (the `[lsp]` table of another crate)
//! are handed back in [`Loaded::extra`].
//!
//! Saving edits the existing text with `toml_edit`, so comments, ordering and
//! unknown keys survive. It writes a preference only when it differs from its
//! default or is already in the file (so defaults that change in a future
//! version still reach users who never touched them). A file that does not
//! parse is copied to `settings.toml.bak` before it is replaced.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use thiserror::Error;
use toml::{Table, Value};
use toml_edit::{DocumentMut, Item, TableLike};

use crate::access::{insert, lookup, validate, values_equal};
use crate::atomic::write_atomic_str;
use crate::model::Preferences;
use crate::schema::{ControlKind, PrefSchema};

/// The version of the `settings.toml` format written by this build.
pub const SETTINGS_VERSION: u32 = 1;

const HEADER: &str = "# ScratchRunner preferences.\n# Only values that differ from the defaults are written; delete a line to reset it.\n\n";

/// What kind of problem a [`LoadWarning`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// The file is not valid TOML; the defaults are used.
    SyntaxError,
    /// The file could not be read; the defaults are used.
    ReadError,
    /// One value is invalid; that preference keeps its default.
    InvalidValue,
    /// A key inside a known table is not a preference; it is ignored.
    UnknownKey,
    /// The file was written by a newer version.
    NewerVersion,
    /// A migration step failed.
    MigrationFailed,
}

/// A problem found while loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadWarning {
    /// The kind of problem.
    pub kind: WarningKind,
    /// The preference id or key concerned, if any.
    pub id: Option<String>,
    /// A message for the user.
    pub message: String,
}

/// The result of loading preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The preferences in effect.
    pub prefs: Preferences,
    /// Problems found, in file order.
    pub warnings: Vec<LoadWarning>,
    /// Top-level keys that are not preference sections (`[lsp]`, `[runtimes]`,
    /// ...), for the crates that own them.
    pub extra: Table,
    /// The format version the file had before it was migrated in memory.
    pub migrated_from: Option<u32>,
    /// A file was found and parsed.
    pub file_used: bool,
}

impl Loaded {
    fn defaults(warning: Option<LoadWarning>) -> Self {
        Self {
            prefs: Preferences::default(),
            warnings: warning.into_iter().collect(),
            extra: Table::new(),
            migrated_from: None,
            file_used: false,
        }
    }

    /// The warnings as one text to show, or `None` when there are none.
    pub fn warning(&self) -> Option<String> {
        if self.warnings.is_empty() {
            return None;
        }
        Some(
            self.warnings
                .iter()
                .map(|w| w.message.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Loads `path`. A missing file is not a warning; an unreadable or invalid
    /// one falls back to the defaults and says why.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::defaults(None),
            Err(e) => Self::defaults(Some(LoadWarning {
                kind: WarningKind::ReadError,
                id: None,
                message: format!("settings.toml could not be read: {e}"),
            })),
        }
    }

    /// Parses the text of a `settings.toml`.
    pub fn parse(text: &str) -> Self {
        let mut doc = match text.parse::<DocumentMut>() {
            Ok(doc) => doc,
            Err(e) => {
                return Self::defaults(Some(LoadWarning {
                    kind: WarningKind::SyntaxError,
                    id: None,
                    message: format!("settings.toml was ignored: {e}. Using defaults."),
                }));
            }
        };
        let mut warnings = Vec::new();
        let file_version = document_version(&doc);
        let mut migrated_from = None;
        if file_version > SETTINGS_VERSION {
            warnings.push(LoadWarning {
                kind: WarningKind::NewerVersion,
                id: None,
                message: format!(
                    "settings.toml was written by a newer version of ScratchRunner (format {file_version}); some settings may be ignored."
                ),
            });
        } else {
            match migrate_document(&mut doc, MIGRATIONS, SETTINGS_VERSION) {
                Ok(from) => migrated_from = from,
                Err(message) => warnings.push(LoadWarning {
                    kind: WarningKind::MigrationFailed,
                    id: None,
                    message: format!("settings.toml could not be migrated: {message}"),
                }),
            }
        }
        let table = match item_to_value(doc.as_item()) {
            Some(Value::Table(table)) => table,
            _ => Table::new(),
        };
        let (prefs, extra) = from_file_table(&table, &mut warnings);
        Self {
            prefs,
            warnings,
            extra,
            migrated_from,
            file_used: true,
        }
    }
}

// ------------------------------------------------------- lenient reading

fn known_paths() -> &'static (HashSet<&'static str>, HashSet<String>) {
    static PATHS: OnceLock<(HashSet<&'static str>, HashSet<String>)> = OnceLock::new();
    PATHS.get_or_init(|| {
        let mut sections = HashSet::new();
        let mut prefixes = HashSet::new();
        for spec in PrefSchema::values() {
            if let Some((first, _)) = spec.id.split_once('.') {
                sections.insert(first);
            }
            let mut prefix = String::new();
            for part in spec.id.split('.') {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(part);
                prefixes.insert(prefix.clone());
            }
        }
        (sections, prefixes)
    })
}

fn from_file_table(file: &Table, warnings: &mut Vec<LoadWarning>) -> (Preferences, Table) {
    let defaults = Preferences::default();
    let mut table = defaults.to_table();
    for spec in PrefSchema::values() {
        let Some(value) = lookup(file, spec.id) else {
            continue;
        };
        let accepted = validate(spec, value.clone()).and_then(|v| {
            if matches!(spec.control, ControlKind::Custom(_) | ControlKind::KeyValueTable(_) | ControlKind::Color) {
                // Shape of nested content: try it against the real structs.
                let mut trial = defaults.to_table();
                insert(&mut trial, spec.id, v.clone());
                Preferences::from_table(trial).map_err(|e| crate::PrefError::Invalid {
                    id: spec.id.to_owned(),
                    message: e.message().to_owned(),
                })?;
            }
            Ok(v)
        });
        match accepted {
            Ok(v) => insert(&mut table, spec.id, v),
            Err(e) => warnings.push(LoadWarning {
                kind: WarningKind::InvalidValue,
                id: Some(spec.id.to_owned()),
                message: format!("{e}; using the default"),
            }),
        }
    }
    let mut extra = Table::new();
    let (sections, prefixes) = known_paths();
    collect_unknown(file, "", sections, prefixes, &mut extra, warnings);
    let prefs = match Preferences::from_table(table) {
        Ok(prefs) => prefs,
        Err(e) => {
            warnings.push(LoadWarning {
                kind: WarningKind::InvalidValue,
                id: None,
                message: format!("settings.toml was ignored: {}. Using defaults.", e.message()),
            });
            defaults
        }
    };
    (prefs, extra)
}

fn collect_unknown(
    table: &Table,
    path: &str,
    sections: &HashSet<&'static str>,
    prefixes: &HashSet<String>,
    extra: &mut Table,
    warnings: &mut Vec<LoadWarning>,
) {
    for (key, value) in table {
        let id = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if path.is_empty() && key == "version" {
            continue;
        }
        if PrefSchema::get(&id).is_some() {
            continue; // a leaf; validated above
        }
        if prefixes.contains(&id) {
            if let Value::Table(inner) = value {
                collect_unknown(inner, &id, sections, prefixes, extra, warnings);
            } else {
                warnings.push(LoadWarning {
                    kind: WarningKind::InvalidValue,
                    id: Some(id.clone()),
                    message: format!("`{id}` must be a table; ignored"),
                });
            }
        } else if path.is_empty() && !sections.contains(key.as_str()) {
            extra.insert(key.clone(), value.clone());
        } else {
            warnings.push(LoadWarning {
                kind: WarningKind::UnknownKey,
                id: Some(id.clone()),
                message: format!("unknown preference `{id}` ignored"),
            });
        }
    }
}

// ------------------------------------------------------------- migration

/// One step of the settings format history.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The format version this step upgrades from; the result is `from + 1`.
    pub from: u32,
    /// The step. It must be idempotent: it may run on a file that is already
    /// in the new shape (a hand-written file has no version).
    pub run: fn(&mut DocumentMut) -> Result<(), String>,
}

/// The migrations of this build, in order.
pub(crate) static MIGRATIONS: &[Migration] = &[Migration {
    from: 0,
    run: migrate_v0_to_v1,
}];

fn document_version(doc: &DocumentMut) -> u32 {
    doc.get("version")
        .and_then(Item::as_integer)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

/// Upgrades `doc` to `target` by running the `migrations` whose `from` is at
/// least the document's version and below `target`, then stores `version`.
/// Returns the version the document had when it was older, `None` when it was
/// already current.
///
/// # Errors
///
/// The message of the failing step; the document may be partly migrated.
pub fn migrate_document(
    doc: &mut DocumentMut,
    migrations: &[Migration],
    target: u32,
) -> Result<Option<u32>, String> {
    let version = document_version(doc);
    if version >= target {
        return Ok(None);
    }
    let mut steps: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.from >= version && m.from < target)
        .collect();
    steps.sort_by_key(|m| m.from);
    for step in steps {
        (step.run)(doc).map_err(|e| format!("from format {}: {e}", step.from))?;
    }
    doc["version"] = toml_edit::value(i64::from(target));
    Ok(Some(version))
}

fn table_like<'a>(doc: &'a mut DocumentMut, id: &str) -> Option<&'a mut dyn TableLike> {
    let mut current: &mut dyn TableLike = doc.as_table_mut();
    if id.is_empty() {
        return Some(current);
    }
    for part in id.split('.') {
        current = current.get_mut(part)?.as_table_like_mut()?;
    }
    Some(current)
}

/// Moves a value from a legacy key to a new id unless the new id is set.
fn move_key(doc: &mut DocumentMut, from_table: &str, from_key: &str, to_id: &str) {
    let taken = table_like(doc, from_table).and_then(|t| t.remove(from_key));
    let Some(item) = taken else { return };
    let Some(value) = item_to_value(&item) else { return };
    if doc_value(doc, to_id).is_none() {
        set_leaf(doc, to_id, &value, true);
    }
}

fn doc_value(doc: &DocumentMut, id: &str) -> Option<Value> {
    let mut item = doc.as_item();
    for part in id.split('.') {
        item = item.as_table_like()?.get(part)?;
    }
    item_to_value(item)
}

/// 0 -> 1: the first ScratchRunner format (`[general]`, `[editor]`,
/// `[execution]`) becomes the CodeRunner-shaped tree. `[editor]` keys move to
/// `text_editing`, `general.default_language` to `languages.current_language`;
/// `restore_session`, `theme` and `scratch_working_directory` are dropped (the
/// session is always restored and the theme follows the system appearance).
/// `[execution]`, `[runtimes]`, `[lsp]` and `[templates]` belong to other
/// crates and stay.
fn migrate_v0_to_v1(doc: &mut DocumentMut) -> Result<(), String> {
    const MAP: &[(&str, &str, &str)] = &[
        ("editor", "font_family", "text_editing.appearance.font.family"),
        ("editor", "font_size", "text_editing.appearance.font.size"),
        ("editor", "line_numbers", "text_editing.appearance.line_numbers.enabled"),
        ("editor", "soft_wrap", "text_editing.appearance.wrap_lines"),
        ("editor", "tab_size", "text_editing.indentation.tab_width"),
        ("editor", "auto_close_brackets", "text_editing.behavior.character_pairs.close_enabled"),
        ("general", "default_language", "languages.current_language"),
    ];
    for (table, key, to) in MAP {
        move_key(doc, table, key, to);
    }
    for key in ["restore_session", "theme", "scratch_working_directory"] {
        if let Some(general) = table_like(doc, "general") {
            general.remove(key);
        }
    }
    let editor_empty = table_like(doc, "editor").is_some_and(|t| t.is_empty());
    if editor_empty {
        doc.remove("editor");
    }
    Ok(())
}

// ---------------------------------------------------------------- saving

/// Why saving failed.
#[derive(Debug, Error)]
pub enum SaveError {
    /// Writing the file failed.
    #[error("could not write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
}

impl Preferences {
    /// Renders the `settings.toml` text for these preferences, starting from
    /// `existing` (the current file text) so its comments, ordering and
    /// unknown keys are kept. See the module docs for what is written.
    pub fn render(&self, existing: Option<&str>) -> String {
        let mut doc = existing
            .and_then(|text| text.parse::<DocumentMut>().ok())
            .unwrap_or_else(|| {
                let mut doc = DocumentMut::new();
                doc.decor_mut().set_prefix(HEADER);
                doc
            });
        if let Err(message) = migrate_document(&mut doc, MIGRATIONS, SETTINGS_VERSION) {
            tracing::warn!("settings migration failed while saving: {message}");
        }
        if document_version(&doc) < SETTINGS_VERSION {
            doc["version"] = toml_edit::value(i64::from(SETTINGS_VERSION));
        }
        let table = self.to_table();
        for spec in PrefSchema::values() {
            let Some(value) = lookup(&table, spec.id) else {
                continue;
            };
            let is_default = values_equal(value, &spec.default_value());
            set_leaf(&mut doc, spec.id, value, !is_default);
        }
        doc.to_string()
    }

    /// Writes the preferences to `path` atomically, keeping what the file
    /// already holds (see [`Preferences::render`]). A file that does not parse
    /// is first copied to `<path>.bak`. A file that already holds exactly
    /// this text is left untouched.
    ///
    /// # Errors
    ///
    /// [`SaveError::Write`] when the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), SaveError> {
        let existing = std::fs::read_to_string(path).ok();
        if let Some(text) = &existing
            && text.parse::<DocumentMut>().is_ok()
            && self.render(Some(text)) == *text
        {
            return Ok(());
        }
        if let Some(text) = &existing
            && text.parse::<DocumentMut>().is_err()
        {
            let mut backup = path.as_os_str().to_owned();
            backup.push(".bak");
            if let Err(e) = std::fs::copy(path, &backup) {
                tracing::warn!("could not back up the invalid settings file: {e}");
            }
        }
        let text = self.render(existing.as_deref());
        write_atomic_str(path, &text).map_err(|source| SaveError::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// Every preference as TOML, defaults included (for export).
    pub fn to_toml_full(&self) -> String {
        let mut doc = DocumentMut::new();
        doc.decor_mut().set_prefix(HEADER);
        doc["version"] = toml_edit::value(i64::from(SETTINGS_VERSION));
        let table = self.to_table();
        for spec in PrefSchema::values() {
            if let Some(value) = lookup(&table, spec.id) {
                set_leaf(&mut doc, spec.id, value, true);
            }
        }
        doc.to_string()
    }
}

/// Sets the value at `id` in the document. An existing leaf is replaced only
/// when its value differs (keeping its comment); a missing one is inserted
/// only when `insert_missing`.
fn set_leaf(doc: &mut DocumentMut, id: &str, value: &Value, insert_missing: bool) {
    let (parents, leaf) = match id.rsplit_once('.') {
        Some((parents, leaf)) => (parents, leaf),
        None => ("", id),
    };
    if table_like(doc, parents).is_none() {
        if !insert_missing {
            return;
        }
        let mut current: &mut dyn TableLike = doc.as_table_mut();
        for part in parents.split('.') {
            if !current.contains_key(part) {
                let mut table = toml_edit::Table::new();
                table.set_implicit(true);
                current.insert(part, Item::Table(table));
            }
            current = match current.get_mut(part).and_then(Item::as_table_like_mut) {
                Some(next) => next,
                None => return,
            };
        }
    }
    let Some(parent) = table_like(doc, parents) else {
        return;
    };
    match parent.get_mut(leaf) {
        Some(item) => {
            if item_to_value(item).is_some_and(|current| values_equal(&current, value)) {
                return;
            }
            let mut new = value_to_edit(value);
            if let Item::Value(old) = item {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None if insert_missing => {
            parent.insert(leaf, Item::Value(value_to_edit(value)));
        }
        None => {}
    }
}

// ------------------------------------------------------------ conversions

fn item_to_value(item: &Item) -> Option<Value> {
    match item {
        Item::None => None,
        Item::Value(v) => edit_value_to_toml(v),
        Item::Table(t) => Some(Value::Table(
            t.iter()
                .filter_map(|(k, v)| Some((k.to_owned(), item_to_value(v)?)))
                .collect(),
        )),
        Item::ArrayOfTables(a) => Some(Value::Array(
            a.iter()
                .map(|t| {
                    Value::Table(
                        t.iter()
                            .filter_map(|(k, v)| Some((k.to_owned(), item_to_value(v)?)))
                            .collect(),
                    )
                })
                .collect(),
        )),
    }
}

fn edit_value_to_toml(value: &toml_edit::Value) -> Option<Value> {
    Some(match value {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::Integer(*i.value()),
        toml_edit::Value::Float(f) => Value::Float(*f.value()),
        toml_edit::Value::Boolean(b) => Value::Boolean(*b.value()),
        toml_edit::Value::Datetime(d) => Value::Datetime(d.value().to_string().parse().ok()?),
        toml_edit::Value::Array(a) => {
            Value::Array(a.iter().filter_map(edit_value_to_toml).collect())
        }
        toml_edit::Value::InlineTable(t) => Value::Table(
            t.iter()
                .filter_map(|(k, v)| Some((k.to_owned(), edit_value_to_toml(v)?)))
                .collect(),
        ),
    })
}

fn value_to_edit(value: &Value) -> toml_edit::Value {
    match value {
        Value::String(s) => toml_edit::Value::from(s.as_str()),
        Value::Integer(i) => toml_edit::Value::from(*i),
        Value::Float(f) => toml_edit::Value::from(*f),
        Value::Boolean(b) => toml_edit::Value::from(*b),
        Value::Datetime(d) => d
            .to_string()
            .parse::<toml_edit::Value>()
            .unwrap_or_else(|_| toml_edit::Value::from(d.to_string())),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(value_to_edit(item));
            }
            toml_edit::Value::Array(array)
        }
        Value::Table(table) => {
            let mut inline = toml_edit::InlineTable::new();
            for (k, v) in table {
                inline.insert(k, value_to_edit(v));
            }
            toml_edit::Value::InlineTable(inline)
        }
    }
}
