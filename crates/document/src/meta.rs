//! Per-file metadata in extended attributes.
//!
//! A document's language, grammar, run settings, breakpoints, ignored
//! diagnostics and last selection travel with the *file* (so they survive
//! renames and moves in Finder) as extended attributes in our namespace
//! `ScratchRunner:` ([`MetaKey`]). CodeRunner uses the same keys under
//! `CodeRunner:`; those are **read** as a fallback (`CodeRunner:Language` holds
//! CodeRunner's language UUID) and only written when
//! [`CodeRunnerCompat::ReadWrite`] is chosen. File systems without extended
//! attributes (FAT, some network shares) or files that refuse them get a JSON
//! sidecar in a metadata directory instead; the caller sees which backend was
//! used ([`MetaBackend`]).
//!
//! The encoding attribute `com.apple.TextEncoding` is separate
//! ([`read_text_encoding_xattr`]); [`crate::save_text`] writes it.
//!
//! Formats: ours are plain UTF-8 strings for text values and JSON for
//! structured ones. CodeRunner's structured payloads are not documented; they
//! are parsed tolerantly (JSON, property list, or whitespace separated
//! numbers) and CodeRunner's line numbers are assumed to count from 1.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::encoding::TextEncoding;
use crate::fileio::hash_bytes;
use crate::state::{Breakpoint, Selection};

/// The attribute CoreFoundation/TextEdit use for the text encoding.
pub const TEXT_ENCODING_XATTR: &str = "com.apple.TextEncoding";
/// Our attribute namespace.
pub const NAMESPACE: &str = "ScratchRunner:";
/// CodeRunner's attribute namespace.
pub const CODERUNNER_NAMESPACE: &str = "CodeRunner:";

/// One metadata key. The attribute name is `ScratchRunner:<name>` (or
/// `CodeRunner:<name>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetaKey {
    /// The language (our id; CodeRunner: its UUID).
    Language,
    /// The grammar / syntax mode (a TextMate scope name).
    Grammar,
    /// The per-file run command.
    RunCommand,
    /// The per-file arguments.
    Arguments,
    /// The per-file compile flags.
    CompilerFlags,
    /// The per-file program input.
    Input,
    /// The breakpoints.
    Breakpoints,
    /// Ignored/disabled diagnostics.
    Diagnostics,
    /// The last selection.
    Selection,
}

impl MetaKey {
    /// Every key.
    pub const ALL: [MetaKey; 9] = [
        Self::Language,
        Self::Grammar,
        Self::RunCommand,
        Self::Arguments,
        Self::CompilerFlags,
        Self::Input,
        Self::Breakpoints,
        Self::Diagnostics,
        Self::Selection,
    ];

    /// The key's name without namespace.
    pub fn name(self) -> &'static str {
        match self {
            Self::Language => "Language",
            Self::Grammar => "Grammar",
            Self::RunCommand => "RunCommand",
            Self::Arguments => "Arguments",
            Self::CompilerFlags => "CompilerFlags",
            Self::Input => "Input",
            Self::Breakpoints => "Breakpoints",
            Self::Diagnostics => "Diagnostics",
            Self::Selection => "Selection",
        }
    }

    /// `ScratchRunner:<name>`.
    pub fn attribute(self) -> String {
        format!("{NAMESPACE}{}", self.name())
    }

    /// `CodeRunner:<name>`.
    pub fn codrunner_attribute(self) -> String {
        format!("{CODERUNNER_NAMESPACE}{}", self.name())
    }
}

/// Diagnostics the user switched off for one file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsMeta {
    /// Messages ignored in this document ("Ignore Messages Like...").
    pub ignored: Vec<String>,
}

/// The metadata stored with a file. `None` means "not set".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileMeta {
    /// The language: one of our ids (also tolerated: a uuid or a name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// CodeRunner's language uuid: filled from `CodeRunner:Language`, and
    /// written there in [`CodeRunnerCompat::ReadWrite`] mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_uuid: Option<String>,
    /// The grammar (TextMate scope name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grammar: Option<String>,
    /// Per-file run command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_command: Option<String>,
    /// Per-file arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// Per-file compile flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_flags: Option<String>,
    /// Per-file program input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Breakpoints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<Breakpoint>>,
    /// Diagnostics settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticsMeta>,
    /// The last selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Vec<Selection>>,
    /// A selection read from CodeRunner as UTF-16 `(location, length)` ranges;
    /// convert with [`FileMeta::selections_for`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_ranges: Option<Vec<(usize, usize)>>,
    /// The `com.apple.TextEncoding` attribute (read only here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<TextEncoding>,
}

