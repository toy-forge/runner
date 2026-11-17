//! Root-folder history (the back/forward buttons) and the path popup chain.

use std::path::{Path, PathBuf};

/// Back/forward stacks of navigator roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootHistory {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    current: PathBuf,
}

impl RootHistory {
    /// History starting at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            current: root.into(),
        }
    }

    /// The current root.
    pub fn current(&self) -> &Path {
        &self.current
    }

    /// Navigates to `to` (clears forward). No-op when it is the current root.
    pub fn go(&mut self, to: impl Into<PathBuf>) -> bool {
        let to = to.into();
        if to == self.current {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.current, to));
        self.forward.clear();
        true
    }

    /// Whether Back is possible.
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether Forward is possible.
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Goes back; returns the new root.
    pub fn back(&mut self) -> Option<&Path> {
        let prev = self.back.pop()?;
        self.forward
            .push(std::mem::replace(&mut self.current, prev));
        Some(&self.current)
    }

    /// Goes forward; returns the new root.
    pub fn forward(&mut self) -> Option<&Path> {
        let next = self.forward.pop()?;
        self.back.push(std::mem::replace(&mut self.current, next));
        Some(&self.current)
    }

    /// Paths for the long-press back menu, nearest first.
    pub fn back_list(&self) -> impl Iterator<Item = &Path> {
        self.back.iter().rev().map(PathBuf::as_path)
    }

    /// Paths for the long-press forward menu, nearest first.
    pub fn forward_list(&self) -> impl Iterator<Item = &Path> {
        self.forward.iter().rev().map(PathBuf::as_path)
    }

    /// `Cmd+Up`: re-root at the parent folder (pushes history).
    pub fn go_parent(&mut self) -> bool {
        match self.current.parent().map(Path::to_path_buf) {
            Some(p) if !p.as_os_str().is_empty() => self.go(p),
            _ => false,
        }
    }
}

/// One item of the path popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChoice {
    /// What the menu shows.
    pub label: String,
    /// Where choosing it goes.
    pub path: PathBuf,
}

/// The label of a folder in the popup (iCloud Drive, the volume name for `/`).
pub fn display_name(path: &Path) -> String {
    if path == Path::new("/") {
        return "Macintosh HD".to_owned();
    }
    match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(n) if n == "com~apple~CloudDocs" => "iCloud Drive".to_owned(),
        Some(n) => n,
        None => path.to_string_lossy().into_owned(),
    }
}

/// The ancestor chain, the folder itself first, `/` last (Finder's path popup).
pub fn ancestor_chain(path: &Path) -> Vec<PathChoice> {
    path.ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| PathChoice {
            label: display_name(p),
            path: p.to_path_buf(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_forward_and_branching() {
        let mut h = RootHistory::new("/a");
        assert!(h.go("/a/b"));
        assert!(h.go("/a/b/c"));
        assert_eq!(h.back(), Some(Path::new("/a/b")));
        assert_eq!(h.back(), Some(Path::new("/a")));
        assert!(!h.can_back());
        assert_eq!(h.forward(), Some(Path::new("/a/b")));
        assert!(h.go("/x"));
        assert!(!h.can_forward(), "a new navigation clears forward");
        assert_eq!(
            h.back_list().collect::<Vec<_>>(),
            [Path::new("/a/b"), Path::new("/a")]
        );
    }

    #[test]
    fn chain_ends_at_the_volume() {
        let chain = ancestor_chain(Path::new("/Users/me/Repos"));
        let labels: Vec<_> = chain.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Repos", "me", "Users", "Macintosh HD"]);
        assert_eq!(
            display_name(Path::new("/x/com~apple~CloudDocs")),
            "iCloud Drive"
        );
    }

    #[test]
    fn parent_stops_at_root() {
        let mut h = RootHistory::new("/Users");
        assert!(h.go_parent());
        assert_eq!(h.current(), Path::new("/"));
        assert!(!h.go_parent());
    }
}
