//! Project-wide search across folders, selected items, custom path lists and
//! open documents ("Search in File System").
//!
//! [`SearchRequest`] describes one search: the source, the mode (contents,
//! filenames or both), the options of the Search sidebar's Options popover,
//! a compiled [`Matcher`] and a result cap. [`search_project`] runs it
//! synchronously; [`spawn_search`] runs it on worker threads and streams
//! [`SearchEvent`]s over a channel (the UI drains them in batches with
//! [`SearchHandle::recv_batch`]).
//!
//! Results carry **byte offsets** into the decoded file text plus 0-based
//! line / UTF-16 column / UTF-16 length for display. [`SearchFile`] records
//! the encoding, a content hash and the decoded text of snapshot documents,
//! so [`ReplacePlan`](crate::project::ReplacePlan) can verify that nothing
//! changed between search and replace.
//!
//! File walking uses the `ignore` crate with all standard filters turned off
//! (neither `.gitignore` nor hidden files are consulted unless
//! [`SearchOptions::hidden`] is set - CodeRunner's excluded-directory
//! preference is explicit, not git-aware). Files are read and decoded
//! (`encoding_rs`, BOM + UTF-8 + fallback), binaries are skipped by sniffing
//! a NUL byte, and the search is cooperatively cancellable at every file and
//! every match.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use encoding_rs::Encoding;
use ignore::{DirEntry, WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::matcher::{MatchError, Matcher};
use crate::project::encoding::{
    TextEncoding, content_hash, decode, default_fallback, looks_binary,
};
use crate::text::{LineIndex, byte_to_utf16, utf16_len};

/// Stable identity of one searched file (a hash of its path, or of its name
/// for an untitled snapshot). Stays the same across searches of the same
/// file, so selections can be remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileId(pub u64);

impl FileId {
    fn of_path(path: &Path) -> Self {
        Self(content_hash(path.as_os_str().as_encoded_bytes()))
    }

    fn of_snapshot(name: &str, index: usize) -> Self {
        let mut key = name.as_bytes().to_vec();
        key.push(0);
        key.extend_from_slice(&(index as u64).to_le_bytes());
        Self(content_hash(&key))
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// Stable identity of one match: its file plus its index in that file's match
/// list (filename matches first, then content matches in byte order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MatchId {
    /// The file the match belongs to.
    pub file: FileId,
    /// Index among the file's matches.
    pub index: u32,
}

/// What the Search sidebar's "item" popup searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SearchMode {
    /// Search both file contents and file names.
    #[default]
    ContentsAndFilenames,
    /// Search file contents only.
    Contents,
    /// Search file names only; a replace renames the files.
    Filenames,
}

impl SearchMode {
    /// The popup labels of report 01 section 8.2.
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::ContentsAndFilenames => "File Contents + Filenames",
            SearchMode::Contents => "File Contents",
            SearchMode::Filenames => "Filenames",
        }
    }

    /// Whether file contents are searched.
    pub fn searches_contents(self) -> bool {
        matches!(
            self,
            SearchMode::ContentsAndFilenames | SearchMode::Contents
        )
    }

    /// Whether file names are searched.
    pub fn searches_filenames(self) -> bool {
        matches!(
            self,
            SearchMode::ContentsAndFilenames | SearchMode::Filenames
        )
    }
}

/// Where a search looks.
#[derive(Debug, Clone)]
pub enum SearchSource {
    /// The folder of the Search sidebar's "Where" popup.
    Folder(PathBuf),
    /// Files/folders currently selected in the file navigator.
    SelectedItems(Vec<PathBuf>),
    /// The "Custom List of Paths..." sheet (absolute paths, one per line).
    CustomPaths(Vec<PathBuf>),
    /// Open documents, searched from in-memory snapshots the UI supplies.
    OpenDocuments(Vec<DocumentSnapshot>),
}

impl SearchSource {
    /// The paths the walk visits (empty for open documents).
    pub fn roots(&self) -> Vec<PathBuf> {
        match self {
            SearchSource::Folder(p) => vec![p.clone()],
            SearchSource::SelectedItems(p) | SearchSource::CustomPaths(p) => p.clone(),
            SearchSource::OpenDocuments(_) => Vec::new(),
        }
    }
}

