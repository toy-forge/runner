//! The debugger (DESIGN 8.9). `scratch-debugger` drives a real CLI debugger
//! (LLDB, pdb, bashdb, node, …) through an adapter; this module starts
//! sessions, drains their events into console output and reactive view
//! state, and renders the stack/variables column that sits to the left of
//! the console text while a session is live.
//!
//! The execution marker — the line the program is stopped at — is drawn by
//! `theme::decorations` as a whole-line tint plus a gutter mark; it refreshes
//! through the document's `text_rev` bump in `handle_event`.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use nsh_base::{dialog, input};
use scratch_console_model::Stream as ConsoleStream;
use scratch_debugger::{
    AdapterRegistry, Breakpoint, DebugEvent, DebugSession, DebuggerAdapter, DebuggerState,
    Frame, FrameId, SessionCommand, SessionConfig, SourceMap, Variable, VariableId,
    VariablesQuery, variables::WatchValue,
};
use scratch_runner::{RunEvent, RunMode, Stream};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;
use tokio::sync::mpsc;

use scratch_document::DocumentId;

use super::state::{self, editor_api, with};

thread_local! {
    static SESSION: RefCell<Option<DebugSession>> = const { RefCell::new(None) };
    /// The source file path the live session was started on — `DebugSession`
    /// keys breakpoints against source files even when debugging compiled binaries.
    static SOURCE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Temp directory created for compiled `RunMode::Debug` runs (`keep_temp = true`),
    /// cleaned up when the debug session stops.
    static DEBUG_TEMP_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Temp script file created for interpreted debug runs, removed when the session ends.
    static DEBUG_TEMP_FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[derive(Clone, PartialEq, Default)]
struct BreakpointSheet {
    open: bool,
    doc: Option<DocumentId>,
    line: u32,
    enabled: bool,
    temporary: bool,
    condition: String,
}

fn bp_sheet() -> Signal<BreakpointSheet> {
    global::<BreakpointSheet>()
}

/// Reactive debugger state for the pane (DESIGN 8.9).
#[derive(Clone, Default)]
pub struct DebugView {
    /// Document the session is debugging.
    pub doc: Option<DocumentId>,
    /// Active debugger adapter id (`gdb`, `lldb`, `pdb`, `xdebug`, …).
    pub adapter_id: String,
    /// Session lifecycle (None = no session).
    pub state: Option<DebuggerState>,
    /// Frames of the last stop.
    pub stack: Vec<Frame>,
    /// Variables of the selected frame (roots; expanded children nest inside).
    pub variables: Vec<Variable>,
    /// Variable nodes the user expanded (stable `VariableId` paths).
    pub expanded: BTreeSet<VariableId>,
    /// Flattened rows for the list: `(variable, depth)` in display order.
    pub flat: Vec<(Variable, usize)>,
    /// Frame selected in the stack pane.
    pub selected: Option<FrameId>,
    /// Stack + variables column visible.
    pub pane: bool,
    /// "Stopped at Breakpoint" style annotation.
    pub annotation: String,
    /// Line the program is stopped at (one-based).
    pub line: Option<u32>,
    /// Watch expressions and their last evaluated values.
    pub watches: Vec<WatchValue>,
    /// Variable currently being value-edited inline (double-click).
    pub editing: Option<VariableId>,
    /// Last non-fatal adapter error (shown inline until the next refresh).
    pub edit_error: Option<String>,
}

fn view_state() -> Signal<DebugView> {
    global::<DebugView>()
}

/// Whether a live session exists for `id`.
pub fn debugging(id: DocumentId) -> bool {
    view_state().with(|view| view.doc == Some(id) && view.state.is_some())
}

/// The stopped line for `id` (one-based), used by `theme::decorations`.
pub fn stopped_line(id: DocumentId) -> Option<u32> {
    view_state().with(|view| match (&view.state, view.doc) {
        (Some(DebuggerState::Paused { .. }), Some(doc)) if doc == id => view.line,
        _ => None,
    })
}

fn send(command: SessionCommand) {
    SESSION.with(|session| {
        if let Some(session) = session.borrow().as_ref() {
            let _ = session.send(command);
        }
    });
}

/// The document's debugger breakpoints (`Document.breakpoints` is 0-based,
/// the debugger's 1-based), keyed to `SOURCE_PATH`.
fn breakpoints_of(id: DocumentId) -> Vec<Breakpoint> {
    let source = SOURCE_PATH.with(|p| p.borrow().clone());
    let Some(source) = source else { return Vec::new() };
    with(|workspace| {
        workspace
            .doc(id)
            .map(|doc| {
                doc.document
                    .breakpoints
                    .as_slice()
                    .iter()
                    .map(|breakpoint| {
                        let mut installed = Breakpoint::new(source.clone(), breakpoint.line + 1);
                        installed.enabled = breakpoint.enabled;
                        installed.temporary = breakpoint.temporary;
                        if let Some(condition) = &breakpoint.condition {
                            installed = installed.with_condition(condition.clone());
                        }
                        installed
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Opens the Edit Breakpoint dialog for `line` (0-based) on `id`.
pub(super) fn edit_breakpoint_dialog(id: DocumentId, line: u32) {
    let bp = with(|workspace| {
        workspace.doc(id).and_then(|d| {
            d.document
                .breakpoints
                .as_slice()
                .iter()
                .find(|b| b.line == line)
                .cloned()
        })
    });
    let Some(bp) = bp else {
        state::toggle_breakpoint(id, line);
        return;
    };
    bp_sheet().set(BreakpointSheet {
        open: true,
        doc: Some(id),
        line,
        enabled: bp.enabled,
        temporary: bp.temporary,
        condition: bp.condition.unwrap_or_default(),
    });
}

/// Context menu items for breakpoints on an editor line or breakpoint list row.
pub(super) fn gutter_context_menu(id: DocumentId, line: u32) -> Vec<MenuItem> {
    let existing = with(|workspace| {
        workspace.doc(id).and_then(|doc| {
            doc.document
                .breakpoints
                .as_slice()
                .iter()
                .find(|b| b.line == line)
                .cloned()
        })
    });
    match existing {
        Some(bp) => vec![
            MenuItem::new("Edit Breakpoint…").on_select(move || edit_breakpoint_dialog(id, line)),
            MenuItem::new(if bp.enabled { "Disable Breakpoint" } else { "Enable Breakpoint" })
                .on_select(move || state::set_breakpoint_enabled(id, line, !bp.enabled)),
            MenuItem::new("Remove Breakpoint").on_select(move || state::toggle_breakpoint(id, line)),
        ],
        None => vec![
            MenuItem::new("Add Breakpoint").on_select(move || state::toggle_breakpoint(id, line)),
        ],
    }
}

/// Returns `(expression, value)` for the identifier under `pos` while `id`
/// is paused in the debugger, for editor hover inspection.
pub(super) fn hover_value(id: DocumentId, pos: nsh_base::EditorPosition) -> Option<(String, String)> {
    if !super::preferences::settings().with_untracked(|p| p.debugger.hover_variables) {
        return None;
    }
    let paused = view_state().with_untracked(|view| {
        view.doc == Some(id) && matches!(view.state, Some(DebuggerState::Paused { .. }))
    });
    if !paused {
        return None;
    }
    let text = state::text(id)?;
    let line_str = text.split('\n').nth(pos.line as usize)?;
    let ident = resolve_hover_ident(line_str, pos.column)?;
    view_state().with_untracked(|view| {
        if let Some(w) = view.watches.iter().find(|w| w.expression == ident) {
            if let Some(val) = &w.value {
                return Some((ident.clone(), val.clone()));
            }
        }
        find_var_value(&view.variables, &ident).map(|val| (ident, val))
    })
}

fn resolve_hover_ident(line_str: &str, utf16_col: u32) -> Option<String> {
    let byte_idx = scratch_search::utf16_to_byte(line_str, utf16_col as usize);
    if byte_idx >= line_str.len() {
        return None;
    }
    let ch = line_str[byte_idx..].chars().next()?;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    if !is_ident(ch) {
        return None;
    }
    let mut start = byte_idx;
    while start > 0 {
        let prev = line_str[..start].chars().next_back()?;
        if !is_ident(prev) {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = byte_idx + ch.len_utf8();
    while end < line_str.len() {
        let next = line_str[end..].chars().next()?;
        if !is_ident(next) {
            break;
        }
        end += next.len_utf8();
    }
    let ident = &line_str[start..end];
    if ident.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(ident.to_owned())
}

fn find_var_value(vars: &[Variable], ident: &str) -> Option<String> {
    let trimmed = ident.trim_start_matches('$');
    for var in vars {
        let name = var.name.trim_start_matches('$');
        if var.name == ident || var.display_name == ident || name == trimmed {
            if let Some(val) = &var.value {
                return Some(val.clone());
            }
            if let Some(desc) = &var.description {
                return Some(desc.clone());
            }
        }
    }
    for var in vars {
        if let Some(found) = find_var_value(&var.children, ident) {
            return Some(found);
        }
    }
    None
}

/// Called from `state::toggle_breakpoint`: mirrors the change into a live
/// session for the same document.
pub fn sync_breakpoints(id: DocumentId) {
    if !debugging(id) {
        return;
    }
    send(SessionCommand::SetBreakpoints(breakpoints_of(id)));
}

/// Run ▸ Run and Debug: compiles first when needed via `RunMode::Debug`,
/// otherwise prepares a snapshot, and starts a `scratch-debugger` session.
pub fn run_in_debugger() {
    let Some(id) = with(|workspace| workspace.active) else { return };
    if let Some(handle) = with(|workspace| workspace.doc_mut(id).and_then(|d| d.handle.take())) {
        handle.cancel();
    }
    stop();
    let info = with(|workspace| {
        let doc = workspace.doc(id)?;
        let def = workspace.registry.as_ref()?.get(&doc.document.language).cloned()?;
        Some((
            doc.document.clone(),
            doc.title.clone(),
            doc.text.clone(),
            def,
            workspace.runner.clone(),
        ))
    });
    let Some((document, title, text, language_def, runner)) = info else { return };
    let registry = AdapterRegistry::builtin();
    let preferred = language_def
        .debugger
        .as_deref()
        .and_then(|dbg_id| registry.get(dbg_id))
        .or_else(|| registry.for_language(&document.language))
        .cloned();
    let Some(adapter) = preferred else {
        console_write(id, ConsoleStream::System, &format!("No debugger is available for {}.", language_def.name));
        return;
    };

    let shell = super::execution::current_shell_settings();
    let runner_settings = super::run_settings::runner_settings(&document);
    let program_args = runner_settings.argument_list().unwrap_or_default();
    let input_text = match &runner_settings.input {
        Some(scratch_runner::ProgramInput::Text(s)) => Some(s.clone()),
        _ => None,
    };

    if language_def.has_compile_step() {
        let mut req = super::execution::request_for(&document, &title, text, &language_def)
            .with_mode(RunMode::Debug)
            .with_shell(shell);
        req.options.keep_temp = true;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let Ok(handle) = runner.start(req, move |msg| {
            let _ = tx.send(msg);
        }) else {
            state::notice("Could not start debug compilation.");
            return;
        };
        let run_id = handle.id();
        let signals = with(|workspace| {
            workspace.doc_mut(id).map(|doc| {
                doc.console.clear();
                doc.annotations.clear();
                doc.console.write_line(ConsoleStream::System, &format!("Preparing debug build for {title}…"));
                doc.handle = Some(handle);
                (doc.running, doc.status, doc.console_rev)
            })
        });
        if let Some((running, status, console_rev)) = signals {
            running.set(true);
            status.set(format!("Compiling {title}…"));
            console_rev.update(|v| *v = v.wrapping_add(1));
        }
        motion::animate_panel();
        state::display().update(|v| v.console_visible = true);
        let saved_doc_path = state::saved_path(&document);
        Owner::detached().run(move || spawn(async move {
            let mut compiled_info = None;
            while let Some(msg) = rx.recv().await {
                if !with(|w| w.doc(id).is_some_and(|d| d.handle.as_ref().is_some_and(|h| h.id() == run_id))) {
                    return;
                }
                match msg.event {
                    RunEvent::Output { stream, bytes, .. } => {
                        let stream = match stream {
                            Stream::Stdout => ConsoleStream::Stdout,
                            Stream::Stderr => ConsoleStream::Stderr,
                        };
                        with(|workspace| {
                            if let Some(doc) = workspace.doc_mut(id) {
                                doc.console.append(stream, &bytes);
                            }
                        });
                        if let Some(rev) = with(|w| w.doc(id).map(|d| d.console_rev)) {
                            rev.update(|v| *v = v.wrapping_add(1));
                        }
                    }
                    RunEvent::Compiled(program) => compiled_info = Some(*program),
                    RunEvent::Finished(summary) => {
                        let sigs = with(|workspace| {
                            workspace.doc_mut(id).map(|doc| {
                                doc.console.finish_streams();
                                if doc.handle.as_ref().map(|h| h.id()) == Some(run_id) {
                                    doc.handle = None;
                                }
                                (doc.running, doc.status, doc.console_rev)
                            })
                        });
                        if let Some(info) = compiled_info {
                            if let Some(tmpdir) = info.env.get("CR_TMPDIR") {
                                DEBUG_TEMP_DIR.with(|slot| {
                                    if let Some(old) = slot.borrow_mut().replace(PathBuf::from(tmpdir)) {
                                        let _ = std::fs::remove_dir_all(old);
                                    }
                                });
                            }
                            let executable = info
                                .compiler
                                .as_deref()
                                .map(PathBuf::from)
                                .filter(|p| p.exists())
                                .or_else(|| {
                                    (adapter.id != "jdb")
                                        .then(|| info.argv.first().map(PathBuf::from))
                                        .flatten()
                                        .filter(|p| p.exists())
                                })
                                .unwrap_or_else(|| info.file.clone());
                            let mut source_map = SourceMap::new();
                            if let Some(saved) = &saved_doc_path {
                                source_map.add(info.file.clone(), saved.clone());
                            }
                            let bp_source = info.file.clone();
                            start_session(
                                id,
                                &title,
                                adapter,
                                executable,
                                bp_source,
                                info.cwd,
                                program_args,
                                info.env.into_iter().collect(),
                                source_map,
                                input_text,
                            );
                        } else {
                            if let Some(m) = &summary.message {
                                console_write(id, ConsoleStream::System, m);
                            }
                            if let scratch_runner::RunStatus::Error(err) = &summary.status {
                                console_write(id, ConsoleStream::Stderr, &err.to_string());
                            }
                            if let Some((running, status, console_rev)) = sigs {
                                running.set(false);
                                status.set(summary.status_text());
                                console_rev.update(|v| *v = v.wrapping_add(1));
                            }
                            super::execution::import_diagnostics(id);
                        }
                        return;
                    }
                    _ => {}
                }
            }
        }));
        return;
    }

    // Interpreted languages debug a temp snapshot.
    let extension = language_def.extensions.first().cloned().unwrap_or_else(|| "txt".into());
    let program = std::env::temp_dir().join(format!("scratchrunner-debug-{id}.{extension}"));
    if std::fs::write(&program, &text).is_err() {
        return;
    }
    DEBUG_TEMP_FILE.with(|slot| {
        if let Some(old) = slot.borrow_mut().replace(program.clone()) {
            let _ = std::fs::remove_file(old);
        }
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755));
    }
    let cwd = resolve_debug_cwd(&document);
    let mut env = shell.variables;
    for (k, v) in runner_settings.environment {
        env.insert(k, v);
    }
    start_session(
        id,
        &title,
        adapter,
        program.clone(),
        program,
        cwd,
        program_args,
        env,
        SourceMap::new(),
        input_text,
    );
}

fn resolve_debug_cwd(document: &scratch_document::Document) -> PathBuf {
    match &document.run.working_folder {
        scratch_document::RunFolder::File => state::saved_path(document)
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir),
        scratch_document::RunFolder::Home => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir),
        scratch_document::RunFolder::Custom(p) => PathBuf::from(p),
    }
}

#[allow(clippy::too_many_arguments)]
fn start_session(
    id: DocumentId,
    title: &str,
    adapter: DebuggerAdapter,
    program: PathBuf,
    source_path: PathBuf,
    cwd: PathBuf,
    program_args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    source_map: SourceMap,
    program_input: Option<String>,
) {
    SOURCE_PATH.with(|p| *p.borrow_mut() = Some(source_path));
    let mut config = SessionConfig::new(adapter.clone(), program)
        .cwd(cwd)
        .env(env)
        .source_map(source_map)
        .breakpoints(breakpoints_of(id));
    for arg in program_args {
        config = config.program_arg(arg);
    }
    if let Some(input_text) = program_input {
        config = config.program_input(input_text);
    }
    let session = match DebugSession::start(config) {
        Ok(session) => session,
        Err(error) => {
            let hint = adapter.install_hint.clone().unwrap_or_default();
            let detail = if hint.is_empty() {
                format!("{error}")
            } else {
                format!("{error}\n\n{hint}")
            };
            console_write(id, ConsoleStream::System, &format!("Could not start {}: {error}", adapter.name));
            SOURCE_PATH.with(|p| *p.borrow_mut() = None);
            if let Some(tmpdir) = DEBUG_TEMP_DIR.with(|p| p.borrow_mut().take()) {
                let _ = std::fs::remove_dir_all(tmpdir);
            }
            if let Some(tmpfile) = DEBUG_TEMP_FILE.with(|p| p.borrow_mut().take()) {
                let _ = std::fs::remove_file(tmpfile);
            }
            if let Some((running, status)) = with(|w| w.doc(id).map(|d| (d.running, d.status))) {
                running.set(false);
                status.set("Debugger Unavailable".into());
            }
            Owner::detached().run(move || spawn(async move {
                let _ = nsh_base::alert_dialog()
                    .title("Could Not Start Debugger")
                    .description(detail)
                    .action("ok", "OK")
                    .show()
                    .await;
            }));
            return;
        }
    };
    SESSION.with(|slot| *slot.borrow_mut() = Some(session));
    view_state().update(|view| {
        *view = DebugView {
            doc: Some(id),
            adapter_id: adapter.id.clone(),
            state: Some(DebuggerState::Starting),
            pane: true,
            ..DebugView::default()
        };
    });
    motion::animate_panel();
    state::display().update(|v| v.console_visible = true);
    let signals = with(|workspace| {
        workspace.doc_mut(id).map(|doc| {
            doc.console
                .write_line(ConsoleStream::System, format!("Debugging with {}", adapter.name).as_str());
            doc.annotations.clear();
            (doc.running, doc.status, doc.console_rev)
        })
    });
    if let Some((running, status, console_rev)) = signals {
        running.set(true);
        status.set(format!("Debugging {title}"));
        console_rev.update(|v| *v = v.wrapping_add(1));
    }
    pump(id);
}

/// Event drain: polls `try_recv` on the engine executor — `DebugSession`
/// stays on the UI thread; 30 ms granularity needs no blocking wait.
fn pump(id: DocumentId) {
    // Detached: menu commands that start a session run under a transient
    // owner whose drop would otherwise cancel the drain (as with the run
    // pump in execution.rs).
    Owner::detached().run(|| spawn(async move {
        loop {
            let events = SESSION.with(|session| {
                let session = session.borrow();
                let mut out = Vec::new();
                if let Some(session) = session.as_ref() {
                    while let Some(event) = session.try_recv() {
                        out.push(event);
                    }
                }
                out
            });
            for event in events {
                if !handle_event(id, event) {
                    return;
                }
            }
            if SESSION.with(|session| session.borrow().is_none()) {
                return;
            }
            timer(Duration::from_millis(30)).await;
        }
    }));
}

/// Applies one event; `false` ends the pump.
fn handle_event(id: DocumentId, event: DebugEvent) -> bool {
    match event {
        DebugEvent::State(state) => {
            let line = match &state {
                DebuggerState::Paused { location, .. } => location.as_ref().and_then(|l| l.line),
                _ => None,
            };
            view_state().update(|view| {
                view.state = Some(state);
                if line.is_none() {
                    view.line = None;
                }
            });
        }
        DebugEvent::Started => {}
        DebugEvent::Output { stream, text } => {
            let stream = match stream {
                scratch_debugger::OutputStream::Stdout => ConsoleStream::Stdout,
                scratch_debugger::OutputStream::Stderr => ConsoleStream::Stderr,
            };
            console_write(id, stream, &text);
        }
        DebugEvent::DebuggerOutput { text } | DebugEvent::ExpressionResult { text, .. } => {
            // The adapter helpers answer variable queries on the debugger's
            // stdout as `SCRATCHDBG:{json}`; the session parses those — they
            // are protocol, not output.
            if !text.starts_with("SCRATCHDBG:") {
                console_write(id, ConsoleStream::Debugger, &text);
            }
        }
        DebugEvent::LogLine { .. } => {}
        DebugEvent::StoppedAt {
            location,
            reason: _,
            annotation,
        } => {
            view_state().update(|view| {
                view.line = location.line;
                view.annotation = annotation;
            });
            bump(id);
        }
        DebugEvent::StackChanged(stack) => {
            let selected = stack.current().map(|frame| frame.id());
            view_state().update(|view| {
                view.stack = stack.frames.clone();
                view.selected = selected;
            });
            if let Some(frame) = selected {
                send(SessionCommand::Variables(VariablesQuery::new(frame)));
            }
        }
        DebugEvent::VariablesChanged { parent, variables, .. } => {
            let sort_mode = super::preferences::settings().with_untracked(|p| p.debugger.variables_sort_mode);
            view_state().update(|view| {
                view.edit_error = None;
                match parent {
                // Fresh roots for the frame: expansion ids are index paths,
                // meaningless after a step, so they are dropped.
                None => {
                    view.variables = variables.roots.clone();
                    view.expanded.clear();
                    rebuild_flat(view, sort_mode);
                }
                // The subtree of a lazily expanded node replaces its
                // children; the node stays open.
                Some(id) => {
                    if let Some(node) = find_variable(&mut view.variables, &id) {
                        node.children = variables.roots.clone();
                        assign_child_ids(node);
                    }
                    view.expanded.insert(id);
                    rebuild_flat(view, sort_mode);
                }
                }
            });
        }
        DebugEvent::WatchValues(values) => {
            view_state().update(|view| {
                for value in values {
                    match view.watches.iter_mut().find(|w| w.expression == value.expression) {
                        Some(watch) => *watch = value,
                        None => view.watches.push(value),
                    }
                }
            });
        }
        DebugEvent::BreakpointsChanged(_) => {}
        DebugEvent::Exited { code } => {
            finish(id, format!("Debug session ended with status {code}"));
            return false;
        }
        DebugEvent::Error { message } => {
            // `report_error` emits Error for recoverable failures (unsupported
            // command, failed variable assignment); `fail()` additionally
            // flips the session state to Error — only that one is fatal.
            let fatal = SESSION.with(|session| {
                session
                    .borrow()
                    .as_ref()
                    .is_some_and(|sess| matches!(sess.state(), DebuggerState::Error { .. }))
            });
            if fatal {
                finish(id, format!("Debugger error: {message}"));
                return false;
            }
            console_write(id, ConsoleStream::System, &format!("Debugger: {message}"));
            view_state().update(|view| view.edit_error = Some(message));
        }
    }
    true
}

/// Writes a console line and bumps `console_rev` outside the borrow.
fn console_write(id: DocumentId, stream: ConsoleStream, text: &str) {
    with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.console.write_line(stream, text);
        }
    });
    let rev = with(|workspace| workspace.doc(id).map(|doc| doc.console_rev));
    if let Some(rev) = rev {
        rev.update(|v| *v = v.wrapping_add(1));
    }
}

/// Ends the session's UI state and writes the final console line.
fn finish(id: DocumentId, message: String) {
    let signals = with(|workspace| {
        workspace.doc_mut(id).map(|doc| {
            if !message.is_empty() {
                doc.console.write_line(ConsoleStream::System, &message);
            }
            (doc.running, doc.status, doc.console_rev)
        })
    });
    if let Some((running, status, console_rev)) = signals {
        running.set(false);
        status.set(String::new());
        console_rev.update(|v| *v = v.wrapping_add(1));
    }
    let hide_auto = super::preferences::settings().with_untracked(|p| p.debugger.hide_view_automatically);
    view_state().update(|view| {
        view.state = None;
        view.adapter_id.clear();
        view.line = None;
        view.annotation.clear();
        view.stack.clear();
        view.variables.clear();
        view.expanded.clear();
        view.flat.clear();
        view.selected = None;
        view.watches.clear();
        view.editing = None;
        view.edit_error = None;
        // CodeRunner `AutomaticallyHideDebuggerView` (default on): the debug
        // column closes with the session instead of showing stale state.
        if hide_auto {
            view.pane = false;
        }
    });
    SESSION.with(|session| *session.borrow_mut() = None);
    SOURCE_PATH.with(|p| *p.borrow_mut() = None);
    if let Some(tmpdir) = DEBUG_TEMP_DIR.with(|p| p.borrow_mut().take()) {
        let _ = std::fs::remove_dir_all(tmpdir);
    }
    if let Some(tmpfile) = DEBUG_TEMP_FILE.with(|p| p.borrow_mut().take()) {
        let _ = std::fs::remove_file(tmpfile);
    }
    bump(id);
}

fn assign_child_ids(node: &mut Variable) {
    let base = node.id.clone();
    for (idx, child) in node.children.iter_mut().enumerate() {
        child.id = base.child(idx);
        assign_child_ids(child);
    }
}

/// Bumps the decoration revision so the editor's `decorations` binding
/// redraws the execution marker.
fn bump(id: DocumentId) {
    state::bump_text_rev(id);
}

/// Node `id` inside `roots`, walked by its index path.
fn find_variable<'a>(roots: &'a mut [Variable], id: &VariableId) -> Option<&'a mut Variable> {
    let (first, rest) = id.path().split_first()?;
    let mut node = roots.get_mut(*first as usize)?;
    for &index in rest {
        node = node.children.get_mut(index as usize)?;
    }
    Some(node)
}

/// Rebuilds the flat display rows from the tree + expansion set under `sort_mode`.
fn rebuild_flat(view: &mut DebugView, sort_mode: scratch_config::VariablesSortMode) {
    use scratch_config::VariablesSortMode;
    view.flat.clear();
    fn walk(view: &mut DebugView, var: &Variable, depth: usize, sort_mode: VariablesSortMode) {
        let open = view.expanded.contains(&var.id);
        view.flat.push((var.clone(), depth));
        if open {
            let mut children: Vec<&Variable> = var.children.iter().collect();
            match sort_mode {
                VariablesSortMode::Name => {
                    children.sort_by_key(|v| v.name.to_lowercase());
                }
                VariablesSortMode::ReverseNatural => children.reverse(),
                VariablesSortMode::Natural => {}
            }
            for child in children {
                walk(view, child, depth + 1, sort_mode);
            }
        }
    }
    let roots = std::mem::take(&mut view.variables);
    let mut ordered: Vec<&Variable> = roots.iter().collect();
    match sort_mode {
        VariablesSortMode::Name => ordered.sort_by_key(|v| v.name.to_lowercase()),
        VariablesSortMode::ReverseNatural => ordered.reverse(),
        VariablesSortMode::Natural => {}
    }
    for var in ordered {
        walk(view, var, 0, sort_mode);
    }
    view.variables = roots;
}

/// Opens/closes a variable row: fetched children collapse locally, unfetched
/// (`expandable`) subtrees request `ExpandVariable` from the session.
fn toggle_variable(index: usize) {
    let sort_mode = super::preferences::settings().with_untracked(|p| p.debugger.variables_sort_mode);
    let (id, expandable, has_children) = {
        let view = view_state().get_untracked();
        let Some((var, _)) = view.flat.get(index) else { return };
        (var.id.clone(), var.expandable, !var.children.is_empty())
    };
    let expanded = view_state().get_untracked().expanded.contains(&id);
    if expanded {
        view_state().update(|view| {
            view.expanded.remove(&id);
            rebuild_flat(view, sort_mode);
        });
    } else if expandable && !has_children {
        if let Some(frame) = view_state().get_untracked().selected {
            send(SessionCommand::ExpandVariable { frame, node: id, depth: 0 });
        }
    } else {
        view_state().update(|view| {
            view.expanded.insert(id);
            rebuild_flat(view, sort_mode);
        });
    }
}

// ------------------------------------------------------------------ controls

/// Continue Execution (⌃⌘Y in CodeRunner's spirit; plain buttons here).
pub fn continue_() {
    send(SessionCommand::Continue);
    view_state().update(|view| {
        view.line = None;
        view.annotation.clear();
    });
    if let Some(id) = view_state().with_untracked(|view| view.doc) {
        bump(id);
    }
}

/// Pause a running program.
pub fn pause() {
    send(SessionCommand::Pause);
}

/// "Pause Execution" (⌃1): pauses a running session, continues a paused
/// one — the menu item toggles by state.
pub fn pause_or_continue() {
    if view_state().with_untracked(|view| matches!(view.state, Some(DebuggerState::Running))) {
        pause();
    } else if view_state().with_untracked(|view| matches!(view.state, Some(DebuggerState::Paused { .. }))) {
        continue_();
    }
}

/// "Clear Breakpoints" (⌘Y): removes the active document's breakpoints and
/// mirrors the change into a live session.
pub fn clear_breakpoints() {
    let Some(id) = with(|workspace| workspace.active) else { return };
    with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.document.breakpoints = scratch_document::Breakpoints::from_list(Vec::new());
            workspace.changed();
        }
    });
    bump(id);
    sync_breakpoints(id);
}
/// Step Over.
pub fn step_over() {
    send(SessionCommand::StepOver);
}
/// Step Into.
pub fn step_in() {
    send(SessionCommand::StepIn);
}
/// Step Out.
pub fn step_out() {
    send(SessionCommand::StepOut);
}

