//! Error types of the language system.

use std::path::PathBuf;

use thiserror::Error;

/// Why a command template could not be expanded (see [`crate::expand_command`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExpandError {
    /// The template uses shell syntax (`;`, `|`, redirects, `$(...)`, globs, a
    /// leading `NAME=value`, ...) that cannot be expressed as an argv. Run it
    /// through a shell instead ([`crate::expand_shell_command`]).
    #[error("the command needs a shell: {0}")]
    NeedsShell(&'static str),
    /// A quote was opened and never closed.
    #[error("unterminated quote in command")]
    UnterminatedQuote,
    /// A `${...}` form that is not supported (supported: `${v}`, `${v:-d}`,
    /// `${v%p}`, `${v%%p}`, `${v#p}`, `${v##p}`).
    #[error("unsupported expansion `${{{0}}}`")]
    UnsupportedExpansion(String),
    /// A `${` without its closing `}`.
    #[error("unterminated `${{` in command")]
    UnterminatedBrace,
    /// The template expands to no words at all.
    #[error("the command is empty")]
    EmptyCommand,
}

/// Why a run cannot be planned (see [`crate::RunSpec::plan`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    /// A template of the spec failed to expand.
    #[error("{context}: {source}")]
    Expand {
        /// Which part of the spec (`step "Compile"`, `run command`, ...).
        context: String,
        /// The underlying error.
        #[source]
        source: ExpandError,
    },
    /// A requirement of a step is not met. The message is meant for the user.
    #[error("{0}")]
    Requirement(String),
    /// The spec itself is wrong (unknown variable source, empty program, ...).
    #[error("invalid run specification: {0}")]
    Invalid(String),
    /// A filesystem probe (include scanning, directory listing) failed.
    #[error("cannot inspect {path}: {message}")]
    Probe {
        /// The path that could not be read.
        path: PathBuf,
        /// The I/O error text.
        message: String,
    },
}

/// Errors of loading, saving, importing and managing language definitions.
#[derive(Debug, Error)]
pub enum LanguageError {
    /// A TOML definition could not be parsed.
    #[error("invalid language definition{}: {message}", origin_suffix(.origin))]
    Parse {
        /// The file or built-in it came from, when known.
        origin: Option<String>,
        /// The parser's message.
        message: String,
    },
    /// A definition could not be written as TOML.
    #[error("cannot serialize language definition: {0}")]
    Serialize(String),
    /// A definition parsed but is not valid.
    #[error("language definition `{id}` is not valid: {}", .problems.join("; "))]
    Invalid {
        /// The definition's id (may be empty).
        id: String,
        /// Every problem found.
        problems: Vec<String>,
    },
    /// A file-system operation failed.
    #[error("{operation} {path}: {source}")]
    Io {
        /// What was attempted (`read`, `write`, ...).
        operation: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A CodeRunner `Language.plist` could not be read.
    #[error("cannot read CodeRunner language: {0}")]
    Plist(String),
    /// No language has this id, uuid or name.
    #[error("no language `{0}`")]
    NotFound(String),
    /// A language with this id already exists.
    #[error("a language with id `{0}` already exists")]
    Duplicate(String),
    /// The operation only applies to bundled languages (`reset`).
    #[error("language `{0}` is not a bundled language")]
    NotBundled(String),
    /// The operation does not apply to bundled languages (`delete`).
    #[error("language `{0}` is bundled and cannot be deleted; disable it instead")]
    BundledLanguage(String),
    /// The registry has no user directory configured.
    #[error("the registry has no user language directory")]
    NoUserDirectory,
}

fn origin_suffix(origin: &Option<String>) -> String {
    match origin {
        Some(origin) => format!(" ({origin})"),
        None => String::new(),
    }
}

impl LanguageError {
    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
