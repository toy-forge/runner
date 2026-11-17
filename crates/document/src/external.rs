//! External change detection: has the file changed under the document?
//!
//! [`check_external`] compares the file on disk with the [`FileStamp`] the
//! document was loaded or saved with: first the cheap modification time and
//! size, and when they differ the content hash, so a `touch` or a save by
//! another tool that wrote the same bytes is not a change. [`decide`] maps the
//! result and the document's dirty flag to what CodeRunner does: reload
//! silently when the document is clean, raise a conflict when it has unsaved
//! edits.

use std::fs;
use std::io;
use std::path::Path;

use crate::fileio::{FileStamp, hash_bytes, resolve_symlink};

/// How the file on disk relates to the document's stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalState {
    /// Same time and size, or same content.
    Unchanged,
    /// Same content but a different modification time: refresh the stamp and
    /// do nothing else (carries the new stamp).
    Touched(FileStamp),
    /// The content differs (carries the new stamp).
    Modified(FileStamp),
    /// The file is gone (deleted or moved away).
    Deleted,
}

/// Compares the file at `path` with `stamp`.
///
/// # Errors
///
/// I/O errors other than "not found" (a file that exists but cannot be read).
pub fn check_external(path: &Path, stamp: &FileStamp) -> io::Result<ExternalState> {
    let target = resolve_symlink(path).target;
    let meta = match fs::metadata(&target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ExternalState::Deleted),
        Err(error) => return Err(error),
    };
    if meta.modified().ok() == stamp.modified && meta.len() == stamp.size {
        return Ok(ExternalState::Unchanged);
    }
    let bytes = match fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ExternalState::Deleted),
        Err(error) => return Err(error),
    };
    let fresh = FileStamp::new(&meta, &bytes);
    if hash_bytes(&bytes) == stamp.content_hash && bytes.len() as u64 == stamp.size {
        Ok(ExternalState::Touched(fresh))
    } else {
        Ok(ExternalState::Modified(fresh))
    }
}

/// What the app should do about an [`ExternalState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAction {
    /// Nothing.
    Nothing,
    /// Only update the stored stamp.
    UpdateStamp(FileStamp),
    /// Reload the file into the clean document without asking.
    SilentReload(FileStamp),
    /// The document has unsaved edits and the file changed: show the
    /// conflict (keep mine / revert to disk / compare).
    Conflict(FileStamp),
    /// The file disappeared and the document is clean: keep the text, mark
    /// the document as having no file on disk.
    FileDeleted,
    /// The file disappeared and the document has unsaved edits: keep editing,
    /// saving will recreate the file.
    FileDeletedWhileDirty,
}

/// The policy: clean documents follow the disk silently, dirty ones conflict.
pub fn decide(state: ExternalState, dirty: bool) -> ExternalAction {
    match (state, dirty) {
        (ExternalState::Unchanged, _) => ExternalAction::Nothing,
        (ExternalState::Touched(stamp), _) => ExternalAction::UpdateStamp(stamp),
        (ExternalState::Modified(stamp), false) => ExternalAction::SilentReload(stamp),
        (ExternalState::Modified(stamp), true) => ExternalAction::Conflict(stamp),
        (ExternalState::Deleted, false) => ExternalAction::FileDeleted,
        (ExternalState::Deleted, true) => ExternalAction::FileDeletedWhileDirty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn write_with_mtime(path: &Path, bytes: &[u8], mtime: SystemTime) {
        fs::write(path, bytes).unwrap();
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    #[test]
    fn unchanged_when_nothing_happened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "hello").unwrap();
        let stamp = FileStamp::of_path(&path).unwrap();
        assert_eq!(
            check_external(&path, &stamp).unwrap(),
            ExternalState::Unchanged
        );
    }

    #[test]
    fn touch_with_same_content_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "hello").unwrap();
        let stamp = FileStamp::of_path(&path).unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        write_with_mtime(&path, b"hello", later);
        match check_external(&path, &stamp).unwrap() {
            ExternalState::Touched(fresh) => assert_ne!(fresh.modified, stamp.modified),
            other => panic!("expected Touched, got {other:?}"),
        }
    }

    #[test]
    fn different_content_is_modified_even_with_the_same_size_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let t = SystemTime::now() - Duration::from_secs(3600);
        write_with_mtime(&path, b"aaaaa", t);
        let stamp = FileStamp::of_path(&path).unwrap();
        // same size, newer mtime, different bytes
        write_with_mtime(&path, b"bbbbb", t + Duration::from_secs(5));
        assert!(matches!(
            check_external(&path, &stamp).unwrap(),
            ExternalState::Modified(_)
        ));
    }

    #[test]
    fn deleted_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "x").unwrap();
        let stamp = FileStamp::of_path(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            check_external(&path, &stamp).unwrap(),
            ExternalState::Deleted
        );
    }

    #[test]
    fn policy_is_silent_when_clean_and_conflicting_when_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "x").unwrap();
        let stamp = FileStamp::of_path(&path).unwrap();
        assert_eq!(
            decide(ExternalState::Unchanged, true),
            ExternalAction::Nothing
        );
        assert_eq!(
            decide(ExternalState::Modified(stamp), false),
            ExternalAction::SilentReload(stamp)
        );
        assert_eq!(
            decide(ExternalState::Modified(stamp), true),
            ExternalAction::Conflict(stamp)
        );
        assert_eq!(
            decide(ExternalState::Touched(stamp), true),
            ExternalAction::UpdateStamp(stamp)
        );
        assert_eq!(
            decide(ExternalState::Deleted, false),
            ExternalAction::FileDeleted
        );
        assert_eq!(
            decide(ExternalState::Deleted, true),
            ExternalAction::FileDeletedWhileDirty
        );
    }
}
