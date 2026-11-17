//! A run request: a language, a document snapshot, the settings.

use std::path::{Path, PathBuf};

use scratch_language::LanguageDef;

use crate::{RunOptions, RunSettings, ShellSettings};

/// What is run: the document as the app sees it when the user presses Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snapshot {
    /// A saved file, run in place (the app saves the document first). The
    /// program's working directory is the file's directory, so sibling files are
    /// visible.
    SavedFile {
        /// The file.
        path: PathBuf,
    },
    /// Text that is not (or not yet) a file: it is written to the run's
    /// temporary directory with the language's extension, then run there.
    Text {
        /// The code.
        text: String,
        /// The document's name, if it has one (`Untitled 2`, `notes.py`). Its
        /// extension is kept when it is one of the language's; Java is named
        /// after its main class and Swift is `main.swift`.
        file_name: Option<String>,
        /// A directory the document belongs to (a locked document, an unsaved
        /// copy of a file in a project folder). Used as the working directory
        /// under [`crate::WorkingDirectory::FileDirectory`] so relative paths
        /// keep working.
        directory: Option<PathBuf>,
    },
}

impl Snapshot {
    /// A saved file.
    pub fn saved(path: impl Into<PathBuf>) -> Self {
        Self::SavedFile { path: path.into() }
    }

    /// Unsaved text without a name.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            file_name: None,
            directory: None,
        }
    }

    /// Unsaved text with a document name.
    pub fn named_text(text: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            file_name: Some(file_name.into()),
            directory: None,
        }
    }
}

/// What the run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunMode {
    /// Compile (if the language compiles), then run (Run, ⌘R).
    #[default]
    Run,
    /// Compile only and report [`crate::RunEvent::Compiled`] (⌘B for languages
    /// with a compile phase). Stops before the program.
    CompileOnly,
    /// Run and Debug: compile with `CR_DEBUGGING=1` / debug flags, then stop
    /// with [`crate::RunEvent::Compiled`] so the debugger crate can launch the
    /// program under its debugger (use [`crate::process::Process`] for the
    /// pty). The engine itself never starts the program in this mode.
    Debug,
    /// Run Without Debugging (⌃⌘R): the same as `Run`; the app uses it to
    /// ignore breakpoints.
    RunWithoutDebugging,
}

/// What to do when the owner already has a run in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnBusy {
    /// Refuse to start ([`crate::StartError::AlreadyRunning`]).
    #[default]
    Reject,
    /// Stop the old run (it ends as `Cancelled`; its late events carry the old
    /// [`crate::RunId`]) and start the new one at once.
    StopPrevious,
}

/// Everything one run needs. Build it with [`RunRequest::new`] and the `with_*`
/// methods; every field is public too.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// The language (cloned from the registry: the run is not affected by later
    /// edits of the definition).
    pub language: LanguageDef,
    /// The document.
    pub snapshot: Snapshot,
    /// What to do.
    pub mode: RunMode,
    /// Per-file Run Settings.
    pub settings: RunSettings,
    /// Global Shell preferences.
    pub shell: ShellSettings,
    /// Engine options.
    pub options: RunOptions,
    /// The document key for [`crate::Runner`]'s one-run-per-document rule
    /// (`None` = not tracked).
    pub owner: Option<String>,
    /// What to do when the owner is busy.
    pub on_busy: OnBusy,
    /// The folder opened in the window ([`crate::WorkingDirectory::ProjectFolder`]).
    pub project_folder: Option<PathBuf>,
    /// The default working directory ([`crate::WorkingDirectory::Default`]).
    pub default_directory: Option<PathBuf>,
    /// The directory of the language bundle, exported as `CR_LANGUAGE_DIR`
    /// (`CR_SCRIPTS_DIR` is `<dir>/Scripts`).
    pub language_dir: Option<PathBuf>,
    /// Where unsaved documents are autosaved (`CR_UNSAVED_DIR`).
    pub unsaved_dir: Option<PathBuf>,
    /// Environment variables from the app, applied last and winning over
    /// everything (`CR_DEBUGGER_DIR`, `CR_DEBUGGER_VARIABLES_SORT_MODE`, ...).
    pub extra_env: std::collections::BTreeMap<String, String>,
    /// The app version, exported as `CR_VERSION`.
    pub app_version: Option<String>,
}

impl RunRequest {
    /// A request to run `snapshot` as `language` with default settings.
    pub fn new(language: &LanguageDef, snapshot: Snapshot) -> Self {
        Self {
            language: language.clone(),
            snapshot,
            mode: RunMode::Run,
            settings: RunSettings::default(),
            shell: ShellSettings::default(),
            options: RunOptions::default(),
            owner: None,
            on_busy: OnBusy::Reject,
            project_folder: None,
            default_directory: None,
            language_dir: None,
            unsaved_dir: None,
            extra_env: Default::default(),
            app_version: None,
        }
    }

    /// Sets the mode.
    pub fn with_mode(mut self, mode: RunMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sets the per-file Run Settings.
    pub fn with_settings(mut self, settings: RunSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Sets the Shell preferences.
    pub fn with_shell(mut self, shell: ShellSettings) -> Self {
        self.shell = shell;
        self
    }

    /// Sets the engine options.
    pub fn with_options(mut self, options: RunOptions) -> Self {
        self.options = options;
        self
    }

    /// Sets the owner key (the document) and what to do when it is busy.
    pub fn with_owner(mut self, owner: impl Into<String>, on_busy: OnBusy) -> Self {
        self.owner = Some(owner.into());
        self.on_busy = on_busy;
        self
    }

    /// Sets the project folder.
    pub fn with_project_folder(mut self, folder: impl Into<PathBuf>) -> Self {
        self.project_folder = Some(folder.into());
        self
    }

    /// The file the snapshot refers to, when it is a saved file.
    pub fn saved_path(&self) -> Option<&Path> {
        match &self.snapshot {
            Snapshot::SavedFile { path } => Some(path),
            Snapshot::Text { .. } => None,
        }
    }
}
