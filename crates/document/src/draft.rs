//! Drafts: a snapshot of a document's unsaved state on disk, so no edit is
//! lost when the app quits or crashes.
//!
//! A [`DraftRecord`] holds the document's metadata and, for a document whose
//! file is not the only copy of its text, the text itself. The previous
//! version of every draft is kept as a recovery snapshot, so a draft that is
//! torn by a crash falls back to the one before.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use scratch_config::write_atomic;
use serde::{Deserialize, Serialize};

use crate::{Document, DocumentId};

/// The saved state of one document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftRecord {
    /// The format version of the record.
    #[serde(default = "version")]
    pub version: u32,
    /// The document's metadata (identity, path, language, encoding, view
    /// state...).
    pub document: Document,
    /// The text (with `\n` line endings) when the draft is the only copy:
    /// unsaved edits of a document backed by a user's file. `None` for an
    /// untitled document, whose text is its file in the Unsaved directory.
    #[serde(default)]
    pub content: Option<String>,
    /// When the draft was written (ms since the epoch).
    #[serde(default)]
    pub saved_ms: u64,
}

fn version() -> u32 {
    1
}

impl DraftRecord {
    /// A record for `document` with its text.
    pub fn with_content(document: Document, content: String, saved_ms: u64) -> Self {
        Self {
            version: version(),
            document,
            content: Some(content),
            saved_ms,
        }
    }

    /// A record for an untitled `document` whose text is in its file.
    pub fn manifest(document: Document, saved_ms: u64) -> Self {
        Self {
            version: version(),
            document,
            content: None,
            saved_ms,
        }
    }

    /// The document's identity.
    pub fn id(&self) -> DocumentId {
        self.document.id
    }
}

/// Reads and writes drafts under a drafts directory, keeping the previous
/// snapshot of each in a recovery directory.
#[derive(Debug, Clone)]
pub struct DraftStore {
    drafts: PathBuf,
    recovery: PathBuf,
}

impl DraftStore {
    /// A store over `drafts` and `recovery`. The directories are created on
    /// the first write.
    pub fn new(drafts: impl Into<PathBuf>, recovery: impl Into<PathBuf>) -> Self {
        Self {
            drafts: drafts.into(),
            recovery: recovery.into(),
        }
    }

    fn file_name(id: DocumentId) -> String {
        format!("{id}.json")
    }

    fn draft_path(&self, id: DocumentId) -> PathBuf {
        self.drafts.join(Self::file_name(id))
    }

    fn recovery_path(&self, id: DocumentId) -> PathBuf {
        self.recovery.join(Self::file_name(id))
    }

    /// The drafts directory.
    pub fn drafts_dir(&self) -> &Path {
        &self.drafts
    }

    /// The recovery directory.
    pub fn recovery_dir(&self) -> &Path {
        &self.recovery
    }

