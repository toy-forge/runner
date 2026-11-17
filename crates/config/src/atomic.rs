//! Whole-file writes that never leave a half-written file behind.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes`: writes a sibling temporary file, flushes it
/// to disk, then renames it over `path`. A reader sees the old contents or
/// the new ones, never a mix. The parent directory is created when missing.
///
/// # Errors
///
/// Any I/O error of creating the directory, writing, syncing or renaming. The
/// temporary file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// [`write_atomic`] for text.
///
/// # Errors
///
/// See [`write_atomic`].
pub fn write_atomic_str(path: &Path, text: &str) -> io::Result<()> {
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.json");
        fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new contents").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new contents");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn creates_the_file_and_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("fresh.txt");
        write_atomic_str(&path, "hello").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn rejects_a_path_without_a_file_name() {
        assert!(write_atomic(Path::new("/"), b"x").is_err());
    }
}
