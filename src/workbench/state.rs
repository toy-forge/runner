//! Engine-thread workbench state. Document ids are stable across tab switches;
//! each mounted editor owns its undo stack until that tab closes.

use std::cell::RefCell;
use std::path::PathBuf;

use nsh_base::{Editor, EditorApi};
use nsh_core::prelude::*;
use scratch_config::Paths;
use scratch_document::{Document, DocumentId, now_ms};
use scratch_language::{LanguageRegistry, PLAIN_TEXT_ID};
use scratch_runner::{RunHandle, Runner};

use super::persistence::{Journal, SavedDocument, Snapshot};

#[derive(Clone, PartialEq)]
pub struct Tab {
    pub id: DocumentId,
    pub title: String,
    pub dirty: bool,
    /// The language's registry id (for the tab's language tile).
    pub language_id: String,
    pub language: String,
    pub syntax: String,
}

#[derive(Clone, PartialEq)]
pub struct Display {
    pub tabs: Vec<Tab>,
    pub active: Option<DocumentId>,
    pub notice: String,
    pub loading: bool,
    pub console_visible: bool,
    pub console_height: f32,
    /// The left navigator is shown.
    pub sidebar_visible: bool,
    /// The navigator sidebar width in points.
    pub sidebar_width: f32,
    /// The sidebar shows the project Search view instead of the file navigator.
    pub sidebar_search: bool,
    /// The console sits on the right of the editors instead of below them.
    pub console_right: bool,
    /// The console's width when it is on the right, in points.
    pub console_width: f32,
    /// The right documentation view is shown.
    pub docs_visible: bool,
    /// The documentation view width in points.
    pub docs_width: f32,
    /// Whether the documentation view is widened via its bottom-edge grabber.
    pub docs_expanded: bool,
    /// The toolbar is shown (View > Hide Toolbar).
    pub toolbar_visible: bool,
    /// The tab strip is shown (View > Hide Tab Bar).
    pub tab_bar_visible: bool,
    /// Toolbar Back/Forward availability (caret history).
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// Whether the main workbench window is key/focused.
    pub window_focused: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            notice: String::new(),
            loading: true,
            console_visible: false,
            console_height: 200.0,
            sidebar_visible: true,
            sidebar_width: 240.0,
            sidebar_search: false,
            console_right: false,
            console_width: 360.0,
            docs_visible: false,
            docs_width: 300.0,
            docs_expanded: false,
            toolbar_visible: true,
            tab_bar_visible: true,
            can_go_back: false,
            can_go_forward: false,
            window_focused: true,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct WindowBoundsSignal(pub Option<super::persistence::SavedBounds>);

pub fn display() -> Signal<Display> { global::<Display>() }

/// Structured result of a finished run, shown in the editor status bar.
#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub code: Option<i32>,
    pub duration_secs: f64,
    pub canceled: bool,
    pub command_not_found: bool,
}

pub struct OpenDocument {
    pub document: Document,
    pub title: String,
    /// Text from the latest editor change; also used before the editor mounts.
    pub text: String,
    pub editor: Option<ComponentRef<Editor>>,
    /// The run's output: stream-tagged lines, bounded by the console model.
    pub console: scratch_console_model::Console,
    /// Bumped whenever `console` changes, so the console view redraws.
    pub console_rev: Signal<u64>,
    /// Bumped whenever `text` or the language changes, so the theme
    /// highlighter's decorations recompute (it cannot be bumped while the
    /// workspace `RefCell` is borrowed, so `edited`/`set_language` return it).
    pub text_rev: Signal<u64>,
    /// The incremental syntect highlighter of `scratch-theme`, created with
    /// the document's first `decorations()` call.
    pub highlighter: Option<scratch_theme::Highlighter>,
    /// Issues found in the run's console output (compiler errors, runtime
    /// failures), placed in the text by `scratch-diagnostics`.
    pub annotations: scratch_diagnostics::AnnotationSet,
    /// Per-document diagnostics state (ignored messages, disabled
    /// diagnostics) — persisted with the session snapshot.
    pub diagnostics_data: scratch_diagnostics::settings::DocumentDiagnosticsData,
    /// Matched console-diagnostic locations per source line (byte ranges in
    /// the line's text), filled by `import_diagnostics` after a run.
    pub console_highlights: std::collections::HashMap<usize, Vec<std::ops::Range<usize>>>,
    /// Bumped when `annotations` changes (the editor's diagnostics binding).
    pub diag_rev: Signal<u64>,
    /// The status line: `Ready`, `Running…`, `Run Succeeded · 0.02s`.
    pub status: Signal<String>,
    /// Live resource usage `(cpu_percent, rss_bytes)` while running.
    pub run_stats: Signal<Option<(f32, u64)>>,
    /// Structured completion info for the status bar.
    pub exit_info: Signal<Option<RunOutcome>>,
    /// Whether a run is in progress.
    pub running: Signal<bool>,
    pub handle: Option<RunHandle>,
    pub saving: bool,
}