/// Stop Debugging: asks the session to quit; `finish` tidies up.
pub fn stop() {
    if let Some(id) = view_state().with_untracked(|view| view.doc) {
        if view_state().with_untracked(|view| view.state.is_some()) {
            send(SessionCommand::Quit);
        }
        finish(id, String::new());
    }
}

fn select_frame(frame: FrameId) {
    send(SessionCommand::SelectFrame(frame));
    send(SessionCommand::Variables(VariablesQuery::new(frame)));
    view_state().update(|view| view.selected = Some(frame));
}

/// The console's input line during a debug session: while the debugger
/// is stopped it is the debugger's console (the line runs as a debugger
/// command — CodeRunner's behavior); while the program runs the line
/// feeds the program's stdin.
pub fn send_input(text: &str) {
    if view_state().with(|view| matches!(view.state, Some(DebuggerState::Paused { .. }))) {
        send(SessionCommand::Repl { command: text.to_owned() });
    } else {
        send(SessionCommand::ProgramInput {
            data: format!("{text}\n"),
        });
    }
}

/// Immutable counterpart of [`find_variable`].
fn find_variable_ref<'a>(roots: &'a [Variable], id: &VariableId) -> Option<&'a Variable> {
    let (first, rest) = id.path().split_first()?;
    let mut var = roots.get(*first as usize)?;
    for index in rest {
        var = var.children.get(*index as usize)?;
    }
    Some(var)
}

