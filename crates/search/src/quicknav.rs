//! QuickNav ("Open Quickly", ⇧⌘O) provider logic.
//!
//! These are pure functions over data the UI supplies: open-document names,
//! recent paths, the current document's symbols, the language list, the menu
//! command list, and an optional [`FileCache`] of files under the file
//! navigator's root. [`quicknav`] aggregates them into one ranked list of
//! [`QuickNavItem`]s.
//!
//! Providers mirror CodeRunner's registered `SearchManager` providers
//! (report 10 section 1.5b):
//!
//! | Provider | Input | Rows |
//! |--|--|--|
//! | `absoluteFile` | query starting `/`, `~` or `file://` | files/dirs, tab-completable |
//! | `line` | `line 12` / `l12` / `12` | "Go to Line 12" |
//! | `symbols` | current document symbols | symbol names |
//! | `files` | [`FileCache`] under the navigator root | files |
//! | `openDocument` | open documents | document names |
//! | `recentDocuments` | recent paths | files |
//! | `languages` | language list | `Set Language: X` / `New Document: X` |
//! | `mainMenu` | menu command list | command titles with shortcuts |
//!
//! Scoring is [`crate::fuzzy`] (subsequence with camelCase/path boundary
//! bonuses); each provider adds a base score, ties are stable in provider
//! order, and results are deduplicated by a stable id. Every loop checks a
//! [`CancelToken`] so a fast typist never waits for a stale query.

use std::collections::HashSet;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::fuzzy::{Fuzzy, FuzzyMatch, rank_cancellable};
use crate::symbols::{Symbol, SymbolKind};

/// What a QuickNav row is (drives the section/ordering and the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum QuickNavKind {
    /// A line number (`line 12`, `l12`, `12`).
    Line,
    /// An absolute or `~` path completion.
    AbsolutePath,
    /// A symbol of the current document.
    Symbol,
    /// An open document.
    OpenDocument,
    /// A recent document.
    RecentDocument,
    /// A file under the file navigator root.
    File,
    /// "Set Language: X".
    Language,
    /// "New Document: X".
    NewDocument,
    /// A menu command.
    MenuCommand,
}

impl QuickNavKind {
    /// Tie-break priority (smaller first) when scores are equal.
    pub fn priority(self) -> u8 {
        match self {
            QuickNavKind::Line => 0,
            QuickNavKind::AbsolutePath => 1,
            QuickNavKind::Symbol => 2,
            QuickNavKind::OpenDocument => 3,
            QuickNavKind::RecentDocument => 4,
            QuickNavKind::File => 5,
            QuickNavKind::Language => 6,
            QuickNavKind::NewDocument => 7,
            QuickNavKind::MenuCommand => 8,
        }
    }

    /// The CodeRunner provider key (useful for persistence and tests).
    pub fn provider(self) -> &'static str {
        match self {
            QuickNavKind::Line => "line",
            QuickNavKind::AbsolutePath => "absoluteFile",
            QuickNavKind::Symbol => "symbols",
            QuickNavKind::OpenDocument => "openDocument",
            QuickNavKind::RecentDocument => "recentDocuments",
            QuickNavKind::File => "files",
            QuickNavKind::Language | QuickNavKind::NewDocument => "languages",
            QuickNavKind::MenuCommand => "mainMenu",
        }
    }
}

/// The icon a QuickNav row draws (36x36 in CodeRunner's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IconKind {
    /// Generic file / document kind icon.
    File,
    /// Folder icon.
    Folder,
    /// An open document.
    Document,
    /// A recent document (clock).
    Recent,
    /// A line-number arrow.
    Line,
    /// A language icon.
    Language,
    /// A new-document icon.
    NewDocument,
    /// A menu command.
    MenuCommand,
    /// A symbol with its kind badge.
    Symbol(SymbolKind),
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuickNavAction {
    /// Open a file/URL path (files and absolute-path rows).
    OpenPath(PathBuf),
    /// Select an already-open document (by path when it has one).
    OpenDocument {
        /// Display name.
        name: String,
        /// Path, when saved.
        path: Option<PathBuf>,
    },
    /// Move the caret to a line (0-based) and UTF-16 column.
    GoToLine {
        /// 0-based line.
        line: u32,
        /// 0-based UTF-16 column.
        column: u32,
    },
    /// Move the caret to a symbol's line (0-based) and UTF-16 column.
    GoToSymbol {
        /// The symbol name.
        name: String,
        /// 0-based line.
        line: u32,
        /// 0-based UTF-16 column.
        column: u32,
    },
    /// Set the current document's language (`Set Language: X`).
    SetLanguage(String),
    /// Create a new untitled document in a language (`New Document: X`).
    NewDocument(String),
    /// Run a menu command (identified by its menu path / command name).
    MenuCommand(String),
}

/// One QuickNav row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickNavItem {
    /// Which provider produced it.
    pub kind: QuickNavKind,
    /// Main text (bold; highlighted with `positions`).
    pub title: String,
    /// Grey second line (path, description, shortcut).
    pub subtitle: String,
    /// The icon to draw.
    pub icon_kind: IconKind,
    /// Higher is better; only comparable within one query.
    pub score: i32,
    /// Byte ranges **of `title`** to bold/highlight.
    pub positions: Vec<Range<usize>>,
    /// What choosing the row does.
    pub action: QuickNavAction,
    /// Stable identity (CodeRunner remembers the last selected result per
    /// query by identifier).
    pub id: String,
}

/// Which QuickNav palette is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum QuickNavMode {
    /// File > Open Quickly (all providers).
    #[default]
    OpenQuickly,
    /// Text > Syntax Mode > Select Language (only `Set Language:` rows).
    SelectLanguage,
    /// File > New from Template > Other Languages (only `New Document:`).
    NewDocument,
}

/// An open document supplied to the `openDocument` provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDocumentItem {
    /// Display name ("main.rs", "Untitled 2").
    pub name: String,
    /// Path when the document is saved.
    pub path: Option<PathBuf>,
}

/// A language supplied to the `languages` provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageChoice {
    /// Display name ("Python", "C# (.NET)").
    pub name: String,
    /// Language id / uuid the action returns.
    pub id: String,
}

