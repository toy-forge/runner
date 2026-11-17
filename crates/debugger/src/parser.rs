//! Transcript decoding and output parsers.
//!
//! Debugger output arrives in arbitrary chunks.  [`AnsiStripper`] removes
//! terminal control sequences (node's REPL and pdb's readline produce a lot of
//! them), `CompiledPatterns` holds the adapter's regexes, and the
//! `parse_*` functions turn complete pieces of transcript into stacks,
//! locations and variable trees.
//!
//! The specialised parsers (`lldb`, `pdb`, `node`) know the exact layout their
//! debugger emits; the generic parser uses the adapter's named-group regexes.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use fancy_regex::Regex;

use crate::adapter::{DebuggerAdapter, ParserKind};
use crate::error::DebugError;
use crate::model::{Frame, FrameId, Location, Stack, StopReason, Thread};
use crate::variables::{Variable, VariableKind};

/// Adapter regexes compiled once per session.
#[derive(Debug)]
pub(crate) struct CompiledPatterns {
    pub prompt: Regex,
    pub termination: Option<Regex>,
    pub crash: Option<Regex>,
    pub ignore: Option<Regex>,
    pub stop_reason: Option<Regex>,
    pub stop_location: Option<Regex>,
    pub frames: Option<Regex>,
    pub threads: Option<Regex>,
    pub breakpoint_hit: Option<Regex>,
    pub breakpoint_set: Option<Regex>,
    pub variables: Option<Regex>,
    pub variables_marker: Option<String>,
    pub kind: ParserKind,
    pub reverse_frames: bool,
    pub hide_frames: Vec<String>,
}

impl CompiledPatterns {
    /// Compile all patterns of an adapter.
    pub fn new(adapter: &DebuggerAdapter) -> Result<Self, DebugError> {
        let compile = |name: &str, text: &str| -> Result<Regex, DebugError> {
            Regex::new(text).map_err(|e| DebugError::InvalidAdapter {
                adapter: adapter.id.clone(),
                message: format!("{name}: {e}"),
            })
        };
        let opt = |name: &str, text: &Option<String>| -> Result<Option<Regex>, DebugError> {
            match text {
                Some(text) if !text.is_empty() => compile(name, text).map(Some),
                _ => Ok(None),
            }
        };
        Ok(Self {
            prompt: compile("prompt", &adapter.prompt)?,
            termination: opt("termination", &adapter.termination)?,
            crash: opt("crash", &adapter.crash)?,
            ignore: opt("ignore", &adapter.ignore)?,
            stop_reason: opt("parsers.stop_reason", &adapter.parsers.stop_reason)?,
            stop_location: opt("parsers.stop_location", &adapter.parsers.stop_location)?,
            frames: opt("parsers.frames", &adapter.parsers.frames)?,
            threads: opt("parsers.threads", &adapter.parsers.threads)?,
            breakpoint_hit: opt("parsers.breakpoint_hit", &adapter.parsers.breakpoint_hit)?,
            breakpoint_set: opt("parsers.breakpoint_set", &adapter.parsers.breakpoint_set)?,
            variables: opt("parsers.variables", &adapter.parsers.variables)?,
            variables_marker: adapter
                .parsers
                .variables_marker
                .clone()
                .filter(|m| !m.is_empty()),
            kind: adapter.parsers.kind,
            reverse_frames: adapter.parsers.reverse_frames,
            hide_frames: if adapter.parsers.hide_frames.is_empty() {
                adapter.options.hide_frames.clone()
            } else {
                adapter.parsers.hide_frames.clone()
            },
        })
    }

    /// Find the first prompt in `text`.
    pub fn find_prompt(&self, text: &str) -> Option<(usize, usize)> {
        self.prompt
            .find(text)
            .ok()
            .flatten()
            .map(|m| (m.start(), m.end()))
    }

    /// Parse the exit status from a termination line.
    pub fn termination_code(&self, line: &str) -> Option<i32> {
        let re = self.termination.as_ref()?;
        let caps = re.captures(line).ok().flatten()?;
        let status = caps.get(1).map(|m| m.as_str()).unwrap_or("0");
        status.parse().ok()
    }

    /// True when a line is a crash marker.
    pub fn is_crash(&self, line: &str) -> bool {
        self.crash
            .as_ref()
            .is_some_and(|re| re.is_match(line).unwrap_or(false))
    }

    /// True when a line should be dropped.
    pub fn is_ignored(&self, line: &str) -> bool {
        self.ignore
            .as_ref()
            .is_some_and(|re| re.is_match(line).unwrap_or(false))
    }
}

/// Streaming ANSI escape-sequence stripper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AnsiState {
    #[default]
    Normal,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Charset,
}

/// Removes CSI/OSC/ESC sequences and C0 control characters from a chunked
/// stream.  Newlines, carriage returns and tabs are preserved.
#[derive(Debug, Default)]
pub struct AnsiStripper {
    state: AnsiState,
}

impl AnsiStripper {
    /// New stripper.
    pub fn new() -> Self {
        Self::default()
    }

