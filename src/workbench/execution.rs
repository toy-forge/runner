//! Per-document execution and the console. Every request is constructed from
//! the selected language definition and the editor's current buffer; the
//! output goes into the document's console model, stream-tagged.

use std::time::Duration;

use nsh_core::prelude::*;
use scratch_console_model::Stream as ConsoleStream;
use scratch_document::{Document, DocumentId};
use scratch_language::LanguageDef;
use scratch_runner::{
    OnBusy, PathSetting, RunError, RunEvent, RunHandle, RunMessage, RunMode, RunRequest,
    RunStatus, ShellSettings, Snapshot, Stream,
};
use scratch_ui_kit::motion;
use tokio::sync::mpsc;

use super::state::{self, RunOutcome, with};

/// Global shell & advanced build preferences for runner requests.
pub(super) fn current_shell_settings() -> ShellSettings {
    super::preferences::settings().with_untracked(|p| ShellSettings {
        shell: p.shell.shell.clone(),
        login_shell: p.shell.run_as_login_shell,
        path: if p.shell.automatically_determine_path {
            PathSetting::Automatic
        } else {
            PathSetting::Custom(p.shell.path.clone())
        },
        variables: p.shell.environment.clone(),
        shell_arguments: p.shell.arguments.clone(),
        compile_shell_arguments: p.shell.compile_arguments.clone(),
        compiler_output_in_temp_dir: p.advanced.write_compiler_output_to_temp_dir,
        ..ShellSettings::default()
    })
}

/// Constructs a run without reading the file on disk. Kept free of NSH global
/// state so unit tests can invoke it directly.
pub fn request_for(document: &Document, title: &str, source: String, language: &LanguageDef) -> RunRequest {
    let snapshot = Snapshot::Text {
        text: source,
        file_name: Some(title.to_owned()),
        directory: state::saved_path(document).and_then(|path| path.parent().map(|parent| parent.to_path_buf())),
    };
    let settings = super::run_settings::runner_settings(document);
    RunRequest::new(language, snapshot).with_settings(settings).with_owner(document.id.to_string(), OnBusy::Reject)
}

fn language_has_debugger(workspace: &super::state::Workspace, language_id: &str) -> bool {
    let adapters = scratch_debugger::AdapterRegistry::builtin();
    let preferred = workspace
        .registry
        .as_ref()
        .and_then(|r| r.get(language_id))
        .and_then(|def| def.debugger.as_deref());
    preferred
        .and_then(|id| adapters.get(id))
        .or_else(|| adapters.for_language(language_id))
        .is_some()
}

/// Primary Run (`⌘R`): automatically enters debugger if enabled breakpoints
/// exist and a debugger adapter is registered for the active language.
pub fn run_active() {
    let should_debug = with(|workspace| {
        let id = workspace.active?;
        let doc = workspace.doc(id)?;
        let has_bp = doc.document.breakpoints.as_slice().iter().any(|bp| bp.enabled);
        let has_dbg = language_has_debugger(workspace, &doc.document.language);
        Some(has_bp && has_dbg)
    }).unwrap_or(false);
    if should_debug {
        super::debugger::run_in_debugger();
    } else {
        run_with_mode(RunMode::Run, None);
    }
}

/// Run Without Debugging (`⌃⌘R`): always executes without attaching debugger.
pub fn run_without_debugging() {
    run_with_mode(RunMode::RunWithoutDebugging, None);
}

/// Compile or Run Diagnostics (`⌘B`): runs CompileOnly for compiled languages,
/// otherwise executes manual diagnostics jobs.
pub fn compile_or_diagnostics() {
    let compiles = with(|workspace| {
        workspace.active
            .and_then(|id| workspace.doc(id))
            .and_then(|doc| workspace.registry.as_ref().and_then(|r| r.get(&doc.document.language)))
            .is_some_and(|def| def.has_compile_step())
    });
    if compiles {
        run_with_mode(RunMode::CompileOnly, None);
    } else {
        super::diagnostics::run_diagnostics();
    }
}