/// Commits an inline value edit on a variable row. Empty text or an
/// unchanged value cancels; otherwise the session runs the adapter's
/// assignment (`pdb: !{path} = {value}`) and refreshes the frame's
/// variables — failures surface through a non-fatal `Error` event.
fn commit_variable_edit(vid: VariableId, value: &str) {
    let value = value.trim().to_owned();
    let target = view_state().with(|view| {
        find_variable_ref(&view.variables, &vid).and_then(|var| {
            Some((view.selected, var.expr.clone()?, var.value.clone()))
        })
    });
    view_state().update(|view| view.editing = None);
    let Some((frame, path, current)) = target else { return };
    if value.is_empty() || current.as_deref() == Some(value.as_str()) {
        return;
    }
    let Some(frame) = frame else { return };
    send(SessionCommand::SetVariable { path, value, frame });
}

/// Adds a watch expression (evaluated at every stop).
pub fn add_watch(expr: &str) {
    let expr = expr.trim();
    if expr.is_empty() {
        return;
    }
    if view_state().with(|view| view.watches.iter().any(|w| w.expression == expr)) {
        return;
    }
    send(SessionCommand::AddWatch {
        expr: expr.to_owned(),
    });
    view_state().update(|view| {
        view.watches.push(WatchValue {
            expression: expr.to_owned(),
            value: None,
            error: None,
        });
    });
}