pub struct Workspace {
    pub documents: Vec<OpenDocument>,
    pub active: Option<DocumentId>,
    pub registry: Option<LanguageRegistry>,
    pub runner: Runner,
    pub journal: Option<Journal>,
    pub paths: Option<Paths>,
    /// Recently opened file paths, most recent first (Open Quickly's
    /// `recentDocuments` provider).
    pub recent: Vec<PathBuf>,
    /// Back/Forward caret-history stacks (DESIGN 8.3).
    pub nav_back: Vec<NavPoint>,
    pub nav_fwd: Vec<NavPoint>,
    pub generation: u64,
    pub queued: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self { documents: Vec::new(), active: None, registry: None, runner: Runner::new(), journal: None, paths: None, recent: Vec::new(), nav_back: Vec::new(), nav_fwd: Vec::new(), generation: 1, queued: 0 }
    }
}

thread_local! { static WORKSPACE: RefCell<Workspace> = RefCell::new(Workspace::default()); }

/// A workspace borrow must not call into NSH controls or another workspace
/// operation; callbacks may otherwise re-enter this `RefCell`.
pub fn with<R>(f: impl FnOnce(&mut Workspace) -> R) -> R {
    WORKSPACE.with_borrow_mut(f)
}

impl Workspace {
    pub fn doc(&self, id: DocumentId) -> Option<&OpenDocument> { self.documents.iter().find(|doc| doc.document.id == id) }
    pub fn doc_mut(&mut self, id: DocumentId) -> Option<&mut OpenDocument> { self.documents.iter_mut().find(|doc| doc.document.id == id) }
    pub fn changed(&mut self) { self.generation = self.generation.saturating_add(1); }
}

pub fn notice(message: impl Into<String>) { display().update(|state| state.notice = message.into()); }

fn visible_title(doc: &OpenDocument, show_all_extensions: bool) -> String {
    if show_all_extensions || doc.document.untitled {
        return doc.title.clone();
    }
    std::path::Path::new(&doc.title)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| doc.title.clone())
}

/// Updates the small reactive view of the workspace, keeping text in editors.
pub fn publish() {
    let show_all_extensions = super::preferences::settings().with_untracked(|p| p.general.documents.show_all_extensions);
    let (tabs, active) = with(|workspace| {
        let tabs = workspace.documents.iter().map(|doc| {
            let def = workspace.registry.as_ref().and_then(|registry| registry.get(&doc.document.language));
            Tab { id: doc.document.id, title: visible_title(doc, show_all_extensions), dirty: doc.document.dirty,
                language_id: doc.document.language.clone(),
                language: def.map_or_else(|| doc.document.language.clone(), |def| def.name.clone()),
                syntax: def.map_or_else(|| "plain text".to_owned(), |def| def.editor_syntax.clone()) }
        }).collect();
        (tabs, workspace.active)
    });
    let changed = display().with_untracked(|view| view.tabs != tabs || view.active != active);
    display().update(|view| { view.tabs = tabs; view.active = active; });
    // Bumped only on a real tab/active change — `publish()` also runs on
    // every text edit (the dirty flag), and rebuilding the application menu
    // per keystroke would be wasted work.
    if changed {
        global::<WorkspaceRevision>().update(|rev| rev.0 += 1);
    }
}

/// The paths of the last opened files, most recent first.
pub fn recents() -> Vec<PathBuf> { with(|workspace| workspace.recent.clone()) }

/// Bumps `WorkspaceRevision` for a change `publish()` cannot see (a
/// document's diagnostics state).
pub fn bump_workspace_rev() {
    global::<WorkspaceRevision>().update(|rev| rev.0 += 1);
}

/// Bumped when the recents list changes; menus subscribe to it.
#[derive(Clone, Default, PartialEq)]
pub struct RecentsRevision(pub u64);

/// Bumped when the language registry is (re)loaded; menus that enumerate
/// languages or templates subscribe to it.
#[derive(Clone, Default, PartialEq)]
pub struct RegistryRevision(pub u64);

/// Bumped by `publish()` whenever the document set or the active document
/// changes; menus that embed the active document's title or diagnostics
/// state subscribe to it.
#[derive(Clone, Default, PartialEq)]
pub struct WorkspaceRevision(pub u64);

/// Bumped when the installed-font-family cache fills (native font queries
/// run on the main thread); the preferences font popup subscribes.
#[derive(Clone, Default, PartialEq)]
pub struct FontRevision(pub u64);

/// Bumps `FontRevision`; the font-cache warm calls it on first fill.
pub fn bump_font_rev() {
    global::<FontRevision>().update(|rev| rev.0 += 1);
}

/// Moves `path` to the front of the recents list (cap 25).
pub fn touched(path: &std::path::Path) {
    with(|workspace| {
        workspace.recent.retain(|p| p != path);
        workspace.recent.insert(0, path.to_path_buf());
        workspace.recent.truncate(25);
    });
    global::<RecentsRevision>().update(|rev| rev.0 += 1);
}

