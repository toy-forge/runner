//! CodeRunner's compile-script protocol, for user-defined languages.
//!
//! A language whose [`crate::CompileSpec`] is `Shell` carries a script that is
//! run before the run command:
//!
//! * the working directory is the directory of the source file;
//! * `$1..$n` are the compile flags (shell-split);
//! * the environment holds the `CR_*` variables ([`crate::Vars::export_env`]):
//!   `CR_FILENAME`, `CR_FILE`, `CR_TMPDIR`, `CR_SUGGESTED_OUTPUT_FILE`,
//!   `CR_DEBUGGING`, `CR_ENCODING`, ...;
//! * exit status 0 means success and the run continues; the **last non-empty
//!   line of stdout** becomes `$compiler` ([`parse_compiler_output`]);
//! * anything else is a compile error: stderr (and stdout) are shown in the
//!   console and parsed by diagnostics. Three statuses are special
//!   ([`CompileExit`]): 127 (command not found: the language's installer hint
//!   or its `errors["127"]` text is shown), 69 (the Xcode license was not
//!   accepted) and 56 (the sandboxed app needs file access; not used here).
//!
//! The compile script and the run command are separate processes; the only
//! state shared between them is `$compiler`.

/// A compile script exit status.
pub const EXIT_COMMAND_NOT_FOUND: i32 = 127;
/// The Xcode license agreement has not been accepted.
pub const EXIT_XCODE_LICENSE: i32 = 69;
/// The sandboxed app must ask for file access.
pub const EXIT_NEEDS_FILE_ACCESS: i32 = 56;

/// The meaning of a compile script's (or compile step's) exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileExit {
    /// 0: the run continues.
    Success,
    /// 127: a program was not found.
    CommandNotFound,
    /// 69: the Xcode license is not accepted.
    XcodeLicense,
    /// 56: file access is needed.
    NeedsFileAccess,
    /// Any other non-zero status: a compile error.
    Failed(i32),
}

impl CompileExit {
    /// Interprets an exit status.
    pub fn from_status(status: i32) -> Self {
        match status {
            0 => Self::Success,
            EXIT_COMMAND_NOT_FOUND => Self::CommandNotFound,
            EXIT_XCODE_LICENSE => Self::XcodeLicense,
            EXIT_NEEDS_FILE_ACCESS => Self::NeedsFileAccess,
            other => Self::Failed(other),
        }
    }

    /// The user-facing text for the special statuses (`None` for success and
    /// ordinary failures; 127 needs the language's own message, see
    /// [`crate::LanguageDef::exit_status_message`]).
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::XcodeLicense => Some(
                "Please open Xcode and accept the Developer License Agreement, then run again.",
            ),
            Self::NeedsFileAccess => Some("ScratchRunner needs permission to access this file."),
            _ => None,
        }
    }
}

/// The value of `$compiler` for a compile script's stdout: its last non-empty
/// line, trimmed of the line break. `None` when the output has no such line.
///
/// ```
/// use scratch_language::parse_compiler_output;
/// assert_eq!(parse_compiler_output("note\n/tmp/a.out\n\n").as_deref(), Some("/tmp/a.out"));
/// assert_eq!(parse_compiler_output("  \n"), None);
/// ```
pub fn parse_compiler_output(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(|line| line.trim_end_matches('\r'))
        .find(|line| !line.trim().is_empty())
        .map(str::to_owned)
}

/// The status line shown while the phases run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    /// "Preparing Run..."
    Preparing,
    /// "Compiling..."
    Compiling,
    /// "Running..."
    Running,
}

impl RunPhase {
    /// CodeRunner's status text.
    pub fn status_text(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing Run...",
            Self::Compiling => "Compiling...",
            Self::Running => "Running...",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_is_the_last_non_empty_line() {
        assert_eq!(parse_compiler_output("a\nb\n").as_deref(), Some("b"));
        assert_eq!(
            parse_compiler_output("a\r\nb\r\n\r\n").as_deref(),
            Some("b")
        );
        assert_eq!(parse_compiler_output("only").as_deref(), Some("only"));
        assert_eq!(parse_compiler_output(""), None);
        assert_eq!(parse_compiler_output("\n \n"), None);
        assert_eq!(
            parse_compiler_output("a\n  /x y  \n").as_deref(),
            Some("  /x y  ")
        );
    }

    #[test]
    fn exit_statuses() {
        assert_eq!(CompileExit::from_status(0), CompileExit::Success);
        assert_eq!(CompileExit::from_status(127), CompileExit::CommandNotFound);
        assert_eq!(CompileExit::from_status(69), CompileExit::XcodeLicense);
        assert_eq!(CompileExit::from_status(56), CompileExit::NeedsFileAccess);
        assert_eq!(CompileExit::from_status(1), CompileExit::Failed(1));
        assert!(
            CompileExit::XcodeLicense
                .message()
                .unwrap()
                .contains("License")
        );
        assert!(CompileExit::Failed(1).message().is_none());
    }
}