/// A menu command supplied to the `mainMenu` provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuCommandItem {
    /// Menu title ("Find Next", "Run").
    pub title: String,
    /// Menu path or command selector ("MainMenu/Find/Find Next").
    pub command: String,
    /// Human-readable shortcut ("⌘G", "⇧⌘O").
    pub shortcut: Option<String>,
}

/// A parsed `line 12` / `l12` / `12` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSpec {
    /// 0-based line, ready for the editor.
    pub line: u32,
    /// 0-based UTF-16 column (`12:5` -> 4).
    pub column: u32,
}

impl LineSpec {
    /// 1-based line as the user typed it.
    pub fn display_line(self) -> u32 {
        self.line + 1
    }

    /// 1-based column as the user typed it.
    pub fn display_column(self) -> u32 {
        self.column + 1
    }
}

/// Parses QuickNav's line provider query (`line 12`, `l12`, `12`,
/// `12:5`). Case-insensitive; returns 0-based positions.
pub fn parse_line_spec(query: &str) -> Option<LineSpec> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    let first_digit = q.find(|c: char| c.is_ascii_digit())?;
    let (prefix, rest) = q.split_at(first_digit);
    let prefix = prefix.trim().to_ascii_lowercase();
    if !prefix.is_empty() && !matches!(prefix.as_str(), "l" | "li" | "lin" | "line") {
        return None;
    }
    let (line_part, column_part) = match rest.split_once(':') {
        Some((line, column)) => (line, Some(column)),
        None => (rest, None),
    };
    if line_part.is_empty() || !line_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let line: u32 = line_part.parse().ok()?;
    if line == 0 {
        return None;
    }
    let column = match column_part {
        None | Some("") => 0,
        Some(column) => {
            if !column.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let one_based: u32 = column.parse().ok()?;
            one_based.checked_sub(1)?
        }
    };
    Some(LineSpec {
        line: line - 1,
        column,
    })
}

/// The `line` provider.
pub fn line_items(query: &str) -> Vec<QuickNavItem> {
    let Some(spec) = parse_line_spec(query) else {
        return Vec::new();
    };
    let title = if spec.column > 0 {
        format!(
            "Go to Line {}, Column {}",
            spec.display_line(),
            spec.display_column()
        )
    } else {
        format!("Go to Line {}", spec.display_line())
    };
    let subtitle = "Go to line in the current document".to_string();
    let mut fuzzy = Fuzzy::new(query.trim());
    let (score, positions) = match fuzzy.score(&title) {
        Some(m) => (1000 + m.score, m.ranges(&title)),
        None => (1000, Vec::new()),
    };
    vec![QuickNavItem {
        kind: QuickNavKind::Line,
        title,
        subtitle,
        icon_kind: IconKind::Line,
        score,
        positions,
        action: QuickNavAction::GoToLine {
            line: spec.line,
            column: spec.column,
        },
        id: format!("line:{}:{}", spec.line, spec.column),
    }]
}

/// One absolute-path completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCompletion {
    /// The filesystem path (unexpanded, absolute).
    pub path: PathBuf,
    /// The text to put back into the field (keeps `~` / `file://` style).
    pub display: String,
    /// Whether it is a directory (display gets a trailing `/`).
    pub is_dir: bool,
}

/// Completes an absolute or `~` path. Empty for non-path queries.
pub fn complete_paths(query: &str) -> Vec<PathCompletion> {
    complete_paths_cancellable(query, &CancelToken::new())
}

