//! ScratchRunner's debugger back end.  No NSH/UI dependency, so it stays
//! fast to test.
//!
//! The crate drives ordinary command-line debuggers (LLDB, pdb, node, jdb,
//! bashdb, perl, phpdbg) through a small TOML adapter format of our own.  It
//! does not use CodeRunner's proprietary `.crDebugger` bundle format or helper
//! binaries.
//!
//! # Main types
//!
//! | Type | Purpose |
//! |--|--|
//! | [`DebuggerAdapter`] / [`AdapterRegistry`] | Data-driven description of a debugger (prompt, verbs, parsers, options). |
//! | [`SessionConfig`] / [`DebugSession`] | Start and drive a debugger session; commands in, [`DebugEvent`]s out. |
//! | [`DebugEvent`], [`DebuggerState`], [`StopReason`], [`Location`], [`Stack`], [`Frame`] | Session state machine and stack/stop model. |
//! | [`Variable`], [`VariableTree`], [`VariableId`], [`SortMode`] | Variables outline, lazy child expansion and ordering. |
//! | [`Breakpoint`], [`BreakpointStore`], [`LineTracker`], [`TextEdit`] | Breakpoints, persistence and line remapping across edits. |
//! | [`SourceMap`] | Map debugger-reported paths to local source files. |
//! | [`ReplHistory`] | Up/down history for the console's debugger prompt. |
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use scratch_debugger::*;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let registry = AdapterRegistry::builtin();
//! let Some(adapter) = registry.for_language("c") else {
//!     return Ok(());
//! };
//! let config = SessionConfig::new(adapter.clone(), PathBuf::from("/tmp/a.out"))
//!     .cwd(PathBuf::from("/tmp"))
//!     .breakpoints(vec![Breakpoint::new("/tmp/a.c", 5)]);
//! let session = DebugSession::start(config)?;
//! loop {
//!     match session.recv_timeout(Duration::from_secs(5)) {
//!         Ok(DebugEvent::StoppedAt { location, annotation, .. }) => {
//!             println!("{annotation} at {}", location.describe());
//!             session.variables(VariablesQuery::new(location.frame))?;
//!         }
//!         Ok(DebugEvent::VariablesChanged { variables, .. }) => {
//!             for root in &variables.roots {
//!                 println!("{} = {:?}", root.name, root.value);
//!             }
//!             session.continue_()?;
//!         }
//!         Ok(DebugEvent::Exited { code }) => {
//!             println!("exit {code}");
//!             break;
//!         }
//!         Ok(DebugEvent::Error { message }) => eprintln!("{message}"),
//!         Ok(_) => {}
//!         Err(_) => break,
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod adapter;
pub mod breakpoints;
pub mod error;
mod helpers;
pub mod model;
pub mod parser;
pub mod repl;
pub mod session;
pub mod source_map;
pub mod variables;

pub use adapter::{
    AdapterLoadError, AdapterLoadReport, AdapterOptions, AdapterRegistry, Availability, Commands,
    CommandFlags, DebuggerAdapter, EchoMode, Helpers, ParserKind, ParserSpec, StartSpec,
    BUILTIN_ADAPTER_IDS, find_executable,
};
pub use breakpoints::{
    group_by_file, Breakpoint, BreakpointKey, BreakpointStore, LineTracker, TextEdit,
};
pub use error::DebugError;
pub use model::{
    DebugEvent, DebuggerState, ExpressionKind, Frame, FrameId, Location, LogDirection,
    OutputStream, SessionCommand, Stack, StopReason, Thread, ThreadId, VariablesQuery,
};
pub use parser::{strip_ansi, AnsiStripper, TranscriptDecoder};
pub use repl::ReplHistory;
pub use session::{DebugSession, SessionConfig};
pub use source_map::SourceMap;
pub use variables::{
    natural_cmp, sort_variables, truncate_values, SortMode, Variable, VariableId, VariableKind,
    VariableTree, WatchValue,
};
