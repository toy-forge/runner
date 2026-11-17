//! Directory entries and the sources they come from.

use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A stable identity of an entry: a hash of its absolute path.
///
/// Used as the `virtual_list` key, the selection element and the expanded
/// set element, so none of them has to clone paths.
pub type EntryId = u64;

/// The identity of `path` (deterministic across runs: SipHash with zero keys).
pub fn path_id(path: &Path) -> EntryId {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

/// File or directory (a symlink is reported as what it points to).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A regular file (or anything that is not a directory).
    File,
    /// A directory.
    Dir,
}

/// One child of a directory.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file name (last path component).
    pub name: Arc<str>,
    /// The absolute path.
    pub path: Arc<Path>,
    /// File or directory.
    pub kind: Kind,
    /// Name starts with a dot.
    pub hidden: bool,
    /// The entry itself is a symlink.
    pub symlink: bool,
    /// [`path_id`] of `path`.
    pub id: EntryId,
}

/// A shared entry; rows and caches hold these.
pub type EntryRef = Arc<Entry>;

impl Entry {
    /// An entry for `path` of `kind`; the name comes from the path.
    pub fn new(path: impl Into<Arc<Path>>, kind: Kind) -> EntryRef {
        let path: Arc<Path> = path.into();
        let name: Arc<str> = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
            .into();
        Arc::new(Entry {
            hidden: name.starts_with('.'),
            id: path_id(&path),
            name,
            path,
            kind,
            symlink: false,
        })
    }

    /// Whether this is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// The extension (lowercase), if any.
    pub fn extension(&self) -> Option<String> {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    }
}

/// Where directory listings come from. The tree never touches `std::fs`
/// directly, so a synthetic source can stand in for benchmarks and tests.
pub trait DirSource: Send + Sync + fmt::Debug {
    /// The children of `dir`, in any order.
    fn list(&self, dir: &Path) -> io::Result<Vec<EntryRef>>;
}

/// The real file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsSource;

impl DirSource for FsSource {
    fn list(&self, dir: &Path) -> io::Result<Vec<EntryRef>> {
        let mut out = Vec::new();
        for item in std::fs::read_dir(dir)? {
            let Ok(item) = item else { continue };
            let Ok(file_type) = item.file_type() else {
                continue;
            };
            let path: PathBuf = item.path();
            let (kind, symlink) = if file_type.is_symlink() {
                match std::fs::metadata(&path) {
                    Ok(m) if m.is_dir() => (Kind::Dir, true),
                    _ => (Kind::File, true),
                }
            } else if file_type.is_dir() {
                (Kind::Dir, false)
            } else {
                (Kind::File, false)
            };
            let name: Arc<str> = item.file_name().to_string_lossy().into_owned().into();
            let path: Arc<Path> = path.into();
            out.push(Arc::new(Entry {
                hidden: name.starts_with('.'),
                id: path_id(&path),
                name,
                path,
                kind,
                symlink,
            }));
        }
        Ok(out)
    }
}

/// A deterministic synthetic tree under `/synth` for benchmarks.
///
/// `Nested { dirs, subdirs, files }` has `dirs` top directories each holding
/// `subdirs` directories each holding `files` files; `Flat { files }` is one
/// directory of `files` files.
#[derive(Debug, Clone, Copy)]
pub enum SynthShape {
    /// Three levels: `dirs * subdirs * files` files in `dirs * subdirs` leaf directories.
    Nested {
        /// Top-level directories.
        dirs: u32,
        /// Directories per top directory.
        subdirs: u32,
        /// Files per leaf directory.
        files: u32,
    },
    /// One directory of many files.
    Flat {
        /// Number of files.
        files: u32,
    },
}

/// Synthetic [`DirSource`], see [`SynthShape`].
#[derive(Debug, Clone, Copy)]
pub struct SynthSource {
    /// The shape of the tree.
    pub shape: SynthShape,
    /// Artificial per-listing latency, to exercise lazy loading.
    pub latency: std::time::Duration,
}

/// The synthetic root path.
pub const SYNTH_ROOT: &str = "/synth";

const EXTS: [&str; 12] = [
    "rs", "py", "js", "ts", "c", "cpp", "swift", "go", "md", "json", "txt", "html",
];

impl SynthSource {
    /// 20 x 25 x 100 = 50 000 files in 520 directories.
    pub fn nested_50k() -> Self {
        Self {
            shape: SynthShape::Nested {
                dirs: 20,
                subdirs: 25,
                files: 100,
            },
            latency: std::time::Duration::ZERO,
        }
    }

    /// One directory of 50 000 files.
    pub fn flat_50k() -> Self {
        Self {
            shape: SynthShape::Flat { files: 50_000 },
            latency: std::time::Duration::ZERO,
        }
    }

    fn file(dir: &Path, i: u32) -> EntryRef {
        let ext = EXTS[(i as usize * 7 + 3) % EXTS.len()];
        Entry::new(dir.join(format!("file_{i:05}.{ext}")), Kind::File)
    }
}

impl DirSource for SynthSource {
    fn list(&self, dir: &Path) -> io::Result<Vec<EntryRef>> {
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }
        let depth = dir
            .strip_prefix(SYNTH_ROOT)
            .map_err(|_| io::Error::from(io::ErrorKind::NotFound))?
            .components()
            .count();
        let mut out = Vec::new();
        match (self.shape, depth) {
            (SynthShape::Flat { files }, 0) => {
                out.extend((0..files).map(|i| Self::file(dir, i)));
            }
            (SynthShape::Nested { dirs, .. }, 0) => {
                out.extend(
                    (0..dirs).map(|i| Entry::new(dir.join(format!("module_{i:02}")), Kind::Dir)),
                );
            }
            (SynthShape::Nested { subdirs, .. }, 1) => {
                out.extend(
                    (0..subdirs).map(|i| Entry::new(dir.join(format!("pkg_{i:02}")), Kind::Dir)),
                );
            }
            (SynthShape::Nested { files, .. }, 2) => {
                out.extend((0..files).map(|i| Self::file(dir, i)));
            }
            _ => return Err(io::Error::from(io::ErrorKind::NotFound)),
        }
        Ok(out)
    }
}
