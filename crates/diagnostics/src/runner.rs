//! Running diagnostics: execute one [`DiagnosticDef`] against a buffer and
//! turn the output into [`Diagnostic`]s.
//!
//! [`run_job`] is the synchronous core; [`DiagnosticsRunner`] runs jobs on
//! worker threads (cancelable, with results delivered through a channel or a
//! callback) and [`perform_test_run`] backs the "Perform Test Run..." button.
//!
//! # How a job runs
//!
//! 1. The buffer is written to a **unique temporary directory** under the
//!    file's real name (`<tmp>/src/main.c`), and the entries of the original
//!    directory are symlinked next to it so relative includes/imports keep
//!    working. If the caller says the buffer equals the file on disk the tool
//!    runs on the real file instead ([`DiagnosticJob::buffer_is_saved`]).
//! 2. By [`DiagnosticKind`]:
//!    * `Command`: the `command` template is expanded with the run
//!      variables (`$filename`, `$flags`, ...) like a run command.
//!    * `CompileScript`: the language's compile phase is planned and run with
//!      the diagnostic's effective flags; output of all steps is collected.
//!    * `Clang`: `xcrun clang -fsyntax-only -fmessage-length=0
//!      -fdiagnostics-parseable-fixits -x <lang> <file> <flags>` (clang prints
//!      no fix-its with `-fno-caret-diagnostics`, so carets stay on; the
//!      option name `[-Wfoo]` becomes the message id).
//!    * `ConsoleText` / `ConsoleHighlight`: no process; the console output
//!      given in the job is interpreted.
//!    * `Conflict`, `RegexCheck`, `InvalidSyntax`: built-in checkers
//!      ([`crate::builtin`]).
//! 3. stdout and stderr are merged in order and interpreted according to the
//!    output mode ([`crate::parse::interpret`]); a custom script is run with
//!    the output on stdin. If the configured mode finds nothing but the tool
//!    failed, all known formats are tried ([`crate::parse::parse_auto`]).
//!
//! Environment of the process: `RunnerEnv` (given by the caller), the run
//! variables (`CR_FILENAME`, `CR_FILE`, `CR_TMPDIR`, `CR_SUGGESTED_OUTPUT_FILE`
//! pointing into the temp dir, ...), the language's `run.env`, and
//! `CR_DIAGNOSTICS=1`, `CR_DIAGNOSTICS_COMMAND`, `CR_DIAGNOSTICS_DIR`.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scratch_language::expand::expand_string;
use scratch_language::{
    CompilePlan, DiagnosticDef, DiagnosticKind, LanguageDef, OutputMode, PlanError, RunContext,
    Vars, resolve_command,
};

use crate::annotation::{Annotation, DocumentContext, resolve_all};
use crate::exec::{CancelToken, ExecError, ExecOutput, ExecSpec, RunnerEnv, execute, find_program};
use crate::install::MissingTool;
use crate::model::{Diagnostic, MatchedLocation, Origin};
use crate::parse::{self, ParseError, ParseOptions};

/// Identifies a job: chosen by the caller (the [`crate::Scheduler`] hands
/// them out) and echoed in the [`JobResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct JobToken(pub u64);

/// Settings for the clang front end (the "clang completion and diagnostics"
/// Advanced preferences).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClangSettings {
    /// Apply the language's compile flags (`ClangFlagsFromUser`).
    pub use_language_flags: bool,
    /// Extra flags (`ExtraClangFlags`).
    pub extra_flags: String,
}

impl Default for ClangSettings {
    fn default() -> Self {
        Self {
            use_language_flags: true,
            extra_flags: "-Wno-unused-command-line-argument".to_owned(),
        }
    }
}

/// Everything needed to run one diagnostic. Build with
/// [`DiagnosticJob::new`] and the chainable setters.
#[derive(Debug, Clone)]
pub struct DiagnosticJob {
    /// The document's language.
    pub language: Arc<LanguageDef>,
    /// The diagnostic to run (after the user's overrides).
    pub diagnostic: DiagnosticDef,
    /// The buffer to check.
    pub text: String,
    /// The document's path; `None` for an unsaved document.
    pub file: Option<PathBuf>,
    /// The buffer equals the file on disk: run on the real file (keeps
    /// project context such as `go.mod`).
    pub buffer_is_saved: bool,
    /// The language's current compile flags (Run Settings); the
    /// diagnostic's own flags replace or extend them.
    pub language_flags: String,
    /// Process environment; `None` = this process's environment.
    pub env: Option<RunnerEnv>,
    /// Kill the tool after this long (default 30 s).
    pub timeout: Duration,
    /// Clang settings.
    pub clang: ClangSettings,
    /// Extra flags for Swift compile diagnostics (`ExtraSwiftFlags`).
    pub swift_flags: String,
    /// Interpreter script to use for `OutputMode::CustomScript` instead of
    /// the definition's `output_script` text (e.g. the language's
    /// `Diagnostics/<uuid>/parser`).
    pub script_path: Option<PathBuf>,
    /// Console output for the console kinds.
    pub console_output: Option<String>,
    /// Where to create temporary directories (default: the system temp dir).
    pub temp_root: Option<PathBuf>,
    /// Try all known formats when the configured one finds nothing but the
    /// tool failed (default true).
    pub fallback_auto: bool,
    /// Shell for shell commands (default `/bin/bash`).
    pub shell: String,
    /// Opaque value handed back in the result (the scheduler's generation).
    pub generation: u64,
}

