//! The Diagnostics menu: run language diagnostics on demand, navigate
//! annotations, apply fixes and edit the document's ignored messages.
//!
//! "Run Diagnostics" (⌘B) runs every enabled job-type diagnostic of the
//! active document's language through [`run_job`] and replaces each
//! diagnostic's `Origin::Document` annotation source — the same merge the
//! console import uses, so the editor keeps one annotation set.
//! Navigation items merge the document's annotations with the LSP
//! diagnostics the workbench keeps per document.

use std::sync::Arc;

use nsh_base::editor::EditorEdit;
use nsh_base::{EditorPosition, EditorRange, EditorSelection, dialog};
use nsh_core::element::desktop::MenuItem;
use scratch_diagnostics::settings::DiagnosticsPrefs as EnginePrefs;
use scratch_diagnostics::{
    CancelToken, DiagnosticJob, DocumentDiagnosticsData, LineIndex, Origin, RuleList, Severity,
    TextPos, entries_for, run_job,
};
use scratch_document::DocumentId;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::state;

// ---------------------------------------------------------------- Prefs

/// The engine's settings built from the app preferences.
fn engine_prefs() -> EnginePrefs {
    let prefs = super::preferences::settings().get_untracked();
    let diagnostics = &prefs.text_editing.diagnostics;
    EnginePrefs {
        while_typing: diagnostics.while_typing,
        delay_secs: diagnostics.delay,
        invalid_syntax: diagnostics.invalid_syntax,
        invalid_syntax_limit_to_one: diagnostics.invalid_syntax_limit_to_one,
        from_console: diagnostics.from_console,
        animate: diagnostics.animate,
        highlight_line: diagnostics.highlight_line,
        underline: diagnostics.underline,
        include_in_symbol_list: diagnostics.symbol_list_includes_annotations,
        icon_only: diagnostics.icon_only,
        ignored: RuleList::from_text(&diagnostics.ignored_diagnostics),
        auto_fix: RuleList::from_text(&diagnostics.auto_fix_diagnostics),
        disabled_diagnostics: diagnostics.disabled_diagnostics.clone(),
        console_highlight_only: diagnostics.console_highlight_only.clone(),
        update_overrides: diagnostics
            .update_overrides
            .iter()
            .map(|(key, bits)| (key.clone(), u8::try_from(*bits).unwrap_or(0)))
            .collect(),
        ..EnginePrefs::default()
    }
}

/// The console-text diagnostic's settings key for `language`.
fn console_key(language: &str) -> String {
    format!("{language}/console")
}

// ---------------------------------------------------------------- Run

/// "Run Diagnostics" (⌘B): runs the active language's job diagnostics now.
pub fn run_diagnostics() {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let found = state::with(|workspace| {
        let doc = workspace.doc(id)?;
        let registry = workspace.registry.as_ref()?;
        let language = registry.get(&doc.document.language)?.clone();
        let prefs = engine_prefs();
        let data = doc.diagnostics_data.clone();
        let (defs, entries) = entries_for(&language, &prefs, &data);
        let jobs: Vec<_> = defs
            .iter()
            .zip(entries.iter())
            .filter(|(_, entry)| entry.enabled && entry.is_job())
            .map(|(def, _)| def.clone())
            .collect();
        Some((language, jobs, state::saved_path(&doc.document)))
    });
    let Some((language, jobs, path)) = found else {
        return;
    };
    if jobs.is_empty() {
        state::notice("This language has no diagnostics to run.");
        return;
    }
    super::execution::status(id, "Running diagnostics…");
    Owner::detached().run(|| {
        spawn(async move {
            for def in &jobs {
                let Some(text) = state::text(id) else {
                    continue;
                };
                let mut job = DiagnosticJob::new(Arc::new(language.clone()), def.clone(), text);
                job.file = path.clone();
                job.buffer_is_saved = path.is_some();
                let name = def.name.clone();
                let file = job.file.clone();
                let buffer = job.text.clone();
                let cancel = CancelToken::new();
                let output = match blocking(move || run_job(&job, &cancel)).await {
                    Ok(Ok(output)) => output,
                    Ok(Err(error)) => {
                        state::notice(format!("{name}: {error}"));
                        continue;
                    }
                    Err(error) => {
                        state::notice(format!("{name}: {error}"));
                        continue;
                    }
                };
                state::with(|workspace| {
                    if let Some(doc) = workspace.doc_mut(id) {
                        let (ours, _) = output.annotations(&buffer, file.as_deref());
                        doc.annotations.replace_source(&name, Origin::Document, ours);
                        workspace.changed();
                    }
                });
                state::bump_diag_rev(id);
            }
            super::execution::status(id, "Diagnostics finished");
        });
    });
}