/// An open document's text, as the editor sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSnapshot {
    /// Display name ("Untitled 2", "main.rs").
    pub name: String,
    /// Path when the document has one.
    pub path: Option<PathBuf>,
    /// The current buffer contents.
    pub text: String,
}

impl DocumentSnapshot {
    /// Creates a snapshot.
    pub fn new(name: impl Into<String>, path: Option<PathBuf>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path,
            text: text.into(),
        }
    }
}

/// Optional predicate restricting matches to a syntax scope.
///
/// This crate has no grammar engine, so the scope selector string alone
/// cannot be interpreted here; a caller that owns a syntax highlighter can
/// compile [`SearchOptions::scope_selector`] and hand the resulting
/// predicate in. The predicate receives the **byte offset of the match start**
/// in the decoded text.
pub type ScopeFilter = Arc<dyn Fn(usize) -> bool + Send + Sync>;

/// The Search sidebar Options popover (report 01 section 8.2).
#[derive(Clone)]
pub struct SearchOptions {
    /// "Search Subdirectories".
    pub subdirectories: bool,
    /// "Hidden files".
    pub hidden: bool,
    /// "Binary file contents": also search files that sniff as binary.
    pub binary: bool,
    /// "Search File Extensions" (`Example: txt, html`). Empty = all.
    pub include_extensions: Vec<String>,
    /// "Exclude File Extensions" (`Example: css, svg`).
    pub exclude_extensions: Vec<String>,
    /// "Exclude Directories" (`Example: .git, vendor`), matched by directory
    /// name.
    pub excluded_directories: Vec<String>,
    /// "Search Scope Selector" (`Example: comment, variable, string`) as
    /// typed; kept for preference persistence and for
    /// [`scope_filter`](Self::scope_filter).
    pub scope_selector: Option<String>,
    /// Optional compiled form of `scope_selector` (see [`ScopeFilter`]).
    pub scope_filter: Option<ScopeFilter>,
    /// Files larger than this are skipped silently. Default: no limit.
    pub max_file_size: u64,
    /// Encoding used when bytes are neither BOM-marked nor valid UTF-8
    /// (windows-1252 decodes every byte).
    pub fallback_encoding: &'static Encoding,
}

impl SearchOptions {
    /// Sets the fallback encoding by WHATWG label (`"utf-8"`, `"shift_jis"`,
    /// ...). Returns `None` for an unknown label.
    pub fn with_fallback_name(mut self, label: &str) -> Option<Self> {
        self.fallback_encoding = Encoding::for_label(label.trim().as_bytes())?;
        Some(self)
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            subdirectories: true,
            hidden: false,
            binary: false,
            include_extensions: Vec::new(),
            exclude_extensions: Vec::new(),
            excluded_directories: Vec::new(),
            scope_selector: None,
            scope_filter: None,
            max_file_size: u64::MAX,
            fallback_encoding: default_fallback(),
        }
    }
}

impl fmt::Debug for SearchOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchOptions")
            .field("subdirectories", &self.subdirectories)
            .field("hidden", &self.hidden)
            .field("binary", &self.binary)
            .field("include_extensions", &self.include_extensions)
            .field("exclude_extensions", &self.exclude_extensions)
            .field("excluded_directories", &self.excluded_directories)
            .field("scope_selector", &self.scope_selector)
            .field(
                "scope_filter",
                &self.scope_filter.as_ref().map(|_| "<predicate>"),
            )
            .field("max_file_size", &self.max_file_size)
            .field("fallback_encoding", &self.fallback_encoding.name())
            .finish()
    }
}

/// One project search.
pub struct SearchRequest {
    /// Folder / selection / custom paths / open documents.
    pub source: SearchSource,
    /// Contents, filenames or both.
    pub mode: SearchMode,
    /// Search sidebar options.
    pub options: SearchOptions,
    /// The compiled find/replace query (search and replace share it).
    pub matcher: Matcher,
    /// Stop after this many matches (`None` = unlimited). CodeRunner asks
    /// "The search is producing a very large number of matches..." at 10 000
    /// and stops if the user declines; the UI owns that policy.
    pub max_results: Option<usize>,
    /// Worker threads; `None` = available parallelism (capped at 8).
    pub threads: Option<usize>,
}