/// Removes a watch expression.
pub fn remove_watch(expr: &str) {
    send(SessionCommand::RemoveWatch {
        expr: expr.to_owned(),
    });
    view_state().update(|view| view.watches.retain(|w| w.expression != expr));
}

/// Console toolbar's debugger segment (DESIGN 8.7): show/hide the pane.
pub fn toggle_pane() {
    motion::animate_panel();
    view_state().update(|view| view.pane = !view.pane);
}

/// Toggle a breakpoint at the caret of the active document (Editor menu).
pub fn toggle_breakpoint_at_caret() {
    let Some(id) = with(|workspace| workspace.active) else { return };
    let Some(api) = editor_api(id) else { return };
    let Some(head) = api.selections().first().map(|s| s.head) else { return };
    state::toggle_breakpoint(id, head.line);
}

// ------------------------------------------------------------------ pane UI

/// Column 1 inside the debug workspace (DESIGN 8.9): stepping toolbar,
/// paused stop reason annotation, Call Stack, and Breakpoints.
pub fn stack_pane() -> impl IntoElement {
    let paused = || view_state().with(|view| matches!(view.state, Some(DebuggerState::Paused { .. })));
    let running = || view_state().with(|view| matches!(view.state, Some(DebuggerState::Running)));
    let stopped = || view_state().with(|view| matches!(view.state, Some(DebuggerState::Paused { .. })));
    let live = || view_state().with(|view| view.state.is_some());
    let tool = |icon: IconName, title: &'static str, action: fn(), enabled: fn() -> bool| {
        view()
            .flex_row()
            .items_center()
            .justify_center()
            .w(px(22.0))
            .h(px(20.0))
            .rounded(px(tokens::radius_sm()))
            .hover(|h| h.bg(tokens::accent))
            .active(|h| h.bg(tokens::capsule_pressed))
            .focusable(enabled)
            .focus_visible(|h| h.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
            .when(!motion::reduced(), motion::state_wash)
            .when(!motion::reduced(), motion::press)
            .active(|h| motion::press_scale(h, true))
            .on_press(move |_: &PressEvent| {
                if enabled() {
                    action();
                }
            })
            .role(AccessibilityRole::Button)
            .label(title)
            .child(dynamic(move || {
                glyph(
                    icon,
                    IconSize::Sm,
                    if enabled() { Tone::Default } else { Tone::Disabled },
                )
                .into_element()
            }))
    };
    view()
        .flex_1()
        .min_w(px(130.0))
        .h_full()
        .flex_col()
        .bg(tokens::sidebar)
        .child(
            view()
                .h(px(24.0))
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .px_1()
                .border_b_1()
                .border_color(tokens::sidebar_border)
                .child(tool(IconName::DebugContinue, "Continue Execution", continue_ as fn(), paused))
                .child(tool(IconName::Pause, "Pause Execution", pause as fn(), running))
                .child(tool(IconName::DebugStepOver, "Step Over", step_over as fn(), stopped))
                .child(tool(IconName::DebugStepIn, "Step Into", step_in as fn(), stopped))
                .child(tool(IconName::DebugStepOut, "Step Out", step_out as fn(), stopped))
                .child(view().flex_1())
                .child(tool(IconName::Stop, "Stop Debugging", stop as fn(), live)),
        )
        .child(when(move || view_state().with(|v| !v.annotation.is_empty()), || {
            view().px_2().py(px(3.0)).bg(tokens::info_muted).border_b_1().border_color(tokens::sidebar_border)
                .child(text(move || view_state().with(|v| v.annotation.clone()))
                    .text_xs().text_color(tokens::info).truncate())
        }))
        .child(list_header("Call Stack"))
        .child(
            virtual_list(
                move || view_state().with(|view| view.stack.len()),
                move |at| stack_row(at.index()),
            )
            .estimated_item_size(px(20.0))
            .flex_1()
            .min_h(px(48.0))
            .empty(
                text("No frames")
                    .text_xs()
                    .text_color(tokens::muted_foreground)
                    .p_2(),
            ),
        )
        .child(list_header("Breakpoints"))
        .child(
            scroll().max_h(px(110.0)).flex_shrink(0.0).child(
                dynamic(move || {
                    let breakpoints = with(|workspace| workspace.active)
                        .and_then(|id| {
                            with(|workspace| {
                                let doc = workspace.doc(id)?;
                                let _ = doc.text_rev.get();
                                Some((id, doc.document.breakpoints.clone()))
                            })
                        })
                        .map(|(id, breakpoints)| {
                            view()
                                .flex_col()
                                .children(
                                    breakpoints
                                        .as_slice()
                                        .iter()
                                        .map(|breakpoint| breakpoint_row(id, breakpoint.clone())),
                                )
                                .into_element()
                        });
                    match breakpoints {
                        Some(element) => element,
                        None => view().into_element(),
                    }
                }),
            ),
        )
}