/// [`complete_paths`] with cancellation (large directories).
pub fn complete_paths_cancellable(query: &str, cancel: &CancelToken) -> Vec<PathCompletion> {
    let trimmed = query.trim_start();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let (raw, file_scheme) = match trimmed.strip_prefix("file://") {
        Some(rest) => (rest, true),
        None => (trimmed, false),
    };
    let (tilde, rest) = if raw == "~" {
        (true, "")
    } else if let Some(rest) = raw.strip_prefix("~/") {
        (true, rest)
    } else if raw.starts_with('~') {
        // `~user` is not supported.
        return Vec::new();
    } else {
        (false, raw)
    };
    if !tilde && !raw.starts_with('/') {
        return Vec::new();
    }
    let home = if tilde { home_dir() } else { None };
    let full = if tilde {
        match &home {
            Some(home) => home.join(rest),
            None => return Vec::new(),
        }
    } else {
        PathBuf::from(rest)
    };
    let ends_slash = raw.ends_with('/') || raw.is_empty() || raw == "~";
    let (dir, prefix) = if raw == "~" {
        match &home {
            Some(home) => (home.clone(), String::new()),
            None => return Vec::new(),
        }
    } else if ends_slash {
        (full.clone(), String::new())
    } else {
        let parent = full
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let prefix = full
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (parent, prefix)
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    let prefix_lower = prefix.to_ascii_lowercase();
    let dir_display = if tilde {
        match &home {
            Some(home) => tilde_display(&dir, home),
            None => return Vec::new(),
        }
    } else {
        dir.display().to_string()
    };
    let mut out: Vec<PathCompletion> = Vec::new();
    let reader = match fs::read_dir(&dir) {
        Ok(reader) => reader,
        Err(_) => return Vec::new(),
    };
    for (i, entry) in reader.enumerate() {
        if i % 64 == 0 && cancel.is_cancelled() {
            break;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !prefix_lower.is_empty() && !name.to_ascii_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let is_dir = meta.is_dir();
        let mut display = join_display(&dir_display, &name);
        if is_dir {
            display.push('/');
        }
        let mut path = dir.join(&name);
        if file_scheme {
            // The caller asked for `file://` URLs; keep the scheme.
            let mut url = String::from("file://");
            url.push_str(&display);
            display = url;
            path = fs::canonicalize(&path).unwrap_or(path);
        }
        out.push(PathCompletion {
            path,
            display,
            is_dir,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(
                a.display
                    .to_ascii_lowercase()
                    .cmp(&b.display.to_ascii_lowercase()),
            )
            .then(a.display.cmp(&b.display))
    });
    out
}

/// Tab-completes `query`: the single completion, or the longest common
/// prefix of several. `None` when there is nothing to add.
pub fn tab_complete_path(query: &str) -> Option<String> {
    let completions = complete_paths(query);
    if completions.is_empty() {
        return None;
    }
    if completions.len() == 1 {
        return Some(completions[0].display.clone());
    }
    let first = &completions[0].display;
    let mut prefix_len = first.len();
    for completion in &completions[1..] {
        prefix_len = common_prefix_len(first, &completion.display, prefix_len);
        if prefix_len == 0 {
            return None;
        }
    }
    if prefix_len <= query.len() {
        return None;
    }
    let mut end = prefix_len;
    while end > 0 && !first.is_char_boundary(end) {
        end -= 1;
    }
    (end > query.len()).then(|| first[..end].to_string())
}

/// The `absoluteFile` provider.
pub fn path_items(query: &str, limit: usize, cancel: &CancelToken) -> Vec<QuickNavItem> {
    let trimmed = query.trim_start();
    if !(trimmed.starts_with('/') || trimmed.starts_with('~') || trimmed.starts_with("file://")) {
        return Vec::new();
    }
    let completions = complete_paths_cancellable(trimmed, cancel);
    if completions.is_empty() {
        return Vec::new();
    }
    let mut fuzzy = Fuzzy::new(trimmed);
    let mut out: Vec<(usize, QuickNavItem)> = Vec::new();
    for (index, completion) in completions.into_iter().enumerate() {
        if index % 256 == 0 && cancel.is_cancelled() {
            break;
        }
        let Some(matched) = fuzzy.score(&completion.display) else {
            continue;
        };
        let title = if completion.is_dir {
            completion
                .display
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&completion.display)
                .to_string()
        } else {
            completion
                .display
                .rsplit('/')
                .next()
                .unwrap_or(&completion.display)
                .to_string()
        };
        if title.is_empty() {
            continue;
        }
        let trailing = if completion.is_dir || completion.display.ends_with('/') {
            1
        } else {
            0
        };
        let positions = suffix_ranges(&completion.display, &title, trailing, &matched);
        let subtitle = parent_display(&completion.display);
        out.push((
            index,
            QuickNavItem {
                kind: QuickNavKind::AbsolutePath,
                title,
                subtitle,
                icon_kind: if completion.is_dir {
                    IconKind::Folder
                } else {
                    IconKind::File
                },
                score: 500 + matched.score,
                positions,
                action: QuickNavAction::OpenPath(completion.path),
                id: format!("path:{}", completion.display),
            },
        ));
    }
    out.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    out.into_iter().take(limit).map(|(_, item)| item).collect()
}

// ---------------------------------------------------------------- FileCache

/// Options of the cached navigator walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCacheOptions {
    /// Include dotfiles (the file navigator's "Show Hidden Files").
    pub include_hidden: bool,
    /// Stop after this many files (protects memory on huge trees).
    pub max_files: usize,
    /// Directory names pruned from the walk.
    pub excluded_directories: Vec<String>,
}

impl Default for FileCacheOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            max_files: 100_000,
            excluded_directories: Vec::new(),
        }
    }
}

/// One cached file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the cache root, with `/` separators.
    pub rel: String,
    /// Last path component.
    pub name: String,
    /// Relative parent (`"src"`, `"."` for root files).
    pub parent: String,
}

/// A completed walk, shared by all searches until the cache is invalidated.
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    /// The generation this snapshot was walked at.
    pub generation: u64,
    /// The root it was walked from.
    pub root: PathBuf,
    /// Files, sorted by relative path.
    pub entries: Vec<FileEntry>,
    /// The `max_files` cap was hit.
    pub truncated: bool,
    /// Walk errors (unreadable directories and the like).
    pub errors: usize,
}

/// Cached walk of the file navigator's root, with a watcher invalidation
/// hook.
///
/// The UI constructs one cache per navigator root and calls
/// [`invalidate`](Self::invalidate) from its filesystem watcher; the next
/// [`snapshot`](Self::snapshot) re-walks lazily. [`generation`](Self::generation)
/// increments on every invalidation so a UI can skip re-rendering when
/// nothing changed.
pub struct FileCache {
    root: RwLock<PathBuf>,
    options: FileCacheOptions,
    state: RwLock<Option<Arc<FileSnapshot>>>,
    generation: AtomicU64,
}

impl std::fmt::Debug for FileCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCache")
            .field("root", &read_lock(&self.root).clone())
            .field("options", &self.options)
            .field("generation", &self.generation.load(Ordering::Relaxed))
            .finish()
    }
}

impl FileCache {
    /// A cache for `root` with default options.
    pub fn new(root: PathBuf) -> Self {
        Self::with_options(root, FileCacheOptions::default())
    }