// ---------------------------------------------------------------- Navigation

/// A merged annotation range (editor positions + error flag): the document
/// annotations plus the LSP diagnostics.
fn annotation_ranges(id: DocumentId) -> Vec<(EditorPosition, EditorPosition, bool)> {
    state::with(|workspace| {
        let mut ranges: Vec<(EditorPosition, EditorPosition, bool)> = Vec::new();
        if let Some(doc) = workspace.doc(id) {
            let index = LineIndex::new(&doc.text);
            for annotation in doc.annotations.visible() {
                if annotation.file.is_none()
                    && let Some((start, end)) = annotation.positions(&index)
                {
                    ranges.push((
                        EditorPosition::new(start.line as u32, start.column as u32),
                        EditorPosition::new(end.line as u32, end.column as u32),
                        annotation.severity == Severity::Error,
                    ));
                }
            }
        }
        for diagnostic in super::lsp::diagnostics(id) {
            ranges.push((
                diagnostic.range.start,
                diagnostic.range.end,
                diagnostic.severity == nsh_base::EditorDiagnosticSeverity::Error,
            ));
        }
        ranges.sort_by_key(|&(start, _, _)| (start.line, start.column));
        ranges
    })
}

/// Moves the caret to the next/previous annotation (errors only when asked).
fn jump(forward: bool, errors_only: bool) {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let Some(api) = state::editor_api(id) else {
        return;
    };
    let ranges = annotation_ranges(id);
    if ranges.is_empty() {
        state::notice("No diagnostics.");
        return;
    }
    let head = api
        .selections()
        .last()
        .map(|s| s.head)
        .unwrap_or(EditorPosition::new(0, 0));
    let found = if forward {
        ranges
            .iter()
            .find(|&&(start, _, error)| {
                (start.line, start.column) > (head.line, head.column) && (!errors_only || error)
            })
            .or_else(|| ranges.iter().find(|&&(_, _, error)| !errors_only || error))
    } else {
        ranges
            .iter()
            .rev()
            .find(|&&(start, _, error)| {
                (start.line, start.column) < (head.line, head.column) && (!errors_only || error)
            })
            .or_else(|| ranges.iter().rev().find(|&&(_, _, error)| !errors_only || error))
    };
    let Some(&(start, end, _)) = found else {
        return;
    };
    let selection = EditorSelection::new(start, end);
    api.set_selections(vec![selection]);
    api.reveal_position(end, true);
    state::focus(id);
}

// ---------------------------------------------------------------- Fixes

