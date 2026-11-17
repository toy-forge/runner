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
use nsh_base::input;
use scratch_console_model::Stream as ConsoleStream;
use scratch_debugger::{
    AdapterRegistry, Breakpoint, DebugEvent, DebugSession, DebuggerState, Frame, FrameId,
    SessionCommand, SessionConfig, Variable, VariableId, VariablesQuery,
    variables::WatchValue,
};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use scratch_document::DocumentId;

use super::state::{self, editor_api, with};

thread_local! {
    static SESSION: RefCell<Option<DebugSession>> = const { RefCell::new(None) };
    /// The program path the live session was started on — `DebugSession`
    /// does not expose it, and breakpoint syncing needs it.
    static PROGRAM: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Reactive debugger state for the pane (DESIGN 8.9).
#[derive(Clone, Default)]
pub struct DebugView {
    /// Document the session is debugging.
    pub doc: Option<DocumentId>,
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
/// the debugger's 1-based), keyed to `program` — the snapshot file for a
/// debug session on unsaved text, the document's file when saved paths match.
fn breakpoints_of(id: DocumentId) -> Vec<Breakpoint> {
    let program = PROGRAM.with(|p| p.borrow().clone());
    let Some(program) = program else { return Vec::new() };
    with(|workspace| {
        workspace
            .doc(id)
            .map(|doc| {
                doc.document
                    .breakpoints
                    .as_slice()
                    .iter()
                    .map(|breakpoint| {
                        let mut installed = Breakpoint::new(program.clone(), breakpoint.line + 1);
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

/// Called from `state::toggle_breakpoint`: mirrors the change into a live
/// session for the same document.
pub fn sync_breakpoints(id: DocumentId) {
    if !debugging(id) {
        return;
    }
    send(SessionCommand::SetBreakpoints(breakpoints_of(id)));
}

/// Run ▸ Run in Debugger: start a session for the active document.
///
/// Interpreted languages debug a snapshot of the text (CodeRunner's "debug
/// the script"); a compiled adapter needs the built binary, which the runner
/// does not hand out yet.
pub fn run_in_debugger() {
    let Some(id) = with(|workspace| workspace.active) else { return };
    stop();
    let Some((language, text, title)) = with(|workspace| {
        workspace
            .doc(id)
            .map(|doc| (doc.document.language.clone(), doc.text.clone(), doc.title.clone()))
    }) else { return };
    let registry = AdapterRegistry::builtin();
    let Some(adapter) = registry.for_language(&language).cloned() else {
        console_write(id, ConsoleStream::System, &format!("No debugger is available for {language}."));
        return;
    };

    // The debugger's "program" is a snapshot of the text for interpreted
    // adapters (bashdb, pdb, node, …).
    let extension = with(|workspace| {
        workspace
            .registry
            .as_ref()
            .and_then(|r| r.get(&language))
            .and_then(|def| def.extensions.first().cloned())
    })
    .unwrap_or_else(|| "txt".into());
    let program = std::env::temp_dir().join(format!("scratchrunner-debug-{id}.{extension}"));
    if std::fs::write(&program, &text).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755));
    }

    PROGRAM.with(|p| *p.borrow_mut() = Some(program.clone()));
    let cwd = with(|workspace| {
        workspace
            .doc(id)
            .and_then(|doc| doc.document.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())))
    })
    .unwrap_or_else(std::env::temp_dir);
    let config = SessionConfig::new(adapter.clone(), program)
        .cwd(cwd)
        .breakpoints(breakpoints_of(id));
    let session = match DebugSession::start(config) {
        Ok(session) => session,
        Err(error) => {
            console_write(id, ConsoleStream::System, &format!("Could not start the debugger: {error}"));
            PROGRAM.with(|p| *p.borrow_mut() = None);
            return;
        }
    };
    SESSION.with(|slot| *slot.borrow_mut() = Some(session));
    view_state().update(|view| {
        *view = DebugView {
            doc: Some(id),
            state: Some(DebuggerState::Starting),
            pane: true,
            ..DebugView::default()
        };
    });
    // Signals are read out of the workspace and set outside `with`: a set
    // inside the borrow notifies subscribers that re-borrow it (panic).
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
            view_state().update(|view| {
                view.edit_error = None;
                match parent {
                // Fresh roots for the frame: expansion ids are index paths,
                // meaningless after a step, so they are dropped.
                None => {
                    view.variables = variables.roots.clone();
                    view.expanded.clear();
                    rebuild_flat(view);
                }
                // The subtree of a lazily expanded node replaces its
                // children; the node stays open.
                Some(id) => {
                    if let Some(node) = find_variable(&mut view.variables, &id) {
                        node.children = variables.roots.clone();
                    }
                    view.expanded.insert(id);
                    rebuild_flat(view);
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
    view_state().update(|view| {
        view.state = None;
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
        if super::preferences::settings().with(|p| p.debugger.hide_view_automatically) {
            view.pane = false;
        }
    });
    SESSION.with(|session| *session.borrow_mut() = None);
    PROGRAM.with(|p| *p.borrow_mut() = None);
    bump(id);
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

/// Rebuilds the flat display rows from the tree + expansion set.
fn rebuild_flat(view: &mut DebugView) {
    view.flat.clear();
    fn walk(view: &mut DebugView, var: &Variable, depth: usize) {
        let open = view.expanded.contains(&var.id);
        view.flat.push((var.clone(), depth));
        if open {
            for child in &var.children {
                walk(view, child, depth + 1);
            }
        }
    }
    let roots = std::mem::take(&mut view.variables);
    for var in &roots {
        walk(view, var, 0);
    }
    view.variables = roots;
}

/// Opens/closes a variable row: fetched children collapse locally, unfetched
/// (`expandable`) subtrees request `ExpandVariable` from the session.
fn toggle_variable(index: usize) {
    let (id, expandable, has_children) = {
        let view = view_state().get_untracked();
        let Some((var, _)) = view.flat.get(index) else { return };
        (var.id.clone(), var.expandable, !var.children.is_empty())
    };
    let expanded = view_state().get_untracked().expanded.contains(&id);
    if expanded {
        view_state().update(|view| {
            view.expanded.remove(&id);
            rebuild_flat(view);
        });
    } else if expandable && !has_children {
        if let Some(frame) = view_state().get_untracked().selected {
            send(SessionCommand::ExpandVariable { frame, node: id, depth: 0 });
        }
    } else {
        view_state().update(|view| {
            view.expanded.insert(id);
            rebuild_flat(view);
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

/// The stack/variables column inside the console (DESIGN 8.9). Shown while a
/// session is live on the active document and `pane` is on.
pub fn pane() -> impl IntoElement {
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
        .w(px(200.0))
        .min_w(px(140.0))
        .flex_shrink_0()
        .h_full()
        .flex_col()
        .bg(tokens::sidebar)
        .border_r_1()
        .border_color(tokens::sidebar_border)
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
        .child(list_header("Stack"))
        .child(
            virtual_list(
                move || view_state().with(|view| view.stack.len()),
                move |at| stack_row(at.index()),
            )
            .estimated_item_size(px(20.0))
            .flex_1()
            .min_h(px(40.0))
            .empty(
                text("No frames")
                    .text_xs()
                    .text_color(tokens::muted_foreground)
                    .p_2(),
            ),
        )
        .child(list_header("Variables"))
        .child(
            virtual_list(
                move || view_state().with(|view| view.flat.len()),
                move |at| variable_row(at.index()),
            )
            .estimated_item_size(px(20.0))
            .flex_1()
            .min_h(px(40.0))
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
            dynamic(move || {
                view_state().with(|v| {
                    view()
                        .flex_col()
                        .children(v.watches.iter().map(|watch| watch_row(watch.clone())))
                        .into_element()
                })
            }),
        )
        .child(list_header("Breakpoints"))
        .child(
            dynamic(move || {
                // The active document's breakpoints (CodeRunner's list),
                // not the last debugged doc's — text_rev subscribes edits.
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
        )
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
            view().w(px(80.0)).child(text_field(
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
        view.flat.get(index).map(|(var, depth)| {
            let label = if var.display_name.is_empty() { var.name.clone() } else { var.display_name.clone() };
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