impl FileMeta {
    /// Nothing is set.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The language, resolved against `registry`: the `language` field by id,
    /// uuid or name, then `language_uuid`.
    pub fn resolve_language<'a>(
        &self,
        registry: &'a scratch_language::LanguageRegistry,
    ) -> Option<&'a scratch_language::LanguageDef> {
        self.language
            .as_deref()
            .and_then(|query| registry.find(query))
            .or_else(|| {
                self.language_uuid
                    .as_deref()
                    .and_then(|query| registry.find(query))
            })
    }

    /// The selections for `text`: `selection` when set, else the CodeRunner
    /// ranges converted with [`crate::position_at_utf16_offset`].
    pub fn selections_for(&self, text: &str) -> Vec<Selection> {
        if let Some(selection) = &self.selection {
            return selection.clone();
        }
        self.selection_ranges
            .iter()
            .flatten()
            .map(|&(location, length)| {
                Selection::new(
                    crate::state::position_at_utf16_offset(text, location),
                    crate::state::position_at_utf16_offset(text, location.saturating_add(length)),
                )
            })
            .collect()
    }

    /// Fills every unset field from `other`.
    fn fill_from(&mut self, other: FileMeta) {
        macro_rules! fill {
            ($($field:ident),*) => { $( if self.$field.is_none() { self.$field = other.$field; } )* };
        }
        fill!(
            language,
            language_uuid,
            grammar,
            run_command,
            arguments,
            compiler_flags,
            input,
            breakpoints,
            diagnostics,
            selection,
            selection_ranges,
            encoding
        );
    }
}

/// How to treat CodeRunner's `CodeRunner:*` attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeRunnerCompat {
    /// Ignore them.
    Off,
    /// Read them when ours are missing (the default); never write them.
    #[default]
    ReadOnly,
    /// Read them, and also write the plain-text ones (language uuid, grammar,
    /// run command, arguments, compile flags, input) so CodeRunner sees our
    /// settings.
    ReadWrite,
}

/// Where [`MetaStore::write`] put the metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaBackend {
    /// Extended attributes on the file.
    Xattr,
    /// A JSON file in the sidecar directory.
    Sidecar,
}