/// Run Selection (`⇧⌃⌘R`): executes the selected text (or current line).
pub fn run_selection() {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let full = api.value();
    let selections = api.selections();
    let selected = if let Some(sel) = selections.last() {
        let lines: Vec<&str> = full.split('\n').collect();
        if sel.anchor == sel.head {
            lines.get(sel.head.line as usize).copied().unwrap_or_default().to_owned()
        } else {
            let (start, end) = if (sel.anchor.line, sel.anchor.column) <= (sel.head.line, sel.head.column) {
                (sel.anchor, sel.head)
            } else {
                (sel.head, sel.anchor)
            };
            let offset_of = |pos: nsh_base::EditorPosition| -> usize {
                let mut offset = 0usize;
                for (i, line) in full.split('\n').enumerate() {
                    if i == pos.line as usize {
                        let bytes = scratch_search::utf16_to_byte(line, pos.column as usize);
                        return (offset + bytes).min(full.len());
                    }
                    offset += line.len() + 1;
                }
                full.len()
            };
            let a = offset_of(start);
            let b = offset_of(end);
            full[a.min(b)..a.max(b)].to_owned()
        }
    } else {
        full
    };
    if selected.trim().is_empty() {
        state::notice("Select code to run.");
        return;
    }
    run_with_mode(RunMode::RunWithoutDebugging, Some(selected));
}

fn run_with_mode(mode: RunMode, override_source: Option<String>) {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    if super::debugger::debugging(id) {
        super::debugger::stop();
    }
    let source = match override_source {
        Some(text) => text,
        None => match state::text(id) {
            Some(text) => text,
            None => return,
        },
    };
    let prepared = with(|workspace| {
        let doc = workspace.doc(id)?;
        let language = workspace.registry.as_ref()?.get(&doc.document.language)?.clone();
        let request = request_for(&doc.document, &doc.title, source, &language)
            .with_mode(mode)
            .with_shell(current_shell_settings());
        Some((request, language, doc.title.clone(), workspace.runner.clone(), doc.status, doc.run_stats, doc.exit_info, doc.running))
    });
    let Some((request, language, title, runner, status, run_stats, exit_info, running)) = prepared else {
        state::notice("Choose a language before running.");
        return;
    };
    if request.language.run.command.trim().is_empty() && request.language.console.renderer.is_none() {
        state::notice("This language has no run command.");
        return;
    }
    // In-process renderers (Markdown's Web console) replace the run: the
    // preview renders the source, so "running" just refreshes the console.
    if request.language.run.command.trim().is_empty() && request.language.console.renderer.is_some() {
        with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                doc.console.clear();
                doc.console.write_line(ConsoleStream::System, "Rendered.");
            }
        });
        run_stats.set(None);
        exit_info.set(None);
        status.set("Rendered".into());
        running.set(false);
        changed(id);
        motion::animate_panel();
        state::display().update(|view| view.console_visible = true);
        with(|workspace| workspace.changed());
        return;
    }
    if mode != RunMode::CompileOnly {
        super::diagnostics::on_run(id);
    }
    let (tx, rx) = mpsc::unbounded_channel();
    match runner.start(request, move |message| { let _ = tx.send(message); }) {
        Ok(handle) => {
            let run_id = handle.id();
            let handle_for_stats = handle.clone();
            with(|workspace| {
                if let Some(doc) = workspace.doc_mut(id) {
                    doc.console.clear();
                    doc.annotations.clear_origin(scratch_diagnostics::Origin::Console);
                    doc.console_highlights.clear();
                    doc.handle = Some(handle);
                }
            });
            state::bump_diag_rev(id);
            run_stats.set(None);
            exit_info.set(None);
            status.set(if mode == RunMode::CompileOnly { "Compiling…" } else { "Preparing…" }.into());
            running.set(true);
            changed(id);
            motion::animate_panel();
            state::display().update(|view| view.console_visible = true);
            with(|workspace| workspace.changed());
            // Detached: the menu bar rebuilds on `running` changes, disposing
            // the owner this menu dispatch ran under — an owned task would be
            // cancelled before its first poll.
            Owner::detached().run(|| {
                spawn(pump(id, run_id, title, language.install_hint(), rx));
                spawn(sample_stats_loop(id, run_id, run_stats, handle_for_stats));
            });
        }
        Err(error) => state::notice(format!("Could not start: {error}")),
    }
}

