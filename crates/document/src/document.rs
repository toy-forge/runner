//! The document model: everything about a document except its text.
//!
//! The text lives in the editor. A [`Document`] is the metadata around it:
//! identity, file, language, encoding, line endings, dirty flag, per-document
//! run settings, breakpoints and view state. It is serialisable (drafts and
//! the session store it) and has the transitions the app needs: opening a
//! file, editing, saving, reloading from disk, changing the language.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use scratch_language::{DetectInput, LanguageRegistry};
use serde::{Deserialize, Serialize};

use crate::encoding::TextEncoding;
use crate::fileio::{FileAccess, FileStamp, LoadedFile, SaveOptions, Saved};
use crate::id::DocumentId;
use crate::line_ending::LineEnding;
use crate::meta::FileMeta;
use crate::state::{Breakpoints, RunSettings, ViewState};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A document's metadata. See the crate documentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    /// The identity, kept for the document's life.
    pub id: DocumentId,
    /// The file. For an untitled document this is its file in the Unsaved
    /// directory (untitled documents are real files); for a saved document it
    /// is where it was opened from or saved to.
    pub path: Option<PathBuf>,
    /// The document has never been saved or opened from a file of the user:
    /// its [`path`](Document::path) is in the Unsaved directory.
    pub untitled: bool,
    /// The language id (`python`); see [`scratch_language::LanguageRegistry`].
    pub language: String,
    /// The user chose the language, so detection leaves it alone.
    pub language_locked: bool,
    /// A syntax mode (TextMate scope) that overrides the language's grammar.
    pub grammar: Option<String>,
    /// The encoding the file is read and written with.
    pub encoding: TextEncoding,
    /// The file has (and keeps) a byte order mark.
    pub bom: bool,
    /// The line ending written on save (the text in the editor uses `\n`).
    pub line_ending: LineEnding,
    /// The file is read-only on disk.
    pub read_only: bool,
    /// The file is locked (`chflags uchg`); it is run from a temporary copy.
    pub locked: bool,
    /// There are edits that are not saved.
    pub dirty: bool,
    /// A scratch document no one has edited: its text is still what it
    /// was created with (a template's rendering or empty). A language
    /// switch on a pristine scratch document replaces its text with the
    /// new language's template, as CodeRunner does; [`mark_edited`]
    /// clears the flag so edited documents keep their text.
    pub pristine: bool,
    /// When the document was created (ms since the epoch).
    pub created_ms: u64,
    /// When it was last edited or saved (ms since the epoch).
    pub updated_ms: u64,
    /// Incremented by every edit.
    pub revision: u64,
    /// The per-file run settings.
    pub run: RunSettings,
    /// The breakpoints.
    pub breakpoints: Breakpoints,
    /// Selections, scroll position and folds.
    pub view: ViewState,
    /// The state of the file when it was last read or written; the base for
    /// external change detection.
    pub stamp: Option<FileStamp>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            id: DocumentId::new(),
            path: None,
            untitled: true,
            language: scratch_language::PLAIN_TEXT_ID.to_owned(),
            language_locked: false,
            grammar: None,
            encoding: TextEncoding::Utf8,
            bom: false,
            line_ending: LineEnding::Lf,
            read_only: false,
            locked: false,
            dirty: false,
            pristine: false,
            created_ms: 0,
            updated_ms: 0,
            revision: 0,
            run: RunSettings::default(),
            breakpoints: Breakpoints::default(),
            view: ViewState::default(),
            stamp: None,
        }
    }
}

impl Document {
    /// A new untitled document whose file is `path` (in the Unsaved
    /// directory) in `language`.
    pub fn untitled(path: PathBuf, language: &str, now_ms: u64) -> Self {
        Self {
            path: Some(path),
            untitled: true,
            language: language.to_owned(),
            created_ms: now_ms,
            updated_ms: now_ms,
            ..Self::default()
        }
    }

    /// A document for a file that was just read. The language comes from the
    /// file's metadata when it has one (and then stays locked), else from
    /// detection on the path and text. `meta` supplies the per-file settings.
    pub fn from_file(
        path: &Path,
        loaded: &LoadedFile,
        meta: &FileMeta,
        registry: &LanguageRegistry,
        now_ms: u64,
    ) -> Self {
        let mut document = Self {
            path: Some(path.to_path_buf()),
            untitled: false,
            created_ms: now_ms,
            updated_ms: now_ms,
            ..Self::default()
        };
        document.apply_loaded(loaded);
        document.apply_meta(meta, &loaded.text, registry);
        if meta.resolve_language(registry).is_none() {
            document.redetect_language(registry, &loaded.text);
        }
        document
    }

