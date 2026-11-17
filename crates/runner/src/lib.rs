//! ScratchRunner's run engine (NSH-free).
//!
//! Turns "run this document" into processes and a stream of events: it plans
//! the run from a [`scratch_language::LanguageDef`] (compile stages, run
//! command), builds CodeRunner's environment contract, executes the program on a
//! pty with stderr on its own pipe, and reports output, stage progress and the
//! final status, time, CPU and peak memory.
//!
//! # Main types
//!
//! | Item | Purpose |
//! |--|--|
//! | [`RunRequest`] | language + [`Snapshot`] (saved path or unsaved text) + [`RunSettings`] (per-file) + [`ShellSettings`] (global) + [`RunOptions`] + [`RunMode`] |
//! | [`start`], [`start_channel`] | start one run; events go to a closure or a `std::sync::mpsc` channel |
//! | [`Runner`] | tracks one run per document, [`OnBusy`] policy, stale-run detection |
//! | [`RunHandle`] | `cancel()`, `write_stdin()`, `send_eof()`, `resize()`, `interrupt()`, `signal()`, `allow_large_output()`, `sample_stats()` |
//! | [`RunMessage`] = [`RunId`] + `seq` + [`RunEvent`] | `Preparing`, `StageStarted/Finished`, `Output{phase, stream, bytes}`, `Started`, `LargeOutput`, `Truncated`, `Compiled`, `Finished` |
//! | [`RunSummary`], [`RunStatus`] | `Exit(code)`, `Signal`, `Cancelled`, `TimedOut`, `OutputLimitExceeded`, `CompileFailed{stage..}`, `Compiled`, `Error(`[`RunError`]`)` |
//! | [`process`] | [`process::Process`] (pty/pipe spawn with exact exit stats), [`process::PtyConsole`] (a pty without a child, for debuggers) |
//! | [`warm_up`] | compute the automatic `PATH` and developer directory off the UI thread at launch |
//! | [`path`] | automatic `PATH` (login shell + `/etc/paths*`, cached, `refresh`), `find_program` |
//! | [`preflight`] | developer tools / `xcrun` shims, Python resolution, missing-tool errors |
//! | [`build_environment`] | the `CR_*` environment layers |
//! | [`Utf8Decoder`], [`OutputGuard`], [`TempRun`] | helpers (decode split characters, output cap, run directory) |
//!
//! # Example
//!
//! ```
//! use scratch_language::LanguageRegistry;
//! use scratch_runner::*;
//!
//! let registry = LanguageRegistry::builtin();
//! let language = registry.get("shell").expect("bundled");
//!
//! let mut shell = ShellSettings::default();
//! shell.path = PathSetting::Custom("/usr/bin:/bin".into()); // skip the login-shell probe here
//! let request = RunRequest::new(language, Snapshot::text("echo hello; echo oops >&2; exit 3\n"))
//!     .with_shell(shell)
//!     .with_owner("document-1", OnBusy::Reject);
//!
//! let runner = Runner::new();
//! let (handle, events) = runner.start_channel(request).expect("thread");
//! let (mut out, mut err) = (Vec::new(), Vec::new());
//! let summary = loop {
//!     let message = events.recv().expect("events end with Finished");
//!     assert_eq!(message.run, handle.id()); // a UI drops messages of other runs
//!     match message.event {
//!         RunEvent::Output { stream: Stream::Stdout, bytes, .. } => out.extend(bytes),
//!         RunEvent::Output { stream: Stream::Stderr, bytes, .. } => err.extend(bytes),
//!         RunEvent::Finished(summary) => break summary,
//!         _ => {}
//!     }
//! };
//! assert_eq!(summary.status, RunStatus::Exit(3));
//! assert!(String::from_utf8_lossy(&out).contains("hello"));
//! assert!(String::from_utf8_lossy(&err).contains("oops"));
//! ```
//!
//! # The pipeline of one run
//!
//! 1. **Prepare.** A unique run directory (`$TMPDIR/ScratchRunner/run-XXXXXX`,
//!    `CR_TMPDIR`, removed afterwards) is made. A [`Snapshot::SavedFile`] is run
//!    in place (the app saves first); a [`Snapshot::Text`] is written to
//!    `<run dir>/src/<name>.<ext>` (extension of the language; Java is named after
//!    its main class, Swift is `main.swift`). The working directory follows
//!    [`WorkingDirectory`]. [`scratch_language::LanguageDef::plan_run`] expands the
//!    language's [`scratch_language::RunSpec`] with the Run Settings overrides
//!    (command, flags, arguments). Program Input text is written to
//!    `cr_input_<run>.txt`.
//! 2. **Compile** ([`RunEvent::StageStarted`] / [`RunEvent::StageFinished`]).
//!    Declarative steps run as plain processes (never through a shell), `via`
//!    wrappers (`xcrun`) are used when installed; a user compile script follows
//!    CodeRunner's protocol ([`scratch_language::protocol`]): cwd = source
//!    directory, `$1..$n` = flags, `CR_*` environment, exit 0 = success with the
//!    last stdout line as `$compiler`, 127 = installer hint, 69 = Xcode licence;
//!    its stdout is shown only on failure. Compile output is
//!    `Output{phase: Compile}`. In [`RunMode::CompileOnly`] and [`RunMode::Debug`]
//!    the run ends here with [`RunEvent::Compiled`] and
//!    [`RunStatus::Compiled`]; the [`ProgramInfo`] (argv, cwd, env, `$compiler`)
//!    is what a debugger needs.
//! 3. **Preflight.** The run command is expanded
//!    ([`scratch_language::resolve_command`]) to an argv or to
//!    `<shell> [-l] [shell args] -c <command> "$@"`; programs are looked up on the
//!    final `PATH`; missing tools give [`RunError::MissingTool`] with the
//!    language's installer hint; Xcode shims without a developer directory give
//!    [`RunError::DeveloperToolsMissing`] (nothing is started: it would open the
//!    system installer); `python`/`python3` are resolved to a real interpreter.
//! 4. **Run** ([`RunEvent::Started`]). Stdout is a pty (the program sees a tty,
//!    echo and `\r\n` as configured by [`Tty`], typed input through
//!    [`RunHandle::write_stdin`]), stderr is a pipe (exact stream tags, see
//!    [`RunOptions::merge_stderr`]). Program Input is redirected to stdin
//!    (`< file`) or exposed as `CR_INPUT`. The program leads its own session and
//!    process group.
//! 5. **Stop.** [`RunHandle::cancel`], the timeout and the output limit send
//!    SIGTERM to the whole group, SIGKILL after [`RunLimits::kill_grace`], and
//!    finally SIGKILL to anything left (so grandchildren die too).
//! 6. **Finish.** [`RunEvent::Finished`] carries the [`RunSummary`]: status, wall
//!    time, user/system CPU and peak RSS (from `wait4`), and a user message (the
//!    language's installer hint after exit 127, "Terminated due to signal: ...").
//!
//! # Concurrency model
//!
//! Every run has its own thread, process group, run directory and [`RunId`]. Any
//! number of runs execute at once (the app allows several documents running).
//! *One run per document* is enforced by [`Runner`]: give the request an owner
//! key ([`RunRequest::with_owner`]); a busy owner is rejected or its old run is
//! replaced ([`OnBusy::StopPrevious`]). Replaced runs still deliver their last
//! events, tagged with the old id: a consumer keeps the id of each document's
//! current run and ignores every [`RunMessage`] whose `run` differs
//! ([`Runner::is_current`]). Messages of one run arrive in order (`seq` counts up)
//! on the run's thread: the sink must be cheap (push into a channel / signal).
//!
//! # Large output
//!
//! [`RunLimits`]: after `warn_output_bytes` a [`RunEvent::LargeOutput`] is sent
//! once (and, with `pause_at_warning`, the program is blocked by back-pressure
//! until [`RunHandle::allow_large_output`] or [`RunHandle::cancel`]); past
//! `max_output_bytes` further output is dropped, [`RunEvent::Truncated`] (the
//! "N more characters not displayed" marker) is sent once and, with
//! `stop_at_limit`, the run is stopped ([`RunStatus::OutputLimitExceeded`]), so an
//! infinite print loop cannot hang the app.
//!
//! # Not in this crate
//!
//! No UI, no ANSI parsing (see `scratch-console-model`), no debugger protocol
//! (use [`process::Process`] / [`process::PtyConsole`]), no diagnostics parsing.