    /// Strip one chunk.  The state carries over between calls.
    pub fn strip(&mut self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            self.state = match self.state {
                AnsiState::Normal => match ch {
                    '\u{1b}' => AnsiState::Escape,
                    '\n' | '\r' | '\t' => {
                        out.push(ch);
                        AnsiState::Normal
                    }
                    c if (c as u32) < 0x20 => AnsiState::Normal,
                    c => {
                        out.push(c);
                        AnsiState::Normal
                    }
                },
                AnsiState::Escape => match ch {
                    '[' => AnsiState::Csi,
                    ']' => AnsiState::Osc,
                    '(' | ')' => AnsiState::Charset,
                    _ => AnsiState::Normal,
                },
                AnsiState::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        AnsiState::Normal
                    } else {
                        AnsiState::Csi
                    }
                }
                AnsiState::Osc => match ch {
                    '\u{7}' => AnsiState::Normal,
                    '\u{1b}' => AnsiState::OscEscape,
                    _ => AnsiState::Osc,
                },
                AnsiState::OscEscape => {
                    if ch == '\\' {
                        AnsiState::Normal
                    } else {
                        AnsiState::Osc
                    }
                }
                AnsiState::Charset => AnsiState::Normal,
            };
        }
        out
    }
}

/// One-shot ANSI stripping, handy in tests.
pub fn strip_ansi(text: &str) -> String {
    AnsiStripper::new().strip(text)
}

/// Normalise newlines: CRLF becomes LF and stray carriage returns are
/// dropped.  Terminal line editors use a bare CR to repaint the same line.
pub fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
                out.push('\n');
            }
            // A lone CR is a cursor return; drop it.
        } else {
            out.push(ch);
        }
    }
    out
}

/// Combined decoder used by the session.
#[derive(Debug, Default)]
pub struct TranscriptDecoder {
    ansi: AnsiStripper,
    utf8: Vec<u8>,
}

impl TranscriptDecoder {
    /// New decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns decoded, stripped, newline-normalised text.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.utf8.extend_from_slice(bytes);
        let text = match std::str::from_utf8(&self.utf8) {
            Ok(text) => {
                let text = text.to_string();
                self.utf8.clear();
                text
            }
            Err(err) => {
                let valid = err.valid_up_to();
                let mut text = String::from_utf8_lossy(&self.utf8[..valid]).into_owned();
                match err.error_len() {
                    Some(len) => {
                        text.push('\u{fffd}');
                        self.utf8.drain(..valid + len);
                    }
                    None => {
                        self.utf8.drain(..valid);
                    }
                }
                text
            }
        };
        let stripped = self.ansi.strip(&text);
        normalize_newlines(&stripped)
    }
}

// ---------------------------------------------------------------------------
// LLDB
// ---------------------------------------------------------------------------

static LLDB_THREAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?<current>\*)?\s*thread #(?<thread>\d+)(?:, queue = '(?<queue>[^']*)')?(?:, name = (?<name>[^,]+))?(?:, stop reason = (?<reason>.*))?(?: id=0x[0-9a-fA-F]+)?$",
    )
    .expect("lldb thread regex")
});

static LLDB_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?<current>\*)?\s*frame #(?<index>\d+): (?<pc>0x[0-9a-fA-F]+) (?<module>[^`]*)`(?<function>.*?)(?: at (?<file>.+):(?<line>\d+)(?::(?<column>\d+))?)?$",
    )
    .expect("lldb frame regex")
});

/// True when a line is an LLDB `frame #N:` line.
pub(crate) fn is_lldb_frame_line(line: &str) -> bool {
    LLDB_FRAME.is_match(line).unwrap_or(false)
}

/// True when a line is a pdb current-frame marker (`> file(line)func()`).
pub(crate) fn is_pdb_stop_line(line: &str) -> bool {
    PDB_STOP.is_match(line).unwrap_or(false)
}

/// True when a line reports a node inspect stop (`break in file:line`).
pub(crate) fn is_node_stop_line(line: &str) -> bool {
    NODE_STOP
        .is_match(strip_node_prompt(line.trim_end()))
        .unwrap_or(false)
}