impl SearchRequest {
    /// A request for `source` with default options and no result cap.
    pub fn new(source: SearchSource, matcher: Matcher) -> Self {
        Self {
            source,
            mode: SearchMode::ContentsAndFilenames,
            options: SearchOptions::default(),
            matcher,
            max_results: None,
            threads: None,
        }
    }
}

impl fmt::Debug for SearchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchRequest")
            .field("source", &self.source)
            .field("mode", &self.mode)
            .field("options", &self.options)
            .field("matcher", &self.matcher)
            .field("max_results", &self.max_results)
            .field("threads", &self.threads)
            .finish()
    }
}

/// Whether a match is in a file's contents or its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MatchKind {
    /// The file name matched (`Filenames` / `File Contents + Filenames`).
    FileName,
    /// The contents matched.
    Content,
}

/// One match of a project search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMatch {
    /// Stable id (file + per-file index).
    pub id: MatchId,
    /// The file.
    pub file: FileId,
    /// Contents or file name.
    pub kind: MatchKind,
    /// 0-based line of the match start (0 for filename matches).
    pub line: u32,
    /// UTF-16 column of the match start on that line.
    pub column: u32,
    /// UTF-16 length of the match (may span lines; then it is the length of
    /// the whole match text, while `line_text` is only the first line).
    pub length: u32,
    /// Byte range in the decoded file text (or in the file name).
    pub byte_range: std::ops::Range<usize>,
    /// The full text of the first line, without its terminator.
    pub line_text: String,
    /// The file's display name.
    pub name: String,
}

impl ProjectMatch {
    /// 1-based line for display.
    pub fn display_line(&self) -> u32 {
        self.line + 1
    }
}

/// A file that produced at least one match.
#[derive(Debug, Clone)]
pub struct SearchFile {
    /// Stable id.
    pub id: FileId,
    /// Absolute or root-relative path as visited (for snapshots: the name).
    pub path: PathBuf,
    /// Display name (last path component, or the snapshot name).
    pub name: String,
    /// In-memory text for open-document snapshots; `None` for disk files.
    pub snapshot: Option<Arc<str>>,
    /// Detected encoding (`None` for snapshots).
    pub encoding: Option<TextEncoding>,
    /// The decoded text has `\r\n` line ends (drives the CRLF matcher).
    pub crlf: bool,
    /// Decoding lost bytes (replacement characters); replace refuses these.
    pub lossy: bool,
    /// The file sniffed as binary.
    pub binary: bool,
    /// The entry is a directory (only filename matches can come from one;
    /// replacing it renames the folder).
    pub is_dir: bool,
    /// File size in bytes.
    pub size: u64,
    /// FNV-1a hash of the decoded text (verifies snapshots).
    pub text_hash: u64,
    /// FNV-1a hash of the raw bytes (verifies disk files).
    pub byte_hash: u64,
}

/// A warning produced while walking or reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIssue {
    /// The file or directory involved, when known.
    pub path: Option<PathBuf>,
    /// Human-readable reason.
    pub message: String,
}

impl SearchIssue {
    fn new(path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }
}

/// Progressive counters (sent at most every 50 ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProgress {
    /// Files read so far.
    pub files_searched: u64,
    /// Matches found so far.
    pub matches: u64,
    /// The file currently being searched, if any.
    pub current: Option<PathBuf>,
}

/// Why a search stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    /// The walk finished.
    Completed,
    /// [`SearchHandle::cancel`] (or a pre-cancelled token) stopped it.
    Cancelled,
    /// [`SearchRequest::max_results`] was reached.
    LimitReached,
    /// The worker thread panicked (see [`SearchDone::errors`]).
    Failed,
}

/// The final event of a search.
#[derive(Debug, Clone)]
pub struct SearchDone {
    /// How it ended.
    pub outcome: SearchOutcome,
    /// Files read.
    pub files_searched: u64,
    /// Matches found (capped by `max_results`).
    pub matches: u64,
    /// The result cap was reached.
    pub truncated: bool,
    /// Wall-clock time.
    pub elapsed: Duration,
    /// Walk/read errors.
    pub errors: Vec<SearchIssue>,
}