/// Failures of [`MetaStore`].
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    /// The file system refused extended attributes and no sidecar directory
    /// is configured.
    #[error("{path}: extended attributes are not available ({source})")]
    Unsupported {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// An I/O error (missing file, unwritable sidecar directory).
    #[error("{path}: {source}")]
    Io {
        /// The file or sidecar.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
}

/// Reads and writes [`FileMeta`].
#[derive(Debug, Clone, Default)]
pub struct MetaStore {
    compat: CodeRunnerCompat,
    sidecar_dir: Option<PathBuf>,
}

impl MetaStore {
    /// A store without sidecar fallback and with read-only CodeRunner compat.
    pub fn new() -> Self {
        Self::default()
    }

    /// The sidecar directory the app uses: `<root>/Metadata`.
    pub fn default_sidecar_dir(paths: &scratch_config::Paths) -> PathBuf {
        paths.root().join("Metadata")
    }

    /// Sets the directory of JSON sidecars (created on first use).
    #[must_use]
    pub fn with_sidecar_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.sidecar_dir = Some(dir.into());
        self
    }

    /// Sets how `CodeRunner:*` attributes are treated.
    #[must_use]
    pub fn with_compat(mut self, compat: CodeRunnerCompat) -> Self {
        self.compat = compat;
        self
    }

    /// The sidecar file for `path`, if a sidecar directory is set.
    pub fn sidecar_path(&self, path: &Path) -> Option<PathBuf> {
        let dir = self.sidecar_dir.as_ref()?;
        // Canonicalise the directory, not the file, so the key is the same
        // before a move (when the file is gone) and for a file not yet created.
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_owned());
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical = fs::canonicalize(parent)
            .unwrap_or_else(|_| parent.to_path_buf())
            .join(&name);
        let hash = hash_bytes(canonical.to_string_lossy().as_bytes());
        Some(dir.join(format!("{hash:016x}-{name}.json")))
    }

    /// Reads the metadata of `path`: our attributes first, then the sidecar,
    /// then (unless [`CodeRunnerCompat::Off`]) CodeRunner's. Never fails; a
    /// file without metadata reads as [`FileMeta::default`].
    pub fn read(&self, path: &Path) -> FileMeta {
        let mut meta = read_namespace(path, NAMESPACE, false);
        if let Some(sidecar) = self.sidecar_path(path)
            && let Ok(bytes) = fs::read(&sidecar)
        {
            match serde_json::from_slice::<FileMeta>(&bytes) {
                Ok(from_sidecar) => meta.fill_from(from_sidecar),
                Err(error) => {
                    tracing::warn!(sidecar = %sidecar.display(), %error, "unreadable metadata sidecar");
                }
            }
        }
        if self.compat != CodeRunnerCompat::Off {
            let legacy = read_namespace(path, CODERUNNER_NAMESPACE, true);
            meta.fill_from(legacy);
        }
        if meta.encoding.is_none() {
            meta.encoding = read_text_encoding_xattr(path);
        }
        meta
    }

    /// Writes `meta` for `path`: every set field becomes an attribute, every
    /// unset field's attribute is removed. When the file refuses attributes
    /// the whole record goes to the sidecar instead.
    ///
    /// # Errors
    ///
    /// [`MetaError::Io`] when the file does not exist or the sidecar cannot be
    /// written; [`MetaError::Unsupported`] when attributes fail and there is no
    /// sidecar directory.
    pub fn write(&self, path: &Path, meta: &FileMeta) -> Result<MetaBackend, MetaError> {
        if let Err(error) = fs::metadata(path) {
            return Err(MetaError::Io {
                path: path.to_path_buf(),
                source: error,
            });
        }
        match self.write_xattrs(path, meta) {
            Ok(()) => {
                if let Some(sidecar) = self.sidecar_path(path) {
                    let _ = fs::remove_file(sidecar);
                }
                Ok(MetaBackend::Xattr)
            }
            Err(source) => {
                let Some(sidecar) = self.sidecar_path(path) else {
                    return Err(MetaError::Unsupported {
                        path: path.to_path_buf(),
                        source,
                    });
                };
                tracing::debug!(path = %path.display(), %source, "extended attributes refused; using a sidecar");
                let io_err = |source: io::Error| MetaError::Io {
                    path: sidecar.clone(),
                    source,
                };
                if let Some(dir) = sidecar.parent() {
                    fs::create_dir_all(dir).map_err(io_err)?;
                }
                let json =
                    serde_json::to_vec_pretty(meta).map_err(|e| io_err(io::Error::other(e)))?;
                scratch_config::write_atomic(&sidecar, &json).map_err(io_err)?;
                Ok(MetaBackend::Sidecar)
            }
        }
    }

    /// Removes all our attributes and the sidecar of `path`.
    pub fn clear(&self, path: &Path) {
        for key in MetaKey::ALL {
            remove_attr(path, &key.attribute());
        }
        if let Some(sidecar) = self.sidecar_path(path) {
            let _ = fs::remove_file(sidecar);
        }
    }

    /// Moves the sidecar of `from` to `to` (after a rename or move; attributes
    /// move with the file by themselves).
    ///
    /// # Errors
    ///
    /// I/O errors moving the sidecar. A missing sidecar is not an error.
    pub fn move_sidecar(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (Some(old), Some(new)) = (self.sidecar_path(from), self.sidecar_path(to)) else {
            return Ok(());
        };
        if old == new || !old.exists() {
            return Ok(());
        }
        fs::rename(old, new)
    }

    /// Copies the metadata of `from` to `to` (duplicate): attributes are
    /// copied by the caller with [`crate::copy_xattrs`]; this copies a sidecar.
    ///
    /// # Errors
    ///
    /// I/O errors copying the sidecar. A missing sidecar is not an error.
    pub fn copy_sidecar(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (Some(old), Some(new)) = (self.sidecar_path(from), self.sidecar_path(to)) else {
            return Ok(());
        };
        if !old.exists() {
            return Ok(());
        }
        fs::copy(old, new).map(|_| ())
    }

    fn write_xattrs(&self, path: &Path, meta: &FileMeta) -> io::Result<()> {
        let ns =
            |key: MetaKey, value: Option<Vec<u8>>| set_or_remove(path, &key.attribute(), value);
        let text = |value: &Option<String>| value.as_ref().map(|v| v.clone().into_bytes());
        ns(MetaKey::Language, text(&meta.language))?;
        ns(MetaKey::Grammar, text(&meta.grammar))?;
        ns(MetaKey::RunCommand, text(&meta.run_command))?;
        ns(MetaKey::Arguments, text(&meta.arguments))?;
        ns(MetaKey::CompilerFlags, text(&meta.compiler_flags))?;
        ns(MetaKey::Input, text(&meta.input))?;
        ns(MetaKey::Breakpoints, json_value(&meta.breakpoints)?)?;
        ns(MetaKey::Diagnostics, json_value(&meta.diagnostics)?)?;
        ns(MetaKey::Selection, json_value(&meta.selection)?)?;
        if let Some(encoding) = meta.encoding {
            set_or_remove(
                path,
                TEXT_ENCODING_XATTR,
                Some(encoding.xattr_value().into_bytes()),
            )?;
        }
        if self.compat == CodeRunnerCompat::ReadWrite {
            let cr = |key: MetaKey, value: Option<Vec<u8>>| {
                set_or_remove(path, &key.codrunner_attribute(), value)
            };
            cr(MetaKey::Language, text(&meta.language_uuid))?;
            cr(MetaKey::Grammar, text(&meta.grammar))?;
            cr(MetaKey::RunCommand, text(&meta.run_command))?;
            cr(MetaKey::Arguments, text(&meta.arguments))?;
            cr(MetaKey::CompilerFlags, text(&meta.compiler_flags))?;
            cr(MetaKey::Input, text(&meta.input))?;
        }
        Ok(())
    }
}