/// Applies the fix of the next annotation after the caret, or every fix.
fn apply_fixes(next_only: bool) {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let Some(api) = state::editor_api(id) else {
        return;
    };
    let plan = state::with(|workspace| {
        let doc = workspace.doc_mut(id)?;
        let index = LineIndex::new(&doc.text);
        let caret = api
            .selections()
            .last()
            .map(|s| {
                index.offset(TextPos {
                    line: s.head.line as usize,
                    column: s.head.column as usize,
                })
            })
            .unwrap_or(0);
        Some(if next_only {
            doc.annotations.next_fix(caret)
        } else {
            doc.annotations.all_fixes()
        })
    });
    let Some(plan) = plan else { return };
    if plan.is_empty() {
        state::notice("No fixes available.");
        return;
    }
    let buffer = api.value();
    let index = LineIndex::new(&buffer);
    let mut edits: Vec<EditorEdit> = plan
        .edits
        .iter()
        .map(|edit| {
            let start = index.position(edit.range.start);
            let end = index.position(edit.range.end);
            EditorEdit::new(
                EditorRange::new(
                    EditorPosition::new(start.line as u32, start.column as u32),
                    EditorPosition::new(end.line as u32, end.column as u32),
                ),
                edit.text.clone(),
            )
        })
        .collect();
    edits.sort_by(|a, b| {
        (b.range.start.line, b.range.start.column).cmp(&(a.range.start.line, a.range.start.column))
    });
    api.edit(edits);
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            for annotation in plan.annotations {
                doc.annotations.dismiss(annotation);
            }
        }
    });
    state::bump_diag_rev(id);
}

// ---------------------------------------------------------------- Sheet

/// The document's ignored-messages sheet.
#[derive(Clone, PartialEq, Default)]
struct IgnoreSheet {
    open: bool,
    draft: String,
}

fn sheet() -> Signal<IgnoreSheet> {
    global()
}

/// "Ignored Messages For Document…": edits this document's ignore list.
fn document_ignored_sheet() {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let current = state::with(|workspace| {
        workspace
            .doc(id)
            .map(|doc| doc.annotations.document_ignore().to_text())
            .unwrap_or_default()
    });
    sheet().set(IgnoreSheet {
        open: true,
        draft: current,
    });
}

/// Writes the sheet's rules into the active document's ignore list.
fn apply_sheet() {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let list = RuleList::from_text(&sheet().with(|s| s.draft.clone()));
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.annotations.set_document_ignore(list.clone());
            doc.diagnostics_data.ignored = list;
        }
    });
    state::bump_diag_rev(id);
    sheet().update(|s| s.open = false);
}

// ---------------------------------------------------------------- Modes

/// "Enable/Disable Diagnostics For \"title\"": toggles the document's
/// master diagnostics flag (saved with the document).
fn toggle_document_diagnostics() {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.diagnostics_data.disabled = !doc.diagnostics_data.disabled;
        }
    });
    state::bump_diag_rev(id);
    state::bump_workspace_rev();
}

/// "Clear Diagnostics": removes every annotation (document, console and
/// LSP-sourced document annotations alike).
fn clear_diagnostics() {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.annotations.clear();
        }
    });
    state::bump_diag_rev(id);
}

/// Runs one job diagnostic on the active document ("Run … Now").
fn run_single(def: scratch_language::DiagnosticDef) {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let found = state::with(|workspace| {
        let doc = workspace.doc(id)?;
        let language = workspace
            .registry
            .as_ref()?
            .get(&doc.document.language)?
            .clone();
        Some((language, state::saved_path(&doc.document)))
    });
    let Some((language, path)) = found else { return };
    let name = def.name.clone();
    Owner::detached().run(|| {
        spawn(async move {
            let Some(text) = state::text(id) else { return };
            let mut job = DiagnosticJob::new(Arc::new(language), def, text);
            job.file = path.clone();
            job.buffer_is_saved = path.is_some();
            let file = job.file.clone();
            let buffer = job.text.clone();
            let cancel = CancelToken::new();
            match blocking(move || run_job(&job, &cancel)).await {
                Ok(Ok(output)) => {
                    state::with(|workspace| {
                        if let Some(doc) = workspace.doc_mut(id) {
                            let (ours, _) = output.annotations(&buffer, file.as_deref());
                            doc.annotations.replace_source(&name, Origin::Document, ours);
                            workspace.changed();
                        }
                    });
                    state::bump_diag_rev(id);
                }
                Ok(Err(error)) => state::notice(format!("{name}: {error}")),
                Err(error) => state::notice(format!("{name}: {error}")),
            }
        });
    });
}