mod engine;
mod env;
mod error;
mod events;
mod handle;
mod output;
pub mod path;
pub mod preflight;
pub mod process;
mod request;
mod runner;
mod settings;
mod stats;
mod temp;

pub use env::{EnvironmentInputs, INHERITED_VARIABLES, build_environment, inherited_environment};
pub use error::{InputError, RunError, StartError};
pub use events::{
    Phase, ProgramInfo, RunEvent, RunId, RunMessage, RunStatus, RunSummary, StageInfo,
    StageOutcome, Stream, signal_message, signal_name,
};
pub use handle::RunHandle;
pub use output::{Admit, OutputGuard, Utf8Decoder};
pub use path::{
    PathProbeError, augmented_path, automatic_path, cached_automatic_path, compose_path,
    dedupe_path, determine_path, find_program, home_dir, login_shell, merge_paths,
    path_helper_paths, refresh_automatic_path,
};
pub use preflight::{developer_dir, refresh_developer_dir};
pub use process::Tty;
pub use request::{OnBusy, RunMode, RunRequest, Snapshot};
pub use runner::{Runner, start, start_channel};
pub use settings::{
    DEFAULT_CUSTOM_PATH, DeveloperDir, InheritEnvironment, PathSetting, ProgramInput, RunLimits,
    RunOptions, RunSettings, ShellSettings, WorkingDirectory,
};
pub use stats::{ProcessStats, sample_tree, sum_tree};
pub use temp::{TEMP_SUBDIRECTORY, TempRun, sweep_stale_runs};

/// Does the slow one-time work of the first run in the background: asks the
/// login shell for the automatic `PATH` ([`automatic_path`]) and detects the
/// developer directory ([`developer_dir`]). Call it once at launch.
pub fn warm_up() -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| {
        let _ = automatic_path();
        let _ = developer_dir();
    })
}
