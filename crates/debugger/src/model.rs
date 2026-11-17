//! Frames, stop reasons, session states, events and commands.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::variables::{SortMode, VariableId, VariableTree};

/// Thread identifier as reported by the debugger.
pub type ThreadId = u32;

/// A frame address that is stable across a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameId {
    /// Debugger thread number (1 on LLDB, 1 for pdb's pseudo thread).
    pub thread: ThreadId,
    /// Frame index within the thread (0 = innermost).
    pub index: u32,
}

impl FrameId {
    /// The most common "current frame" value.
    pub const fn main() -> Self {
        Self { thread: 1, index: 0 }
    }
}

impl std::fmt::Display for FrameId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}#{}", self.thread, self.index)
    }
}

/// One stack frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// Thread the frame belongs to.
    pub thread: ThreadId,
    /// Frame number inside the thread.
    pub index: u32,
    /// Function or method name.
    pub name: String,
    /// Source file, when the frame has debug information.
    pub file: Option<PathBuf>,
    /// One-based source line.
    pub line: Option<u32>,
    /// True for the frame the debugger has selected.
    pub current: bool,
}

impl Frame {
    /// A stable id for this frame.
    pub fn id(&self) -> FrameId {
        FrameId {
            thread: self.thread,
            index: self.index,
        }
    }
}

/// One debugger thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// Thread number.
    pub id: ThreadId,
    /// Name or description (queue name in LLDB).
    pub name: String,
    /// True for the selected thread.
    pub selected: bool,
}

/// The stack reported for a stop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// Threads, in debugger order.
    pub threads: Vec<Thread>,
    /// Frames, all threads, in debugger order.
    pub frames: Vec<Frame>,
}

impl Stack {
    /// The selected frame, if the debugger marked one.
    pub fn current(&self) -> Option<&Frame> {
        self.frames
            .iter()
            .find(|f| f.current)
            .or_else(|| self.frames.iter().find(|f| f.index == 0))
    }

    /// Look up a frame by id.
    pub fn frame(&self, id: FrameId) -> Option<&Frame> {
        self.frames
            .iter()
            .find(|f| f.thread == id.thread && f.index == id.index)
    }

    /// Frames of one thread, in order.
    pub fn thread_frames(&self, thread: ThreadId) -> impl Iterator<Item = &Frame> {
        self.frames.iter().filter(move |f| f.thread == thread)
    }

    /// True when the stack has no frames at all.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Where the program is stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Frame that owns the location.
    pub frame: FrameId,
    /// Source file.
    pub file: Option<PathBuf>,
    /// One-based line.
    pub line: Option<u32>,
    /// Function name.
    pub function: Option<String>,
}

impl Location {
    /// A location that only carries a frame.
    pub fn at_frame(frame: FrameId) -> Self {
        Self {
            frame,
            file: None,
            line: None,
            function: None,
        }
    }

    /// Short human description, used by the stack and the console.
    pub fn describe(&self) -> String {
        let file = self
            .file
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_else(|| "<unknown>".into());
        let line = self.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
        match &self.function {
            Some(function) => format!("{function} at {file}:{line}"),
            None => format!("{file}:{line}"),
        }
    }
}

/// Why the debugger stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// A breakpoint (normal or temporary/one-shot) was hit.
    Breakpoint {
        /// True for a temporary breakpoint.
        temporary: bool,
        /// Debugger-side breakpoint id, when it was reported.
        id: Option<u32>,
    },
    /// A step-over completed.
    StepOver,
    /// A step-in completed.
    StepIn,
    /// A step-out completed.
    StepOut,
    /// The user paused execution (SIGINT / pause verb).
    Pause,
    /// The debugger stopped at its initial entry point.
    Entry,
    /// A signal stopped the program.
    Signal {
        /// Signal name, e.g. `SIGSEGV`.
        name: String,
    },
    /// An uncaught language exception stopped the program.
    Exception {
        /// Exception class.
        name: String,
        /// Exception message, when available.
        message: Option<String>,
    },
    /// A reason we could not classify.
    Unknown(String),
}

impl StopReason {
    /// True when this stop is a breakpoint hit.
    pub fn is_breakpoint(&self) -> bool {
        matches!(self, StopReason::Breakpoint { .. })
    }

