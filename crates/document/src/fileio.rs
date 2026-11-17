//! Reading and writing text files.
//!
//! * [`load_text`] reads a file: large-file guard, encoding detection
//!   ([`crate::detect_encoding`], including the `com.apple.TextEncoding`
//!   attribute), BOM, line-ending detection and normalisation to `\n`, a
//!   [`FileStamp`] for later change detection and the file's access state.
//! * [`save_text`] writes text back: restores the line endings, encodes
//!   (refusing to lose characters), writes atomically (temporary file in the
//!   same directory, fsync, rename) **preserving permissions and extended
//!   attributes**, writes through symbolic links, and reports locked files.
//! * [`file_access`] tells writable, read-only and locked (`chflags uchg`)
//!   files apart; [`copy_for_run`] implements the locked-file fallback of
//!   running from a temporary copy.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::encoding::{
    DecodeError, EncodingHints, EncodingSource, TextEncoding, Unrepresentable, decode,
    decode_lossy, detect_encoding, encode,
};
use crate::line_ending::{self, LineEnding, LineEndingInfo};

/// Files larger than this are refused unless the caller passes
/// [`ReadOptions::allow_large`] (`Open Anyway`).
pub const DEFAULT_LARGE_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// The alert text CodeRunner shows for a locked document that is run.
pub const LOCKED_RUN_MESSAGE: &str = "The document cannot be saved because it is locked. The document will be copied to a temporary folder for running.";

// ---------------------------------------------------------------------------
// Stamps and access

/// A cheap fingerprint of a file's state: modification time, size and a hash
/// of the content. Two equal stamps mean the same bytes (for all practical
/// purposes); see [`check_external`](crate::check_external) for the change checks built on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileStamp {
    /// The modification time, when the file system provides one.
    pub modified: Option<SystemTime>,
    /// The size in bytes.
    pub size: u64,
    /// FNV-1a 64 hash of the content.
    pub content_hash: u64,
}

impl FileStamp {
    /// The stamp of a file whose bytes are `bytes` and whose metadata is `meta`.
    pub fn new(meta: &fs::Metadata, bytes: &[u8]) -> Self {
        Self {
            modified: meta.modified().ok(),
            size: bytes.len() as u64,
            content_hash: hash_bytes(bytes),
        }
    }

    /// Reads the file at `path` and stamps it.
    ///
    /// # Errors
    ///
    /// Any I/O error of reading the file.
    pub fn of_path(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        let meta = fs::metadata(path)?;
        Ok(Self::new(&meta, &bytes))
    }
}

/// FNV-1a 64-bit hash.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Whether a file can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAccess {
    /// Writable by the current user.
    Writable,
    /// Not writable (permissions).
    ReadOnly,
    /// Locked with the Finder's *Locked* checkbox (`chflags uchg`): even the
    /// owner cannot write or delete it until it is unlocked.
    Locked,
    /// There is no such file.
    Missing,
}

impl FileAccess {
    /// The document can be saved in place.
    pub fn can_save(self) -> bool {
        matches!(self, Self::Writable | Self::Missing)
    }
}

/// The access state of `path` (following symbolic links).
pub fn file_access(path: &Path) -> FileAccess {
    let Ok(meta) = fs::metadata(path) else {
        return FileAccess::Missing;
    };
    if is_immutable(&meta) {
        return FileAccess::Locked;
    }
    if can_write(path) {
        FileAccess::Writable
    } else {
        FileAccess::ReadOnly
    }
}

#[cfg(target_os = "macos")]
fn is_immutable(meta: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_IMMUTABLE: u32 = 0x0000_0002;
    const SF_IMMUTABLE: u32 = 0x0002_0000;
    meta.st_flags() & (UF_IMMUTABLE | SF_IMMUTABLE) != 0
}

#[cfg(not(target_os = "macos"))]
fn is_immutable(_meta: &fs::Metadata) -> bool {
    false
}

