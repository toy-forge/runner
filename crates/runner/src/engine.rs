//! The run pipeline, executed on one thread per run:
//!
//! ```text
//! Preparing  temp dir, source file (unsaved text is written), working directory,
//!            PATH and developer tools, plan (scratch_language), Program Input file
//! Compile    declarative steps, or the compile script (CodeRunner's protocol)
//! Resolve    run command -> argv (direct, or `<shell> [-l] -c` when it needs a shell)
//! Run        pty for stdout, pipe for stderr, timeout, cancel, output guard
//! Finished   RunSummary: status, wall time, CPU, peak memory, hint
//! ```

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use scratch_language::{
    CompileExit, CompilePlan, LanguageDef, PlanError, ResolvedCommand, RunContext, RunPlan, Vars,
    java_main_class, parse_compiler_output,
};

use crate::env::{EnvironmentInputs, build_environment, inherited_environment};
use crate::handle::{Control, Inbound, RunHandle};
use crate::output::OutputGuard;
use crate::path::{compose_path, find_program, home_dir};
use crate::preflight::{
    developer_dir, developer_tools_message, is_xcode_shim, resolve_run_program, resolve_step_argv,
};
use crate::process::{Process, ProcessConfig, ProcessEvent, ProcessExit, StdinSource};
use crate::temp::TempRun;
use crate::{
    DeveloperDir, Phase, ProgramInfo, ProgramInput, RunError, RunEvent, RunId, RunLimits,
    RunMessage, RunMode, RunRequest, RunStatus, RunSummary, Snapshot, StageInfo, StageOutcome,
    StartError, Stream, WorkingDirectory, events,
};

pub(crate) type Sink = Arc<dyn Fn(RunMessage) + Send + Sync>;

/// How long the engine sleeps at most between checks of its flags.
const TICK: Duration = Duration::from_millis(250);

/// Starts the run thread.
pub(crate) fn spawn(request: RunRequest, sink: Sink) -> Result<RunHandle, StartError> {
    let id = RunId::next();
    let (tx, rx) = mpsc::channel();
    let size = (request.options.tty.cols, request.options.tty.rows);
    let control = Arc::new(Control::new(id, tx, size));
    let engine = Engine {
        run: id,
        ctl: control.clone(),
        rx,
        sink,
        seq: 0,
        generation: 0,
        started: Instant::now(),
        guard: OutputGuard::new(
            request.options.limits.warn_output_bytes,
            request.options.limits.max_output_bytes,
        ),
        limits: request.options.limits.clone(),
        kill_leftovers: request.options.kill_leftover_processes,
        limits_lifted: false,
        last: LastProcess::default(),
        phase: Phase::Compile,
    };
    std::thread::Builder::new()
        .name(format!("run-{id}"))
        .spawn(move || engine.run(request))?;
    Ok(RunHandle { control })
}

#[derive(Debug, Clone, Copy, Default)]
struct LastProcess {
    wall: Duration,
    cpu_user: Duration,
    cpu_sys: Duration,
    peak_rss: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    TimedOut,
    OutputLimit,
}

struct Supervised {
    exit: ProcessExit,
    stop: Option<StopReason>,
}

struct Engine {
    run: RunId,
    ctl: Arc<Control>,
    rx: Receiver<Inbound>,
    sink: Sink,
    seq: u64,
    generation: u64,
    started: Instant,
    guard: OutputGuard,
    limits: RunLimits,
    kill_leftovers: bool,
    limits_lifted: bool,
    last: LastProcess,
    phase: Phase,
}

/// What a finished compile phase hands on.
enum CompileOutcome {
    /// Stdout of the compile script (for `$compiler`), if the language has one.
    Done(Option<String>),
    /// The run ended during compilation.
    Ended(RunSummary),
}

/// Everything resolved while preparing.
struct Context<'a> {
    req: &'a RunRequest,
    plan: RunPlan,
    path: String,
    developer_tools: Option<PathBuf>,
    src_dir: PathBuf,
    file: PathBuf,
    cwd: PathBuf,
    temp_dir: PathBuf,
    input_file: Option<PathBuf>,
}