/// Parse `bt all` output.
pub(crate) fn parse_lldb_stack(text: &str, source_map: &crate::source_map::SourceMap, cwd: &Path) -> Stack {
    let mut stack = Stack::default();
    let mut current_thread: Option<u32> = None;
    let mut saw_current_thread = false;
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(caps) = LLDB_THREAD.captures(line).ok().flatten() {
            let Some(thread) = caps.name("thread") else {
                continue;
            };
            let Ok(thread_id) = thread.as_str().parse::<u32>() else {
                continue;
            };
            let selected = caps.name("current").is_some();
            let name = caps
                .name("queue")
                .map(|m| m.as_str().to_string())
                .or_else(|| caps.name("name").map(|m| m.as_str().to_string()))
                .unwrap_or_default();
            current_thread = Some(thread_id);
            saw_current_thread |= selected;
            if let Some(existing) = stack.threads.iter_mut().find(|t| t.id == thread_id) {
                existing.selected |= selected;
                if existing.name.is_empty() {
                    existing.name = name;
                }
            } else {
                stack.threads.push(Thread {
                    id: thread_id,
                    name,
                    selected,
                });
            }
            continue;
        }
        if let Some(caps) = LLDB_FRAME.captures(line).ok().flatten() {
            let index: u32 = caps
                .name("index")
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            let function = caps
                .name("function")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let function = function
                .split_once('(')
                .map(|(name, _)| name.to_string())
                .unwrap_or(function);
            let file = caps.name("file").map(|m| {
                let reported = PathBuf::from(m.as_str());
                source_map.resolve_in(&reported, cwd)
            });
            let line = caps
                .name("line")
                .and_then(|m| m.as_str().parse::<u32>().ok());
            let thread = current_thread.unwrap_or(1);
            // A segment can contain a stop notification followed by `bt all`
            // output; both describe frame #0.  Keep one copy.
            if let Some(existing) = stack
                .frames
                .iter_mut()
                .find(|frame| frame.thread == thread && frame.index == index)
            {
                existing.current |= caps.name("current").is_some();
                if existing.file.is_none() {
                    existing.file = file;
                    existing.line = line;
                }
                if existing.name.is_empty() {
                    existing.name = function;
                }
                continue;
            }
            stack.frames.push(Frame {
                thread,
                index,
                name: function,
                file,
                line,
                current: caps.name("current").is_some(),
            });
        }
    }
    if !saw_current_thread
        && let Some(first) = stack.threads.first_mut() {
            first.selected = true;
        }
    if !stack.frames.iter().any(|f| f.current) {
        let selected_thread = stack
            .threads
            .iter()
            .find(|t| t.selected)
            .map(|t| t.id)
            .unwrap_or_else(|| stack.frames.first().map(|f| f.thread).unwrap_or(1));
        if let Some(frame) = stack
            .frames
            .iter_mut()
            .find(|f| f.thread == selected_thread && f.index == 0)
        {
            frame.current = true;
        }
    }
    stack
}

/// Parse a stop segment: reason and location.
pub(crate) fn parse_lldb_stop(
    text: &str,
    patterns: &CompiledPatterns,
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Option<(Location, StopReason)> {
    let stack = parse_lldb_stack(text, source_map, cwd);
    let frame = stack.current()?.clone();
    let location = Location {
        frame: frame.id(),
        file: frame.file.clone(),
        line: frame.line,
        function: Some(frame.name.clone()),
    };
    let reason_text = if let Some(re) = &patterns.stop_reason {
        re.captures(text)
            .ok()
            .flatten()
            .and_then(|c| c.name("reason").map(|m| m.as_str().to_string()))
    } else {
        None
    };
    let reason = match reason_text.as_deref() {
        Some(text) => parse_lldb_reason(text),
        None => StopReason::Unknown("stop".into()),
    };
    Some((location, reason))
}

fn parse_lldb_reason(reason: &str) -> StopReason {
    let reason = reason.trim();
    if let Some(rest) = reason.strip_prefix("breakpoint") {
        let id = rest
            .trim()
            .split('.')
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        return StopReason::Breakpoint {
            temporary: false,
            id,
        };
    }
    if reason.starts_with("step over") {
        return StopReason::StepOver;
    }
    if reason.starts_with("step in") || reason.starts_with("step into") {
        return StopReason::StepIn;
    }
    if reason.starts_with("step out") {
        return StopReason::StepOut;
    }
    if let Some(signal) = reason.strip_prefix("signal ") {
        let name = signal.split_whitespace().next().unwrap_or(signal).to_string();
        if name == "SIGINT" || name == "SIGSTOP" {
            return StopReason::Pause;
        }
        return StopReason::Signal { name };
    }
    if reason.starts_with("EXC_") || reason.contains("EXC_BAD") {
        return StopReason::Signal {
            name: reason.to_string(),
        };
    }
    StopReason::Unknown(reason.to_string())
}

/// Parse the "Breakpoint 1: ..." acknowledgement.
pub(crate) fn parse_breakpoint_set(
    text: &str,
    patterns: &CompiledPatterns,
) -> Option<String> {
    let re = patterns.breakpoint_set.as_ref()?;
    let caps = re.captures(text).ok().flatten()?;
    caps.name("id").map(|m| m.as_str().to_string())
}

/// Parse `frame variable -s ... -T` output.
pub(crate) fn parse_lldb_variables(text: &str) -> Vec<Variable> {
    let mut roots: Vec<Variable> = Vec::new();
    // Open aggregates with their indentation.
    let mut stack: Vec<(usize, Variable)> = Vec::new();

    let attach = |node: Variable, stack: &mut Vec<(usize, Variable)>, roots: &mut Vec<Variable>| {
        if let Some((_, parent)) = stack.last_mut() {
            parent.children.push(node);
        } else {
            roots.push(node);
        }
    };

    for raw in text.lines() {
        let raw = raw.trim_end();
        if raw.trim().is_empty() {
            continue;
        }
        if raw.trim_start().starts_with("error:") {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if line == "}" {
            if let Some((_, node)) = stack.pop() {
                attach(node, &mut stack, &mut roots);
            }
            continue;
        }
        // Close deeper aggregates that this line cannot belong to.
        while let Some((open_indent, _)) = stack.last() {
            if *open_indent >= indent {
                if let Some((_, node)) = stack.pop() {
                    attach(node, &mut stack, &mut roots);
                }
            } else {
                break;
            }
        }

        let ends_open = line.ends_with('{');
        let body = if ends_open {
            line[..line.len() - 1].trim_end()
        } else {
            line
        };
        let (kind, body) = split_scope(body);
        let Some((type_name, name, value)) = parse_typed_line(body) else {
            continue;
        };
        let is_elided = ends_open || value.as_deref().is_some_and(|v| v.ends_with("{...}"));
        let cleaned_value = value
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim_end_matches(" {...}").to_string());
        let variable = Variable {
            id: crate::variables::VariableId::root(0),
            display_name: name.clone(),
            name,
            type_name: Some(type_name),
            kind,
            value: cleaned_value,
            description: None,
            raw_value: None,
            expr: None,
            children: Vec::new(),
            expandable: is_elided,
            truncated: false,
        };
        if ends_open {
            stack.push((indent, variable));
        } else {
            attach(variable, &mut stack, &mut roots);
        }
    }
    while let Some((_, node)) = stack.pop() {
        attach(node, &mut stack, &mut roots);
    }
    // Build debugger expressions for every node.
    for (index, root) in roots.iter_mut().enumerate() {
        set_exprs(root, &root.name.clone(), index);
    }
    roots
}

fn set_exprs(node: &mut Variable, expr: &str, _index: usize) {
    node.expr = Some(expr.to_string());
    let parent_type = node.type_name.clone().unwrap_or_default();
    for child in &mut node.children {
        let child_expr = if child.name.starts_with('[') {
            format!("{expr}{}", child.name)
        } else if child.name.starts_with('*') {
            let name = child.name.trim_start_matches('*');
            format!("{expr}->{name}")
        } else if parent_type.ends_with('*') {
            format!("{expr}->{}", child.name)
        } else {
            format!("{expr}.{}", child.name)
        };
        set_exprs(child, &child_expr, 0);
    }
}

fn split_scope(line: &str) -> (VariableKind, &str) {
    for (prefix, kind) in [
        ("ARG:", VariableKind::Parameter),
        ("LOCAL:", VariableKind::Variable),
        ("STATIC:", VariableKind::Static),
        ("GLOBAL:", VariableKind::Global),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return (kind, rest.trim_start());
        }
    }
    (VariableKind::Variable, line)
}

