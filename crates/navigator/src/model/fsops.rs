//! Non-overwriting filesystem operations for navigator actions.
//!
//! New destinations are created exclusively. A failed recursive copy may leave
//! a partial copy at its new destination; it never removes or replaces an existing item.

use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

/// Why an operation failed.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// The supplied file name is empty or contains a path separator.
    #[error("invalid name \"{0}\"")]
    InvalidName(String),
    /// The destination already exists.
    #[error("\"{0}\" already exists")]
    Exists(PathBuf),
    /// A directory cannot be copied or moved into itself.
    #[error("cannot move or copy a folder into itself")]
    IntoItself,
    /// The operation is not supported for this kind of file or platform.
    #[error("{0}")]
    Unsupported(&'static str),
    /// The underlying I/O error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Collision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Return an error when the destination exists.
    Fail,
    /// Use a new name such as `name copy 2`.
    Uniquify,
    /// Remove the existing item first (the caller confirmed with the user).
    Replace,
}

/// Validate one file name, without accepting relative or absolute paths.
pub fn check_name(name: &str) -> Result<(), FsError> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.contains('/') || n.contains('\0') {
        return Err(FsError::InvalidName(name.to_owned()));
    }
    Ok(())
}

/// Split a filename into its stem and extension; dotfiles have no extension.
pub fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// The UTF-16 selection range of a name's stem.
pub fn stem_range_utf16(name: &str) -> (u32, u32) {
    (0, split_ext(name).0.encode_utf16().count() as u32)
}

fn map_io(error: io::Error, path: &Path) -> FsError {
    if error.kind() == ErrorKind::AlreadyExists {
        FsError::Exists(path.to_path_buf())
    } else {
        FsError::Io(error)
    }
}

/// Create an empty file using exclusive creation.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, FsError> {
    check_name(name)?;
    let path = dir.join(name.trim());
    File::create_new(&path).map_err(|e| map_io(e, &path))?;
    Ok(path)
}

/// Create a directory without replacing an existing entry.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf, FsError> {
    check_name(name)?;
    let path = dir.join(name.trim());
    fs::create_dir(&path).map_err(|e| map_io(e, &path))?;
    Ok(path)
}

// The kernel performs the existence check and rename as one operation. Ordinary
// std::fs::rename overwrites on Unix even if a prior existence check succeeded.
fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    let source = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| FsError::InvalidName(from.display().to_string()))?;
    let destination = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| FsError::InvalidName(to.display().to_string()))?;
    #[cfg(target_os = "macos")]
    // SAFETY: both C strings are valid and NUL-terminated for the duration of the call.
    let result = unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    // SAFETY: both C strings are valid and NUL-terminated for the duration of the call.
    let result = unsafe {
        libc::renameat2(libc::AT_FDCWD, source.as_ptr(), libc::AT_FDCWD, destination.as_ptr(), libc::RENAME_NOREPLACE)
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = {
        let _ = (source, destination);
        return Err(FsError::Unsupported("exclusive rename is unavailable on this platform"));
    };
    if result == 0 { Ok(()) } else { Err(map_io(io::Error::last_os_error(), to)) }
}

/// Rename a single entry within its parent. A conflicting name is rejected.
pub fn rename(from: &Path, name: &str, conflict: Conflict) -> Result<PathBuf, FsError> {
    check_name(name)?;
    let to = from.with_file_name(name.trim());
    if from == to { return Ok(to); }
    match rename_exclusive(from, &to) {
        Err(FsError::Exists(_)) if conflict == Conflict::Replace => {
            remove_any(&to)?;
            fs::rename(from, &to).map_err(|e| map_io(e, &to))?;
            Ok(to)
        }
        Err(FsError::Exists(_)) if conflict == Conflict::Uniquify => {
            let (stem, ext) = split_ext(name.trim());
            let parent = from.parent().ok_or_else(|| FsError::InvalidName(from.display().to_string()))?;
            for index in 2.. {
                let candidate = parent.join(numbered_name(stem, ext, None, index));
                match rename_exclusive(from, &candidate) {
                    Err(FsError::Exists(_)) => continue,
                    Ok(()) => return Ok(candidate),
                    Err(e) => return Err(e),
                }
            }
            unreachable!()
        }
        Ok(()) => Ok(to),
        Err(e) => Err(e),
    }
}