impl Context<'_> {
    /// The complete environment of a process of this run. `extra` is the step's
    /// own environment (language `env` and step `env`).
    fn environment(&self, extra: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut plan_env = self.plan.process_env();
        plan_env.extend(extra.clone());
        let input = if self.req.settings.input_via_environment {
            self.input_file.as_deref()
        } else {
            None
        };
        build_environment(&EnvironmentInputs {
            shell: &self.req.shell,
            inherit: self.req.options.inherit_environment,
            path: &self.path,
            developer_dir: self.developer_tools.as_deref(),
            plan_env: &plan_env,
            run_env: &self.req.settings.environment,
            run_command: &self.plan.command,
            input_file: input,
            app_version: self.req.app_version.as_deref(),
            extra_env: &self.req.extra_env,
        })
    }
}

impl Engine {
    fn emit(&mut self, event: RunEvent) {
        let message = RunMessage {
            run: self.run,
            seq: self.seq,
            event,
        };
        self.seq += 1;
        (self.sink)(message);
    }

    fn run(mut self, request: RunRequest) {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| self.pipeline(&request)));
        let summary = match result {
            Ok(Ok(summary)) => summary,
            Ok(Err(error)) => self.summary(RunStatus::Error(error), None),
            Err(panic) => {
                let text = panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "the run thread panicked".to_owned());
                self.summary(RunStatus::Error(RunError::Internal(text)), None)
            }
        };
        self.ctl.mark_finished();
        self.emit(RunEvent::Finished(summary));
    }

    fn summary(&self, status: RunStatus, message: Option<String>) -> RunSummary {
        RunSummary {
            run: self.run,
            status,
            phase: self.phase,
            wall: self.last.wall,
            total_wall: self.started.elapsed(),
            cpu_user: self.last.cpu_user,
            cpu_sys: self.last.cpu_sys,
            peak_rss: self.last.peak_rss,
            message,
            output_bytes: self.guard.total(),
        }
    }

    fn cancelled(&self) -> RunSummary {
        self.summary(RunStatus::Cancelled, None)
    }

    // ---------------------------------------------------------------- pipeline

    fn pipeline(&mut self, req: &RunRequest) -> Result<RunSummary, RunError> {
        self.emit(RunEvent::Preparing);
        let language = &req.language;

        let mut temp = TempRun::create(req.options.temp_root.as_deref())
            .map_err(|e| RunError::TempFile(e.to_string()))?;
        let temp_dir = temp.dir().to_path_buf();

        let (src_dir, file, file_name, text_directory) = prepare_source(req, &temp_dir)?;
        let cwd = working_directory(req, &src_dir, text_directory.as_deref())?;
        if self.ctl.is_cancelled() {
            return Ok(self.cancelled());
        }

        // PATH and developer tools. The first automatic PATH asks a login shell.
        let base_path = req.shell.base_path();
        let developer_tools = match &req.options.developer_dir {
            DeveloperDir::Detect => developer_dir(),
            DeveloperDir::Assume(dir) => dir.clone(),
        };

        // The plan.
        let mut snapshot_env = inherited_environment(req.options.inherit_environment);
        if let Some(home) = home_dir() {
            snapshot_env
                .entry("HOME".to_owned())
                .or_insert_with(|| home.to_string_lossy().into_owned());
        }
        snapshot_env.extend(req.shell.variables.clone());
        snapshot_env.insert("PATH".to_owned(), base_path.clone());
        let mut vars = Vars::for_source(&src_dir, &file_name);
        if req.shell.compiler_output_in_temp_dir {
            vars = vars.with_tmpdir(&temp_dir);
        } else {
            vars.set("tmpdir", temp_dir.to_string_lossy().into_owned());
        }
        vars = vars.with_env(snapshot_env).with_runid(self.run.get());
        if let Some(dir) = &req.language_dir {
            vars = vars.with_language_dir(dir);
        }
        if let Some(dir) = &req.unsaved_dir {
            vars.set("unsaved_dir", dir.to_string_lossy().into_owned());
        }
        let mut context = RunContext::new(vars);
        context.compile_flags = req.settings.compile_flags.clone();
        context.run_command = req
            .settings
            .run_command
            .clone()
            .filter(|command| !command.trim().is_empty());
        context.arguments = req.settings.argument_list()?;
        context.debugging = req.mode == RunMode::Debug;
        let plan = language.plan_run(&context).map_err(plan_error)?;
        let path = compose_path(&plan.path_prepend, &base_path, &plan.path_append);

        // Program Input.
        let input_file = match &req.settings.input {
            None => None,
            Some(ProgramInput::File(path)) => {
                if !path.is_file() {
                    return Err(RunError::InputFile(format!(
                        "{} does not exist",
                        path.display()
                    )));
                }
                Some(path.clone())
            }
            Some(ProgramInput::Text(text)) => {
                let path = temp_dir.join(format!("cr_input_{}.txt", self.run));
                std::fs::write(&path, text).map_err(|e| RunError::InputFile(e.to_string()))?;
                Some(path)
            }
        };

        let ctx = Context {
            req,
            plan,
            path,
            developer_tools,
            src_dir,
            file,
            cwd,
            temp_dir,
            input_file,
        };

        // Compile.
        let compiler_stdout = match self.compile(&ctx)? {
            CompileOutcome::Done(stdout) => stdout,
            CompileOutcome::Ended(summary) => return Ok(summary),
        };
        if self.ctl.is_cancelled() {
            return Ok(self.cancelled());
        }

        if matches!(req.mode, RunMode::CompileOnly | RunMode::Debug) {
            // Best effort for compile-only: a language without a run command still
            // compiles. A debug run needs the command.
            let resolved = self.resolve_run(&ctx, compiler_stdout.as_deref());
            let info = match resolved {
                Ok(info) => info,
                Err(RunError::NoRunCommand { .. }) if req.mode == RunMode::CompileOnly => {
                    self.program_info(&ctx, Vec::new(), compiler_stdout.as_deref())
                }
                Err(error) => return Err(error),
            };
            if req.options.keep_temp {
                temp.keep();
            }
            self.emit(RunEvent::Compiled(Box::new(info)));
            return Ok(self.summary(RunStatus::Compiled, None));
        }

        // Run.
        if req.options.keep_temp {
            temp.keep();
        }
        let info = self.resolve_run(&ctx, compiler_stdout.as_deref())?;
        self.run_program(&ctx, info)
    }

    // ----------------------------------------------------------------- compile

    fn compile(&mut self, ctx: &Context<'_>) -> Result<CompileOutcome, RunError> {
        let req = ctx.req;
        let timeout = req.options.limits.compile_timeout;
        match &ctx.plan.compile {
            CompilePlan::None => Ok(CompileOutcome::Done(None)),
            CompilePlan::Steps(steps) => {
                self.phase = Phase::Compile;
                let count = steps.len();
                for (index, step) in steps.iter().enumerate() {
                    if self.ctl.is_cancelled() {
                        return Ok(CompileOutcome::Ended(self.cancelled()));
                    }
                    for dir in &step.mkdirs {
                        std::fs::create_dir_all(dir)
                            .map_err(|e| RunError::TempFile(format!("{}: {e}", dir.display())))?;
                    }
                    let argv = resolve_step_argv(
                        step,
                        &req.language,
                        &ctx.path,
                        ctx.developer_tools.is_some(),
                    )?;
                    let mut config = ProcessConfig::piped(argv);
                    config.cwd = Some(step.cwd.clone());
                    config.env = ctx.environment(&step.env);
                    self.emit(RunEvent::StageStarted(StageInfo {
                        index,
                        count,
                        name: stage_name(&step.name, &step.program),
                        command: step.display(),
                    }));
                    let supervised = self.supervise(Supervision {
                        config,
                        phase: Phase::Compile,
                        timeout,
                        collect_stdout: None,
                        announce: None,
                    })?;
                    if let Some(summary) = self.finish_stage(
                        index,
                        &stage_name(&step.name, &step.program),
                        &supervised,
                        step.missing_tool.as_deref(),
                        req,
                    ) {
                        return Ok(CompileOutcome::Ended(summary));
                    }
                    if let Some(produces) = &step.produces
                        && produces.must_exist
                    {
                        if !Path::new(&produces.value).exists() {
                            let message = produces
                                .missing_message
                                .clone()
                                .unwrap_or_else(|| format!("{} was not produced.", produces.value));
                            self.emit(RunEvent::Output {
                                phase: Phase::Compile,
                                stream: Stream::Stderr,
                                bytes: format!("{message}\n").into_bytes(),
                            });
                            return Ok(CompileOutcome::Ended(self.summary(
                                RunStatus::CompileFailed {
                                    stage: stage_name(&step.name, &step.program),
                                    exit_code: Some(0),
                                    signal: None,
                                },
                                Some(message),
                            )));
                        }
                        // A `pdf` console finds the file it renders by the path
                        // its producing step leaves in the output.
                        if req.language.console.kind == scratch_language::ConsoleType::Pdf {
                            self.emit(RunEvent::Output {
                                phase: Phase::Compile,
                                stream: Stream::Stdout,
                                bytes: format!("{}\n", produces.value).into_bytes(),
                            });
                        }
                    }
                }
                Ok(CompileOutcome::Done(None))
            }
            CompilePlan::Shell(shell_plan) => {
                self.phase = Phase::Compile;
                let shell = find_program(&req.shell.shell, &ctx.path).ok_or_else(|| {
                    RunError::MissingTool {
                        program: req.shell.shell.clone(),
                        message: format!("The shell `{}` was not found.", req.shell.shell),
                    }
                })?;
                let script = ctx.temp_dir.join("compile.sh");
                std::fs::write(&script, &shell_plan.script)
                    .map_err(|e| RunError::TempFile(e.to_string()))?;
                let mut argv = vec![shell.to_string_lossy().into_owned()];
                argv.extend(req.shell.compile_shell_arguments.iter().cloned());
                argv.push(script.to_string_lossy().into_owned());
                argv.extend(shell_plan.args.iter().cloned());
                let mut config = ProcessConfig::piped(argv.clone());
                config.cwd = Some(shell_plan.cwd.clone());
                config.env = ctx.environment(&BTreeMap::new());
                self.emit(RunEvent::StageStarted(StageInfo {
                    index: 0,
                    count: 1,
                    name: "Compile".to_owned(),
                    command: format!("{} compile.sh", req.shell.shell),
                }));
                let mut stdout = Vec::new();
                let supervised = self.supervise(Supervision {
                    config,
                    phase: Phase::Compile,
                    timeout,
                    collect_stdout: Some(&mut stdout),
                    announce: None,
                })?;
                let failed = supervised.stop.is_some() || supervised.exit.code != Some(0);
                if failed && !stdout.is_empty() && supervised.stop.is_none() {
                    // On failure the script's stdout is shown as error text.
                    self.emit(RunEvent::Output {
                        phase: Phase::Compile,
                        stream: Stream::Stderr,
                        bytes: stdout.clone(),
                    });
                }
                if let Some(summary) = self.finish_stage(0, "Compile", &supervised, None, req) {
                    return Ok(CompileOutcome::Ended(summary));
                }
                Ok(CompileOutcome::Done(Some(
                    String::from_utf8_lossy(&stdout).into_owned(),
                )))
            }
        }
    }

    /// Emits `StageFinished` and, when the stage did not succeed, returns the
    /// run's summary.
    fn finish_stage(
        &mut self,
        index: usize,
        name: &str,
        supervised: &Supervised,
        missing_tool: Option<&str>,
        req: &RunRequest,
    ) -> Option<RunSummary> {
        let exit = &supervised.exit;
        let success = supervised.stop.is_none() && exit.code == Some(0);
        self.emit(RunEvent::StageFinished(StageOutcome {
            index,
            name: name.to_owned(),
            success,
            exit_code: exit.code,
            signal: exit.signal,
            wall: exit.wall,
        }));
        if success {
            return None;
        }
        match supervised.stop {
            Some(StopReason::Cancelled) => return Some(self.cancelled()),
            Some(StopReason::TimedOut) => {
                return Some(self.summary(RunStatus::TimedOut, None));
            }
            Some(StopReason::OutputLimit) => {
                return Some(self.summary(RunStatus::OutputLimitExceeded, None));
            }
            None => {}
        }
        let message = match exit.code.map(CompileExit::from_status) {
            Some(CompileExit::CommandNotFound) => missing_tool
                .map(str::to_owned)
                .or_else(|| req.language.exit_status_message(127)),
            Some(CompileExit::XcodeLicense) => {
                CompileExit::XcodeLicense.message().map(str::to_owned)
            }
            Some(CompileExit::NeedsFileAccess) => {
                CompileExit::NeedsFileAccess.message().map(str::to_owned)
            }
            Some(CompileExit::Failed(code)) => req.language.errors.get(&code.to_string()).cloned(),
            _ => exit.signal.map(events::signal_message),
        };
        Some(self.summary(
            RunStatus::CompileFailed {
                stage: name.to_owned(),
                exit_code: exit.code,
                signal: exit.signal,
            },
            message,
        ))
    }

    // ------------------------------------------------------------- run command

    fn program_info(
        &self,
        ctx: &Context<'_>,
        argv: Vec<String>,
        compiler_stdout: Option<&str>,
    ) -> ProgramInfo {
        let compiler = compiler_stdout
            .and_then(parse_compiler_output)
            .or_else(|| ctx.plan.vars.get_str("compiler").filter(|c| !c.is_empty()));
        ProgramInfo {
            pid: None,
            argv,
            cwd: ctx.cwd.clone(),
            env: ctx.environment(&BTreeMap::new()),
            compiler,
            file: ctx.file.clone(),
        }
    }

    /// Expands the run command into the argv to start (preflight included).
    fn resolve_run(
        &self,
        ctx: &Context<'_>,
        compiler_stdout: Option<&str>,
    ) -> Result<ProgramInfo, RunError> {
        let req = ctx.req;
        let command = ctx.plan.command.trim();
        if command.is_empty() {
            return Err(RunError::NoRunCommand {
                renderer: req.language.console.renderer.clone(),
            });
        }
        let mut vars = ctx.plan.vars.clone();
        if let Some(stdout) = compiler_stdout
            && let Some(compiler) = parse_compiler_output(stdout)
        {
            vars.set_compiler(&compiler);
        }
        if ctx.cwd != ctx.src_dir {
            // The program does not run next to the file: name it by full path.
            vars.set("filename", ctx.file.to_string_lossy().into_owned());
        }
        let resolved = scratch_language::resolve_command(command, &vars)
            .map_err(|e| RunError::InvalidRunCommand(e.to_string()))?;
        let tools = ctx.developer_tools.is_some();
        let mut argv =
            resolved.to_argv(&req.shell.shell, req.shell.login_shell, &ctx.plan.arguments);
        match &resolved {
            ResolvedCommand::Argv(_) => {
                let program = resolve_run_program(&argv[0], &req.language, &ctx.path, tools)?;
                argv[0] = program.to_string_lossy().into_owned();
            }
            ResolvedCommand::Shell(_) => {
                let shell = find_program(&req.shell.shell, &ctx.path).ok_or_else(|| {
                    RunError::MissingTool {
                        program: req.shell.shell.clone(),
                        message: format!("The shell `{}` was not found.", req.shell.shell),
                    }
                })?;
                if !tools
                    && let Some(word) = resolved.program()
                    && !word.contains('/')
                    && let Some(found) = find_program(&word, &ctx.path)
                    && is_xcode_shim(&found)
                {
                    return Err(RunError::DeveloperToolsMissing {
                        message: developer_tools_message(&word),
                        program: word,
                    });
                }
                let insert_at = 1 + usize::from(req.shell.login_shell);
                for (offset, extra) in req.shell.shell_arguments.iter().enumerate() {
                    argv.insert(insert_at + offset, extra.clone());
                }
                argv[0] = shell.to_string_lossy().into_owned();
            }
        }
        let mut info = self.program_info(ctx, argv, compiler_stdout);
        // The environment of the program sees the filename the command used.
        if ctx.cwd != ctx.src_dir {
            info.env.insert(
                "filename".to_owned(),
                ctx.file.to_string_lossy().into_owned(),
            );
        }
        Ok(info)
    }

    // --------------------------------------------------------------------- run

    fn run_program(
        &mut self,
        ctx: &Context<'_>,
        info: ProgramInfo,
    ) -> Result<RunSummary, RunError> {
        let req = ctx.req;
        self.phase = Phase::Run;
        let mut config = ProcessConfig::pty(info.argv.clone());
        config.cwd = Some(info.cwd.clone());
        config.env = info.env.clone();
        let mut tty = req.options.tty.clone();
        if let Some((cols, rows)) = self.ctl.size() {
            tty.cols = cols;
            tty.rows = rows;
        }
        // TERM comes from the Shell Variables; keep the kernel's idea of the
        // terminal consistent with it.
        config.tty = Some(tty);
        config.merge_stderr = req.options.merge_stderr;
        config.stdin = match (&ctx.input_file, req.settings.input_via_environment) {
            (Some(path), false) => StdinSource::File(path.clone()),
            _ => StdinSource::Tty,
        };
        let supervised = self.supervise(Supervision {
            config,
            phase: Phase::Run,
            timeout: req.settings.timeout,
            collect_stdout: None,
            announce: Some(info),
        })?;
        let exit = &supervised.exit;
        let status_message = |code: i32| req.language.exit_status_message(code);
        Ok(match supervised.stop {
            Some(StopReason::Cancelled) => self.summary(RunStatus::Cancelled, None),
            Some(StopReason::TimedOut) => self.summary(RunStatus::TimedOut, None),
            Some(StopReason::OutputLimit) => self.summary(RunStatus::OutputLimitExceeded, None),
            None => match (exit.signal, exit.code) {
                (Some(signal), _) => self.summary(
                    RunStatus::Signal(signal),
                    Some(events::signal_message(signal)),
                ),
                (None, Some(code)) => {
                    let message = if code == 0 {
                        None
                    } else {
                        status_message(code)
                    };
                    self.summary(RunStatus::Exit(code), message)
                }
                (None, None) => self.summary(RunStatus::Exit(-1), None),
            },
        })
    }

    // -------------------------------------------------------------- supervision

    /// Starts a process and drives it to its end: forwards output, enforces the
    /// timeout, the cancel flag and the output guard, escalates SIGTERM to
    /// SIGKILL.
    fn supervise(&mut self, supervision: Supervision<'_>) -> Result<Supervised, RunError> {
        let Supervision {
            config,
            phase,
            timeout,
            mut collect_stdout,
            announce,
        } = supervision;
        let limits = self.limits.clone();
        let program = config.argv.first().cloned().unwrap_or_default();
        self.generation += 1;
        let generation = self.generation;
        let sender = self.ctl.sender();
        let process = Process::spawn(config, move |event| {
            let _ = sender.send(Inbound::Proc { generation, event });
        })
        .map_err(|error| RunError::Spawn {
            program,
            error: error.to_string(),
        })?;
        let process = Arc::new(process);
        if phase == Phase::Run {
            self.ctl.attach(process.clone());
        }
        if let Some(mut info) = announce {
            info.pid = Some(process.pid());
            self.emit(RunEvent::Started(info));
        }

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut stop: Option<StopReason> = None;
        let mut kill_at: Option<Instant> = None;
        let exit = loop {
            let now = Instant::now();
            if !self.limits_lifted && self.ctl.large_output_allowed() {
                // The user chose to continue: no limit, and resume a paused program.
                self.limits_lifted = true;
                self.guard.set_limits(None, u64::MAX);
                process.pause_output(false);
            }
            if stop.is_none() && self.ctl.is_cancelled() {
                begin_stop(
                    &process,
                    &mut stop,
                    &mut kill_at,
                    StopReason::Cancelled,
                    limits.kill_grace,
                );
            }
            if stop.is_none() && deadline.is_some_and(|d| now >= d) {
                begin_stop(
                    &process,
                    &mut stop,
                    &mut kill_at,
                    StopReason::TimedOut,
                    limits.kill_grace,
                );
            }
            if kill_at.is_some_and(|at| now >= at) {
                let _ = process.signal(libc::SIGKILL);
                kill_at = None;
            }
            let mut wait = TICK;
            for at in [deadline.filter(|_| stop.is_none()), kill_at]
                .into_iter()
                .flatten()
            {
                wait = wait.min(at.saturating_duration_since(now));
            }
            match self.rx.recv_timeout(wait) {
                Ok(Inbound::Proc {
                    generation: g,
                    event,
                }) if g == generation => match event {
                    ProcessEvent::Stdout(bytes) => {
                        if let Some(reason) =
                            self.output(phase, Stream::Stdout, bytes, &mut collect_stdout, &process)
                        {
                            begin_stop(
                                &process,
                                &mut stop,
                                &mut kill_at,
                                reason,
                                limits.kill_grace,
                            );
                        }
                    }
                    ProcessEvent::Stderr(bytes) => {
                        if let Some(reason) =
                            self.output(phase, Stream::Stderr, bytes, &mut None, &process)
                        {
                            begin_stop(
                                &process,
                                &mut stop,
                                &mut kill_at,
                                reason,
                                limits.kill_grace,
                            );
                        }
                    }
                    ProcessEvent::Exited(exit) => break exit,
                },
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RunError::Internal(
                        "the run's event channel closed".to_owned(),
                    ));
                }
            }
        };
        if phase == Phase::Run {
            self.ctl.detach();
        }
        if stop.is_some() || (phase == Phase::Run && self.kill_leftovers) {
            // Whatever ignored the terminate signal, and what the program left behind.
            let _ = process.signal(libc::SIGKILL);
        }
        self.last = LastProcess {
            wall: exit.wall,
            cpu_user: exit.cpu_user,
            cpu_sys: exit.cpu_sys,
            peak_rss: exit.max_rss,
        };
        Ok(Supervised { exit, stop })
    }

    /// Forwards one chunk of output through the guard. Returns the reason to stop
    /// the process, if the chunk caused one.
    fn output(
        &mut self,
        phase: Phase,
        stream: Stream,
        bytes: Vec<u8>,
        collect: &mut Option<&mut Vec<u8>>,
        process: &Process,
    ) -> Option<StopReason> {
        if let (Stream::Stdout, Some(buffer)) = (stream, collect.as_deref_mut()) {
            buffer.extend_from_slice(&bytes);
            return None;
        }
        let limits = self.limits.clone();
        let admit = self.guard.admit(bytes.len());
        let mut reason = None;
        if admit.keep > 0 {
            let kept = if admit.keep == bytes.len() {
                bytes
            } else {
                bytes[..admit.keep].to_vec()
            };
            self.emit(RunEvent::Output {
                phase,
                stream,
                bytes: kept,
            });
        }
        if admit.warn {
            self.emit(RunEvent::LargeOutput {
                bytes: self.guard.total(),
            });
            if limits.pause_at_warning {
                process.pause_output(true);
            }
        }
        if admit.truncated {
            self.emit(RunEvent::Truncated {
                limit: limits.max_output_bytes,
            });
            if limits.stop_at_limit {
                reason = Some(StopReason::OutputLimit);
            }
        }
        reason
    }
}

