//! Where the app keeps its files.
//!
//! ```text
//! ~/Library/Application Support/ScratchRunner/
//!     settings.toml       preferences
//!     Languages/          user language bundles
//!     Themes/             user themes
//!     KeyBindings/        user key-binding sets
//!     Debuggers/          user debugger bundles
//!     Docs/               documentation source definitions
//!     Unsaved/            untitled documents, as real files
//!     session.json        open windows and tabs
//!     window-state.json   window frames, split positions, pane visibility
//!     recent.json         recent documents
//!     logs/
//! ~/Library/Caches/ScratchRunner/
//! ```

use std::io;
use std::path::{Path, PathBuf};

/// Placeholder CodeRunner uses in directory preferences for
/// `~/Library/Application Support`.
pub const APPLICATION_SUPPORT_PLACEHOLDER: &str = "$(APPLICATION_SUPPORT)";

/// The app's storage roots and the paths under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
    caches: PathBuf,
}

impl Paths {
    /// The production layout, from `$HOME`: `~/Library/Application
    /// Support/ScratchRunner` and `~/Library/Caches/ScratchRunner`. `None`
    /// when `HOME` is not set.
    pub fn system() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(Self::new(
            home.join("Library")
                .join("Application Support")
                .join("ScratchRunner"),
            home.join("Library").join("Caches").join("ScratchRunner"),
        ))
    }

    /// The default storage root, `~/Library/Application Support/ScratchRunner`.
    pub fn default_root() -> Option<PathBuf> {
        Self::system().map(|paths| paths.root)
    }

    /// Storage in `root` and caches in `caches`. Nothing is created until
    /// [`Paths::ensure_dirs`].
    pub fn new(root: impl Into<PathBuf>, caches: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            caches: caches.into(),
        }
    }

    /// A self-contained layout for tests: storage in `<base>/Application
    /// Support/ScratchRunner` and caches in `<base>/Caches/ScratchRunner`,
    /// mirroring `~/Library`.
    pub fn in_dir(base: impl AsRef<Path>) -> Self {
        let base = base.as_ref();
        Self::new(
            base.join("Application Support").join("ScratchRunner"),
            base.join("Caches").join("ScratchRunner"),
        )
    }

    /// The storage root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The cache directory (`~/Library/Caches/ScratchRunner`).
    pub fn caches(&self) -> PathBuf {
        self.caches.clone()
    }

    /// `settings.toml`: the preferences.
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.toml")
    }

    /// `Languages/`: user language bundles.
    pub fn languages(&self) -> PathBuf {
        self.root.join("Languages")
    }

    /// `Themes/`: user themes.
    pub fn themes(&self) -> PathBuf {
        self.root.join("Themes")
    }

    /// `KeyBindings/`: user key-binding sets.
    pub fn key_bindings(&self) -> PathBuf {
        self.root.join("KeyBindings")
    }

    /// `Debuggers/`: user debugger bundles.
    pub fn debuggers(&self) -> PathBuf {
        self.root.join("Debuggers")
    }

    /// `Docs/`: documentation source definitions.
    pub fn docs(&self) -> PathBuf {
        self.root.join("Docs")
    }

    /// `Unsaved/`: untitled documents, stored as real files.
    pub fn unsaved(&self) -> PathBuf {
        self.root.join("Unsaved")
    }

    /// `session.json`: the open windows and tabs, restored at launch.
    pub fn session(&self) -> PathBuf {
        self.root.join("session.json")
    }

    /// `window-state.json`: window frames, split positions and pane visibility.
    pub fn window_state(&self) -> PathBuf {
        self.root.join("window-state.json")
    }

    /// `recent.json`: the recent documents.
    pub fn recent(&self) -> PathBuf {
        self.root.join("recent.json")
    }

    /// `logs/`: diagnostic logs.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Every directory the app writes to, in creation order.
    pub fn dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.languages(),
            self.themes(),
            self.key_bindings(),
            self.debuggers(),
            self.docs(),
            self.unsaved(),
            self.logs(),
            self.caches(),
        ]
    }

    /// Creates the directories the app writes to. Safe to call repeatedly.
    ///
    /// # Errors
    ///
    /// The first I/O error from creating a directory.
    pub fn ensure_dirs(&self) -> io::Result<()> {
        for dir in self.dirs() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// `~/Library/Application Support` as this layout sees it: the parent of
    /// the storage root (the root itself when it has no parent).
    pub fn application_support(&self) -> PathBuf {
        self.root
            .parent()
            .map_or_else(|| self.root.clone(), Path::to_path_buf)
    }

    /// Expands a directory preference such as the autosave directory:
    /// `$(APPLICATION_SUPPORT)` becomes [`Paths::application_support`] and a
    /// leading `~` becomes `$HOME`. Anything else is returned unchanged.
    pub fn expand_dir_template(&self, template: &str) -> PathBuf {
        if let Some(rest) = template.strip_prefix(APPLICATION_SUPPORT_PLACEHOLDER) {
            let rest = rest.trim_start_matches('/');
            let base = self.application_support();
            return if rest.is_empty() { base } else { base.join(rest) };
        }
        expand_tilde(template)
    }

    /// The directory untitled documents live in, from the
    /// `advanced.autosave_documents_directory` preference. An empty value
    /// falls back to [`Paths::unsaved`].
    pub fn autosave_directory(&self, template: &str) -> PathBuf {
        if template.trim().is_empty() {
            self.unsaved()
        } else {
            self.expand_dir_template(template)
        }
    }
}

