//! Read-only import of CodeRunner's preferences.
//!
//! CodeRunner keeps its preferences in `~/Library/Preferences/
//! com.krill.CodeRunner.plist`, flat under the last component of the
//! `Defaults.plist` identifier. [`import_from_cr_defaults`] maps every key
//! that has a [`PrefSpec::cr_key`](crate::PrefSpec) onto a [`Preferences`];
//! nothing is ever written to CodeRunner's files.

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value as Plist};
use thiserror::Error;
use toml::{Table, Value};

#[cfg(test)]
use crate::access::validate;
use crate::model::{Preferences, defaults};
use crate::schema::{ControlKind, PrefId, PrefSchema, PrefSpec};

/// CodeRunner's bundle identifier (the name of its preferences domain).
pub const CR_BUNDLE_ID: &str = "com.krill.CodeRunner";

/// A CodeRunner key that was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedKey {
    /// The CodeRunner key.
    pub key: String,
    /// Why.
    pub reason: String,
}

/// The outcome of an import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// The default preferences with the imported values applied.
    pub prefs: Preferences,
    /// The ids that were set from CodeRunner's values.
    pub imported: Vec<PrefId>,
    /// Mapped keys whose value could not be used.
    pub skipped: Vec<SkippedKey>,
    /// Keys without a ScratchRunner counterpart (window frames, licence and
    /// updater state, ...). Only names are kept, never values.
    pub unmapped: Vec<String>,
}

/// Why CodeRunner's preferences could not be read.
#[derive(Debug, Error)]
pub enum ImportError {
    /// The file could not be read or parsed.
    #[error("could not read {path}: {message}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The cause.
        message: String,
    },
    /// The plist root is not a dictionary.
    #[error("{0} is not a preferences dictionary")]
    NotADictionary(PathBuf),
}

/// `~/Library/Preferences/com.krill.CodeRunner.plist`.
pub fn cr_preferences_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Preferences")
            .join(format!("{CR_BUNDLE_ID}.plist")),
    )
}

/// Reads a preferences plist (binary or XML) without modifying it.
///
/// # Errors
///
/// [`ImportError`] when the file is unreadable or not a dictionary.
pub fn read_cr_preferences(path: &Path) -> Result<Dictionary, ImportError> {
    match Plist::from_file(path) {
        Ok(Plist::Dictionary(dict)) => Ok(dict),
        Ok(_) => Err(ImportError::NotADictionary(path.to_owned())),
        Err(e) => Err(ImportError::Read {
            path: path.to_owned(),
            message: e.to_string(),
        }),
    }
}

/// Maps a CodeRunner preferences dictionary onto the defaults.
pub fn import_from_cr_defaults(dict: &Dictionary) -> ImportReport {
    let mut report = ImportReport {
        prefs: Preferences::default(),
        imported: Vec::new(),
        skipped: Vec::new(),
        unmapped: Vec::new(),
    };
    for (key, value) in dict {
        let Some(spec) = PrefSchema::by_cr_key(key) else {
            report.unmapped.push(key.clone());
            continue;
        };
        if !spec.import {
            report.skipped.push(SkippedKey {
                key: key.clone(),
                reason: "not imported".to_owned(),
            });
            continue;
        }
        if key == "Font" {
            match decode_font(value) {
                Some((family, size)) => {
                    let ok_family = report
                        .prefs
                        .set(spec.id, Value::String(family))
                        .is_ok();
                    let ok_size = report
                        .prefs
                        .set("text_editing.appearance.font.size", Value::Float(size))
                        .is_ok();
                    if ok_family {
                        report.imported.push(spec.id);
                    }
                    if ok_size {
                        report.imported.push("text_editing.appearance.font.size");
                    }
                }
                None => report.skipped.push(SkippedKey {
                    key: key.clone(),
                    reason: "the archived font could not be decoded".to_owned(),
                }),
            }
            continue;
        }
        let Some(mut converted) = cr_to_value(spec, value) else {
            report.skipped.push(SkippedKey {
                key: key.clone(),
                reason: "unsupported value".to_owned(),
            });
            continue;
        };
        if matches!(key.as_str(), "ColorTheme" | "ColorThemeLight" | "ColorThemeDark") {
            map_theme_name(&mut converted);
        }
        if key == "Environment" {
            fix_term_program(&mut converted);
        }
        match report.prefs.set(spec.id, converted) {
            Ok(()) => report.imported.push(spec.id),
            Err(e) => report.skipped.push(SkippedKey {
                key: key.clone(),
                reason: e.to_string(),
            }),
        }
    }
    report
}

/// CodeRunner's default themes become ScratchRunner's.
fn map_theme_name(value: &mut Value) {
    let mapped = match value.as_str() {
        Some("CodeRunner") => defaults::THEME_DARK,
        Some("CodeRunner Light") => defaults::THEME_LIGHT,
        _ => return,
    };
    *value = Value::String(mapped.to_owned());
}

