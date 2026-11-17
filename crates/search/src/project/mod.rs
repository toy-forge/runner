//! Project search and replace ("Search in File System" / the Search sidebar).
//!
//! * [`search`] - the walker, [`SearchRequest`], streamed [`SearchEvent`]s.
//! * [`replace`] - [`ReplacePlan`], [`ReplaceOptions`], the [`ReplaceJournal`]
//!   behind "Undo Replace".
//! * [`encoding`] - BOM/UTF-8/fallback detection and round-trip-safe
//!   re-encoding.
//!
//! The engine is NSH-free and does not touch the UI: it reads files from
//! disk or from text snapshots and reports byte offsets, lines and UTF-16
//! columns. The Search sidebar owns grouping, batching and the large-result
//! confirmation.

pub mod encoding;
pub mod replace;
pub mod search;

pub use encoding::{
    Decoded, EncodeError, TextEncoding, content_hash, decode, decode_as, default_fallback, encode,
    looks_binary, roundtrips,
};
pub use replace::{
    ConflictPolicy, FileReplaceOutcome, JournalEntry, PlannedFile, PlannedMatch, RenameConflict,
    RenameOutcome, RenameStatus, ReplaceIssue, ReplaceIssueKind, ReplaceJournal, ReplaceOptions,
    ReplacePlan, ReplacePlanError, ReplaceReport, ReplaceSummary, TargetSelection, UndoFailure,
    UndoReport,
};
pub use search::{
    DocumentSnapshot, FileId, MatchId, MatchKind, ProjectMatch, ScopeFilter, SearchDone,
    SearchEvent, SearchFile, SearchHandle, SearchIssue, SearchMode, SearchOptions, SearchOutcome,
    SearchProgress, SearchRequest, SearchResults, SearchSource, search_project,
    search_project_with, spawn_search, spawn_search_with,
};