impl DiagnosticJob {
    /// A job with defaults.
    pub fn new(language: Arc<LanguageDef>, diagnostic: DiagnosticDef, text: impl Into<String>) -> Self {
        Self {
            language,
            diagnostic,
            text: text.into(),
            file: None,
            buffer_is_saved: false,
            language_flags: String::new(),
            env: None,
            timeout: Duration::from_secs(30),
            clang: ClangSettings::default(),
            swift_flags: String::new(),
            script_path: None,
            console_output: None,
            temp_root: None,
            fallback_auto: true,
            shell: "/bin/bash".to_owned(),
            generation: 0,
        }
    }

    /// Sets the document path.
    #[must_use]
    pub fn file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Marks the buffer as identical to the file on disk.
    #[must_use]
    pub fn saved(mut self, saved: bool) -> Self {
        self.buffer_is_saved = saved;
        self
    }

    /// Sets the language's current compile flags.
    #[must_use]
    pub fn language_flags(mut self, flags: impl Into<String>) -> Self {
        self.language_flags = flags.into();
        self
    }

    /// Sets the process environment.
    #[must_use]
    pub fn env(mut self, env: RunnerEnv) -> Self {
        self.env = Some(env);
        self
    }

    /// Sets the timeout.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the console output (console kinds).
    #[must_use]
    pub fn console_output(mut self, output: impl Into<String>) -> Self {
        self.console_output = Some(output.into());
        self
    }

    /// Sets the temp root.
    #[must_use]
    pub fn temp_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.temp_root = Some(root.into());
        self
    }
}

/// Why a job produced no diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// The job was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The tool ran longer than the timeout.
    #[error("the diagnostic did not finish within {0:?}")]
    Timeout(Duration),
    /// The tool is not installed.
    #[error("{} is not installed", .0.program)]
    ToolMissing(MissingTool),
    /// The run could not be planned (a requirement of the language is not
    /// met, a template is invalid, the diagnostic is incomplete); the
    /// message is meant for the user.
    #[error("{0}")]
    Plan(String),
    /// Temporary files or processes failed.
    #[error("{0}")]
    Io(String),
    /// The output could not be interpreted (bad regex, bad JSON).
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// The custom interpreter script failed.
    #[error("the diagnostics script failed: {0}")]
    Script(String),
    /// A worker thread panicked (a bug).
    #[error("internal error: {0}")]
    Internal(String),
}

/// A finished job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobOutput {
    /// The diagnostic entry's id; every diagnostic has it as `source`.
    pub source: String,
    /// What was found. Files may name the temporary copy: use
    /// [`JobOutput::annotations`].
    pub diagnostics: Vec<Diagnostic>,
    /// The raw output that was interpreted.
    pub raw_output: String,
    /// The command line that ran (display only).
    pub command: String,
    /// Exit code of the (last) tool process.
    pub exit_code: Option<i32>,
    /// How long it took.
    pub duration: Duration,
    /// Temporary copies of the document the output may refer to.
    pub aliases: Vec<PathBuf>,
    /// Directory relative file names in the output are relative to.
    pub cwd: Option<PathBuf>,
    /// The configured output mode found nothing and the all-formats fallback
    /// was used.
    pub used_fallback: bool,
    /// Console highlight only (`ConsoleHighlight`): use
    /// [`JobOutput::highlights`], not the diagnostics.
    pub highlight_only: bool,
    /// stderr of the custom interpreter script.
    pub script_stderr: String,
    /// The generation the job was started with.
    pub generation: u64,
}

impl JobOutput {
    /// Places the diagnostics in `text` (the document's *current* buffer),
    /// returning the annotations for this document and those about other
    /// files. `file` is the document's path.
    pub fn annotations(&self, text: &str, file: Option<&Path>) -> (Vec<Annotation>, Vec<Annotation>) {
        let mut ctx = DocumentContext::new(text);
        if let Some(f) = file {
            ctx = ctx.path(f);
        }
        for a in &self.aliases {
            ctx = ctx.alias(a.clone());
        }
        if let Some(c) = &self.cwd {
            ctx = ctx.cwd(c);
        }
        resolve_all(&self.diagnostics, &ctx)
    }