/// Streamed by [`SearchHandle`].
#[derive(Debug)]
pub enum SearchEvent {
    /// A file with at least one match is starting; its [`SearchEvent::Match`]s
    /// follow.
    FileStart {
        /// The file.
        file: FileId,
        /// Its path.
        path: PathBuf,
    },
    /// One match. `matched.line`, `column`, `length` and `line_text` are the
    /// display values; `matched.byte_range` is the replace value.
    Match {
        /// The file.
        file: FileId,
        /// Its path.
        path: PathBuf,
        /// The match itself.
        matched: ProjectMatch,
    },
    /// Counters, for the progress row / status label.
    Progress(SearchProgress),
    /// Last event; the channel closes afterwards.
    Done(SearchDone),
}

impl SearchEvent {
    /// The match carried by a [`SearchEvent::Match`].
    pub fn matched(&self) -> Option<&ProjectMatch> {
        match self {
            SearchEvent::Match { matched, .. } => Some(matched),
            _ => None,
        }
    }
}

/// Everything a synchronous search found.
#[derive(Debug, Clone)]
pub struct SearchResults {
    /// Files with matches, sorted by path.
    pub files: Vec<SearchFile>,
    /// All matches, sorted by file then byte offset.
    pub matches: Vec<ProjectMatch>,
    /// How the search ended.
    pub outcome: SearchOutcome,
    /// Wall-clock time.
    pub elapsed: Duration,
    /// Walk/read errors (also embedded in [`SearchDone`] for streaming callers).
    pub errors: Vec<SearchIssue>,
    /// The result cap was reached.
    pub truncated: bool,
    /// The request's mode (the replace plan needs it).
    pub mode: SearchMode,
    /// The request's fallback encoding (replace re-decodes with it).
    pub fallback: &'static Encoding,
}

impl SearchResults {
    /// The file record for `id`.
    pub fn file(&self, id: FileId) -> Option<&SearchFile> {
        self.files.iter().find(|f| f.id == id)
    }

    /// The matches of one file, in order.
    pub fn matches_of(&self, file: FileId) -> impl Iterator<Item = &ProjectMatch> {
        self.matches.iter().filter(move |m| m.file == file)
    }

    /// Number of matches (equals `matches.len()`).
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Whether nothing matched.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Nothing found.
    pub fn empty(mode: SearchMode, fallback: &'static Encoding) -> Self {
        Self {
            files: Vec::new(),
            matches: Vec::new(),
            outcome: SearchOutcome::Completed,
            elapsed: Duration::ZERO,
            errors: Vec::new(),
            truncated: false,
            mode,
            fallback,
        }
    }
}

/// Handle of an asynchronous search: a channel of [`SearchEvent`]s plus a
/// cancellation token and the worker's final result.
pub struct SearchHandle {
    events: Receiver<SearchEvent>,
    cancel: CancelToken,
    join: Option<JoinHandle<SearchResults>>,
}

impl SearchHandle {
    /// Requests cancellation. The worker stops at the next file/match and the
    /// final event reports [`SearchOutcome::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The cancellation token (clone it into UI state).
    pub fn token(&self) -> &CancelToken {
        &self.cancel
    }

    /// The raw event channel.
    pub fn events(&self) -> &Receiver<SearchEvent> {
        &self.events
    }

    /// Blocks for the next event; `None` once the channel closed.
    pub fn recv(&self) -> Option<SearchEvent> {
        self.events.recv().ok()
    }

    /// Blocks up to `timeout` for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<SearchEvent, mpsc::RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Drains up to `max` events. Waits for the first, then keeps taking
    /// events that are already available (or arrive within `wait` between
    /// two events). This is the UI's result batching primitive: a 60 Hz
    /// run-loop can ask for a batch per tick.
    pub fn recv_batch(&self, max: usize, wait: Duration) -> Vec<SearchEvent> {
        let mut out = Vec::new();
        if max == 0 {
            return out;
        }
        match self.events.recv() {
            Ok(e) => out.push(e),
            Err(_) => return out,
        }
        let deadline = Instant::now() + wait;
        while out.len() < max {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self.events.recv_timeout(deadline - now) {
                Ok(e) => out.push(e),
                Err(_) => break,
            }
        }
        out
    }

    /// Waits for the worker and returns the collected results. Cancels first
    /// when the caller never finished the stream, so this cannot hang.
    pub fn join(mut self) -> SearchResults {
        if let Some(handle) = self.join.take() {
            return join_worker(handle);
        }
        SearchResults::empty(SearchMode::ContentsAndFilenames, default_fallback())
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.join.take() {
            // The channel is unbounded, so the worker cannot block on sends;
            // joining here makes cancellation deterministic when the UI drops
            // the handle.
            let _ = handle.join();
        }
    }
}

