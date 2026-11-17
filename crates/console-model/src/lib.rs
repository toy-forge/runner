//! `scratch-console-model`: the UI-free model behind ScratchRunner's console.
//!
//! No NSH dependency: everything here is plain Rust, testable with `cargo test`.
//!
//! # Main types
//!
//! * [`Console`] - a bounded line store. Feed it bytes with
//!   [`Console::append`]`(stream, bytes)` (any chunking), read lines by index
//!   ([`Console::visible_line`]) to render them in a `virtual_list`. It parses
//!   ANSI ([`AnsiParser`]), applies carriage-return overwrite semantics, tags
//!   every run with its [`Stream`] (stdout / stderr / input / debugger /
//!   system), keeps stable [`LineId`]s, evicts old lines past
//!   [`ConsoleConfig`] limits ([`Console::truncated`] gives the marker text) and
//!   reports large output ([`GuardEvent`]).
//! * [`Filter`] / [`OutputFilter`] - the output pop-up (All / Program / Stdout /
//!   Stderr / Debugger / Errors / ...) plus a text or regex field;
//!   [`Console::set_filter`], then the `visible_*` accessors.
//!   [`ScopeSelector`] is the small scope-selector engine behind it.
//! * [`LineView`] - one line: `text()`, `runs()`, `level()`, [`LineView::links`]
//!   and [`LineView::spans`] (runs cut at link / search / selection boundaries,
//!   each a [`Span`] ready to render as one styled text).
//! * [`detect_links`] / [`Link`] / [`LinkTarget`] - `file:line:col`, Python
//!   tracebacks, rustc/clang diagnostics, MSVC style and URLs.
//! * [`ConsoleSelection`] / [`Pos`] - selection by `(LineId, byte offset)`;
//!   [`Console::selection_text`], [`Console::word_selection`], ...
//! * [`SearchQuery`] / [`Matcher`] / [`Match`] - find in console:
//!   [`Console::find_all`], [`Console::find_next`], [`Console::find_prev`].
//! * [`Style`], [`AnsiPalette`] - colours; [`textutil`] - UTF-8 / UTF-16 /
//!   display-column conversions.
//!
//! # Example
//!
//! ```
//! use scratch_console_model::*;
//!
//! let mut console = Console::default();
//! console.append(Stream::Stdout, b"Name? \x1b[32mbob\x1b[0m\r\n");
//! console.append(Stream::Stderr, b"main.c:3:5: error: boom\n");
//!
//! assert_eq!(console.visible_count(), 2);
//! let line = console.visible_line(1).unwrap();
//! assert_eq!(line.level(), Level::Error);
//! let links = line.links();
//! assert_eq!(
//!     links[0].target,
//!     LinkTarget::File { path: "main.c".into(), line: Some(3), column: Some(5) }
//! );
//!
//! // Only stderr:
//! console.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
//! assert_eq!(console.visible_count(), 1);
//! ```
//!
//! # Threading
//!
//! [`Console`] is `Send` but not shared: keep it on the UI thread (for example
//! in an `Rc<RefCell<_>>`), read the child's pipes on other threads and send
//! the byte chunks to the UI thread, which calls `append` once per batch and
//! then bumps one revision signal.

pub mod ansi;
mod console;
mod filter;
mod line;
pub mod links;
mod query;
mod scope;
mod search;
mod selection;
pub mod style;
mod tag;
pub mod textutil;

pub use ansi::{AnsiEvent, AnsiParser, EraseMode, strip_ansi};
pub use console::{Change, Console, ConsoleConfig, GuardEvent, Truncated};
pub use filter::{Filter, FilterError, OutputFilter};
pub use line::{LineId, LineView, Marks, Run, Span, SpanOptions, StyledRun};
pub use links::{Link, LinkTarget, classify_level, detect_links};
pub use query::{Matcher, QueryError, SearchQuery};
pub use scope::{ScopeSelector, SelectorError};
pub use search::Match;
pub use selection::{LineSelection, Pos, ConsoleSelection};
pub use style::{AnsiPalette, Attrs, Color, ResolvedStyle, Rgb, Style};
pub use tag::{Level, ScopeMask, Stream};