/// Empties the recents list (File > Open Recent > Clear Menu).
pub fn clear_recents() {
    with(|workspace| workspace.recent.clear());
    global::<RecentsRevision>().update(|rev| rev.0 += 1);
}

pub fn add(saved: SavedDocument) -> DocumentId {
    if let Some(path) = saved_path(&saved.document) { touched(&path); }
    let id = saved.document.id;
    let file_backed = saved.document.path.is_some();
    // Document signals must outlive the scope `add` happens to run in — a
    // transient owner (an event handler, a `dynamic` region) would take them
    // down on disposal and every later read would panic.
    let (console_rev, text_rev, diag_rev, status, run_stats, exit_info, running) = Owner::detached().run(|| {
        (signal(0), signal(0), signal(0), signal("Ready".into()), signal(None), signal(None), signal(false))
    });
    let diagnostics_data = saved.diagnostics.unwrap_or_default();
    let mut annotations = scratch_diagnostics::AnnotationSet::new();
    annotations.set_document_ignore(diagnostics_data.ignored.clone());
    let doc = OpenDocument { document: saved.document, title: saved.title, text: saved.text,
        editor: None, console: scratch_console_model::Console::new(scratch_console_model::ConsoleConfig::default()),
        console_rev, text_rev, highlighter: None,
        annotations, diagnostics_data, console_highlights: std::collections::HashMap::new(), diag_rev,
        status, run_stats, exit_info, running, handle: None, saving: false };
    let insert_mode = super::preferences::settings().with_untracked(|p| p.general.tabs.insert_new_tabs);
    with(|workspace| {
        match insert_mode {
            scratch_config::InsertNewTabs::BeforeExisting => workspace.documents.insert(0, doc),
            scratch_config::InsertNewTabs::AfterSelected => {
                let idx = workspace.active
                    .and_then(|active| workspace.documents.iter().position(|d| d.document.id == active))
                    .map(|pos| pos + 1)
                    .unwrap_or(workspace.documents.len());
                workspace.documents.insert(idx, doc);
            }
            scratch_config::InsertNewTabs::AfterExisting => workspace.documents.push(doc),
        }
        workspace.active = Some(id);
        workspace.changed();
    });
    // A restored document's language may predate a rename (a draft kept as
    // `clip-renamed.sh`): detection by path puts it right before the LSP
    // and the highlighter see it. Detection does not run without a path:
    // there is nothing for it to read but a fallback.
    if file_backed && redetect(id) { bump_text_rev(id); }
    publish();
    super::lsp::document_opened(id);
    id
}

/// File > New Tab (⌘T, the tab bar's `+`): a scratch document in the
/// active document's language — CodeRunner's behavior — rendered from
/// that language's template when `Use templates for new files` is on.
pub fn new_document() {
    let language = with(|workspace| {
        let active = workspace.active.and_then(|id| workspace.doc(id)).map(|doc| doc.document.language.as_str());
        new_document_language(active, workspace.registry.as_ref())
    });
    new_document_in(&language);
}

/// The language File > New Tab creates a document in: the active
/// document's, or the registry's default.
fn new_document_language(active: Option<&str>, registry: Option<&LanguageRegistry>) -> String {
    active.map(str::to_owned)
        .or_else(|| registry.map(|registry| registry.default_language().id.clone()))
        .unwrap_or_else(|| "shell".to_owned())
}