fn join_worker(handle: JoinHandle<SearchResults>) -> SearchResults {
    match handle.join() {
        Ok(results) => results,
        Err(_) => {
            let mut empty =
                SearchResults::empty(SearchMode::ContentsAndFilenames, default_fallback());
            empty.outcome = SearchOutcome::Failed;
            empty
                .errors
                .push(SearchIssue::new(None, "the search worker panicked"));
            empty
        }
    }
}

/// Runs a search synchronously (no event stream, no cancellation).
pub fn search_project(request: SearchRequest) -> SearchResults {
    run(request, CancelToken::new(), None)
}

/// Runs a search synchronously with an externally owned cancellation token.
/// A pre-cancelled token returns immediately with
/// [`SearchOutcome::Cancelled`].
pub fn search_project_with(request: SearchRequest, cancel: CancelToken) -> SearchResults {
    run(request, cancel, None)
}

/// Starts a search on worker threads. The returned handle streams events and
/// ends with [`SearchEvent::Done`].
pub fn spawn_search(request: SearchRequest) -> SearchHandle {
    spawn_search_with(request, CancelToken::new())
}

/// Like [`spawn_search`] with an externally owned cancellation token.
pub fn spawn_search_with(request: SearchRequest, cancel: CancelToken) -> SearchHandle {
    let (tx, rx) = mpsc::channel();
    let token = cancel.clone();
    let builder = thread::Builder::new().name("scratch-search".into());
    let handle = match builder.spawn(move || run(request, token, Some(tx))) {
        Ok(handle) => handle,
        Err(_) => {
            // Could not spawn a thread: report a failed, empty result.
            let (tx, rx) = mpsc::channel();
            let done = SearchDone {
                outcome: SearchOutcome::Failed,
                files_searched: 0,
                matches: 0,
                truncated: false,
                elapsed: Duration::ZERO,
                errors: vec![SearchIssue::new(None, "could not start the search thread")],
            };
            let _ = tx.send(SearchEvent::Done(done));
            drop(tx);
            let mut empty =
                SearchResults::empty(SearchMode::ContentsAndFilenames, default_fallback());
            empty.outcome = SearchOutcome::Failed;
            return SearchHandle {
                events: rx,
                cancel,
                join: Some(thread::spawn(move || empty)),
            };
        }
    };
    SearchHandle {
        events: rx,
        cancel,
        join: Some(handle),
    }
}

#[derive(Debug, Default)]
struct ResultsBuilder {
    files: Vec<SearchFile>,
    matches: Vec<ProjectMatch>,
    errors: Vec<SearchIssue>,
}