    /// A cache with explicit options.
    pub fn with_options(root: PathBuf, options: FileCacheOptions) -> Self {
        Self {
            root: RwLock::new(root),
            options,
            state: RwLock::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// The current root.
    pub fn root(&self) -> PathBuf {
        read_lock(&self.root).clone()
    }

    /// Changes the root and invalidates the cache.
    pub fn set_root(&self, root: PathBuf) {
        *write_lock(&self.root) = root;
        self.invalidate();
    }

    /// The options in force.
    pub fn options(&self) -> &FileCacheOptions {
        &self.options
    }

    /// Drops the cached walk; called by the navigator's FSEvents watcher.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        *write_lock(&self.state) = None;
    }

    /// Changes since construction (also bumped by [`invalidate`](Self::invalidate)).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The (possibly cached) walk, walking first if needed.
    pub fn snapshot(&self) -> Arc<FileSnapshot> {
        if let Some(snapshot) = read_lock(&self.state).as_ref() {
            return snapshot.clone();
        }
        let mut slot = write_lock(&self.state);
        if let Some(snapshot) = slot.as_ref() {
            return snapshot.clone();
        }
        let root = self.root();
        let snapshot = Arc::new(self.walk(&root));
        *slot = Some(snapshot.clone());
        snapshot
    }

    /// The files of the cache (empty until a walk; call [`snapshot`](Self::snapshot)).
    pub fn files(&self) -> Vec<PathBuf> {
        self.snapshot()
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect()
    }

    /// The `files` provider: fuzzy-ranks the cached relative paths.
    pub fn items(&self, query: &str, limit: usize, cancel: &CancelToken) -> Vec<QuickNavItem> {
        if query.trim().is_empty() || limit == 0 {
            return Vec::new();
        }
        let snapshot = self.snapshot();
        let ranked = rank_cancellable(
            query,
            snapshot
                .entries
                .iter()
                .map(|entry| (entry, entry.rel.as_str())),
            limit,
            cancel,
        );
        let Some(ranked) = ranked else {
            return Vec::new();
        };
        ranked
            .into_iter()
            .map(|row| {
                let entry = row.item;
                let positions = suffix_ranges(&entry.rel, &entry.name, 0, &row.matched);
                QuickNavItem {
                    kind: QuickNavKind::File,
                    title: entry.name.clone(),
                    subtitle: entry.parent.clone(),
                    icon_kind: IconKind::File,
                    score: 100 + row.matched.score,
                    positions,
                    action: QuickNavAction::OpenPath(entry.path.clone()),
                    id: format!("file:{}", entry.path.display()),
                }
            })
            .collect()
    }

    fn walk(&self, root: &Path) -> FileSnapshot {
        let generation = self.generation();
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut errors = 0usize;
        let mut builder = WalkBuilder::new(root);
        builder
            .standard_filters(false)
            .hidden(!self.options.include_hidden)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false)
            .follow_links(false);
        if !self.options.excluded_directories.is_empty() {
            let excluded: HashSet<String> = self
                .options
                .excluded_directories
                .iter()
                .map(|d| d.trim().trim_end_matches('/').to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect();
            let filter_root = root.to_path_buf();
            builder.filter_entry(move |entry| {
                if entry.path() == filter_root.as_path() {
                    return true;
                }
                if entry.file_type().is_some_and(|t| t.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    !excluded.contains(&name)
                } else {
                    true
                }
            });
        }
        for result in builder.build() {
            match result {
                Ok(entry) => {
                    if entry.path() == root || !entry.path().is_file() {
                        continue;
                    }
                    let Ok(rel) = entry.path().strip_prefix(root) else {
                        continue;
                    };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let parent = rel
                        .rsplit_once('/')
                        .map(|(parent, _)| parent.to_string())
                        .unwrap_or_else(|| ".".to_string());
                    entries.push(FileEntry {
                        path: entry.path().to_path_buf(),
                        rel,
                        name,
                        parent,
                    });
                    if entries.len() >= self.options.max_files {
                        truncated = true;
                        break;
                    }
                }
                Err(_) => errors += 1,
            }
        }
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        FileSnapshot {
            generation,
            root: root.to_path_buf(),
            entries,
            truncated,
            errors,
        }
    }
}

// --------------------------------------------------------- recent documents

/// Recently opened documents ("Open Recent"), most recent first.
#[derive(Debug, Clone)]
pub struct RecentDocuments {
    entries: Vec<PathBuf>,
    capacity: usize,
}

impl Default for RecentDocuments {
    fn default() -> Self {
        Self::new(20)
    }
}

impl RecentDocuments {
    /// A list capped at `capacity` (at least 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Records `path` at the front, moving duplicates and trimming.
    pub fn record(&mut self, path: PathBuf) {
        self.entries.retain(|p| p != &path);
        self.entries.insert(0, path);
        self.entries.truncate(self.capacity);
    }

    /// Removes a path.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|p| p != path);
        self.entries.len() != before
    }

    /// Empties the list ("Clear Menu").
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Most recent first.
    pub fn list(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The `recentDocuments` provider.
    pub fn items(&self, query: &str, limit: usize, cancel: &CancelToken) -> Vec<QuickNavItem> {
        recent_items(query, &self.entries, limit, cancel)
    }
}

/// The `recentDocuments` provider over a plain path list.
pub fn recent_items(
    query: &str,
    recent: &[PathBuf],
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    document_items(
        query,
        recent.iter().map(|p| (p.as_path(), None)),
        QuickNavKind::RecentDocument,
        limit,
        cancel,
    )
}

/// The `openDocument` provider.
pub fn open_document_items(
    query: &str,
    documents: &[OpenDocumentItem],
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    let mut fuzzy = Fuzzy::new(query);
    let mut rows: Vec<(usize, QuickNavItem)> = Vec::new();
    for (index, doc) in documents.iter().enumerate() {
        if index % 64 == 0 && cancel.is_cancelled() {
            break;
        }
        let path_text = doc.path.as_ref().map(|p| display_path(p));
        let (matched, haystack, trailing) = match fuzzy.score(&doc.name) {
            Some(m) => (m, doc.name.clone(), 0usize),
            None => match path_text
                .as_deref()
                .and_then(|p| fuzzy.score(p).map(|m| (m, p.to_string())))
            {
                Some((m, p)) => (m, p, 0),
                None => continue,
            },
        };
        let positions = suffix_ranges(
            &haystack,
            &doc.name,
            if haystack == doc.name { 0 } else { trailing },
            &matched,
        );
        rows.push((
            index,
            QuickNavItem {
                kind: QuickNavKind::OpenDocument,
                title: doc.name.clone(),
                subtitle: path_text.clone().unwrap_or_else(|| "Unsaved".to_string()),
                icon_kind: IconKind::Document,
                score: 250 + matched.score,
                positions,
                action: QuickNavAction::OpenDocument {
                    name: doc.name.clone(),
                    path: doc.path.clone(),
                },
                id: match &doc.path {
                    Some(path) => format!("doc:{}", path.display()),
                    None => format!("doc:{}", doc.name),
                },
            },
        ));
    }
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    rows.into_iter().take(limit).map(|(_, item)| item).collect()
}

fn document_items<'a>(
    query: &str,
    documents: impl Iterator<Item = (&'a Path, Option<&'a str>)>,
    kind: QuickNavKind,
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    let mut fuzzy = Fuzzy::new(query);
    let mut rows: Vec<(usize, QuickNavItem)> = Vec::new();
    for (index, (path, extra)) in documents.enumerate() {
        if index % 64 == 0 && cancel.is_cancelled() {
            break;
        }
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let haystack = display_path(path);
        let Some(matched) = fuzzy.score(&haystack) else {
            continue;
        };
        let positions = suffix_ranges(&haystack, &title, 0, &matched);
        let base = if kind == QuickNavKind::RecentDocument {
            150
        } else {
            200
        };
        rows.push((
            index,
            QuickNavItem {
                kind,
                title,
                subtitle: extra.map(str::to_string).unwrap_or(haystack),
                icon_kind: IconKind::Recent,
                score: base + matched.score,
                positions,
                action: QuickNavAction::OpenPath(path.to_path_buf()),
                id: format!("{}:{}", kind.provider(), path.display()),
            },
        ));
    }
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    rows.into_iter().take(limit).map(|(_, item)| item).collect()
}