async fn sample_stats_loop(
    id: DocumentId,
    run_id: scratch_runner::RunId,
    run_stats: Signal<Option<(f32, u64)>>,
    handle: RunHandle,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let still_current = with(|workspace| {
            workspace.doc(id).is_some_and(|doc| {
                doc.handle.as_ref().is_some_and(|h| h.id() == run_id) && doc.running.get_untracked()
            })
        });
        if !still_current {
            run_stats.set(None);
            return;
        }
        let sampled = blocking({
            let handle = handle.clone();
            move || handle.sample_stats()
        }).await.ok().flatten();
        if let Some(stats) = sampled {
            run_stats.set(Some((stats.cpu_percent, stats.rss_bytes)));
        }
    }
}

pub fn stop_active() {
    if let Some(handle) = with(|workspace| workspace.active.and_then(|id| workspace.doc(id)?.handle.clone())) {
        handle.cancel();
    }
    if with(|workspace| workspace.active).is_some_and(super::debugger::debugging) {
        super::debugger::stop();
    }
}

/// Sends one line of input to the active document's running process, and
/// shows it in the console as input.
pub fn send_input(line: &str) {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    let handle = with(|workspace| workspace.doc(id).and_then(|doc| doc.handle.clone()));
    match handle {
        Some(handle) => {
            let mut bytes = line.as_bytes().to_vec();
            bytes.push(b'\n');
            if let Err(error) = handle.write_stdin(&bytes) {
                state::notice(format!("Could not send input: {error}"));
                return;
            }
            with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.console.write_line(ConsoleStream::Input, line); } });
            changed(id);
        }
        None => state::notice("Run a document before sending input."),
    }
}

/// Empties the active document's console.
pub fn clear_active() {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.console.clear();
            doc.annotations.clear_origin(scratch_diagnostics::Origin::Console);
            doc.console_highlights.clear();
        }
    });
    state::bump_diag_rev(id);
    changed(id);
}

/// Tells the console view that a document's output changed.
fn changed(id: DocumentId) {
    let rev = with(|workspace| workspace.doc(id).map(|doc| doc.console_rev));
    if let Some(rev) = rev { rev.update(|value| *value = value.wrapping_add(1)); }
}

/// Writes one message into the document's console, then redraws it.
fn write(id: DocumentId, stream: ConsoleStream, text: &str) {
    with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.console.write_line(stream, text); } });
    changed(id);
}

