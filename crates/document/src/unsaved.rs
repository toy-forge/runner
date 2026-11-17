//! The Unsaved store: untitled documents are real files.
//!
//! Like CodeRunner, a new document is a file (`Untitled.py`, `Untitled 2.py`)
//! in the Unsaved directory (`~/Library/Application Support/ScratchRunner/Unsaved`
//! by default, `advanced.autosave_documents_directory`), so it can be run
//! from there like any file and survives a crash. Next to the files, a hidden
//! `.scratchrunner/` directory keeps a [`DraftRecord`] per document:
//!
//! * for an **untitled** document a *manifest* (identity, language, encoding,
//!   view state; the text is the file itself);
//! * for a document backed by a **user's file** with unsaved edits a *draft*
//!   holding the metadata and the full text, so the edits can be recovered
//!   after a crash without touching the user's file.
//!
//! The app feeds edits to an [`AutosaveScheduler`] (a debounce with a
//! maximum wait) and, when documents come due, calls
//! [`UnsavedStore::autosave`]. At launch [`UnsavedStore::recover`] returns
//! the documents to reopen and [`UnsavedStore::cleanup_orphans`] removes
//! leftovers.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use scratch_language::{IndentStyle, LanguageRegistry, TemplateContext, render_template};

use crate::draft::{DraftRecord, DraftStore};
use crate::encoding::TextEncoding;
use crate::external::{ExternalState, check_external};
use crate::fileio::{FileStamp, ReadOptions, SaveError, SaveOptions, decode_bytes, save_text};
use crate::naming;
use crate::state::{Position, position_at_byte_offset};
use crate::{Document, DocumentId, line_ending};

const STATE_DIR: &str = ".scratchrunner";

/// Failures of the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An I/O error.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// Writing the document's file failed.
    #[error(transparent)]
    Save(#[from] SaveError),
    /// The language is unknown to the registry.
    #[error("unknown language `{0}`")]
    UnknownLanguage(String),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// What a new document starts with.
#[derive(Debug, Clone, Copy)]
pub enum InitialContent<'a> {
    /// Nothing.
    Empty,
    /// The language's default template, rendered (empty without one).
    Template,
    /// This text verbatim (New from Clipboard).
    Verbatim(&'a str),
    /// This text as a template: `<##>` places the caret and `___FILENAME___`
    /// style placeholders are substituted (the first-launch sample).
    Rendered(&'a str),
}

/// Options of [`UnsavedStore::create_untitled`].
#[derive(Debug, Clone)]
pub struct NewDocumentOptions {
    /// Indentation preference for templates.
    pub indent: IndentStyle,
    /// The encoding of the new file.
    pub encoding: TextEncoding,
    /// Names that count as taken besides the files in the directory.
    pub taken: HashSet<String>,
    /// The creation time (ms since the epoch).
    pub now_ms: u64,
}

impl Default for NewDocumentOptions {
    fn default() -> Self {
        Self {
            indent: IndentStyle::default(),
            encoding: TextEncoding::Utf8,
            taken: HashSet::new(),
            now_ms: crate::now_ms(),
        }
    }
}

/// A freshly created untitled document.
#[derive(Debug, Clone, PartialEq)]
pub struct NewUntitled {
    /// The document (its file exists in the Unsaved directory).
    pub document: Document,
    /// The initial text.
    pub text: String,
    /// Where the caret belongs: the template's `<##>`, else the end of the
    /// text.
    pub caret: Position,
}

/// A document found by [`UnsavedStore::recover`].
#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    /// The document, with the text's state as it was.
    pub document: Document,
    /// The text to put in the editor (`\n` line endings).
    pub text: String,
    /// What was recovered.
    pub kind: RecoveredKind,
}

/// The state of the file behind a recovered draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// The file is as it was when the draft was made.
    Unchanged,
    /// The file changed since: the recovered edits conflict with it.
    Changed,
    /// The file is gone.
    Missing,
}

/// What kind of document was recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveredKind {
    /// An untitled document, read back from its file.
    Untitled,
    /// Unsaved edits of a document with a file of its own.
    UnsavedEdits {
        /// How the file looks now.
        disk: DiskState,
    },
}