struct Supervision<'a> {
    config: ProcessConfig,
    phase: Phase,
    timeout: Option<Duration>,
    /// Keep stdout here instead of forwarding it (the compile script's
    /// `$compiler`).
    collect_stdout: Option<&'a mut Vec<u8>>,
    /// Emit `Started` with this info once the pid is known.
    announce: Option<ProgramInfo>,
}

fn begin_stop(
    process: &Process,
    stop: &mut Option<StopReason>,
    kill_at: &mut Option<Instant>,
    reason: StopReason,
    grace: Duration,
) {
    *stop = Some(reason);
    let _ = process.signal(libc::SIGTERM);
    *kill_at = Some(Instant::now() + grace);
}

fn stage_name(name: &str, program: &str) -> String {
    if name.is_empty() {
        program.to_owned()
    } else {
        name.to_owned()
    }
}

fn plan_error(error: PlanError) -> RunError {
    match error {
        PlanError::Requirement(message) => RunError::Plan(message),
        PlanError::Expand { .. } => RunError::InvalidRunCommand(error.to_string()),
        other => RunError::Plan(other.to_string()),
    }
}

/// Makes a file name that is safe as a path component: no separators or
/// control characters, not empty, not too long.
pub(crate) fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    let cleaned: String = cleaned.chars().take(64).collect();
    if cleaned.is_empty() {
        "main".to_owned()
    } else {
        cleaned
    }
}