    /// Console spans (all `matched` ranges of the diagnostics and their
    /// children) for colouring and linking console text.
    pub fn highlights(&self) -> Vec<MatchedLocation> {
        fn walk(d: &Diagnostic, out: &mut Vec<MatchedLocation>) {
            out.extend(d.matched.iter().cloned());
            for c in &d.children {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        for d in &self.diagnostics {
            walk(d, &mut out);
        }
        out.sort_by_key(|m| (m.range.start, m.range.end));
        out
    }
}

/// The result delivered to the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct JobResult {
    /// The token the job was spawned with.
    pub token: JobToken,
    /// Id of the diagnostic entry.
    pub diagnostic_id: String,
    /// Generation of the buffer the job started with.
    pub generation: u64,
    /// Output or error.
    pub outcome: Result<JobOutput, RunError>,
}

// ----------------------------------------------------------------- workspace

struct Workspace {
    tmp: tempfile::TempDir,
    dir: PathBuf,
    file: PathBuf,
    out_dir: PathBuf,
    in_place: bool,
}

const MAX_LINKED_SIBLINGS: usize = 2000;

fn file_name_for(job: &DiagnosticJob) -> String {
    if let Some(name) = job.file.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()) {
        return name.to_owned();
    }
    match job.language.extensions.first() {
        Some(ext) => format!("untitled.{ext}"),
        None => "untitled".to_owned(),
    }
}

fn io_err(context: &str, e: &std::io::Error) -> RunError {
    RunError::Io(format!("{context}: {e}"))
}

fn prepare(job: &DiagnosticJob) -> Result<Workspace, RunError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("scratch-diag-");
    let tmp = match &job.temp_root {
        Some(root) => builder.tempdir_in(root),
        None => builder.tempdir(),
    }
    .map_err(|e| io_err("cannot create a temporary directory", &e))?;
    let out_dir = tmp.path().join("out");
    std::fs::create_dir_all(&out_dir).map_err(|e| io_err("cannot create the output directory", &e))?;
    let name = file_name_for(job);
    if job.buffer_is_saved
        && let Some(real) = job.file.as_ref().filter(|f| f.is_file())
        && let Some(parent) = real.parent()
    {
        return Ok(Workspace {
            dir: parent.to_path_buf(),
            file: real.clone(),
            out_dir,
            in_place: true,
            tmp,
        });
    }
    let dir = tmp.path().join("src");
    std::fs::create_dir_all(&dir).map_err(|e| io_err("cannot create the source directory", &e))?;
    let file = dir.join(&name);
    std::fs::write(&file, job.text.as_bytes()).map_err(|e| io_err("cannot write the buffer", &e))?;
    if let Some(parent) = job.file.as_deref().and_then(Path::parent)
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten().take(MAX_LINKED_SIBLINGS) {
            let n = entry.file_name();
            if n.to_string_lossy().starts_with('.') || n.to_string_lossy() == name {
                continue;
            }
            // Best effort: a missing link only affects relative includes.
            let _ = std::os::unix::fs::symlink(entry.path(), dir.join(&n));
        }
    }
    Ok(Workspace {
        dir,
        file,
        out_dir,
        in_place: false,
        tmp,
    })
}

fn base_vars(job: &DiagnosticJob, ws: &Workspace, env: &RunnerEnv, flags: &str) -> Vars {
    let name = ws
        .file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("untitled")
        .to_owned();
    Vars::for_source(&ws.dir, &name)
        .with_env(env.vars.clone())
        .with_tmpdir(&ws.out_dir)
        .with_flags(flags)
        .with_encoding(job.language.output_encoding)
}

/// `run.env` of the language: variables and PATH additions.
fn language_env(language: &LanguageDef, vars: &Vars) -> (BTreeMap<String, String>, Vec<String>, Vec<String>) {
    let spec = &language.run.env;
    let usable = |e: &str| !e.starts_with("$home") || vars.is_set("home");
    let expand = |t: &String| expand_string(t, vars).ok();
    (
        spec.vars
            .iter()
            .filter_map(|(k, v)| expand(v).map(|v| (k.clone(), v)))
            .collect(),
        spec.path_prepend.iter().filter(|e| usable(e)).filter_map(expand).collect(),
        spec.path_append.iter().filter(|e| usable(e)).filter_map(expand).collect(),
    )
}

struct ProcessCtx<'a> {
    job: &'a DiagnosticJob,
    ws: &'a Workspace,
    env: BTreeMap<String, String>,
    path: String,
    deadline: Instant,
    cancel: &'a CancelToken,
}

