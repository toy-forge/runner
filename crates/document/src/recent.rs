//! The recent documents list (`File > Open Recent`).
//!
//! A capped, most-recent-first list of paths with `Clear Menu`, an existence
//! filter (files that were deleted do not show) and a per-app JSON file
//! (`recent.json`). It does not touch the system's own recent documents list.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use scratch_config::write_atomic;
use serde::{Deserialize, Serialize};

/// The default capacity (`NSDocumentController`'s default of ten).
pub const DEFAULT_RECENT_CAP: usize = 10;

/// A recent document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    /// The file or folder.
    pub path: PathBuf,
    /// When it was last opened (ms since the epoch).
    pub opened_ms: u64,
}

/// A menu row: the title and, when another entry has the same file name, the
/// folder to tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentLabel {
    /// The file name.
    pub title: String,
    /// The parent folder, set only for ambiguous names.
    pub detail: Option<String>,
    /// The entry's path.
    pub path: PathBuf,
}

/// See the crate documentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentDocuments {
    cap: usize,
    entries: Vec<RecentEntry>,
}

impl Default for RecentDocuments {
    fn default() -> Self {
        Self::new(DEFAULT_RECENT_CAP)
    }
}

impl RecentDocuments {
    /// An empty list holding at most `cap` entries.
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: Vec::new(),
        }
    }

    /// The capacity.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Changes the capacity, dropping the oldest entries if needed.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
        self.entries.truncate(cap);
    }

    /// Notes that `path` was opened at `now_ms`: it moves to the front (an
    /// existing entry is not duplicated).
    pub fn add(&mut self, path: &Path, now_ms: u64) {
        let path = normalize(path);
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            RecentEntry {
                path,
                opened_ms: now_ms,
            },
        );
        self.entries.truncate(self.cap);
    }

    /// Forgets `path`.
    pub fn remove(&mut self, path: &Path) {
        let path = normalize(path);
        self.entries.retain(|e| e.path != path);
    }

    /// Updates the entry of a file that was renamed or moved.
    pub fn rename(&mut self, from: &Path, to: &Path) {
        let (from, to) = (normalize(from), normalize(to));
        for entry in &mut self.entries {
            if entry.path == from {
                entry.path = to.clone();
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.entries.retain(|e| seen.insert(e.path.clone()));
    }

    /// `Clear Menu`.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// All entries, most recent first, whether or not the files exist.
    pub fn entries(&self) -> &[RecentEntry] {
        &self.entries
    }

    /// The entries whose files exist, most recent first: what the menu shows.
    pub fn existing(&self) -> Vec<&RecentEntry> {
        self.existing_by(|p| p.exists())
    }

    /// Like [`RecentDocuments::existing`] with a custom existence test.
    pub fn existing_by(&self, exists: impl Fn(&Path) -> bool) -> Vec<&RecentEntry> {
        self.entries.iter().filter(|e| exists(&e.path)).collect()
    }

    /// Removes the entries whose files are gone for good. Returns how many.
    pub fn prune_missing(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.path.exists());
        before - self.entries.len()
    }

    /// Menu rows for the existing entries: the file name, plus the parent
    /// folder when two rows would read the same.
    pub fn labels(&self) -> Vec<RecentLabel> {
        let existing = self.existing();
        let name = |e: &RecentEntry| {
            e.path.file_name().map_or_else(
                || e.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        existing
            .iter()
            .map(|e| {
                let title = name(e);
                let ambiguous = existing.iter().filter(|o| name(o) == title).count() > 1;
                let detail = ambiguous
                    .then(|| e.path.parent().map(|p| p.display().to_string()))
                    .flatten();
                RecentLabel {
                    title,
                    detail,
                    path: e.path.clone(),
                }
            })
            .collect()
    }

    /// Reads `path`; a missing or damaged file is an empty list with `cap`.
    pub fn load(path: &Path, cap: usize) -> Self {
        let mut list: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        list.set_cap(cap);
        list
    }

    /// Writes `path` atomically.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(
            path,
            &serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )
    }
}

fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() {
        path.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let mut recent = RecentDocuments::new(3);
        for (i, name) in ["/a", "/b", "/c", "/b", "/d"].iter().enumerate() {
            recent.add(Path::new(name), i as u64);
        }
        let paths: Vec<_> = recent
            .entries()
            .iter()
            .map(|e| e.path.to_str().unwrap())
            .collect();
        assert_eq!(paths, ["/d", "/b", "/c"]);
        assert_eq!(
            recent.entries()[1].opened_ms,
            3,
            "re-opening refreshes the time"
        );
        recent.set_cap(1);
        assert_eq!(recent.entries().len(), 1);
    }

    #[test]
    fn trailing_slashes_do_not_duplicate() {
        let mut recent = RecentDocuments::default();
        recent.add(Path::new("/work/proj/"), 1);
        recent.add(Path::new("/work/proj"), 2);
        assert_eq!(recent.entries().len(), 1);
        recent.remove(Path::new("/work/proj/"));
        assert!(recent.entries().is_empty());
    }

    #[test]
    fn clear_and_remove() {
        let mut recent = RecentDocuments::default();
        recent.add(Path::new("/a"), 1);
        recent.add(Path::new("/b"), 2);
        recent.remove(Path::new("/a"));
        assert_eq!(recent.entries().len(), 1);
        recent.clear();
        assert!(recent.entries().is_empty());
    }

    #[test]
    fn existence_filter_hides_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept.py");
        let gone = dir.path().join("gone.py");
        fs::write(&kept, "x").unwrap();
        fs::write(&gone, "x").unwrap();
        let mut recent = RecentDocuments::default();
        recent.add(&kept, 1);
        recent.add(&gone, 2);
        fs::remove_file(&gone).unwrap();
        assert_eq!(recent.existing().len(), 1);
        assert_eq!(recent.entries().len(), 2, "still remembered until pruned");
        assert_eq!(recent.prune_missing(), 1);
        assert_eq!(recent.entries().len(), 1);
    }

    #[test]
    fn rename_follows_the_file() {
        let mut recent = RecentDocuments::default();
        recent.add(Path::new("/a/x.py"), 1);
        recent.add(Path::new("/a/y.py"), 2);
        recent.rename(Path::new("/a/x.py"), Path::new("/a/y.py"));
        assert_eq!(
            recent.entries().len(),
            1,
            "no duplicate after renaming onto an entry"
        );
        recent.rename(Path::new("/a/y.py"), Path::new("/b/z.py"));
        assert_eq!(recent.entries()[0].path, Path::new("/b/z.py"));
    }

    #[test]
    fn labels_add_the_folder_only_when_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&two).unwrap();
        let a = one.join("main.py");
        let b = two.join("main.py");
        let c = one.join("util.py");
        for f in [&a, &b, &c] {
            fs::write(f, "x").unwrap();
        }
        let mut recent = RecentDocuments::default();
        for (i, f) in [&a, &b, &c].iter().enumerate() {
            recent.add(f, i as u64);
        }
        let labels = recent.labels();
        assert_eq!(labels.len(), 3);
        let by_title: Vec<_> = labels
            .iter()
            .map(|l| (l.title.as_str(), l.detail.is_some()))
            .collect();
        assert_eq!(
            by_title,
            [("util.py", false), ("main.py", true), ("main.py", true)]
        );
    }

    #[test]
    fn persists_and_tolerates_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent.json");
        let mut recent = RecentDocuments::new(5);
        recent.add(Path::new("/a"), 1);
        recent.save(&path).unwrap();
        assert_eq!(RecentDocuments::load(&path, 5), recent);
        assert_eq!(
            RecentDocuments::load(&path, 0).entries().len(),
            0,
            "a smaller cap truncates"
        );
        fs::write(&path, b"garbage").unwrap();
        assert_eq!(RecentDocuments::load(&path, 7), RecentDocuments::new(7));
        assert_eq!(
            RecentDocuments::load(&dir.path().join("none.json"), 7).cap(),
            7
        );
    }
}