/// The file name an unsaved document gets in the run directory.
pub(crate) fn temp_source_name(language: &LanguageDef, text: &str, hint: Option<&str>) -> String {
    let hint_path = hint.map(Path::new);
    let hint_ext = hint_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    let ext = hint_ext
        .filter(|ext| {
            language
                .extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
        .or_else(|| language.extensions.first().cloned())
        .unwrap_or_else(|| "txt".to_owned());
    let hint_stem = hint_path
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .filter(|stem| !stem.is_empty())
        .map(sanitize_stem);
    let stem = match ext.as_str() {
        // javac wants the public class's name; swiftc only accepts top-level code
        // in main.swift.
        "java" => java_main_class(text)
            .map(|class| class.rsplit('.').next().unwrap_or(&class).to_owned())
            .or_else(|| {
                hint_stem.clone().filter(|s| {
                    s.chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                })
            })
            .unwrap_or_else(|| "Main".to_owned()),
        "swift" => "main".to_owned(),
        _ => hint_stem.unwrap_or_else(|| "main".to_owned()),
    };
    format!("{stem}.{ext}")
}

/// Resolves the snapshot to a file on disk: `(source dir, file, file name,
/// directory hint of unsaved text)`.
fn prepare_source(
    req: &RunRequest,
    temp_dir: &Path,
) -> Result<(PathBuf, PathBuf, String, Option<PathBuf>), RunError> {
    match &req.snapshot {
        Snapshot::SavedFile { path } => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir()
                    .map_err(|e| RunError::SourceMissing(e.to_string()))?
                    .join(path)
            };
            if !path.is_file() {
                return Err(RunError::SourceMissing(path.display().to_string()));
            }
            let dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"));
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((dir, path, name, None))
        }
        Snapshot::Text {
            text,
            file_name,
            directory,
        } => {
            let name = temp_source_name(&req.language, text, file_name.as_deref());
            let dir = temp_dir.join("src");
            std::fs::create_dir_all(&dir).map_err(|e| RunError::TempFile(e.to_string()))?;
            let path = dir.join(&name);
            std::fs::write(&path, text).map_err(|e| RunError::TempFile(e.to_string()))?;
            Ok((dir, path, name, directory.clone()))
        }
    }
}