/// Parse `(type) name = value` (value optional).
fn parse_typed_line(line: &str) -> Option<(String, String, Option<String>)> {
    let close = line.rfind(") ")?;
    if !line.starts_with('(') || close < 2 {
        return None;
    }
    let type_name = &line[1..close];
    let rest = line[close + 2..].trim_start();
    let (name, value) = match rest.split_once(" = ") {
        Some((name, value)) => (name.trim(), Some(value.to_string())),
        None => match rest.strip_suffix(" =") {
            Some(name) => (name.trim(), None),
            None => (rest.trim(), None),
        },
    };
    if name.is_empty() {
        return None;
    }
    Some((type_name.to_string(), name.to_string(), value))
}

// ---------------------------------------------------------------------------
// pdb
// ---------------------------------------------------------------------------

static PDB_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?<current>> |  )(?<file>.+)\((?<line>\d+)\)(?<function>[^(]*)\(\)(?:->(?<ret>.*))?$",
    )
    .expect("pdb frame regex")
});

static PDB_STOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^> (?<file>.+)\((?<line>\d+)\)(?<function>[^(]*)\(\)(?:->(?<ret>.*))?$",
    )
    .expect("pdb stop regex")
});

/// Parse `where` output.  Frames are reversed so index 0 is the innermost
/// frame (the current one).  Hidden frames keep their raw index so relative
/// `up`/`down` selection stays correct.
pub(crate) fn parse_pdb_stack(
    text: &str,
    hide_frames: &[String],
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Stack {
    let mut collected: Vec<(bool, String, u32, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let Some(caps) = PDB_FRAME.captures(line).ok().flatten() else {
            continue;
        };
        let current = caps.name("current").map(|m| m.as_str()) == Some("> ");
        let Some(file) = caps.name("file") else {
            continue;
        };
        let file = file.as_str().to_string();
        let line_no = caps
            .name("line")
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let function = caps
            .name("function")
            .map(|m| m.as_str().trim().to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "<module>".to_string());
        collected.push((current, file, line_no, function));
    }
    collected.reverse();
    let mut frames = Vec::new();
    let mut current_index = None;
    for (raw_index, (current, file, line, function)) in collected.into_iter().enumerate() {
        if current {
            current_index = Some(raw_index);
        }
        if hidden(&file, hide_frames) {
            continue;
        }
        let resolved = source_map.resolve_in(Path::new(&file), cwd);
        frames.push(Frame {
            thread: 1,
            index: raw_index as u32,
            name: function,
            file: Some(resolved),
            line: Some(line),
            current,
        });
    }
    if let Some(index) = current_index {
        for frame in &mut frames {
            frame.current = frame.index as usize == index;
        }
    } else if let Some(frame) = frames.first_mut() {
        frame.current = true;
    }
    Stack {
        threads: vec![Thread {
            id: 1,
            name: "Main Thread".into(),
            selected: true,
        }],
        frames,
    }
}

fn hidden(file: &str, hide_frames: &[String]) -> bool {
    hide_frames.iter().any(|needle| file.contains(needle))
}

/// Extract the innermost stop location from a pdb segment.
pub(crate) fn parse_pdb_stop(
    text: &str,
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Option<Location> {
    let mut location = None;
    for line in text.lines() {
        let line = line.trim_end();
        let Some(caps) = PDB_STOP.captures(line).ok().flatten() else {
            continue;
        };
        let file = caps.name("file")?.as_str();
        let line_no = caps.name("line")?.as_str().parse::<u32>().ok();
        let function = caps
            .name("function")
            .map(|m| m.as_str().trim().to_string())
            .filter(|f| !f.is_empty());
        location = Some(Location {
            frame: FrameId::main(),
            file: Some(source_map.resolve_in(Path::new(file), cwd)),
            line: line_no,
            function,
        });
    }
    location
}

/// Parse the exception name and message from a pdb post-mortem transcript.
pub(crate) fn parse_python_exception(text: &str) -> (String, Option<String>) {
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('>') || line.starts_with("->") {
            continue;
        }
        if let Some((name, message)) = line.split_once(": ") {
            let name = name.trim();
            if name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && !name.contains(' ')
            {
                return (name.to_string(), Some(message.trim().to_string()));
            }
        }
        if line.ends_with("Error") || line.ends_with("Exception") {
            return (line.to_string(), None);
        }
    }
    ("Exception".into(), None)
}

// ---------------------------------------------------------------------------
// node inspect
// ---------------------------------------------------------------------------

static NODE_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^#(?<index>\d+) (?<function>.*?) (?<file>.+):(?<line>\d+):(?<col>\d+)$",
    )
    .expect("node frame regex")
});