/// Next unused untitled tab title (`Untitled.ext`, `Untitled 2.ext`, …).
fn next_untitled_title(workspace: &Workspace, extension: &str) -> String {
    let first = format!("Untitled.{extension}");
    if !workspace.documents.iter().any(|doc| doc.title == first) {
        return first;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("Untitled {n}.{extension}");
        if !workspace.documents.iter().any(|doc| doc.title == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// First-launch welcome document: runs `FIRST_LAUNCH_TEXT` (`echo "Welcome to ScratchRunner!"`)
/// in Shell Script with caret at its `<##>` marker.
pub fn new_first_launch_document() {
    let (use_templates, user_indent) = super::preferences::settings().with_untracked(|p| {
        (
            p.general.documents.use_templates_for_new_files,
            scratch_language::IndentStyle {
                soft_tabs: p.text_editing.indentation.soft_tabs,
                width: p.text_editing.indentation.tab_width as u8,
            },
        )
    });
    let language = scratch_language::FIRST_LAUNCH_LANGUAGE;
    let (title, text, caret) = with(|workspace| {
        let def = workspace.registry.as_ref().and_then(|r| r.get(language));
        let extension = def.and_then(|d| d.extensions.first().map(String::as_str)).unwrap_or("sh");
        let title = next_untitled_title(workspace, extension);
        if !use_templates {
            return (title, String::new(), None);
        }
        let indent = def.map(|d| d.effective_indent(user_indent)).unwrap_or(user_indent);
        let rendered = scratch_language::render_template(
            scratch_language::FIRST_LAUNCH_TEXT,
            &scratch_language::TemplateContext::for_file(&title, indent),
        );
        (title, rendered.text, rendered.caret)
    });
    add_document(language, &title, text, caret, true);
}

/// A document in `language`, text from the language's default template when
/// Preferences > Documents > `Use templates for new files` is on; the caret
/// lands where the template's `<##>` was.
pub fn new_document_in(language: &str) {
    let (use_templates, user_indent) = super::preferences::settings().with_untracked(|p| {
        (
            p.general.documents.use_templates_for_new_files,
            scratch_language::IndentStyle {
                soft_tabs: p.text_editing.indentation.soft_tabs,
                width: p.text_editing.indentation.tab_width as u8,
            },
        )
    });
    let (title, text, caret) = with(|workspace| {
        let fallback = |language: &str| {
            let extension = workspace.registry.as_ref().and_then(|r| r.get(language))
                .and_then(|def| def.extensions.first().cloned()).unwrap_or_else(|| "txt".to_owned());
            (next_untitled_title(workspace, &extension), String::new(), None)
        };
        let Some(registry) = workspace.registry.as_ref() else { return fallback(language); };
        let Some(def) = registry.get(language) else { return fallback(language); };
        let extension = def.extensions.first().map(String::as_str).unwrap_or("txt");
        let title = next_untitled_title(workspace, extension);
        if !use_templates { return (title, String::new(), None); }
        let indent = def.effective_indent(user_indent);
        let rendered = registry.new_document(language, &scratch_language::TemplateContext::for_file(&title, indent));
        (title, rendered.text, rendered.caret)
    });
    add_document(language, &title, text, caret, true);
}

/// A document seeded with `text`, in `language` or the default one ("New
/// from Clipboard", "Duplicate"). The text is the user's, so the document
/// is not pristine.
pub fn new_document_with(language: Option<&str>, text: String) {
    let (language, title) = with(|workspace| {
        let language = language.map(str::to_owned)
            .or_else(|| workspace.registry.as_ref().map(|r| r.default_language().id.clone()))
            .unwrap_or_else(|| "shell".to_owned());
        let extension = workspace.registry.as_ref().and_then(|r| r.get(&language))
            .and_then(|d| d.extensions.first().cloned()).unwrap_or_else(|| "txt".to_owned());
        let title = next_untitled_title(workspace, &extension);
        (language, title)
    });
    add_document(&language, &title, text, None, false);
}

/// Adds a document of `language` with `title` and `text`, placing the caret
/// at `caret` (a template's `<##>` marker) when given; focuses the new tab.
/// `pristine` marks a never-edited scratch document, which a language
/// switch swaps to the new language's template.
fn add_document(language: &str, title: &str, text: String, caret: Option<(u32, u32)>, pristine: bool) -> DocumentId {
    let document = Document { language: language.into(), pristine, ..Document::default() };
    let id = add(SavedDocument { document, title: title.into(), text, diagnostics: None });
    if let Some((line, column)) = caret {
        spawn(async move {
            next_frame().await;
            if let Some(api) = editor_api(id) {
                let at = nsh_base::EditorPosition::new(line, column);
                api.set_selections(vec![nsh_base::EditorSelection::caret(at)]);
                api.reveal_position(at, true);
            }
        });
    }
    focus(id);
    id
}

/// Creates documents from a named template group of `language` (File > New
/// from Template): one tab per file of the group; the first gets the focus
/// and its caret marker.
pub fn new_document_from_template(language: &str, template_name: &str) {
    let user_indent = super::preferences::settings().with_untracked(|p| {
        scratch_language::IndentStyle {
            soft_tabs: p.text_editing.indentation.soft_tabs,
            width: p.text_editing.indentation.tab_width as u8,
        }
    });
    let template = with(|workspace| {
        let registry = workspace.registry.as_ref()?;
        let def = registry.get(language)?;
        let group = registry.templates_for(language).into_iter().find(|g| g.name == template_name)?;
        let indent = def.effective_indent(user_indent);
        Some((
            indent,
            group
                .files
                .iter()
                .map(|file| (file.extension.clone(), file.text.clone()))
                .collect::<Vec<_>>(),
        ))
    });
    let Some((indent, files)) = template else {
        notice(format!("Language {language} has no template “{template_name}”."));
        return;
    };
    let total = files.len();
    let mut first_id = None;
    for (extension, source) in files {
        let (title, text, caret) = with(|workspace| {
            let title = next_untitled_title(workspace, &extension);
            let rendered = scratch_language::render_template(
                &source,
                &scratch_language::TemplateContext::for_file(&title, indent),
            );
            (title, rendered.text, rendered.caret)
        });
        let id = add_document(language, &title, text, caret, true);
        if first_id.is_none() {
            first_id = Some(id);
        }
    }
    if total > 1
        && let Some(first_id) = first_id
    {
        activate(first_id);
    }
}

pub fn activate(id: DocumentId) {
    let switching = with(|workspace| workspace.doc(id).is_some() && workspace.active != Some(id));
    if switching { record_jump(); }
    let exists = with(|workspace| {
        if workspace.doc(id).is_none() { return false; }
        workspace.active = Some(id); workspace.changed(); true
    });
    if exists { publish(); focus(id); }
}

// ------------------------------------------------------- caret history

/// A caret location the toolbar's Back/Forward buttons can return to.
#[derive(Clone, Copy, PartialEq)]
pub struct NavPoint {
    pub doc: DocumentId,
    pub position: nsh_base::EditorPosition,
}

thread_local! { static NAVIGATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// The active document's caret.
fn nav_point() -> Option<NavPoint> {
    let id = with(|workspace| workspace.active)?;
    let position = editor_api(id)?.selections().first()?.head;
    Some(NavPoint { doc: id, position })
}

/// Mirrors the stacks' emptiness into `Display` for the toolbar's dimming.
fn sync_nav() {
    let (back, forward) = with(|workspace| (!workspace.nav_back.is_empty(), !workspace.nav_fwd.is_empty()));
    display().update(|view| { view.can_go_back = back; view.can_go_forward = forward; });
}

/// Remembers the current caret so Back can return to it; called before an
/// explicit jump (Go to Line, Open Quickly, a search result, a tab switch).
/// A jump clears the forward stack.
pub fn record_jump() {
    if NAVIGATING.with(|flag| flag.get()) { return; }
    let Some(point) = nav_point() else { return; };
    let pushed = with(|workspace| {
        if workspace.nav_back.last() == Some(&point) { return false; }
        workspace.nav_back.push(point);
        workspace.nav_fwd.clear();
        true
    });
    if pushed { sync_nav(); }
}

/// Jumps to `point`; `false` when its document is closed (the entry is dead).
fn jump_to(point: NavPoint) -> bool {
    if !with(|workspace| workspace.doc(point.doc).is_some()) { return false; }
    let _guard = NavGuard::new();
    activate(point.doc);
    if let Some(api) = editor_api(point.doc) {
        api.set_selections(vec![nsh_base::EditorSelection::caret(point.position)]);
        api.reveal_position(point.position, true);
        focus(point.doc);
    }
    true
}

/// Suppresses [`record_jump`] while Back/Forward is applying a location.
struct NavGuard;
impl NavGuard {
    fn new() -> Self {
        NAVIGATING.with(|flag| flag.set(true));
        NavGuard
    }
}
impl Drop for NavGuard {
    fn drop(&mut self) { NAVIGATING.with(|flag| flag.set(false)); }
}

/// Toolbar Back (`true`) / Forward (`false`).
pub fn nav_step(back: bool) {
    let current = nav_point();
    loop {
        let point = with(|workspace| if back { workspace.nav_back.pop() } else { workspace.nav_fwd.pop() });
        match point {
            None => { sync_nav(); return; }
            Some(point) => {
                if jump_to(point) {
                    if let Some(current) = current {
                        with(|workspace| if back { workspace.nav_fwd.push(current) } else { workspace.nav_back.push(current) });
                    }
                    sync_nav();
                    return;
                }
            }
        }
    }
}

pub fn focus(id: DocumentId) {
    // Detached: callers often run inside a panel about to be unmounted —
    // an owner-bound task would be cancelled before `next_frame`.
    Owner::detached().run(|| spawn(async move {
        next_frame().await;
        if let Some(api) = editor_api(id) { api.focus(); }
    }));
}

/// Selects the tab `delta` positions from the active one, wrapping (Window
/// menu's Select Previous / Next Tab).
pub fn step_tab(delta: i32) {
    let id = with(|workspace| {
        if workspace.documents.is_empty() { return None; }
        let len = workspace.documents.len() as i32;
        let active = workspace.documents.iter().position(|doc| Some(doc.document.id) == workspace.active).unwrap_or(0) as i32;
        Some(workspace.documents[(((active + delta) % len + len) % len) as usize].document.id)
    });
    if let Some(id) = id { activate(id); }
}

/// Moves the active tab `delta` slots, clamped at the ends (Window menu's
/// Move Tab Left / Right).
pub fn move_active_tab(delta: i32) {
    let moved = with(|workspace| {
        let Some(active) = workspace.active else { return false };
        let Some(at) = workspace.documents.iter().position(|doc| doc.document.id == active) else { return false };
        let len = workspace.documents.len() as i32;
        let to = (at as i32 + delta).clamp(0, len - 1);
        if to as usize == at { return false; }
        let doc = workspace.documents.remove(at);
        workspace.documents.insert(to as usize, doc);
        workspace.changed();
        true
    });
    if moved { publish(); }
}

/// The mounted editor API for integrations such as search and diagnostics.
pub fn editor_api(id: DocumentId) -> Option<std::rc::Rc<EditorApi>> {
    let reference = with(|workspace| workspace.doc(id).and_then(|doc| doc.editor))?;
    reference.api::<EditorApi>()
}

/// Reads the current editor rather than a saved disk copy.
pub fn text(id: DocumentId) -> Option<String> {
    if let Some(editor) = editor_api(id) { return Some(editor.value()); }
    with(|workspace| workspace.doc(id).map(|doc| doc.text.clone()))
}

pub fn edited(id: DocumentId, value: &str) {
    let (changed, candidate) = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return (false, false); };
        if doc.text == value { return (false, false); }
        // A document with a detected language may still be told what it is:
        // `#!/bin/sh` or a modeline in a Plain Text document re-detects it.
        // Documents that already have a language keep it (an explicitly
        // chosen one is `language_locked` and skipped by `redetect` too).
        let candidate = doc.document.language == PLAIN_TEXT_ID;
        doc.text = value.to_owned();
        doc.document.mark_edited(now_ms());
        workspace.changed();
        (true, candidate)
    });
    if !changed { return; }
    let detected = candidate && redetect(id);
    bump_text_rev(id);
    publish();
    if detected {
        super::lsp::document_closed(id);
        super::lsp::document_opened(id);
    } else {
        super::lsp::document_changed(id);
    }
    super::diagnostics::on_edit(id);
}

/// `text_rev` is bumped outside `with`: its subscribers (the editor's
/// decorations binding) re-borrow the workspace when notified.
pub fn bump_text_rev(id: DocumentId) {
    let rev = with(|workspace| workspace.doc(id).map(|doc| doc.text_rev));
    if let Some(rev) = rev { rev.update(|rev| *rev = rev.wrapping_add(1)); }
}

/// Toggles a breakpoint on `line` of `id` (`Toggle Breakpoint at Current
/// Line`, a line-number press). `text_rev` bumps so the gutter marks refresh.
pub fn toggle_breakpoint(id: DocumentId, line: u32) {
    let changed = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        doc.document.breakpoints.toggle(line);
        workspace.changed(); true
    });
    if changed {
        bump_text_rev(id);
        super::debugger::sync_breakpoints(id);
    }
}

