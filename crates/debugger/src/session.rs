//! The debug session actor.
//!
//! [`DebugSession::start`] spawns the debugger (and, for attach-style
//! adapters, the debuggee), then a single actor thread owns every process and
//! talks to the UI over an event channel.  Commands go the other way through
//! [`SessionCommand`].
//!
//! The actor is deliberately synchronous inside one thread: commands are
//! queued and sent one at a time when the debugger is ready (its prompt was
//! seen).  Output arrives in arbitrary chunks, is ANSI-stripped and split into
//! prompt-delimited segments.  A segment can contain an asynchronous stop
//! (LLDB prints the stop notification while its command interpreter is idle),
//! so stops are detected from the stream rather than from prompts alone.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize, SlavePty};

use crate::adapter::{
    join_nul, quote_argv, quote_double, quote_python, render, Availability, DebuggerAdapter,
    EchoMode, ParserKind,
};
use crate::breakpoints::{Breakpoint, BreakpointKey};
use crate::error::DebugError;
use crate::helpers::{init_file, python_helper};
use crate::model::{
    DebugEvent, DebuggerState, ExpressionKind, FrameId, LogDirection, Location, OutputStream,
    SessionCommand, Stack, StopReason, VariablesQuery,
};
use crate::parser::{
    parse_breakpoint_set, parse_generic_stack, parse_generic_variables, parse_json_variables,
    parse_lldb_stack, parse_lldb_stop, parse_lldb_variables, parse_node_stack, parse_node_stop,
    parse_pdb_stack, parse_pdb_stop, parse_python_exception, CompiledPatterns, TranscriptDecoder,
};
use crate::source_map::SourceMap;
use crate::variables::{
    sort_variables, truncate_values, Variable, VariableId, VariableTree, WatchValue,
};

/// Default time to wait for the debugger's first prompt.
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for a graceful kill during cleanup.
const KILL_GRACE: Duration = Duration::from_millis(120);

/// Configuration for one debug session.
///
/// The runner prepares the binary (compiling with debug information when
/// needed) and passes its path plus a [`SourceMap`].  Example:
///
/// ```no_run
/// use std::path::PathBuf;
/// use scratch_debugger::{AdapterRegistry, DebugSession, SessionConfig};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = AdapterRegistry::builtin();
/// let adapter = registry.get("lldb").expect("lldb adapter").clone();
/// let config = SessionConfig::new(adapter, PathBuf::from("/tmp/a.out"))
///     .cwd(PathBuf::from("/tmp"));
/// let session = DebugSession::start(config)?;
/// session.set_breakpoints(vec![scratch_debugger::Breakpoint::new("/tmp/a.c", 4)])?;
/// session.continue_()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Adapter driving the CLI debugger.
    pub adapter: DebuggerAdapter,
    /// Prepared program (binary or script) to debug.
    pub program: PathBuf,
    /// Arguments for the program.
    pub program_args: Vec<String>,
    /// Working directory (usually the document's directory).
    pub cwd: PathBuf,
    /// Extra environment variables; inherits the app environment.
    pub env: BTreeMap<String, String>,
    /// Maps debugger paths to local source files.
    pub source_map: SourceMap,
    /// Breakpoints installed before launch.
    pub breakpoints: Vec<Breakpoint>,
    /// Text written to the program's stdin after launch.
    pub program_input: Option<String>,
    /// Emit raw debugger log lines ([`DebugEvent::LogLine`]).
    pub verbose: bool,
    /// Default variable depth.
    pub depth: u8,
    /// Default "show all variables" flag.
    pub show_all: bool,
    /// Default "show types" flag.
    pub show_types: bool,
    /// Default variables order.
    pub sort: crate::variables::SortMode,
    /// How long to wait for the first prompt.
    pub start_timeout: Duration,
}

impl SessionConfig {
    /// New configuration for an adapter and a prepared program.
    pub fn new(adapter: DebuggerAdapter, program: impl Into<PathBuf>) -> Self {
        Self {
            adapter,
            program: program.into(),
            program_args: Vec::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: BTreeMap::new(),
            source_map: SourceMap::new(),
            breakpoints: Vec::new(),
            program_input: None,
            verbose: false,
            depth: 2,
            show_all: false,
            show_types: true,
            sort: crate::variables::SortMode::Natural,
            start_timeout: DEFAULT_START_TIMEOUT,
        }
    }

    /// Set the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Add one program argument.
    pub fn program_arg(mut self, arg: impl Into<String>) -> Self {
        self.program_args.push(arg.into());
        self
    }

    /// Set extra environment variables.
    pub fn env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set the source map.
    pub fn source_map(mut self, map: SourceMap) -> Self {
        self.source_map = map;
        self
    }

    /// Set initial breakpoints.
    pub fn breakpoints(mut self, breakpoints: Vec<Breakpoint>) -> Self {
        self.breakpoints = breakpoints;
        self
    }

    /// Set program stdin input.
    pub fn program_input(mut self, input: impl Into<String>) -> Self {
        self.program_input = Some(input.into());
        self
    }

    /// Enable the verbose debugger log.
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set variable defaults.
    pub fn variables(mut self, depth: u8, show_all: bool, show_types: bool, sort: crate::variables::SortMode) -> Self {
        self.depth = depth;
        self.show_all = show_all;
        self.show_types = show_types;
        self.sort = sort;
        self
    }

    /// Set the startup timeout.
    pub fn start_timeout(mut self, timeout: Duration) -> Self {
        self.start_timeout = timeout;
        self
    }
}

/// Handle to a running debugger session.
///
/// Dropping the handle stops the debugger and cleans up every child process.
#[derive(Debug)]
pub struct DebugSession {
    tx: Sender<Msg>,
    events: Receiver<DebugEvent>,
    state: Arc<Mutex<DebuggerState>>,
    actor: Option<JoinHandle<()>>,
    quit_sent: bool,
}

impl DebugSession {
    /// Start a session.
    pub fn start(config: SessionConfig) -> Result<Self, DebugError> {
        config.adapter.validate()?;
        let adapter = config.adapter.clone();
        let patterns = CompiledPatterns::new(&adapter)?;
        if patterns.prompt.is_match("").unwrap_or(false) {
            return Err(DebugError::InvalidAdapter {
                adapter: adapter.id.clone(),
                message: "prompt regex matches the empty string".into(),
            });
        }
        let session_dir = create_session_dir()?;

        // Built-in helpers.
        let mut extra_env = config.env.clone();
        let mut helper_dir = session_dir.clone();
        if !adapter.helpers.python.is_empty() {
            helper_dir = session_dir.join("python");
            std::fs::create_dir_all(&helper_dir)?;
            for helper in &adapter.helpers.python {
                let (name, content) = python_helper(helper).ok_or_else(|| {
                    DebugError::InvalidAdapter {
                        adapter: adapter.id.clone(),
                        message: format!("unknown python helper `{helper}`"),
                    }
                })?;
                std::fs::write(helper_dir.join(name), content)?;
            }
            let existing = std::env::var("PYTHONPATH").unwrap_or_default();
            let value = if existing.is_empty() {
                helper_dir.display().to_string()
            } else {
                format!("{}:{}", helper_dir.display(), existing)
            };
            extra_env.insert("PYTHONPATH".into(), value);
        }
        let init_path = if let Some(name) = &adapter.start.init {
            let content = init_file(name).ok_or_else(|| DebugError::InvalidAdapter {
                adapter: adapter.id.clone(),
                message: format!("unknown init file `{name}`"),
            })?;
            let path = session_dir.join(format!("{name}_init"));
            std::fs::write(&path, content)?;
            Some(path)
        } else {
            None
        };

        let (tx, rx) = mpsc::channel();

        // Program terminal (LLDB needs a separate tty for program I/O).
        let program_io = if adapter.options.program_tty {
            Some(ProgramIo::open(tx.clone())?)
        } else {
            None
        };
        let program_tty = program_io
            .as_ref()
            .map(|p| p.tty_path.display().to_string())
            .unwrap_or_default();

        // Open a free port for attach-style debuggers (jdb).
        let port = if adapter.start.server.is_some() {
            free_port()?
        } else {
            0
        };

        let mut vars = BTreeMap::new();
        vars.insert("program".into(), config.program.display().to_string());
        vars.insert(
            "program_dir".into(),
            config
                .program
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        vars.insert(
            "program_stem".into(),
            config
                .program
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        vars.insert(
            "program_base".into(),
            config
                .program
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        vars.insert("qprogram".into(), quote_double(&config.program.display().to_string()));
        vars.insert("program_args".into(), config.program_args.join(" "));
        vars.insert("program_args_list".into(), join_nul(&config.program_args));
        vars.insert("quoted_args".into(), quote_argv(&config.program_args));
        vars.insert("session_dir".into(), session_dir.display().to_string());
        vars.insert("helper_dir".into(), helper_dir.display().to_string());
        vars.insert(
            "init_file".into(),
            init_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        vars.insert("program_tty".into(), program_tty);
        vars.insert("port".into(), port.to_string());
        vars.insert(
            "runid".into(),
            format!("{}-{}", std::process::id(), epoch_nanos()),
        );

        // Attach-style debuggers need their debuggee started first.
        let server_child = if let Some(argv) = adapter.server_argv(&vars) {
            if argv.is_empty() {
                None
            } else {
                let mut child = spawn_pipe_command(&argv, &config.cwd, &extra_env, true)?;
                if let Some(stdout) = child.stdout.take() {
                    spawn_pump(Box::new(stdout), tx.clone(), false);
                }
                if let Some(stderr) = child.stderr.take() {
                    spawn_pump(Box::new(stderr), tx.clone(), false);
                }
                Some(child)
            }
        } else {
            None
        };

        // The debugger itself.
        let argv = adapter.start_argv(&vars);
        let Some(program) = argv.first() else {
            return Err(DebugError::Spawn("debugger argv is empty".into()));
        };
        if !adapter.availability().is_available() && !Path::new(program).exists()
            && let Availability::Missing { tool, hint } = adapter.availability() {
                return Err(DebugError::ToolNotFound { tool, hint });
            }
        let mut debugger = if adapter.start.pty {
            spawn_debugger_pty(&argv, &config.cwd, &extra_env, tx.clone())?
        } else {
            spawn_debugger_pipes(&argv, &config.cwd, &extra_env, tx.clone())?
        };
        if debugger.pid == 0 {
            debugger.pid = debugger.child.pid().unwrap_or(0);
        }

        let state = Arc::new(Mutex::new(DebuggerState::Starting));
        let (event_tx, event_rx) = mpsc::channel();
        let actor = Actor {
            adapter,
            patterns,
            cfg: config,
            session_dir,
            vars,
            debugger,
            program_io,
            server: server_child,
            event_tx,
            shared_state: Arc::clone(&state),
            rx,
            jobs: VecDeque::new(),
            in_flight: None,
            ready: false,
            started: false,
            state: DebuggerState::Starting,
            decoder: TranscriptDecoder::new(),
            program_decoder: TranscriptDecoder::new(),
            pending: String::new(),
            segment: String::new(),
            segment_scan: 0,
            reset_segment_after_line: false,
            completed_segment: None,
            program_partial: String::new(),
            suppress_echo: None,
            stop_open: false,
            stop_finalized: true,
            termination: None,
            crash_seen: false,
            finished: false,
            started_at: Instant::now(),
            breakpoints: Vec::new(),
            next_bp_id: 1,
            applied: Vec::new(),
            stack: Stack::default(),
            selected_frame: FrameId::main(),
            variable_trees: HashMap::new(),
            watches: Vec::new(),
            watch_batch: None,
            stop_seq: 0,
            refresh_seq: 0,
            prefill_pending: None,
            pause_pending: false,
            pause_deadline: None,
            pause_sent: false,
            prompt_settle: None,
            quit_requested: false,
        };
        let actor = actor.with_breakpoints_from_config();
        let join = thread::Builder::new()
            .name("scratch-debugger".into())
            .spawn(move || actor.run())
            .map_err(|e| DebugError::Spawn(format!("could not start session thread: {e}")))?;
        Ok(Self {
            tx,
            events: event_rx,
            state,
            actor: Some(join),
            quit_sent: false,
        })
    }

    /// The last known state (without waiting for an event).
    pub fn state(&self) -> DebuggerState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or(DebuggerState::Error {
                message: "state lock poisoned".into(),
            })
    }

    /// Event receiver, for callers that prefer to select/poll themselves.
    pub fn events(&self) -> &Receiver<DebugEvent> {
        &self.events
    }

    /// Block for up to `timeout` waiting for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<DebugEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    /// Poll for an event without blocking.
    pub fn try_recv(&self) -> Option<DebugEvent> {
        self.events.try_recv().ok()
    }

    /// Send a raw command.
    pub fn send(&self, command: SessionCommand) -> Result<(), DebugError> {
        self.tx
            .send(Msg::Command(command))
            .map_err(|_| DebugError::SessionGone)
    }

    /// Replace the breakpoint list (see [`crate::Breakpoint`]).
    pub fn set_breakpoints(&self, breakpoints: Vec<Breakpoint>) -> Result<(), DebugError> {
        self.send(SessionCommand::SetBreakpoints(breakpoints))
    }

    /// Remove every breakpoint, using the adapter's clear-all verb when it has
    /// one.
    pub fn clear_all_breakpoints(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::ClearAllBreakpoints)
    }

    /// Ask the debugger to print its breakpoint table.  The raw output arrives
    /// as [`DebugEvent::DebuggerOutput`].
    pub fn list_breakpoints(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::ListBreakpoints)
    }

    /// Resume execution.
    pub fn continue_(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::Continue)
    }

