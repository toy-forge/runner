//! The temporary directory of a run: the unsaved document, build products,
//! the Program Input file and the compile script live there (`CR_TMPDIR`).

use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A unique directory `<root>/ScratchRunner/run-XXXXXX` (made with
/// `mkdtemp`, mode 0700). It is removed, with everything in it, when dropped
/// (unless [`TempRun::keep`] was called).
#[derive(Debug)]
pub struct TempRun {
    dir: PathBuf,
    keep: bool,
}

/// The name of the directory under the temp root that holds all run directories.
pub const TEMP_SUBDIRECTORY: &str = "ScratchRunner";

impl TempRun {
    /// Makes a run directory under `root` (default: the system temp directory).
    ///
    /// # Errors
    ///
    /// The I/O error of creating the directories.
    pub fn create(root: Option<&Path>) -> io::Result<Self> {
        let base = root
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir)
            .join(TEMP_SUBDIRECTORY);
        std::fs::create_dir_all(&base)?;
        let template = base.join("run-XXXXXX");
        let template = CString::new(template.into_os_string().into_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in temp path"))?;
        let mut bytes = template.into_bytes_with_nul();
        // SAFETY: `bytes` is a writable NUL-terminated buffer ending in XXXXXX.
        let result = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        bytes.pop();
        Ok(Self {
            dir: PathBuf::from(OsStr::from_bytes(&bytes)),
            keep: false,
        })
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Leaves the directory in place when dropped (for debugging a run).
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TempRun {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.dir)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(%error, dir = %self.dir.display(), "could not remove run directory");
        }
    }
}

/// Removes run directories under `root` (default: the system temp directory)
/// that were last modified more than `older_than` ago. Call once at launch to
/// clean up after a crash. Returns how many were removed.
pub fn sweep_stale_runs(root: Option<&Path>, older_than: Duration) -> usize {
    let base = root
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(TEMP_SUBDIRECTORY);
    let Ok(entries) = std::fs::read_dir(&base) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("run-") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= older_than);
        if stale && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_are_unique_private_and_removed_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let a = TempRun::create(Some(root.path())).unwrap();
        let b = TempRun::create(Some(root.path())).unwrap();
        assert_ne!(a.dir(), b.dir());
        assert!(a.dir().starts_with(root.path().join(TEMP_SUBDIRECTORY)));
        assert!(
            a.dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("run-")
        );
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(a.dir()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let dir = a.dir().to_path_buf();
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        drop(a);
        assert!(!dir.exists());
        assert!(b.dir().exists());
    }

    #[test]
    fn keep_leaves_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let mut run = TempRun::create(Some(root.path())).unwrap();
        run.keep();
        let dir = run.dir().to_path_buf();
        drop(run);
        assert!(dir.exists());
    }

    #[test]
    fn sweeping_removes_only_old_run_directories() {
        let root = tempfile::tempdir().unwrap();
        let mut run = TempRun::create(Some(root.path())).unwrap();
        run.keep();
        let other = root.path().join(TEMP_SUBDIRECTORY).join("not-a-run");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(
            sweep_stale_runs(Some(root.path()), Duration::from_secs(3600)),
            0
        );
        assert_eq!(sweep_stale_runs(Some(root.path()), Duration::ZERO), 1);
        assert!(!run.dir().exists());
        assert!(other.exists());
    }
}
