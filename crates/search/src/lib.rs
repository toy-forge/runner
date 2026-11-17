//! ScratchRunner's search engine: the find bar, project search & replace,
//! fuzzy matching, QuickNav (Open Quickly) and symbol extraction. NSH-free,
//! so it unit-tests without a window server.
//!
//! # Main types
//!
//! * [`Matcher`] - one compiled find/replace query (Text / Exact Word / Word
//!   Starts With / Word Ends With / Ignoring Whitespace / Regular
//!   Expression), working on `&str` with byte offsets. [`MatchMode`] and
//!   [`MatcherOptions`] are its inputs; [`Found`], [`Matches`] and
//!   [`ReplaceAll`] its outputs; [`Replacement`] templates support `$1`,
//!   `${name}` and `\U \L \E \u \l`.
//! * [`LineIndex`] / [`Position`] - byte offset <-> 0-based line and UTF-16
//!   column conversions for the NSH editor.
//! * [`project`] - [`SearchRequest`]/[`spawn_search`] walk a folder,
//!   selection, custom path list or open documents on worker threads and
//!   stream [`SearchEvent`]s; [`ReplacePlan`] applies accepted matches with
//!   encoding-safe writes, filename renames and an undo journal.
//! * [`fuzzy`] - subsequence scoring with match positions, shared by QuickNav,
//!   popup type-to-search and the command palette.
//! * [`quicknav`] - pure provider functions (absolute paths, line specs,
//!   symbols, files, recent/open documents, languages, menu commands) and the
//!   `QuickNavQuery -> Vec<QuickNavItem>` aggregation.
//! * [`symbols`] - heuristic symbol extraction for the bundled languages.
//! * [`patterns`] - the find bar's Insert Pattern menu and the user's
//!   `TextFinderUserPatterns`.
//! * [`cancel`] - the cooperative [`CancelToken`].
//!
//! # Example
//!
//! ```
//! use scratch_search::{apply_edits, MatchMode, Matcher};
//!
//! let matcher = Matcher::simple("cat", MatchMode::Text, true).unwrap();
//! let all = matcher.find_all("A cat and a CAT").unwrap();
//! assert_eq!(all, vec![2..5, 12..15]);
//!
//! let replacement = matcher.compile_replacement("dog").unwrap();
//! let replaced = matcher.replace_all("A cat and a CAT", None, &replacement, None).unwrap();
//! assert_eq!(apply_edits("A cat and a CAT", &replaced.edits).unwrap(), "A dog and a dog");
//! ```
//!
//! The crate-level docs of each module explain the finer points (CRLF mode,
//! encodings, cancellation, result batching).

pub mod cancel;
pub mod edit;
pub mod fuzzy;
pub mod matcher;
pub mod onig;
pub mod patterns;
pub mod project;
pub mod quicknav;
pub mod replacement;
pub mod symbols;
pub mod text;

pub use cancel::CancelToken;
pub use edit::{
    EditError, PositionEdit, TextEdit, apply_edits, invert_edits, to_position_edits, validate_edits,
};
pub use matcher::{
    Found, MatchError, MatchMode, Matcher, MatcherOptions, Matches, ReplaceAll, ReplaceStep,
};
pub use patterns::{
    InsertPattern, UserPattern, UserPatterns, builtin_find_patterns, builtin_replace_patterns,
    capture_group_pattern, current_search_as_regex, escaped_replacement, named_group_pattern,
    replace_insertions_for,
};
pub use project::*;
pub use quicknav::*;
pub use replacement::Replacement;
pub use symbols::{
    LanguageId, SUPPORTED_LANGUAGES, Symbol, SymbolKind, SymbolOptions, detect_language,
    extract_symbols, extract_symbols_with,
};
pub use text::{
    LineIndex, Position, PositionRange, byte_to_utf16, ceil_char_boundary, floor_char_boundary,
    utf16_len, utf16_to_byte,
};