static NODE_STOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?<kind>break|step|pause) in (?<file>[^:\r\n]+):(?<line>\d+)$")
        .expect("node stop regex")
});

/// node's REPL can leave its prompt glued to the first line of output after
/// ANSI cursor moves are stripped ("debug> break in app.js:5").
fn strip_node_prompt(mut line: &str) -> &str {
    while let Some(rest) = line.strip_prefix("debug> ") {
        line = rest.trim_start();
    }
    line
}

pub(crate) fn parse_node_stack(
    text: &str,
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Stack {
    let mut frames = Vec::new();
    for line in text.lines() {
        let line = strip_node_prompt(line.trim_end());
        let Some(caps) = NODE_FRAME.captures(line).ok().flatten() else {
            continue;
        };
        let index = caps.name("index").and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let name = caps.name("function").map(|m| m.as_str().to_string()).unwrap_or_default();
        let file = caps.name("file").map(|m| m.as_str()).unwrap_or("");
        let line_no = caps.name("line").and_then(|m| m.as_str().parse().ok());
        frames.push(Frame {
            thread: 1,
            index,
            name,
            file: Some(source_map.resolve_in(Path::new(file), cwd)),
            line: line_no,
            current: index == 0,
        });
    }
    Stack {
        threads: vec![Thread {
            id: 1,
            name: "Main Thread".into(),
            selected: true,
        }],
        frames,
    }
}

/// Extract a node stop location and its kind.
pub(crate) fn parse_node_stop(
    text: &str,
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Option<(Location, bool)> {
    let mut result = None;
    for line in text.lines() {
        let line = strip_node_prompt(line.trim_end());
        let Some(caps) = NODE_STOP.captures(line).ok().flatten() else {
            continue;
        };
        let file = caps.name("file")?.as_str();
        let line_no = caps.name("line")?.as_str().parse::<u32>().ok();
        let is_break = caps.name("kind").map(|m| m.as_str()) == Some("break");
        result = Some((
            Location {
                frame: FrameId::main(),
                file: Some(source_map.resolve_in(Path::new(file), cwd)),
                line: line_no,
                function: None,
            },
            is_break,
        ));
    }
    result
}

// ---------------------------------------------------------------------------
// Generic parser
// ---------------------------------------------------------------------------

/// Parse a stack with the adapter's named-group regexes.
pub(crate) fn parse_generic_stack(
    text: &str,
    patterns: &CompiledPatterns,
    source_map: &crate::source_map::SourceMap,
    cwd: &Path,
) -> Stack {
    let mut frames = Vec::new();
    let mut threads: Vec<Thread> = Vec::new();
    if let Some(re) = &patterns.threads {
        for caps in re.captures_iter(text).flatten() {
            let id = caps
                .name("thread")
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(1);
            let name = caps.name("name").map(|m| m.as_str().to_string()).unwrap_or_default();
            let selected = caps
                .name("current")
                .is_some_and(|m| !m.as_str().trim().is_empty());
            threads.push(Thread {
                id,
                name,
                selected,
            });
        }
    }
    if let Some(re) = &patterns.frames {
        for caps in re.captures_iter(text).flatten() {
            let index = caps
                .name("index")
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(frames.len() as u32);
            let name = caps
                .name("name")
                .or_else(|| caps.name("function"))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let file = caps.name("file").map(|m| {
                source_map.resolve_in(Path::new(m.as_str()), cwd)
            });
            let line = caps
                .name("line")
                .and_then(|m| m.as_str().parse::<u32>().ok());
            let thread = caps
                .name("thread")
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(1);
            let current = caps
                .name("current")
                .is_some_and(|m| !m.as_str().trim().is_empty());
            frames.push(Frame {
                thread,
                index,
                name,
                file,
                line,
                current,
            });
        }
    }
    if patterns.reverse_frames {
        // Debuggers such as bashdb and perl print the innermost frame last.
        frames.reverse();
    }
    let mut stack = Stack { threads, frames };
    if stack.threads.is_empty() {
        stack.threads.push(Thread {
            id: 1,
            name: "Main Thread".into(),
            selected: true,
        });
    }
    if !stack.frames.iter().any(|f| f.current)
        && let Some(frame) = stack.frames.first_mut() {
            frame.current = true;
        }
    stack
}

/// Generic variable parser (flat lines with named groups).
pub(crate) fn parse_generic_variables(
    text: &str,
    patterns: &CompiledPatterns,
) -> Vec<Variable> {
    let Some(re) = &patterns.variables else {
        return Vec::new();
    };
    let mut variables = Vec::new();
    for caps in re.captures_iter(text).flatten() {
        let Some(name) = caps.name("name") else {
            continue;
        };
        let name = name.as_str().to_string();
        let type_name = caps.name("type").map(|m| m.as_str().to_string());
        let value = caps.name("value").map(|m| m.as_str().to_string());
        let kind = caps
            .name("kind")
            .map(|m| VariableKind::parse(m.as_str()))
            .unwrap_or(VariableKind::Variable);
        variables.push(Variable {
            id: crate::variables::VariableId::root(0),
            display_name: name.clone(),
            name: name.clone(),
            type_name,
            kind,
            value,
            description: None,
            raw_value: None,
            expr: Some(name),
            children: Vec::new(),
            expandable: false,
            truncated: false,
        });
    }
    variables
}

// ---------------------------------------------------------------------------
// JSON variables helper output
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct VarPayload {
    #[serde(default)]
    variables: Vec<VarNode>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct VarNode {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(rename = "type", default)]
    type_name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    raw_value: Option<String>,
    #[serde(default)]
    expr: Option<String>,
    #[serde(default)]
    children: Vec<VarNode>,
    #[serde(default)]
    expandable: bool,
    #[serde(default)]
    truncated: bool,
}

/// Extract and parse the JSON variables payload after `marker`.
pub(crate) fn parse_json_variables(
    text: &str,
    marker: &str,
) -> Result<Vec<Variable>, DebugError> {
    let mut payload_text = None;
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(marker) {
            payload_text = Some(rest.trim().to_string());
        }
    }
    let Some(payload_text) = payload_text else {
        return Err(DebugError::Message(
            "debugger produced no variables payload".into(),
        ));
    };
    let payload: VarPayload = serde_json::from_str(&payload_text)
        .map_err(|e| DebugError::Message(format!("variables payload: {e}")))?;
    if let Some(error) = payload.error {
        return Err(DebugError::Message(error));
    }
    Ok(payload.variables.into_iter().map(convert_var_node).collect())
}

fn convert_var_node(node: VarNode) -> Variable {
    let name = node.name;
    Variable {
        id: crate::variables::VariableId::root(0),
        display_name: node.display_name.unwrap_or_else(|| name.clone()),
        name,
        type_name: node.type_name,
        kind: node
            .kind
            .map(|kind| VariableKind::parse(&kind))
            .unwrap_or(VariableKind::Variable),
        value: node.value,
        description: node.description,
        raw_value: node.raw_value,
        expr: node.expr,
        children: node.children.into_iter().map(convert_var_node).collect(),
        expandable: node.expandable,
        truncated: node.truncated,
    }
}

// ---------------------------------------------------------------------------
// Expression results
// ---------------------------------------------------------------------------

/// Clean the text of an expression evaluation down to its value.
pub(crate) fn extract_expression_value(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if line.starts_with("error:")
        || line.starts_with("***")
        || line.starts_with("Traceback")
    {
        return line;
    }
    if let Some((_, value)) = line.rsplit_once(" = ") {
        return value.trim().to_string();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map::SourceMap;

    #[test]
    fn ansi_stripping_handles_code_runner_and_node_sequences() {
        assert_eq!(
            strip_ansi("\u{1b}[1;34mbreak\u{1b}[0m t.py:10"),
            "break t.py:10"
        );
        assert_eq!(
            strip_ansi("\u{1b}[1G\u{1b}[0Jdebug> \u{1b}[8G"),
            "debug> "
        );
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{7}b"), "ab");
    }

    #[test]
    fn stripper_handles_split_sequences() {
        let mut stripper = AnsiStripper::new();
        assert_eq!(stripper.strip("x\u{1b}"), "x");
        assert_eq!(stripper.strip("[32mred\u{1b}"), "red");
        assert_eq!(stripper.strip("[0m!"), "!");
    }

    #[test]
    fn decoder_keeps_partial_utf8() {
        let mut decoder = TranscriptDecoder::new();
        let text = "héllo\n";
        let bytes = text.as_bytes();
        let first = decoder.push(&bytes[..2]);
        assert_eq!(first, "h");
        let second = decoder.push(&bytes[2..]);
        assert_eq!(second, "éllo\n");
    }

    #[test]
    fn newline_normalisation() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nbc\nd");
    }

    #[test]
    fn lldb_stack_parses_threads_and_frames() {
        let text = "* thread #1, queue = 'com.apple.main-thread', stop reason = breakpoint 1.1\n   * frame #0: 0x000000010000052c t2`main at /work/t2.c:16\n     frame #1: 0x0000000181360d7c dyld`start + 6652\n";
        let stack = parse_lldb_stack(text, &SourceMap::new(), Path::new("/work"));
        assert_eq!(stack.threads.len(), 1);
        assert!(stack.threads[0].selected);
        assert_eq!(stack.frames.len(), 2);
        assert_eq!(stack.frames[0].name, "main");
        assert_eq!(stack.frames[0].line, Some(16));
        assert!(stack.frames[0].current);
        assert_eq!(stack.frames[1].file, None);
    }

    #[test]
    fn lldb_frame_with_arguments_strips_them() {
        let text = "   * frame #0: 0x00000001000004bc t2`add(a=0, b=0) at /work/t2.c:7\n";
        let stack = parse_lldb_stack(text, &SourceMap::new(), Path::new("/work"));
        assert_eq!(stack.frames[0].name, "add");
    }

    #[test]
    fn lldb_stop_reason_mapping() {
        assert!(matches!(
            parse_lldb_reason("breakpoint 1.1"),
            StopReason::Breakpoint { id: Some(1), .. }
        ));
        assert_eq!(parse_lldb_reason("step over"), StopReason::StepOver);
        assert_eq!(parse_lldb_reason("step in"), StopReason::StepIn);
        assert_eq!(parse_lldb_reason("step out"), StopReason::StepOut);
        assert_eq!(
            parse_lldb_reason("signal SIGSTOP"),
            StopReason::Pause
        );
        assert!(matches!(
            parse_lldb_reason("signal SIGSEGV"),
            StopReason::Signal { .. }
        ));
    }

    #[test]
    fn lldb_variables_parse_nesting_and_scope() {
        let text = "ARG: (int) a = 0\nLOCAL: (Point) p = {\n  (int) x = 1\n  (int) y = 2\n}\nLOCAL: (int *) vals = 0x123 {...}\n";
        let vars = parse_lldb_variables(text);
        assert_eq!(vars.len(), 3);
        assert_eq!(vars[0].kind, VariableKind::Parameter);
        assert_eq!(vars[1].children.len(), 2);
        assert_eq!(vars[1].children[1].expr.as_deref(), Some("p.y"));
        assert!(vars[2].expandable);
        assert_eq!(vars[2].expr.as_deref(), Some("vals"));
    }

    #[test]
    fn pdb_stack_is_reversed_and_hides_internal_frames() {
        let text = "  /usr/lib/python3.15/bdb.py(912)run()\n  /work/t.py(16)<module>()\n> /work/t.py(10)main()\n";
        let stack = parse_pdb_stack(
            text,
            &["bdb.py".into()],
            &SourceMap::new(),
            Path::new("/work"),
        );
        assert_eq!(stack.frames.len(), 2);
        assert_eq!(stack.frames[0].name, "main");
        assert_eq!(stack.frames[0].index, 0);
        assert!(stack.frames[0].current);
        assert_eq!(stack.frames[1].name, "<module>");
    }

    #[test]
    fn pdb_stop_location_is_the_last_marker() {
        let text = "> /work/a.py(3)add()\n-> return a + b\n";
        let location = parse_pdb_stop(text, &SourceMap::new(), Path::new("/work")).unwrap();
        assert_eq!(location.line, Some(3));
        assert_eq!(location.function.as_deref(), Some("add"));
    }

    #[test]
    fn python_exception_extraction() {
        let text = "Traceback (most recent call last):\n  File \"x.py\", line 2\n    return 10 // n\nZeroDivisionError: division by zero\nUncaught exception. Entering post mortem debugging\n";
        let (name, message) = parse_python_exception(text);
        assert_eq!(name, "ZeroDivisionError");
        assert_eq!(message.as_deref(), Some("division by zero"));
    }

    #[test]
    fn node_stack_and_stop() {
        let text = "#0 (anonymous) t.js:3:13\n#1 wrap t.js:9:2\n";
        let stack = parse_node_stack(text, &SourceMap::new(), Path::new("/work"));
        assert_eq!(stack.frames.len(), 2);
        assert_eq!(stack.frames[0].file.as_deref(), Some(Path::new("/work/t.js")));
        let (location, is_break) = parse_node_stop(
            "break in t.js:3\n",
            &SourceMap::new(),
            Path::new("/work"),
        )
        .unwrap();
        assert!(is_break);
        assert_eq!(location.line, Some(3));
    }

    #[test]
    fn generic_parser_uses_named_groups() {
        let adapter = DebuggerAdapter::from_toml(
            r#"
id = "x"
name = "X"
prompt = "^> $"
[start]
program = "sh"
[commands]
backtrace = "bt"
[parsers]
frames = "(?m)^#(?<index>\\d+) (?<name>\\S+) (?<file>\\S+):(?<line>\\d+)$"
"#,
        )
        .unwrap();
        let patterns = CompiledPatterns::new(&adapter).unwrap();
        let stack = parse_generic_stack(
            "#0 main a.c:3\n#1 start a.c:9\n",
            &patterns,
            &SourceMap::new(),
            Path::new("/work"),
        );
        assert_eq!(stack.frames.len(), 2);
        assert_eq!(stack.frames[1].name, "start");
        assert!(stack.frames[0].current);
    }

    #[test]
    fn json_variables_round_trip() {
        let text = "SCRATCHDBG:{\"variables\":[{\"name\":\"n\",\"value\":\"3\",\"type\":\"int\",\"kind\":\"variable\",\"expr\":\"n\"},{\"name\":\"p\",\"type\":\"dict\",\"children\":[{\"name\":\"'x'\",\"value\":\"1\",\"expr\":\"p['x']\"}],\"expandable\":false}]}\n";
        let vars = parse_json_variables(text, "SCRATCHDBG:").unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[1].children.len(), 1);
        assert_eq!(vars[1].children[0].expr.as_deref(), Some("p['x']"));
    }

    #[test]
    fn fixture_lldb_stop_transcript() {
        let text = include_str!("../tests/fixtures/lldb_stop.txt");
        let adapter = crate::adapter::AdapterRegistry::builtin()
            .get("lldb")
            .unwrap()
            .clone();
        let patterns = CompiledPatterns::new(&adapter).unwrap();
        let (location, reason) =
            parse_lldb_stop(text, &patterns, &SourceMap::new(), Path::new("/tmp")).unwrap();
        assert_eq!(location.line, Some(16));
        assert_eq!(location.function.as_deref(), Some("main"));
        assert!(matches!(reason, StopReason::Breakpoint { id: Some(1), .. }));
    }

    #[test]
    fn fixture_lldb_bt_transcript() {
        let text = include_str!("../tests/fixtures/lldb_bt.txt");
        let stack = parse_lldb_stack(text, &SourceMap::new(), Path::new("/tmp"));
        assert_eq!(stack.frames.len(), 2);
        assert!(stack.frames[0].current);
        assert_eq!(stack.frames[0].name, "main");
        assert_eq!(stack.frames[1].name, "start + 6652");
    }

    #[test]
    fn fixture_lldb_variables_transcript() {
        let text = include_str!("../tests/fixtures/lldb_variables.txt");
        let vars = parse_lldb_variables(text);
        assert_eq!(vars.len(), 4);
        assert_eq!(vars[1].children.len(), 2);
        assert_eq!(vars[1].children[0].expr.as_deref(), Some("p.x"));
        assert_eq!(vars[2].value.as_deref(), Some("4"));
    }

    #[test]
    fn fixture_lldb_nested_variables_transcript() {
        let text = include_str!("../tests/fixtures/lldb_nested_variables.txt");
        let vars = parse_lldb_variables(text);
        assert_eq!(vars.len(), 1);
        let item = &vars[0];
        assert_eq!(item.name, "it");
        assert_eq!(item.children.len(), 4);
        let pos = item.children.iter().find(|v| v.name == "pos").unwrap();
        assert_eq!(pos.expr.as_deref(), Some("it.pos"));
        assert_eq!(pos.children.len(), 2);
    }

    #[test]
    fn fixture_pdb_where_transcript() {
        let text = include_str!("../tests/fixtures/pdb_where.txt");
        let adapter = crate::adapter::AdapterRegistry::builtin()
            .get("pdb")
            .unwrap()
            .clone();
        let patterns = CompiledPatterns::new(&adapter).unwrap();
        let stack = parse_pdb_stack(
            text,
            &patterns.hide_frames,
            &SourceMap::new(),
            Path::new("/tmp"),
        );
        assert_eq!(stack.frames.len(), 2);
        assert_eq!(stack.frames[0].name, "main");
        assert!(stack.frames[0].current);
        assert_eq!(stack.frames[1].name, "<module>");
    }

    #[test]
    fn fixture_node_bt_transcript() {
        let text = include_str!("../tests/fixtures/node_bt.txt");
        let stack = parse_node_stack(text, &SourceMap::new(), Path::new("/tmp"));
        assert_eq!(stack.frames.len(), 3);
        assert_eq!(stack.frames[0].name, "(anonymous)");
        assert_eq!(stack.frames[0].line, Some(3));
    }

    #[test]
    fn fixture_pdb_variables_transcript() {
        let text = include_str!("../tests/fixtures/pdb_variables.txt");
        let vars = parse_json_variables(text, "SCRATCHDBG:").unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].value.as_deref(), Some("3"));
        assert_eq!(vars[1].children.len(), 1);
        assert_eq!(vars[1].children[0].expr.as_deref(), Some("p['x']"));
    }

    #[test]
    fn expression_values_are_cleaned() {
        assert_eq!(extract_expression_value("(int) $0 = 4\n"), "4");
        assert_eq!(extract_expression_value("3\n"), "3");
        assert_eq!(
            extract_expression_value("error: use of undeclared identifier 'x'\n"),
            "error: use of undeclared identifier 'x'"
        );
    }
}