/// The result of [`UnsavedStore::recover`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryReport {
    /// The documents to reopen, oldest first.
    pub recovered: Vec<Recovered>,
    /// Drafts that could not be used, for the log.
    pub warnings: Vec<String>,
}

/// The result of [`UnsavedStore::cleanup_orphans`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// Drafts and manifests that no live document owns.
    pub removed_drafts: Vec<DocumentId>,
    /// Empty untitled files that went with them.
    pub removed_files: Vec<PathBuf>,
    /// Stale temporary files from interrupted writes.
    pub removed_temporary: usize,
}

/// See the crate documentation.
#[derive(Debug, Clone)]
pub struct UnsavedStore {
    dir: PathBuf,
    drafts: DraftStore,
}

impl UnsavedStore {
    /// A store over the directory `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let state = dir.join(STATE_DIR);
        Self {
            drafts: DraftStore::new(state.join("drafts"), state.join("recovery")),
            dir,
        }
    }

    /// The store for the preference `autosave_documents_directory` (which may
    /// contain `$(APPLICATION_SUPPORT)`).
    pub fn from_paths(paths: &scratch_config::Paths, autosave_directory_pref: &str) -> Self {
        Self::new(paths.autosave_directory(autosave_directory_pref))
    }

    /// The Unsaved directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The draft store (for tests and tools).
    pub fn drafts(&self) -> &DraftStore {
        &self.drafts
    }

    /// Whether `path` lies inside the Unsaved directory.
    pub fn contains(&self, path: &Path) -> bool {
        let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let dir = canon(&self.dir);
        let parent = path.parent().map(canon);
        parent.is_some_and(|p| p.starts_with(dir))
    }

    /// Creates a new untitled document in `language` (an id of `registry`):
    /// picks the next free `Untitled[ n].<ext>`, writes the file with the
    /// initial content and records the manifest.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownLanguage`], or I/O errors creating the directory
    /// or the files.
    pub fn create_untitled(
        &self,
        registry: &LanguageRegistry,
        language: &str,
        content: InitialContent<'_>,
        options: &NewDocumentOptions,
    ) -> Result<NewUntitled, StoreError> {
        let def = registry
            .get(language)
            .ok_or_else(|| StoreError::UnknownLanguage(language.to_owned()))?;
        fs::create_dir_all(&self.dir).map_err(io_error(&self.dir))?;
        let extension = def.extensions.first().map_or("", String::as_str);
        let on_disk = naming::dir_has(&self.dir);
        let name =
            naming::next_untitled_name(extension, |n| on_disk(n) || options.taken.contains(n));
        let path = self.dir.join(&name);

        let context = TemplateContext::for_file(&name, options.indent);
        let (text, caret) = match content {
            InitialContent::Empty => (String::new(), None),
            InitialContent::Verbatim(text) => (line_ending::normalize(text).into_owned(), None),
            InitialContent::Template => {
                let rendered = registry.new_document(language, &context);
                (rendered.text, rendered.caret_offset)
            }
            InitialContent::Rendered(template) => {
                let rendered = render_template(template, &context);
                (rendered.text, rendered.caret_offset)
            }
        };
        let caret = position_at_byte_offset(&text, caret.unwrap_or(text.len()));

        let mut document = Document::untitled(path.clone(), language, options.now_ms);
        document.encoding = options.encoding;
        document.view.set_caret(caret);
        let save = SaveOptions {
            encoding: options.encoding,
            ..SaveOptions::default()
        };
        let saved = save_text(&path, &text, &save)?;
        document.stamp = Some(saved.stamp);
        self.drafts
            .save(&DraftRecord::manifest(document.clone(), options.now_ms))
            .map_err(io_error(&self.dir))?;
        Ok(NewUntitled {
            document,
            text,
            caret,
        })
    }

    /// The document a very first launch opens: a Shell Script with the
    /// welcome sample (`scratch_language::FIRST_LAUNCH_TEXT`), caret at its
    /// marker.
    ///
    /// # Errors
    ///
    /// As [`UnsavedStore::create_untitled`].
    pub fn create_first_launch(
        &self,
        registry: &LanguageRegistry,
        options: &NewDocumentOptions,
    ) -> Result<NewUntitled, StoreError> {
        self.create_untitled(
            registry,
            scratch_language::FIRST_LAUNCH_LANGUAGE,
            InitialContent::Rendered(scratch_language::FIRST_LAUNCH_TEXT),
            options,
        )
    }

    /// `New from Clipboard`: a new untitled document holding `text` (verbatim)
    /// in the language the text looks like (shebang, modeline, content), or in
    /// `fallback` when nothing matches.
    ///
    /// # Errors
    ///
    /// As [`UnsavedStore::create_untitled`].
    pub fn create_from_clipboard(
        &self,
        registry: &LanguageRegistry,
        text: &str,
        fallback: &str,
        options: &NewDocumentOptions,
    ) -> Result<NewUntitled, StoreError> {
        let language = registry
            .guess_from_text(text)
            .map_or(fallback, |guess| guess.language.id.as_str());
        self.create_untitled(registry, language, InitialContent::Verbatim(text), options)
    }

    /// Writes the current state of `document` (whose text is `text`, `\n`
    /// line endings): an untitled document's file and manifest, or the draft
    /// of a document with unsaved edits. Returns the new stamp of the
    /// untitled file (store it in `document.stamp`), `None` for drafts.
    ///
    /// # Errors
    ///
    /// Write errors; nothing is lost if one occurs, the previous state stays.
    pub fn autosave(
        &self,
        document: &Document,
        text: &str,
    ) -> Result<Option<FileStamp>, StoreError> {
        let now = crate::now_ms();
        if document.untitled {
            let Some(path) = &document.path else {
                return Ok(None);
            };
            fs::create_dir_all(&self.dir).map_err(io_error(&self.dir))?;
            let saved = save_text(path, text, &document.save_options())?;
            let mut stored = document.clone();
            stored.stamp = Some(saved.stamp);
            self.drafts
                .save(&DraftRecord::manifest(stored, now))
                .map_err(io_error(&self.dir))?;
            Ok(Some(saved.stamp))
        } else {
            self.drafts
                .save(&DraftRecord::with_content(
                    document.clone(),
                    text.to_owned(),
                    now,
                ))
                .map_err(io_error(&self.dir))?;
            Ok(None)
        }
    }

    /// Forgets the draft of a document that was saved to its file or closed
    /// without saving; keeps the untitled file (see [`UnsavedStore::discard`]).
    ///
    /// # Errors
    ///
    /// I/O errors deleting the draft files.
    pub fn clear_draft(&self, id: DocumentId) -> Result<(), StoreError> {
        self.drafts.remove(id).map_err(io_error(&self.dir))
    }

    /// Removes an untitled document for good (it was closed without saving, or
    /// saved elsewhere): its manifest and its file in the Unsaved directory.
    /// Files outside the directory are never deleted.
    ///
    /// # Errors
    ///
    /// I/O errors other than "not found".
    pub fn discard(&self, document: &Document) -> Result<(), StoreError> {
        self.clear_draft(document.id)?;
        if document.untitled
            && let Some(path) = document.path.as_deref().filter(|p| self.contains(p))
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(path)(error)),
            }
        }
        Ok(())
    }

    /// Cleans up after an untitled document was saved to a file of its own
    /// (see [`Document::relocate`], which returns `unsaved_path`): its manifest
    /// and its file in the Unsaved directory are removed. A path outside the
    /// directory is never deleted.
    ///
    /// # Errors
    ///
    /// I/O errors other than "not found".
    pub fn finish_save_as(&self, id: DocumentId, unsaved_path: &Path) -> Result<(), StoreError> {
        self.clear_draft(id)?;
        if self.contains(unsaved_path) {
            match fs::remove_file(unsaved_path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(unsaved_path)(error)),
            }
        }
        Ok(())
    }

    /// Renames an untitled document's file for a new language (`Untitled.py`
    /// becomes `Untitled.rb`; the number is kept unless taken) and updates
    /// `document.path`. Returns the new path, or `None` when the document is
    /// not untitled or the name stays.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownLanguage`] or I/O errors.
    pub fn retarget_untitled(
        &self,
        document: &mut Document,
        registry: &LanguageRegistry,
        language: &str,
    ) -> Result<Option<PathBuf>, StoreError> {
        if !document.untitled {
            return Ok(None);
        }
        let def = registry
            .get(language)
            .ok_or_else(|| StoreError::UnknownLanguage(language.to_owned()))?;
        let Some(path) = document.path.clone().filter(|p| self.contains(p)) else {
            return Ok(None);
        };
        let current = document.title();
        let extension = def.extensions.first().map_or("", String::as_str);
        let on_disk = naming::dir_has(&self.dir);
        let new_name = naming::retarget_untitled_name(&current, extension, |n| on_disk(n));
        if new_name == current {
            return Ok(None);
        }
        let new_path = self.dir.join(&new_name);
        fs::rename(&path, &new_path).map_err(io_error(&path))?;
        document.path = Some(new_path.clone());
        self.drafts
            .save(&DraftRecord::manifest(document.clone(), crate::now_ms()))
            .map_err(io_error(&self.dir))?;
        Ok(Some(new_path))
    }

    /// Reads every draft and manifest: the documents to reopen after a quit or
    /// a crash. Untitled documents come back from their files; a manifest whose
    /// file is gone is recovered from its recorded text when it has one and
    /// dropped otherwise. Drafts of documents with a file carry the unsaved
    /// edits and how the file on disk looks now.
    pub fn recover(&self) -> RecoveryReport {
        let mut report = RecoveryReport::default();
        for id in self.drafts.ids() {
            match self.drafts.load(id) {
                Ok(Some(record)) => match self.recover_one(record) {
                    Ok(Some(recovered)) => report.recovered.push(recovered),
                    Ok(None) => {
                        let _ = self.drafts.remove(id);
                    }
                    Err(warning) => report.warnings.push(warning),
                },
                Ok(None) => {}
                Err(error) => report
                    .warnings
                    .push(format!("draft {id} is unreadable: {error}")),
            }
        }
        report
            .recovered
            .sort_by_key(|r| (r.document.created_ms, r.document.updated_ms));
        report
    }

    fn recover_one(&self, record: DraftRecord) -> Result<Option<Recovered>, String> {
        let DraftRecord {
            document, content, ..
        } = record;
        let id = document.id;
        if document.untitled {
            let Some(path) = document.path.clone() else {
                return Ok(None);
            };
            match fs::read(&path) {
                Ok(bytes) => {
                    let read = ReadOptions {
                        forced_encoding: Some(document.encoding),
                        ..ReadOptions::default()
                    };
                    let decoded = decode_bytes(&bytes, &read, None)
                        .or_else(|_| decode_bytes(&bytes, &ReadOptions::default(), None))
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok(Some(Recovered {
                        document,
                        text: decoded.text,
                        kind: RecoveredKind::Untitled,
                    }))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => match content {
                    Some(text) => {
                        save_text(&path, &text, &document.save_options()).map_err(|e| {
                            format!("draft {id}: cannot restore {}: {e}", path.display())
                        })?;
                        Ok(Some(Recovered {
                            document,
                            text,
                            kind: RecoveredKind::Untitled,
                        }))
                    }
                    None => Ok(None),
                },
                Err(error) => Err(format!("{}: {error}", path.display())),
            }
        } else {
            let Some(text) = content else {
                return Ok(None);
            };
            let disk = match (&document.path, &document.stamp) {
                (Some(path), Some(stamp)) => match check_external(path, stamp) {
                    Ok(ExternalState::Unchanged | ExternalState::Touched(_)) => {
                        DiskState::Unchanged
                    }
                    Ok(ExternalState::Modified(_)) => DiskState::Changed,
                    Ok(ExternalState::Deleted) => DiskState::Missing,
                    Err(_) => DiskState::Changed,
                },
                (Some(path), None) => {
                    if path.exists() {
                        DiskState::Changed
                    } else {
                        DiskState::Missing
                    }
                }
                _ => DiskState::Missing,
            };
            Ok(Some(Recovered {
                document,
                text,
                kind: RecoveredKind::UnsavedEdits { disk },
            }))
        }
    }

    /// Removes what no live document owns: drafts and manifests of ids not in
    /// `live` (with their untitled file when that file is empty; a file with
    /// content is kept, it is the user's text), and temporary files of
    /// interrupted writes older than an hour. Call after the recovered
    /// documents were adopted.
    pub fn cleanup_orphans(&self, live: &HashSet<DocumentId>) -> CleanupReport {
        let mut report = CleanupReport::default();
        for id in self.drafts.ids() {
            if live.contains(&id) {
                continue;
            }
            if let Ok(Some(record)) = self.drafts.load(id) {
                let document = record.document;
                if let Some(path) = document.path.as_deref().filter(|_| document.untitled) {
                    let empty = fs::metadata(path).is_ok_and(|m| m.len() == 0);
                    if empty && self.contains(path) && fs::remove_file(path).is_ok() {
                        report.removed_files.push(path.to_path_buf());
                    }
                }
            }
            if self.drafts.remove(id).is_ok() {
                report.removed_drafts.push(id);
            }
        }
        let stale = SystemTime::now().checked_sub(Duration::from_secs(3600));
        for dir in [
            &self.dir,
            self.drafts.drafts_dir(),
            self.drafts.recovery_dir(),
        ] {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !(name.starts_with('.') && name.ends_with(".tmp")) {
                    continue;
                }
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .zip(stale)
                    .is_some_and(|(m, s)| m < s);
                if old && fs::remove_file(entry.path()).is_ok() {
                    report.removed_temporary += 1;
                }
            }
        }
        report
    }
}

