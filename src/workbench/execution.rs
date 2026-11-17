//! Per-document execution and the console. Every request is constructed from
//! the selected language definition and the editor's current buffer; the
//! output goes into the document's console model, stream-tagged.

use nsh_core::prelude::*;
use scratch_console_model::Stream as ConsoleStream;
use scratch_document::{Document, DocumentId};
use scratch_language::LanguageDef;
use scratch_runner::{OnBusy, RunEvent, RunMessage, RunRequest, Snapshot, Stream, RunStatus};
use scratch_ui_kit::motion;
use tokio::sync::mpsc;

use super::state::{self, with};

/// Constructs a run without reading the file on disk. This is the integration
/// seam for future run settings and debugger UI.
pub fn request_for(document: &Document, title: &str, source: String, language: &LanguageDef) -> RunRequest {
    let snapshot = Snapshot::Text {
        text: source,
        file_name: Some(title.to_owned()),
        directory: state::saved_path(document).and_then(|path| path.parent().map(|parent| parent.to_path_buf())),
    };
    let settings = super::run_settings::runner_settings(document);
    RunRequest::new(language, snapshot).with_settings(settings).with_owner(document.id.to_string(), OnBusy::Reject)
}

pub fn run_active() {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    let Some(source) = state::text(id) else { return; };
    let prepared = with(|workspace| {
        let doc = workspace.doc(id)?;
        let language = workspace.registry.as_ref()?.get(&doc.document.language)?;
        Some((request_for(&doc.document, &doc.title, source, language), workspace.runner.clone(), doc.status, doc.running))
    });
    let Some((request, runner, status, running)) = prepared else { state::notice("Choose a language before running."); return; };
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
        status.set("Rendered".into());
        running.set(false);
        changed(id);
        motion::animate_panel();
        state::display().update(|view| view.console_visible = true);
        with(|workspace| workspace.changed());
        return;
    }
    let (tx, rx) = mpsc::unbounded_channel();
    match runner.start(request, move |message| { let _ = tx.send(message); }) {
        Ok(handle) => {
            let run_id = handle.id();
            with(|workspace| {
                if let Some(doc) = workspace.doc_mut(id) {
                    doc.console.clear();
                    doc.annotations.clear_origin(scratch_diagnostics::Origin::Console);
                    doc.console_highlights.clear();
                    doc.handle = Some(handle);
                }
            });
            state::bump_diag_rev(id);
            status.set("Preparing…".into());
            running.set(true);
            changed(id);
            motion::animate_panel();
            state::display().update(|view| view.console_visible = true);
            with(|workspace| workspace.changed());
            // Detached: the menu bar rebuilds on `running` changes, disposing
            // the owner this menu dispatch ran under — an owned task would be
            // cancelled before its first poll.
            Owner::detached().run(|| spawn(pump(id, run_id, rx)));
        }
        Err(error) => state::notice(format!("Could not start: {error}")),
    }
}

pub fn stop_active() {
    if let Some(handle) = with(|workspace| workspace.active.and_then(|id| workspace.doc(id)?.handle.clone())) { handle.cancel(); }
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

async fn pump(id: DocumentId, run_id: scratch_runner::RunId, mut events: mpsc::UnboundedReceiver<RunMessage>) {
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
            RunEvent::Truncated { .. } => write(id, ConsoleStream::System, "[Output truncated by run limit]"),
            RunEvent::Finished(summary) => {
                with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.console.finish_streams(); } });
                // Whether the program produced anything, before the footer's
                // system lines are written; an empty console may close again.
                let produced_output = with(|workspace| workspace.doc(id).map(|doc| doc.console.line_count() > 0).unwrap_or(false));
                if let Some(message) = &summary.message { write(id, ConsoleStream::System, message); }
                if let RunStatus::Error(error) = &summary.status { write(id, ConsoleStream::Stderr, &error.to_string()); }
                // CodeRunner's footer: the end state, the exit status (or
                // signal) and the wall time.
                let detail = match &summary.status {
                    RunStatus::Exit(code) if *code != 0 => format!(" · Exit Status {code}"),
                    RunStatus::Signal(number) => format!(" · Terminated due to signal: {number}"),
                    RunStatus::CompileFailed { exit_code: Some(code), .. } => format!(" · Exit Status {code}"),
                    _ => String::new(),
                };
                let line = format!("{}{} · {:.2}s", summary.status_text(), detail, summary.total_wall.as_secs_f64());
                write(id, ConsoleStream::System, &line);
                finish(id, run_id, &line);
                import_diagnostics(id);
                // "Automatically Hide Console": a clean finish with no output
                // dismisses the console; output or a failure keeps it up.
                if !produced_output
                    && matches!(summary.status, RunStatus::Exit(0))
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
    finish(id, run_id, "Run disconnected");
}

pub(super) fn status(id: DocumentId, text: &str) {
    let status = with(|workspace| workspace.doc(id).map(|doc| doc.status));
    if let Some(status) = status { status.set(text.to_owned()); }
}

/// Imports the run's console output as editor annotations (DESIGN:
/// "Include errors from the console in the editor"): compiler errors,
/// Python tracebacks, rustc/go/clang/javac/node formats, placed in the
/// document's text by `scratch-diagnostics`.
fn import_diagnostics(id: DocumentId) {
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
fn finish(id: DocumentId, run_id: scratch_runner::RunId, text: &str) {
    let signals = with(|workspace| {
        let doc = workspace.doc_mut(id)?;
        if doc.handle.as_ref().map(|h| h.id()) == Some(run_id) { doc.handle = None; }
        Some((doc.status, doc.running))
    });
    if let Some((status, running)) = signals {
        status.set(text.to_owned());
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
