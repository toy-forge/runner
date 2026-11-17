//! What a run reports: [`RunMessage`]s carrying [`RunEvent`]s, ending with a
//! [`RunSummary`].

use std::fmt;
use std::time::Duration;

use scratch_language::RunPhase;

use crate::RunError;

/// Identifies one run. Ids are unique within the process and increase with
/// every run, so a consumer that remembers the id of a document's current run
/// can drop the late events of an older one ([`crate::Runner::is_current`]).
/// The number is also exported to the program as `CR_RUNID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub(crate) u64);

impl RunId {
    /// A new, process-unique id.
    pub(crate) fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The number (`CR_RUNID`).
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Which stream a chunk of output came from. Exact: stdout is the pty, stderr is
/// its own pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stream {
    /// Standard output (the pty: also carries the echo of typed input).
    Stdout,
    /// Standard error.
    Stderr,
}

/// The phase of the run an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Compile stages (declarative steps or the compile script).
    Compile,
    /// The program itself.
    Run,
}

/// An event with the id of the run it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct RunMessage {
    /// The run.
    pub run: RunId,
    /// Position of this message in the run's sequence (0, 1, 2, ...).
    pub seq: u64,
    /// What happened.
    pub event: RunEvent,
}

/// What a run reports, in order. `Finished` is always last.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// The run is being prepared (status text "Preparing Run...").
    Preparing,
    /// A compile stage begins (status text "Compiling...").
    StageStarted(StageInfo),
    /// A compile stage ended.
    StageFinished(StageOutcome),
    /// Raw output bytes (not necessarily on a character boundary: use
    /// [`crate::Utf8Decoder`] per stream). Compile output has
    /// `phase == Phase::Compile`.
    Output {
        /// Compile or program output.
        phase: Phase,
        /// Where it came from.
        stream: Stream,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// The program was started (status text "Running...").
    Started(ProgramInfo),
    /// The program has printed `bytes` bytes, past
    /// [`crate::RunLimits::warn_output_bytes`]. The app may ask the user to stop
    /// the run ([`crate::RunHandle::cancel`]) or to continue
    /// ([`crate::RunHandle::allow_large_output`]).
    LargeOutput {
        /// Bytes of output so far.
        bytes: u64,
    },
    /// Output past [`crate::RunLimits::max_output_bytes`] is being dropped
    /// ("... N more characters not displayed ..."). When
    /// [`crate::RunLimits::stop_at_limit`] is set the run is stopped and ends
    /// with [`RunStatus::OutputLimitExceeded`].
    Truncated {
        /// The limit that was hit.
        limit: u64,
    },
    /// The compile phase succeeded and the run stops here: the mode was
    /// [`crate::RunMode::CompileOnly`] or [`crate::RunMode::Debug`]. A debugger
    /// takes the program from this value.
    Compiled(Box<ProgramInfo>),
    /// The run is over. Nothing follows.
    Finished(RunSummary),
}

/// A compile stage that is starting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageInfo {
    /// Zero-based position.
    pub index: usize,
    /// How many stages there are (1 for a compile script).
    pub count: usize,
    /// The stage's label ("Compile", "Link").
    pub name: String,
    /// The command line, for display.
    pub command: String,
}

/// A compile stage that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutcome {
    /// Zero-based position.
    pub index: usize,
    /// The stage's label.
    pub name: String,
    /// Whether the stage succeeded.
    pub success: bool,
    /// Its exit status, when it exited.
    pub exit_code: Option<i32>,
    /// The signal that killed it, if any.
    pub signal: Option<i32>,
    /// How long it took.
    pub wall: Duration,
}

/// A fully resolved program: what a run executes, or what a debugger is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramInfo {
    /// The process id, once started (`None` in [`RunEvent::Compiled`]).
    pub pid: Option<u32>,
    /// The argv that is (or would be) executed, after expansion, with the
    /// user's arguments. For a shell command: `[shell, "-c", ...]`.
    pub argv: Vec<String>,
    /// The working directory.
    pub cwd: std::path::PathBuf,
    /// The complete environment of the program.
    pub env: std::collections::BTreeMap<String, String>,
    /// The value of `$compiler` after the compile phase (the executable, the
    /// jar, `dir:Class`), when there is one.
    pub compiler: Option<String>,
    /// The source file that is run.
    pub file: std::path::PathBuf,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    /// The program exited with this status (0 = success).
    Exit(i32),
    /// The program was killed by a signal that the engine did not send.
    Signal(i32),
    /// Stopped by the user ([`crate::RunHandle::cancel`]).
    Cancelled,
    /// Stopped because it ran past the timeout.
    TimedOut,
    /// Stopped because of too much output ([`RunEvent::Truncated`]).
    OutputLimitExceeded,
    /// A compile stage failed.
    CompileFailed {
        /// The stage's label.
        stage: String,
        /// Its exit status, when it exited.
        exit_code: Option<i32>,
        /// The signal that killed it, if any.
        signal: Option<i32>,
    },
    /// Compile-only or debug preparation succeeded; no program ran.
    Compiled,
    /// The run could not start (missing tool, bad command, ...).
    Error(RunError),
}