    /// Pause a running program.
    pub fn pause(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::Pause)
    }

    /// Step over.
    pub fn step_over(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::StepOver)
    }

    /// Step in.
    pub fn step_into(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::StepIn)
    }

    /// Step out.
    pub fn step_out(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::StepOut)
    }

    /// Select a frame.
    pub fn select_frame(&self, frame: FrameId) -> Result<(), DebugError> {
        self.send(SessionCommand::SelectFrame(frame))
    }

    /// Fetch a frame's variables.
    pub fn variables(&self, query: VariablesQuery) -> Result<(), DebugError> {
        self.send(SessionCommand::Variables(query))
    }

    /// Expand one variable node.
    pub fn expand_variable(
        &self,
        frame: FrameId,
        node: VariableId,
        depth: u8,
    ) -> Result<(), DebugError> {
        self.send(SessionCommand::ExpandVariable { frame, node, depth })
    }

    /// Edit a variable's value.
    pub fn set_variable(
        &self,
        frame: FrameId,
        path: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), DebugError> {
        self.send(SessionCommand::SetVariable {
            frame,
            path: path.into(),
            value: value.into(),
        })
    }

    /// Print a language-level description.
    pub fn print_description(&self, expr: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::PrintDescription { expr: expr.into() })
    }

    /// Evaluate an expression.
    pub fn evaluate(&self, expr: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::Evaluate { expr: expr.into() })
    }

    /// Add a watch expression.
    pub fn add_watch(&self, expr: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::AddWatch { expr: expr.into() })
    }

    /// Remove a watch expression.
    pub fn remove_watch(&self, expr: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::RemoveWatch { expr: expr.into() })
    }

    /// Send a REPL command.
    pub fn repl(&self, command: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::Repl {
            command: command.into(),
        })
    }

    /// Forward text to the program's stdin.
    pub fn send_program_input(&self, data: impl Into<String>) -> Result<(), DebugError> {
        self.send(SessionCommand::ProgramInput { data: data.into() })
    }

    /// Ask the session to stop.  Events may still arrive until the actor
    /// exits.
    pub fn quit(&self) -> Result<(), DebugError> {
        self.send(SessionCommand::Quit)
    }
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        if !self.quit_sent {
            let _ = self.tx.send(Msg::Command(SessionCommand::Quit));
            self.quit_sent = true;
        }
        if let Some(handle) = self.actor.take() {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Internal messages and process plumbing
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Msg {
    Command(SessionCommand),
    Debugger(Vec<u8>),
    DebuggerEof,
    Program(Vec<u8>),
    ProgramEof,
}

fn spawn_pump(reader: Box<dyn Read + Send>, tx: Sender<Msg>, debugger: bool) {
    let _ = thread::Builder::new()
        .name("scratch-dbg-reader".into())
        .spawn(move || pump(reader, tx, debugger));
}

fn pump(mut reader: Box<dyn Read + Send>, tx: Sender<Msg>, debugger: bool) {
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let msg = if debugger {
                    Msg::Debugger(buffer[..n].to_vec())
                } else {
                    Msg::Program(buffer[..n].to_vec())
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = tx.send(if debugger {
        Msg::DebuggerEof
    } else {
        Msg::ProgramEof
    });
}

struct DebuggerIo {
    child: DebuggerChild,
    writer: Box<dyn Write + Send>,
    pid: u32,
    pgid: i32,
}

impl std::fmt::Debug for DebuggerIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebuggerIo")
            .field("pid", &self.pid)
            .field("pgid", &self.pgid)
            .finish()
    }
}

impl DebuggerIo {
    fn write_line(&mut self, line: &str) -> Result<(), DebugError> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<(), DebugError> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    fn kill_tree(&mut self) {
        if self.pgid > 0 {
            unsafe {
                libc::kill(-self.pgid, libc::SIGTERM);
            }
            thread::sleep(KILL_GRACE);
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
enum DebuggerChild {
    Pipes(std::process::Child),
    Pty(Box<dyn Child + Send + Sync>),
}

impl DebuggerChild {
    fn pid(&self) -> Option<u32> {
        match self {
            DebuggerChild::Pipes(child) => Some(child.id()),
            DebuggerChild::Pty(child) => child.process_id(),
        }
    }

    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            DebuggerChild::Pipes(child) => child.kill(),
            DebuggerChild::Pty(child) => child.kill(),
        }
    }

    fn wait(&mut self) -> std::io::Result<()> {
        match self {
            DebuggerChild::Pipes(child) => {
                let _ = child.wait();
            }
            DebuggerChild::Pty(child) => {
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

struct ProgramIo {
    writer: Option<Box<dyn Write + Send>>,
    tty_path: PathBuf,
    _master: Box<dyn MasterPty + Send>,
    _slave: Box<dyn SlavePty + Send>,
}

impl std::fmt::Debug for ProgramIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramIo")
            .field("tty_path", &self.tty_path)
            .finish()
    }
}

impl ProgramIo {
    fn open(tx: Sender<Msg>) -> Result<Self, DebugError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DebugError::Spawn(format!("program pty: {e}")))?;
        let tty_path = pair
            .master
            .tty_name()
            .ok_or_else(|| DebugError::Spawn("program pty has no tty name".into()))?;
        disable_echo(&tty_path);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| DebugError::Spawn(format!("program pty reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| DebugError::Spawn(format!("program pty writer: {e}")))?;
        spawn_pump(reader, tx, false);
        Ok(Self {
            writer: Some(writer),
            tty_path,
            _master: pair.master,
            _slave: pair.slave,
        })
    }

    fn write(&mut self, data: &str) -> Result<(), DebugError> {
        if let Some(writer) = &mut self.writer {
            writer.write_all(data.as_bytes())?;
            writer.flush()?;
        }
        Ok(())
    }
}

fn disable_echo(tty_path: &Path) {
    use std::ffi::CString;
    let Ok(path) = CString::new(tty_path.as_os_str().as_encoded_bytes()) else {
        return;
    };
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return;
        }
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut term) == 0 {
            term.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
            libc::tcsetattr(fd, libc::TCSANOW, &term);
        }
        libc::close(fd);
    }
}