/// A debounce with a maximum wait for autosaving: a document is due when it
/// has been quiet for `delay`, or when it has been dirty for `max_wait` even
/// though edits keep coming. Times are milliseconds from any monotonic or
/// wall clock the caller uses consistently; nothing here sleeps.
///
/// ```
/// use scratch_document::{AutosaveScheduler, DocumentId};
///
/// let mut scheduler = AutosaveScheduler::new(1_000, 10_000);
/// let id = DocumentId::new();
/// scheduler.note_edit(id, 0);
/// scheduler.note_edit(id, 600);
/// assert!(scheduler.due(1_200).is_empty());          // edited 600 ms ago
/// assert_eq!(scheduler.due(1_600), vec![id]);          // quiet for 1 s
/// scheduler.mark_saved(id);
/// assert_eq!(scheduler.next_deadline(), None);
/// ```
#[derive(Debug, Clone)]
pub struct AutosaveScheduler {
    delay: u64,
    max_wait: u64,
    /// id -> (first unsaved edit, last edit)
    pending: HashMap<DocumentId, (u64, u64)>,
}

impl AutosaveScheduler {
    /// A scheduler that waits `delay_ms` after the last edit and at most
    /// `max_wait_ms` after the first unsaved one.
    pub fn new(delay_ms: u64, max_wait_ms: u64) -> Self {
        Self {
            delay: delay_ms,
            max_wait: max_wait_ms.max(delay_ms),
            pending: HashMap::new(),
        }
    }