impl ProcessCtx<'_> {
    fn exec(&self, argv: Vec<String>, cwd: &Path, extra_env: &BTreeMap<String, String>) -> Result<ExecOutput, RunError> {
        let mut spec = ExecSpec::new(argv, cwd);
        spec.env = self.env.clone();
        spec.env.extend(extra_env.clone());
        spec.path.clone_from(&self.path);
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        spec.timeout = Some(remaining.max(Duration::from_millis(50)));
        execute(&spec, self.cancel).map_err(|e| self.map_exec(e, &spec.argv))
    }

    fn map_exec(&self, e: ExecError, argv: &[String]) -> RunError {
        match e {
            ExecError::Cancelled => RunError::Cancelled,
            ExecError::Timeout(_) => RunError::Timeout(self.job.timeout),
            ExecError::NotFound(program) => RunError::ToolMissing(MissingTool::new(
                &program,
                self.job.language.exit_status_message(127),
            )),
            ExecError::Io(m) => RunError::Io(format!("{}: {m}", argv.first().map_or("", String::as_str))),
        }
    }

    fn missing(&self, program: &str, message: Option<String>) -> RunError {
        RunError::ToolMissing(MissingTool::new(
            program,
            message.or_else(|| self.job.language.exit_status_message(127)),
        ))
    }
}

fn plan_message(e: &PlanError) -> String {
    match e {
        PlanError::Requirement(m) => m.clone(),
        other => other.to_string(),
    }
}

struct Raw {
    output: String,
    command: String,
    code: Option<i32>,
}

fn run_command(ctx: &ProcessCtx<'_>, vars: &Vars) -> Result<Raw, RunError> {
    let template = ctx
        .job
        .diagnostic
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| RunError::Plan(format!("The diagnostic \"{}\" has no command.", ctx.job.diagnostic.name)))?;
    let resolved = resolve_command(template, vars).map_err(|e| RunError::Plan(format!("Invalid command: {e}")))?;
    let argv = resolved.to_argv(&ctx.job.shell, false, &[]);
    let mut extra = BTreeMap::new();
    extra.insert("CR_DIAGNOSTICS_COMMAND".to_owned(), template.to_owned());
    let out = ctx.exec(argv.clone(), &ctx.ws.dir, &extra)?;
    if out.code == Some(127)
        && let Some(program) = resolved.program()
        && find_program(&program, &ctx.path).is_none()
    {
        return Err(ctx.missing(&program, None));
    }
    Ok(Raw {
        output: out.stdout,
        command: argv.join(" "),
        code: out.code,
    })
}

fn clang_language(job: &DiagnosticJob, file_name: &str) -> &'static str {
    match job.language.id.as_str() {
        "c" => "c",
        "cpp" => "c++",
        "objective-c" => "objective-c",
        "objective-cpp" => "objective-c++",
        _ => match Path::new(file_name).extension().and_then(|e| e.to_str()) {
            Some("cpp" | "cc" | "cxx" | "hpp" | "hh") => "c++",
            Some("m") => "objective-c",
            Some("mm") => "objective-c++",
            _ => "c",
        },
    }
}

