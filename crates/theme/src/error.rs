//! Errors of theme loading, import/export and library management.

use std::path::PathBuf;

/// Everything that can go wrong in this crate's fallible operations.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// The bytes are not a property list.
    #[error("not a property list: {0}")]
    Plist(String),
    /// A property list that is not a theme.
    #[error("not a theme file: {0}")]
    Format(String),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The OS error.
        source: std::io::Error,
    },
    /// No theme has this name.
    #[error("no theme named `{0}`")]
    NotFound(String),
    /// A theme with this name exists.
    #[error("a theme named `{0}` already exists")]
    Exists(String),
    /// A built-in theme cannot be changed this way.
    #[error("built-in themes cannot be {0}")]
    Builtin(&'static str),
    /// The name is empty or unusable as a file name.
    #[error("invalid theme name `{0}`")]
    InvalidName(String),
    /// The library has no user directory.
    #[error("the library has no user themes directory")]
    NoUserDir,
}

impl ThemeError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> ThemeError {
        ThemeError::Io {
            path: path.into(),
            source,
        }
    }
}