/// Sets a diagnostic's update mode override (`mode` in bits; `None`
/// restores the language's default).
fn set_update_mode(language: String, diagnostic: String, mode: Option<i64>) {
    super::preferences::write(move |prefs| {
        let key = format!("{language}/{diagnostic}");
        let modes = &mut prefs.text_editing.diagnostics.update_overrides;
        match mode {
            Some(bits) => {
                modes.insert(key, bits);
            }
            None => {
                modes.remove(&key);
            }
        }
    });
}

/// Toggles a diagnostic for the active document ("Disable For Document …").
fn toggle_diagnostic_for_document(language: String, diagnostic: String) {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    let key = format!("{language}/{diagnostic}");
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            if doc.diagnostics_data.disabled_diagnostics.contains(&key) {
                doc.diagnostics_data.disabled_diagnostics.remove(&key);
            } else {
                doc.diagnostics_data.disabled_diagnostics.insert(key.clone());
            }
        }
    });
    state::bump_diag_rev(id);
    state::bump_workspace_rev();
}

/// The console text diagnostic's display mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleTextMode {
    /// Editor annotations plus console highlights.
    Messages,
    /// Console highlights only.
    HighlightOnly,
    /// Nothing.
    Disabled,
}

/// Sets the console text mode (Diagnostics > Console Text).
fn set_console_text_mode(language: String, mode: ConsoleTextMode) {
    let key = console_key(&language);
    super::preferences::write(move |prefs| {
        let diagnostics = &mut prefs.text_editing.diagnostics;
        diagnostics.console_highlight_only.remove(&key);
        diagnostics.disabled_diagnostics.remove(&key);
        match mode {
            ConsoleTextMode::Messages => {}
            ConsoleTextMode::HighlightOnly => {
                diagnostics.console_highlight_only.insert(key.clone());
            }
            ConsoleTextMode::Disabled => {
                diagnostics.disabled_diagnostics.insert(key.clone());
            }
        }
    });
    // Annotations already placed stay in the document until the next run,
    // unless the mode just removed them; highlights redraw immediately.
    let Some(id) = state::with(|workspace| workspace.active) else { return };
    if mode != ConsoleTextMode::Messages {
        state::with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                doc.annotations.clear_origin(scratch_diagnostics::Origin::Console);
            }
        });
        state::bump_diag_rev(id);
    }
    state::bump_console_rev(id);
}

/// "Customize ……": Preferences > Languages > Diagnostics for `language`.
fn customize(language: String) {
    super::preferences::open_language_diagnostics(&language);
}

/// The sheet's overlay, mounted at the root by `mod.rs`.
pub fn overlay() -> impl IntoElement {
    when(move || sheet().with(|s| s.open), ignore_sheet)
}

fn ignore_sheet() -> impl IntoElement {
    let field = text_area(
        "Regular expressions, one per line",
        move || SharedString::from(sheet().with(|s| s.draft.clone())),
        move |text: &str| sheet().update(|s| s.draft = text.to_owned()),
        TextFieldOptions::new(),
        None,
    );
    dialog()
        .label("Ignored Messages For Document")
        .on_cancel(|| sheet().update(|s| s.open = false))
        .flex_1()
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(tokens::scrim)
                .on_press(|| sheet().update(|s| s.open = false)),
        )
        .child(
            view().flex_1().items_center().pt(px(44.0)).child(
                view()
                    .w(px(520.0))
                    .p(px(20.0))
                    .gap(px(12.0))
                    .rounded(px(tokens::radius_xl()))
                    .bg(tokens::background)
                    .border_1()
                    .border_color(tokens::border)
                    .shadow_lg()
                    .shadow_color(tokens::shadow())
                    .child(
                        type_subhead()
                            .semibold()
                            .apply(text("Ignored Messages For Document"))
                            .text_color(tokens::foreground),
                    )
                    .child(
                        type_body()
                            .apply(text(
                                "Regular expressions, one per line. Matching diagnostics stay hidden in this document only.",
                            ))
                            .text_color(tokens::muted_foreground),
                    )
                    .child(field)
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .mt(px(4.0))
                            .child(button(
                                "Cancel",
                                ButtonOptions::new()
                                    .variant(ButtonVariant::Default)
                                    .size(ControlSize::Lg),
                                || sheet().update(|s| s.open = false),
                            ))
                            .child(button(
                                "Save",
                                ButtonOptions::new()
                                    .variant(ButtonVariant::Primary)
                                    .size(ControlSize::Lg),
                                apply_sheet,
                            )),
                    )),
        )
}