fn run_clang(ctx: &ProcessCtx<'_>) -> Result<Raw, RunError> {
    let job = ctx.job;
    let file_name = ctx.ws.file.file_name().and_then(|n| n.to_str()).unwrap_or("untitled.c");
    let mut args: Vec<String> = [
        "-fsyntax-only",
        "-fmessage-length=0",
        "-fdiagnostics-parseable-fixits",
        "-x",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(clang_language(job, file_name).to_owned());
    args.push(file_name.to_owned());
    if job.clang.use_language_flags {
        let flags = job.diagnostic.effective_flags(&job.language_flags);
        args.extend(scratch_language::split_words(&flags).unwrap_or_else(|_| flags.split_whitespace().map(str::to_owned).collect()));
    }
    if !ctx.ws.in_place
        && let Some(parent) = job.file.as_deref().and_then(Path::parent)
    {
        args.push("-iquote".to_owned());
        args.push(parent.to_string_lossy().into_owned());
    }
    args.extend(job.clang.extra_flags.split_whitespace().map(str::to_owned));
    let mut argv = Vec::new();
    if find_program("xcrun", &ctx.path).is_some() {
        argv.push("xcrun".to_owned());
    }
    argv.push("clang".to_owned());
    argv.extend(args);
    let out = ctx.exec(argv.clone(), &ctx.ws.dir, &BTreeMap::new())?;
    Ok(Raw {
        output: out.stdout,
        command: argv.join(" "),
        code: out.code,
    })
}

fn run_compile_script(ctx: &ProcessCtx<'_>, vars: Vars) -> Result<Raw, RunError> {
    let job = ctx.job;
    let mut flags = job.diagnostic.effective_flags(&job.language_flags);
    if job.language.id == "swift" && !job.swift_flags.trim().is_empty() {
        flags = format!("{flags} {}", job.swift_flags).trim().to_owned();
    }
    let mut run = RunContext::new(vars);
    run.compile_flags = Some(flags);
    let plan = job.language.plan_run(&run).map_err(|e| RunError::Plan(plan_message(&e)))?;
    let mut output = String::new();
    let mut commands = Vec::new();
    let mut code = Some(0);
    match &plan.compile {
        CompilePlan::None => {
            return Err(RunError::Plan(format!(
                "{} has no compile step to produce diagnostics from.",
                job.language.name
            )));
        }
        CompilePlan::Steps(steps) => {
            let mut path = ctx.path.clone();
            let extra_path = plan.path_prepend.iter().chain(&plan.path_append).count();
            if extra_path > 0 {
                path = RunnerEnv {
                    vars: BTreeMap::new(),
                    path: ctx.path.clone(),
                }
                .path_with(&plan.path_prepend, &plan.path_append);
            }
            for step in steps {
                for dir in &step.mkdirs {
                    std::fs::create_dir_all(dir).map_err(|e| io_err("cannot create a build directory", &e))?;
                }
                let use_via = step.via.first().is_some_and(|w| find_program(w, &path).is_some());
                let argv = step.argv(use_via);
                if find_program(&argv[0], &path).is_none() {
                    return Err(ctx.missing(&step.program, step.missing_tool.clone()));
                }
                let mut env = plan.process_env();
                env.extend(step.env.clone());
                env.insert("CR_DIAGNOSTICS".to_owned(), "1".to_owned());
                let sub = ProcessCtx {
                    path: path.clone(),
                    env: ctx.env.clone(),
                    job,
                    ws: ctx.ws,
                    deadline: ctx.deadline,
                    cancel: ctx.cancel,
                };
                let out = sub.exec(argv, &step.cwd, &env)?;
                commands.push(step.display());
                output.push_str(&out.stdout);
                code = out.code;
                if out.code == Some(127) {
                    return Err(ctx.missing(&step.program, step.missing_tool.clone()));
                }
                if out.code != Some(0) {
                    break;
                }
            }
        }
        CompilePlan::Shell(shell) => {
            let script = ctx.ws.tmp.path().join("compile.sh");
            std::fs::write(&script, &shell.script).map_err(|e| io_err("cannot write the compile script", &e))?;
            let mut argv = vec![job.shell.clone(), script.to_string_lossy().into_owned()];
            argv.extend(shell.args.iter().cloned());
            let mut env = shell.env.clone();
            env.insert("CR_DIAGNOSTICS".to_owned(), "1".to_owned());
            let out = ctx.exec(argv.clone(), &shell.cwd, &env)?;
            commands.push(argv.join(" "));
            output.push_str(&out.stdout);
            code = out.code;
            if out.code == Some(127) {
                return Err(ctx.missing("the compiler", None));
            }
        }
    }
    Ok(Raw {
        output,
        command: commands.join(" && "),
        code,
    })
}

// ------------------------------------------------------------- interpretation

/// Runs a custom interpreter script on `output`.
fn run_script(ctx: &ProcessCtx<'_>, output: &str, ws_dir: &Path) -> Result<(String, String), RunError> {
    let def = &ctx.job.diagnostic;
    let script_path = if let Some(p) = &ctx.job.script_path {
        p.clone()
    } else {
        let text = def.output_script.as_deref().unwrap_or_default();
        if !(text.starts_with("#!") || text.contains('\n')) {
            return Err(RunError::Script(format!(
                "the interpreter script of \"{}\" is not available",
                def.name
            )));
        }
        let dir = ctx.ws.tmp.path().join("diagnostics");
        std::fs::create_dir_all(&dir).map_err(|e| io_err("cannot create the script directory", &e))?;
        let path = dir.join("parser");
        let body = if text.starts_with("#!") {
            text.to_owned()
        } else {
            format!("#!/bin/sh\n{text}")
        };
        std::fs::write(&path, body).map_err(|e| io_err("cannot write the interpreter script", &e))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| io_err("cannot make the script executable", &e))?;
        path
    };
    let mut spec = ExecSpec::new(vec![script_path.to_string_lossy().into_owned()], ws_dir);
    spec.env = ctx.env.clone();
    spec.env.insert("CR_DIAGNOSTICS".to_owned(), "1".to_owned());
    spec.env.insert(
        "CR_DIAGNOSTICS_DIR".to_owned(),
        script_path.parent().unwrap_or(ws_dir).to_string_lossy().into_owned(),
    );
    spec.path.clone_from(&ctx.path);
    spec.stdin = Some(output.as_bytes().to_vec());
    spec.merge_stderr = false;
    spec.timeout = Some(ctx.deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(2)));
    let out = execute(&spec, ctx.cancel).map_err(|e| match e {
        ExecError::NotFound(p) => RunError::Script(format!("{p} not found")),
        other => ctx.map_exec(other, &spec.argv),
    })?;
    if out.code != Some(0) {
        let msg = out.stderr.trim();
        return Err(RunError::Script(if msg.is_empty() {
            format!("exit status {}", out.code.map_or_else(|| "?".to_owned(), |c| c.to_string()))
        } else {
            msg.to_owned()
        }));
    }
    Ok((out.stdout, out.stderr))
}