struct Shared {
    matcher: Matcher,
    mode: SearchMode,
    options: SearchOptions,
    include_extensions: HashSet<String>,
    exclude_extensions: HashSet<String>,
    excluded_directories: HashSet<String>,
    cancel: CancelToken,
    max_results: usize,
    threads: usize,
    tx: Option<Sender<SearchEvent>>,
    files_done: AtomicU64,
    matches_found: AtomicU64,
    limit_reached: AtomicBool,
    last_progress: Mutex<Instant>,
    builder: Mutex<ResultsBuilder>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.cancel.is_cancelled() || self.limit_reached.load(Ordering::Relaxed)
    }

    fn emit(&self, event: SearchEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(event);
        }
    }

    fn issue(&self, path: Option<&Path>, message: impl Into<String>) {
        if let Ok(mut builder) = self.builder.lock() {
            builder.errors.push(SearchIssue::new(path, message));
        }
    }

    fn wants_extension(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !self.include_extensions.is_empty() && !self.include_extensions.contains(&ext) {
            return false;
        }
        !self.exclude_extensions.contains(&ext)
    }

    fn progress(&self, current: &Path) {
        let Ok(mut last) = self.last_progress.lock() else {
            return;
        };
        if last.elapsed() < Duration::from_millis(50) {
            return;
        }
        *last = Instant::now();
        drop(last);
        self.emit(SearchEvent::Progress(SearchProgress {
            files_searched: self.files_done.load(Ordering::Relaxed),
            matches: self.matches_found.load(Ordering::Relaxed),
            current: Some(current.to_path_buf()),
        }));
    }

    /// Reserves one match slot. `false` means the cap was reached.
    fn reserve_match(&self) -> bool {
        let n = self.matches_found.fetch_add(1, Ordering::Relaxed);
        if n >= self.max_results as u64 {
            self.matches_found
                .store(self.max_results as u64, Ordering::Relaxed);
            self.limit_reached.store(true, Ordering::Relaxed);
            false
        } else {
            true
        }
    }

    fn visit(&self, entry: Result<DirEntry, ignore::Error>) -> WalkState {
        if self.stopped() {
            return WalkState::Quit;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let path = error_path(&err).map(Path::to_path_buf);
                self.issue(path.as_deref(), err.to_string());
                return WalkState::Continue;
            }
        };
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            // Folders can match in `Filenames` mode (a replace renames them),
            // but the searched root itself is never a match.
            if entry.depth() > 0 && self.mode.searches_filenames() {
                self.search_directory_name(entry.path());
            }
            return if self.stopped() {
                WalkState::Quit
            } else {
                WalkState::Continue
            };
        }
        let path = entry.path();
        if !self.wants_extension(path) {
            return WalkState::Continue;
        }
        self.search_disk_file(path);
        if self.stopped() {
            WalkState::Quit
        } else {
            WalkState::Continue
        }
    }

    fn search_directory_name(&self, path: &Path) {
        let id = FileId::of_path(path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let file = SearchFile {
            id,
            path: path.to_path_buf(),
            name,
            snapshot: None,
            encoding: None,
            crlf: false,
            lossy: false,
            binary: false,
            is_dir: true,
            size: 0,
            text_hash: 0,
            byte_hash: 0,
        };
        self.search_text(file, "");
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.progress(path);
    }

    fn search_disk_file(&self, path: &Path) {
        let meta = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                self.issue(Some(path), err.to_string());
                return;
            }
        };
        if !meta.is_file() || meta.len() > self.options.max_file_size {
            return;
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.issue(Some(path), err.to_string());
                return;
            }
        };
        let binary = looks_binary(&bytes);
        if binary && !self.options.binary {
            return;
        }
        let decoded = decode(&bytes, self.options.fallback_encoding);
        let text = decoded.text;
        let id = FileId::of_path(path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let file = SearchFile {
            id,
            path: path.to_path_buf(),
            name,
            snapshot: None,
            encoding: Some(decoded.encoding),
            crlf: text.contains("\r\n"),
            lossy: decoded.lossy,
            binary,
            is_dir: false,
            size: meta.len(),
            text_hash: content_hash(text.as_bytes()),
            byte_hash: content_hash(&bytes),
        };
        self.search_text(file, &text);
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.progress(path);
    }

    fn search_snapshot(&self, index: usize, doc: &DocumentSnapshot) {
        if self.stopped() {
            return;
        }
        let id = FileId::of_snapshot(&doc.name, index);
        let text: Arc<str> = Arc::from(doc.text.as_str());
        let file = SearchFile {
            id,
            path: doc.path.clone().unwrap_or_else(|| PathBuf::from(&doc.name)),
            name: doc.name.clone(),
            snapshot: Some(text.clone()),
            encoding: None,
            crlf: text.contains("\r\n"),
            lossy: false,
            binary: false,
            is_dir: false,
            size: text.len() as u64,
            text_hash: content_hash(text.as_bytes()),
            byte_hash: content_hash(text.as_bytes()),
        };
        self.search_text(file, &text);
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.progress(&doc.path.clone().unwrap_or_else(|| PathBuf::from(&doc.name)));
    }

    fn search_text(&self, file: SearchFile, text: &str) {
        let mut found: Vec<ProjectMatch> = Vec::new();
        let mut next_index: u32 = 0;

        if self.mode.searches_filenames() {
            let matcher = &self.matcher;
            match matcher.find_all(&file.name) {
                Ok(ranges) => {
                    for range in ranges {
                        if self.stopped() || !self.reserve_match() {
                            break;
                        }
                        let line = &file.name[..range.start.min(file.name.len())];
                        let column = utf16_len(line) as u32;
                        let length = utf16_len(&file.name[range.clone()]) as u32;
                        let matched = ProjectMatch {
                            id: MatchId {
                                file: file.id,
                                index: next_index,
                            },
                            file: file.id,
                            kind: MatchKind::FileName,
                            line: 0,
                            column,
                            length,
                            byte_range: range,
                            line_text: file.name.clone(),
                            name: file.name.clone(),
                        };
                        next_index += 1;
                        found.push(matched);
                    }
                }
                Err(err) => self.issue(Some(&file.path), match_error_message(&err)),
            }
        }

        if self.mode.searches_contents() && !self.stopped() && !file.is_dir {
            let index = LineIndex::new(text);
            let matcher = self.matcher.with_crlf(file.crlf);
            for result in matcher.iter(text) {
                if self.stopped() {
                    break;
                }
                let range = match result {
                    Ok(range) => range,
                    Err(err) => {
                        self.issue(Some(&file.path), match_error_message(&err));
                        break;
                    }
                };
                if let Some(filter) = &self.options.scope_filter
                    && !filter(range.start)
                {
                    continue;
                }
                if !self.reserve_match() {
                    break;
                }
                let line = index.line_of(range.start);
                let line_range = index.line_range(text, line);
                let line_text = text[line_range.clone()].to_string();
                let column =
                    byte_to_utf16(&line_text, range.start.saturating_sub(line_range.start));
                let length = utf16_len(&text[range.clone()]) as u32;
                let matched = ProjectMatch {
                    id: MatchId {
                        file: file.id,
                        index: next_index,
                    },
                    file: file.id,
                    kind: MatchKind::Content,
                    line: line as u32,
                    column: column as u32,
                    length,
                    line_text,
                    name: file.name.clone(),
                    byte_range: range,
                };
                next_index += 1;
                found.push(matched);
            }
        }

        if found.is_empty() {
            return;
        }
        let file_id = file.id;
        let path = file.path.clone();
        if let Ok(mut builder) = self.builder.lock() {
            builder.files.push(file);
            builder.matches.extend(found.iter().cloned());
        }
        self.emit(SearchEvent::FileStart {
            file: file_id,
            path: path.clone(),
        });
        for matched in found {
            self.emit(SearchEvent::Match {
                file: file_id,
                path: path.clone(),
                matched,
            });
        }
    }
}