// -------------------------------------------------------------- providers

/// The `symbols` provider: the current document's symbol list.
pub fn symbol_items(
    query: &str,
    symbols: &[Symbol],
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    let mut fuzzy = Fuzzy::new(query);
    let mut rows: Vec<(usize, QuickNavItem)> = Vec::new();
    for (index, symbol) in symbols.iter().enumerate() {
        if index % 128 == 0 && cancel.is_cancelled() {
            break;
        }
        let Some(matched) = fuzzy.score(&symbol.name) else {
            continue;
        };
        rows.push((
            index,
            QuickNavItem {
                kind: QuickNavKind::Symbol,
                title: symbol.name.clone(),
                subtitle: format!("{} \u{00b7} Line {}", symbol.kind.label(), symbol.line + 1),
                icon_kind: IconKind::Symbol(symbol.kind),
                score: 180 + matched.score - (symbol.level as i32) * 2,
                positions: suffix_ranges(&symbol.name, &symbol.name, 0, &matched),
                action: QuickNavAction::GoToSymbol {
                    name: symbol.name.clone(),
                    line: symbol.line,
                    column: symbol.column,
                },
                id: format!("symbol:{}:{}", symbol.name, symbol.line),
            },
        ));
    }
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    rows.into_iter().take(limit).map(|(_, item)| item).collect()
}

/// The `languages` provider.
pub fn language_items(
    query: &str,
    languages: &[LanguageChoice],
    mode: QuickNavMode,
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    let mut fuzzy = Fuzzy::new(query);
    let mut rows: Vec<(usize, QuickNavItem)> = Vec::new();
    let mut order = 0usize;
    for language in languages {
        if order.is_multiple_of(64) && cancel.is_cancelled() {
            break;
        }
        let Some(matched) = fuzzy.score(&language.name) else {
            continue;
        };
        let positions_for = |title: &str| suffix_ranges(&language.name, title, 0, &matched);
        if mode != QuickNavMode::NewDocument {
            let title = format!("Set Language: {}", language.name);
            rows.push((
                order,
                QuickNavItem {
                    kind: QuickNavKind::Language,
                    title: title.clone(),
                    subtitle: "Set the language of the current document".to_string(),
                    icon_kind: IconKind::Language,
                    score: 220 + matched.score,
                    positions: positions_for(&title),
                    action: QuickNavAction::SetLanguage(language.id.clone()),
                    id: format!("language:set:{}", language.id),
                },
            ));
            order += 1;
        }
        if mode != QuickNavMode::SelectLanguage {
            let title = format!("New Document: {}", language.name);
            rows.push((
                order,
                QuickNavItem {
                    kind: QuickNavKind::NewDocument,
                    title: title.clone(),
                    subtitle: "Create a new untitled document".to_string(),
                    icon_kind: IconKind::NewDocument,
                    score: 200 + matched.score,
                    positions: positions_for(&title),
                    action: QuickNavAction::NewDocument(language.id.clone()),
                    id: format!("language:new:{}", language.id),
                },
            ));
            order += 1;
        }
    }
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    rows.into_iter().take(limit).map(|(_, item)| item).collect()
}

/// The `mainMenu` provider.
pub fn menu_items(
    query: &str,
    commands: &[MenuCommandItem],
    limit: usize,
    cancel: &CancelToken,
) -> Vec<QuickNavItem> {
    let mut fuzzy = Fuzzy::new(query);
    let mut rows: Vec<(usize, QuickNavItem)> = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        if index % 128 == 0 && cancel.is_cancelled() {
            break;
        }
        let Some(matched) = fuzzy.score(&command.title) else {
            continue;
        };
        rows.push((
            index,
            QuickNavItem {
                kind: QuickNavKind::MenuCommand,
                title: command.title.clone(),
                subtitle: command.shortcut.clone().unwrap_or_default(),
                icon_kind: IconKind::MenuCommand,
                score: 120 + matched.score,
                positions: suffix_ranges(&command.title, &command.title, 0, &matched),
                action: QuickNavAction::MenuCommand(command.command.clone()),
                id: format!("menu:{}", command.command),
            },
        ));
    }
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    rows.into_iter().take(limit).map(|(_, item)| item).collect()
}

// --------------------------------------------------------------- aggregate

/// Everything one Open Quickly keystroke searches.
#[derive(Debug, Clone)]
pub struct QuickNavQuery {
    /// The field's text.
    pub text: String,
    /// Directory relative paths would resolve against (informational).
    pub cwd: PathBuf,
    /// Which palette is open.
    pub mode: QuickNavMode,
    /// Maximum rows to return.
    pub limit: usize,
    /// Open documents.
    pub open_documents: Vec<OpenDocumentItem>,
    /// Recent document paths, most recent first.
    pub recent_documents: Vec<PathBuf>,
    /// Symbols of the current document.
    pub symbols: Vec<Symbol>,
    /// Languages for the `languages` provider.
    pub languages: Vec<LanguageChoice>,
    /// Menu commands for the `mainMenu` provider.
    pub menu_commands: Vec<MenuCommandItem>,
    /// Cached navigator-root walk for the `files` provider.
    pub files: Option<Arc<FileCache>>,
}

impl Default for QuickNavQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            cwd: std::env::current_dir().unwrap_or_default(),
            mode: QuickNavMode::OpenQuickly,
            limit: 20,
            open_documents: Vec::new(),
            recent_documents: Vec::new(),
            symbols: Vec::new(),
            languages: Vec::new(),
            menu_commands: Vec::new(),
            files: None,
        }
    }
}