fn stamp(diags: &mut [Diagnostic], source: &str, origin: Origin) {
    for d in diags {
        d.source = Some(source.to_owned());
        d.origin = origin;
        stamp(&mut d.children, source, origin);
    }
}

struct Interpreted {
    diagnostics: Vec<Diagnostic>,
    used_fallback: bool,
    script_stderr: String,
}

fn interpret(ctx: &ProcessCtx<'_>, output: &str, code: Option<i32>, origin: Origin) -> Result<Interpreted, RunError> {
    let job = ctx.job;
    let def = &job.diagnostic;
    let opts = ParseOptions {
        language: Some(job.language.id.clone()),
        origin,
    };
    let mut script_stderr = String::new();
    let mut diagnostics = match def.output_mode {
        OutputMode::CustomScript => {
            let (stdout, stderr) = run_script(ctx, output, &ctx.ws.dir)?;
            script_stderr = stderr;
            parse::parse_script_json(&stdout, output, &opts)?
        }
        mode => parse::interpret(output, mode, def.regex.as_deref(), &opts)?,
    };
    let mut used_fallback = false;
    if diagnostics.is_empty()
        && job.fallback_auto
        && origin == Origin::Document
        && code != Some(0)
        && !output.trim().is_empty()
        && def.output_mode != OutputMode::CustomScript
    {
        diagnostics = parse::parse_auto(output, &opts);
        used_fallback = !diagnostics.is_empty();
    }
    Ok(Interpreted {
        diagnostics,
        used_fallback,
        script_stderr,
    })
}

fn output_base(job: &DiagnosticJob, started: Instant) -> JobOutput {
    JobOutput {
        source: job.diagnostic.id.clone(),
        diagnostics: Vec::new(),
        raw_output: String::new(),
        command: String::new(),
        exit_code: None,
        duration: started.elapsed(),
        aliases: Vec::new(),
        cwd: None,
        used_fallback: false,
        highlight_only: false,
        script_stderr: String::new(),
        generation: job.generation,
    }
}

/// Runs one job to completion on the current thread.
///
/// # Errors
///
/// [`RunError`]: cancelled, timed out, tool missing, plan/IO/parse problems.
pub fn run_job(job: &DiagnosticJob, cancel: &CancelToken) -> Result<JobOutput, RunError> {
    let started = Instant::now();
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let def = &job.diagnostic;
    let mut out = output_base(job, started);
    match def.kind {
        DiagnosticKind::Conflict | DiagnosticKind::RegexCheck | DiagnosticKind::InvalidSyntax => {
            out.diagnostics = crate::builtin::run_builtin(def.kind, &job.language, &job.text);
            out.command = format!("built-in: {}", def.name);
        }
        DiagnosticKind::ConsoleText | DiagnosticKind::ConsoleHighlight => {
            let ws = prepare_minimal(job)?;
            let env = job.env.clone().unwrap_or_else(RunnerEnv::from_process);
            let ctx = ProcessCtx {
                job,
                ws: &ws,
                env: env.vars.clone(),
                path: env.path_with(&[], &[]),
                deadline: Instant::now() + job.timeout,
                cancel,
            };
            let text = job.console_output.clone().unwrap_or_default();
            let i = interpret(&ctx, &text, Some(0), Origin::Console)?;
            out.diagnostics = i.diagnostics;
            out.script_stderr = i.script_stderr;
            out.raw_output = text;
            out.highlight_only = def.kind == DiagnosticKind::ConsoleHighlight;
            out.command = "console output".to_owned();
        }
        DiagnosticKind::CompileScript | DiagnosticKind::Command | DiagnosticKind::Clang => {
            let ws = prepare(job)?;
            let env = job.env.clone().unwrap_or_else(RunnerEnv::from_process);
            let flags = def.effective_flags(&job.language_flags);
            let vars = base_vars(job, &ws, &env, &flags);
            let (lang_vars, prepend, append) = language_env(&job.language, &vars);
            let mut process_env = env.vars.clone();
            process_env.extend(vars.export_env());
            process_env.extend(lang_vars);
            process_env.insert("CR_DIAGNOSTICS".to_owned(), "1".to_owned());
            process_env.insert(
                "CR_DIAGNOSTICS_DIR".to_owned(),
                ws.tmp.path().join("diagnostics").to_string_lossy().into_owned(),
            );
            let ctx = ProcessCtx {
                job,
                ws: &ws,
                env: process_env,
                path: env.path_with(&prepend, &append),
                deadline: Instant::now() + job.timeout,
                cancel,
            };
            let raw = match def.kind {
                DiagnosticKind::Command => run_command(&ctx, &vars)?,
                DiagnosticKind::Clang => run_clang(&ctx)?,
                _ => run_compile_script(&ctx, vars)?,
            };
            let i = interpret(&ctx, &raw.output, raw.code, Origin::Document)?;
            out.diagnostics = i.diagnostics;
            out.used_fallback = i.used_fallback;
            out.script_stderr = i.script_stderr;
            out.raw_output = raw.output;
            out.command = raw.command;
            out.exit_code = raw.code;
            out.cwd = Some(ws.dir.clone());
            if !ws.in_place {
                // Tools print the real path (/private/var/...) of a temp copy
                // addressed through a symlink (/var/...): remember both while
                // the file still exists.
                out.aliases.push(ws.file.clone());
                if let Ok(real) = ws.file.canonicalize()
                    && real != ws.file
                {
                    out.aliases.push(real);
                }
            }
        }
    }
    let origin = if matches!(def.kind, DiagnosticKind::ConsoleText | DiagnosticKind::ConsoleHighlight) {
        Origin::Console
    } else {
        Origin::Document
    };
    stamp(&mut out.diagnostics, &def.id, origin);
    out.duration = started.elapsed();
    Ok(out)
}

