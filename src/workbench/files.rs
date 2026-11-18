//! File commands. Saving captures a revision and never marks a later edit clean.

use std::path::{Path, PathBuf};

use nsh_base::alert_dialog;
use nsh_core::file_dialog::{OpenDialog, SaveDialog};
use nsh_core::prelude::*;
use scratch_document::{Document, DocumentId, ExternalState, FileMeta, ReadOptions, Saved, check_external, load_text, now_ms, save_text};

use super::persistence::SavedDocument;
use super::state::{self, with};

pub fn open_dialog() {
    spawn(async move {
        match pick_paths(OpenDialog::files().multiple(true)).await {
            Ok(Some(paths)) => for path in paths { open_path(path).await; },
            Ok(None) => {},
            Err(error) => state::notice(format!("Could not open the file dialog: {error}")),
        }
    });
}

pub async fn open_path(path: PathBuf) {
    let existing = with(|workspace| workspace.documents.iter().find(|doc| state::saved_path(&doc.document).as_deref() == Some(&path)).map(|doc| doc.document.id));
    if let Some(id) = existing { state::activate(id); return; }
    let replace_candidate = if super::preferences::settings().with(|p| p.advanced.replace_untitled_document) {
        with(|workspace| {
            (workspace.documents.len() == 1).then(|| workspace.documents.first())
                .flatten()
                .filter(|doc| doc.document.untitled && doc.document.pristine)
                .map(|doc| doc.document.id)
        })
    } else {
        None
    };
    let source = path.clone();
    let loaded = match blocking(move || load_text(&source, &ReadOptions::default())).await {
        Ok(Ok(loaded)) => loaded,
        Ok(Err(error)) => { state::notice(format!("Could not open {}: {error}", path.display())); return; }
        Err(error) => { state::notice(format!("Could not open {}: {error}", path.display())); return; }
    };
    let saved = with(|workspace| {
        let registry = workspace.registry.as_ref()?;
        let document = Document::from_file(&path, &loaded, &FileMeta::default(), registry, now_ms());
        Some(SavedDocument { document, title: state::path_title(&path), text: loaded.text, diagnostics: None })
    });
    if let Some(saved) = saved {
        let id = state::add(saved);
        if let Some(old) = replace_candidate {
            state::remove(old);
        }
        state::focus(id);
    }
}

pub fn new_window() {
    super::native::spawn_window(&[]);
}

pub fn move_active_tab_to_new_window() {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    if display_tabs_len() < 2 {
        return;
    }
    let busy = with(|workspace| {
        workspace
            .doc(id)
            .map(|doc| (doc.saving, doc.running.get() || doc.handle.is_some()))
    });
    match busy {
        Some((true, _)) => {
            state::notice("Wait for the save to finish before moving this document to a new window.");
            return;
        }
        Some((_, true)) => {
            state::notice("Stop the running script before moving this document to a new window.");
            return;
        }
        None => return,
        _ => {}
    }
    super::native::spawn_detached_doc(id);
}

fn display_tabs_len() -> usize {
    super::state::display().with_untracked(|v| v.tabs.len())
}

/// Opens a file and places the caret on `line` (1-based), for search results
/// and `file:line` links.
pub async fn open_path_at(path: PathBuf, line: u32) {
    let target = nsh_base::EditorPosition::new(line.saturating_sub(1), 0);
    open_path_at_range(path, target, target).await;
}

/// Opens a file and selects `start..end` (or goes to `start.line` when at column 0).
pub async fn open_path_at_range(
    path: PathBuf,
    start: nsh_base::EditorPosition,
    end: nsh_base::EditorPosition,
) {
    state::record_jump();
    open_path(path.clone()).await;
    let Some(id) = with(|workspace| {
        let active = workspace.active?;
        let doc = workspace.doc(active)?;
        (state::saved_path(&doc.document).as_ref() == Some(&path)).then_some(active)
    }) else { return; };
    next_frame().await;
    next_frame().await;
    let still_same = with(|workspace| {
        workspace.active == Some(id)
            && workspace
                .doc(id)
                .is_some_and(|doc| state::saved_path(&doc.document).as_ref() == Some(&path))
    });
    if !still_same {
        return;
    }
    if let Some(api) = state::editor_api(id) {
        if start != end || start.column != 0 {
            api.set_selections(vec![nsh_base::EditorSelection::new(start, end)]);
        } else {
            api.go_to_line(start.line);
        }
        api.reveal_position(start, true);
    }
    state::focus(id);
}

pub fn save_active(save_as: bool) {
    if let Some(id) = with(|workspace| workspace.active) { save(id, save_as, false); }
}

