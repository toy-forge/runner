//! File operations behind Rename, Move To, Duplicate, New File and Delete:
//! with conflict reporting instead of silent overwrites, extended attributes
//! carried along, and Trash instead of deletion.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fileio::copy_xattrs;
use crate::naming::{self, NameError};

/// What can go wrong in a file operation.
#[derive(Debug, thiserror::Error)]
pub enum FileOpError {
    /// The destination exists (and is not the source). Ask to replace it and
    /// retry with `overwrite = true`.
    #[error("{} already exists", .existing.display())]
    Conflict {
        /// The file in the way.
        existing: PathBuf,
    },
    /// The source does not exist.
    #[error("{} does not exist", .0.display())]
    NotFound(PathBuf),
    /// The new name is not acceptable.
    #[error("invalid name: {0}")]
    InvalidName(#[from] NameError),
    /// The destination is inside the source folder.
    #[error("cannot move a folder into itself")]
    IntoItself,
    /// An I/O error.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> FileOpError + '_ {
    move |source| {
        if source.kind() == io::ErrorKind::NotFound && !path.exists() {
            FileOpError::NotFound(path.to_path_buf())
        } else {
            FileOpError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    }
}

/// Whether two paths are the same file (hard link, or a case-only difference
/// on a case-insensitive volume).
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Renames the file or folder at `path` to `new_name` in the same directory.
/// A rename that only changes the case is allowed on case-insensitive volumes.
/// Returns the new path.
///
/// # Errors
///
/// [`FileOpError::InvalidName`], [`FileOpError::Conflict`] (without
/// `overwrite`), [`FileOpError::NotFound`] or [`FileOpError::Io`].
pub fn rename(path: &Path, new_name: &str, overwrite: bool) -> Result<PathBuf, FileOpError> {
    naming::validate_file_name(new_name)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let destination = parent.join(new_name.trim());
    move_to(path, &destination, overwrite)
}

/// Moves `path` to `destination` (a full path, including the new file name).
/// Across volumes the file is copied (with its extended attributes) and the
/// original removed. Returns `destination`.
///
/// # Errors
///
/// As [`rename`], plus [`FileOpError::IntoItself`].
pub fn move_to(path: &Path, destination: &Path, overwrite: bool) -> Result<PathBuf, FileOpError> {
    if fs::symlink_metadata(path).is_err() {
        return Err(FileOpError::NotFound(path.to_path_buf()));
    }
    if path == destination {
        return Ok(destination.to_path_buf());
    }
    if destination.starts_with(path) {
        return Err(FileOpError::IntoItself);
    }
    let exists = fs::symlink_metadata(destination).is_ok();
    if exists && !same_file(path, destination) {
        if !overwrite {
            return Err(FileOpError::Conflict {
                existing: destination.to_path_buf(),
            });
        }
        if destination.is_dir() && !destination.is_symlink() {
            return Err(FileOpError::Conflict {
                existing: destination.to_path_buf(),
            });
        }
    }
    match fs::rename(path, destination) {
        Ok(()) => Ok(destination.to_path_buf()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            copy_recursively(path, destination).map_err(io_err(destination))?;
            let removal = if path.is_dir() && !path.is_symlink() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
            removal.map_err(io_err(path))?;
            Ok(destination.to_path_buf())
        }
        Err(error) => Err(io_err(path)(error)),
    }
}

fn copy_recursively(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &to.join(entry.file_name()))?;
        }
        let _ = fs::set_permissions(to, meta.permissions());
        copy_xattrs(from, to);
    } else if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        let _ = fs::remove_file(to);
        std::os::unix::fs::symlink(target, to)?;
    } else {
        fs::copy(from, to)?; // keeps permissions
        copy_xattrs(from, to);
    }
    Ok(())
}

/// Copies `from` to `to` (a full path), keeping permissions and extended
/// attributes.
///
/// # Errors
///
/// [`FileOpError::Conflict`] when `to` exists and `overwrite` is false; I/O
/// errors.
pub fn copy_to(from: &Path, to: &Path, overwrite: bool) -> Result<PathBuf, FileOpError> {
    if fs::symlink_metadata(from).is_err() {
        return Err(FileOpError::NotFound(from.to_path_buf()));
    }
    if fs::symlink_metadata(to).is_ok() && !overwrite {
        return Err(FileOpError::Conflict {
            existing: to.to_path_buf(),
        });
    }
    copy_recursively(from, to).map_err(io_err(to))?;
    Ok(to.to_path_buf())
}