/// A workspace without a copy of the buffer (console kinds).
fn prepare_minimal(job: &DiagnosticJob) -> Result<Workspace, RunError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("scratch-diag-");
    let tmp = match &job.temp_root {
        Some(root) => builder.tempdir_in(root),
        None => builder.tempdir(),
    }
    .map_err(|e| io_err("cannot create a temporary directory", &e))?;
    let dir = job
        .file
        .as_deref()
        .and_then(Path::parent)
        .filter(|p| p.is_dir())
        .map_or_else(|| tmp.path().to_path_buf(), Path::to_path_buf);
    Ok(Workspace {
        out_dir: tmp.path().join("out"),
        file: job.file.clone().unwrap_or_else(|| tmp.path().join("untitled")),
        dir,
        in_place: true,
        tmp,
    })
}

// -------------------------------------------------------------- async runner

type Notifier = Arc<dyn Fn() + Send + Sync>;
/// Computes the environment lazily on the worker thread.
pub type EnvProvider = Arc<dyn Fn() -> RunnerEnv + Send + Sync>;

/// Handle to a running job.
#[derive(Debug, Clone)]
pub struct JobHandle {
    /// The job's token.
    pub token: JobToken,
    cancel: CancelToken,
}

impl JobHandle {
    /// Cancels the job: its process group is killed and the result will be
    /// [`RunError::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Runs jobs on worker threads and delivers [`JobResult`]s.
///
/// Typical use from a UI: call [`DiagnosticsRunner::spawn`] for the jobs the
/// [`crate::Scheduler`] asks for, set a notifier that wakes the UI
/// ([`DiagnosticsRunner::with_notifier`]) and drain results with
/// [`DiagnosticsRunner::try_recv`] on the UI thread.
pub struct DiagnosticsRunner {
    tx: Sender<JobResult>,
    rx: Mutex<Receiver<JobResult>>,
    running: Arc<Mutex<BTreeMap<JobToken, CancelToken>>>,
    notifier: Option<Notifier>,
    env_provider: Option<EnvProvider>,
    next_token: AtomicU64,
}

impl Default for DiagnosticsRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticsRunner {
    /// A runner without notifier or environment provider.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx: Mutex::new(rx),
            running: Arc::new(Mutex::new(BTreeMap::new())),
            notifier: None,
            env_provider: None,
            next_token: AtomicU64::new(1 << 40),
        }
    }

    /// Calls `notify` (from the worker thread) whenever a result was queued,
    /// to wake the UI.
    #[must_use]
    pub fn with_notifier(mut self, notify: impl Fn() + Send + Sync + 'static) -> Self {
        self.notifier = Some(Arc::new(notify));
        self
    }

    /// Supplies the process environment for jobs that do not set one. The
    /// provider runs on the worker thread (it may probe a login shell).
    #[must_use]
    pub fn with_env_provider(mut self, provider: impl Fn() -> RunnerEnv + Send + Sync + 'static) -> Self {
        self.env_provider = Some(Arc::new(provider));
        self
    }

    /// A token that cannot collide with scheduler tokens (for ad-hoc jobs).
    pub fn fresh_token(&self) -> JobToken {
        JobToken(self.next_token.fetch_add(1, Ordering::Relaxed))
    }

    /// Starts a job; the result arrives through [`DiagnosticsRunner::try_recv`].
    pub fn spawn(&self, token: JobToken, job: DiagnosticJob) -> JobHandle {
        let tx = self.tx.clone();
        let notifier = self.notifier.clone();
        self.spawn_inner(token, job, move |result| {
            let _ = tx.send(result);
            if let Some(n) = &notifier {
                n();
            }
        })
    }

    /// Starts a job and calls `done` on the worker thread with the result
    /// (nothing is queued).
    pub fn spawn_with(
        &self,
        token: JobToken,
        job: DiagnosticJob,
        done: impl FnOnce(JobResult) + Send + 'static,
    ) -> JobHandle {
        self.spawn_inner(token, job, done)
    }

    fn spawn_inner(
        &self,
        token: JobToken,
        mut job: DiagnosticJob,
        done: impl FnOnce(JobResult) + Send + 'static,
    ) -> JobHandle {
        let cancel = CancelToken::new();
        if let Ok(mut running) = self.running.lock() {
            running.insert(token, cancel.clone());
        }
        let running = Arc::clone(&self.running);
        let provider = self.env_provider.clone();
        let worker_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("diagnostics-job".to_owned())
            .spawn(move || {
                if job.env.is_none()
                    && let Some(p) = &provider
                {
                    job.env = Some(p());
                }
                let diagnostic_id = job.diagnostic.id.clone();
                let generation = job.generation;
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| run_job(&job, &worker_cancel)))
                    .unwrap_or_else(|_| Err(RunError::Internal("the diagnostics worker panicked".to_owned())));
                if let Ok(mut r) = running.lock() {
                    r.remove(&token);
                }
                done(JobResult {
                    token,
                    diagnostic_id,
                    generation,
                    outcome,
                });
            });
        if spawned.is_err()
            && let Ok(mut r) = self.running.lock()
        {
            r.remove(&token);
        }
        JobHandle { token, cancel }
    }

    /// Cancels a job by token (no-op when it already finished).
    pub fn cancel(&self, token: JobToken) {
        if let Ok(running) = self.running.lock()
            && let Some(c) = running.get(&token)
        {
            c.cancel();
        }
    }

    /// Cancels every running job.
    pub fn cancel_all(&self) {
        if let Ok(running) = self.running.lock() {
            for c in running.values() {
                c.cancel();
            }
        }
    }

    /// Number of jobs still running.
    pub fn running_count(&self) -> usize {
        self.running.lock().map_or(0, |r| r.len())
    }

    /// The next finished result, if any (non-blocking).
    pub fn try_recv(&self) -> Option<JobResult> {
        self.rx.lock().ok()?.try_recv().ok()
    }

    /// Waits up to `timeout` for the next finished result.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<JobResult> {
        self.rx.lock().ok()?.recv_timeout(timeout).ok()
    }

    /// All results that are ready.
    pub fn drain(&self) -> Vec<JobResult> {
        std::iter::from_fn(|| self.try_recv()).collect()
    }
}