impl QuickNavQuery {
    /// A query with `text` and defaults elsewhere.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// Runs every provider for `query` (respecting the mode and limit), ranks
/// the rows and deduplicates them by id. Cancellation returns what was
/// gathered so far (usually nothing).
pub fn quicknav(query: &QuickNavQuery, cancel: &CancelToken) -> Vec<QuickNavItem> {
    if cancel.is_cancelled() {
        return Vec::new();
    }
    let text = query.text.as_str();
    let trimmed = text.trim();
    let limit = query.limit.max(1);
    let mut all: Vec<(usize, QuickNavItem)> = Vec::new();
    let mut order = 0usize;

    if trimmed.is_empty() {
        // CodeRunner collapses the empty palette; language palettes show
        // every language, Open Quickly shows open + recent documents.
        match query.mode {
            QuickNavMode::SelectLanguage | QuickNavMode::NewDocument => {
                append(
                    &mut all,
                    &mut order,
                    language_items("", &query.languages, query.mode, limit, cancel),
                );
            }
            QuickNavMode::OpenQuickly => {
                append(
                    &mut all,
                    &mut order,
                    open_documents_empty(&query.open_documents, limit),
                );
                append(
                    &mut all,
                    &mut order,
                    recent_items_empty(&query.recent_documents, limit),
                );
            }
        }
        return finish(all, limit);
    }

    // The language palettes list languages only — no files, symbols or
    // commands (a Select Language palette is not an Open Quickly).
    if matches!(query.mode, QuickNavMode::SelectLanguage | QuickNavMode::NewDocument) {
        append(
            &mut all,
            &mut order,
            language_items(trimmed, &query.languages, query.mode, limit, cancel),
        );
        return finish(all, limit);
    }

    append(&mut all, &mut order, line_items(trimmed));
    if cancel.is_cancelled() {
        return finish(all, limit);
    }
    if trimmed.starts_with('/') || trimmed.starts_with('~') || trimmed.starts_with("file://") {
        append(&mut all, &mut order, path_items(trimmed, limit, cancel));
    }
    append(
        &mut all,
        &mut order,
        symbol_items(trimmed, &query.symbols, limit, cancel),
    );
    append(
        &mut all,
        &mut order,
        open_document_items(trimmed, &query.open_documents, limit, cancel),
    );
    append(
        &mut all,
        &mut order,
        recent_items(trimmed, &query.recent_documents, limit, cancel),
    );
    if let Some(files) = &query.files {
        append(&mut all, &mut order, files.items(trimmed, limit, cancel));
    }
    append(
        &mut all,
        &mut order,
        language_items(trimmed, &query.languages, query.mode, limit, cancel),
    );
    append(
        &mut all,
        &mut order,
        menu_items(trimmed, &query.menu_commands, limit, cancel),
    );
    finish(all, limit)
}

fn append(all: &mut Vec<(usize, QuickNavItem)>, order: &mut usize, items: Vec<QuickNavItem>) {
    for item in items {
        all.push((*order, item));
        *order += 1;
    }
}

fn finish(mut all: Vec<(usize, QuickNavItem)>, limit: usize) -> Vec<QuickNavItem> {
    all.sort_by(|a, b| {
        b.1.score
            .cmp(&a.1.score)
            .then(a.1.kind.priority().cmp(&b.1.kind.priority()))
            .then(a.1.title.cmp(&b.1.title))
            .then(a.0.cmp(&b.0))
    });
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(limit.min(all.len()));
    for (_, item) in all {
        if !seen.insert(item.id.clone()) {
            continue;
        }
        out.push(item);
        if out.len() == limit {
            break;
        }
    }
    out
}

fn open_documents_empty(documents: &[OpenDocumentItem], limit: usize) -> Vec<QuickNavItem> {
    documents
        .iter()
        .take(limit)
        .map(|doc| QuickNavItem {
            kind: QuickNavKind::OpenDocument,
            title: doc.name.clone(),
            subtitle: doc
                .path
                .as_ref()
                .map(|p| display_path(p))
                .unwrap_or_else(|| "Unsaved".to_string()),
            icon_kind: IconKind::Document,
            score: 0,
            positions: Vec::new(),
            action: QuickNavAction::OpenDocument {
                name: doc.name.clone(),
                path: doc.path.clone(),
            },
            id: match &doc.path {
                Some(path) => format!("doc:{}", path.display()),
                None => format!("doc:{}", doc.name),
            },
        })
        .collect()
}

fn recent_items_empty(recent: &[PathBuf], limit: usize) -> Vec<QuickNavItem> {
    recent
        .iter()
        .take(limit)
        .map(|path| {
            let title = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| display_path(path));
            QuickNavItem {
                kind: QuickNavKind::RecentDocument,
                title,
                subtitle: display_path(path),
                icon_kind: IconKind::Recent,
                score: 0,
                positions: Vec::new(),
                action: QuickNavAction::OpenPath(path.clone()),
                id: format!("recent:{}", path.display()),
            }
        })
        .collect()
}

// ----------------------------------------------------------------- helpers

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `~`-abbreviated display path.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn tilde_display(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn join_display(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn parent_display(display: &str) -> String {
    match display.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    }
}

fn common_prefix_len(a: &str, b: &str, limit: usize) -> usize {
    let mut i = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        let len = ca.len_utf8();
        if i + len > limit || ca != cb {
            break;
        }
        i += len;
    }
    i
}