/// Column 2 inside the debug workspace (DESIGN 8.9): Variables tree + Watches.
pub fn variables_pane() -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(150.0))
        .h_full()
        .flex_col()
        .bg(tokens::sidebar)
        .child(variables_header())
        .child(
            virtual_list(
                move || view_state().with(|view| view.flat.len()),
                move |at| variable_row(at.index()),
            )
            .estimated_item_size(px(20.0))
            .flex_1()
            .min_h(px(48.0))
            .empty(
                text("No variables")
                    .text_xs()
                    .text_color(tokens::muted_foreground)
                    .p_2(),
            ),
        )
        .child(dynamic(move || {
            match view_state().with(|view| view.edit_error.clone()) {
                Some(message) => text(message)
                    .text_xs()
                    .text_color(tokens::destructive)
                    .px_2()
                    .into_element(),
                None => view().h(px(0.0)).into_element(),
            }
        }))
        .child(list_header("Watches"))
        .child(watch_input())
        .child(
            scroll().max_h(px(90.0)).flex_shrink(0.0).child(
                dynamic(move || {
                    view_state().with(|v| {
                        view()
                            .flex_col()
                            .children(v.watches.iter().map(|watch| watch_row(watch.clone())))
                            .into_element()
                    })
                }),
            ),
        )
}