fn match_error_message(err: &MatchError) -> String {
    err.to_string()
}

fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        _ => None,
    }
}

fn normalized_extensions(list: &[String]) -> HashSet<String> {
    list.iter()
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

fn run(
    request: SearchRequest,
    cancel: CancelToken,
    tx: Option<Sender<SearchEvent>>,
) -> SearchResults {
    let started = Instant::now();
    let SearchRequest {
        source,
        mode,
        options,
        matcher,
        max_results,
        threads,
    } = request;
    let fallback = options.fallback_encoding;
    let max_results = max_results.unwrap_or(usize::MAX);
    let default_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let threads = threads.unwrap_or(default_threads).clamp(1, 8);
    let include_extensions = normalized_extensions(&options.include_extensions);
    let exclude_extensions = normalized_extensions(&options.exclude_extensions);
    let excluded_directories = options
        .excluded_directories
        .iter()
        .map(|d| d.trim().trim_end_matches('/').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let shared = Arc::new(Shared {
        matcher,
        mode,
        options,
        include_extensions,
        exclude_extensions,
        excluded_directories,
        cancel: cancel.clone(),
        max_results,
        threads,
        tx: tx.clone(),
        files_done: AtomicU64::new(0),
        matches_found: AtomicU64::new(0),
        limit_reached: AtomicBool::new(false),
        last_progress: Mutex::new(Instant::now() - Duration::from_secs(1)),
        builder: Mutex::new(ResultsBuilder::default()),
    });

    if !cancel.is_cancelled() {
        match &source {
            SearchSource::OpenDocuments(docs) => {
                for (i, doc) in docs.iter().enumerate() {
                    if shared.stopped() {
                        break;
                    }
                    shared.search_snapshot(i, doc);
                }
            }
            source => walk(source.roots(), &shared),
        }
    }

    let outcome = if cancel.is_cancelled() {
        SearchOutcome::Cancelled
    } else if shared.limit_reached.load(Ordering::Relaxed) {
        SearchOutcome::LimitReached
    } else {
        SearchOutcome::Completed
    };

    let files_searched = shared.files_done.load(Ordering::Relaxed);
    let (mut files, mut matches, errors) = match Arc::try_unwrap(shared) {
        Ok(shared) => {
            let builder = shared.builder.into_inner().unwrap_or_default();
            (builder.files, builder.matches, builder.errors)
        }
        Err(shared) => {
            let builder = shared
                .builder
                .lock()
                .map(|mut b| std::mem::take(&mut *b))
                .unwrap_or_default();
            (builder.files, builder.matches, builder.errors)
        }
    };
    files.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
    matches.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.kind.cmp(&b.kind))
            .then(a.byte_range.start.cmp(&b.byte_range.start))
            .then(a.byte_range.end.cmp(&b.byte_range.end))
    });
    let elapsed = started.elapsed();
    let truncated = matches.len() >= max_results && max_results != usize::MAX;
    let done = SearchDone {
        outcome,
        files_searched,
        matches: matches.len() as u64,
        truncated,
        elapsed,
        errors: errors.clone(),
    };
    if let Some(tx) = &tx {
        let _ = tx.send(SearchEvent::Done(done));
    }
    SearchResults {
        files,
        matches,
        outcome,
        elapsed,
        errors,
        truncated,
        mode,
        fallback,
    }
}