/// CodeRunner exports `TERM_PROGRAM=CodeRunner`; ScratchRunner exports its own.
fn fix_term_program(value: &mut Value) {
    if let Value::Table(table) = value
        && table.get("TERM_PROGRAM").and_then(Value::as_str) == Some("CodeRunner")
    {
        table.insert(
            "TERM_PROGRAM".to_owned(),
            Value::String(defaults::TERM_PROGRAM.to_owned()),
        );
    }
}

/// Converts a CodeRunner value to the value of the preference `spec`, or
/// `None` when it has no usable form. The result still needs validation.
pub(crate) fn cr_to_value(spec: &PrefSpec, value: &Plist) -> Option<Value> {
    let key = spec.cr_key?;
    let plain = plain_value(value)?;
    // Enumerations CodeRunner stores differently.
    if let ControlKind::Popup(options) | ControlKind::Radio(options) = spec.control {
        for option in options.iter().filter(|o| !o.separator) {
            if let Some(cr) = option.cr
                && crate::access::values_equal(&cr.to_value(), &plain)
            {
                return Some(option.value.to_value());
            }
        }
        if options.iter().any(|o| o.cr.is_some()) {
            return None;
        }
    }
    Some(match key {
        "GlobalDocs" => convert_global_docs(&plain)?,
        "DocsState" => convert_docs_state(&plain)?,
        _ => plain,
    })
}

/// A plist value as a TOML value; data and dates have no counterpart.
fn plain_value(value: &Plist) -> Option<Value> {
    Some(match value {
        Plist::Boolean(b) => Value::Boolean(*b),
        Plist::Integer(i) => Value::Integer(i.as_signed().or_else(|| i.as_unsigned().and_then(|u| i64::try_from(u).ok()))?),
        Plist::Real(f) => Value::Float(*f),
        Plist::String(s) => Value::String(s.clone()),
        Plist::Array(items) => Value::Array(items.iter().filter_map(plain_value).collect()),
        Plist::Dictionary(dict) => Value::Table(
            dict.iter()
                .filter_map(|(k, v)| Some((k.clone(), plain_value(v)?)))
                .collect::<Table>(),
        ),
        _ => return None,
    })
}

fn convert_global_docs(value: &Value) -> Option<Value> {
    let entries = value.as_array()?;
    let converted = entries
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_table()?;
            let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
            let kind = match entry.get("Type").and_then(Value::as_integer) {
                Some(1) => "bookmark",
                Some(2) => "text_find",
                _ => "search",
            };
            let mut table = Table::new();
            table.insert("id".to_owned(), Value::String(text("uuid")));
            table.insert("name".to_owned(), Value::String(text("Name")));
            table.insert("kind".to_owned(), Value::String(kind.to_owned()));
            table.insert("url".to_owned(), Value::String(text("URL")));
            table.insert("post".to_owned(), Value::String(text("POST")));
            table.insert(
                "enabled".to_owned(),
                Value::Boolean(entry.get("Enabled").and_then(Value::as_bool).unwrap_or(true)),
            );
            Some(Value::Table(table))
        })
        .collect();
    Some(Value::Array(converted))
}

fn convert_docs_state(value: &Value) -> Option<Value> {
    let states = value.as_table()?;
    let converted: Table = states
        .iter()
        .filter_map(|(id, state)| {
            let state = state.as_table()?;
            let mut table = Table::new();
            table.insert(
                "transparency".to_owned(),
                Value::Boolean(state.get("Transparency").and_then(Value::as_bool).unwrap_or(false)),
            );
            table.insert(
                "expects_redirect_from_search_results".to_owned(),
                Value::Boolean(
                    state
                        .get("expectsRedirectFromSearchResults")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ),
            );
            if let Some(zoom) = state.get("ZoomLevel").and_then(|v| match v {
                Value::Float(f) => Some(*f),
                Value::Integer(i) => Some(*i as f64),
                _ => None,
            }) {
                table.insert("zoom_level".to_owned(), Value::Float(zoom));
            }
            Some((id.clone(), Value::Table(table)))
        })
        .collect();
    Some(Value::Table(converted))
}

/// Decodes the `NSKeyedArchiver` blob CodeRunner stores in `Font` into a font
/// name and size.
fn decode_font(value: &Plist) -> Option<(String, f64)> {
    let Plist::Data(bytes) = value else { return None };
    let archive = Plist::from_reader(std::io::Cursor::new(bytes)).ok()?;
    let objects = archive.as_dictionary()?.get("$objects")?.as_array()?;
    for object in objects {
        let Some(dict) = object.as_dictionary() else { continue };
        let (Some(name), Some(size)) = (dict.get("NSName"), dict.get("NSSize")) else {
            continue;
        };
        let name = match name {
            Plist::Uid(uid) => objects.get(usize::try_from(uid.get()).ok()?)?.as_string()?,
            Plist::String(s) => s.as_str(),
            _ => continue,
        };
        let size = size
            .as_real()
            .or_else(|| size.as_signed_integer().map(|i| i as f64))?;
        return Some((name.to_owned(), size));
    }
    None
}

/// Validates through the schema; exposed for tests.
#[cfg(test)]
pub(crate) fn convert_and_validate(spec: &PrefSpec, value: &Plist) -> Option<Value> {
    validate(spec, cr_to_value(spec, value)?).ok()
}