fn json_value<T: Serialize>(value: &Option<T>) -> io::Result<Option<Vec<u8>>> {
    value
        .as_ref()
        .map(|v| serde_json::to_vec(v).map_err(io::Error::other))
        .transpose()
}

fn set_or_remove(path: &Path, name: &str, value: Option<Vec<u8>>) -> io::Result<()> {
    match value {
        Some(bytes) => xattr::set(path, name, &bytes),
        None => {
            remove_attr(path, name);
            Ok(())
        }
    }
}

fn remove_attr(path: &Path, name: &str) {
    // Removing an attribute that is not there fails with ENOATTR: fine.
    let _ = xattr::remove(path, name);
}

/// The `com.apple.TextEncoding` attribute of `path`, if it has a readable one.
pub fn read_text_encoding_xattr(path: &Path) -> Option<TextEncoding> {
    let value = xattr::get(path, TEXT_ENCODING_XATTR).ok()??;
    TextEncoding::from_xattr_value(&String::from_utf8_lossy(&value))
}

fn get_string(path: &Path, name: &str) -> Option<String> {
    let bytes = xattr::get(path, name).ok()??;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn get_bytes(path: &Path, name: &str) -> Option<Vec<u8>> {
    xattr::get(path, name).ok()?
}

fn read_namespace(path: &Path, namespace: &str, legacy: bool) -> FileMeta {
    let attr = |key: MetaKey| format!("{namespace}{}", key.name());
    let mut meta = FileMeta::default();
    let language = get_string(path, &attr(MetaKey::Language));
    if legacy {
        meta.language_uuid = language;
    } else {
        meta.language = language;
    }
    meta.grammar = get_string(path, &attr(MetaKey::Grammar));
    meta.run_command = get_string(path, &attr(MetaKey::RunCommand));
    meta.arguments = get_string(path, &attr(MetaKey::Arguments));
    meta.compiler_flags = get_string(path, &attr(MetaKey::CompilerFlags));
    meta.input = get_string(path, &attr(MetaKey::Input));
    meta.breakpoints =
        get_bytes(path, &attr(MetaKey::Breakpoints)).and_then(|b| parse_breakpoints(&b, legacy));
    meta.diagnostics = get_bytes(path, &attr(MetaKey::Diagnostics)).map(|b| parse_diagnostics(&b));
    if let Some(bytes) = get_bytes(path, &attr(MetaKey::Selection)) {
        if legacy {
            meta.selection_ranges = parse_ranges(&bytes);
        } else {
            meta.selection = serde_json::from_slice(&bytes).ok();
        }
    }
    meta
}

// ---------------------------------------------------------------------------
// Tolerant parsers

#[derive(Debug, Clone, PartialEq)]
enum Loose {
    Number(f64),
    Text(String),
    Bool(bool),
    List(Vec<Loose>),
    Map(Vec<(String, Loose)>),
}

fn loose_from_json(value: &serde_json::Value) -> Loose {
    match value {
        serde_json::Value::Number(n) => Loose::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Loose::Text(s.clone()),
        serde_json::Value::Bool(b) => Loose::Bool(*b),
        serde_json::Value::Array(a) => Loose::List(a.iter().map(loose_from_json).collect()),
        serde_json::Value::Object(o) => Loose::Map(
            o.iter()
                .map(|(k, v)| (k.clone(), loose_from_json(v)))
                .collect(),
        ),
        serde_json::Value::Null => Loose::List(Vec::new()),
    }
}

fn loose_from_plist(value: &plist::Value) -> Loose {
    match value {
        plist::Value::Integer(i) => Loose::Number(
            i.as_signed()
                .map(|v| v as f64)
                .or_else(|| i.as_unsigned().map(|v| v as f64))
                .unwrap_or(0.0),
        ),
        plist::Value::Real(r) => Loose::Number(*r),
        plist::Value::String(s) => Loose::Text(s.clone()),
        plist::Value::Boolean(b) => Loose::Bool(*b),
        plist::Value::Array(a) => Loose::List(a.iter().map(loose_from_plist).collect()),
        plist::Value::Dictionary(d) => Loose::Map(
            d.iter()
                .map(|(k, v)| (k.clone(), loose_from_plist(v)))
                .collect(),
        ),
        _ => Loose::List(Vec::new()),
    }
}

fn load_loose(bytes: &[u8]) -> Option<Loose> {
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) {
        return Some(loose_from_json(&json));
    }
    if (bytes.starts_with(b"bplist") || bytes.trim_ascii_start().starts_with(b"<?xml"))
        && let Ok(value) = plist::Value::from_reader(io::Cursor::new(bytes))
    {
        return Some(loose_from_plist(&value));
    }
    None
}