fn can_write(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// A path with its symbolic link resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Where reads and writes go: the link's target (or the path itself).
    pub target: PathBuf,
    /// The path is a symbolic link.
    pub is_symlink: bool,
}

/// Resolves a symbolic link, also when it dangles (a save through it then
/// creates the target).
pub fn resolve_symlink(path: &Path) -> ResolvedPath {
    let is_symlink = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_symlink {
        return ResolvedPath {
            target: path.to_path_buf(),
            is_symlink: false,
        };
    }
    if let Ok(target) = fs::canonicalize(path) {
        return ResolvedPath {
            target,
            is_symlink: true,
        };
    }
    let target = fs::read_link(path)
        .map(|link| {
            if link.is_absolute() {
                link
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(link)
            }
        })
        .unwrap_or_else(|_| path.to_path_buf());
    ResolvedPath {
        target,
        is_symlink: true,
    }
}

// ---------------------------------------------------------------------------
// Loading

/// What can go wrong reading a document.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// The file is larger than the limit; offer `Open Anyway`.
    #[error("{path} is {size} bytes, over the {limit} byte limit")]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// Its size.
        size: u64,
        /// The limit that applied.
        limit: u64,
    },
    /// The path is a directory.
    #[error("{0} is a folder")]
    IsDirectory(PathBuf),
    /// The bytes are not valid in the requested encoding.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// Options of [`load_text`].
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    /// The default encoding preference.
    pub default_encoding: TextEncoding,
    /// Read with exactly this encoding (`Reinterpret`); an error when the
    /// bytes do not fit.
    pub forced_encoding: Option<TextEncoding>,
    /// The size above which files are refused; `None` for no limit.
    pub large_file_limit: Option<u64>,
    /// Ignore the limit (`Open Anyway`).
    pub allow_large: bool,
    /// Consult the `com.apple.TextEncoding` attribute.
    pub use_xattr_hint: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            default_encoding: TextEncoding::Utf8,
            forced_encoding: None,
            large_file_limit: Some(DEFAULT_LARGE_FILE_LIMIT),
            allow_large: false,
            use_xattr_hint: true,
        }
    }
}

impl ReadOptions {
    /// Options from the preferences (`general.documents.file_encoding`).
    pub fn from_preferences(prefs: &scratch_config::Preferences) -> Self {
        Self::with_default_label(&prefs.general.documents.file_encoding)
    }

    /// Options with `default_encoding` from the preferences label
    /// (`general.documents.file_encoding`); unknown labels mean UTF-8.
    pub fn with_default_label(label: &str) -> Self {
        Self {
            default_encoding: TextEncoding::from_label(label).unwrap_or_default(),
            ..Self::default()
        }
    }
}

/// A text file read from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedFile {
    /// The text with every line ending converted to `\n`.
    pub text: String,
    /// The encoding the bytes were read with.
    pub encoding: TextEncoding,
    /// The file starts with a byte order mark (kept when saving).
    pub bom: bool,
    /// What decided the encoding; `None` when it was forced.
    pub encoding_source: Option<EncodingSource>,
    /// The line endings the file had (restore them when saving).
    pub line_ending: LineEndingInfo,
    /// The fingerprint of the bytes read.
    pub stamp: FileStamp,
    /// Whether the file can be written.
    pub access: FileAccess,
    /// The path is a symbolic link.
    pub is_symlink: bool,
    /// Where the bytes came from (the link's target for a symlink).
    pub resolved: PathBuf,
    /// The text contains NUL characters although the encoding is not UTF-16/32:
    /// probably not a text file.
    pub suspected_binary: bool,
    /// The bytes were invalid in the forced encoding and some were replaced
    /// (never set by [`load_text`], which fails instead; see
    /// [`decode_bytes`]).
    pub lossy: bool,
}

impl LoadedFile {
    /// The line ending to write back: the dominant one.
    pub fn line_ending_style(&self) -> LineEnding {
        self.line_ending.dominant
    }
}