pub fn save(id: DocumentId, save_as: bool, close_after: bool) {
    let available = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        if doc.saving { return false; }
        doc.saving = true; true
    });
    if available { spawn(save_task(id, save_as, close_after)); }
}

fn finish_saving(id: DocumentId) {
    with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.saving = false; } });
}

/// Writes an editor snapshot, refusing to overwrite a file changed externally.
fn write_document(path: &Path, text: &str, document: &Document, replacing_original: bool) -> Result<Saved, String> {
    if replacing_original {
        if let Some(stamp) = &document.stamp {
            match check_external(path, stamp).map_err(|e| e.to_string())? {
                ExternalState::Unchanged | ExternalState::Touched(_) => {},
                ExternalState::Modified(_) | ExternalState::Deleted => return Err("The file changed outside ScratchRunner. Use Save As to keep your edits without overwriting it.".into()),
            }
        } else if path.exists() {
            return Err("The original file's state is unknown. Use Save As to keep your edits without overwriting it.".into());
        }
    }
    save_text(path, text, &document.save_options()).map_err(|error| error.to_string())
}

async fn save_task(id: DocumentId, save_as: bool, close_after: bool) {
    let Some((original, title)) = with(|workspace| workspace.doc(id).map(|doc| (state::saved_path(&doc.document), doc.title.clone()))) else { return; };
    let target = if !save_as && original.is_some() {
        original.clone().unwrap()
    } else {
        match pick_save_path(SaveDialog::new().file_name(title)).await {
            Ok(Some(path)) => path,
            Ok(None) => { finish_saving(id); return; },
            Err(error) => { finish_saving(id); state::notice(format!("Could not open the save dialog: {error}")); return; }
        }
    };
    let Some(text) = state::text(id) else { finish_saving(id); return; };
    let Some(document) = with(|workspace| workspace.doc(id).map(|doc| doc.document.clone())) else { return; };
    let revision = document.revision;
    let replacing_original = original.as_deref() == Some(target.as_path());
    let destination = target.clone();
    let result = blocking(move || write_document(&destination, &text, &document, replacing_original)).await;
    let saved = match result {
        Ok(Ok(saved)) => saved,
        Ok(Err(error)) => { finish_saving(id); state::notice(format!("Could not save {}: {error}", target.display())); return; }
        Err(error) => { finish_saving(id); state::notice(format!("Could not save {}: {error}", target.display())); return; }
    };
    let clean = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        doc.document.relocate(target.clone());
        doc.title = state::path_title(&target);
        let clean = doc.document.revision == revision;
        if clean { doc.document.mark_saved(&saved, now_ms()); } else { doc.document.stamp = Some(saved.stamp); }
        doc.saving = false;
        workspace.changed();
        clean
    });
    // Save As may land on a new extension: detection follows it unless the
    // user chose the language (`state::redetect` skips a locked document).
    if state::redetect(id) {
        state::bump_text_rev(id);
        super::lsp::document_closed(id);
        super::lsp::document_opened(id);
    }
    state::publish();
    if close_after && clean { state::remove(id); }
    else if close_after { state::notice("The document changed while it was saving. Save again before closing."); }
    else { state::notice(if clean { "Saved." } else { "Saved an earlier version; newer edits remain unsaved." }); }
}

/// "Revert to Saved": reloads the file-backed document's text from disk,
/// discarding unsaved edits (no-op for scratch documents).
pub fn revert_active() {
    let found = with(|workspace| {
        let id = workspace.active?;
        let doc = workspace.doc(id)?;
        Some((id, state::saved_path(&doc.document)?))
    });
    let Some((id, path)) = found else {
        state::notice("The document has no saved file to revert to.");
        return;
    };
    spawn(async move {
        let source = path.clone();
        let loaded = match blocking(move || load_text(&source, &ReadOptions::default())).await {
            Ok(Ok(loaded)) => loaded,
            Ok(Err(error)) => {
                state::notice(format!("Could not revert {}: {error}", path.display()));
                return;
            }
            Err(error) => {
                state::notice(format!("Could not revert {}: {error}", path.display()));
                return;
            }
        };
        let text = loaded.text.clone();
        state::with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                doc.document.apply_loaded(&loaded);
                doc.text = text.clone();
                workspace.changed();
            }
        });
        if let Some(api) = state::editor_api(id) { api.set_value(text); }
        state::bump_text_rev(id);
        state::publish();
        super::lsp::document_changed(id);
        state::notice("Reverted to the saved file.");
    });
}