async fn pump(
    id: DocumentId,
    run_id: scratch_runner::RunId,
    title: String,
    install_hint: Option<String>,
    mut events: mpsc::UnboundedReceiver<RunMessage>,
) {
    let mut prompted_large_output = false;
    'outer: while let Some(message) = events.recv().await {
        let current = with(|workspace| workspace.doc(id).is_some_and(|doc| doc.handle.as_ref().map(|h| h.id()) == Some(run_id)));
        if !current { return; }
        // A flooding child can queue thousands of output chunks behind a
        // busy engine; draining them into one batch keeps the redraw cost
        // (one `changed` bump per batch) off the per-chunk path.
        let mut batch = vec![message];
        while batch.len() < 256 {
            match events.try_recv() { Ok(next) => batch.push(next), Err(_) => break }
        }
        let mut dirty = false;
        for message in batch {
        match message.event {
            RunEvent::Output { stream, bytes, .. } => {
                let stream = match stream { Stream::Stdout => ConsoleStream::Stdout, Stream::Stderr => ConsoleStream::Stderr };
                with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.console.append(stream, &bytes); } });
                dirty = true;
            }
            RunEvent::Preparing => status(id, "Preparing…"),
            RunEvent::StageStarted(stage) => status(id, &format!("{}…", stage.name)),
            RunEvent::Started(_) => status(id, "Running…"),
            RunEvent::LargeOutput { bytes } => {
                if !prompted_large_output {
                    prompted_large_output = true;
                    let mb = bytes as f64 / (1024.0 * 1024.0);
                    let handle = with(|workspace| workspace.doc(id).and_then(|d| d.handle.clone()));
                    if let Some(handle) = handle {
                        let alert_title = title.clone();
                        Owner::detached().run(move || spawn(async move {
                            let choice = nsh_base::alert_dialog()
                                .title("Large Output Warning")
                                .description(format!(
                                    "\"{alert_title}\" has produced {mb:.1} MB of output. Continue or stop the run?"
                                ))
                                .action("continue", "Continue")
                                .cancel("stop", "Stop")
                                .show()
                                .await;
                            if choice.ok().and_then(|r| r.action).as_deref() == Some("continue") {
                                handle.allow_large_output();
                            } else {
                                handle.cancel();
                            }
                        }));
                    }
                }
            }
            RunEvent::Compiled(program) => {
                let target = program.compiler.as_deref().unwrap_or("compiled binary");
                write(id, ConsoleStream::System, &format!("Compiled: {target}"));
            }
            RunEvent::Truncated { .. } => write(id, ConsoleStream::System, "[Output truncated by run limit]"),
            RunEvent::Finished(summary) => {
                with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.console.finish_streams(); } });
                // Whether the program produced anything, before the footer's
                // system lines are written; an empty console may close again.
                let produced_output = with(|workspace| workspace.doc(id).map(|doc| doc.console.line_count() > 0).unwrap_or(false));
                if let Some(message) = &summary.message { write(id, ConsoleStream::System, message); }
                if let RunStatus::Error(error) = &summary.status { write(id, ConsoleStream::Stderr, &error.to_string()); }
                let exit_code = match &summary.status {
                    RunStatus::Exit(code) => Some(*code),
                    RunStatus::CompileFailed { exit_code, .. } => *exit_code,
                    RunStatus::Compiled => Some(0),
                    _ => None,
                };
                let missing_tool = exit_code == Some(127)
                    || matches!(summary.status, RunStatus::Error(RunError::MissingTool { .. } | RunError::DeveloperToolsMissing { .. } | RunError::PythonMissing { .. }));
                if missing_tool {
                    if let Some(hint) = &install_hint {
                        if summary.message.as_ref().is_none_or(|m| !m.contains(hint)) {
                            write(id, ConsoleStream::System, hint);
                        }
                        state::notice(hint.clone());
                    }
                }
                // CodeRunner's footer: the end state, the exit status (or
                // signal) and the wall time.
                let detail = match &summary.status {
                    RunStatus::Exit(code) if *code != 0 => format!(" · Exit Status {code}"),
                    RunStatus::Signal(number) => format!(" · Terminated due to signal: {number}"),
                    RunStatus::CompileFailed { exit_code: Some(code), .. } => format!(" · Exit Status {code}"),
                    _ => String::new(),
                };
                let duration_secs = summary.total_wall.as_secs_f64();
                let line = format!("{}{} · {:.2}s", summary.status_text(), detail, duration_secs);
                write(id, ConsoleStream::System, &line);
                let outcome = RunOutcome {
                    code: exit_code,
                    duration_secs,
                    canceled: matches!(summary.status, RunStatus::Cancelled),
                    command_not_found: missing_tool,
                };
                finish(id, run_id, &line, Some(outcome));
                import_diagnostics(id);
                if !state::display().with_untracked(|v| v.window_focused) {
                    state::notice(format!("{title}: {line}"));
                }
                // "Automatically Hide Console": a clean finish with no output
                // dismisses the console; output or a failure keeps it up.
                if !produced_output
                    && matches!(summary.status, RunStatus::Exit(0) | RunStatus::Compiled)
                    && super::preferences::settings().with_untracked(|p| p.general.console.hide_automatically)
                {
                    motion::animate_panel();
                    state::display().update(|view| view.console_visible = false);
                }
                if dirty { changed(id); }
                return;
            }
            _ => {}
        }
        }
        if dirty { changed(id); }
        continue 'outer;
    }
    finish(id, run_id, "Run disconnected", None);
}

pub(super) fn status(id: DocumentId, text: &str) {
    let status = with(|workspace| workspace.doc(id).map(|doc| doc.status));
    if let Some(status) = status { status.set(text.to_owned()); }
}