fn walk(roots: Vec<PathBuf>, shared: &Arc<Shared>) {
    let Some(first) = roots.first() else { return };
    let mut builder = WalkBuilder::new(first);
    for root in &roots[1..] {
        builder.add(root);
    }
    builder
        .standard_filters(false)
        .hidden(!shared.options.hidden)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .max_depth(if shared.options.subdirectories {
            None
        } else {
            Some(1)
        });

    if !shared.excluded_directories.is_empty() {
        let excluded = shared.excluded_directories.clone();
        builder.filter_entry(move |entry| {
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                !excluded.contains(&name)
            } else {
                true
            }
        });
    }

    builder.threads(shared.threads);
    let shared = shared.clone();
    builder.build_parallel().run(|| {
        let shared = shared.clone();
        Box::new(move |entry| shared.visit(entry))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::{MatchMode, Matcher};

    #[test]
    fn ids_are_stable() {
        let a = FileId::of_path(Path::new("/tmp/a.rs"));
        let b = FileId::of_path(Path::new("/tmp/a.rs"));
        let c = FileId::of_path(Path::new("/tmp/b.rs"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        let s1 = FileId::of_snapshot("Untitled", 0);
        let s2 = FileId::of_snapshot("Untitled", 1);
        assert_ne!(s1, s2);
    }

    #[test]
    fn modes() {
        assert!(SearchMode::ContentsAndFilenames.searches_contents());
        assert!(SearchMode::ContentsAndFilenames.searches_filenames());
        assert!(SearchMode::Filenames.searches_filenames());
        assert!(!SearchMode::Filenames.searches_contents());
        assert_eq!(SearchMode::Contents.label(), "File Contents");
    }

    #[test]
    fn search_snapshot_document() {
        let matcher = Matcher::simple("needle", MatchMode::Text, true).unwrap();
        let mut request = SearchRequest::new(
            SearchSource::OpenDocuments(vec![DocumentSnapshot::new(
                "Untitled 1",
                None,
                "first\nneedle here\n",
            )]),
            matcher,
        );
        request.mode = SearchMode::Contents;
        let results = search_project(request);
        assert_eq!(results.matches.len(), 1);
        let m = &results.matches[0];
        assert_eq!(m.line, 1);
        assert_eq!(m.column, 0);
        assert_eq!(m.line_text, "needle here");
        assert!(results.files[0].snapshot.is_some());
        assert_eq!(results.outcome, SearchOutcome::Completed);
    }

    #[test]
    fn pre_cancelled_search_is_cancelled() {
        let matcher = Matcher::simple("x", MatchMode::Text, true).unwrap();
        let token = CancelToken::new();
        token.cancel();
        let request = SearchRequest::new(SearchSource::CustomPaths(vec![]), matcher);
        let results = search_project_with(request, token);
        assert_eq!(results.outcome, SearchOutcome::Cancelled);
        assert!(results.matches.is_empty());
    }

    #[test]
    fn fallback_encoding_by_name() {
        let options = SearchOptions::default()
            .with_fallback_name("shift_jis")
            .expect("known");
        assert_eq!(options.fallback_encoding.name(), "Shift_JIS");
        assert!(
            SearchOptions::default()
                .with_fallback_name("nope")
                .is_none()
        );
    }
}