/// Updates any open document referencing `from` after an external rename to `to`.
pub(super) fn relocate_open_path(from: &std::path::Path, to: &std::path::Path) {
    let changed = with(|workspace| {
        let mut any = false;
        for doc in &mut workspace.documents {
            if state::saved_path(&doc.document).as_deref() == Some(from) {
                doc.document.relocate(to.to_path_buf());
                doc.title = state::path_title(to);
                any = true;
            }
        }
        if any {
            workspace.changed();
        }
        any
    });
    if changed {
        state::publish();
    }
}

/// Reloads an already-open, unmodified document from disk after an external
/// edit (such as Project Replace or Undo Replace).
pub(super) async fn reload_path_if_clean(path: PathBuf) {
    let target = with(|workspace| {
        workspace
            .documents
            .iter()
            .find(|doc| !doc.document.dirty && state::saved_path(&doc.document).as_ref() == Some(&path))
            .map(|doc| doc.document.id)
    });
    let Some(id) = target else { return; };
    let source = path.clone();
    let Ok(Ok(loaded)) = blocking(move || load_text(&source, &ReadOptions::default())).await else { return; };
    let text = loaded.text.clone();
    let applied = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        if doc.document.dirty || state::saved_path(&doc.document).as_ref() != Some(&path) {
            return false;
        }
        doc.document.apply_loaded(&loaded);
        doc.text = text.clone();
        workspace.changed();
        true
    });
    if !applied {
        return;
    }
    if let Some(api) = state::editor_api(id) { api.set_value(text); }
    state::bump_text_rev(id);
    state::publish();
    super::lsp::document_changed(id);
}

/// "Close Other Tabs" (⌥⌘W): closes every document except the active one,
/// asking about unsaved changes per tab like a single close does.
pub fn close_other_tabs() {
    let others = with(|workspace| {
        workspace
            .documents
            .iter()
            .filter(|doc| Some(doc.document.id) != workspace.active)
            .map(|doc| doc.document.id)
            .collect::<Vec<_>>()
    });
    for id in others { close(id); }
}

pub fn close_active() {
    if let Some(id) = with(|workspace| workspace.active) { close(id); }
}

pub fn close(id: DocumentId) {
    let Some((dirty, title, saving, running)) = with(|workspace| {
        workspace.doc(id).map(|doc| (doc.document.dirty, doc.title.clone(), doc.saving, doc.running.get()))
    }) else { return; };
    if saving { state::notice("Wait for the save to finish before closing this document."); return; }
    if running {
        // CodeRunner confirms before killing a document's run.
        Owner::detached().run(|| spawn(async move {
            let response = alert_dialog()
                .title(format!("“{title}” is currently running."))
                .description("Are you sure you want to close it?")
                .destructive("close", "Close")
                .cancel("cancel", "Cancel")
                .show().await;
            match response {
                Ok(response) if response.action.as_deref() == Some("close") => {
                    with(|workspace| { if let Some(doc) = workspace.doc_mut(id) {
                        if let Some(handle) = doc.handle.take() { handle.cancel(); }
                        doc.running.set(false);
                    } });
                    close(id);
                }
                Ok(_) => {},
                Err(error) => state::notice(format!("Could not confirm closing: {error}")),
            }
        }));
        return;
    }
    if !dirty { state::remove(id); return; }
    spawn(async move {
        let response = alert_dialog()
            .title(format!("Save changes to “{title}”?"))
            .description("Your changes will be lost if you don't save them.")
            .action("save", "Save")
            .destructive("discard", "Don't Save")
            .cancel("cancel", "Cancel")
            .show().await;
        match response {
            Ok(response) if response.action.as_deref() == Some("save") => save(id, false, true),
            Ok(response) if response.action.as_deref() == Some("discard") => state::remove(id),
            Ok(_) => {},
            Err(error) => state::notice(format!("Could not confirm closing: {error}")),
        }
    });
}