fn map_get<'a>(map: &'a [(String, Loose)], names: &[&str]) -> Option<&'a Loose> {
    map.iter()
        .find(|(key, _)| names.iter().any(|n| key.eq_ignore_ascii_case(n)))
        .map(|(_, value)| value)
}

fn numbers_in(text: &str) -> Vec<f64> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Parses a breakpoints attribute. `one_based`: the numbers count lines from
/// 1 (CodeRunner's) and are converted to 0-based; the payload may be JSON, a
/// property list or numbers separated by anything.
fn parse_breakpoints(bytes: &[u8], one_based: bool) -> Option<Vec<Breakpoint>> {
    let to_line = |n: f64| -> Option<u32> {
        let n = if one_based { n - 1.0 } else { n };
        (n >= 0.0 && n <= f64::from(u32::MAX)).then_some(n as u32)
    };
    let from_item = |item: &Loose| -> Option<Breakpoint> {
        match item {
            Loose::Number(n) => Some(Breakpoint::at(to_line(*n)?)),
            Loose::Map(map) => {
                let Loose::Number(n) = map_get(map, &["line", "lineNumber"])? else {
                    return None;
                };
                let temporary = matches!(
                    map_get(map, &["temporary"]),
                    Some(Loose::Bool(true)) | Some(Loose::Number(1.0))
                );
                Some(Breakpoint {
                    line: to_line(*n)?,
                    temporary,
                    ..Breakpoint::at(0)
                })
            }
            Loose::Text(text) => numbers_in(text)
                .first()
                .and_then(|n| to_line(*n))
                .map(Breakpoint::at),
            _ => None,
        }
    };
    let list: Vec<Breakpoint> = match load_loose(bytes) {
        Some(Loose::List(items)) => items.iter().filter_map(from_item).collect(),
        Some(Loose::Map(map)) => match map_get(&map, &["lines", "breakpoints"]) {
            Some(Loose::List(items)) => items.iter().filter_map(from_item).collect(),
            _ => return None,
        },
        Some(_) => return None,
        None => numbers_in(&String::from_utf8_lossy(bytes))
            .into_iter()
            .filter_map(|n| to_line(n).map(Breakpoint::at))
            .collect(),
    };
    Some(
        crate::state::Breakpoints::from_list(list)
            .as_slice()
            .to_vec(),
    )
}