/// Removes a file or a whole folder; the caller asked for [`Conflict::Replace`].
fn remove_any(path: &Path) -> Result<(), FsError> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path).map_err(|e| map_io(e, path))
    } else {
        fs::remove_file(path).map_err(|e| map_io(e, path))
    }
}

fn numbered_name(stem: &str, extension: Option<&str>, word: Option<&str>, index: u32) -> String {
    let mut name = stem.to_owned();
    if let Some(word) = word { name.push(' '); name.push_str(word); }
    if index > 1 { name.push_str(&format!(" {index}")); }
    if let Some(extension) = extension { name.push('.'); name.push_str(extension); }
    name
}

// Each level creates a new entry exclusively. Symlinks are copied as links,
// never followed recursively; sockets, devices and FIFOs are not copied.
fn copy_recursive(from: &Path, to: &Path) -> Result<(), FsError> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        std::os::unix::fs::symlink(target, to).map_err(|e| map_io(e, to))?;
    } else if meta.is_dir() {
        fs::create_dir(to).map_err(|e| map_io(e, to))?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())?;
    } else if meta.is_file() {
        let mut input = File::open(from)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(to).map_err(|e| map_io(e, to))?;
        io::copy(&mut input, &mut output)?;
        output.set_permissions(meta.permissions())?;
    } else {
        return Err(FsError::Unsupported("cannot copy special files"));
    }
    Ok(())
}

fn check_not_inside(src: &Path, dir: &Path) -> Result<(), FsError> {
    // Resolve aliases in the destination as well as in the source: a symlink
    // pointing into a source directory is still inside that directory.
    if fs::symlink_metadata(src)?.is_dir() {
        let source = src.canonicalize()?;
        let destination = dir.canonicalize()?;
        if destination.starts_with(source) { return Err(FsError::IntoItself); }
    }
    Ok(())
}