    /// Writes `record`. The draft it replaces is first kept as the recovery
    /// snapshot, then the new one is renamed into place, so a crash leaves
    /// either the old draft or the new one, and a readable snapshot either way.
    ///
    /// # Errors
    ///
    /// I/O errors writing either file.
    pub fn save(&self, record: &DraftRecord) -> io::Result<()> {
        let id = record.id();
        let bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        let path = self.draft_path(id);
        match fs::read(&path) {
            Ok(previous) if previous != bytes => {
                write_atomic(&self.recovery_path(id), &previous)?;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        write_atomic(&path, &bytes)
    }

    /// Reads the draft of `id`. A draft that does not parse is replaced by
    /// its recovery snapshot, if that one reads. `Ok(None)` means there is no
    /// draft at all.
    ///
    /// # Errors
    ///
    /// The error of the recovery snapshot when both are unreadable.
    pub fn load(&self, id: DocumentId) -> io::Result<Option<DraftRecord>> {
        match read_record(&self.draft_path(id)) {
            Ok(record) => Ok(Some(record)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                tracing::warn!(%id, %error, "draft is unreadable; trying its recovery snapshot");
                read_record(&self.recovery_path(id)).map(Some)
            }
        }
    }

    /// The ids that have a draft (or only a recovery snapshot), in no
    /// particular order. Unparseable file names are skipped.
    pub fn ids(&self) -> Vec<DocumentId> {
        let mut ids: Vec<DocumentId> = [&self.drafts, &self.recovery]
            .into_iter()
            .filter_map(|dir| fs::read_dir(dir).ok())
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json")?.parse::<DocumentId>().ok()
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Deletes the draft and its recovery snapshot. Missing files are not an
    /// error.
    ///
    /// # Errors
    ///
    /// I/O errors other than "not found".
    pub fn remove(&self, id: DocumentId) -> io::Result<()> {
        for path in [self.draft_path(id), self.recovery_path(id)] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn read_record(path: &Path) -> io::Result<DraftRecord> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: DocumentId, content: &str, revision: u64) -> DraftRecord {
        let mut document = Document::untitled(PathBuf::from("/u/Untitled.py"), "python", 1);
        document.id = id;
        document.revision = revision;
        DraftRecord::with_content(document, content.to_owned(), 10)
    }

    fn store() -> (tempfile::TempDir, DraftStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DraftStore::new(dir.path().join("drafts"), dir.path().join("recovery"));
        (dir, store)
    }

    #[test]
    fn round_trips_a_draft() {
        let (_dir, store) = store();
        let id = DocumentId::new();
        let draft = record(id, "print('hi')", 3);
        store.save(&draft).unwrap();
        assert_eq!(store.load(id).unwrap(), Some(draft));
    }

    #[test]
    fn missing_draft_is_none() {
        let (_dir, store) = store();
        assert_eq!(store.load(DocumentId::new()).unwrap(), None);
    }

    #[test]
    fn keeps_the_previous_snapshot_for_recovery() {
        let (dir, store) = store();
        let id = DocumentId::new();
        store.save(&record(id, "first", 1)).unwrap();
        store.save(&record(id, "second", 2)).unwrap();
        let previous =
            read_record(&dir.path().join("recovery").join(format!("{id}.json"))).unwrap();
        assert_eq!(previous.content.as_deref(), Some("first"));
    }

    #[test]
    fn corrupt_draft_falls_back_to_recovery() {
        let (dir, store) = store();
        let id = DocumentId::new();
        store.save(&record(id, "good", 1)).unwrap();
        store.save(&record(id, "newer", 2)).unwrap();
        fs::write(
            dir.path().join("drafts").join(format!("{id}.json")),
            b"{ trunc",
        )
        .unwrap();
        assert_eq!(
            store.load(id).unwrap().unwrap().content.as_deref(),
            Some("good")
        );
    }

    #[test]
    fn corrupt_draft_without_recovery_is_an_error() {
        let (dir, store) = store();
        let id = DocumentId::new();
        fs::create_dir_all(dir.path().join("drafts")).unwrap();
        fs::write(
            dir.path().join("drafts").join(format!("{id}.json")),
            b"{ trunc",
        )
        .unwrap();
        assert!(store.load(id).is_err());
    }

    #[test]
    fn remove_deletes_both_files_and_tolerates_missing_ones() {
        let (_dir, store) = store();
        let id = DocumentId::new();
        store.save(&record(id, "a", 1)).unwrap();
        store.save(&record(id, "b", 2)).unwrap();
        store.remove(id).unwrap();
        store.remove(id).unwrap();
        assert_eq!(store.load(id).unwrap(), None);
    }

    #[test]
    fn lists_ids_and_skips_junk() {
        let (dir, store) = store();
        let (a, b) = (DocumentId::new(), DocumentId::new());
        store.save(&record(a, "a", 1)).unwrap();
        store.save(&record(b, "b", 1)).unwrap();
        fs::write(dir.path().join("drafts").join("notes.txt"), b"x").unwrap();
        fs::write(dir.path().join("drafts").join("nope.json"), b"x").unwrap();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(store.ids(), expected);
    }

    #[test]
    fn records_from_older_files_still_parse() {
        let id = DocumentId::new();
        let json = format!(r#"{{"document":{{"id":"{id}","language":"go"}}}}"#);
        let parsed: DraftRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id(), id);
        assert_eq!(parsed.content, None);
        assert_eq!(parsed.version, 1);
    }
}