    /// The name shown in the tab and title bar: the file name.
    pub fn title(&self) -> String {
        self.path.as_deref().and_then(Path::file_name).map_or_else(
            || "Untitled".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// The file's directory, if it has a file.
    pub fn directory(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// Takes the state of a file that was read: encoding, BOM, line endings,
    /// access, stamp. The document becomes clean.
    pub fn apply_loaded(&mut self, loaded: &LoadedFile) {
        self.encoding = loaded.encoding;
        self.bom = loaded.bom;
        self.line_ending = loaded.line_ending.dominant;
        self.read_only = loaded.access == FileAccess::ReadOnly;
        self.locked = loaded.access == FileAccess::Locked;
        self.stamp = Some(loaded.stamp);
        self.dirty = false;
    }

    /// Takes the per-file settings stored with the file: language (locking
    /// it), grammar, run settings, breakpoints and selection.
    pub fn apply_meta(&mut self, meta: &FileMeta, text: &str, registry: &LanguageRegistry) {
        if let Some(language) = meta.resolve_language(registry) {
            self.language = language.id.clone();
            self.language_locked = true;
        }
        if let Some(grammar) = &meta.grammar {
            self.grammar = Some(grammar.clone());
        }
        self.run = RunSettings {
            run_command: meta.run_command.clone(),
            arguments: meta.arguments.clone().unwrap_or_default(),
            compiler_flags: meta.compiler_flags.clone(),
            input: meta.input.clone().unwrap_or_default(),
            // File metadata does not store these yet; the recovery journal does.
            ..RunSettings::default()
        };
        if let Some(breakpoints) = &meta.breakpoints {
            self.breakpoints = Breakpoints::from_list(breakpoints.iter().cloned());
        }
        let selections = meta.selections_for(text);
        if !selections.is_empty() {
            self.view.selections = selections;
        }
    }

    /// The metadata to store with the file (see [`crate::MetaStore::write`]).
    /// Language and grammar are stored only when the user chose them, so
    /// detection keeps working for the rest; `registry` supplies the
    /// CodeRunner uuid for compatibility.
    pub fn to_meta(&self, registry: Option<&LanguageRegistry>) -> FileMeta {
        let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        FileMeta {
            language: self.language_locked.then(|| self.language.clone()),
            language_uuid: registry
                .filter(|_| self.language_locked)
                .and_then(|r| r.get(&self.language))
                .and_then(|l| l.uuid.clone()),
            grammar: self.grammar.clone(),
            run_command: self.run.run_command.clone(),
            arguments: some(&self.run.arguments),
            compiler_flags: self.run.compiler_flags.clone(),
            input: some(&self.run.input),
            breakpoints: (!self.breakpoints.is_empty())
                .then(|| self.breakpoints.as_slice().to_vec()),
            diagnostics: None,
            selection: (!self.view.selections.is_empty()).then(|| self.view.selections.clone()),
            selection_ranges: None,
            encoding: Some(self.encoding),
        }
    }

    /// Chooses the language by detection (extension, name, shebang, modeline)
    /// unless the user locked it. Returns whether the language changed.
    pub fn redetect_language(&mut self, registry: &LanguageRegistry, text: &str) -> bool {
        if self.language_locked {
            return false;
        }
        let mut input = DetectInput::new().text(text);
        if let Some(path) = &self.path {
            input = input.path(path);
        }
        let detected = registry.detect(&input).language.id.clone();
        let changed = detected != self.language;
        self.language = detected;
        changed
    }

    /// The user picked a language (`Run > Language`): it is locked and any
    /// grammar override is dropped (`Syntax Mode` sets one again).
    pub fn set_language(&mut self, id: &str) {
        self.language = id.to_owned();
        self.language_locked = true;
        self.grammar = None;
    }

    /// The user picked a syntax mode only (`Text > Syntax Mode`).
    pub fn set_grammar(&mut self, grammar: Option<String>) {
        self.grammar = grammar;
    }

    /// Records an edit.
    pub fn mark_edited(&mut self, now_ms: u64) {
        self.dirty = true;
        self.pristine = false;
        self.revision += 1;
        self.updated_ms = now_ms;
    }

    /// Records a successful save.
    pub fn mark_saved(&mut self, saved: &Saved, now_ms: u64) {
        self.stamp = Some(saved.stamp);
        self.dirty = false;
        self.updated_ms = now_ms;
        self.locked = false;
        self.read_only = false;
    }

    /// The document now lives at `path` (Save As, Move To, Rename): it is no
    /// longer untitled. Returns the previous path of an untitled document, the
    /// file in the Unsaved directory the caller should delete.
    pub fn relocate(&mut self, path: PathBuf) -> Option<PathBuf> {
        let previous = self.untitled.then(|| self.path.take()).flatten();
        self.path = Some(path);
        self.untitled = false;
        previous
    }

    /// The options for [`crate::save_text`] that reproduce this document's
    /// encoding, BOM and line endings.
    pub fn save_options(&self) -> SaveOptions {
        SaveOptions {
            encoding: self.encoding,
            bom: self.bom,
            line_ending: self.line_ending,
            ..SaveOptions::default()
        }
    }

    /// Whether the run settings differ from the language defaults (the
    /// toolbar wrench is highlighted).
    pub fn has_custom_run_settings(&self) -> bool {
        self.run.is_custom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileio::{ReadOptions, load_text, save_text};
    use crate::state::{Breakpoint, Position, Selection};
    use std::fs;

    fn registry() -> LanguageRegistry {
        LanguageRegistry::builtin()
    }

    #[test]
    fn an_edit_clears_pristine() {
        let mut doc = Document { pristine: true, untitled: true, ..Document::default() };
        assert!(doc.pristine);
        doc.mark_edited(1);
        assert!(!doc.pristine && doc.dirty && doc.revision == 1);
    }

    #[test]
    fn opening_a_file_detects_the_language_without_locking_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.py");
        fs::write(&path, "print('hi')\r\n").unwrap();
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        let doc = Document::from_file(&path, &loaded, &FileMeta::default(), &registry(), 5);
        assert_eq!(doc.language, "python");
        assert!(!doc.language_locked && !doc.dirty && !doc.untitled);
        assert_eq!(doc.line_ending, LineEnding::CrLf);
        assert_eq!(doc.title(), "script.py");
        assert_eq!(doc.created_ms, 5);
    }

    #[test]
    fn file_metadata_overrides_detection_and_locks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "echo hi\n").unwrap();
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        let meta = FileMeta {
            language: Some("shell".into()),
            arguments: Some("-x".into()),
            breakpoints: Some(vec![Breakpoint::at(0)]),
            ..FileMeta::default()
        };
        let doc = Document::from_file(&path, &loaded, &meta, &registry(), 0);
        assert_eq!(
            (doc.language.as_str(), doc.language_locked),
            ("shell", true)
        );
        assert!(doc.has_custom_run_settings());
        assert!(doc.breakpoints.contains(0));
    }

    #[test]
    fn redetection_respects_the_lock() {
        let registry = registry();
        let mut doc = Document::untitled(
            PathBuf::from("/u/Untitled.txt"),
            scratch_language::PLAIN_TEXT_ID,
            0,
        );
        doc.path = Some(PathBuf::from("/u/Untitled.py"));
        assert!(doc.redetect_language(&registry, "print(1)"));
        assert_eq!(doc.language, "python");
        doc.set_language("ruby");
        assert!(!doc.redetect_language(&registry, "print(1)"));
        assert_eq!(doc.language, "ruby");
    }

    #[test]
    fn shebang_changes_an_untitled_document() {
        let registry = registry();
        let mut doc = Document::untitled(
            PathBuf::from("/u/Untitled"),
            scratch_language::PLAIN_TEXT_ID,
            0,
        );
        assert!(doc.redetect_language(&registry, "#!/usr/bin/env python3\nprint(1)\n"));
        assert_eq!(doc.language, "python");
    }

    #[test]
    fn edit_save_cycle_tracks_dirty_and_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "x\n").unwrap();
        let loaded = load_text(&path, &ReadOptions::default()).unwrap();
        let mut doc = Document::from_file(&path, &loaded, &FileMeta::default(), &registry(), 1);
        doc.mark_edited(2);
        assert!(doc.dirty);
        assert_eq!((doc.revision, doc.updated_ms), (1, 2));
        let saved = save_text(&path, "y\n", &doc.save_options()).unwrap();
        doc.mark_saved(&saved, 3);
        assert!(!doc.dirty);
        assert_eq!(doc.stamp, Some(saved.stamp));
    }

    #[test]
    fn relocating_an_untitled_document_returns_the_unsaved_file() {
        let mut doc = Document::untitled(PathBuf::from("/u/Untitled.py"), "python", 0);
        let old = doc.relocate(PathBuf::from("/home/me/real.py"));
        assert_eq!(old, Some(PathBuf::from("/u/Untitled.py")));
        assert!(!doc.untitled);
        assert_eq!(doc.title(), "real.py");
        assert_eq!(doc.relocate(PathBuf::from("/home/me/other.py")), None);
    }

    #[test]
    fn to_meta_stores_only_user_choices() {
        let registry = registry();
        let mut doc = Document::untitled(PathBuf::from("/u/Untitled.py"), "python", 0);
        let meta = doc.to_meta(Some(&registry));
        assert_eq!(meta.language, None, "a detected language is not stored");
        doc.set_language("python");
        doc.run.arguments = "--fast".into();
        doc.view.selections = vec![Selection::caret(Position::new(2, 1))];
        let meta = doc.to_meta(Some(&registry));
        assert_eq!(meta.language.as_deref(), Some("python"));
        assert_eq!(meta.arguments.as_deref(), Some("--fast"));
        assert_eq!(meta.selection.as_ref().map(Vec::len), Some(1));
        assert_eq!(
            meta.language_uuid,
            registry.get("python").and_then(|l| l.uuid.clone())
        );
    }

    #[test]
    fn serde_round_trip_and_tolerance() {
        let mut doc = Document::untitled(PathBuf::from("/u/Untitled.py"), "python", 7);
        doc.breakpoints.toggle(4);
        doc.encoding = TextEncoding::ShiftJis;
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
        // unknown fields and missing fields are fine
        let minimal: Document = serde_json::from_str(r#"{"language":"go","future":1}"#).unwrap();
        assert_eq!(minimal.language, "go");
        assert_eq!(minimal.encoding, TextEncoding::Utf8);
    }
}