/// The directory the program runs in (see [`WorkingDirectory`]).
fn working_directory(
    req: &RunRequest,
    src_dir: &Path,
    text_directory: Option<&Path>,
) -> Result<PathBuf, RunError> {
    let file_rule = || {
        text_directory
            .filter(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| src_dir.to_path_buf())
    };
    let dir = match &req.settings.working_directory {
        WorkingDirectory::FileDirectory => file_rule(),
        WorkingDirectory::ProjectFolder => req
            .project_folder
            .clone()
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(file_rule),
        WorkingDirectory::Default => req
            .default_directory
            .clone()
            .or_else(home_dir)
            .unwrap_or_else(file_rule),
        WorkingDirectory::Custom(dir) => dir.clone(),
    };
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(RunError::WorkingDirectory(dir.display().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(id: &str) -> LanguageDef {
        scratch_language::LanguageRegistry::builtin()
            .get(id)
            .expect("builtin language")
            .clone()
    }

    #[test]
    fn unsaved_names_follow_the_language() {
        assert_eq!(temp_source_name(&lang("python"), "", None), "main.py");
        assert_eq!(
            temp_source_name(&lang("python"), "", Some("Untitled 2")),
            "Untitled 2.py"
        );
        assert_eq!(
            temp_source_name(&lang("python"), "", Some("notes.py")),
            "notes.py"
        );
        assert_eq!(
            temp_source_name(&lang("python"), "", Some("notes.txt")),
            "notes.py"
        );
        assert_eq!(temp_source_name(&lang("c"), "", Some("a/b:c.C")), "b_c.c");
        assert_eq!(
            temp_source_name(&lang("swift"), "", Some("Untitled")),
            "main.swift"
        );
        let java = "public class Hello { public static void main(String[] a) {} }";
        assert_eq!(
            temp_source_name(&lang("java"), java, Some("Untitled")),
            "Hello.java"
        );
        assert_eq!(
            temp_source_name(&lang("java"), "class X {}", Some("Untitled 3")),
            "Main.java"
        );
        assert_eq!(
            temp_source_name(&lang("java"), "class X {}", Some("Thing")),
            "Thing.java"
        );
    }

    #[test]
    fn stems_are_sanitized() {
        assert_eq!(sanitize_stem("../etc/passwd"), "_etc_passwd");
        assert_eq!(sanitize_stem("   "), "main");
        assert_eq!(sanitize_stem(".hidden"), "hidden");
        assert_eq!(sanitize_stem(&"x".repeat(200)).len(), 64);
    }
}
