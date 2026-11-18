//! Versioned workbench snapshots. Each launch writes a new journal file; legacy
//! session and draft files are read-only. A single writer preserves write order.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use scratch_config::{Paths, write_atomic};
use scratch_document::{Document, DocumentId, DraftRecord, FileMeta, ReadOptions, Session, UnsavedStore, load_text, now_ms};
use scratch_language::LanguageRegistry;
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

/// A complete editable document, including its unsaved text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDocument {
    pub document: Document,
    pub title: String,
    pub text: String,
    /// Per-document diagnostics state (ignored messages, disabled
    /// diagnostics; Diagnostics > "Disable Diagnostics For …").
    #[serde(default)]
    pub diagnostics: Option<scratch_diagnostics::settings::DocumentDiagnosticsData>,
}

fn default_true() -> bool { true }
fn default_sidebar_width() -> f32 { 240.0 }
fn default_console_height() -> f32 { 200.0 }
fn default_console_width() -> f32 { 360.0 }
fn default_docs_width() -> f32 { 300.0 }

/// Saved window frame in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A complete, self-contained workbench snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub documents: Vec<SavedDocument>,
    pub active: Option<DocumentId>,
    #[serde(default = "default_console_height")]
    pub console_height: f32,
    #[serde(default)]
    pub console_visible: bool,
    #[serde(default)]
    pub console_right: bool,
    #[serde(default = "default_console_width")]
    pub console_width: f32,
    #[serde(default = "default_true")]
    pub sidebar_visible: bool,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    #[serde(default)]
    pub docs_visible: bool,
    #[serde(default = "default_docs_width")]
    pub docs_width: f32,
    #[serde(default = "default_true")]
    pub toolbar_visible: bool,
    #[serde(default = "default_true")]
    pub tab_bar_visible: bool,
    #[serde(default)]
    pub window_bounds: Option<SavedBounds>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            documents: Vec::new(),
            active: None,
            console_height: default_console_height(),
            console_visible: false,
            console_right: false,
            console_width: default_console_width(),
            sidebar_visible: true,
            sidebar_width: default_sidebar_width(),
            docs_visible: false,
            docs_width: default_docs_width(),
            toolbar_visible: true,
            tab_bar_visible: true,
            window_bounds: None,
        }
    }
}

fn parse(bytes: &[u8]) -> io::Result<Snapshot> {
    let snapshot: Snapshot = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if snapshot.version != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsupported workbench version {}", snapshot.version)));
    }
    let mut seen = HashSet::new();
    if snapshot.documents.iter().any(|doc| doc.document.id == DocumentId::nil() || !seen.insert(doc.document.id)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or duplicate document identity"));
    }
    Ok(snapshot)
}

fn read_snapshot(path: &Path) -> io::Result<Snapshot> {
    parse(&fs::read(path)?)
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("backup")
}

/// Writes a snapshot and retains the preceding valid snapshot. Damaged files
/// are never replaced, even when an external actor modifies a running journal.
fn save_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let bytes = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
    parse(&bytes)?;
    match fs::read(path) {
        Ok(previous) => {
            parse(&previous)?;
            let backup = backup_path(path);
            if backup.exists() {
                read_snapshot(&backup)?;
            }
            write_atomic(&backup, &previous)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_atomic(path, &bytes)
}

/// Reads the newest valid workbench journal, trying its backup first if the
/// latest write was damaged. No file is modified during restore.
pub fn restore(paths: &Paths) -> (Snapshot, Option<String>) {
    let dir = paths.root().join("WorkbenchSessions");
    let mut entries = match fs::read_dir(&dir) {
        Ok(entries) => entries.filter_map(Result::ok).filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json")).collect::<Vec<_>>(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return (Snapshot::default(), Some(format!("Cannot read recovery folder: {error}"))),
    };
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.metadata().and_then(|m| m.modified()).ok()));
    let mut warnings = Vec::new();
    for entry in entries {
        let path = entry.path();
        match read_snapshot(&path) {
            Ok(snapshot) => return (snapshot, (!warnings.is_empty()).then(|| warnings.join(" "))),
            Err(error) => {
                warnings.push(format!("{} is unreadable ({error}); the file was preserved.", path.display()));
                if let Ok(snapshot) = read_snapshot(&backup_path(&path)) {
                    warnings.push("Restored the previous recovery snapshot.".into());
                    return (snapshot, Some(warnings.join(" ")));
                }
            }
        }
    }
    let (snapshot, legacy_warning) = restore_legacy(paths);
    if let Some(warning) = legacy_warning { warnings.push(warning); }
    (snapshot, (!warnings.is_empty()).then(|| warnings.join(" ")))
}