fn spawn_pipe_command(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    stdin_null: bool,
) -> Result<std::process::Child, DebugError> {
    use std::os::unix::process::CommandExt;
    let Some(program) = argv.first() else {
        return Err(DebugError::Spawn("empty command".into()));
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(if stdin_null {
            std::process::Stdio::null()
        } else {
            std::process::Stdio::piped()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| DebugError::Spawn(format!("{program}: {e}")))
}

fn spawn_debugger_pipes(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    tx: Sender<Msg>,
) -> Result<DebuggerIo, DebugError> {
    let mut child = spawn_pipe_command(argv, cwd, env, false)?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| DebugError::Spawn("debugger stdin unavailable".into()))?;
    if let Some(stdout) = child.stdout.take() {
        spawn_pump(Box::new(stdout), tx.clone(), true);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_pump(Box::new(stderr), tx, true);
    }
    Ok(DebuggerIo {
        child: DebuggerChild::Pipes(child),
        writer: Box::new(stdin),
        pid,
        pgid: pid as i32,
    })
}

fn spawn_debugger_pty(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    tx: Sender<Msg>,
) -> Result<DebuggerIo, DebugError> {
    let Some(program) = argv.first() else {
        return Err(DebugError::Spawn("empty debugger command".into()));
    };
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 48,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| DebugError::Spawn(format!("debugger pty: {e}")))?;
    let mut builder = CommandBuilder::new(program);
    builder.args(&argv[1..]);
    builder.cwd(cwd);
    for (key, value) in env {
        builder.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| DebugError::Spawn(format!("{program}: {e}")))?;
    let pid = child.process_id().unwrap_or(0);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| DebugError::Spawn(format!("debugger pty reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| DebugError::Spawn(format!("debugger pty writer: {e}")))?;
    spawn_pump(reader, tx, true);
    Ok(DebuggerIo {
        child: DebuggerChild::Pty(child),
        writer,
        pid,
        pgid: pid as i32,
    })
}

fn free_port() -> Result<u16, DebugError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| DebugError::Spawn(format!("allocate TCP port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| DebugError::Spawn(format!("allocate TCP port: {e}")))?
        .port();
    drop(listener);
    Ok(port)
}

fn create_session_dir() -> Result<PathBuf, DebugError> {
    let dir = std::env::temp_dir().join(format!(
        "scratch-debugger-{}-{}",
        std::process::id(),
        epoch_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Over,
    In,
    Out,
}

#[derive(Debug)]
enum JobKind {
    SetBreakpoint {
        key: BreakpointKey,
        id: u64,
    },
    ClearBreakpoint {
        key: BreakpointKey,
    },
    ClearAll,
    BreakpointList,
    Launch,
    Continue,
    Step(StepKind),
    /// Second hop of a pdb step-out: pdb stops at the callee's `--Return--`
    /// event, so one more step is needed to land in the caller.
    StepOutFinish,
    PauseCommand,
    Backtrace,
    SelectFrameThread,
    SelectFrame {
        frame: FrameId,
    },
    Variables {
        query: VariablesQuery,
        refresh_watches: bool,
    },
    VariablesAt {
        frame: FrameId,
        node: VariableId,
        query: VariablesQuery,
    },
    Evaluate {
        expr: String,
        kind: ExpressionKind,
    },
    WatchEval {
        expr: String,
    },
    SetVariable {
        frame: FrameId,
        path: String,
        value: String,
    },
    Repl,
    Setup,
}

#[derive(Debug)]
struct Job {
    line: String,
    kind: JobKind,
    stop_seq_at_send: u64,
}

#[derive(Debug, Clone)]
struct AppliedBreakpoint {
    key: BreakpointKey,
    local_id: u64,
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

struct Actor {
    adapter: DebuggerAdapter,
    patterns: CompiledPatterns,
    cfg: SessionConfig,
    session_dir: PathBuf,
    vars: BTreeMap<String, String>,
    debugger: DebuggerIo,
    program_io: Option<ProgramIo>,
    server: Option<std::process::Child>,
    event_tx: Sender<DebugEvent>,
    shared_state: Arc<Mutex<DebuggerState>>,
    rx: Receiver<Msg>,
    jobs: VecDeque<Job>,
    in_flight: Option<Job>,
    ready: bool,
    started: bool,
    state: DebuggerState,
    decoder: TranscriptDecoder,
    program_decoder: TranscriptDecoder,
    pending: String,
    segment: String,
    segment_scan: usize,
    reset_segment_after_line: bool,
    completed_segment: Option<String>,
    program_partial: String,
    suppress_echo: Option<String>,
    stop_open: bool,
    stop_finalized: bool,
    termination: Option<i32>,
    crash_seen: bool,
    finished: bool,
    started_at: Instant,
    breakpoints: Vec<Breakpoint>,
    next_bp_id: u64,
    applied: Vec<AppliedBreakpoint>,
    stack: Stack,
    selected_frame: FrameId,
    variable_trees: HashMap<FrameId, VariableTree>,
    watches: Vec<String>,
    watch_batch: Option<Vec<WatchValue>>,
    stop_seq: u64,
    refresh_seq: u64,
    prefill_pending: Option<String>,
    pause_pending: bool,
    pause_deadline: Option<Instant>,
    pause_sent: bool,
    /// Deadline for treating a prompt at the very end of the input buffer as a
    /// real prompt.  Readline-style debuggers (node) redraw their prompt while
    /// echoing, so a chunk can end in something that looks like a prompt.
    prompt_settle: Option<Instant>,
    quit_requested: bool,
}

impl Actor {
    fn with_breakpoints_from_config(mut self) -> Self {
        self.breakpoints = std::mem::take(&mut self.cfg.breakpoints);
        for bp in &mut self.breakpoints {
            if bp.id == 0 {
                bp.id = self.next_bp_id;
                self.next_bp_id += 1;
            } else {
                self.next_bp_id = self.next_bp_id.max(bp.id + 1);
            }
        }
        self.prefill_pending = self.cfg.program_input.take();
        self
    }

    fn run(mut self) {
        self.emit(DebugEvent::State(DebuggerState::Starting));
        loop {
            if self.finished {
                break;
            }
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Msg::Command(command)) => {
                    if let Err(err) = self.handle_command(command) {
                        self.report_error(err.to_string());
                    }
                }
                Ok(Msg::Debugger(bytes)) => self.on_debugger_bytes(&bytes),
                Ok(Msg::DebuggerEof) => {
                    if !self.finished {
                        if self.termination.is_some() {
                            self.finish_with_exit();
                        } else if self.started {
                            self.fail("debugger process exited unexpectedly");
                        } else {
                            self.fail("debugger exited before it was ready");
                        }
                    }
                }
                Ok(Msg::Program(bytes)) => self.on_program_bytes(&bytes),
                Ok(Msg::ProgramEof) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Some(deadline) = self.pause_deadline
                && Instant::now() >= deadline {
                    self.pause_deadline = None;
                    self.signal_pause();
                }
            if self.prompt_settle.is_some_and(|deadline| Instant::now() >= deadline) {
                self.prompt_settle = None;
                self.process_pending(true);
            }
            if !self.started
                && !self.finished
                && self.started_at.elapsed() > self.cfg.start_timeout
            {
                self.fail("timed out waiting for the debugger prompt");
            }
            if self.finished {
                self.flush_program_output();
                break;
            }
            self.maybe_dispatch();
        }
        self.shutdown();
    }

    // -- event plumbing ---------------------------------------------------

    fn emit(&self, event: DebugEvent) {
        if let DebugEvent::State(state) = &event
            && let Ok(mut shared) = self.shared_state.lock() {
                *shared = state.clone();
            }
        let _ = self.event_tx.send(event);
    }

    fn set_state(&mut self, state: DebuggerState) {
        self.state = state.clone();
        self.emit(DebugEvent::State(state));
    }

    fn report_error(&self, message: impl Into<String>) {
        self.emit(DebugEvent::Error {
            message: message.into(),
        });
    }

    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.emit(DebugEvent::Error {
            message: message.clone(),
        });
        self.set_state(DebuggerState::Error { message });
        self.finished = true;
    }

    fn finish_with_exit(&mut self) {
        let code = self.termination.take().unwrap_or(0);
        self.emit(DebugEvent::Exited { code });
        self.set_state(DebuggerState::Exited { code });
        self.finished = true;
    }

    fn verbose_log(&self, direction: LogDirection, text: impl Into<String>) {
        if self.cfg.verbose {
            self.emit(DebugEvent::LogLine {
                direction,
                text: text.into(),
            });
        }
    }

    // -- input decoding ---------------------------------------------------

    fn on_debugger_bytes(&mut self, bytes: &[u8]) {
        let text = self.decoder.push(bytes);
        if text.is_empty() {
            return;
        }
        self.pending.push_str(&text);
        self.process_pending(false);
    }

    fn on_program_bytes(&mut self, bytes: &[u8]) {
        let text = self.program_decoder.push(bytes);
        if text.is_empty() {
            return;
        }
        self.program_partial.push_str(&text);
        while let Some(position) = self.program_partial.find('\n') {
            let line: String = self.program_partial.drain(..position + 1).collect();
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            if !line.is_empty() {
                self.emit(DebugEvent::Output {
                    stream: OutputStream::Stdout,
                    text: line.to_string(),
                });
            }
        }
    }

    /// Split complete prompt-terminated segments out of the pending buffer.
    ///
    /// `settled` is false while bytes are streaming in.  Readline-based
    /// debuggers such as node redraw their prompt on every keystroke, so the
    /// tail of a chunk can look like a prompt; wait for a moment of silence
    /// before accepting it.
    fn process_pending(&mut self, settled: bool) {
        loop {
            if self.finished {
                return;
            }
            let prompt = self.patterns.find_prompt(&self.pending);
            if let Some((start, end)) = prompt {
                if end <= start {
                    break;
                }
                let tail_prompt = end == self.pending.len();
                if tail_prompt
                    && !settled
                    && self.in_flight.is_some()
                    && matches!(self.adapter.options.echo, EchoMode::Prefix)
                {
                    self.prompt_settle = Some(Instant::now() + Duration::from_millis(30));
                    break;
                }
                self.prompt_settle = None;
                let before: String = self.pending[..start].to_string();
                self.pending.drain(..end);
                self.feed_text(&before);
                if self.finished {
                    return;
                }
                self.finish_segment();
                if self.finished {
                    return;
                }
                self.ready = true;
                self.on_prompt();
            } else {
                let split = match self.pending.rfind('\n') {
                    Some(index) => index + 1,
                    None => 0,
                };
                if split == 0 {
                    break;
                }
                let text: String = self.pending.drain(..split).collect();
                self.feed_text(&text);
                if self.finished {
                    return;
                }
                break;
            }
        }
    }

    fn feed_text(&mut self, text: &str) {
        self.segment.push_str(text);
        while let Some(offset) = self.segment[self.segment_scan..].find('\n') {
            let end = self.segment_scan + offset;
            let line = self.segment[self.segment_scan..end].to_string();
            self.segment_scan = end + 1;
            self.on_line(&line);
            if self.reset_segment_after_line {
                // The stop notification that just finished is not part of the
                // next command's output.  Drop it before the refresh command
                // (`bt all`, `where`) is sent, otherwise its frames would be
                // parsed twice.
                self.reset_segment_after_line = false;
                self.segment.clear();
                self.segment_scan = 0;
            }
        }
    }

    fn finish_segment(&mut self) {
        if self.segment_scan < self.segment.len() {
            let line = self.segment[self.segment_scan..].to_string();
            self.segment_scan = self.segment.len();
            self.on_line(&line);
        }
        let text = std::mem::take(&mut self.segment);
        self.segment_scan = 0;
        self.completed_segment = Some(text.clone());
        self.on_segment(&text);
    }

    // -- line handling ----------------------------------------------------

    fn on_line(&mut self, line: &str) {
        if self.cfg.verbose {
            self.verbose_log(LogDirection::Response, line.to_string());
        }
        let trimmed = line.trim_end();
        // Readline-style debuggers redraw their prompt while typing and, once
        // ANSI cursor moves are stripped, can leave the prompt glued to the
        // first line of real output ("debug> break in app.js:5").  Peel the
        // prompt off and handle the remainder as its own line.
        if let EchoMode::Prefix = self.adapter.options.echo {
            let prefix = self.adapter.options.echo_prefix.as_str();
            if !prefix.is_empty()
                && let Some(rest) = trimmed.strip_prefix(prefix)
            {
                if let Some(command) = self.suppress_echo.clone()
                    && trimmed.trim_end().ends_with(command.trim())
                {
                    self.suppress_echo = None;
                    return;
                }
                let mut rest = rest.trim_start();
                while let Some(stripped) = rest.strip_prefix(prefix) {
                    rest = stripped.trim_start();
                }
                if rest.is_empty() {
                    return;
                }
                self.suppress_echo = None;
                return self.on_line(rest);
            }
        }
        if let Some(command) = self.suppress_echo.clone() {
            if self.is_echo(&command, trimmed) {
                return;
            }
            self.suppress_echo = None;
        }
        if trimmed.is_empty() {
            return;
        }
        if self.patterns.is_ignored(trimmed) {
            return;
        }
        // Termination is checked before the program-output prefix: node prints
        // "Waiting for the debugger to disconnect..." with its `< ` prefix.
        if let Some(code) = self.patterns.termination_code(trimmed) {
            self.termination = Some(code);
            self.emit(DebugEvent::DebuggerOutput {
                text: trimmed.to_string(),
            });
            self.finish_with_exit();
            return;
        }
        if let Some(prefix) = self.adapter.options.program_prefix.clone()
            && let Some(rest) = trimmed.strip_prefix(&prefix) {
                self.emit(DebugEvent::Output {
                    stream: OutputStream::Stdout,
                    text: rest.trim_start().to_string(),
                });
                return;
            }
        if self.pause_pending
            && (trimmed.contains(" launched:") || trimmed.contains(" resuming"))
        {
            self.pause_pending = false;
            // Give the dynamic loader a moment to reach the program's own
            // code; interrupting during dyld startup stops in an internal
            // frame instead of the program.
            self.pause_deadline = Some(Instant::now() + Duration::from_millis(150));
        }
        if self.patterns.is_crash(trimmed) {
            self.crash_seen = true;
        }
        self.detect_stop(trimmed);
        self.emit_debugger_or_program(trimmed);
    }

    fn is_echo(&self, command: &str, line: &str) -> bool {
        match self.adapter.options.echo {
            EchoMode::None => false,
            EchoMode::Line => {
                let line = line.trim();
                let command = command.trim();
                line == command || line.ends_with(command) && line.starts_with('(')
            }
            EchoMode::Prefix => {
                let prefix = self.adapter.options.echo_prefix.as_str();
                !prefix.is_empty() && line.trim_start().starts_with(prefix)
            }
        }
    }

    fn emit_debugger_or_program(&mut self, line: &str) {
        if self.adapter.options.program_tty {
            self.emit(DebugEvent::DebuggerOutput {
                text: line.to_string(),
            });
            return;
        }
        let running = matches!(self.state, DebuggerState::Running);
        let looks_debugger = is_debugger_line(line, self.patterns.kind);
        if running && !looks_debugger {
            self.emit(DebugEvent::Output {
                stream: OutputStream::Stdout,
                text: line.to_string(),
            });
        } else {
            self.emit(DebugEvent::DebuggerOutput {
                text: line.to_string(),
            });
        }
    }

    /// Whether pdb just reported the return event of the frame we asked to
    /// step out of.  `pdb return` stops inside the returning frame, so a
    /// second hop is needed to reach the caller.
    fn is_pdb_return_event(&self) -> bool {
        self.patterns.kind == ParserKind::Pdb
            && matches!(
                self.in_flight.as_ref().map(|job| &job.kind),
                Some(JobKind::Step(StepKind::Out))
            )
            && self.segment.contains("--Return--")
    }

    fn detect_stop(&mut self, line: &str) {
        if self.termination.is_some() {
            return;
        }
        if !self.started || self.stop_finalized {
            if self.patterns.kind == ParserKind::Lldb
                && line.contains("Process ")
                && line.contains("stopped")
            {
                self.stop_open = true;
            }
            return;
        }
        match self.patterns.kind {
            ParserKind::Lldb => {
                if line.contains("Process ") && line.contains("stopped") {
                    self.stop_open = true;
                    return;
                }
                if self.stop_open
                    && (line.contains("Target ")
                        || crate::parser::is_lldb_frame_line(line))
                {
                    self.finalize_stop_from_segment(None);
                }
            }
            ParserKind::Pdb => {
                if line.starts_with("> ") && crate::parser::is_pdb_stop_line(line) {
                    if self.is_pdb_return_event() {
                        // Defer: the caller hop is issued when pdb prompts.
                        return;
                    }
                    self.finalize_stop_from_segment(None);
                }
            }
            ParserKind::Node => {
                if crate::parser::is_node_stop_line(line) {
                    self.finalize_stop_from_segment(None);
                }
            }
            _ => {
                if let Some(re) = &self.patterns.stop_location
                    && re.is_match(line).unwrap_or(false) {
                        self.finalize_stop_from_segment(None);
                    }
            }
        }
    }

    fn finalize_stop_from_segment(&mut self, forced_reason: Option<StopReason>) {
        if self.stop_finalized {
            return;
        }
        let text = self.segment.clone();
        let kind = self.patterns.kind;
        let source_map = &self.cfg.source_map;
        let cwd = &self.cfg.cwd;
        let mut location = None;
        let has_forced_reason = forced_reason.is_some();
        let mut reason = forced_reason.unwrap_or_else(|| self.reason_from_job());
        if !has_forced_reason && self.crash_seen {
            let (name, message) = parse_python_exception(&text);
            reason = StopReason::Exception { name, message };
        }
        match kind {
            ParserKind::Lldb => {
                if let Some((loc, parsed)) =
                    parse_lldb_stop(&text, &self.patterns, source_map, cwd)
                {
                    location = Some(loc);
                    if !has_forced_reason {
                        reason = parsed;
                    }
                }
            }
            ParserKind::Pdb => {
                location = parse_pdb_stop(&text, source_map, cwd);
            }
            ParserKind::Node => {
                if let Some((loc, is_break)) = parse_node_stop(&text, source_map, cwd) {
                    location = Some(loc);
                    if !has_forced_reason {
                        reason = if is_break {
                            StopReason::Breakpoint {
                                temporary: false,
                                id: None,
                            }
                        } else {
                            self.reason_from_job()
                        };
                    }
                }
            }
            _ => {
                location = parse_generic_stop(&text, &self.patterns, source_map, cwd);
            }
        }
        // Some debuggers announce a breakpoint hit separately from the
        // location line; use the adapter's hit pattern as a fallback when the
        // stop would otherwise look like a pause.
        if !has_forced_reason
            && matches!(reason, StopReason::Pause | StopReason::Unknown(_))
            && let Some(hit) = &self.patterns.breakpoint_hit
            && hit.is_match(&text).unwrap_or(false)
        {
            reason = StopReason::Breakpoint {
                temporary: false,
                id: None,
            };
        }
        if kind == ParserKind::Lldb && location.is_none() {
            // The stop notification is split across chunks; wait for the
            // frame line instead of reporting a location-less pause.
            return;
        }
        self.finalize_stop(location, reason);
    }

    fn reason_from_job(&self) -> StopReason {
        match self.in_flight.as_ref().map(|job| &job.kind) {
            Some(JobKind::Step(StepKind::Over)) => StopReason::StepOver,
            Some(JobKind::Step(StepKind::In)) => StopReason::StepIn,
            Some(JobKind::Step(StepKind::Out)) | Some(JobKind::StepOutFinish) => {
                StopReason::StepOut
            }
            Some(JobKind::PauseCommand) => StopReason::Pause,
            Some(JobKind::Launch) | Some(JobKind::Continue) => StopReason::Pause,
            _ => StopReason::Unknown("stop".into()),
        }
    }

    fn finalize_stop(&mut self, location: Option<Location>, reason: StopReason) {
        if self.stop_finalized {
            return;
        }
        self.stop_finalized = true;
        self.stop_open = false;
        self.pause_sent = false;
        self.stop_seq += 1;
        let location = location.unwrap_or_else(|| Location::at_frame(self.selected_frame));
        let reason = self.enrich_reason(reason, &location);
        let frame = location.frame;
        self.selected_frame = frame;
        let annotation = reason.annotation();
        if reason.is_crash() {
            let message = match &reason {
                StopReason::Signal { name } => name.clone(),
                StopReason::Exception { name, message } => match message {
                    Some(message) => format!("{name}: {message}"),
                    None => name.clone(),
                },
                other => other.annotation(),
            };
            self.set_state(DebuggerState::Crashed { message, frame });
        } else {
            self.set_state(DebuggerState::Paused {
                location: Some(location.clone()),
                frame,
                reason: reason.clone(),
            });
        }
        self.emit(DebugEvent::StoppedAt {
            location: location.clone(),
            reason: reason.clone(),
            annotation,
        });
        self.consume_temporary_breakpoint(&location);
        self.reset_segment_after_line = self.in_flight.is_none();
        self.enqueue_refresh();
    }

    fn enrich_reason(&mut self, reason: StopReason, location: &Location) -> StopReason {
        let matched = self.breakpoint_at(location);
        let temporary = matched
            .as_ref()
            .map(|bp| bp.temporary)
            .unwrap_or(false);
        match reason {
            StopReason::Breakpoint { temporary: t, id } => StopReason::Breakpoint {
                temporary: t || temporary,
                id,
            },
            StopReason::Pause | StopReason::Unknown(_) | StopReason::Entry => {
                if self.pause_sent {
                    return StopReason::Pause;
                }
                if let Some(bp) = matched {
                    StopReason::Breakpoint {
                        temporary: bp.temporary,
                        id: bp
                            .debugger_id
                            .as_deref()
                            .and_then(|id| id.parse().ok()),
                    }
                } else {
                    reason
                }
            }
            other => other,
        }
    }

    fn breakpoint_at(&self, location: &Location) -> Option<Breakpoint> {
        let file = location.file.as_ref()?;
        let line = location.line?;
        self.breakpoints
            .iter()
            .find(|bp| {
                bp.is_effective()
                    && bp.line == line
                    && (bp.file == *file
                        || bp.file.file_name() == file.file_name())
            })
            .cloned()
    }

    fn consume_temporary_breakpoint(&mut self, location: &Location) {
        let Some(file) = &location.file else {
            return;
        };
        let Some(line) = location.line else {
            return;
        };
        let matched_id = self
            .breakpoints
            .iter()
            .find(|bp| {
                bp.is_effective()
                    && bp.temporary
                    && bp.line == line
                    && (bp.file == *file || bp.file.file_name() == file.file_name())
            })
            .map(|bp| bp.id);
        if let Some(id) = matched_id {
            self.breakpoints.retain(|bp| bp.id != id);
            self.applied.retain(|bp| bp.local_id != id);
            self.emit(DebugEvent::BreakpointsChanged(self.breakpoints.clone()));
        }
    }

    fn enqueue_refresh(&mut self) {
        if self.refresh_seq == self.stop_seq {
            return;
        }
        self.refresh_seq = self.stop_seq;
        let line = render(&self.adapter.commands.backtrace, &self.vars);
        if line.is_empty() {
            self.emit(DebugEvent::Error {
                message: "this debugger does not support backtraces".into(),
            });
            self.enqueue_variables(self.selected_frame, true, true);
            return;
        }
        self.jobs.push_front(Job {
            line,
            kind: JobKind::Backtrace,
            stop_seq_at_send: self.stop_seq,
        });
    }

    fn enqueue_variables(&mut self, frame: FrameId, refresh_watches: bool, front: bool) {
        let query = VariablesQuery {
            frame,
            depth: self.cfg.depth,
            show_all: self.cfg.show_all,
            show_types: self.cfg.show_types,
            sort: self.cfg.sort,
        };
        let template = self.adapter.commands.variables.clone().unwrap_or_default();
        let job = Job {
            line: self.render_variables_line(&template, &query),
            kind: JobKind::Variables {
                query,
                refresh_watches,
            },
            stop_seq_at_send: self.stop_seq,
        };
        if front {
            self.jobs.push_front(job);
        } else {
            self.jobs.push_back(job);
        }
    }

    fn render_variables_line(&self, template: &str, query: &VariablesQuery) -> String {
        if template.is_empty() {
            return String::new();
        }
        let mut vars = self.vars.clone();
        vars.insert("depth".into(), query.depth.to_string());
        vars.insert(
            "ptr_depth".into(),
            self.adapter.options.ptr_depth.to_string(),
        );
        vars.insert("show_all".into(), u8::from(query.show_all).to_string());
        vars.insert("show_types".into(), u8::from(query.show_types).to_string());
        vars.insert(
            "types_flag".into(),
            if query.show_types {
                self.adapter.flags.types.clone()
            } else {
                String::new()
            },
        );
        vars.insert(
            "globals_flag".into(),
            if query.show_all {
                self.adapter.flags.globals.clone()
            } else {
                String::new()
            },
        );
        vars.insert(
            "max_children".into(),
            self.adapter.options.max_children.to_string(),
        );
        render(template, &vars)
    }

    // -- prompt / job completion -----------------------------------------

    fn on_prompt(&mut self) {
        if !self.started {
            self.started = true;
            self.emit(DebugEvent::Started);
            self.startup();
            return;
        }
        if let Some(job) = self.in_flight.take() {
            let text = self.completed_segment.take().unwrap_or_default();
            self.complete_job(job, &text);
        } else {
            self.completed_segment = None;
            self.on_segment_after_prompt();
        }
    }

    fn on_segment(&mut self, text: &str) {
        if !self.started || self.finished {
            return;
        }
        if self.stop_open && !self.stop_finalized {
            self.finalize_stop_from_segment(None);
        }
        if self.crash_seen && !self.stop_finalized {
            let (name, message) = parse_python_exception(text);
            let location = parse_pdb_stop(text, &self.cfg.source_map, &self.cfg.cwd);
            self.finalize_stop(
                location,
                StopReason::Exception { name, message },
            );
        } else if self.crash_seen && self.stop_finalized
            && let DebuggerState::Paused { reason, .. } = self.state.clone()
                && !reason.is_crash() {
                    let (name, message) = parse_python_exception(text);
                    let location = self.last_location();
                    self.set_state(DebuggerState::Crashed {
                        message: match message {
                            Some(message) => format!("{name}: {message}"),
                            None => name.clone(),
                        },
                        frame: location.as_ref().map(|l| l.frame).unwrap_or(self.selected_frame),
                    });
                }
        self.crash_seen = false;
        if self.termination.is_some() && !self.finished {
            self.finish_with_exit();
        }
    }

    fn on_segment_after_prompt(&mut self) {
        if self.crash_seen {
            let text = self.completed_segment.take().unwrap_or_default();
            let (name, message) = parse_python_exception(&text);
            let location = parse_pdb_stop(&text, &self.cfg.source_map, &self.cfg.cwd);
            self.stop_finalized = false;
            self.finalize_stop(location, StopReason::Exception { name, message });
            self.crash_seen = false;
        }
    }

    fn last_location(&self) -> Option<Location> {
        match &self.state {
            DebuggerState::Paused {
                location: Some(location),
                ..
            } => Some(location.clone()),
            _ => None,
        }
    }

    fn startup(&mut self) {
        self.ready = true;
        let setup = self.adapter.options.setup_commands.clone();
        for line in setup {
            self.jobs.push_back(Job {
                line: render(&line, &self.vars),
                kind: JobKind::Setup,
                stop_seq_at_send: 0,
            });
        }
        self.sync_breakpoints();
        self.emit(DebugEvent::BreakpointsChanged(self.breakpoints.clone()));
        let launch = render(&self.adapter.commands.launch, &self.vars);
        if launch.is_empty() {
            self.fail("this debugger has no launch command");
            return;
        }
        self.jobs.push_back(Job {
            line: launch,
            kind: JobKind::Launch,
            stop_seq_at_send: 0,
        });
        self.maybe_dispatch();
    }

    fn complete_job(&mut self, job: Job, text: &str) {
        match job.kind {
            JobKind::SetBreakpoint { key, id } => {
                let debugger_id = parse_breakpoint_set(text, &self.patterns);
                if text.contains("error:") {
                    self.report_error(format!(
                        "could not set breakpoint at {}:{}",
                        key.file.display(),
                        key.line
                    ));
                }
                if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.id == id) {
                    bp.verified = !text.contains("error:");
                    bp.debugger_id = debugger_id.clone();
                }
                if !self.applied.iter().any(|applied| applied.key == key) {
                    self.applied.push(AppliedBreakpoint { key, local_id: id });
                }
                self.emit(DebugEvent::BreakpointsChanged(self.breakpoints.clone()));
            }
            JobKind::ClearBreakpoint { key } => {
                self.applied.retain(|applied| applied.key != key);
            }
            JobKind::ClearAll => {
                self.applied.clear();
            }
            JobKind::BreakpointList => {
                let text = text.trim();
                if !text.is_empty() {
                    self.emit(DebugEvent::DebuggerOutput {
                        text: text.to_string(),
                    });
                }
            }
            JobKind::Launch | JobKind::Continue => {
                if !self.stop_finalized
                    && (self.adapter.options.prompt_means_paused
                        || job.stop_seq_at_send < self.stop_seq)
                    {
                        self.finalize_stop_from_segment(None);
                    }
                if !self.stop_finalized
                    && matches!(self.state, DebuggerState::Starting | DebuggerState::Paused { .. })
                {
                    self.stop_finalized = false;
                    self.set_state(DebuggerState::Running);
                } else if matches!(self.state, DebuggerState::Starting) {
                    self.set_state(DebuggerState::Running);
                }
                if matches!(job.kind, JobKind::Launch) {
                    self.write_prefill();
                }
                if self.pause_pending {
                    self.pause_pending = false;
                    self.pause_deadline =
                        Some(Instant::now() + Duration::from_millis(150));
                }
            }
            JobKind::Step(step) => {
                // LLDB acknowledges a step with a prompt before the inferior
                // actually stops; the stop arrives asynchronously.  Only
                // finalize here when the adapter's prompt means "paused".
                if self.adapter.options.prompt_means_paused {
                    if step == StepKind::Out
                        && self.patterns.kind == ParserKind::Pdb
                        && text.contains("--Return--")
                    {
                        // pdb halted at the callee's return event; step once
                        // more to reach the caller frame.  The intermediate
                        // event is not reported as a stop.
                        self.stop_finalized = false;
                        self.set_state(DebuggerState::Running);
                        self.jobs.push_front(Job {
                            line: self.adapter.commands.step_over.clone(),
                            kind: JobKind::StepOutFinish,
                            stop_seq_at_send: self.stop_seq,
                        });
                        return;
                    }
                    if !self.stop_finalized {
                        let reason = match step {
                            StepKind::Over => StopReason::StepOver,
                            StepKind::In => StopReason::StepIn,
                            StepKind::Out => StopReason::StepOut,
                        };
                        self.finalize_stop_from_segment(Some(reason));
                    }
                }
            }
            JobKind::StepOutFinish => {
                if !self.stop_finalized && self.adapter.options.prompt_means_paused {
                    self.finalize_stop_from_segment(Some(StopReason::StepOut));
                }
            }
            JobKind::PauseCommand => {
                if !self.stop_finalized && self.adapter.options.prompt_means_paused {
                    self.finalize_stop_from_segment(Some(StopReason::Pause));
                }
            }
            JobKind::Backtrace => {
                let stack = match self.patterns.kind {
                    ParserKind::Lldb => parse_lldb_stack(
                        text,
                        &self.cfg.source_map,
                        &self.cfg.cwd,
                    ),
                    ParserKind::Pdb => parse_pdb_stack(
                        text,
                        &self.patterns.hide_frames,
                        &self.cfg.source_map,
                        &self.cfg.cwd,
                    ),
                    ParserKind::Node => {
                        parse_node_stack(text, &self.cfg.source_map, &self.cfg.cwd)
                    }
                    _ => parse_generic_stack(
                        text,
                        &self.patterns,
                        &self.cfg.source_map,
                        &self.cfg.cwd,
                    ),
                };
                let selected = stack
                    .current()
                    .map(|frame| frame.id())
                    .unwrap_or(self.selected_frame);
                self.stack = stack.clone();
                self.selected_frame = selected;
                self.emit(DebugEvent::StackChanged(stack));
                self.enqueue_variables(selected, true, true);
            }
            JobKind::SelectFrameThread => {}
            JobKind::SelectFrame { frame } => {
                self.selected_frame = frame;
                let line = render(&self.adapter.commands.backtrace, &self.vars);
                if line.is_empty() {
                    self.enqueue_variables(frame, true, true);
                } else {
                    self.jobs.push_front(Job {
                        line,
                        kind: JobKind::Backtrace,
                        stop_seq_at_send: self.stop_seq,
                    });
                }
            }
            JobKind::Variables {
                query,
                refresh_watches,
            } => {
                let frame = self.resolve_frame(query.frame);
                match self.parse_variables(text, &query) {
                    Ok(mut roots) => {
                        truncate_values(&mut roots, self.adapter.options.max_value_chars);
                        sort_variables(&mut roots, query.sort);
                        let mut tree = VariableTree::new(frame, roots);
                        tree.sort(query.sort);
                        self.variable_trees.insert(frame, tree.clone());
                        self.emit(DebugEvent::VariablesChanged {
                            frame,
                            parent: None,
                            variables: tree,
                        });
                    }
                    Err(err) => {
                        self.report_error(err.to_string());
                        self.emit(DebugEvent::VariablesChanged {
                            frame,
                            parent: None,
                            variables: VariableTree::empty(frame),
                        });
                    }
                }
                if refresh_watches {
                    self.start_watch_batch();
                }
            }
            JobKind::VariablesAt {
                frame, node, query, ..
            } => {
                let mut roots = match self.parse_variables(text, &query) {
                    Ok(roots) => roots,
                    Err(err) => {
                        self.report_error(err.to_string());
                        Vec::new()
                    }
                };
                if roots.len() == 1 && !roots[0].children.is_empty() {
                    roots = roots.remove(0).children;
                }
                truncate_values(&mut roots, self.adapter.options.max_value_chars);
                sort_variables(&mut roots, query.sort);
                if let Some(tree) = self.variable_trees.get_mut(&frame) {
                    tree.replace_children(&node, roots.clone());
                }
                self.emit(DebugEvent::VariablesChanged {
                    frame,
                    parent: Some(node),
                    variables: VariableTree::new(frame, roots),
                });
            }
            JobKind::Evaluate { kind, expr } => {
                let value = crate::parser::extract_expression_value(text);
                if kind == ExpressionKind::Watch {
                    self.record_watch_value(&expr, &value);
                } else {
                    self.emit(DebugEvent::ExpressionResult { kind, text: value });
                }
            }
            JobKind::WatchEval { expr } => {
                let value = crate::parser::extract_expression_value(text);
                self.record_watch_value(&expr, &value);
            }
            JobKind::SetVariable {
                frame,
                path,
                value,
            } => {
                let _ = (&path, &value);
                if text.contains("error:") || text.contains("***") {
                    self.report_error(format!("could not assign {path}"));
                }
                self.enqueue_variables(frame, false, true);
            }
            JobKind::Setup => {}
            JobKind::Repl => {
                let value = text.trim().to_string();
                if !value.is_empty() {
                    self.emit(DebugEvent::ExpressionResult {
                        kind: ExpressionKind::Repl,
                        text: value,
                    });
                }
            }
        }
    }

    fn resolve_frame(&self, frame: FrameId) -> FrameId {
        if self.stack.frame(frame).is_some() {
            frame
        } else {
            self.selected_frame
        }
    }

    fn parse_variables(
        &self,
        text: &str,
        query: &VariablesQuery,
    ) -> Result<Vec<Variable>, DebugError> {
        match self.patterns.kind {
            ParserKind::Lldb => Ok(parse_lldb_variables(text)),
            ParserKind::Pdb => match &self.patterns.variables_marker {
                Some(marker) => parse_json_variables(text, marker),
                None => Ok(parse_generic_variables(text, &self.patterns)),
            },
            ParserKind::Node => Ok(Vec::new()),
            _ => {
                let _ = query;
                Ok(parse_generic_variables(text, &self.patterns))
            }
        }
    }

    fn write_prefill(&mut self) {
        if let Some(input) = self.prefill_pending.take()
            && let Err(err) = self.write_program_input(&input) {
                self.report_error(err.to_string());
            }
    }

    fn write_program_input(&mut self, data: &str) -> Result<(), DebugError> {
        if let Some(program) = &mut self.program_io {
            program.write(data)
        } else if matches!(self.state, DebuggerState::Running) {
            self.debugger.write_bytes(data.as_bytes())
        } else {
            Err(DebugError::WrongState(
                "program is not running; input would be read as a debugger command".into(),
            ))
        }
    }

    // -- command handling -------------------------------------------------

    fn handle_command(&mut self, command: SessionCommand) -> Result<(), DebugError> {
        match command {
            SessionCommand::SetBreakpoints(list) => {
                let mut list = list;
                for bp in &mut list {
                    if bp.id == 0 {
                        bp.id = self.next_bp_id;
                        self.next_bp_id += 1;
                    } else {
                        self.next_bp_id = self.next_bp_id.max(bp.id + 1);
                    }
                }
                self.breakpoints = list;
                self.emit(DebugEvent::BreakpointsChanged(self.breakpoints.clone()));
                if self.breakpoints.is_empty() {
                    self.apply_clear_all();
                } else {
                    self.sync_breakpoints();
                }
            }
            SessionCommand::ClearAllBreakpoints => {
                self.breakpoints.clear();
                self.emit(DebugEvent::BreakpointsChanged(Vec::new()));
                self.apply_clear_all();
            }
            SessionCommand::ListBreakpoints => {
                self.require_paused()?;
                let template = self
                    .adapter
                    .commands
                    .breakpoint_list
                    .clone()
                    .unwrap_or_default();
                if template.is_empty() {
                    return Err(DebugError::Unsupported("breakpoint listing"));
                }
                self.jobs.push_back(Job {
                    line: render(&template, &self.vars),
                    kind: JobKind::BreakpointList,
                    stop_seq_at_send: self.stop_seq,
                });
            }
            SessionCommand::Continue => {
                self.require_paused_or_running()?;
                // `stop_finalized` is cleared when the job is actually sent so
                // that output from a refresh command that is still in flight is
                // not mistaken for a new stop.
                let line = render(&self.adapter.commands.continue_cmd, &self.vars);
                self.push_job(line, JobKind::Continue)?;
            }
            SessionCommand::Pause => {
                if !matches!(self.state, DebuggerState::Running) {
                    return Err(DebugError::WrongState(
                        "pause is only valid while the program is running".into(),
                    ));
                }
                if !self.adapter.options.supports_pause {
                    return Err(DebugError::Unsupported("pause"));
                }
                if matches!(
                    self.in_flight.as_ref().map(|job| &job.kind),
                    Some(JobKind::Launch)
                        | Some(JobKind::Continue)
                        | Some(JobKind::Step(_))
                        | Some(JobKind::StepOutFinish)
                ) {
                    // The program is starting; interrupt it as soon as the
                    // debugger reports that it is actually running.
                    self.pause_pending = true;
                    self.stop_finalized = false;
                    return Ok(());
                }
                let command = self.adapter.commands.pause.clone();
                if !command.is_empty() {
                    self.stop_finalized = false;
                    self.push_job(render(&command, &self.vars), JobKind::PauseCommand)?;
                } else if self.adapter.options.pause_with_signal {
                    self.stop_finalized = false;
                    self.signal_pause();
                } else {
                    return Err(DebugError::Unsupported("pause"));
                }
            }
            SessionCommand::StepOver => self.push_step(StepKind::Over)?,
            SessionCommand::StepIn => self.push_step(StepKind::In)?,
            SessionCommand::StepOut => self.push_step(StepKind::Out)?,
            SessionCommand::SelectFrame(frame) => {
                self.require_paused()?;
                self.push_select_frame(frame)?;
            }
            SessionCommand::Variables(mut query) => {
                self.require_paused()?;
                query.frame = self.resolve_frame(query.frame);
                if query.depth == 0 {
                    query.depth = self.cfg.depth;
                }
                let template = self.adapter.commands.variables.clone().unwrap_or_default();
                let job = Job {
                    line: self.render_variables_line(&template, &query),
                    kind: JobKind::Variables {
                        query,
                        refresh_watches: false,
                    },
                    stop_seq_at_send: self.stop_seq,
                };
                self.push_job_struct(job);
            }
            SessionCommand::ExpandVariable { frame, node, depth } => {
                self.require_paused()?;
                self.push_expand(frame, node, depth)?;
            }
            SessionCommand::SetVariable {
                frame,
                path,
                value,
            } => {
                self.require_paused()?;
                let template = self
                    .adapter
                    .commands
                    .set_variable
                    .clone()
                    .ok_or(DebugError::Unsupported("variable assignment"))?;
                let mut vars = self.vars.clone();
                vars.insert("path".into(), path.clone());
                vars.insert("expr".into(), path.clone());
                vars.insert("value".into(), value.clone());
                let line = render(&template, &vars);
                self.jobs.push_back(Job {
                    line,
                    kind: JobKind::SetVariable {
                        frame: self.resolve_frame(frame),
                        path,
                        value,
                    },
                    stop_seq_at_send: self.stop_seq,
                });
            }
            SessionCommand::PrintDescription { expr } => {
                self.require_paused()?;
                let template = self
                    .adapter
                    .commands
                    .print_description
                    .clone()
                    .or_else(|| Some(self.adapter.commands.evaluate.clone()))
                    .filter(|t| !t.is_empty())
                    .ok_or(DebugError::Unsupported("print description"))?;
                let line = self.render_expression(&template, &expr);
                self.jobs.push_back(Job {
                    line,
                    kind: JobKind::Evaluate {
                        expr,
                        kind: ExpressionKind::PrintDescription,
                    },
                    stop_seq_at_send: self.stop_seq,
                });
            }
            SessionCommand::Evaluate { expr } => {
                self.require_paused()?;
                let line = self.render_expression(&self.adapter.commands.evaluate, &expr);
                self.jobs.push_back(Job {
                    line,
                    kind: JobKind::Evaluate {
                        expr,
                        kind: ExpressionKind::Evaluate,
                    },
                    stop_seq_at_send: self.stop_seq,
                });
            }
            SessionCommand::AddWatch { expr } => {
                if !self.watches.iter().any(|w| w == &expr) {
                    self.watches.push(expr.clone());
                }
                if self.state.is_paused() {
                    self.start_watch_batch();
                }
            }
            SessionCommand::RemoveWatch { expr } => {
                self.watches.retain(|w| w != &expr);
                self.emit(DebugEvent::WatchValues(
                    self.watches
                        .iter()
                        .map(|expr| WatchValue {
                            expression: expr.clone(),
                            value: None,
                            error: None,
                        })
                        .collect(),
                ));
            }
            SessionCommand::Repl { command } => {
                if !self.started {
                    return Err(DebugError::WrongState("debugger is not ready".into()));
                }
                if !command.trim().is_empty() {
                    self.jobs.push_back(Job {
                        line: command,
                        kind: JobKind::Repl,
                        stop_seq_at_send: self.stop_seq,
                    });
                }
            }
            SessionCommand::ProgramInput { data } => {
                self.write_program_input(&data)?;
            }
            SessionCommand::Quit => {
                self.quit_requested = true;
                if let Some(quit) = &self.adapter.commands.quit
                    && !quit.is_empty() {
                        let _ = self.debugger.write_line(quit);
                    }
                self.set_state(DebuggerState::Terminated);
                self.finished = true;
            }
        }
        Ok(())
    }

    fn require_paused(&self) -> Result<(), DebugError> {
        if self.state.is_paused() {
            Ok(())
        } else {
            Err(DebugError::WrongState(
                "the debugger is not stopped at a breakpoint or step".into(),
            ))
        }
    }

    fn require_paused_or_running(&self) -> Result<(), DebugError> {
        match self.state {
            DebuggerState::Paused { .. }
            | DebuggerState::Crashed { .. }
            | DebuggerState::Starting => Ok(()),
            DebuggerState::Running => Err(DebugError::WrongState(
                "the program is already running".into(),
            )),
            _ => Err(DebugError::WrongState(
                "the program has already finished".into(),
            )),
        }
    }

    fn push_step(&mut self, step: StepKind) -> Result<(), DebugError> {
        self.require_paused()?;
        let template = match step {
            StepKind::Over => &self.adapter.commands.step_over,
            StepKind::In => &self.adapter.commands.step_in,
            StepKind::Out => &self.adapter.commands.step_out,
        };
        // `stop_finalized` is cleared when the job is actually sent; clearing it
        // here would let output from a refresh command that is still in flight
        // look like a fresh stop.
        self.push_job(render(template, &self.vars), JobKind::Step(step))
    }

    fn push_job(&mut self, line: String, kind: JobKind) -> Result<(), DebugError> {
        let job = Job {
            line,
            kind,
            stop_seq_at_send: self.stop_seq,
        };
        self.push_job_struct(job);
        Ok(())
    }

    fn push_job_struct(&mut self, mut job: Job) {
        if job.line.is_empty() {
            // Unsupported by this adapter: report it and let the queue flow.
            let what = match &job.kind {
                JobKind::Continue => "continue",
                JobKind::Step(_) | JobKind::StepOutFinish => "stepping",
                JobKind::Backtrace => "backtrace",
                JobKind::BreakpointList => "breakpoint listing",
                JobKind::Variables { .. } => "variables",
                JobKind::VariablesAt { .. } => "variable expansion",
                JobKind::Evaluate { .. } => "expression evaluation",
                JobKind::SetVariable { .. } => "variable assignment",
                JobKind::Repl => "REPL",
                JobKind::PauseCommand => "pause",
                JobKind::SelectFrameThread | JobKind::SelectFrame { .. } => "frame selection",
                JobKind::Launch => "launch",
                JobKind::Setup => "setup",
                _ => "command",
            };
            self.report_error(format!("this debugger does not support {what}"));
            match std::mem::replace(&mut job.kind, JobKind::ClearAll) {
                JobKind::Variables { query, .. } => {
                    let frame = query.frame;
                    self.emit(DebugEvent::VariablesChanged {
                        frame,
                        parent: None,
                        variables: VariableTree::empty(frame),
                    });
                }
                JobKind::VariablesAt { frame, node, .. } => {
                    self.emit(DebugEvent::VariablesChanged {
                        frame,
                        parent: Some(node),
                        variables: VariableTree::empty(frame),
                    });
                }
                JobKind::Evaluate { kind, .. } => {
                    self.emit(DebugEvent::ExpressionResult {
                        kind,
                        text: String::new(),
                    });
                }
                _ => {}
            }
            return;
        }
        job.stop_seq_at_send = self.stop_seq;
        self.jobs.push_back(job);
    }

    fn push_select_frame(&mut self, target: FrameId) -> Result<(), DebugError> {
        if let Some(template) = self.adapter.commands.select_frame.clone() {
            if template.is_empty() {
                return Err(DebugError::Unsupported("frame selection"));
            }
            let mut vars = self.vars.clone();
            vars.insert("thread".into(), target.thread.to_string());
            vars.insert("frame".into(), target.index.to_string());
            vars.insert("index".into(), target.index.to_string());
            let rendered = render(&template, &vars);
            let lines: Vec<&str> = rendered
                .split('\n')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            if lines.is_empty() {
                return Err(DebugError::Unsupported("frame selection"));
            }
            for (index, line) in lines.iter().enumerate() {
                let kind = if index + 1 == lines.len() {
                    JobKind::SelectFrame { frame: target }
                } else {
                    JobKind::SelectFrameThread
                };
                self.jobs.push_back(Job {
                    line: (*line).to_string(),
                    kind,
                    stop_seq_at_send: self.stop_seq,
                });
            }
            self.selected_frame = target;
            return Ok(());
        }
        // Relative selection (pdb, perl, bashdb).
        let current = self.stack.current().map(|f| f.index).unwrap_or(0);
        let delta = target.index as i64 - current as i64;
        if delta == 0 {
            self.selected_frame = target;
            self.enqueue_variables(target, true, true);
            return Ok(());
        }
        let (template, count) = if delta > 0 {
            (
                self.adapter
                    .commands
                    .frame_up
                    .clone()
                    .ok_or(DebugError::Unsupported("frame selection"))?,
                delta as u32,
            )
        } else {
            (
                self.adapter
                    .commands
                    .frame_down
                    .clone()
                    .ok_or(DebugError::Unsupported("frame selection"))?,
                (-delta) as u32,
            )
        };
        let mut vars = self.vars.clone();
        vars.insert("count".into(), count.to_string());
        let line = render(&template, &vars);
        self.selected_frame = target;
        self.jobs.push_back(Job {
            line,
            kind: JobKind::SelectFrame { frame: target },
            stop_seq_at_send: self.stop_seq,
        });
        Ok(())
    }

    fn push_expand(
        &mut self,
        frame: FrameId,
        node: VariableId,
        depth: u8,
    ) -> Result<(), DebugError> {
        let frame = self.resolve_frame(frame);
        let Some(tree) = self.variable_trees.get(&frame) else {
            return Err(DebugError::WrongState(
                "variables have not been fetched for this frame".into(),
            ));
        };
        let Some(variable) = tree.find(&node) else {
            return Err(DebugError::WrongState("unknown variable node".into()));
        };
        let expr = variable
            .expr
            .clone()
            .unwrap_or_else(|| variable.name.clone());
        let depth = if depth == 0 { self.cfg.depth.max(2) } else { depth };
        let query = VariablesQuery {
            frame,
            depth,
            show_all: self.cfg.show_all,
            show_types: self.cfg.show_types,
            sort: self.cfg.sort,
        };
        if let Some(template) = &self.adapter.commands.variables_at
            && !template.is_empty() {
                let mut vars = self.vars.clone();
                vars.insert("depth".into(), depth.to_string());
                vars.insert(
                    "ptr_depth".into(),
                    self.adapter.options.ptr_depth.to_string(),
                );
                vars.insert("max_children".into(), self.adapter.options.max_children.to_string());
                vars.insert("expr".into(), expr.clone());
                vars.insert("qexpr".into(), quote_python(&expr));
                vars.insert("path".into(), expr.clone());
                vars.insert(
                    "types_flag".into(),
                    if query.show_types {
                        self.adapter.flags.types.clone()
                    } else {
                        String::new()
                    },
                );
                self.jobs.push_back(Job {
                    line: render(template, &vars),
                    kind: JobKind::VariablesAt {
                        frame,
                        node,
                        query,
                    },
                    stop_seq_at_send: self.stop_seq,
                });
                return Ok(());
            }
        // No subtree command: refetch the frame one level deeper.
        let deeper = VariablesQuery {
            depth: depth.saturating_add(1),
            ..query
        };
        let template = self.adapter.commands.variables.clone().unwrap_or_default();
        self.jobs.push_back(Job {
            line: self.render_variables_line(&template, &deeper),
            kind: JobKind::Variables {
                query: deeper,
                refresh_watches: false,
            },
            stop_seq_at_send: self.stop_seq,
        });
        Ok(())
    }

    fn render_expression(&self, template: &str, expr: &str) -> String {
        let mut vars = self.vars.clone();
        vars.insert("expr".into(), expr.to_string());
        vars.insert("qexpr".into(), quote_python(expr));
        vars.insert("path".into(), expr.to_string());
        vars.insert("qpath".into(), quote_python(expr));
        render(template, &vars)
    }

    fn signal_pause(&mut self) {
        let pid = self.debugger.pid;
        if pid == 0 {
            return;
        }
        let pgid = self.debugger.pgid;
        if pgid > 0 {
            unsafe {
                libc::kill(-pgid, libc::SIGINT);
            }
        } else {
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        }
        self.pause_sent = true;
        self.verbose_log(LogDirection::Command, "<SIGINT>");
    }

    // -- breakpoints ------------------------------------------------------

    /// Clear every breakpoint in the debugger.  Prefer the adapter's single
    /// clear-all command; otherwise remove the applied breakpoints one by one.
    fn apply_clear_all(&mut self) {
        let template = self
            .adapter
            .commands
            .breakpoint_clear_all
            .clone()
            .unwrap_or_default();
        if !template.is_empty() && !self.applied.is_empty() {
            self.applied.clear();
            self.jobs.push_back(Job {
                line: render(&template, &self.vars),
                kind: JobKind::ClearAll,
                stop_seq_at_send: self.stop_seq,
            });
        } else {
            self.sync_breakpoints();
        }
    }

    fn sync_breakpoints(&mut self) {
        let wanted: Vec<(BreakpointKey, u64)> = self
            .breakpoints
            .iter()
            .filter(|bp| bp.is_effective())
            .map(|bp| (bp.key(), bp.id))
            .collect();
        let wanted_keys: Vec<&BreakpointKey> = wanted.iter().map(|(key, _)| key).collect();
        // Removals first.
        let stale: Vec<AppliedBreakpoint> = self
            .applied
            .iter()
            .filter(|applied| !wanted_keys.contains(&&applied.key))
            .cloned()
            .collect();
        for applied in stale {
            self.applied.retain(|a| a.key != applied.key);
            if let Some(template) = self.adapter.commands.breakpoint_clear.clone() {
                let mut vars = self.vars.clone();
                vars.insert(
                    "file".into(),
                    applied.key.file.display().to_string(),
                );
                vars.insert(
                    "file_base".into(),
                    applied
                        .key
                        .file
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                vars.insert(
                    "file_stem".into(),
                    applied
                        .key
                        .file
                        .file_stem()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                vars.insert("line".into(), applied.key.line.to_string());
                self.jobs.push_back(Job {
                    line: render(&template, &vars),
                    kind: JobKind::ClearBreakpoint {
                        key: applied.key.clone(),
                    },
                    stop_seq_at_send: self.stop_seq,
                });
            }
        }
        // Additions.
        for (key, id) in wanted {
            if self.applied.iter().any(|applied| applied.key == key) {
                continue;
            }
            let Some(bp) = self.breakpoints.iter().find(|bp| bp.id == id).cloned() else {
                continue;
            };
            let (template, append_temporary) = if bp.temporary {
                match &self.adapter.commands.breakpoint_set_temporary {
                    Some(template) => (template.clone(), false),
                    None => (self.adapter.commands.breakpoint_set.clone(), true),
                }
            } else {
                (self.adapter.commands.breakpoint_set.clone(), false)
            };
            let mut vars = self.vars.clone();
            vars.insert("file".into(), bp.file.display().to_string());
            vars.insert(
                "file_base".into(),
                bp.file
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            vars.insert(
                "file_stem".into(),
                bp.file
                    .file_stem()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            vars.insert("line".into(), bp.line.to_string());
            vars.insert(
                "condition".into(),
                bp.condition.clone().unwrap_or_default(),
            );
            vars.insert(
                "qcondition".into(),
                bp.condition
                    .as_deref()
                    .map(quote_double)
                    .unwrap_or_default(),
            );
            let condition_clause = match (&bp.condition, &self.adapter.commands.condition_clause) {
                (Some(_), Some(template)) => {
                    let clause_vars = vars.clone();
                    render(template, &clause_vars)
                }
                _ => String::new(),
            };
            clause_vars_placeholder(&mut vars, "condition_clause", condition_clause);
            let temporary_clause = if append_temporary {
                self.adapter
                    .commands
                    .temporary_clause
                    .as_ref()
                    .map(|template| render(template, &vars))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            vars.insert("temporary_clause".into(), temporary_clause);
            let line = render(&template, &vars);
            if line.is_empty() {
                continue;
            }
            self.jobs.push_back(Job {
                line,
                kind: JobKind::SetBreakpoint { key, id },
                stop_seq_at_send: self.stop_seq,
            });
        }
    }

    // -- watch expressions ------------------------------------------------

    fn start_watch_batch(&mut self) {
        if self.watches.is_empty() {
            return;
        }
        let values: Vec<WatchValue> = self
            .watches
            .iter()
            .map(|expr| WatchValue {
                expression: expr.clone(),
                value: None,
                error: None,
            })
            .collect();
        self.watch_batch = Some(values);
        for expr in self.watches.clone() {
            let line = self.render_expression(&self.adapter.commands.evaluate, &expr);
            if line.is_empty() {
                self.record_watch_value(&expr, "unsupported");
                continue;
            }
            self.jobs.push_back(Job {
                line,
                kind: JobKind::WatchEval { expr },
                stop_seq_at_send: self.stop_seq,
            });
        }
        self.maybe_finish_watch_batch();
    }

    fn record_watch_value(&mut self, expr: &str, value: &str) {
        if let Some(batch) = &mut self.watch_batch
            && let Some(entry) = batch.iter_mut().find(|v| v.expression == expr) {
                if value.starts_with("error") || value.starts_with("***") {
                    entry.error = Some(value.to_string());
                } else {
                    entry.value = Some(value.to_string());
                }
            }
        self.maybe_finish_watch_batch();
    }

    fn maybe_finish_watch_batch(&mut self) {
        let pending = self.jobs.iter().any(|job| matches!(job.kind, JobKind::WatchEval { .. }))
            || matches!(
                self.in_flight.as_ref().map(|job| &job.kind),
                Some(JobKind::WatchEval { .. })
            );
        if !pending
            && let Some(batch) = self.watch_batch.take() {
                self.emit(DebugEvent::WatchValues(batch));
            }
    }

    // -- dispatch ---------------------------------------------------------

    fn maybe_dispatch(&mut self) {
        if self.finished || self.in_flight.is_some() || !self.ready {
            return;
        }
        loop {
            let Some(job) = self.jobs.pop_front() else {
                return;
            };
            // The second hop of a pdb step-out runs while the session still
            // considers itself Running (the callee return event is not
            // reported as a stop), but pdb is sitting at its prompt.
            let internal_hop = matches!(job.kind, JobKind::StepOutFinish);
            if !internal_hop && !self.can_send() {
                self.jobs.push_front(job);
                return;
            }
            if job.line.is_empty() {
                self.complete_job_without_prompt(job);
                continue;
            }
            if let Err(err) = self.debugger.write_line(&job.line) {
                self.fail(format!("could not send debugger command: {err}"));
                return;
            }
            self.verbose_log(LogDirection::Command, job.line.clone());
            self.suppress_echo = Some(job.line.clone());
            self.ready = false;
            if matches!(
                job.kind,
                JobKind::Launch
                    | JobKind::Continue
                    | JobKind::Step(_)
                    | JobKind::StepOutFinish
                    | JobKind::PauseCommand
            ) {
                self.stop_finalized = false;
                if !matches!(job.kind, JobKind::PauseCommand) {
                    self.set_state(DebuggerState::Running);
                }
            }
            self.in_flight = Some(Job {
                stop_seq_at_send: self.stop_seq,
                ..job
            });
            if matches!(
                self.in_flight.as_ref().map(|j| &j.kind),
                Some(JobKind::Launch)
            ) {
                self.write_prefill();
            }
            return;
        }
    }

    fn can_send(&self) -> bool {
        if self.adapter.options.requires_paused_commands {
            self.state.is_paused() || matches!(self.state, DebuggerState::Starting)
        } else {
            true
        }
    }

    fn complete_job_without_prompt(&mut self, job: Job) {
        match &job.kind {
            JobKind::Variables { query, .. } => {
                let frame = query.frame;
                self.emit(DebugEvent::VariablesChanged {
                    frame,
                    parent: None,
                    variables: VariableTree::empty(frame),
                });
            }
            JobKind::VariablesAt { frame, node, .. } => {
                self.emit(DebugEvent::VariablesChanged {
                    frame: *frame,
                    parent: Some(node.clone()),
                    variables: VariableTree::empty(*frame),
                });
            }
            JobKind::Evaluate { kind, .. } => {
                self.emit(DebugEvent::ExpressionResult {
                    kind: *kind,
                    text: String::new(),
                });
            }
            JobKind::Backtrace => {
                self.emit(DebugEvent::StackChanged(Stack::default()));
                self.enqueue_variables(self.selected_frame, true, true);
            }
            _ => {}
        }
    }

    // -- cleanup ----------------------------------------------------------

    fn flush_program_output(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(25)) {
                Ok(Msg::Program(bytes)) => self.on_program_bytes(&bytes),
                Ok(Msg::ProgramEof) => break,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn shutdown(&mut self) {
        if !self.program_partial.is_empty() {
            let line = std::mem::take(&mut self.program_partial);
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            if !line.is_empty() {
                self.emit(DebugEvent::Output {
                    stream: OutputStream::Stdout,
                    text: line.to_string(),
                });
            }
        }
        if let Some(server) = &mut self.server {
            let pid = server.id() as i32;
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            thread::sleep(KILL_GRACE);
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = server.kill();
            let _ = server.wait();
        }
        self.debugger.kill_tree();
        let _ = std::fs::remove_dir_all(&self.session_dir);
        if self.quit_requested && !matches!(self.state, DebuggerState::Terminated) {
            self.set_state(DebuggerState::Terminated);
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn clause_vars_placeholder(vars: &mut BTreeMap<String, String>, key: &str, value: String) {
    vars.insert(key.to_string(), value);
}

/// Lines that are clearly debugger transcript rather than program output.
fn is_debugger_line(line: &str, kind: ParserKind) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return true;
    }
    let common = line.starts_with("Process ")
        || line.starts_with("* thread")
        || line.starts_with("thread #")
        || line.starts_with("frame #")
        || line.starts_with("Target ")
        || line.starts_with("Breakpoint ")
        || line.starts_with("Deleted breakpoint")
        || line.starts_with("Resuming thread")
        || line.starts_with("--Call--")
        || line.starts_with("--Return--")
        || line.starts_with("Traceback ")
        || line.starts_with("The program finished")
        || line.starts_with("Uncaught exception")
        || line.starts_with("Waiting for the debugger")
        || line.starts_with("Debugger ")
        || line.starts_with("< ");
    if common {
        return true;
    }
    match kind {
        ParserKind::Pdb => {
            line.starts_with("> ")
                || line.starts_with("-> ")
                || line.contains("(Pdb)")
                || line.starts_with("***")
        }
        ParserKind::Node => line.starts_with("debug> ") || line.starts_with("break in ") || line.starts_with("step in "),
        ParserKind::Lldb => line.contains("stop reason =") || line.starts_with("error:"),
        _ => false,
    }
}

/// Generic stop location parser for adapters that only provide regexes.
fn parse_generic_stop(
    text: &str,
    patterns: &CompiledPatterns,
    source_map: &SourceMap,
    cwd: &Path,
) -> Option<Location> {
    let re = patterns.stop_location.as_ref()?;
    let mut location = None;
    for caps in re.captures_iter(text).flatten() {
        let file = caps
            .name("file")
            .map(|m| source_map.resolve_in(Path::new(m.as_str()), cwd));
        let line = caps
            .name("line")
            .and_then(|m| m.as_str().parse::<u32>().ok());
        let function = caps
            .name("function")
            .or_else(|| caps.name("name"))
            .map(|m| m.as_str().to_string());
        let thread = caps
            .name("thread")
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(1);
        let index = caps
            .name("frame")
            .or_else(|| caps.name("index"))
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        location = Some(Location {
            frame: FrameId { thread, index },
            file,
            line,
            function,
        });
    }
    location
}