impl Drop for DiagnosticsRunner {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

// ------------------------------------------------------------------ test run

/// The report of "Perform Test Run...".
#[derive(Debug, Clone, PartialEq)]
pub struct TestRunReport {
    /// "Output from the diagnostic run:" - the raw text interpreted.
    pub raw_output: String,
    /// The command that ran.
    pub command: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// What the output was parsed into (empty on error).
    pub diagnostics: Vec<Diagnostic>,
    /// Annotations placed in the sample buffer (this document).
    pub annotations: Vec<Annotation>,
    /// Diagnostics about other files.
    pub foreign: Vec<Annotation>,
    /// The all-formats fallback was needed.
    pub used_fallback: bool,
    /// Error text when the run failed.
    pub error: Option<RunError>,
}

impl TestRunReport {
    /// "The diagnostics test run produced N diagnostic(s)."
    pub fn summary(&self) -> String {
        let n = self.annotations.len() + self.foreign.len();
        format!(
            "The diagnostics test run produced {n} diagnostic{}.",
            if n == 1 { "" } else { "s" }
        )
    }
}

/// Runs a diagnostic once against `job.text` and returns the raw output and
/// the parsed annotations (the "Perform Test Run..." button). Blocks until
/// the tool finishes; run it on a worker thread. A failing run is reported
/// in [`TestRunReport::error`] together with whatever output exists.
pub fn perform_test_run(job: &DiagnosticJob, cancel: &CancelToken) -> TestRunReport {
    match run_job(job, cancel) {
        Ok(out) => {
            let (annotations, foreign) = out.annotations(&job.text, job.file.as_deref());
            TestRunReport {
                raw_output: out.raw_output,
                command: out.command,
                exit_code: out.exit_code,
                diagnostics: out.diagnostics,
                annotations,
                foreign,
                used_fallback: out.used_fallback,
                error: None,
            }
        }
        Err(e) => TestRunReport {
            raw_output: String::new(),
            command: String::new(),
            exit_code: None,
            diagnostics: Vec::new(),
            annotations: Vec::new(),
            foreign: Vec::new(),
            used_fallback: false,
            error: Some(e),
        },
    }
}