fn parse_diagnostics(bytes: &[u8]) -> DiagnosticsMeta {
    let strings = |items: &[Loose]| -> Vec<String> {
        items
            .iter()
            .filter_map(|i| {
                if let Loose::Text(s) = i {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect()
    };
    let ignored = match load_loose(bytes) {
        Some(Loose::List(items)) => strings(&items),
        Some(Loose::Map(map)) => match map_get(&map, &["ignored", "ignoredDiagnostics"]) {
            Some(Loose::List(items)) => strings(items),
            _ => Vec::new(),
        },
        Some(_) => Vec::new(),
        None => String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
    };
    DiagnosticsMeta { ignored }
}

/// UTF-16 `(location, length)` ranges: `{3, 5}` pairs (NSStringFromRange),
/// JSON `[[3,5]]` / `[{"location":3,"length":5}]`, or a property list of the
/// same.
fn parse_ranges(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    let pair = |item: &Loose| -> Option<(usize, usize)> {
        match item {
            Loose::List(v) => match v.as_slice() {
                [Loose::Number(a), Loose::Number(b)] => Some((*a as usize, *b as usize)),
                _ => None,
            },
            Loose::Map(map) => {
                let Loose::Number(a) = map_get(map, &["location", "loc"])? else {
                    return None;
                };
                let Loose::Number(b) = map_get(map, &["length", "len"])? else {
                    return None;
                };
                Some((*a as usize, *b as usize))
            }
            Loose::Text(text) => {
                let n = numbers_in(text);
                (n.len() == 2).then(|| (n[0] as usize, n[1] as usize))
            }
            _ => None,
        }
    };
    let ranges: Vec<(usize, usize)> = match load_loose(bytes) {
        Some(Loose::List(items)) => {
            // `[3,5]` (one range) or a list of ranges
            if let [Loose::Number(a), Loose::Number(b)] = items.as_slice() {
                vec![(*a as usize, *b as usize)]
            } else {
                items.iter().filter_map(pair).collect()
            }
        }
        Some(other) => pair(&other).into_iter().collect(),
        None => {
            // "{3, 5}" or "{3, 5}\n{9, 0}"
            String::from_utf8_lossy(bytes)
                .split('}')
                .filter_map(|chunk| {
                    let n = numbers_in(chunk);
                    (n.len() == 2).then(|| (n[0] as usize, n[1] as usize))
                })
                .collect()
        }
    };
    (!ranges.is_empty()).then_some(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Position;

    fn xattrs_work(dir: &Path) -> bool {
        let probe = dir.join("probe");
        fs::write(&probe, "x").unwrap();
        xattr::set(&probe, "user.probe", b"1").is_ok()
    }

    fn sample() -> FileMeta {
        FileMeta {
            language: Some("python".into()),
            grammar: Some("source.python".into()),
            run_command: Some("python3 -u $filename".into()),
            arguments: Some("--fast 'a b'".into()),
            compiler_flags: Some("-O2".into()),
            input: Some("line 1\nline 2\n".into()),
            breakpoints: Some(vec![
                Breakpoint::at(3),
                Breakpoint {
                    line: 9,
                    temporary: true,
                    ..Breakpoint::at(0)
                },
            ]),
            diagnostics: Some(DiagnosticsMeta {
                ignored: vec!["unused variable".into()],
            }),
            selection: Some(vec![Selection::new(
                Position::new(1, 2),
                Position::new(1, 7),
            )]),
            ..FileMeta::default()
        }
    }

    #[test]
    fn round_trips_through_xattrs() {
        let dir = tempfile::tempdir().unwrap();
        if !xattrs_work(dir.path()) {
            eprintln!("skipped: no xattr support");
            return;
        }
        let path = dir.path().join("a.py");
        fs::write(&path, "print(1)").unwrap();
        let store = MetaStore::new();
        assert_eq!(store.write(&path, &sample()).unwrap(), MetaBackend::Xattr);
        assert_eq!(store.read(&path), sample());
        assert_eq!(
            xattr::get(&path, "ScratchRunner:Language")
                .unwrap()
                .unwrap(),
            b"python"
        );
        assert!(
            xattr::get(&path, "CodeRunner:Language").unwrap().is_none(),
            "read-only compat never writes"
        );
    }

    #[test]
    fn unset_fields_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        if !xattrs_work(dir.path()) {
            eprintln!("skipped: no xattr support");
            return;
        }
        let path = dir.path().join("a.py");
        fs::write(&path, "x").unwrap();
        let store = MetaStore::new();
        store.write(&path, &sample()).unwrap();
        let mut smaller = sample();
        smaller.breakpoints = None;
        smaller.input = None;
        store.write(&path, &smaller).unwrap();
        let read = store.read(&path);
        assert_eq!(read.breakpoints, None);
        assert_eq!(read.input, None);
        assert_eq!(read.language.as_deref(), Some("python"));
        store.clear(&path);
        assert!(store.read(&path).is_empty());
    }

    #[test]
    fn reads_codrunner_attributes_as_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        if !xattrs_work(dir.path()) {
            eprintln!("skipped: no xattr support");
            return;
        }
        let path = dir.path().join("Untitled.ts");
        fs::write(&path, "let a: number = 1;\nlet b = 2;\n").unwrap();
        xattr::set(
            &path,
            "CodeRunner:Language",
            b"8CF92C94-BDD9-4CAB-B52D-271639671821",
        )
        .unwrap();
        xattr::set(&path, "CodeRunner:Arguments", b"--x").unwrap();
        xattr::set(&path, "CodeRunner:Breakpoints", b"[2, 5]").unwrap();
        xattr::set(&path, "CodeRunner:Selection", b"{19, 5}").unwrap();
        xattr::set(&path, "com.apple.TextEncoding", b"utf-8;134217984").unwrap();
        let meta = MetaStore::new().read(&path);
        assert_eq!(meta.language, None);
        assert_eq!(
            meta.language_uuid.as_deref(),
            Some("8CF92C94-BDD9-4CAB-B52D-271639671821")
        );
        assert_eq!(meta.arguments.as_deref(), Some("--x"));
        assert_eq!(
            meta.breakpoints,
            Some(vec![Breakpoint::at(1), Breakpoint::at(4)]),
            "1-based -> 0-based"
        );
        assert_eq!(meta.encoding, Some(TextEncoding::Utf8));
        let selections = meta.selections_for("let a: number = 1;\nlet b = 2;\n");
        assert_eq!(
            selections,
            vec![Selection::new(Position::new(1, 0), Position::new(1, 5))]
        );

        let off = MetaStore::new()
            .with_compat(CodeRunnerCompat::Off)
            .read(&path);
        assert_eq!(off.language_uuid, None);
        assert_eq!(off.arguments, None);
    }

    #[test]
    fn our_attributes_win_over_codrunner() {
        let dir = tempfile::tempdir().unwrap();
        if !xattrs_work(dir.path()) {
            eprintln!("skipped: no xattr support");
            return;
        }
        let path = dir.path().join("f.sh");
        fs::write(&path, "x").unwrap();
        xattr::set(&path, "CodeRunner:Arguments", b"old").unwrap();
        xattr::set(&path, "ScratchRunner:Arguments", b"new").unwrap();
        assert_eq!(
            MetaStore::new().read(&path).arguments.as_deref(),
            Some("new")
        );
    }

    #[test]
    fn read_write_compat_mirrors_plain_values() {
        let dir = tempfile::tempdir().unwrap();
        if !xattrs_work(dir.path()) {
            eprintln!("skipped: no xattr support");
            return;
        }
        let path = dir.path().join("f.py");
        fs::write(&path, "x").unwrap();
        let store = MetaStore::new().with_compat(CodeRunnerCompat::ReadWrite);
        let mut meta = sample();
        meta.language_uuid = Some("UUID-1".into());
        store.write(&path, &meta).unwrap();
        assert_eq!(
            xattr::get(&path, "CodeRunner:Language").unwrap().unwrap(),
            b"UUID-1"
        );
        assert_eq!(
            xattr::get(&path, "CodeRunner:Arguments").unwrap().unwrap(),
            b"--fast 'a b'"
        );
        assert!(
            xattr::get(&path, "CodeRunner:Breakpoints")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn resolves_languages_by_id_uuid_or_name() {
        let registry = scratch_language::LanguageRegistry::builtin();
        let python = registry.get("python").unwrap();
        let by_id = FileMeta {
            language: Some("python".into()),
            ..FileMeta::default()
        };
        assert_eq!(by_id.resolve_language(&registry).unwrap().id, "python");
        if let Some(uuid) = &python.uuid {
            let by_uuid = FileMeta {
                language_uuid: Some(uuid.clone()),
                ..FileMeta::default()
            };
            assert_eq!(by_uuid.resolve_language(&registry).unwrap().id, "python");
        }
        let unknown = FileMeta {
            language: Some("klingon".into()),
            ..FileMeta::default()
        };
        assert!(unknown.resolve_language(&registry).is_none());
    }

    #[test]
    fn sidecar_is_used_when_there_are_no_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let side = dir.path().join("meta");
        let path = dir.path().join("a.py");
        fs::write(&path, "x").unwrap();
        // Force the sidecar path through a store whose xattr write fails: a
        // path that is a *file in a missing directory* would not do, so test
        // the sidecar directly through `read` + a manual sidecar file.
        let store = MetaStore::new().with_sidecar_dir(&side);
        let sidecar = store.sidecar_path(&path).unwrap();
        fs::create_dir_all(&side).unwrap();
        fs::write(&sidecar, serde_json::to_vec(&sample()).unwrap()).unwrap();
        assert_eq!(store.read(&path).language.as_deref(), Some("python"));
        assert_eq!(store.read(&path).breakpoints, sample().breakpoints);
        // a successful xattr write removes the sidecar
        if xattrs_work(dir.path()) {
            assert_eq!(store.write(&path, &sample()).unwrap(), MetaBackend::Xattr);
            assert!(!sidecar.exists());
        }
        // moving
        fs::write(&sidecar, b"{}").unwrap();
        let moved = dir.path().join("b.py");
        fs::rename(&path, &moved).unwrap();
        store.move_sidecar(&path, &moved).unwrap();
        assert!(store.sidecar_path(&moved).unwrap().exists());
        assert!(!sidecar.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn locked_files_fall_back_to_the_sidecar() {
        use std::os::unix::ffi::OsStrExt;
        struct Unlock(std::ffi::CString);
        impl Drop for Unlock {
            fn drop(&mut self) {
                // SAFETY: valid NUL-terminated path.
                unsafe { libc::chflags(self.0.as_ptr(), 0) };
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.py");
        fs::write(&path, "x").unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path; UF_IMMUTABLE is 2.
        if unsafe { libc::chflags(c_path.as_ptr(), 2) } != 0 {
            eprintln!("skipped: chflags not permitted here");
            return;
        }
        let _unlock = Unlock(c_path);
        let side = dir.path().join("meta");
        let store = MetaStore::new().with_sidecar_dir(&side);
        match store.write(&path, &sample()) {
            Ok(MetaBackend::Sidecar) => {
                assert_eq!(store.read(&path).language.as_deref(), Some("python"));
                assert_eq!(store.read(&path).breakpoints, sample().breakpoints);
            }
            Ok(MetaBackend::Xattr) => {
                eprintln!("note: this file system allows attributes on locked files")
            }
            Err(error) => panic!("unexpected failure: {error}"),
        }
        let without = MetaStore::new();
        if let Err(error) = without.write(&path, &sample()) {
            assert!(matches!(error, MetaError::Unsupported { .. }));
        }
    }

    #[test]
    fn write_without_sidecar_reports_unsupported_for_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let err = MetaStore::new()
            .write(&dir.path().join("missing"), &sample())
            .unwrap_err();
        assert!(matches!(err, MetaError::Io { .. }));
    }

    #[test]
    fn sidecar_names_are_stable_and_distinct() {
        let store = MetaStore::new().with_sidecar_dir("/meta");
        let a = store.sidecar_path(Path::new("/x/a.py")).unwrap();
        let b = store.sidecar_path(Path::new("/y/a.py")).unwrap();
        assert_ne!(a, b);
        assert_eq!(a, store.sidecar_path(Path::new("/x/a.py")).unwrap());
        assert!(a.to_string_lossy().ends_with("-a.py.json"));
    }

    #[test]
    fn breakpoint_parsers_accept_many_shapes() {
        let lines = |b: &[u8], one| {
            parse_breakpoints(b, one)
                .unwrap()
                .iter()
                .map(|b| b.line)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(b"[3, 1, 3]", false), [1, 3]);
        assert_eq!(
            lines(br#"[{"line": 4}, {"line": 7, "temporary": true}]"#, false),
            [4, 7]
        );
        assert_eq!(lines(b"2 5 9", true), [1, 4, 8]);
        assert_eq!(lines(br#"{"lines": [2]}"#, true), [1]);
        assert_eq!(lines(b"", false), Vec::<u32>::new());
        let mut buffer = Vec::new();
        plist::to_writer_binary(&mut buffer, &vec![3_i64, 6]).unwrap();
        assert_eq!(lines(&buffer, true), [2, 5]);
        assert!(parse_breakpoints(b"\"str\"", false).is_none());
    }

    #[test]
    fn range_and_diagnostics_parsers() {
        assert_eq!(parse_ranges(b"{3, 5}\n{9, 0}"), Some(vec![(3, 5), (9, 0)]));
        assert_eq!(
            parse_ranges(br#"[{"location":2,"length":4}]"#),
            Some(vec![(2, 4)])
        );
        assert_eq!(parse_ranges(b"[[1,2],[3,4]]"), Some(vec![(1, 2), (3, 4)]));
        assert_eq!(parse_ranges(b"[7,1]"), Some(vec![(7, 1)]));
        assert_eq!(parse_ranges(b"garbage"), None);
        assert_eq!(parse_diagnostics(br#"["a","b"]"#).ignored, ["a", "b"]);
        assert_eq!(parse_diagnostics(b"one\n\n two \n").ignored, ["one", "two"]);
        assert_eq!(parse_diagnostics(br#"{"ignored":["x"]}"#).ignored, ["x"]);
    }
}