    /// The defaults: 1.5 s of quiet, 10 s at most.
    pub fn standard() -> Self {
        Self::new(1_500, 10_000)
    }

    /// Records an edit of `id` at `now_ms`.
    pub fn note_edit(&mut self, id: DocumentId, now_ms: u64) {
        self.pending
            .entry(id)
            .and_modify(|(_, last)| *last = now_ms)
            .or_insert((now_ms, now_ms));
    }

    fn deadline(&self, first: u64, last: u64) -> u64 {
        (last + self.delay).min(first + self.max_wait)
    }

    /// The documents to save now, in no particular order. They stay pending
    /// until [`AutosaveScheduler::mark_saved`].
    pub fn due(&self, now_ms: u64) -> Vec<DocumentId> {
        self.pending
            .iter()
            .filter(|(_, (first, last))| self.deadline(*first, *last) <= now_ms)
            .map(|(id, _)| *id)
            .collect()
    }

    /// The time of the next deadline, for arming a timer; `None` when nothing
    /// is pending.
    pub fn next_deadline(&self) -> Option<u64> {
        self.pending
            .values()
            .map(|(first, last)| self.deadline(*first, *last))
            .min()
    }

    /// `id` was saved (or closed): stop tracking it.
    pub fn mark_saved(&mut self, id: DocumentId) {
        self.pending.remove(&id);
    }