/// Import known session and draft formats without running migration or cleanup
/// writers. Individual failures are reported and their files are left intact.
fn restore_legacy(paths: &Paths) -> (Snapshot, Option<String>) {
    let path = paths.session();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Snapshot::default(), None),
        Err(error) => return (Snapshot::default(), Some(format!("Cannot read {}: {error}", path.display()))),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => return (Snapshot::default(), Some(format!("Could not restore {}: {error}; it was preserved.", path.display()))),
    };
    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    if version > u64::from(scratch_document::SESSION_VERSION) || !(value.get("tabs").is_some() || value.get("windows").is_some()) {
        return (Snapshot::default(), Some(format!("Unsupported session in {}; it was preserved.", path.display())));
    }
    let session = match Session::parse(&bytes) {
        Ok(session) => session,
        Err(error) => return (Snapshot::default(), Some(format!("Could not restore {}: {error}; it was preserved.", path.display()))),
    };
    let registry = LanguageRegistry::builtin();
    let store = UnsavedStore::new(paths.unsaved());
    let mut result = Snapshot::default();
    let mut warnings = Vec::new();
    for window in &session.windows {
        for tab in &window.tabs {
            let record = match store.drafts().load(tab.id) {
                Ok(record) => record,
                Err(error) => { warnings.push(format!("Draft for {} could not be read: {error}", tab.title)); None }
            };
            let restored = record.and_then(|record| restore_record(record).ok()).or_else(|| {
                let path = tab.path.as_ref()?;
                match load_text(path, &ReadOptions::default()) {
                    Ok(loaded) => {
                        let mut document = Document::from_file(path, &loaded, &FileMeta::default(), &registry, now_ms());
                        document.id = tab.id;
                        document.untitled = tab.untitled;
                        document.language = tab.language.clone();
                        document.language_locked = tab.language_locked;
                        Some(SavedDocument { document, title: tab.title.clone(), text: loaded.text, diagnostics: None })
                    }
                    Err(error) => { warnings.push(format!("{} could not be restored: {error}", path.display())); None }
                }
            });
            match restored {
                Some(doc) => result.documents.push(doc),
                None => warnings.push(format!("{} could not be restored; its previous files were preserved.", tab.title)),
            }
        }
    }
    if let Some(window) = session.active_window.and_then(|i| session.windows.get(i)).or_else(|| session.windows.first()) {
        result.active = window.active;
        result.console_visible = window.chrome.console.visible;
        result.console_height = window.chrome.console.size.unwrap_or(200.0);
    }
    (result, (!warnings.is_empty()).then(|| warnings.join(" ")))
}

fn restore_record(record: DraftRecord) -> io::Result<SavedDocument> {
    let document = record.document;
    let text = match record.content {
        Some(text) => text,
        None => {
            let path = document.path.as_ref().ok_or_else(|| io::Error::other("draft has no text or file"))?;
            load_text(path, &ReadOptions::default()).map_err(io::Error::other)?.text
        }
    };
    let title = document.title();
    Ok(SavedDocument { document, title, text, diagnostics: None })
}

struct Job {
    generation: u64,
    snapshot: Snapshot,
    complete: Option<mpsc::Sender<Result<(), String>>>,
}

/// Ordered, off-thread snapshot writer. `results` must be drained by the UI.
pub struct Journal {
    tx: mpsc::Sender<Job>,
    pub results: mpsc::Receiver<(u64, Result<(), String>)>,
}

impl Journal {
    pub fn new(paths: &Paths) -> Self {
        let path = paths.root().join("WorkbenchSessions").join(format!("{}.json", uuid::Uuid::new_v4()));
        let (tx, rx) = mpsc::channel::<Job>();
        let (results_tx, results) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let outcome = save_snapshot(&path, &job.snapshot).map_err(|error| error.to_string());
                let _ = results_tx.send((job.generation, outcome.clone()));
                if let Some(done) = job.complete { let _ = done.send(outcome); }
            }
        });
        Self { tx, results }
    }

    pub fn write(&self, generation: u64, snapshot: Snapshot) -> Result<(), String> {
        self.tx.send(Job { generation, snapshot, complete: None }).map_err(|error| error.to_string())
    }

    pub fn flush(&self, generation: u64, snapshot: Snapshot) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (complete, rx) = mpsc::channel();
        self.tx.send(Job { generation, snapshot, complete: Some(complete) }).map_err(|error| error.to_string())?;
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_restore_dirty_text_and_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let file = paths.root().join("WorkbenchSessions/session.json");
        let mut doc = Document::default();
        doc.mark_edited(1);
        let first = Snapshot {
            active: Some(doc.id),
            documents: vec![SavedDocument { document: doc, title: "Untitled".into(), text: "first".into(), diagnostics: None }],
            ..Default::default()
        };
        save_snapshot(&file, &first).unwrap();
        let mut second = first.clone();
        second.documents[0].text = "edited".into();
        save_snapshot(&file, &second).unwrap();
        assert_eq!(restore(&paths).0, second);
        fs::write(&file, b"{broken").unwrap();
        let (recovered, warning) = restore(&paths);
        assert_eq!(recovered, first);
        assert!(warning.is_some());
        assert!(save_snapshot(&file, &second).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{broken");
    }

    #[test]
    fn unparseable_legacy_session_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        fs::create_dir_all(paths.root()).unwrap();
        fs::write(paths.session(), b"{ damaged").unwrap();
        let (_, warning) = restore(&paths);
        assert!(warning.is_some());
        assert_eq!(fs::read(paths.session()).unwrap(), b"{ damaged");
    }
}