impl RunStatus {
    /// Whether the run succeeded (`Exit(0)` or `Compiled`).
    pub fn is_success(&self) -> bool {
        matches!(self, RunStatus::Exit(0) | RunStatus::Compiled)
    }

    /// The program's exit code, if it exited.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            RunStatus::Exit(code) => Some(*code),
            _ => None,
        }
    }
}

/// The result of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// The run.
    pub run: RunId,
    /// How it ended.
    pub status: RunStatus,
    /// The phase the run was in when it ended.
    pub phase: Phase,
    /// Wall-clock time of the last process (the program, or the failed stage).
    pub wall: Duration,
    /// Wall-clock time of the whole run, preparation and compilation included.
    pub total_wall: Duration,
    /// User CPU time of that process.
    pub cpu_user: Duration,
    /// System CPU time of that process.
    pub cpu_sys: Duration,
    /// Peak resident memory of that process, in bytes.
    pub peak_rss: u64,
    /// A message to show after the output: the language's installer hint after
    /// exit status 127, `Terminated due to signal: SIGSEGV (11)`, the Xcode
    /// license text after 69, ...
    pub message: Option<String>,
    /// Program output, in bytes (both streams), after the limit was applied.
    pub output_bytes: u64,
}

impl RunSummary {
    /// CodeRunner's status text for the end of the run ("Run Succeeded",
    /// "Compile Error", "Run Stopped", ...).
    pub fn status_text(&self) -> String {
        match &self.status {
            RunStatus::Exit(0) => "Run Succeeded".to_owned(),
            RunStatus::Exit(_) | RunStatus::Signal(_) => "Run Ended".to_owned(),
            RunStatus::Cancelled | RunStatus::TimedOut | RunStatus::OutputLimitExceeded => {
                match self.phase {
                    Phase::Compile => "Compile Stopped".to_owned(),
                    Phase::Run => "Run Stopped".to_owned(),
                }
            }
            RunStatus::CompileFailed { .. } => "Compile Error".to_owned(),
            RunStatus::Compiled => "Compile Succeeded".to_owned(),
            RunStatus::Error(RunError::InvalidRunCommand(_)) => "Invalid Run Command".to_owned(),
            RunStatus::Error(RunError::TempFile(_)) => {
                "Unable to write temp file. Save and try again.".to_owned()
            }
            RunStatus::Error(_) => "Run Failed".to_owned(),
        }
    }

    /// The status line shown while the run is in `phase`
    /// ([`RunPhase::status_text`]).
    pub fn phase_text(phase: Phase) -> &'static str {
        match phase {
            Phase::Compile => RunPhase::Compiling.status_text(),
            Phase::Run => RunPhase::Running.status_text(),
        }
    }
}

/// The name of a signal number (`SIGSEGV`), `SIG<n>` for unknown ones.
pub fn signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGSYS => "SIGSYS",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGUSR2 => "SIGUSR2",
        _ => return format!("SIG{signal}"),
    };
    name.to_owned()
}

/// CodeRunner's text for a program killed by a signal.
pub fn signal_message(signal: i32) -> String {
    format!(
        "Terminated due to signal: {} ({signal})",
        signal_name(signal)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(status: RunStatus, phase: Phase) -> RunSummary {
        RunSummary {
            run: RunId(1),
            status,
            phase,
            wall: Duration::ZERO,
            total_wall: Duration::ZERO,
            cpu_user: Duration::ZERO,
            cpu_sys: Duration::ZERO,
            peak_rss: 0,
            message: None,
            output_bytes: 0,
        }
    }

    #[test]
    fn run_ids_increase() {
        let a = RunId::next();
        let b = RunId::next();
        assert!(b > a);
        assert_eq!(a.to_string(), a.get().to_string());
    }

    #[test]
    fn status_texts_follow_coderunner() {
        assert_eq!(
            summary(RunStatus::Exit(0), Phase::Run).status_text(),
            "Run Succeeded"
        );
        assert_eq!(
            summary(RunStatus::Exit(3), Phase::Run).status_text(),
            "Run Ended"
        );
        assert_eq!(
            summary(RunStatus::Cancelled, Phase::Compile).status_text(),
            "Compile Stopped"
        );
        assert_eq!(
            summary(RunStatus::Cancelled, Phase::Run).status_text(),
            "Run Stopped"
        );
        assert_eq!(
            summary(RunStatus::Compiled, Phase::Compile).status_text(),
            "Compile Succeeded"
        );
        let failed = RunStatus::CompileFailed {
            stage: "Compile".into(),
            exit_code: Some(1),
            signal: None,
        };
        assert_eq!(
            summary(failed, Phase::Compile).status_text(),
            "Compile Error"
        );
        assert!(RunStatus::Compiled.is_success());
        assert!(!RunStatus::Exit(1).is_success());
    }

    #[test]
    fn signal_texts() {
        assert_eq!(
            signal_message(libc::SIGSEGV),
            "Terminated due to signal: SIGSEGV (11)"
        );
        assert_eq!(signal_name(99), "SIG99");
    }
}