/// Imports the run's console output as editor annotations (DESIGN:
/// "Include errors from the console in the editor"): compiler errors,
/// Python tracebacks, rustc/go/clang/javac/node formats, placed in the
/// document's text by `scratch-diagnostics`.
pub(super) fn import_diagnostics(id: DocumentId) {
    use scratch_diagnostics::{DocumentContext, console};
    let parsed = with(|workspace| {
        let doc = workspace.doc(id)?;
        // Keep each line's byte start in the joined text so console-wide
        // match ranges map back to per-line ranges for highlighting.
        let mut output = String::new();
        let mut starts = Vec::new();
        for index in 0..doc.console.line_count() {
            let Some(line) = doc.console.line(index) else { continue };
            starts.push((index, output.len(), line.text().len()));
            output.push_str(line.text());
            output.push('\n');
        }
        let language = workspace.registry.as_ref().and_then(|registry| registry.get(&doc.document.language)).cloned();
        Some((console::import_console(&output, language.as_ref()), starts, doc.document.language.clone(), doc.text.clone(), state::saved_path(&doc.document)))
    });
    let Some((import, starts, language_id, text, path)) = parsed else { return; };
    if import.diagnostics.is_empty() && import.highlights.is_empty() { return; }
    let prefs = super::preferences::settings().with_untracked(|prefs| {
        let diagnostics = &prefs.text_editing.diagnostics;
        let key = format!("{language_id}/console");
        // Diagnostics > Console Text: "Disabled" and "Console Highlight
        // Only" both keep the matches out of the editor.
        let off = !diagnostics.from_console
            || diagnostics.disabled_diagnostics.contains(&key)
            || diagnostics.console_highlight_only.contains(&key);
        scratch_diagnostics::DiagnosticsPrefs { from_console: !off, ..Default::default() }
    });
    let rev = with(|workspace| {
        let doc = workspace.doc_mut(id)?;
        let mut context = DocumentContext::new(&text);
        if let Some(path) = &path { context = context.path(path); }
        console::import_into(&mut doc.annotations, &context, &import, &prefs);
        doc.console_highlights.clear();
        for matched in &import.highlights {
            // The line whose text contains `matched`'s start, then forward
            // while the match spills over the joined newlines.
            let Some(slot) = starts.iter().rposition(|(_, start, _)| *start <= matched.range.start) else { continue };
            for &(index, start, len) in &starts[slot..] {
                let line_end = start + len;
                if matched.range.end <= start { break; }
                let local = matched.range.start.saturating_sub(start);
                let end = matched.range.end.min(line_end);
                if local < end {
                    doc.console_highlights.entry(index).or_default().push(local..end);
                }
                if matched.range.end <= line_end { break; }
            }
        }
        Some((doc.console_rev, doc.diag_rev))
    });
    if let Some((console_rev, diag_rev)) = rev {
        console_rev.update(|v| *v = v.wrapping_add(1));
        diag_rev.update(|v| *v = v.wrapping_add(1));
    }
}

/// Ends a run: clears its handle and the running state, and sets the status.
fn finish(id: DocumentId, run_id: scratch_runner::RunId, text: &str, outcome: Option<RunOutcome>) {
    let signals = with(|workspace| {
        let doc = workspace.doc_mut(id)?;
        if doc.handle.as_ref().map(|h| h.id()) == Some(run_id) { doc.handle = None; }
        Some((doc.status, doc.run_stats, doc.exit_info, doc.running))
    });
    if let Some((status, run_stats, exit_info, running)) = signals {
        status.set(text.to_owned());
        run_stats.set(None);
        exit_info.set(outcome);
        running.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scratch_language::LanguageRegistry;
    use scratch_runner::{PathSetting, RunStatus, ShellSettings};
    use std::time::Duration;

    #[test]
    fn run_request_executes_the_edited_snapshot_not_the_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sh");
        std::fs::write(&path, "printf old").unwrap();
        let document = Document { untitled: false, path: Some(path), language: "shell".into(), ..Document::default() };
        let registry = LanguageRegistry::builtin();
        let language = registry.get("shell").unwrap();
        let mut request = request_for(&document, "script.sh", "printf edited".into(), language);
        request.shell = ShellSettings { path: PathSetting::Custom("/usr/bin:/bin".into()), ..ShellSettings::default() };
        let (_, rx) = scratch_runner::Runner::new().start_channel(request).unwrap();
        let mut output = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).unwrap().event {
                RunEvent::Output { bytes, .. } => output.extend(bytes),
                RunEvent::Finished(summary) => { assert_eq!(summary.status, RunStatus::Exit(0)); break; },
                _ => {},
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("edited"));
        assert_eq!(std::fs::read_to_string(dir.path().join("script.sh")).unwrap(), "printf old");
    }
}