/// File > Print… — the active document's text rendered as light syntax-colored
/// HTML through the system print panel (CodeRunner prints the source listing, ⌘P).
pub fn print_active() {
    let Some(id) = state::with(|workspace| workspace.active) else { return };
    let title = state::with(|workspace| workspace.doc(id).map(|doc| doc.title.clone()))
        .unwrap_or_else(|| "Untitled".to_owned());
    if let Some((html, plain, margins)) = super::theme::print_html(id, &title) {
        super::native::print_colored_document(&title, &html, &plain, margins);
    } else if let Some(text) = state::text(id) {
        let html = format!("<pre style=\"font-family:Menlo,monospace;font-size:8pt\">{}</pre>",
            text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
        super::native::print_colored_document(&title, &html, &text, (40.0, 40.0, 40.0, 40.0));
    }
}

/// File > New from Clipboard: the clipboard's text as a new document in
/// the language the text looks like (the default language when unsure).
pub fn new_from_clipboard() {
    spawn(async {
        let text = clipboard_string().await;
        if text.is_empty() {
            state::notice("The clipboard has no text.");
            return;
        }
        // The paste guesses a language (shebang, modeline, content patterns).
        let language = with(|workspace| {
            workspace.registry.as_ref()
                .and_then(|registry| registry.guess_from_text(&text))
                .map(|guess| guess.language.id.clone())
        });
        state::new_document_with(language.as_deref(), text);
    });
}

/// File > Duplicate: the active document's language and text as a new
/// unsaved document.
pub fn duplicate_active() {
    let found = with(|workspace| {
        let id = workspace.active?;
        let language = workspace.doc(id)?.document.language.clone();
        Some((id, language))
    });
    let Some((id, language)) = found else { return; };
    let Some(text) = state::text(id) else { return; };
    state::new_document_with(Some(&language), text);
}

/// File > Close Document Without Saving: drops the tab and its unsaved
/// edits without asking.
pub fn close_active_without_saving() {
    if let Some(id) = with(|workspace| workspace.active) { state::remove(id); }
}

/// File > Go to Current Document: reveals the file-backed document in the
/// navigator (scratch documents have no place to show).
pub fn go_to_current_document() {
    let found = with(|workspace| {
        let id = workspace.active?;
        let doc = workspace.doc(id)?;
        state::saved_path(&doc.document)
    });
    match found {
        Some(path) => super::navigator::reveal(&path),
        None => state::notice("The document has no file to show."),
    }
}

/// File > Rename…/Move To…: the save panel picks the file-backed
/// document's new path; the file is renamed and the document relocated.
fn relocate_active(verb: &'static str, past: &'static str) {
    let found = with(|workspace| {
        let id = workspace.active?;
        let doc = workspace.doc(id)?;
        Some((id, state::saved_path(&doc.document)?, doc.title.clone()))
    });
    let Some((id, path, title)) = found else {
        state::notice("Only a saved file can be renamed or moved.");
        return;
    };
    spawn(async move {
        let mut dialog = SaveDialog::new().title(verb).button_label(verb).file_name(title);
        if let Some(dir) = path.parent() { dialog = dialog.directory(dir.to_path_buf()); }
        let target = match pick_save_path(dialog).await {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => { state::notice(format!("Could not open the panel: {error}")); return; }
        };
        if target == path { return; }
        if target.exists() {
            state::notice(format!("A file already exists at {}.", target.display()));
            return;
        }
        let (from, to) = (path.clone(), target.clone());
        match blocking(move || std::fs::rename(&from, &to)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => { state::notice(format!("Could not {verb} the file: {error}")); return; }
            Err(error) => { state::notice(format!("Could not {verb} the file: {error}")); return; }
        }
        with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                doc.document.relocate(target.clone());
                doc.title = state::path_title(&target);
                workspace.changed();
            }
        });
        // The new name may claim another extension: detection follows it
        // unless the user chose the language.
        if state::redetect(id) {
            state::bump_text_rev(id);
            super::lsp::document_closed(id);
            super::lsp::document_opened(id);
        }
        state::publish();
        state::notice(format!("{past} {}.", target.display()));
    });
}

pub fn rename_active() { relocate_active("Rename", "Renamed to"); }

pub fn move_active() { relocate_active("Move To", "Moved to"); }

/// File > Share — the named service gets the document's file URL, or its
/// text while unsaved.
pub fn share_active(service: &'static str) {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    let Some(text) = state::text(id) else { return; };
    let path = with(|workspace| workspace.doc(id).and_then(|doc| state::saved_path(&doc.document)))
        .map(|path| path.to_string_lossy().into_owned());
    super::native::share(service, path, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use scratch_document::{FileMeta, LineEnding};
    use scratch_language::LanguageRegistry;

    #[test]
    fn saves_edits_preserving_line_endings_and_refuses_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.py");
        std::fs::write(&path, b"one\r\n").unwrap();
        let file = load_text(&path, &ReadOptions::default()).unwrap();
        let doc = Document::from_file(&path, &file, &FileMeta::default(), &LanguageRegistry::builtin(), now_ms());
        assert_eq!(doc.line_ending, LineEnding::CrLf);
        write_document(&path, "two\n", &doc, true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two\r\n");
        assert!(write_document(&path, "stale\n", &doc, true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"two\r\n");
    }
}