    /// True when the breakpoint that stopped us is temporary.
    pub fn is_temporary_breakpoint(&self) -> bool {
        matches!(
            self,
            StopReason::Breakpoint {
                temporary: true,
                ..
            }
        )
    }

    /// The annotation shown next to the stopped line in the editor.
    ///
    /// These strings mirror CodeRunner: "Stopped at Breakpoint",
    /// "Stopped at Temporary Breakpoint", "Paused Execution", ...
    pub fn annotation(&self) -> String {
        match self {
            StopReason::Breakpoint { temporary: false, .. } => "Stopped at Breakpoint".into(),
            StopReason::Breakpoint { temporary: true, .. } => {
                "Stopped at Temporary Breakpoint".into()
            }
            StopReason::StepOver => "Stopped at Step Over".into(),
            StopReason::StepIn => "Stopped at Step In".into(),
            StopReason::StepOut => "Stopped at Step Out".into(),
            StopReason::Pause | StopReason::Entry => "Paused Execution".into(),
            StopReason::Signal { name } => format!("Stopped at Signal {name}"),
            StopReason::Exception { name, .. } => format!("Stopped at Exception {name}"),
            StopReason::Unknown(reason) => format!("Stopped ({reason})"),
        }
    }

    /// Whether this reason represents a crash for session state purposes.
    pub fn is_crash(&self) -> bool {
        matches!(self, StopReason::Signal { .. } | StopReason::Exception { .. })
    }
}

/// Session state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebuggerState {
    /// The debugger process is starting and we have not seen its prompt.
    Starting,
    /// The program is executing.
    Running,
    /// The program is stopped and commands can be used.
    Paused {
        /// Where the program stopped, when known.
        location: Option<Location>,
        /// Current frame.
        frame: FrameId,
        /// Why it stopped.
        reason: StopReason,
    },
    /// The program exited normally (or with a status).
    Exited {
        /// Process exit status.
        code: i32,
    },
    /// The program crashed (signal or uncaught exception); it may still be
    /// inspected until the session is stopped.
    Crashed {
        /// Signal/exception description.
        message: String,
        /// Frame the crash happened in.
        frame: FrameId,
    },
    /// The session failed.
    Error {
        /// Human readable description.
        message: String,
    },
    /// The user stopped the session.
    Terminated,
}

impl DebuggerState {
    /// True when commands that need a stopped program are valid.
    pub fn is_paused(&self) -> bool {
        matches!(
            self,
            DebuggerState::Paused { .. } | DebuggerState::Crashed { .. }
        )
    }

    /// Short status string.
    pub fn status_text(&self) -> &'static str {
        match self {
            DebuggerState::Starting => "Starting Debugger...",
            DebuggerState::Running => "Running...",
            DebuggerState::Paused { .. } => "Paused",
            DebuggerState::Exited { .. } => "Run Ended",
            DebuggerState::Crashed { .. } => "Debugger Error",
            DebuggerState::Error { .. } => "Debugger Error",
            DebuggerState::Terminated => "Debugger Stopped",
        }
    }
}

/// Which program stream a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// Direction of a debugger log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogDirection {
    /// A command we sent to the debugger.
    Command,
    /// A line the debugger produced.
    Response,
}

/// What produced an [`DebugEvent::ExpressionResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpressionKind {
    /// `Evaluate` / an expression in the REPL.
    Evaluate,
    /// `Print Description` (`po` in LLDB, `pp` in pdb).
    PrintDescription,
    /// A free-form debugger REPL command's textual result.
    Repl,
    /// A watch expression evaluation.
    Watch,
}

