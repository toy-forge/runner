//! The crate's error type.

/// An error from a server, from the connection to it, or from an edit that cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LspError {
    /// The server is not running (never started, exited, or being restarted).
    #[error("the language server is not running")]
    NotRunning,
    /// The server did not answer in time.
    #[error("{method} timed out")]
    Timeout {
        /// The request that timed out.
        method: String,
    },
    /// The request was cancelled, by the caller or because a newer request replaced it.
    #[error("the request was cancelled")]
    Cancelled,
    /// The server could not answer because the document changed meanwhile (LSP `ContentModified`).
    /// Ask again after the next edit.
    #[error("the document changed while the server was answering")]
    ContentModified,
    /// The server answered with an error.
    #[error("{message}")]
    Server {
        /// The JSON-RPC error code.
        code: i64,
        /// The server's message.
        message: String,
    },
    /// The server process could not be started or died during initialization.
    #[error("{0}")]
    Start(String),
    /// The server spoke something that is not the protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The server does not support the feature (it did not advertise the capability).
    #[error("the language server does not support {0}")]
    Unsupported(String),
    /// The document is not open in the manager.
    #[error("unknown document {0}")]
    UnknownDocument(String),
    /// An edit is malformed or does not fit the text it targets.
    #[error("invalid edit: {0}")]
    InvalidEdit(String),
    /// A file system problem (virtual workspace, applying edits to closed files).
    #[error("{0}")]
    Io(String),
    /// A configuration file (the server catalogue, a language table, user overrides) is invalid.
    #[error("{0}")]
    Config(String),
}

impl LspError {
    /// Whether the error just means "this answer is stale"; UIs should drop it silently.
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Cancelled | Self::ContentModified)
    }

    /// Maps a JSON-RPC error object's code and message to the matching variant.
    pub(crate) fn from_rpc(code: i64, message: String) -> Self {
        match code {
            -32800 => Self::Cancelled,
            -32801 => Self::ContentModified,
            _ => Self::Server { code, message },
        }
    }
}

impl From<std::io::Error> for LspError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// The crate's result type.
pub type Result<T, E = LspError> = std::result::Result<T, E>;