/// Byte ranges of `haystack`'s matched chars that also fall inside `title`,
/// where `title` is `haystack` minus `trailing` bytes (usually `/`) at the
/// end. Positions are re-based to `title`.
fn suffix_ranges(
    haystack: &str,
    title: &str,
    trailing: usize,
    matched: &FuzzyMatch,
) -> Vec<Range<usize>> {
    if title.is_empty() {
        return Vec::new();
    }
    let Some(end) = haystack.len().checked_sub(trailing) else {
        return Vec::new();
    };
    let Some(start) = end.checked_sub(title.len()) else {
        return Vec::new();
    };
    if !haystack.is_char_boundary(start)
        || !haystack.is_char_boundary(end)
        || &haystack[start..end] != title
    {
        return Vec::new();
    }
    matched
        .ranges(haystack)
        .into_iter()
        .filter_map(|range| {
            let s = range.start.max(start);
            let e = range.end.min(end);
            (s < e).then(|| s - start..e - start)
        })
        .collect()
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn line_spec_forms() {
        assert_eq!(
            parse_line_spec("12"),
            Some(LineSpec {
                line: 11,
                column: 0
            })
        );
        assert_eq!(
            parse_line_spec("line 12"),
            Some(LineSpec {
                line: 11,
                column: 0
            })
        );
        assert_eq!(
            parse_line_spec("Line12"),
            Some(LineSpec {
                line: 11,
                column: 0
            })
        );
        assert_eq!(
            parse_line_spec("l 3"),
            Some(LineSpec { line: 2, column: 0 })
        );
        assert_eq!(
            parse_line_spec("li7"),
            Some(LineSpec { line: 6, column: 0 })
        );
        assert_eq!(
            parse_line_spec("lin 9:4"),
            Some(LineSpec { line: 8, column: 3 })
        );
        assert_eq!(parse_line_spec("0"), None);
        assert_eq!(parse_line_spec("line"), None);
        assert_eq!(parse_line_spec("foo12"), None);
        assert_eq!(parse_line_spec("12x"), None);
        assert_eq!(parse_line_spec("line 1:0"), None);
        let items = line_items("l12");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, QuickNavKind::Line);
        assert_eq!(items[0].title, "Go to Line 12");
        assert_eq!(
            items[0].action,
            QuickNavAction::GoToLine {
                line: 11,
                column: 0
            }
        );
        assert!(
            items[0]
                .positions
                .iter()
                .any(|r| &items[0].title[r.clone()] == "12")
        );
    }

    #[test]
    fn path_completion_and_tab() {
        let dir = tempdir();
        fs::create_dir(dir.path().join("Documents")).unwrap();
        fs::create_dir(dir.path().join("Downloads")).unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        fs::write(dir.path().join(".hidden"), "x").unwrap();
        let base = dir.path().display().to_string();

        let all = complete_paths(&format!("{base}/"));
        let names: Vec<_> = all
            .iter()
            .map(|c| {
                c.display
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
            })
            .collect();
        assert!(names.contains(&"Documents"));
        assert!(names.contains(&"Downloads"));
        assert!(names.contains(&"notes.txt"));
        assert!(!names.contains(&".hidden"));
        assert!(all.iter().find(|c| c.is_dir).is_some());

        let doc = complete_paths(&format!("{base}/Doc"));
        assert_eq!(doc.len(), 1);
        assert!(doc[0].is_dir);
        assert_eq!(doc[0].display, format!("{base}/Documents/"));

        let hidden = complete_paths(&format!("{base}/.h"));
        assert_eq!(hidden.len(), 1);
        assert!(!hidden[0].is_dir);

        // Tab completion adds the only match.
        assert_eq!(
            tab_complete_path(&format!("{base}/not")),
            Some(format!("{base}/notes.txt"))
        );
        // Common prefix of Documents/ and Downloads/ is "Do".
        let common = tab_complete_path(&format!("{base}/D")).unwrap();
        assert!(common.ends_with("/Do"), "{common}");
        assert_eq!(tab_complete_path(&format!("{base}/zzz")), None);
        assert!(complete_paths("relative/path").is_empty());
    }

    #[test]
    fn path_items_are_fuzzy_ranked() {
        let dir = tempdir();
        fs::write(dir.path().join("main.rs"), "").unwrap();
        fs::write(dir.path().join("main_test.rs"), "").unwrap();
        let query = format!("{}/ma", dir.path().display());
        let items = path_items(&query, 10, &CancelToken::new());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, QuickNavKind::AbsolutePath);
        assert!(
            items
                .iter()
                .all(|i| i.subtitle.ends_with(&dir.path().display().to_string()))
        );
        assert!(
            items[0]
                .positions
                .iter()
                .any(|r| &items[0].title[r.clone()] == "ma")
        );
    }

    #[test]
    fn file_cache_walks_and_invalidates() {
        let dir = tempdir();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        let cache = FileCache::new(dir.path().to_path_buf());
        let snapshot = cache.snapshot();
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].rel, "README.md");
        assert_eq!(snapshot.entries[1].rel, "src/main.rs");
        assert_eq!(snapshot.entries[1].parent, "src");
        assert_eq!(cache.files().len(), 2);

        let items = cache.items("mrs", 10, &CancelToken::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "main.rs");
        assert_eq!(
            items[0].action,
            QuickNavAction::OpenPath(dir.path().join("src/main.rs"))
        );
        assert!(items[0].positions.len() >= 2);

        let generation = cache.generation();
        fs::write(dir.path().join("extra.py"), "").unwrap();
        cache.invalidate();
        assert!(cache.generation() > generation);
        let snapshot = cache.snapshot();
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.generation, cache.generation());
    }

    #[test]
    fn file_cache_excludes_directories() {
        let dir = tempdir();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/x.js"), "").unwrap();
        fs::write(dir.path().join("app.js"), "").unwrap();
        let cache = FileCache::with_options(
            dir.path().to_path_buf(),
            FileCacheOptions {
                excluded_directories: vec!["node_modules".into()],
                ..Default::default()
            },
        );
        assert_eq!(cache.snapshot().entries.len(), 1);
    }

    #[test]
    fn recent_documents() {
        let mut recent = RecentDocuments::new(3);
        recent.record(PathBuf::from("/a"));
        recent.record(PathBuf::from("/b"));
        recent.record(PathBuf::from("/a"));
        assert_eq!(recent.list(), [PathBuf::from("/a"), PathBuf::from("/b")]);
        recent.record(PathBuf::from("/c"));
        recent.record(PathBuf::from("/d"));
        assert_eq!(
            recent.list(),
            [
                PathBuf::from("/d"),
                PathBuf::from("/c"),
                PathBuf::from("/a")
            ]
        );
        assert!(recent.remove(Path::new("/c")));
        assert!(!recent.remove(Path::new("/zzz")));
        recent.clear();
        assert!(recent.is_empty());
        let items = recent_items("b", &[PathBuf::from("/tmp/b.rs")], 10, &CancelToken::new());
        assert_eq!(items[0].title, "b.rs");
    }

    #[test]
    fn open_document_and_symbol_items() {
        let docs = vec![
            OpenDocumentItem {
                name: "main.rs".into(),
                path: Some(PathBuf::from("/p/main.rs")),
            },
            OpenDocumentItem {
                name: "Untitled 2".into(),
                path: None,
            },
        ];
        let items = open_document_items("main", &docs, 10, &CancelToken::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].subtitle, display_path(Path::new("/p/main.rs")));
        let items = open_document_items("untitled", &docs, 10, &CancelToken::new());
        assert_eq!(items[0].subtitle, "Unsaved");

        let symbols = vec![
            Symbol {
                name: "parse_line_spec".into(),
                kind: SymbolKind::Function,
                line: 3,
                column: 0,
                level: 0,
            },
            Symbol {
                name: "QuickNav".into(),
                kind: SymbolKind::Struct,
                line: 10,
                column: 0,
                level: 0,
            },
        ];
        let items = symbol_items("parse", &symbols, 10, &CancelToken::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, QuickNavKind::Symbol);
        assert!(items[0].subtitle.contains("Function"));
        assert_eq!(
            items[0].action,
            QuickNavAction::GoToSymbol {
                name: "parse_line_spec".into(),
                line: 3,
                column: 0
            }
        );
    }

    #[test]
    fn language_and_menu_items() {
        let languages = vec![
            LanguageChoice {
                name: "Python".into(),
                id: "python".into(),
            },
            LanguageChoice {
                name: "Rust".into(),
                id: "rust".into(),
            },
        ];
        let all = language_items(
            "py",
            &languages,
            QuickNavMode::OpenQuickly,
            10,
            &CancelToken::new(),
        );
        assert_eq!(all.len(), 2); // Set + New
        assert_eq!(all[0].title, "Set Language: Python");
        assert_eq!(all[1].title, "New Document: Python");
        let only_new = language_items(
            "",
            &languages,
            QuickNavMode::NewDocument,
            10,
            &CancelToken::new(),
        );
        assert_eq!(only_new.len(), 2);
        assert!(only_new.iter().all(|i| i.kind == QuickNavKind::NewDocument));
        let only_set = language_items(
            "",
            &languages,
            QuickNavMode::SelectLanguage,
            10,
            &CancelToken::new(),
        );
        assert!(only_set.iter().all(|i| i.kind == QuickNavKind::Language));

        let commands = vec![MenuCommandItem {
            title: "Find Next".into(),
            command: "MainMenu/Find/Find Next".into(),
            shortcut: Some("\u{2318}G".into()),
        }];
        let items = menu_items("find", &commands, 10, &CancelToken::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].subtitle, "\u{2318}G");
        assert_eq!(
            items[0].action,
            QuickNavAction::MenuCommand("MainMenu/Find/Find Next".into())
        );
    }

    #[test]
    fn quicknav_aggregates_and_orders() {
        let dir = tempdir();
        fs::write(dir.path().join("file12.rs"), "").unwrap();
        let cache = Arc::new(FileCache::new(dir.path().to_path_buf()));
        let mut query = QuickNavQuery::new("12");
        query.files = Some(cache);
        query.open_documents = vec![OpenDocumentItem {
            name: "Untitled 1".into(),
            path: None,
        }];
        query.symbols = vec![Symbol {
            name: "line_12".into(),
            kind: SymbolKind::Function,
            line: 11,
            column: 0,
            level: 0,
        }];
        query.languages = vec![LanguageChoice {
            name: "Rust".into(),
            id: "rust".into(),
        }];
        let items = quicknav(&query, &CancelToken::new());
        assert_eq!(items[0].kind, QuickNavKind::Line);
        assert!(items.iter().any(|i| i.kind == QuickNavKind::Symbol));
        assert!(items.iter().any(|i| i.kind == QuickNavKind::File));

        let empty = QuickNavQuery {
            open_documents: vec![OpenDocumentItem {
                name: "A".into(),
                path: None,
            }],
            recent_documents: vec![PathBuf::from("/tmp/b.rs")],
            ..Default::default()
        };
        let items = quicknav(&empty, &CancelToken::new());
        assert_eq!(items[0].kind, QuickNavKind::OpenDocument);
        assert_eq!(items[1].kind, QuickNavKind::RecentDocument);

        let languages = QuickNavQuery {
            mode: QuickNavMode::SelectLanguage,
            languages: vec![LanguageChoice {
                name: "Rust".into(),
                id: "rust".into(),
            }],
            ..Default::default()
        };
        let items = quicknav(&languages, &CancelToken::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, QuickNavAction::SetLanguage("rust".into()));

        let mut path_query = QuickNavQuery::new(format!("{}/", dir.path().display()));
        path_query.files = Some(Arc::new(FileCache::new(dir.path().to_path_buf())));
        let items = quicknav(&path_query, &CancelToken::new());
        assert!(items.iter().any(|i| i.kind == QuickNavKind::AbsolutePath));
    }

    #[test]
    fn cancellation_and_dedupe() {
        let token = CancelToken::new();
        token.cancel();
        let items = quicknav(&QuickNavQuery::new("a"), &token);
        assert!(items.is_empty());

        let token = CancelToken::new();
        let docs = vec![OpenDocumentItem {
            name: "a.rs".into(),
            path: Some(PathBuf::from("/x/a.rs")),
        }];
        // The same path in recent documents must not appear twice.
        let mut query = QuickNavQuery::new("a");
        query.open_documents = docs;
        query.recent_documents = vec![PathBuf::from("/x/a.rs")];
        // openDocument id and recent id differ, so both may appear; the shared
        // file cache row uses the same path id as recent, so dedupe applies.
        let cache = Arc::new(FileCache::new(PathBuf::from("/x")));
        let _ = cache;
        query.limit = 10;
        let items = quicknav(&query, &CancelToken::new());
        let ids: HashSet<_> = items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(ids.len(), items.len());
        assert!(!items.is_empty());
        let _ = token;
    }

    #[test]
    fn huge_file_list_rank_is_fast() {
        let dir = tempdir();
        for i in 0..2_000 {
            fs::write(dir.path().join(format!("file_{i}.rs")), "").unwrap();
        }
        let cache = FileCache::new(dir.path().to_path_buf());
        let start = std::time::Instant::now();
        let items = cache.items("f19", 20, &CancelToken::new());
        assert!(!items.is_empty());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