/// Enables/disables the breakpoint on `line` (a disabled one keeps its
/// place but never stops, CodeRunner's "Disable Breakpoint").
pub fn set_breakpoint_enabled(id: DocumentId, line: u32, enabled: bool) {
    let changed = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        let Some(breakpoint) = doc.document.breakpoints.get_mut(line) else { return false };
        if breakpoint.enabled == enabled { return false; }
        breakpoint.enabled = enabled;
        workspace.changed();
        true
    });
    if changed {
        bump_text_rev(id);
        super::debugger::sync_breakpoints(id);
    }
}

/// Sets or clears the condition of the breakpoint on `line` (CodeRunner's
/// Edit Breakpoint); an empty text clears it.
pub fn set_breakpoint_condition(id: DocumentId, line: u32, condition: Option<String>) {
    let condition = condition.filter(|c| !c.trim().is_empty());
    let changed = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        let Some(breakpoint) = doc.document.breakpoints.get_mut(line) else { return false };
        if breakpoint.condition == condition { return false; }
        breakpoint.condition = condition;
        workspace.changed();
        true
    });
    if changed {
        bump_text_rev(id);
        super::debugger::sync_breakpoints(id);
    }
}

/// Bumps `diag_rev` outside `with`, like [`bump_text_rev`].
pub fn bump_diag_rev(id: DocumentId) {
    let rev = with(|workspace| workspace.doc(id).map(|doc| doc.diag_rev));
    if let Some(rev) = rev { rev.update(|rev| *rev = rev.wrapping_add(1)); }
}