/// Legacy combined debugger pane wrapper (kept for callers outside `workspace_panels`).
#[allow(dead_code)]
pub fn pane() -> impl IntoElement {
    view().w(px(400.0)).h_full().flex_row()
        .child(view().w(px(190.0)).h_full().border_r_1().border_color(tokens::sidebar_border).child(stack_pane()))
        .child(view().w(px(210.0)).h_full().border_r_1().border_color(tokens::sidebar_border).child(variables_pane()))
}

/// Breakpoint Condition / Options dialog overlay.
pub(super) fn overlay() -> impl IntoElement {
    let shown = computed(move || bp_sheet().with(|s| s.open));
    view().child(dynamic(move || {
        if !shown.get() {
            return view().into_element();
        }
        let initial = bp_sheet().get_untracked();
        let Some(doc_id) = initial.doc else { return view().into_element(); };
        let line = initial.line;
        let enabled = signal(initial.enabled);
        let condition = signal(initial.condition.clone());
        let close = || bp_sheet().update(|s| s.open = false);
        let save = move || {
            state::set_breakpoint_enabled(doc_id, line, enabled.get_untracked());
            state::set_breakpoint_condition(doc_id, line, Some(condition.get_untracked()));
            close();
        };
        dialog()
            .label("Edit Breakpoint")
            .on_cancel(close)
            .flex_1()
            .child(view().absolute().inset_0().bg(tokens::scrim))
            .child(view().flex_1().items_center().justify_center()
                .child(view().w(px(360.0)).p_5().gap_3()
                    .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
                    .bg(tokens::background).shadow_lg().shadow_color(tokens::shadow())
                    .child(type_subhead().semibold().apply(text(format!("Breakpoint at Line {}", line + 1))).text_color(tokens::foreground))
                    .child(checkbox("Enabled", move || enabled.get(), CheckboxOptions::new(), move |v| enabled.set(v)))
                    .child(view().gap_1()
                        .child(type_body().medium().apply(text("Condition:")).text_color(tokens::foreground))
                        .child(text_field(
                            "e.g. count > 10",
                            move || SharedString::from(condition.get()),
                            move |v: &str| condition.set(v.to_owned()),
                            TextFieldOptions::new().mono(true).accessibility_label("Breakpoint condition"),
                            None,
                        )))
                    .child(view().flex_row().items_center().gap_2().pt_2()
                        .child(button("Remove", ButtonOptions::new().variant(ButtonVariant::Destructive).size(ControlSize::Sm), move || {
                            state::toggle_breakpoint(doc_id, line);
                            close();
                        }))
                        .child(view().flex_1())
                        .child(button("Cancel", ButtonOptions::new().size(ControlSize::Sm), close))
                        .child(button("OK", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Sm), save)))))
            .into_element()
    }))
}