fn copy_named(src: &Path, dir: &Path, always_copy_name: bool, policy: Conflict) -> Result<PathBuf, FsError> {
    check_not_inside(src, dir)?;
    let name = src.file_name().ok_or_else(|| FsError::InvalidName(src.display().to_string()))?;
    let original = dir.join(name);
    if !always_copy_name {
        match copy_recursive(src, &original) {
            Err(FsError::Exists(_)) if policy == Conflict::Uniquify => {},
            Ok(()) => return Ok(original),
            Err(e) => return Err(e),
        }
    }
    let name = name.to_string_lossy();
    let is_dir = fs::symlink_metadata(src)?.is_dir();
    let (stem, ext) = if is_dir { (name.as_ref(), None) } else { split_ext(&name) };
    for index in 1.. {
        let target = dir.join(numbered_name(stem, ext, Some("copy"), index));
        match copy_recursive(src, &target) {
            Err(FsError::Exists(_)) => continue,
            Ok(()) => return Ok(target),
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Duplicate an item alongside its source without overwriting another item.
pub fn duplicate(path: &Path) -> Result<PathBuf, FsError> {
    let parent = path.parent().ok_or_else(|| FsError::InvalidName(path.display().to_string()))?;
    copy_named(path, parent, true, Conflict::Uniquify)
}

/// Copy an item into a directory. Symlinks are copied as symlinks.
pub fn copy_into(src: &Path, dir: &Path, conflict: Conflict) -> Result<PathBuf, FsError> {
    copy_named(src, dir, false, conflict)
}

/// Move into a directory atomically and without overwriting; cross-volume moves fail.
pub fn move_into(src: &Path, dir: &Path, conflict: Conflict) -> Result<PathBuf, FsError> {
    check_not_inside(src, dir)?;
    if src.parent() == Some(dir) { return Ok(src.to_path_buf()); }
    let name = src.file_name().ok_or_else(|| FsError::InvalidName(src.display().to_string()))?;
    let target = dir.join(name);
    match rename_exclusive(src, &target) {
        Err(FsError::Exists(_)) if conflict == Conflict::Uniquify => {
            let name = name.to_string_lossy();
            let is_dir = fs::symlink_metadata(src)?.is_dir();
            let (stem, ext) = if is_dir { (name.as_ref(), None) } else { split_ext(&name) };
            for index in 1.. {
                let target = dir.join(numbered_name(stem, ext, Some("copy"), index));
                match rename_exclusive(src, &target) {
                    Err(FsError::Exists(_)) => continue,
                    Ok(()) => return Ok(target),
                    Err(e) => return Err(e),
                }
            }
            unreachable!()
        }
        Ok(()) => Ok(target),
        Err(e) => Err(e),
    }
}

/// Decode a local `file:` URL, including non-UTF-8 path bytes.
pub fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = if rest.starts_with('/') { rest } else { rest.strip_prefix("localhost/").map(|_| &rest["localhost".len()..]).filter(|_| !rest.is_empty())? };
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let digits = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(digits).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            if matches!(bytes[i], b'?' | b'#') { return None; }
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.contains(&0) { return None; }
    Some(PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_and_create_reject_existing_entries_without_losing_contents() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("one"), "first").unwrap();
        fs::write(d.path().join("two"), "second").unwrap();
        assert!(matches!(create_file(d.path(), "two"), Err(FsError::Exists(_))));
        assert!(matches!(rename(&d.path().join("one"), "two", Conflict::Fail), Err(FsError::Exists(_))));
        assert_eq!(fs::read(d.path().join("one")).unwrap(), b"first");
        assert_eq!(fs::read(d.path().join("two")).unwrap(), b"second");
        rename(&d.path().join("one"), "three", Conflict::Fail).unwrap();
        assert_eq!(fs::read(d.path().join("three")).unwrap(), b"first");
    }

    #[test]
    fn copy_and_move_never_replace_and_duplicate_uniquifies() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("file.txt");
        fs::write(&src, "source").unwrap();
        let dest = d.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("file.txt"), "keep").unwrap();
        assert!(matches!(move_into(&src, &dest, Conflict::Fail), Err(FsError::Exists(_))));
        assert!(matches!(copy_into(&src, &dest, Conflict::Fail), Err(FsError::Exists(_))));
        assert_eq!(fs::read(dest.join("file.txt")).unwrap(), b"keep");
        let dup = duplicate(&src).unwrap();
        assert_eq!(dup.file_name().unwrap(), "file copy.txt");
        assert_eq!(duplicate(&src).unwrap().file_name().unwrap(), "file copy 2.txt");
        assert_eq!(copy_into(&src, &dest, Conflict::Uniquify).unwrap().file_name().unwrap(), "file copy.txt");
        assert_eq!(fs::read(&src).unwrap(), b"source");
    }

    #[test]
    fn symlink_alias_cannot_copy_folder_into_itself() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("inside")).unwrap();
        let alias = d.path().join("alias");
        std::os::unix::fs::symlink(source.join("inside"), &alias).unwrap();
        assert!(matches!(copy_into(&source, &alias, Conflict::Uniquify), Err(FsError::IntoItself)));
        assert!(matches!(move_into(&source, &alias, Conflict::Fail), Err(FsError::IntoItself)));
    }

    #[test]
    fn url_and_name_validation() {
        assert!(check_name("../other").is_err());
        assert!(check_name("..").is_err());
        assert_eq!(stem_range_utf16("😀x.rs"), (0, 3));
        assert_eq!(path_from_file_url("file:///tmp/a%20b"), Some(PathBuf::from("/tmp/a b")));
        assert_eq!(path_from_file_url("file://remote/tmp/a"), None);
        assert_eq!(path_from_file_url("file:///tmp/%"), None);
        assert_eq!(path_from_file_url("file:///tmp/%00"), None);
        assert_eq!(path_from_file_url("file:///tmp/%FF").unwrap().as_os_str().as_bytes(), b"/tmp/\xff");
    }
}
