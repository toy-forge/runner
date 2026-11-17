# scratch-runner

ScratchRunner's run engine. NSH-free (no UI dependency, fast `cargo test`); depends only on
`scratch-language`. The crate-level docs (`cargo doc -p scratch-runner --open`) have the full
description; this is the short version.

```rust
let runner = Runner::new();                       // one per app
let request = RunRequest::new(&language_def, Snapshot::saved("/work/main.c"))
    .with_settings(run_settings)                  // per-file: command, flags, arguments, input, env, cwd, timeout
    .with_shell(shell_prefs)                      // Preferences > Shell
    .with_owner("doc-42", OnBusy::Reject);        // one run per document
let (handle, events) = runner.start_channel(request)?;   // or runner.start(request, |msg| ...)
// events: std::sync::mpsc::Receiver<RunMessage>; no async runtime needed
handle.write_stdin(b"typed text\n")?;  handle.resize(120, 30)?;  handle.cancel();
```

## Public API

| Item | Notes |
|--|--|
| `RunRequest::new(&LanguageDef, Snapshot)` + `with_mode/settings/shell/options/owner/project_folder` | fields are public too |
| `Snapshot::{SavedFile{path}, Text{text, file_name, directory}}` | saved files run in place; text goes to `<run dir>/src/<name>.<ext>` |
| `RunMode::{Run, CompileOnly, Debug, RunWithoutDebugging}` | `CompileOnly`/`Debug` stop after compiling with `RunEvent::Compiled(ProgramInfo)` |
| `RunSettings` | `run_command`, `compile_flags`, `arguments` (raw, `argument_list()` previews), `input: ProgramInput::{Text, File}`, `input_via_environment`, `environment`, `working_directory: WorkingDirectory::{FileDirectory, ProjectFolder, Default, Custom}`, `timeout` |
| `ShellSettings` | `shell` (`/bin/bash`), `login_shell`, `path: PathSetting::{Automatic, Custom}`, `variables` (`LC_CTYPE`, `TERM=dumb`, `TERM_PROGRAM`), `shell_arguments`, `compile_shell_arguments`, `compiler_output_in_temp_dir`; serde |
| `RunOptions` | `limits: RunLimits`, `tty: Tty`, `merge_stderr`, `kill_leftover_processes`, `temp_root`, `keep_temp`, `developer_dir`, `inherit_environment` |
| `Runner::{start, start_channel, active, handle, is_current, cancel, cancel_all, active_count}` | one run per owner; `OnBusy::{Reject, StopPrevious}` |
| `start`, `start_channel` | untracked runs |
| `RunHandle::{id, pid, is_finished, cancel, write_stdin, send_eof, resize, interrupt, signal, allow_large_output, sample_stats}` | never blocks |
| `RunMessage{run: RunId, seq, event: RunEvent}` | `Preparing`, `StageStarted(StageInfo)`, `StageFinished(StageOutcome)`, `Output{phase, stream, bytes}`, `Started(ProgramInfo)`, `LargeOutput`, `Truncated`, `Compiled(ProgramInfo)`, `Finished(RunSummary)` |
| `RunSummary{status, phase, wall, total_wall, cpu_user, cpu_sys, peak_rss, message, output_bytes}` + `status_text()` | CodeRunner's status strings |
| `RunStatus::{Exit, Signal, Cancelled, TimedOut, OutputLimitExceeded, CompileFailed, Compiled, Error(RunError)}` | |
| `RunError` | `MissingTool`, `DeveloperToolsMissing`, `PythonMissing`, `NoRunCommand`, `InvalidRunCommand`, `InvalidArguments`, `WorkingDirectory`, `SourceMissing`, `TempFile`, `InputFile`, `Plan`, `Spawn`, `Internal` |
| `process::{Process, ProcessConfig, ProcessEvent, ProcessExit, Tty, StdinSource, PtyConsole}` | the pty/pipe spawn the debugger crate reuses |
| `path::*` | `automatic_path`, `refresh_automatic_path`, `determine_path`, `path_helper_paths`, `dedupe_path`, `compose_path`, `find_program`, `augmented_path`, `login_shell`, `home_dir` |
| `preflight::*`, `developer_dir`, `refresh_developer_dir` | `is_xcode_shim`, `resolve_python`, `developer_tools_message` |
| `build_environment`, `inherited_environment` | the environment contract (see module docs of `env`) |
| `Utf8Decoder`, `OutputGuard`, `TempRun`, `sweep_stale_runs`, `ProcessStats`, `warm_up` | helpers |

## Behaviour worth knowing

* **Pty on stdout, pipe on stderr.** The program sees a tty on stdout (and stdin, unless Program Input
  redirects it); stream tags are exact. Python's `input(prompt)` writes the prompt to *stderr* when stdin is a
  terminal, so with separate streams that prompt is tagged stderr; set `RunOptions::merge_stderr` to put
  both on the pty (CodeRunner's behaviour, tags lost).
* **Environment** is built from a whitelist, not the app's environment: `PATH` (automatic from the login shell
  + `/etc/paths*`, deduped, cached until Refresh; or custom; plus the language's `path_prepend/append`),
  `DEVELOPER_DIR`, Shell Variables, `CR_*`, per-run variables (cannot override `CR_*`).
* **Stopping** sends SIGTERM to the process group, SIGKILL after `kill_grace` (2 s), then SIGKILL to whatever is
  left: grandchildren die with the program.
* **Large output**: `LargeOutput` once at 16 MiB (optionally pausing the program until the user answers),
  drop + `Truncated` + stop at 64 MiB (all configurable in `RunLimits`).
* **Xcode shims** (`/usr/bin/clang`, `python3`, `git`, `xcrun`...) are never started without a developer
  directory: the run ends with `RunError::DeveloperToolsMissing` and the `xcode-select --install` guidance.
* **Concurrency**: every run has its own thread, process group and temp directory; `Runner` enforces one run
  per document; stale events are recognised by `RunMessage::run`.

## Tests

`cargo test -p scratch-runner`: unit tests per module, `tests/run_basic.rs` (python3 / bash: tagging, tty,
stdin echo, input redirect, env contract, cancel kills grandchildren, SIGTERM-ignoring programs, timeout,
large output, flood, login mode, PATH, working directories, one run per document, ...) and `tests/compile.rs`
(clang, rustc, go, node, java, swift, compile errors, compile-only, debug mode, compile script protocol,
shim refusal). Tests whose toolchain is missing print `SKIP:` and return.

## Not done

* Live "Time/CPU/Memory" is `RunHandle::sample_stats()` (one `ps` per call); no push stream.
* The installer flow of CodeRunner (`installer.sh` y/n in the console) is replaced by a message
  (`RunSummary::message`, `RunError::MissingTool`).
* Output decoding to non-UTF-8 console encodings; `CR_ENCODING` is exported but the bytes are passed through.
* Parent/child runs for web-resource consoles (G27) belong to the app layer.
* `MACOSX_DEPLOYMENT_TARGET` is not set by default (CodeRunner sets 10.13); `ShellSettings::deployment_target`.
