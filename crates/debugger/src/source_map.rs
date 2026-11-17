//! Mapping between paths reported by the debugger and local source paths.
//!
//! A prepared binary may have been compiled in a temporary directory, from a
//! different checkout, or through a symlink.  The runner gives the session a
//! program path and a [`SourceMap`]; every parsed frame and stop location is
//! passed through it before it reaches the UI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A bidirectional-ish map from debugger-reported paths to local files.
///
/// Exact matches win, then canonical paths, then file-name matches when the
/// match is unambiguous.  Unmapped paths are returned unchanged.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    forward: BTreeMap<PathBuf, PathBuf>,
}

impl SourceMap {
    /// An empty map (identity).
    pub fn new() -> Self {
        Self::default()
    }

    /// Map `reported` to `local`.
    pub fn add(&mut self, reported: impl Into<PathBuf>, local: impl Into<PathBuf>) {
        self.forward.insert(reported.into(), local.into());
    }

    /// Builder form of [`Self::add`].
    pub fn with(mut self, reported: impl Into<PathBuf>, local: impl Into<PathBuf>) -> Self {
        self.add(reported, local);
        self
    }

    /// Number of explicit mappings.
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// True when there are no mappings.
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Resolve a debugger path.
    pub fn resolve(&self, reported: &Path) -> PathBuf {
        if let Some(local) = self.forward.get(reported) {
            return local.clone();
        }
        if let Ok(canonical) = reported.canonicalize() {
            if let Some(local) = self.forward.get(&canonical) {
                return local.clone();
            }
            for (key, value) in &self.forward {
                if key.canonicalize().is_ok_and(|k| k == canonical) {
                    return value.clone();
                }
            }
        }
        if let Some(name) = reported.file_name() {
            let mut matches = self
                .forward
                .iter()
                .filter(|(key, _)| key.file_name() == Some(name))
                .map(|(_, value)| value.clone());
            if let (Some(first), None) = (matches.next(), matches.next()) {
                return first;
            }
        }
        reported.to_path_buf()
    }

    /// Resolve a path that may be relative to the program's working directory.
    pub fn resolve_in(&self, reported: &Path, cwd: &Path) -> PathBuf {
        let absolute = if reported.is_absolute() {
            reported.to_path_buf()
        } else {
            cwd.join(reported)
        };
        self.resolve(&absolute)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_mapping_wins() {
        let mut map = SourceMap::new();
        map.add("/tmp/old/a.c", "/work/a.c");
        assert_eq!(map.resolve(Path::new("/tmp/old/a.c")), PathBuf::from("/work/a.c"));
        assert_eq!(map.resolve(Path::new("/other/b.c")), PathBuf::from("/other/b.c"));
    }

    #[test]
    fn unique_file_name_mapping_is_used() {
        let mut map = SourceMap::new();
        map.add("/tmp/old/a.c", "/work/a.c");
        assert_eq!(map.resolve(Path::new("/build/a.c")), PathBuf::from("/work/a.c"));
    }

    #[test]
    fn relative_paths_join_the_working_directory() {
        let mut map = SourceMap::new();
        map.add("/work/run/a.c", "/src/a.c");
        assert_eq!(
            map.resolve_in(Path::new("a.c"), Path::new("/work/run")),
            PathBuf::from("/src/a.c")
        );
    }
}