    /// Whether `id` has unsaved edits tracked.
    pub fn is_pending(&self, id: DocumentId) -> bool {
        self.pending.contains_key(&id)
    }

    /// Everything pending, for a final flush at quit.
    pub fn pending(&self) -> Vec<DocumentId> {
        self.pending.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileio::{ReadOptions, load_text};
    use std::collections::HashSet;

    fn registry() -> LanguageRegistry {
        LanguageRegistry::builtin()
    }

    fn store() -> (tempfile::TempDir, UnsavedStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = UnsavedStore::new(dir.path().join("Unsaved"));
        (dir, store)
    }

    fn new_doc(store: &UnsavedStore, language: &str, content: InitialContent<'_>) -> NewUntitled {
        store
            .create_untitled(
                &registry(),
                language,
                content,
                &NewDocumentOptions::default(),
            )
            .unwrap()
    }

    #[test]
    fn new_documents_are_real_files_with_counting_names() {
        let (_dir, store) = store();
        let a = new_doc(&store, "python", InitialContent::Empty);
        let b = new_doc(&store, "python", InitialContent::Empty);
        let c = new_doc(&store, "c", InitialContent::Empty);
        assert_eq!(a.document.title(), "Untitled.py");
        assert_eq!(b.document.title(), "Untitled 2.py");
        assert_eq!(c.document.title(), "Untitled.c");
        assert!(a.document.path.as_ref().unwrap().is_file());
        assert!(a.document.untitled && !a.document.dirty);
        assert_eq!(store.drafts().ids().len(), 3);
    }

    #[test]
    fn taken_names_are_skipped() {
        let (_dir, store) = store();
        let options = NewDocumentOptions {
            taken: HashSet::from(["Untitled.py".to_owned()]),
            ..NewDocumentOptions::default()
        };
        let doc = store
            .create_untitled(&registry(), "python", InitialContent::Empty, &options)
            .unwrap();
        assert_eq!(doc.document.title(), "Untitled 2.py");
    }

    #[test]
    fn templates_render_with_the_caret_where_the_marker_was() {
        let (_dir, store) = store();
        let doc = new_doc(
            &store,
            "shell",
            InitialContent::Rendered("#!/bin/bash\necho \"hi\"<##>"),
        );
        assert_eq!(doc.text, "#!/bin/bash\necho \"hi\"");
        assert_eq!(doc.caret, Position::new(1, 9));
        assert_eq!(doc.document.view.caret(), Position::new(1, 9));
        let on_disk = fs::read_to_string(doc.document.path.as_ref().unwrap()).unwrap();
        assert_eq!(on_disk, doc.text);
    }

    #[test]
    fn language_default_templates_and_verbatim_clipboard_text() {
        let (_dir, store) = store();
        let templated = new_doc(&store, "c", InitialContent::Template);
        assert_eq!(templated.document.language, "c");
        let verbatim = new_doc(&store, "python", InitialContent::Verbatim("a\r\nb"));
        assert_eq!(verbatim.text, "a\nb");
        assert_eq!(verbatim.caret, Position::new(1, 1), "caret at the end");
    }

    #[test]
    fn first_launch_document_is_the_welcome_shell_script() {
        let (_dir, store) = store();
        let doc = store
            .create_first_launch(&registry(), &NewDocumentOptions::default())
            .unwrap();
        assert_eq!(doc.document.language, "shell");
        assert!(doc.text.starts_with("#!/bin/bash\necho"));
        assert!(!doc.text.contains("<##>"));
        assert_eq!(doc.document.title(), "Untitled.sh");
    }

    #[test]
    fn clipboard_documents_pick_the_language_from_the_text() {
        let (_dir, store) = store();
        let options = NewDocumentOptions::default();
        let py = store
            .create_from_clipboard(
                &registry(),
                "#!/usr/bin/env python3\nprint(1)\n",
                "plain-text",
                &options,
            )
            .unwrap();
        assert_eq!(py.document.language, "python");
        assert_eq!(py.document.title(), "Untitled.py");
        let plain = store
            .create_from_clipboard(&registry(), "just some words", "plain-text", &options)
            .unwrap();
        assert_eq!(plain.document.language, "plain-text");
    }

    #[test]
    fn unknown_language_is_an_error() {
        let (_dir, store) = store();
        let err = store
            .create_untitled(
                &registry(),
                "klingon",
                InitialContent::Empty,
                &NewDocumentOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::UnknownLanguage(_)));
    }