/// Bumps `console_rev` outside `with`, so the console view redraws after
/// changes that are not appends (diagnostic highlights, mode switches).
pub fn bump_console_rev(id: DocumentId) {
    let rev = with(|workspace| workspace.doc(id).map(|doc| doc.console_rev));
    if let Some(rev) = rev { rev.update(|rev| *rev = rev.wrapping_add(1)); }
}

/// The console-origin annotations of `id` as editor diagnostics, merged into
/// the editor's `diagnostics` prop after the language server's (`lsp.rs`).
/// The `diag_rev` read subscribes the binding to new imports.
pub fn console_diagnostics(id: DocumentId) -> Vec<nsh_base::EditorDiagnostic> {
    use nsh_base::{EditorDiagnosticSeverity, EditorPosition, EditorRange};
    let from_console = super::preferences::settings().with_untracked(|p| p.text_editing.diagnostics.from_console);
    with(|workspace| {
        let Some(doc) = workspace.doc(id) else { return Vec::new() };
        let _ = doc.diag_rev.get();
        if doc.annotations.counts().total() == 0 { return Vec::new(); }
        let index = scratch_diagnostics::LineIndex::new(&doc.text);
        doc.annotations.visible().iter().filter(|annotation| {
            from_console || annotation.origin != scratch_diagnostics::Origin::Console
        }).filter_map(|annotation| {
            let (start, end) = annotation.positions(&index)?;
            let mut diagnostic = nsh_base::EditorDiagnostic::default();
            diagnostic.range = EditorRange::new(
                EditorPosition::new(start.line as u32, start.column as u32),
                EditorPosition::new(end.line as u32, end.column as u32),
            );
            diagnostic.severity = match annotation.severity {
                scratch_diagnostics::Severity::Error => EditorDiagnosticSeverity::Error,
                scratch_diagnostics::Severity::Warning => EditorDiagnosticSeverity::Warning,
                scratch_diagnostics::Severity::Notice => EditorDiagnosticSeverity::Info,
            };
            diagnostic.message = SharedString::from(annotation.message.clone());
            diagnostic.source = annotation.source.clone().map(SharedString::from);
            Some(diagnostic)
        }).collect()
    })
}