/// The bytes of a file decoded to document text.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedBytes {
    /// Text with `\n` line endings.
    pub text: String,
    /// The encoding used.
    pub encoding: TextEncoding,
    /// The byte order mark was present.
    pub bom: bool,
    /// What decided the encoding.
    pub source: Option<EncodingSource>,
    /// The original line endings.
    pub line_ending: LineEndingInfo,
    /// Invalid bytes were replaced (only with `lossy`).
    pub lossy: bool,
    /// Looks binary.
    pub suspected_binary: bool,
}

/// Decodes the bytes of a file the way [`load_text`] does, without touching
/// the file system. `xattr_hint` is the `com.apple.TextEncoding` value.
///
/// # Errors
///
/// [`DecodeError`] when `options.forced_encoding` is set and does not fit.
pub fn decode_bytes(
    bytes: &[u8],
    options: &ReadOptions,
    xattr_hint: Option<TextEncoding>,
) -> Result<DecodedBytes, DecodeError> {
    let (encoding, bom, source) = match options.forced_encoding {
        Some(encoding) => (encoding, false, None),
        None => {
            let detected = detect_encoding(
                bytes,
                &EncodingHints {
                    xattr: xattr_hint,
                    default: options.default_encoding,
                },
            );
            (detected.encoding, detected.bom, Some(detected.source))
        }
    };
    let decoded = if options.forced_encoding.is_some() {
        decode(bytes, encoding)?
    } else {
        // Detection only picks encodings that decode, except for the final
        // fallback which cannot fail either; stay lossless-safe regardless.
        match decode(bytes, encoding) {
            Ok(decoded) => decoded,
            Err(_) => decode_lossy(bytes, encoding),
        }
    };
    let info = line_ending::detect(&decoded.text);
    let text = line_ending::normalize(&decoded.text).into_owned();
    let suspected_binary = !matches!(
        encoding,
        TextEncoding::Utf16
            | TextEncoding::Utf16Be
            | TextEncoding::Utf16Le
            | TextEncoding::Utf32
            | TextEncoding::Utf32Be
            | TextEncoding::Utf32Le
    ) && text.contains('\0');
    Ok(DecodedBytes {
        text,
        encoding,
        bom: decoded.bom || bom,
        source,
        line_ending: info,
        lossy: decoded.had_errors,
        suspected_binary,
    })
}

/// Reads the text file at `path`.
///
/// # Errors
///
/// [`ReadError`]: I/O failure, a folder, a file over the size limit
/// (`TooLarge`), or bytes invalid in a forced encoding.
pub fn load_text(path: &Path, options: &ReadOptions) -> Result<LoadedFile, ReadError> {
    let io_error = |source: io::Error| ReadError::Io {
        path: path.to_path_buf(),
        source,
    };
    let resolved = resolve_symlink(path);
    let meta = fs::metadata(&resolved.target).map_err(io_error)?;
    if meta.is_dir() {
        return Err(ReadError::IsDirectory(path.to_path_buf()));
    }
    if let (Some(limit), false) = (options.large_file_limit, options.allow_large)
        && meta.len() > limit
    {
        return Err(ReadError::TooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            limit,
        });
    }
    let bytes = fs::read(&resolved.target).map_err(io_error)?;
    let hint = if options.use_xattr_hint {
        crate::meta::read_text_encoding_xattr(&resolved.target)
    } else {
        None
    };
    let decoded = decode_bytes(&bytes, options, hint)?;
    Ok(LoadedFile {
        text: decoded.text,
        encoding: decoded.encoding,
        bom: decoded.bom,
        encoding_source: decoded.source,
        line_ending: decoded.line_ending,
        stamp: FileStamp::new(&meta, &bytes),
        access: file_access(&resolved.target),
        is_symlink: resolved.is_symlink,
        resolved: resolved.target,
        suspected_binary: decoded.suspected_binary,
        lossy: decoded.lossy,
    })
}