// ---------------------------------------------------------------- Menu/keys

/// Keyboard shortcuts of this module. Called once from the root.
pub fn install() {
    effect(|| {
        let _ = global::<super::EditorFocus>().get();
        let _ = global::<super::bindings::BindingsRevision>().get();
        super::bindings::on("run.runDiagnostics", run_diagnostics);
        super::bindings::on("diag.applyNextFix", || apply_fixes(true));
        super::bindings::on("diag.applyAllFixes", || apply_fixes(false));
        super::bindings::on("diag.nextDiagnostic", || jump(true, false));
        super::bindings::on("diag.nextError", || jump(true, true));
        super::bindings::on("diag.previousDiagnostic", || jump(false, false));
        super::bindings::on("diag.previousError", || jump(false, true));
    });
}

/// The Diagnostics menu's items.
///
/// After the navigation items come the document toggle and one submenu per
/// diagnostic source of the active document's language (Compile Script,
/// Invalid Syntax, … — `entries_for` merges the language's definitions with
/// the built-ins), then the Console Text modes — the same shape CodeRunner's
/// Diagnostics menu has.
pub fn menu_items() -> Vec<MenuItem> {
    let item = |title: &str, id: &str, action: fn()| {
        let keys = super::bindings::keys_for(id);
        let item = MenuItem::new(title).on_select(action);
        if keys.is_empty() { item } else { item.shortcut(&keys) }
    };
    let mut items = vec![
        // Run Diagnostics lives in the Run menu (⌘B) — repeating it here
        // would bind the same keystroke twice.
        item("Clear Diagnostics", "diag.clear", clear_diagnostics),
        MenuItem::separator(),
        item("Apply Next Fix", "diag.applyNextFix", || apply_fixes(true)),
        item("Apply All Fixes", "diag.applyAllFixes", || apply_fixes(false)),
        item("Jump To Next Diagnostic", "diag.nextDiagnostic", || jump(true, false)),
        item("Jump To Next Error", "diag.nextError", || jump(true, true)),
        item("Jump To Previous Diagnostic", "diag.previousDiagnostic", || jump(false, false)),
        item("Jump To Previous Error", "diag.previousError", || jump(false, true)),
        MenuItem::separator(),
    ];

    // The active document's language's diagnostics sources, each with its
    // update modes and the per-document disable — CodeRunner's per-source
    // submenus ("Compile Script", "Invalid Syntax", …).
    let context = state::with(|workspace| {
        let id = workspace.active?;
        let doc = workspace.doc(id)?;
        let language = workspace.registry.as_ref()?.get(&doc.document.language)?.clone();
        Some((doc.title.clone(), doc.diagnostics_data.clone(), language))
    });
    if let Some((title, data, language)) = context {
        let language_id = language.id.clone();
        let enabled = !data.disabled;
        items.push(
            MenuItem::new(format!("{} Diagnostics For \"{title}\"", if enabled { "Disable" } else { "Enable" }))
                .on_select(toggle_document_diagnostics),
        );
        let prefs = engine_prefs();
        let (defs, entries) = entries_for(&language, &prefs, &data);
        for (def, entry) in defs.iter().zip(entries.iter()) {
            if entry.is_console() {
                continue;
            }
            items.push(MenuItem::submenu(def.name.clone(), diagnostic_submenu(&language_id, def, entry, &title, &data)));
        }
        items.push(MenuItem::submenu("Console Text", console_text_submenu(&language_id)));
    }
    // CodeRunner names this item after the active document.
    let document_title = state::with(|workspace| workspace.active.and_then(|id| workspace.doc(id)).map(|doc| doc.title.clone()));
    let ignored_title = document_title.map(|title| format!("Ignored Messages For \"{title}\"…"))
        .unwrap_or_else(|| "Ignored Messages For Document…".to_owned());
    items.extend([
        MenuItem::separator(),
        item(&ignored_title, "diag.ignoredForDocument", document_ignored_sheet),
        item("Ignored Messages…", "diag.ignoredMessages", super::preferences::open_text_editing_diagnostics),
        MenuItem::separator(),
        item("Diagnostics Preferences…", "diag.preferences", super::preferences::open_text_editing_diagnostics),
    ]);
    items
}