/// Re-detects `id`'s language from its path and text (extension, file name,
/// shebang, modeline). Skipped for a document whose language the user chose
/// (`language_locked`) and for one without a language registry yet. Returns
/// whether the language changed; callers re-sync the highlighter and the
/// language server when it did.
pub fn redetect(id: DocumentId) -> bool {
    with(|workspace| {
        let Workspace { registry, documents, .. } = workspace;
        let Some(registry) = registry.as_ref() else { return false; };
        let Some(doc) = documents.iter_mut().find(|doc| doc.document.id == id) else { return false; };
        doc.document.redetect_language(registry, &doc.text)
    })
}

/// `Untitled.txt` renamed for another `extension` (`Untitled.py`): the last
/// suffix is replaced, or appended when there is none.
fn retitle(title: &str, extension: &str) -> String {
    match title.rfind('.') {
        Some(dot) if dot > 0 => format!("{}.{extension}", &title[..dot]),
        _ => format!("{title}.{extension}"),
    }
}

pub fn set_language(id: DocumentId, language: &str) {
    let (use_templates, user_indent) = super::preferences::settings().with_untracked(|p| {
        (
            p.general.documents.use_templates_for_new_files,
            scratch_language::IndentStyle {
                soft_tabs: p.text_editing.indentation.soft_tabs,
                width: p.text_editing.indentation.tab_width as u8,
            },
        )
    });
    let (changed, swap) = with(|workspace| {
        let Some(extension) = workspace.registry.as_ref().and_then(|registry| registry.get(language))
            .map(|def| def.extensions.first().cloned()) else { return (false, None) };
        let Some(doc) = workspace.doc(id) else { return (false, None) };
        if doc.document.language == language { return (false, None); }
        // A scratch document's title tracks its language's extension
        // (`Untitled.txt` becomes `Untitled.py`); a saved file keeps its
        // real name, the language being a view over it.
        let title = match (&extension, doc.document.untitled) {
            (Some(extension), true) => retitle(&doc.title, extension),
            _ => doc.title.clone(),
        };
        // A pristine scratch document (never edited, never saved — its
        // text is still a template's or empty) takes the new language's
        // template, as CodeRunner does; once edited, only the language
        // changes and the text is kept.
        let swap = workspace.registry.as_ref().and_then(|registry| {
            let indent = registry.get(language).map(|def| def.effective_indent(user_indent)).unwrap_or_default();
            template_swap(registry, &doc.document, language, &title, use_templates, indent)
        });
        let Some(doc) = workspace.doc_mut(id) else { return (false, None) };
        doc.document.set_language(language);
        doc.title = title;
        workspace.changed(); (true, swap)
    });
    if let Some((text, caret)) = swap {
        with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                doc.text = text.clone();
                doc.document.dirty = false;
                doc.document.pristine = true;
            }
        });
        if let Some(api) = editor_api(id) {
            api.set_value(text);
            if let Some((line, column)) = caret {
                let at = nsh_base::EditorPosition::new(line, column);
                api.set_selections(vec![nsh_base::EditorSelection::caret(at)]);
                api.reveal_position(at, true);
            }
        }
        with(|workspace| {
            if let Some(doc) = workspace.doc_mut(id) {
                // Writing the swapped template is not an edit: the
                // document stays pristine and unmodified under the new
                // language (the editor's set_value may have reported one).
                doc.document.dirty = false;
                doc.document.pristine = true;
            }
        });
    }
    if changed {
        bump_text_rev(id);
        publish();
        // The server holds the document under its old language: close and
        // reopen so it reattaches under the new one (and the wrong server's
        // diagnostics and completions are dropped).
        super::lsp::document_closed(id);
        super::lsp::document_opened(id);
    }
}

/// The text a switch to `language` puts into `doc`, as `(text, caret)`:
/// a pristine scratch document takes the new language's template — empty
/// when the language has none (the `None` / plain-text mode) or when
/// `use_templates` is off; `None` once the document has user edits, so
/// only the language changes and its text is kept.
fn template_swap(
    registry: &LanguageRegistry,
    doc: &Document,
    language: &str,
    title: &str,
    use_templates: bool,
    indent: scratch_language::IndentStyle,
) -> Option<(String, Option<(u32, u32)>)> {
    if !(doc.pristine && doc.untitled) { return None; }
    if !use_templates { return Some((String::new(), None)); }
    let rendered = registry.new_document(language, &scratch_language::TemplateContext::for_file(title, indent));
    Some((rendered.text, rendered.caret))
}