    #[test]
    fn autosave_writes_the_file_and_recovery_reads_it_back() {
        let (_dir, store) = store();
        let mut created = new_doc(&store, "python", InitialContent::Empty);
        created.document.mark_edited(5);
        created.document.view.set_caret(Position::new(0, 8));
        let stamp = store.autosave(&created.document, "print(1)\n").unwrap();
        assert!(stamp.is_some());
        let path = created.document.path.clone().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "print(1)\n");

        // "crash": a new store over the same directory
        let reopened = UnsavedStore::new(store.dir());
        let report = reopened.recover();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.recovered.len(), 1);
        let recovered = &report.recovered[0];
        assert_eq!(recovered.kind, RecoveredKind::Untitled);
        assert_eq!(recovered.text, "print(1)\n");
        assert_eq!(recovered.document.id, created.document.id);
        assert!(recovered.document.dirty);
        assert_eq!(recovered.document.view.caret(), Position::new(0, 8));
    }

    #[test]
    fn crlf_untitled_documents_keep_their_ending_through_recovery() {
        let (_dir, store) = store();
        let mut created = new_doc(&store, "python", InitialContent::Empty);
        created.document.line_ending = crate::LineEnding::CrLf;
        store.autosave(&created.document, "a\nb\n").unwrap();
        let path = created.document.path.clone().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\r\nb\r\n");
        let report = store.recover();
        assert_eq!(report.recovered[0].text, "a\nb\n");
    }

    #[test]
    fn a_deleted_untitled_file_is_restored_from_nothing_and_dropped() {
        let (_dir, store) = store();
        let created = new_doc(&store, "python", InitialContent::Empty);
        fs::remove_file(created.document.path.as_ref().unwrap()).unwrap();
        let report = store.recover();
        assert!(report.recovered.is_empty());
        assert!(
            store.drafts().ids().is_empty(),
            "the useless manifest is dropped"
        );
    }

    #[test]
    fn unsaved_edits_of_a_real_file_are_recovered_without_touching_it() {
        let (dir, store) = store();
        let file = dir.path().join("real.py");
        fs::write(&file, "original\n").unwrap();
        let loaded = load_text(&file, &ReadOptions::default()).unwrap();
        let mut document =
            Document::from_file(&file, &loaded, &crate::FileMeta::default(), &registry(), 1);
        document.mark_edited(2);
        store.autosave(&document, "edited\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "original\n");

        let report = store.recover();
        assert_eq!(report.recovered.len(), 1);
        assert_eq!(report.recovered[0].text, "edited\n");
        assert_eq!(
            report.recovered[0].kind,
            RecoveredKind::UnsavedEdits {
                disk: DiskState::Unchanged
            }
        );

        // the file changes behind our back, then is deleted
        fs::write(&file, "someone else\n").unwrap();
        let report = store.recover();
        assert_eq!(
            report.recovered[0].kind,
            RecoveredKind::UnsavedEdits {
                disk: DiskState::Changed
            }
        );
        fs::remove_file(&file).unwrap();
        let report = store.recover();
        assert_eq!(
            report.recovered[0].kind,
            RecoveredKind::UnsavedEdits {
                disk: DiskState::Missing
            }
        );
    }

    #[test]
    fn discard_removes_manifest_and_file_but_never_foreign_files() {
        let (dir, store) = store();
        let created = new_doc(&store, "python", InitialContent::Empty);
        let path = created.document.path.clone().unwrap();
        store.discard(&created.document).unwrap();
        assert!(!path.exists());
        assert!(store.drafts().ids().is_empty());

        let outside = dir.path().join("precious.py");
        fs::write(&outside, "keep").unwrap();
        let mut sneaky = Document::untitled(outside.clone(), "python", 0);
        sneaky.untitled = true;
        store.discard(&sneaky).unwrap();
        assert!(outside.exists(), "a file outside Unsaved is never deleted");
    }

    #[test]
    fn save_as_cleans_up_the_unsaved_file() {
        let (dir, store) = store();
        let mut created = new_doc(&store, "python", InitialContent::Empty);
        let unsaved = created
            .document
            .relocate(dir.path().join("saved.py"))
            .unwrap();
        store.finish_save_as(created.document.id, &unsaved).unwrap();
        assert!(!unsaved.exists());
        assert!(store.drafts().ids().is_empty());
        let outside = dir.path().join("keep.py");
        fs::write(&outside, "x").unwrap();
        store.finish_save_as(created.document.id, &outside).unwrap();
        assert!(outside.exists());
    }

    #[test]
    fn retargeting_renames_the_untitled_file() {
        let (_dir, store) = store();
        let mut created = new_doc(&store, "python", InitialContent::Empty);
        let _second = new_doc(&store, "python", InitialContent::Empty);
        let old = created.document.path.clone().unwrap();
        let moved = store
            .retarget_untitled(&mut created.document, &registry(), "ruby")
            .unwrap()
            .unwrap();
        assert_eq!(moved.file_name().unwrap(), "Untitled.rb");
        assert!(!old.exists() && moved.exists());
        assert_eq!(created.document.path.as_ref(), Some(&moved));
        // unchanged extension: nothing to do
        assert_eq!(
            store
                .retarget_untitled(&mut created.document, &registry(), "ruby")
                .unwrap(),
            None
        );
        // the manifest follows the new path
        let report = store.recover();
        assert!(
            report
                .recovered
                .iter()
                .any(|r| r.document.path.as_ref() == Some(&moved))
        );
    }

    #[test]
    fn cleanup_removes_orphans_and_empty_files_but_keeps_live_and_content() {
        let (_dir, store) = store();
        let live = new_doc(&store, "python", InitialContent::Empty);
        let orphan_empty = new_doc(&store, "c", InitialContent::Empty);
        let mut orphan_full = new_doc(&store, "go", InitialContent::Empty);
        orphan_full.document.mark_edited(1);
        store
            .autosave(&orphan_full.document, "package main\n")
            .unwrap();
        let stale = store.dir().join(".Untitled.py.1.1.tmp");
        fs::write(&stale, "x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(7200);
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let fresh = store.dir().join(".Untitled.py.2.2.tmp");
        fs::write(&fresh, "x").unwrap();

        let keep = HashSet::from([live.document.id]);
        let report = store.cleanup_orphans(&keep);
        assert_eq!(report.removed_drafts.len(), 2);
        assert_eq!(
            report.removed_files,
            vec![orphan_empty.document.path.clone().unwrap()]
        );
        assert_eq!(report.removed_temporary, 1);
        assert!(!stale.exists() && fresh.exists());
        assert!(live.document.path.as_ref().unwrap().exists());
        assert!(
            orphan_full.document.path.as_ref().unwrap().exists(),
            "content is never deleted"
        );
        assert_eq!(store.drafts().ids(), vec![live.document.id]);
    }

    #[test]
    fn expands_the_preference_template() {
        let dir = tempfile::tempdir().unwrap();
        let paths = scratch_config::Paths::in_dir(dir.path());
        let store =
            UnsavedStore::from_paths(&paths, "$(APPLICATION_SUPPORT)/ScratchRunner/Unsaved");
        assert_eq!(store.dir(), paths.unsaved());
        let store = UnsavedStore::from_paths(&paths, "");
        assert_eq!(store.dir(), paths.unsaved());
    }

    #[test]
    fn scheduler_debounces_and_caps_the_wait() {
        let mut s = AutosaveScheduler::new(1_000, 5_000);
        let (a, b) = (DocumentId::new(), DocumentId::new());
        s.note_edit(a, 0);
        s.note_edit(b, 500);
        assert_eq!(s.next_deadline(), Some(1_000));
        assert!(s.due(999).is_empty());
        assert_eq!(s.due(1_000), vec![a]);
        // typing continuously: a is edited every 400 ms but must still save by 5 s
        for t in (400..=4_800).step_by(400) {
            s.note_edit(a, t);
            assert!(!s.due(t).contains(&a), "still being typed at {t}");
        }
        assert!(s.due(5_000).contains(&a));
        s.mark_saved(a);
        assert!(!s.is_pending(a));
        assert_eq!(s.pending(), vec![b]);
        assert!(s.due(100_000).contains(&b));
    }
}
