//! Why a run could not start or could not finish normally.

use thiserror::Error;

/// A run-level error: the run did not get as far as executing the program (or a
/// compile stage), or something outside the program's control went wrong.
///
/// Every variant carries a message that is meant to be shown to the user as it
/// is (CodeRunner shows such messages in the console as error text).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunError {
    /// A program the language needs is not installed. `message` already
    /// includes the language's installer hint when it has one.
    #[error("{message}")]
    MissingTool {
        /// The program that was looked up (`go`, `clang`, `python3`).
        program: String,
        /// The text to show.
        message: String,
    },
    /// The program is an Xcode command line tools shim (`/usr/bin/clang`,
    /// `python3`, `git`, ...) and no developer directory is installed: running
    /// it would open the system's installer dialog, so the run is not started.
    #[error("{message}")]
    DeveloperToolsMissing {
        /// The shim that was about to be started.
        program: String,
        /// The `xcode-select` guidance to show.
        message: String,
    },
    /// `python`/`python3` is only the Xcode shim and no real Python was found.
    #[error("{message}")]
    PythonMissing {
        /// The guidance to show.
        message: String,
    },
    /// The language has no run command (Markdown renders in-process).
    #[error("This language has no run command.")]
    NoRunCommand {
        /// The language's in-process renderer (`markdown`), when it has one.
        renderer: Option<String>,
    },
    /// The run command (or a compile template) cannot be expanded.
    #[error("Invalid Run Command: {0}")]
    InvalidRunCommand(String),
    /// The run arguments have unbalanced quotes.
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    /// The working directory is missing.
    #[error(
        "Error: ShellTask: The working directory \"{0}\" does not exist or is not a directory."
    )]
    WorkingDirectory(String),
    /// The saved file the run was asked to execute does not exist.
    #[error("The file \"{0}\" does not exist. Save the document and try again.")]
    SourceMissing(String),
    /// The unsaved document or the run directory could not be written.
    #[error("Unable to write temp file. Save and try again. ({0})")]
    TempFile(String),
    /// The Program Input could not be written or read.
    #[error("Failed to write temporary input file. Error: {0}")]
    InputFile(String),
    /// The language's run specification refused to plan (a requirement of a
    /// step is not met, a probe failed). The text is for the user.
    #[error("{0}")]
    Plan(String),
    /// A process could not be started.
    #[error("{program} could not start: {error}")]
    Spawn {
        /// The program.
        program: String,
        /// The operating system's explanation.
        error: String,
    },
    /// A bug or an unexpected failure of the engine itself.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Why [`crate::Runner::start`] refused to start a run.
#[derive(Debug, Error)]
pub enum StartError {
    /// The owner (document) already has a run in progress and the request's
    /// [`crate::OnBusy`] is `Reject`.
    #[error("a run is already in progress for `{owner}` (run {run})")]
    AlreadyRunning {
        /// The owner key of the request.
        owner: String,
        /// The run that is in progress.
        run: crate::RunId,
    },
    /// The thread of the run could not be created.
    #[error("could not start the run thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// Why input could not be sent to a run.
#[derive(Debug, Error)]
pub enum InputError {
    /// The run has ended.
    #[error("the run has ended")]
    NotRunning,
    /// The pipe or pty rejected the bytes (the program closed its input).
    #[error("could not write to the program: {0}")]
    Io(#[from] std::io::Error),
}