/// Expands a leading `~` or `~/` to `$HOME`. Other text is returned as a path
/// unchanged, as is `~` when `HOME` is not set.
pub fn expand_tilde(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME");
    match (text, home) {
        ("~", Some(home)) => PathBuf::from(home),
        (text, Some(home)) if text.starts_with("~/") => PathBuf::from(home).join(&text[2..]),
        (text, _) => PathBuf::from(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_the_storage_tree() {
        let paths = Paths::new("/tmp/sr", "/tmp/cache");
        assert_eq!(paths.settings(), Path::new("/tmp/sr/settings.toml"));
        assert_eq!(paths.languages(), Path::new("/tmp/sr/Languages"));
        assert_eq!(paths.themes(), Path::new("/tmp/sr/Themes"));
        assert_eq!(paths.key_bindings(), Path::new("/tmp/sr/KeyBindings"));
        assert_eq!(paths.debuggers(), Path::new("/tmp/sr/Debuggers"));
        assert_eq!(paths.docs(), Path::new("/tmp/sr/Docs"));
        assert_eq!(paths.unsaved(), Path::new("/tmp/sr/Unsaved"));
        assert_eq!(paths.session(), Path::new("/tmp/sr/session.json"));
        assert_eq!(paths.window_state(), Path::new("/tmp/sr/window-state.json"));
        assert_eq!(paths.recent(), Path::new("/tmp/sr/recent.json"));
        assert_eq!(paths.logs(), Path::new("/tmp/sr/logs"));
        assert_eq!(paths.caches(), Path::new("/tmp/cache"));
    }

    #[test]
    fn system_layout_matches_library() {
        if std::env::var_os("HOME").is_none() {
            eprintln!("skipped: HOME is not set");
            return;
        }
        let paths = Paths::system().unwrap();
        assert!(paths.root().ends_with("Library/Application Support/ScratchRunner"));
        assert!(paths.caches().ends_with("Library/Caches/ScratchRunner"));
        assert_eq!(Paths::default_root().as_deref(), Some(paths.root()));
    }

    #[test]
    fn creates_the_directories_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        paths.ensure_dirs().unwrap();
        paths.ensure_dirs().unwrap();
        for path in paths.dirs() {
            assert!(path.is_dir(), "{} missing", path.display());
        }
        assert!(paths.caches().ends_with("Caches/ScratchRunner"));
    }

    #[test]
    fn expands_the_autosave_directory_template() {
        let paths = Paths::new("/x/Application Support/ScratchRunner", "/x/Caches/ScratchRunner");
        assert_eq!(
            paths.autosave_directory("$(APPLICATION_SUPPORT)/ScratchRunner/Unsaved"),
            Path::new("/x/Application Support/ScratchRunner/Unsaved")
        );
        assert_eq!(
            paths.autosave_directory(""),
            Path::new("/x/Application Support/ScratchRunner/Unsaved")
        );
        assert_eq!(paths.autosave_directory("/abs/dir"), Path::new("/abs/dir"));
        assert_eq!(
            paths.expand_dir_template("$(APPLICATION_SUPPORT)"),
            Path::new("/x/Application Support")
        );
    }

    #[test]
    fn expands_tilde_with_home() {
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expand_tilde("~"), PathBuf::from(&home));
            assert_eq!(expand_tilde("~/a/b"), PathBuf::from(home).join("a/b"));
        }
        assert_eq!(expand_tilde("/abs"), Path::new("/abs"));
        assert_eq!(expand_tilde("rel~"), Path::new("rel~"));
    }
}