pub fn remove(id: DocumentId) {
    with(|workspace| {
        let Some(index) = workspace.documents.iter().position(|doc| doc.document.id == id) else { return; };
        let doc = workspace.documents.remove(index);
        if let Some(handle) = doc.handle { handle.cancel(); }
        if workspace.active == Some(id) { workspace.active = workspace.documents.get(index).or_else(|| workspace.documents.last()).map(|doc| doc.document.id); }
        workspace.changed();
    });
    publish();
    super::lsp::document_closed(id);
    if let Some(id) = with(|workspace| workspace.active) { focus(id); }
}

/// Captures all current editor buffers before a background write.
pub fn snapshot() -> Snapshot {
    let ids = with(|workspace| workspace.documents.iter().map(|doc| doc.document.id).collect::<Vec<_>>());
    let values = ids.iter().map(|id| text(*id)).collect::<Vec<_>>();
    let view = display().get_untracked();
    let window_bounds = global::<WindowBoundsSignal>().get_untracked().0;
    with(|workspace| Snapshot {
        documents: workspace.documents.iter().zip(values).map(|(doc, value)| SavedDocument {
            document: doc.document.clone(), title: doc.title.clone(), text: value.unwrap_or_else(|| doc.text.clone()),
            diagnostics: (!doc.diagnostics_data.is_empty()).then(|| doc.diagnostics_data.clone()),
        }).collect(),
        active: workspace.active,
        console_height: view.console_height,
        console_visible: view.console_visible,
        console_right: view.console_right,
        console_width: view.console_width,
        sidebar_visible: view.sidebar_visible,
        sidebar_width: view.sidebar_width,
        docs_visible: view.docs_visible,
        docs_width: view.docs_width,
        toolbar_visible: view.toolbar_visible,
        tab_bar_visible: view.tab_bar_visible,
        window_bounds,
        ..Snapshot::default()
    })
}

pub fn path_title(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// Actual file path for a saved document; recovered untitled files remain drafts.
pub fn saved_path(doc: &Document) -> Option<PathBuf> {
    if doc.untitled { None } else { doc.path.clone() }
}

#[cfg(test)]
mod tests {
    use super::{new_document_language, retitle, template_swap};
    use scratch_document::Document;
    use scratch_language::{IndentStyle, LanguageRegistry, TemplateContext};

    #[test]
    fn retitle_swaps_the_extension() {
        assert_eq!(retitle("Untitled.txt", "py"), "Untitled.py");
        assert_eq!(retitle("clip-renamed.sh", "py"), "clip-renamed.py");
        assert_eq!(retitle("notes.archive.txt", "sh"), "notes.archive.sh");
        assert_eq!(retitle("Untitled 2", "py"), "Untitled 2.py");
    }

    /// A pristine scratch document (untitled, never edited).
    fn scratch(language: &str) -> Document {
        Document { pristine: true, untitled: true, language: language.into(), ..Document::default() }
    }

    const INDENT: IndentStyle = IndentStyle { soft_tabs: true, width: 4 };

    #[test]
    fn a_new_tab_inherits_the_active_language() {
        let registry = LanguageRegistry::builtin();
        assert_eq!(new_document_language(Some("python"), Some(&registry)), "python");
        assert_eq!(new_document_language(Some("plain-text"), Some(&registry)), "plain-text");
        assert_eq!(new_document_language(None, Some(&registry)), registry.default_language().id);
        assert_eq!(new_document_language(None, None), "shell");
    }

    #[test]
    fn a_pristine_document_swaps_to_the_new_language_template() {
        let registry = LanguageRegistry::builtin();
        let doc = scratch("shell");
        let (text, _) = template_swap(&registry, &doc, "go", "Untitled.go", true, INDENT).unwrap();
        let expected = registry.new_document("go", &TemplateContext::for_file("Untitled.go", INDENT)).text;
        assert_eq!(text, expected);
        assert!(!text.is_empty(), "the Go template renders initial text");
    }

    #[test]
    fn an_edited_document_keeps_its_text() {
        let registry = LanguageRegistry::builtin();
        let mut doc = scratch("shell");
        doc.mark_edited(1);
        assert!(template_swap(&registry, &doc, "go", "Untitled.go", true, INDENT).is_none());
        // A document that was never pristine (New from Clipboard) keeps
        // its text as well.
        let pasted = Document { untitled: true, language: "plain-text".into(), ..Document::default() };
        assert!(template_swap(&registry, &pasted, "python", "Untitled.py", true, INDENT).is_none());
        // And a saved (non-untitled) document too.
        let saved = Document { pristine: true, untitled: false, language: "shell".into(), ..Document::default() };
        assert!(template_swap(&registry, &saved, "go", "main.go", true, INDENT).is_none());
    }

    #[test]
    fn the_none_language_has_no_template() {
        let registry = LanguageRegistry::builtin();
        let doc = scratch("python");
        let (text, caret) = template_swap(&registry, &doc, "plain-text", "Untitled.txt", true, INDENT).unwrap();
        assert!(text.is_empty() && caret.is_none());
        // Templates off: a pristine document swaps to empty text.
        let (text, _) = template_swap(&registry, &doc, "go", "Untitled.go", false, INDENT).unwrap();
        assert!(text.is_empty());
    }
}