/// One diagnostic source's submenu: run it, choose when it updates, disable
/// it for this document, customize it — CodeRunner's "Compile Script" shape.
fn diagnostic_submenu(
    language: &str,
    def: &scratch_language::DiagnosticDef,
    entry: &scratch_diagnostics::scheduler::Entry,
    title: &str,
    data: &DocumentDiagnosticsData,
) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if entry.is_job() {
        let def = def.clone();
        items.push(MenuItem::new(format!("Run \"{}\" Now", def.name)).on_select(move || run_single(def.clone())));
        items.push(MenuItem::separator());
    }
    let modes: [(i64, &str); 3] = [(1, "Update On Edit"), (2, "Update On Run"), (4, "Update Manually")];
    for (bits, label) in modes {
        let language = language.to_owned();
        let diagnostic = def.id.clone();
        let default = def.update.bits();
        // A mode matching the definition's default removes the override.
        items.push(
            MenuItem::new(label)
                .on_select(move || set_update_mode(language.clone(), diagnostic.clone(), (bits != default).then_some(bits)))
                .checked(entry.update.bits() & bits != 0),
        );
    }
    items.push(MenuItem::separator());
    let key = format!("{language}/{}", def.id);
    let disabled_here = data.disabled_diagnostics.contains(&key);
    let toggle_language = language.to_owned();
    let diagnostic = def.id.clone();
    items.push(
        MenuItem::new(format!("{} For Document \"{title}\"", if disabled_here { "Enable" } else { "Disable" }))
            .on_select(move || toggle_diagnostic_for_document(toggle_language.clone(), diagnostic.clone())),
    );
    items.push(MenuItem::separator());
    let customize_language = language.to_owned();
    items.push(MenuItem::new(format!("Customize \"{}\"…", def.name)).on_select(move || customize(customize_language.clone())));
    items
}

/// The Console Text submenu's three display modes (CodeRunner's):
/// annotations plus console highlights, highlights only, or disabled.
fn console_text_submenu(language: &str) -> Vec<MenuItem> {
    let diagnostics = &super::preferences::settings().get_untracked().text_editing.diagnostics;
    let key = console_key(language);
    let current = if diagnostics.disabled_diagnostics.contains(&key) {
        ConsoleTextMode::Disabled
    } else if diagnostics.console_highlight_only.contains(&key) {
        ConsoleTextMode::HighlightOnly
    } else {
        ConsoleTextMode::Messages
    };
    [("Console Highlight & Messages", ConsoleTextMode::Messages),
     ("Console Highlight Only", ConsoleTextMode::HighlightOnly),
     ("Disabled", ConsoleTextMode::Disabled)]
    .into_iter()
    .map(|(label, mode)| {
        let language = language.to_owned();
        MenuItem::new(label).on_select(move || set_console_text_mode(language.clone(), mode)).checked(current == mode)
    })
    .collect()
}