/// Events emitted by a debug session.
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// The state machine changed.
    State(DebuggerState),
    /// The debugger is ready for commands (its prompt was seen).
    Started,
    /// A line of the debuggee's output.
    Output {
        /// Stream.
        stream: OutputStream,
        /// Line contents without the trailing newline.
        text: String,
    },
    /// A line produced by the debugger itself (command output, notices).
    DebuggerOutput {
        /// Line contents.
        text: String,
    },
    /// One line of the raw debugger log, emitted when verbose mode is on.
    LogLine {
        /// Command or response.
        direction: LogDirection,
        /// Text.
        text: String,
    },
    /// The program stopped.  `annotation` is the "Stopped at ..." text.
    StoppedAt {
        /// Where.
        location: Location,
        /// Why.
        reason: StopReason,
        /// Editor annotation text.
        annotation: String,
    },
    /// The stack was refreshed.
    StackChanged(Stack),
    /// Variables were refreshed.  `parent` is set for a lazily expanded
    /// subtree, whose roots should replace the children of that node.
    VariablesChanged {
        /// Frame the variables belong to.
        frame: FrameId,
        /// Node that was expanded, if any.
        parent: Option<VariableId>,
        /// Variable roots (or the expanded subtree).
        variables: VariableTree,
    },
    /// The breakpoint list changed (including verified/disabled/ghost flags).
    BreakpointsChanged(Vec<crate::breakpoints::Breakpoint>),
    /// Watch expressions were evaluated at a stop.
    WatchValues(Vec<crate::variables::WatchValue>),
    /// The text result of an expression or REPL command.
    ExpressionResult {
        /// What produced it.
        kind: ExpressionKind,
        /// Cleaned result text.
        text: String,
    },
    /// The program exited.  The session shuts down after this event.
    Exited {
        /// Exit status.
        code: i32,
    },
    /// The session failed.
    Error {
        /// Description.
        message: String,
    },
}

/// Variable-query options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariablesQuery {
    /// Frame to inspect.
    pub frame: FrameId,
    /// Child depth to fetch.
    pub depth: u8,
    /// Include file/global/static variables.
    pub show_all: bool,
    /// Include type names (the UI's "Show Types").
    pub show_types: bool,
    /// Presentation order.
    pub sort: SortMode,
}

impl VariablesQuery {
    /// Query with the session's default depth and flags.
    pub fn new(frame: FrameId) -> Self {
        Self {
            frame,
            depth: 2,
            show_all: false,
            show_types: true,
            sort: SortMode::Natural,
        }
    }
}

/// Commands accepted by a [`crate::DebugSession`].
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Replace the breakpoint list.  Disabled and ghost breakpoints are kept
    /// locally but never sent to the debugger.
    SetBreakpoints(Vec<crate::breakpoints::Breakpoint>),
    /// Remove every breakpoint, locally and in the debugger.
    ClearAllBreakpoints,
    /// Print the debugger's breakpoint table as `DebuggerOutput`.
    ListBreakpoints,
    /// Resume execution.
    Continue,
    /// Pause a running program (the adapter's pause command or SIGINT).
    Pause,
    /// Step over the current statement.
    StepOver,
    /// Step into the current call.
    StepIn,
    /// Step out of the current function.
    StepOut,
    /// Select a stack frame.
    SelectFrame(FrameId),
    /// Fetch variables for a frame.
    Variables(VariablesQuery),
    /// Fetch the subtree under one variable node (lazy expansion).
    ExpandVariable {
        /// Frame the node belongs to.
        frame: FrameId,
        /// Node to expand.
        node: VariableId,
        /// Desired depth below the node (0 = the session default + 1).
        depth: u8,
    },
    /// Assign a value to a variable expression.
    SetVariable {
        /// Debugger expression of the variable (from [`crate::Variable::expr`]).
        path: String,
        /// New value, as a language expression.
        value: String,
        /// Frame the variable belongs to.
        frame: FrameId,
    },
    /// Print a language-level description (`po` / `pp`).
    PrintDescription {
        /// Expression.
        expr: String,
    },
    /// Evaluate an expression and return its value.
    Evaluate {
        /// Expression.
        expr: String,
    },
    /// Add a watch expression; it is evaluated at every stop.
    AddWatch {
        /// Expression.
        expr: String,
    },
    /// Remove a watch expression.
    RemoveWatch {
        /// Expression.
        expr: String,
    },
    /// Send a raw debugger command (the REPL).
    Repl {
        /// Command line.
        command: String,
    },
    /// Forward text to the running program's stdin.
    ProgramInput {
        /// Bytes to write.
        data: String,
    },
    /// Stop the session and clean up all processes.
    Quit,
}