/// Duplicates `path` next to itself as `<name> copy.ext`, `<name> copy 2.ext`
/// ... Returns the new path.
///
/// # Errors
///
/// I/O errors.
pub fn duplicate(path: &Path) -> Result<PathBuf, FileOpError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| FileOpError::NotFound(path.to_path_buf()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let copy_name = naming::duplicate_name(&name, naming::dir_has(parent));
    copy_to(path, &parent.join(copy_name), false)
}

/// Creates an empty file `name` in `parent`; the name is made unique
/// (`name 2.ext`) when it exists. Returns the path.
///
/// # Errors
///
/// [`FileOpError::InvalidName`] or I/O errors.
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, FileOpError> {
    naming::validate_file_name(name)?;
    let unique = naming::numbered_name(name.trim(), naming::dir_has(parent));
    let path = parent.join(unique);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(io_err(&path))?;
    Ok(path)
}

/// Creates a folder `name` in `parent` (made unique like [`create_file`]).
///
/// # Errors
///
/// [`FileOpError::InvalidName`] or I/O errors.
pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, FileOpError> {
    naming::validate_file_name(name)?;
    let unique = naming::numbered_name(name.trim(), naming::dir_has(parent));
    let path = parent.join(unique);
    fs::create_dir(&path).map_err(io_err(&path))?;
    Ok(path)
}

/// Where a trashed item went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trashed {
    /// Through the system Trash (the Finder's *Put Back* works).
    System,
    /// Moved into a Trash folder by hand; carries the new location.
    Moved(PathBuf),
}

/// Moves `path` to the Trash. The system's own mechanism is used first; when
/// it fails the item is moved into `~/.Trash` under a unique name.
///
/// # Errors
///
/// [`FileOpError::NotFound`] or I/O errors from the fallback.
pub fn trash_item(path: &Path) -> Result<Trashed, FileOpError> {
    if fs::symlink_metadata(path).is_err() {
        return Err(FileOpError::NotFound(path.to_path_buf()));
    }
    match trash::delete(path) {
        Ok(()) => Ok(Trashed::System),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "system trash failed; moving to ~/.Trash");
            let home =
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or_else(|| FileOpError::Io {
                        path: path.to_path_buf(),
                        source: io::Error::other("HOME is not set"),
                    })?;
            move_to_trash_dir(path, &home.join(".Trash")).map(Trashed::Moved)
        }
    }
}

/// Moves `path` into the folder `trash_dir` (created if needed), numbering
/// the name when it is taken (`a 2.py`). Returns the new path.
///
/// # Errors
///
/// I/O errors.
pub fn move_to_trash_dir(path: &Path, trash_dir: &Path) -> Result<PathBuf, FileOpError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| FileOpError::NotFound(path.to_path_buf()))?;
    fs::create_dir_all(trash_dir).map_err(io_err(trash_dir))?;
    let unique = naming::numbered_name(&name, naming::dir_has(trash_dir));
    move_to(path, &trash_dir.join(unique), false)
}

/// What the Save panel starts with (`Document prepareSavePanel:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePanelDefaults {
    /// The folder to start in; `None` leaves the system default.
    pub directory: Option<PathBuf>,
    /// The suggested file name, with the language's extension.
    pub file_name: String,
    /// The extensions of the language (the panel also allows other types).
    pub allowed_extensions: Vec<String>,
}