/// The "add a watch" field: Return installs the expression (the session
/// re-evaluates it at every stop).
fn watch_input() -> impl IntoElement {
    let draft = signal(String::new());
    view()
        .h(px(tokens::row_sm()))
        .px_2()
        .items_center()
        .child(
            input()
                .value(move || draft.get())
                .placeholder(|| "Watch expression")
                .placeholder_color(|| color(tokens::muted_foreground()))
                .caret_color(|| color(tokens::primary()))
                .on_change(move |value: &str| draft.set(value.to_owned()))
                .on_submit(move |value: &str| {
                    add_watch(value);
                    draft.set(String::new());
                }),
        )
}

/// One watch row: expression, latest value (or error), and a remove button.
fn watch_row(watch: WatchValue) -> impl IntoElement {
    let (expr, shown, error) = (
        watch.expression.clone(),
        watch.value.clone().or(watch.error.clone()).unwrap_or_else(|| "—".into()),
        watch.error.is_some(),
    );
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(tokens::row_sm()))
        .px_2()
        .child(text(expr).text_xs().text_color(tokens::foreground))
        .child(view().flex_1())
        .child(
            text(shown)
                .text_xs()
                .text_color(move || {
                    if error { tokens::destructive() } else { tokens::muted_foreground() }
                }),
        )
        .child(
            view()
                .w(px(16.0))
                .h(px(16.0))
                .items_center()
                .justify_center()
                .rounded(px(tokens::radius_xs()))
                .hit_slop(EdgeInsets::all(px(4.0)))
                .hover(|h| h.bg(tokens::accent))
                .active(|h| h.bg(tokens::capsule_pressed))
                .focusable(true)
                .focus_visible(|h| h.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
                .when(!motion::reduced(), motion::state_wash)
                .when(!motion::reduced(), motion::press)
                .active(|h| motion::press_scale(h, true))
                .on_press({
                    let expr = watch.expression.clone();
                    move |_: &PressEvent| remove_watch(&expr)
                })
                .role(AccessibilityRole::Button)
                .label("Remove watch")
                .child(glyph(IconName::Close, IconSize::Xs, Tone::Muted)),
        )
}

/// One document breakpoint: an enable checkbox (CodeRunner's "Disable
/// Breakpoint"), its line, and an inline condition field (Edit Breakpoint).
fn breakpoint_row(id: DocumentId, breakpoint: scratch_document::Breakpoint) -> impl IntoElement {
    let line = breakpoint.line;
    let enabled = breakpoint.enabled;
    let draft = signal(breakpoint.condition.unwrap_or_default());
    let condition_key = format!("bp-{id}-{line}");
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(tokens::row_md()))
        .px_2()
        .on_context_menu(move |e: &ContextMenuEvent| {
            drop(popup_menu_at(gutter_context_menu(id, line), e.page_position));
        })
        .child(
            view().child(checkbox(
                format!("Line {}", line + 1),
                enabled,
                CheckboxOptions::new(),
                move |on| state::set_breakpoint_enabled(id, line, on),
            )),
        )
        .child(view().flex_1())
        .child(
            view().w(px(72.0)).child(text_field(
                "if …",
                move || SharedString::from(draft.get()),
                move |value: &str| {
                    draft.set(value.to_owned());
                    let value = value.to_owned();
                    super::preferences::defer(condition_key.clone(), move || {
                        state::set_breakpoint_condition(id, line, Some(value.clone()))
                    });
                },
                TextFieldOptions::new()
                    .mono(true)
                    .accessibility_label("Breakpoint condition"),
                None,
            )),
        )
        .child(icon_button(IconName::Pencil, "Edit Breakpoint…", IconButtonOptions::new().size(ControlSize::Xs), move || {
            edit_breakpoint_dialog(id, line);
        }))
}

fn list_header(title: &'static str) -> impl IntoElement {
    view()
        .h(px(20.0))
        .flex_row()
        .items_center()
        .px_2()
        .bg(tokens::muted)
        .border_t_1().border_b_1()
        .border_color(tokens::sidebar_border)
        .child(
            text(title)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens::muted_foreground),
        )
}

fn variables_header() -> impl IntoElement {
    view()
        .h(px(20.0))
        .flex_row()
        .items_center()
        .pl_2()
        .pr_1()
        .bg(tokens::muted)
        .border_t_1()
        .border_b_1()
        .border_color(tokens::sidebar_border)
        .child(
            text("Variables")
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens::muted_foreground)
                .flex_1(),
        )
        .child(icon_button(
            IconName::Ellipsis,
            "Variables options",
            IconButtonOptions::new().size(ControlSize::Xs),
            || {
                spawn(async {
                    let _ = popup_menu(variables_menu()).await;
                });
            },
        ))
}

fn variables_menu() -> Vec<MenuItem> {
    use scratch_config::VariablesSortMode;
    let adapter_id = view_state().with_untracked(|v| {
        if v.adapter_id.is_empty() { "default".to_owned() } else { v.adapter_id.clone() }
    });
    let (sort_mode, show_types, hover_vars, hide_auto) = super::preferences::settings().with_untracked(|p| {
        (
            p.debugger.variables_sort_mode,
            p.debugger.variables_display_types.get(&adapter_id).copied().unwrap_or(true),
            p.debugger.hover_variables,
            p.debugger.hide_view_automatically,
        )
    });
    let sort_item = |label: &'static str, mode: VariablesSortMode| {
        MenuItem::new(label)
            .checked(sort_mode == mode)
            .on_select(move || {
                super::preferences::write(move |p| p.debugger.variables_sort_mode = mode);
                view_state().update(|v| rebuild_flat(v, mode));
            })
    };
    let toggle_types_id = adapter_id.clone();
    vec![
        MenuItem::new("Sort Variables By").disabled(true),
        sort_item("Natural Order", VariablesSortMode::Natural),
        sort_item("Name", VariablesSortMode::Name),
        sort_item("Reverse Natural Order", VariablesSortMode::ReverseNatural),
        MenuItem::separator(),
        MenuItem::new("Show Variable Types").checked(show_types).on_select(move || {
            let key = toggle_types_id.clone();
            super::preferences::write(move |p| {
                let next = !p.debugger.variables_display_types.get(&key).copied().unwrap_or(true);
                p.debugger.variables_display_types.insert(key, next);
            });
        }),
        MenuItem::new("Show Values on Hover").checked(hover_vars).on_select(|| {
            super::preferences::write(|p| p.debugger.hover_variables = !p.debugger.hover_variables);
        }),
        MenuItem::new("Automatically Hide Debugger View").checked(hide_auto).on_select(|| {
            super::preferences::write(|p| {
                p.debugger.hide_view_automatically = !p.debugger.hide_view_automatically;
            });
        }),
    ]
}