// ---------------------------------------------------------------------------
// Saving

/// What can go wrong saving a document.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// The text has characters the encoding cannot hold; nothing was written.
    #[error(transparent)]
    Unrepresentable(#[from] Unrepresentable),
    /// The file is locked (`chflags uchg`); nothing was written.
    #[error("{0} is locked")]
    Locked(PathBuf),
    /// The file is read-only and the caller did not ask to overwrite it.
    #[error("{0} is read-only")]
    ReadOnly(PathBuf),
    /// The target is a folder.
    #[error("{0} is a folder")]
    IsDirectory(PathBuf),
    /// Writing failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
}

/// How to write a document.
#[derive(Debug, Clone, Copy)]
pub struct SaveOptions {
    /// The encoding to write.
    pub encoding: TextEncoding,
    /// Write a byte order mark (UTF-16/32 always do).
    pub bom: bool,
    /// The line ending to write; the text is expected to use `\n`.
    pub line_ending: LineEnding,
    /// Also set the `com.apple.TextEncoding` attribute to the encoding.
    pub write_encoding_xattr: bool,
    /// Write even if the file is read-only (the user confirmed the
    /// overwrite). A locked file is never written.
    pub overwrite_read_only: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            encoding: TextEncoding::Utf8,
            bom: false,
            line_ending: LineEnding::Lf,
            write_encoding_xattr: true,
            overwrite_read_only: false,
        }
    }
}

/// The result of [`save_text`].
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The new stamp of the file.
    pub stamp: FileStamp,
    /// The path written (the link target for a symlink).
    pub written_to: PathBuf,
    /// The number of bytes written.
    pub bytes: usize,
}