/// The Save panel defaults: the folder the document was duplicated from, else
/// (with the sidebar visible) the folder new files go to in the file navigator,
/// else the system's choice. The name is the document's current file name.
pub fn save_panel_defaults(
    document_title: &str,
    duplicated_from: Option<&Path>,
    sidebar_visible: bool,
    navigator_directory: Option<&Path>,
    language_extensions: &[String],
) -> SavePanelDefaults {
    let directory = duplicated_from
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            sidebar_visible
                .then_some(navigator_directory)
                .flatten()
                .map(Path::to_path_buf)
        });
    SavePanelDefaults {
        directory,
        file_name: document_title.to_owned(),
        allowed_extensions: language_extensions.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn save_panel_directory_rules() {
        let exts = vec!["py".to_owned()];
        let from = Path::new("/src/orig.py");
        let nav = Path::new("/nav");
        let a = save_panel_defaults("Untitled.py", Some(from), true, Some(nav), &exts);
        assert_eq!(a.directory.as_deref(), Some(Path::new("/src")));
        let b = save_panel_defaults("Untitled.py", None, true, Some(nav), &exts);
        assert_eq!(b.directory.as_deref(), Some(nav));
        let c = save_panel_defaults("Untitled.py", None, false, Some(nav), &exts);
        assert_eq!(c.directory, None);
        assert_eq!(
            (c.file_name.as_str(), c.allowed_extensions.as_slice()),
            ("Untitled.py", &exts[..])
        );
    }

    #[test]
    fn rename_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.py", "1");
        let b = rename(&a, "b.py", false).unwrap();
        assert_eq!(b, dir.path().join("b.py"));
        assert!(!a.exists() && b.exists());
    }

    #[test]
    fn rename_reports_conflicts_and_can_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.py", "A");
        let b = write(dir.path(), "b.py", "B");
        match rename(&a, "b.py", false) {
            Err(FileOpError::Conflict { existing }) => assert_eq!(existing, b),
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&a).unwrap(), "A", "nothing happened");
        rename(&a, "b.py", true).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");
        assert!(!a.exists());
    }

    #[test]
    fn case_only_rename_is_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "readme.md", "x");
        let upper = rename(&a, "README.md", false).unwrap();
        assert!(upper.exists());
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["README.md"]);
    }

    #[test]
    fn rename_validates_the_name_and_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.py", "1");
        assert!(matches!(
            rename(&a, "x/y", false),
            Err(FileOpError::InvalidName(_))
        ));
        assert!(matches!(
            rename(&a, "", false),
            Err(FileOpError::InvalidName(_))
        ));
        assert!(matches!(
            rename(&dir.path().join("zzz"), "q", false),
            Err(FileOpError::NotFound(_))
        ));
    }

    #[test]
    fn move_to_another_folder_and_not_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let a = write(dir.path(), "a.py", "1");
        let moved = move_to(&a, &sub.join("a.py"), false).unwrap();
        assert!(moved.exists() && !a.exists());
        assert!(matches!(
            move_to(&sub, &sub.join("inner"), false),
            Err(FileOpError::IntoItself)
        ));
    }

    #[test]
    fn overwriting_never_replaces_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a", "1");
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        assert!(matches!(
            move_to(&a, &folder, true),
            Err(FileOpError::Conflict { .. })
        ));
    }

    #[test]
    fn duplicate_numbers_copies_and_keeps_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.py", "body");
        let has_xattr = xattr::set(&a, "user.scratch-test", b"v").is_ok();
        let one = duplicate(&a).unwrap();
        let two = duplicate(&a).unwrap();
        assert_eq!(one.file_name().unwrap(), "a copy.py");
        assert_eq!(two.file_name().unwrap(), "a copy 2.py");
        assert_eq!(fs::read_to_string(&two).unwrap(), "body");
        if has_xattr {
            assert_eq!(
                xattr::get(&one, "user.scratch-test").unwrap().as_deref(),
                Some(&b"v"[..])
            );
        }
        let three = duplicate(&one).unwrap();
        assert_eq!(three.file_name().unwrap(), "a copy 3.py");
    }

    #[test]
    fn duplicates_folders_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("proj");
        fs::create_dir_all(folder.join("src")).unwrap();
        write(&folder.join("src"), "main.rs", "fn main() {}");
        let copy = duplicate(&folder).unwrap();
        assert_eq!(copy.file_name().unwrap(), "proj copy");
        assert_eq!(
            fs::read_to_string(copy.join("src/main.rs")).unwrap(),
            "fn main() {}"
        );
    }

    #[test]
    fn copy_reports_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a", "1");
        let b = write(dir.path(), "b", "2");
        assert!(matches!(
            copy_to(&a, &b, false),
            Err(FileOpError::Conflict { .. })
        ));
        copy_to(&a, &b, true).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "1");
    }

    #[test]
    fn creates_files_and_folders_with_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let one = create_file(dir.path(), "new.py").unwrap();
        let two = create_file(dir.path(), "new.py").unwrap();
        assert_eq!(one.file_name().unwrap(), "new.py");
        assert_eq!(two.file_name().unwrap(), "new 2.py");
        let folder = create_folder(dir.path(), "untitled folder").unwrap();
        let folder2 = create_folder(dir.path(), "untitled folder").unwrap();
        assert!(folder.is_dir());
        assert_eq!(folder2.file_name().unwrap(), "untitled folder 2");
        assert!(create_file(dir.path(), "a/b").is_err());
    }

    #[test]
    fn trash_folder_fallback_numbers_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join("Trash");
        let a = write(dir.path(), "a.py", "1");
        let first = move_to_trash_dir(&a, &trash).unwrap();
        let again = write(dir.path(), "a.py", "2");
        let second = move_to_trash_dir(&again, &trash).unwrap();
        assert_eq!(first.file_name().unwrap(), "a.py");
        assert_eq!(second.file_name().unwrap(), "a 2.py");
        assert!(!a.exists());
        assert!(matches!(
            trash_item(&dir.path().join("missing")),
            Err(FileOpError::NotFound(_))
        ));
    }
}