/// One stack-frame row: `name  file:line`, selecting the frame on press.
fn stack_row(index: usize) -> impl IntoElement {
    let frame = move || {
        let _ = view_state().get();
        let view = view_state().get_untracked();
        view.stack.get(index).cloned()
    };
    let selected = move || {
        let view = view_state().get();
        view.stack
            .get(index)
            .is_some_and(|frame| view.selected == Some(frame.id()))
    };
    view()
        .h(px(20.0))
        .w_full()
        .flex_row()
        .items_center()
        .px_2()
        .gap_1()
        .bg(move || {
            if selected() {
                tokens::sidebar_accent()
            } else {
                tokens::sidebar()
            }
        })
        .hover(|s| s.bg(tokens::sidebar_accent))
        .on_press(move |_: &PressEvent| {
            let view = view_state().get_untracked();
            if let Some(frame) = view.stack.get(index) {
                select_frame(frame.id());
            }
        })
        .child(
            type_mono()
                .apply(text(move || {
                    frame().map_or_else(String::new, |frame| {
                        let at = match (&frame.file, frame.line) {
                            (Some(file), Some(line)) => format!(
                                "  {}:{line}",
                                file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                            ),
                            _ => String::new(),
                        };
                        format!("{}{at}", frame.name)
                    })
                }))
                .text_xs()
                .truncate()
                .text_color(move || {
                    if selected() {
                        tokens::sidebar_accent_foreground()
                    } else {
                        tokens::sidebar_foreground()
                    }
                }),
        )
}

/// One variable row: a disclosure triangle on nodes with children (fetched
/// or `expandable`), the name indented by depth, `name = value` after it.
/// Pressing the row toggles expansion.
fn variable_row(index: usize) -> impl IntoElement {
    let row = move || {
        let _ = view_state().get();
        let view = view_state().get_untracked();
        let adapter_id = if view.adapter_id.is_empty() { "default" } else { view.adapter_id.as_str() };
        let show_types = super::preferences::settings().with(|p| {
            p.debugger.variables_display_types.get(adapter_id).copied().unwrap_or(true)
        });
        view.flat.get(index).map(|(var, depth)| {
            let base = if var.display_name.is_empty() { var.name.clone() } else { var.display_name.clone() };
            let label = match (show_types, &var.type_name) {
                (true, Some(ty)) if !ty.trim().is_empty() => format!("{base}: {ty}"),
                _ => base,
            };
            let text = match (&var.value, &var.description) {
                (Some(value), _) => format!("{label} = {value}"),
                (None, Some(desc)) => format!("{label} = {desc}"),
                (None, None) => label,
            };
            let parent = var.expandable || !var.children.is_empty();
            let open = parent && view.expanded.contains(&var.id);
            (text, *depth, parent, open, var.id.clone(), var.expr.clone())
        })
    };
    view()
        .h(px(18.0))
        .w_full()
        .flex_row()
        .items_center()
        .pl(px(4.0))
        .pr_2()
        .gap(px(2.0))
        .hover(|h| h.bg(tokens::sidebar_accent))
        .on_press(move |event: &PressEvent| {
            let Some((_, _, parent, _, vid, expr)) = row() else { return };
            // Double-click a settable leaf turns the row into a value field
            // (CodeRunner: inline "Edit Value" on the variables outline).
            if event.clicks >= 2 && expr.is_some() {
                view_state().update(|v| {
                    v.editing = Some(vid);
                    v.edit_error = None;
                });
            } else if parent {
                toggle_variable(index);
            }
        })
        .child(dynamic(move || {
            let Some((_, depth, parent, open, _, _)) = row() else { return view().into_element() };
            view()
                .flex_row()
                .items_center()
                .w(px(12.0 + depth as f32 * 12.0))
                .justify_end()
                .child(if parent {
                    glyph(
                        if open { IconName::ChevronDown } else { IconName::ChevronRight },
                        IconSize::Xs,
                        Tone::Muted,
                    )
                    .into_element()
                } else {
                    view().into_element()
                })
                .into_element()
        }))
        .child(dynamic(move || {
            let Some((label, _, _, _, vid, expr)) = row() else {
                return view().into_element();
            };
            let editing = view_state().with(|v| v.editing.as_ref() == Some(&vid));
            if editing {
                let draft = signal(label.rsplit(" = ").next().unwrap_or("").to_owned());
                let vid = vid.clone();
                return view()
                    .flex_1()
                    .child(
                        input()
                            .value(move || draft.get())
                            .caret_color(|| color(tokens::primary()))
                            .on_change(move |value: &str| draft.set(value.to_owned()))
                            .on_submit(move |value: &str| {
                                commit_variable_edit(vid.clone(), value)
                            }),
                    )
                    .into_element();
            }
            type_mono()
                .apply(text(move || label.clone()))
                .text_xs()
                .truncate()
                .text_color(move || {
                    if expr.is_some() {
                        tokens::sidebar_foreground()
                    } else {
                        tokens::muted_foreground()
                    }
                })
                .into_element()
        }))
}

/// Whether the pane should render for the active tab (the console adds it
/// to the left of the output when this is true).
pub fn pane_visible() -> bool {
    let view = view_state().get();
    view.pane
        && view.doc.is_some_and(|doc| {
            with(|workspace| workspace.active) == Some(doc) && with(|workspace| workspace.doc(doc).is_some())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scratch_debugger::VariableKind;

    fn var(id: VariableId, name: &str, value: Option<&str>, children: Vec<Variable>) -> Variable {
        Variable {
            id,
            display_name: name.to_owned(),
            name: name.to_owned(),
            type_name: None,
            kind: VariableKind::Variable,
            value: value.map(str::to_owned),
            description: None,
            raw_value: None,
            expr: Some(name.to_owned()),
            children,
            expandable: false,
            truncated: false,
        }
    }

    #[test]
    fn assign_child_ids_prefixes_nested_variables() {
        let mut root = var(
            VariableId::root(1),
            "pt",
            None,
            vec![
                var(VariableId::root(0), "x", Some("10"), Vec::new()),
                var(VariableId::root(1), "y", Some("20"), Vec::new()),
            ],
        );
        assign_child_ids(&mut root);
        assert_eq!(root.children[0].id, VariableId::root(1).child(0));
        assert_eq!(root.children[1].id, VariableId::root(1).child(1));
    }

    #[test]
    fn hover_ident_handles_utf16_and_eol() {
        let line = "😀 foo = 123";
        assert_eq!(resolve_hover_ident(line, 3), Some("foo".to_owned()));
        assert_eq!(resolve_hover_ident(line, 50), None);
        assert_eq!(resolve_hover_ident(line, 10), None);
    }

    #[test]
    fn find_var_value_prefers_top_level_locals_over_struct_children() {
        let vars = vec![
            var(
                VariableId::root(0),
                "p",
                None,
                vec![var(VariableId::root(0).child(0), "x", Some("99"), Vec::new())],
            ),
            var(VariableId::root(1), "x", Some("42"), Vec::new()),
        ];
        assert_eq!(find_var_value(&vars, "x").as_deref(), Some("42"));
    }
}
