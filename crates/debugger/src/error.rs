//! Error type for the debugger back end.

use std::path::PathBuf;

/// Errors produced while loading adapters, starting sessions or talking to a
/// debugger.
///
/// The crate keeps errors typed at its public boundary.  Fatal session errors
/// are also delivered to the UI as [`crate::DebugEvent::Error`] so a session
/// that fails after it started is not silent.
#[derive(Debug, thiserror::Error)]
pub enum DebugError {
    /// No adapter with this id exists in the registry.
    #[error("unknown debugger adapter `{0}`")]
    UnknownAdapter(String),

    /// The adapter's command line (or one of its required helper tools) is not
    /// installed.
    #[error("debugger tool not found: {tool}")]
    ToolNotFound {
        /// The tool that was looked up.
        tool: String,
        /// Human readable installation hint.
        hint: String,
    },

    /// The debugger process could not be spawned.
    #[error("could not start debugger: {0}")]
    Spawn(String),

    /// The adapter definition is malformed (bad regex, missing field, ...).
    #[error("invalid debugger adapter `{adapter}`: {message}")]
    InvalidAdapter {
        /// Adapter id, when known.
        adapter: String,
        /// What is wrong.
        message: String,
    },

    /// The session thread has gone away.
    #[error("debugger session is not running")]
    SessionGone,

    /// A command is not supported by the selected debugger.
    #[error("unsupported by this debugger: {0}")]
    Unsupported(&'static str),

    /// A command was used in the wrong session state.
    #[error("{0}")]
    WrongState(String),

    /// I/O error.
    #[error("debugger I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The adapter needs a file that does not exist.
    #[error("missing file: {}", .0.display())]
    MissingFile(PathBuf),

    /// Anything else, described at the boundary.
    #[error("{0}")]
    Message(String),
}