/// Saves `text` (`\n` line endings) to `path`.
///
/// The text is encoded first: when `options.encoding` cannot represent it the
/// call fails with [`SaveError::Unrepresentable`] and the file is untouched.
/// Locked files fail with [`SaveError::Locked`]; read-only ones with
/// [`SaveError::ReadOnly`] unless `options.overwrite_read_only` is set.
/// The write is atomic and keeps the permissions and extended attributes of an
/// existing file; symbolic links are written through.
///
/// # Errors
///
/// See [`SaveError`].
pub fn save_text(path: &Path, text: &str, options: &SaveOptions) -> Result<Saved, SaveError> {
    let resolved = resolve_symlink(path);
    let target = resolved.target;
    let io_error = |source: io::Error| SaveError::Io {
        path: target.clone(),
        source,
    };
    if target.is_dir() {
        return Err(SaveError::IsDirectory(target));
    }
    match file_access(&target) {
        FileAccess::Locked => return Err(SaveError::Locked(target)),
        FileAccess::ReadOnly if !options.overwrite_read_only => {
            return Err(SaveError::ReadOnly(target));
        }
        _ => {}
    }
    let with_endings = line_ending::apply(text, options.line_ending);
    let bytes = encode(&with_endings, options.encoding, options.bom)?;
    write_atomic_preserving(&target, &bytes).map_err(io_error)?;
    if options.write_encoding_xattr {
        // Best effort: file systems without extended attributes just skip it.
        if let Err(error) = xattr::set(
            &target,
            crate::meta::TEXT_ENCODING_XATTR,
            options.encoding.xattr_value().as_bytes(),
        ) {
            tracing::debug!(path = %target.display(), %error, "could not set com.apple.TextEncoding");
        }
    }
    let meta = fs::metadata(&target).map_err(io_error)?;
    Ok(Saved {
        stamp: FileStamp::new(&meta, &bytes),
        written_to: target,
        bytes: bytes.len(),
    })
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replaces the file at `path` with `bytes` atomically: a temporary file next
/// to it is written, flushed, given the old file's permissions and extended
/// attributes, and renamed over it. When the directory does not allow
/// creating files but the file itself is writable, the file is rewritten in
/// place instead. Missing parent directories are not created.
///
/// # Errors
///
/// Any I/O error; the temporary file is removed on failure.
pub fn write_atomic_preserving(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let existing = fs::metadata(path).ok();
    let temporary = dir.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let attempt = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(meta) = &existing {
            // Best effort: the owner may not be allowed to chmod (e.g. other owner).
            let _ = fs::set_permissions(&temporary, meta.permissions());
            copy_xattrs(path, &temporary);
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    match attempt {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            if error.kind() == io::ErrorKind::PermissionDenied && existing.is_some() {
                // The directory is not writable but the file may be.
                let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
                file.write_all(bytes)?;
                file.sync_all()
            } else {
                Err(error)
            }
        }
    }
}

/// Copies every extended attribute of `from` to `to`, ignoring failures
/// (some, like `com.apple.provenance`, cannot be set by applications).
pub fn copy_xattrs(from: &Path, to: &Path) {
    let Ok(names) = xattr::list(from) else {
        return;
    };
    for name in names {
        if let Ok(Some(value)) = xattr::get(from, &name)
            && let Err(error) = xattr::set(to, &name, &value)
        {
            tracing::trace!(attribute = ?name, %error, "extended attribute not copied");
        }
    }
}

// ---------------------------------------------------------------------------
// Locked files: run from a temporary copy

/// A temporary copy of a document made to run it when the original cannot be
/// written. The directory is removed when this is dropped.
#[derive(Debug)]
pub struct RunCopy {
    dir: PathBuf,
    path: PathBuf,
}

impl RunCopy {
    /// The copy's path (same file name as the original).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The temporary directory holding the copy; use it as the working
    /// directory of the run.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for RunCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Writes `bytes` to `<tmp_root>/run-<n>/<original file name>`; see
/// [`LOCKED_RUN_MESSAGE`].
///
/// # Errors
///
/// Any I/O error creating the directory or file.
pub fn copy_for_run(original: &Path, bytes: &[u8], tmp_root: &Path) -> io::Result<RunCopy> {
    let name = original
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let dir = tmp_root.join(format!(
        "run-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let result = File::create(&path).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(error);
    }
    Ok(RunCopy { dir, path })
}

/// Where a document runs from.
#[derive(Debug)]
pub enum RunLocation {
    /// The document's own file (saved in place first).
    InPlace(PathBuf),
    /// A temporary copy, because the file is locked or read-only.
    Copy(RunCopy),
}

impl RunLocation {
    /// The file to run.
    pub fn path(&self) -> &Path {
        match self {
            Self::InPlace(path) => path,
            Self::Copy(copy) => copy.path(),
        }
    }

    /// The directory to run in.
    pub fn dir(&self) -> &Path {
        match self {
            Self::InPlace(path) => path.parent().unwrap_or_else(|| Path::new(".")),
            Self::Copy(copy) => copy.dir(),
        }
    }

    /// The run uses a temporary copy (show [`LOCKED_RUN_MESSAGE`]).
    pub fn is_copy(&self) -> bool {
        matches!(self, Self::Copy(_))
    }
}

/// Chooses where to run a document whose current text is `bytes` (already
/// encoded): in place when the file can be saved, else a temporary copy.
///
/// # Errors
///
/// I/O errors creating the temporary copy.
pub fn run_location(original: &Path, bytes: &[u8], tmp_root: &Path) -> io::Result<RunLocation> {
    let target = resolve_symlink(original).target;
    if file_access(&target).can_save() {
        Ok(RunLocation::InPlace(original.to_path_buf()))
    } else {
        copy_for_run(original, bytes, tmp_root).map(RunLocation::Copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn opts() -> SaveOptions {
        SaveOptions {
            write_encoding_xattr: false,
            ..SaveOptions::default()
        }
    }

    #[test]
    fn loads_utf8_with_crlf_and_restores_it_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        assert_eq!(loaded.text, "one\ntwo\n");
        assert_eq!(loaded.line_ending_style(), LineEnding::CrLf);
        assert_eq!(loaded.encoding, TextEncoding::Utf8);
        assert!(!loaded.bom && !loaded.suspected_binary);
        assert_eq!(loaded.access, FileAccess::Writable);

        let options = SaveOptions {
            line_ending: loaded.line_ending_style(),
            ..opts()
        };
        save_text(&path, "one\ntwo\nthree\n", &options).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn bom_and_encoding_survive_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.txt");
        let bytes = encode("héllo\n", TextEncoding::Utf16Le, true).unwrap();
        fs::write(&path, &bytes).unwrap();
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        assert_eq!((loaded.encoding, loaded.bom), (TextEncoding::Utf16Le, true));
        assert_eq!(loaded.text, "héllo\n");
        let options = SaveOptions {
            encoding: loaded.encoding,
            bom: loaded.bom,
            ..opts()
        };
        save_text(&path, &loaded.text, &options).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn save_refuses_to_lose_characters_and_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l.txt");
        fs::write(&path, "original").unwrap();
        let options = SaveOptions {
            encoding: TextEncoding::Latin1,
            ..opts()
        };
        let err = save_text(&path, "smile 😀", &options).unwrap_err();
        assert!(matches!(err, SaveError::Unrepresentable(ref u) if u.character == '😀'));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temporary left"
        );
    }

    #[test]
    fn large_file_guard_and_open_anyway() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        fs::write(&path, vec![b'x'; 2048]).unwrap();
        let strict = ReadOptions {
            large_file_limit: Some(1024),
            ..ReadOptions::default()
        };
        match load_text(&path, &strict) {
            Err(ReadError::TooLarge { size, limit, .. }) => assert_eq!((size, limit), (2048, 1024)),
            other => panic!("expected TooLarge, got {other:?}"),
        }
        let anyway = ReadOptions {
            allow_large: true,
            ..strict
        };
        assert_eq!(load_text(&path, &anyway).unwrap().text.len(), 2048);
    }

    #[test]
    fn reading_a_folder_or_missing_file_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load_text(dir.path(), &ReadOptions::default()),
            Err(ReadError::IsDirectory(_))
        ));
        assert!(matches!(
            load_text(&dir.path().join("nope"), &ReadOptions::default()),
            Err(ReadError::Io { .. })
        ));
    }

    #[test]
    fn forced_encoding_is_strict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, b"caf\xE9").unwrap();
        let forced = ReadOptions {
            forced_encoding: Some(TextEncoding::Utf8),
            ..ReadOptions::default()
        };
        assert!(matches!(
            load_text(&path, &forced),
            Err(ReadError::Decode(_))
        ));
        let latin = ReadOptions {
            forced_encoding: Some(TextEncoding::Latin1),
            ..ReadOptions::default()
        };
        let loaded = load_text(&path, &latin).unwrap();
        assert_eq!(
            (loaded.text.as_str(), loaded.encoding_source),
            ("café", None)
        );
    }

    #[test]
    fn detects_the_encoding_attribute_written_by_a_save() {
        if !xattr::SUPPORTED_PLATFORM {
            eprintln!("skipped: no xattr support");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        let options = SaveOptions {
            encoding: TextEncoding::Latin1,
            ..SaveOptions::default()
        };
        save_text(&path, "café\n", &options).unwrap();
        match xattr::get(&path, "com.apple.TextEncoding") {
            Ok(Some(value)) => assert_eq!(value, b"iso-8859-1;513"),
            _ => {
                eprintln!("skipped: file system without extended attributes");
                return;
            }
        }
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        assert_eq!(loaded.encoding, TextEncoding::Latin1);
        assert_eq!(loaded.encoding_source, Some(EncodingSource::Xattr));
    }

    #[test]
    fn save_preserves_permissions_and_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.sh");
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let has_xattr = xattr::set(&path, "user.scratch-test", b"keep me").is_ok();
        save_text(&path, "#!/bin/sh\necho hi\n", &opts()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        if has_xattr {
            assert_eq!(
                xattr::get(&path, "user.scratch-test").unwrap().as_deref(),
                Some(&b"keep me"[..])
            );
        } else {
            eprintln!("note: xattrs unavailable, attribute check skipped");
        }
    }

    #[test]
    fn saves_through_a_symlink_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&real, "old").unwrap();
        symlink(&real, &link).unwrap();
        let loaded = load_text(&link, &ReadOptions::default()).unwrap();
        assert!(loaded.is_symlink);
        assert_eq!(loaded.text, "old");
        save_text(&link, "new", &opts()).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }

    #[test]
    fn dangling_symlink_save_creates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.txt");
        symlink("target.txt", &link).unwrap();
        save_text(&link, "created", &opts()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("target.txt")).unwrap(),
            "created"
        );
    }

    #[test]
    fn read_only_files_need_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ro.txt");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(file_access(&path), FileAccess::ReadOnly);
        assert!(matches!(
            save_text(&path, "y", &opts()),
            Err(SaveError::ReadOnly(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
        let force = SaveOptions {
            overwrite_read_only: true,
            ..opts()
        };
        save_text(&path, "y", &force).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "y");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[cfg(target_os = "macos")]
    struct Unlock(PathBuf);

    #[cfg(target_os = "macos")]
    impl Drop for Unlock {
        fn drop(&mut self) {
            use std::os::unix::ffi::OsStrExt;
            if let Ok(c) = std::ffi::CString::new(self.0.as_os_str().as_bytes()) {
                // SAFETY: valid NUL-terminated path.
                unsafe { libc::chflags(c.as_ptr(), 0) };
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn lock(path: &Path) -> Option<Unlock> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: valid NUL-terminated path; UF_IMMUTABLE is 2.
        let rc = unsafe { libc::chflags(c.as_ptr(), 2) };
        (rc == 0).then(|| Unlock(path.to_path_buf()))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn locked_files_are_detected_and_run_from_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.py");
        fs::write(&path, "print(1)\n").unwrap();
        let Some(_guard) = lock(&path) else {
            eprintln!("skipped: chflags not permitted here");
            return;
        };
        assert_eq!(file_access(&path), FileAccess::Locked);
        assert!(matches!(
            save_text(&path, "x", &opts()),
            Err(SaveError::Locked(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "print(1)\n");

        let location = run_location(&path, b"print(2)\n", tmp.path()).unwrap();
        assert!(location.is_copy());
        assert_eq!(location.path().file_name(), path.file_name());
        assert_eq!(fs::read(location.path()).unwrap(), b"print(2)\n");
        let copy_dir = location.dir().to_path_buf();
        drop(location);
        assert!(!copy_dir.exists(), "temporary copy removed on drop");
    }

    #[test]
    fn writable_files_run_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.py");
        fs::write(&path, "1").unwrap();
        let location = run_location(&path, b"1", tmp.path()).unwrap();
        assert!(!location.is_copy());
        assert_eq!(location.dir(), dir.path());
    }

    #[test]
    fn stamps_differ_when_content_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.txt");
        fs::write(&path, "aaa").unwrap();
        let a = FileStamp::of_path(&path).unwrap();
        fs::write(&path, "bbb").unwrap();
        let b = FileStamp::of_path(&path).unwrap();
        assert_eq!(a.size, b.size);
        assert_ne!(a.content_hash, b.content_hash);
        assert_eq!(hash_bytes(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn suspects_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.bin");
        fs::write(&path, b"abc\0def").unwrap();
        assert!(
            load_text(&path, &ReadOptions::default())
                .unwrap()
                .suspected_binary
        );
    }
}
