//! Language servers: completions, hovers and diagnostics for the editors.
//!
//! `state.rs` calls [`document_opened`], [`document_changed`] and
//! [`document_closed`] outside any workspace borrow. [`attach`] wires one
//! editor's completion, hover and diagnostics props, and [`overlay`] (mounted
//! once at the root) shows the hover card. A server starts the first time a
//! document of its language is synced; a missing one is a quiet state with a
//! status-bar notice, and completion falls back to words and keywords.
//!
//! Every server call runs on the Tokio runtime. Syncs go through one worker,
//! in order, so a late sync never overwrites newer text; requests wait for it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use nsh_base::editor::{
    EditorCodeActionRequest, EditorCompletionAcceptEvent, EditorCompletionItem,
    EditorCompletionKind, EditorCompletionRequest, EditorCompletionTrigger,
    EditorDefinitionRequest, EditorDefinitionTrigger, EditorEdit,
};
use nsh_base::text_view::PartStyle;
use nsh_base::{
    EditorDiagnostic, EditorDiagnosticCodeDescription, EditorDiagnosticRelatedInformation,
    EditorDiagnosticSeverity, EditorDiagnosticTag, EditorElement, EditorHoverEvent,
    EditorLocation, EditorPosition, EditorRange, EditorSelection, EditorStyles, PopoverEvent,
    input, popover,
};
use nsh_core::element::{Style, StyleRefinement};
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use nsh_core::overlay::Placement;
use nsh_core::PointerEvents;
use scratch_ui_kit::prelude::*;
use scratch_document::DocumentId;
use scratch_lsp::{
    AppliedEdit, Block, CodeAction, CompletionItem, CompletionKind, CompletionTrigger,
    Diagnostic, FormattingOptions, InsertFormat, Location, Manager, ManagerConfig, ManagerEvent,
    MessageLevel, Position, Range, RichText, ServerCatalog, ServerStatus, Severity, TextEdit,
};
use tokio::sync::{mpsc, oneshot};

use super::preferences;
use super::state::{self, with};

/// How long typing pauses before the server hears the text.
const SYNC_DELAY: Duration = Duration::from_millis(150);
/// The most characters a hover card shows.
const HOVER_LIMIT: usize = 1200;
/// The most completion items handed to the editor.
const COMPLETION_LIMIT: usize = 200;

// Engine-thread state.

/// What this module knows of one open document.
struct Doc {
    uri: String,
    language: String,
    sync: SyncState,
    /// The last completion list, for the additional edits of an accepted item.
    completions: Vec<CompletionItem>,
    /// Raw LSP diagnostics from the last publishDiagnostics notification.
    diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct Lsp {
    sender: Option<mpsc::UnboundedSender<Command>>,
    manager: Option<Manager>,
    docs: HashMap<DocumentId, Doc>,
    /// Servers that are not installed or are turned off: their documents are not synced.
    quiet: HashSet<String>,
    hover: u64,
    definition_hover: u64,
    last_anchor: Option<Bounds<Points>>,
}

thread_local! { static LSP: RefCell<Lsp> = RefCell::new(Lsp::default()); }

/// Like `state::with`: no NSH or workspace calls inside.
fn lsp<R>(f: impl FnOnce(&mut Lsp) -> R) -> R { LSP.with_borrow_mut(f) }

/// Each open document's diagnostics, as its editor shows them.
#[derive(Clone, Default, PartialEq)]
struct Diagnostics(HashMap<DocumentId, Vec<EditorDiagnostic>>);

/// Each open document's completion trigger characters, from its server.
#[derive(Clone, Default, PartialEq)]
struct Triggers(HashMap<DocumentId, Vec<SharedString>>);

/// The hover card: where it points and what it says. It keeps its content
/// while it closes, so the exit plays in place.
#[derive(Clone, Default, PartialEq)]
struct HoverCard {
    open: bool,
    document: Option<DocumentId>,
    anchor: Bounds<Points>,
    problems: Vec<(EditorDiagnosticSeverity, String)>,
    blocks: Vec<Block>,
}

impl HoverCard {
    fn is_empty(&self) -> bool { self.problems.is_empty() && self.blocks.is_empty() }
}

/// The editor that has the keyboard focus, so a shortcut completes there.
#[derive(Clone, Default, PartialEq)]
struct Focused(Option<DocumentId>);

/// Returns the document id of the currently focused editor, if any.
pub(super) fn focused_document() -> Option<DocumentId> { global::<Focused>().get().0 }

#[derive(Clone, PartialEq)]
struct ServerStatusEntry {
    key: String,
    name: String,
    languages: String,
    badge: String,
    detail: Option<String>,
    ready: bool,
    error: bool,
}

#[derive(Clone, Default, PartialEq)]
struct ServerStatuses(std::collections::BTreeMap<String, ServerStatusEntry>);

#[derive(Clone, Default, PartialEq)]
struct StatusSheetOpen(bool);

#[derive(Clone, Default, PartialEq)]
struct RenameSheet {
    open: bool,
    doc: Option<DocumentId>,
    position: Position,
    original: String,
    draft: String,
}

// Public hooks.

/// Starts the manager and its event pump, and binds Ctrl-Space, Option-Escape,
/// F12, Shift-Cmd-Space, F2, and Ctrl-Shift-I. Called once from the root.
pub fn install() {
    let mut initial = std::collections::BTreeMap::new();
    for def in ServerCatalog::builtin().definitions() {
        initial.insert(
            def.id.clone(),
            ServerStatusEntry {
                key: def.id.clone(),
                name: def.name.clone(),
                languages: def.languages.keys().cloned().collect::<Vec<_>>().join(", "),
                badge: if def.enabled { "Idle".to_owned() } else { "Disabled".to_owned() },
                detail: Some(def.install_hint.clone()).filter(|s| !s.is_empty()),
                ready: false,
                error: false,
            },
        );
    }
    global::<ServerStatuses>().set(ServerStatuses(initial));

    let data = std::env::var_os("SCRATCHRUNNER_DATA_DIR").map(PathBuf::from);
    let path = std::env::var("PATH").unwrap_or_default();
    let mut config = ManagerConfig::new(std::env::temp_dir(), path, std::env::vars().collect());
    if let Some(data) = data { config = config.with_cache_dir(data.join("lsp-workspaces")); }
    // Unsaved documents get their own directory; a server never indexes the home folder.
    config.workspace_root = config.cache_dir.clone();
    let (manager, events) = Manager::new(config);
    let (sender, commands) = mpsc::unbounded_channel();
    lsp(|lsp| { lsp.manager = Some(manager.clone()); lsp.sender = Some(sender); });
    spawn(async move { let _ = background(worker(manager, commands)).await; });
    spawn(pump(events));
    effect(|| {
        let _ = global::<super::bindings::BindingsRevision>().get();
        let focused = global::<Focused>().get().0.is_some();
        super::bindings::on("lsp.serverStatus", open_status_sheet);
        if !focused { return; }
        super::bindings::on("text.showCompletions", complete_focused);
        super::bindings::on("lsp.goToDefinition", go_to_definition);
        super::bindings::on("lsp.signatureHelp", signature_help);
        super::bindings::on("lsp.rename", rename_symbol);
        super::bindings::on("lsp.formatDocument", format_document);
        on_key("alt+escape", |_| complete_focused());
    });
}

/// A document opened, or its language changed: the server hears it now.
pub fn document_opened(id: DocumentId) {
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else { return; };
        if landed.await.is_ok() { fetch_triggers(id, manager, uri).await; }
    });
}

/// A document's text changed: the server hears it once typing pauses.
/// Editor reads wait for a task: this runs inside the editor's `on_change`.
pub fn document_changed(id: DocumentId) {
    let edits = lsp(|lsp| lsp.docs.get_mut(&id).map(|doc| doc.sync.edited()));
    hide_hover();
    maybe_trigger_signature_help(id);
    spawn(async move {
        let Some(edits) = edits else { sync_now(id); return; };
        timer(SYNC_DELAY).await;
        let due = lsp(|lsp| lsp.docs.get(&id).is_some_and(|doc| doc.sync.edits == edits && doc.sync.pending()));
        if due { sync_now(id); }
    });
}

/// A document closed: its server forgets it, its requests are cancelled and
/// its diagnostics cleared.
pub fn document_closed(id: DocumentId) {
    let uri = lsp(|lsp| lsp.docs.remove(&id).map(|doc| doc.uri));
    if let Some(uri) = uri { send(Command::Close(uri)); }
    global::<Diagnostics>().update(|all| { all.0.remove(&id); });
    global::<Triggers>().update(|all| { all.0.remove(&id); });
    if global::<HoverCard>().with_untracked(|card| card.document == Some(id)) { hide_hover(); }
    if global::<RenameSheet>().with_untracked(|sheet| sheet.open && sheet.doc == Some(id)) {
        global::<RenameSheet>().update(|sheet| sheet.open = false);
    }
    if global::<Focused>().with_untracked(|focused| focused.0 == Some(id)) { global::<Focused>().set(Focused(None)); }
}

/// The LSP diagnostics of a document (for the Diagnostics menu's merged
/// navigation; console annotations live in `doc.annotations`).
pub fn diagnostics(id: DocumentId) -> Vec<EditorDiagnostic> {
    global::<Diagnostics>().with(|all| all.0.get(&id).cloned().unwrap_or_default())
}

/// Wires an editor to its document's language server: completion, hover,
/// go-to-definition, code actions, and diagnostics.
pub fn attach(editor: EditorElement, id: DocumentId) -> EditorElement {
    let diagnostics = computed(move || {
        let _ = preferences::settings().with(|p| p.text_editing.diagnostics.from_console);
        let mut all = global::<Diagnostics>().with(|all| all.0.get(&id).cloned().unwrap_or_default());
        all.extend(state::console_diagnostics(id));
        all
    });
    let triggers = computed(move || global::<Triggers>().with(|all| all.0.get(&id).cloned().unwrap_or_default()));
    editor
        .styles(editor_styles)
        .completion(true)
        .completion_trigger_characters(triggers)
        .diagnostics(diagnostics)
        .on_completion_request(move |request: &EditorCompletionRequest| completion_requested(id, request.clone()))
        .on_completion_accept(move |event: &EditorCompletionAcceptEvent| completion_accepted(id, &event.item))
        .on_hover(move |event: &EditorHoverEvent| hover_requested(id, event.clone()))
        .on_hover_end(hide_hover)
        .on_definition_request(move |request: &EditorDefinitionRequest| definition_requested(id, *request))
        .on_code_action_request(move |request: &EditorCodeActionRequest| code_action_requested(id, *request))
        .on_selection_change(|_: &Vec<EditorSelection>| hide_hover())
        .on_focus(move || {
            global::<Focused>().set(Focused(Some(id)));
            super::editor_focused();
        })
        .on_blur(move || {
            if global::<Focused>().with_untracked(|focused| focused.0 == Some(id)) { global::<Focused>().set(Focused(None)); }
        })
}

/// Stops every server. The root awaits it before the app quits.
pub async fn shutdown() {
    let Some(manager) = lsp(|lsp| lsp.manager.clone()) else { return; };
    let _ = background(async move { let _ = tokio::time::timeout(Duration::from_secs(2), manager.shutdown()).await; }).await;
}

/// Hover card, Rename Symbol sheet, and LSP Server Status modal, mounted once
/// at the root.
pub fn overlay() -> impl IntoElement {
    let card = global::<HoverCard>();
    let anchor = move || card.with(|card| card.anchor);
    view()
        .child(nsh_core::prelude::overlay()
            .left(move || Length::from(anchor().origin.x))
            .top(move || Length::from(anchor().origin.y))
            .w(move || Length::from(anchor().size.width))
            .h(move || Length::from(anchor().size.height))
            .pointer_events(PointerEvents::None)
            .child(popover()
                .open(move || Some(card.with(|card| card.open)))
                .on_open_change(|event: &PopoverEvent| {
                    if let PopoverEvent::OpenChange(false, _) = event { hide_hover(); }
                })
                .placement(Placement::Bottom)
                .offset(px(6.0))
                .margin(px(8.0))
                .popup_style(|s| s.max_w(px(360.0)).p(px(12.0)).gap_2().rounded(px(design_tokens::radius_lg()))
                    .bg(design_tokens::popover()).border_1().border_color(design_tokens::border())
                    .shadow_md().shadow_color(design_tokens::shadow()))
                .popup_starting_style(motion::popup_starting)
                .popup_ending_style(motion::popup_ending)
                .trigger(view().w_full().h_full().focusable(false))
                .child(hover_content(card))))
        .child(when(move || global::<RenameSheet>().with(|s| s.open), rename_sheet_view))
        .child(when(move || global::<StatusSheetOpen>().with(|s| s.0), status_sheet_view))
}

fn hover_content(card: Signal<HoverCard>) -> impl IntoElement {
    view().gap_2().label("Hover information")
        .child(each(move || card.with(|card| card.problems.clone()), |problem: &(EditorDiagnosticSeverity, String)| problem.1.clone(), |row, _| {
            let (severity, message) = row.get_untracked();
            let (icon, tone) = severity_glyph(severity);
            view().flex_row().gap_2().items_start()
                .label(format!("{}: {message}", severity_label(severity)))
                .child(view().pt(px(2.0)).child(glyph(icon, IconSize::Xs, tone)))
                .child(type_body().apply(text(message)).flex_1().min_w(px(0.0)).text_color(design_tokens::popover_foreground))
        }))
        .child(when(move || card.with(|card| !card.problems.is_empty() && !card.blocks.is_empty()), || {
            separator(SeparatorOrientation::Horizontal)
        }))
        .child(dynamic(move || {
            let blocks = card.with(|card| card.blocks.clone());
            untracked(|| view().gap_2().children(blocks.into_iter().map(block_view)).into_element())
        }))
}

/// The completion list's and the diagnostics' looks (DESIGN 8.6.2, 8.6.3),
/// merged over the TextMate theme's editor part styles (`theme.rs`).
fn editor_styles() -> EditorStyles {
    fn part(style: StyleRefinement) -> Option<PartStyle> { Some(PartStyle(Style::from(style))) }
    let underline = |color: AdaptiveColor| part(StyleRefinement::default().text_decoration_color(color));
    let mut styles = super::theme::editor_styles();
    styles.completion_menu = part(StyleRefinement::default().p(px(4.0)).min_w(px(280.0)).max_w(px(520.0))
        .rounded(px(design_tokens::radius_lg())).bg(design_tokens::popover()).border_color(design_tokens::border())
        .shadow_md().shadow_color(design_tokens::shadow()));
    styles.completion_item = part(StyleRefinement::default().h(px(design_tokens::row_sm())).px(px(8.0)).rounded(px(design_tokens::radius_sm()))
        .text_size(px(type_code_caption().size())).text_color(design_tokens::popover_foreground()));
    styles.completion_match = part(StyleRefinement::default().font_weight(FontWeight::SEMIBOLD));
    styles.completion_detail = part(StyleRefinement::default().text_size(px(type_caption().size())).text_color(design_tokens::muted_foreground()));
    styles.completion_kind = part(StyleRefinement::default().text_size(px(type_caption().size())).text_color(design_tokens::muted_foreground()));
    styles.completion_documentation = part(StyleRefinement::default().max_w(px(320.0)).p(px(12.0))
        .rounded(px(design_tokens::radius_lg())).bg(design_tokens::popover()).border_color(design_tokens::border())
        .text_size(px(type_body().size())).text_color(design_tokens::popover_foreground())
        .shadow_md().shadow_color(design_tokens::shadow()));
    if preferences::settings().with(|p| p.text_editing.diagnostics.underline) {
        styles.diagnostic_error = underline(design_tokens::diag_error());
        styles.diagnostic_warning = underline(design_tokens::diag_warning());
        styles.diagnostic_info = underline(design_tokens::diag_info());
        styles.diagnostic_hint = underline(design_tokens::diag_hint());
    } else {
        styles.diagnostic_error = None;
        styles.diagnostic_warning = None;
        styles.diagnostic_info = None;
        styles.diagnostic_hint = None;
    }
    styles
}

// Sync.

/// What has been sent for one document. The version mirrors `Manager::sync`'s
/// counter: 1 on the first sync, one more each time the language or text changed.
#[derive(Debug, Default, Clone, PartialEq)]
struct SyncState {
    /// The protocol version of the last sync.
    version: i32,
    /// A digest of the language and text last sent.
    sent: Option<u64>,
    /// Edits counted so far, and the count when the last sync went out.
    edits: u64,
    synced: u64,
}

impl SyncState {
    /// Counts an edit; the debounce syncs only if no other came after it.
    fn edited(&mut self) -> u64 {
        self.edits += 1;
        self.edits
    }

    /// Records a sync of `text` as `language` and returns its version.
    fn record(&mut self, language: &str, text: &str) -> i32 {
        let digest = digest(language, text);
        if self.sent != Some(digest) {
            self.version += 1;
            self.sent = Some(digest);
        }
        self.synced = self.edits;
        self.version
    }

    /// Whether the editor holds edits the server has not heard yet.
    fn pending(&self) -> bool { self.edits != self.synced }

    /// Whether diagnostics computed for `version` still describe the text:
    /// none for an older version, and none while newer edits wait.
    fn accepts(&self, version: Option<i32>) -> bool {
        self.sent.is_some() && !self.pending() && version.is_none_or(|version| version >= self.version)
    }
}

fn digest(language: &str, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    language.hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// What the worker does, in order.
enum Command {
    Sync { uri: String, language: String, text: String, done: Option<oneshot::Sender<()>> },
    Close(String),
}

fn send(command: Command) {
    if let Some(sender) = lsp(|lsp| lsp.sender.clone()) { let _ = sender.send(command); }
}

async fn worker(manager: Manager, mut commands: mpsc::UnboundedReceiver<Command>) {
    while let Some(command) = commands.recv().await {
        match command {
            Command::Sync { uri, language, text, done } => {
                if let Err(error) = manager.sync(&uri, &language, text).await {
                    tracing::warn!("language server sync of {uri}: {error}");
                }
                if let Some(done) = done { let _ = done.send(()); }
            }
            Command::Close(uri) => manager.close(&uri).await,
        }
    }
}

/// The URI a document is known by: its file, or `untitled:<id>`, which the
/// manager gives a file in a virtual workspace.
fn uri_of(id: DocumentId, path: Option<PathBuf>) -> String {
    match path {
        Some(path) => scratch_lsp::path_to_uri(&path),
        None => format!("untitled:{id}"),
    }
}

/// The catalogue id of the server for `language`, when one is enabled.
fn server_key(language: &str) -> Option<String> {
    ServerCatalog::builtin().server_for_language(language).filter(|server| server.enabled).map(|server| server.id.clone())
}

/// Sends the document as the editor has it now, and returns what a request
/// needs once the sync has landed. `None` when no server serves the language.
fn sync_now(id: DocumentId) -> Option<(oneshot::Receiver<()>, Manager, String)> {
    let (language, path) = with(|w| w.doc(id).map(|doc| (doc.document.language.clone(), state::saved_path(&doc.document))))?;
    let text = state::text(id)?;
    let uri = uri_of(id, path);
    let key = server_key(&language);
    let (renamed, manager, served) = lsp(|lsp| {
        let served = key.as_ref().is_some_and(|key| !lsp.quiet.contains(key));
        let doc = lsp.docs.entry(id).or_insert_with(|| Doc {
            uri: uri.clone(),
            language: language.clone(),
            sync: SyncState::default(),
            completions: Vec::new(),
            diagnostics: Vec::new(),
        });
        // Save As gives the document a new URI: the old one closes and versions start again.
        let renamed = (doc.uri != uri).then(|| std::mem::replace(&mut doc.uri, uri.clone()));
        if renamed.is_some() { doc.sync = SyncState::default(); }
        doc.language = language.clone();
        if served { doc.sync.record(&language, &text); } else { doc.sync.synced = doc.sync.edits; }
        (renamed, lsp.manager.clone(), served)
    });
    if let Some(old) = renamed {
        send(Command::Close(old));
        global::<Diagnostics>().update(|all| { all.0.remove(&id); });
    }
    if !served { return None; }
    let (done, landed) = oneshot::channel();
    send(Command::Sync { uri: uri.clone(), language, text, done: Some(done) });
    Some((landed, manager?, uri))
}

// Events.

async fn pump(mut events: mpsc::UnboundedReceiver<ManagerEvent>) {
    while let Some(event) = events.recv().await {
        match event {
            ManagerEvent::Diagnostics { uri, version, diagnostics } => {
                let target = lsp(|lsp| lsp.docs.iter().find(|(_, doc)| doc.uri == uri).filter(|(_, doc)| doc.sync.accepts(version)).map(|(id, _)| *id));
                let Some(id) = target else { continue; };
                lsp(|lsp| {
                    if let Some(doc) = lsp.docs.get_mut(&id) {
                        doc.diagnostics = diagnostics.clone();
                    }
                });
                let converted = diagnostics.iter().map(|diagnostic| to_editor_diagnostic(diagnostic, &uri)).collect();
                global::<Diagnostics>().update(|all| { all.0.insert(id, converted); });
            }
            ManagerEvent::Status { key, name, status } => {
                let quiet = matches!(status, ServerStatus::NotInstalled { .. } | ServerStatus::Disabled);
                let was_quiet = lsp(|lsp| if quiet { !lsp.quiet.insert(key.clone()) } else { lsp.quiet.remove(&key); false });
                let (badge, detail, ready, is_error) = match &status {
                    ServerStatus::Disabled => ("Disabled".to_owned(), None, false, false),
                    ServerStatus::NotInstalled { hint } => ("Not Installed".to_owned(), Some(hint.clone()), false, false),
                    ServerStatus::Starting => ("Starting…".to_owned(), None, false, false),
                    ServerStatus::Ready { version } => ("Ready".to_owned(), version.clone().map(|v| format!("Version {v}")), true, false),
                    ServerStatus::Error { message } => ("Error".to_owned(), Some(first_line(message).to_owned()), false, true),
                    ServerStatus::Stopped { reason } => ("Stopped".to_owned(), Some(first_line(reason).to_owned()), false, false),
                };
                global::<ServerStatuses>().update(|all| {
                    if let Some(entry) = all.0.get_mut(&key) {
                        entry.name = name.clone();
                        entry.badge = badge;
                        entry.detail = detail;
                        entry.ready = ready;
                        entry.error = is_error;
                    } else {
                        all.0.insert(key.clone(), ServerStatusEntry {
                            key: key.clone(),
                            name: name.clone(),
                            languages: String::new(),
                            badge,
                            detail,
                            ready,
                            error: is_error,
                        });
                    }
                });
                if let Some(message) = status_notice(&name, &status).filter(|_| !was_quiet) { state::notice(message); }
                if status.is_ready() { refresh_triggers(&key); }
            }
            ManagerEvent::Message { name, level, message, .. } => {
                if matches!(level, MessageLevel::Error) { state::notice(format!("{name}: {}", first_line(&message))); }
            }
        }
    }
}

/// The status bar's words for a server's state, when it is worth saying.
fn status_notice(name: &str, status: &ServerStatus) -> Option<String> {
    Some(match status {
        ServerStatus::Disabled | ServerStatus::NotInstalled { .. } => return None,
        ServerStatus::Starting => format!("Starting {name}…"),
        ServerStatus::Ready { .. } => format!("{name} is ready."),
        ServerStatus::Error { message } => format!("{name} could not start: {}", first_line(message)),
        ServerStatus::Stopped { reason } => format!("{name} stopped ({}). Restarting soon.", first_line(reason)),
    })
}

fn first_line(text: &str) -> &str { text.lines().next().unwrap_or_default() }

/// Asks a server that became ready for its documents' trigger characters.
fn refresh_triggers(key: &str) {
    let (manager, docs) = lsp(|lsp| {
        let docs: Vec<_> = lsp.docs.iter().filter(|(_, doc)| server_key(&doc.language).as_deref() == Some(key)).map(|(id, doc)| (*id, doc.uri.clone())).collect();
        (lsp.manager.clone(), docs)
    });
    let Some(manager) = manager else { return; };
    for (id, uri) in docs { spawn(fetch_triggers(id, manager.clone(), uri)); }
}

async fn fetch_triggers(id: DocumentId, manager: Manager, uri: String) {
    let Ok(found) = background(async move { manager.trigger_characters(&uri).await }).await else { return; };
    let characters: Vec<SharedString> = found.completion.into_iter().map(SharedString::from).collect();
    if lsp(|lsp| lsp.docs.contains_key(&id)) { global::<Triggers>().update(|all| { all.0.insert(id, characters); }); }
}

// Completion.

fn complete_focused() {
    let Some(id) = global::<Focused>().get_untracked().0 else { return; };
    if let Some(api) = state::editor_api(id) { api.show_completions(); }
}

fn completion_requested(id: DocumentId, request: EditorCompletionRequest) {
    spawn(async move {
        let synced = sync_now(id);
        let mut items = None;
        if let Some((landed, manager, uri)) = synced {
            let _ = landed.await;
            let position = Position::new(request.position.line, request.position.column);
            let trigger = match (&request.trigger, &request.character) {
                (EditorCompletionTrigger::Character, Some(character)) => CompletionTrigger::Character(character.clone()),
                _ => CompletionTrigger::Invoked,
            };
            let character = request.character.clone();
            let answer = background(async move { manager.completion(&uri, position, trigger, character).await }).await;
            if let Ok(Ok(list)) = answer { items = Some((list.items, list.is_incomplete)); }
        }
        // No server, or it failed: words of the document and the language's keywords.
        let (items, incomplete) = match items {
            Some(found) => found,
            None => (fallback(id, &request).await, false),
        };
        let Some(api) = state::editor_api(id) else { return; };
        if items.is_empty() { api.hide_completions(); return; }
        let start = query_start(&request);
        let shown = items.iter().take(COMPLETION_LIMIT).map(|item| to_editor_item(item, request.position, start)).collect();
        lsp(|lsp| { if let Some(doc) = lsp.docs.get_mut(&id) { doc.completions = items.into_iter().take(COMPLETION_LIMIT).collect(); } });
        api.present_completions(request.id, shown, Some(incomplete));
    });
}

async fn fallback(id: DocumentId, request: &EditorCompletionRequest) -> Vec<CompletionItem> {
    let Some(text) = state::text(id) else { return Vec::new(); };
    let Some((language, path)) = with(|w| w.doc(id).map(|doc| (doc.document.language.clone(), state::saved_path(&doc.document)))) else { return Vec::new(); };
    let directory = path.and_then(|path| path.parent().map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(std::env::temp_dir);
    let caret = Position::new(request.position.line, request.position.column);
    // Path completion reads directories.
    blocking(move || scratch_lsp::fallback_completions(&text, caret, &language, &directory)).await.unwrap_or_default()
}

/// The column the editor's query starts at, on the caret's line.
fn query_start(request: &EditorCompletionRequest) -> EditorPosition {
    let typed = request.offset.saturating_sub(request.query_start);
    EditorPosition::new(request.position.line, request.position.column.saturating_sub(typed))
}

/// A server item as the editor's list shows it. A replace range that is just
/// the query is left to the editor, which keeps it current as typing goes on.
fn to_editor_item(item: &CompletionItem, caret: EditorPosition, query: EditorPosition) -> EditorCompletionItem {
    let mut out = EditorCompletionItem::new(item.label.clone());
    out.kind = item.kind.map(editor_kind);
    out.detail = item.label_detail.clone().or_else(|| item.detail.clone()).map(SharedString::from);
    out.documentation = item.documentation.as_ref().map(RichText::to_plain).filter(|text| !text.is_empty()).map(SharedString::from);
    out.filter_text = item.filter_text.clone().map(SharedString::from);
    out.sort_text = item.sort_text.clone().map(SharedString::from);
    let (range, text) = match &item.text_edit {
        Some(edit) => (Some(editor_range(edit.range)), edit.new_text.clone()),
        None => (None, item.insert_text.clone().unwrap_or_else(|| item.label.clone())),
    };
    // The editor inserts plain text; a snippet's placeholders become their defaults.
    let text = match item.insert_format {
        InsertFormat::Snippet => scratch_lsp::expand_snippet(&text).text,
        InsertFormat::PlainText => text,
    };
    out.range = range.filter(|range| !(range.start == query && range.end == caret));
    if text != item.label || out.range.is_some() { out.insert_text = Some(SharedString::from(text)); }
    out
}

fn editor_kind(kind: CompletionKind) -> EditorCompletionKind {
    use CompletionKind as L;
    use EditorCompletionKind as K;
    match kind {
        L::Text => K::Text, L::Method => K::Method, L::Function => K::Function, L::Constructor => K::Constructor,
        L::Field => K::Field, L::Variable => K::Variable, L::Class => K::Class, L::Interface => K::Interface,
        L::Module => K::Module, L::Property => K::Property, L::Unit => K::Unit, L::Value => K::Value,
        L::Enum => K::Enum, L::Keyword => K::Keyword, L::Snippet => K::Snippet, L::Color => K::Color,
        L::File => K::File, L::Reference => K::Reference, L::Folder => K::Folder, L::EnumMember => K::EnumMember,
        L::Constant => K::Constant, L::Struct => K::Struct, L::Event => K::Event, L::Operator => K::Operator,
        L::TypeParameter => K::TypeParameter,
    }
}

/// Applies an accepted item's additional edits (an import at the top), asking
/// the server for them when the list left them out. They are applied only if
/// the lines they touch have not changed since.
fn completion_accepted(id: DocumentId, shown: &EditorCompletionItem) {
    let found = lsp(|lsp| {
        let doc = lsp.docs.get(&id)?;
        doc.completions.iter().find(|item| item.label == shown.label.as_ref()).cloned().map(|item| (item, doc.uri.clone(), lsp.manager.clone()))
    });
    let Some((item, uri, manager)) = found else { return; };
    spawn(async move {
        // The text as the accepted item left it.
        let Some(before) = state::text(id) else { return; };
        let edits = if !item.additional_edits.is_empty() || item.raw.is_none() {
            item.additional_edits
        } else {
            let Some(manager) = manager else { return; };
            match background(async move { manager.resolve_completion(&uri, &item).await }).await {
                Ok(Ok(resolved)) => resolved.additional_edits,
                _ => return,
            }
        };
        let Some(last) = edits.iter().map(|edit| edit.range.end.line).max() else { return; };
        let (Some(api), Some(now)) = (state::editor_api(id), state::text(id)) else { return; };
        if !lines_unchanged(&before, &now, last) { return; }
        api.edit(edits.iter().map(editor_edit).collect());
    });
}

/// Whether lines `0..=last` are the same in both texts.
fn lines_unchanged(before: &str, now: &str, last: u32) -> bool {
    let take = last as usize + 1;
    before.split('\n').take(take).eq(now.split('\n').take(take))
}

// Hover.

fn hover_requested(id: DocumentId, event: EditorHoverEvent) {
    // Mod held asks for a definition link, not a hover.
    if event.modifiers.cmd { return; }
    let sequence = lsp(|lsp| {
        lsp.hover += 1;
        lsp.last_anchor = Some(event.page_bounds);
        lsp.hover
    });
    let problems: Vec<_> = event.diagnostics.iter().map(|diagnostic| (diagnostic.severity, diagnostic.message.to_string())).collect();
    let mut initial_blocks = Vec::new();
    if let Some((expr, val)) = super::debugger::hover_value(id, event.position) {
        initial_blocks.push(Block::CodeBlock {
            language: None,
            code: format!("{expr} = {val}"),
        });
    }
    let card = HoverCard { open: true, document: Some(id), anchor: event.page_bounds, problems, blocks: initial_blocks.clone() };
    if !card.is_empty() { global::<HoverCard>().set(card.clone()); }
    let position = Position::new(event.position.line, event.position.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else { return; };
        let _ = landed.await;
        let answer = background(async move { manager.hover(&uri, position).await }).await;
        if lsp(|lsp| lsp.hover) != sequence { return; }
        let Ok(Ok(Some(hover))) = answer else { return; };
        let mut combined = initial_blocks;
        combined.extend(clip(hover.contents, HOVER_LIMIT));
        if combined.is_empty() && card.problems.is_empty() { return; }
        global::<HoverCard>().set(HoverCard { blocks: combined, ..card });
    });
}

// ------------------------------------------------ Go to Definition / Signature / Actions / Rename / Format

fn definition_requested(id: DocumentId, request: EditorDefinitionRequest) {
    let seq = if request.trigger == EditorDefinitionTrigger::Hover {
        lsp(|lsp| { lsp.definition_hover += 1; lsp.definition_hover })
    } else {
        0
    };
    let pos = Position::new(request.position.line, request.position.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            if request.trigger == EditorDefinitionTrigger::Key {
                state::notice("No active language server for this document.");
            }
            return;
        };
        let _ = landed.await;
        let uri_clone = uri.clone();
        let Ok(Ok(def)) = background(async move { manager.definition(&uri_clone, pos).await }).await else {
            if request.trigger == EditorDefinitionTrigger::Key {
                state::notice("No definition found.");
            }
            return;
        };
        let Some(first) = def.targets.into_iter().next() else {
            if request.trigger == EditorDefinitionTrigger::Key {
                state::notice("No definition found.");
            }
            return;
        };
        match request.trigger {
            EditorDefinitionTrigger::Hover => {
                if lsp(|lsp| lsp.definition_hover) != seq { return; }
                if let Some(api) = state::editor_api(id) {
                    let link_range = def.origin.map(editor_range).unwrap_or(request.range);
                    api.present_definition_link(request.id, link_range);
                }
            }
            EditorDefinitionTrigger::Press | EditorDefinitionTrigger::Key => {
                jump_to_location(id, &uri, first).await;
            }
        }
    });
}

/// Navigates to the definition of the symbol under the caret (`F12`).
pub fn go_to_definition() {
    let Some(id) = with(|w| w.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
    let pos = Position::new(caret.line, caret.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            state::notice("No active language server for this document.");
            return;
        };
        let _ = landed.await;
        let uri_clone = uri.clone();
        let Ok(Ok(def)) = background(async move { manager.definition(&uri_clone, pos).await }).await else {
            state::notice("No definition found.");
            return;
        };
        let Some(first) = def.targets.into_iter().next() else {
            state::notice("No definition found.");
            return;
        };
        jump_to_location(id, &uri, first).await;
    });
}

async fn jump_to_location(own_id: DocumentId, own_uri: &str, location: Location) {
    if location.uri == own_uri {
        state::record_jump();
        if let Some(api) = state::editor_api(own_id) {
            let start = editor_position(location.range.start);
            let end = editor_position(location.range.end);
            api.set_selections(vec![EditorSelection::new(start, end)]);
            api.reveal_position(start, true);
        }
        state::focus(own_id);
        return;
    }
    if let Ok(path) = scratch_lsp::uri_to_path(&location.uri) {
        let start = editor_position(location.range.start);
        let end = editor_position(location.range.end);
        super::files::open_path_at_range(path, start, end).await;
    }
}

fn maybe_trigger_signature_help(id: DocumentId) {
    let Some(api) = state::editor_api(id) else { return; };
    let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
    if caret.column == 0 { return; }
    let text = api.value();
    let Some(line) = text.split('\n').nth(caret.line as usize) else { return; };
    let prev_ch = line.encode_utf16()
        .take(caret.column as usize)
        .last()
        .and_then(|u| char::from_u32(u as u32));
    if matches!(prev_ch, Some('(' | ',')) {
        signature_help_for(id, caret);
    }
}

/// Requests parameter/signature hints at the caret (`⇧⌘Space` or typing `(` / `,`).
pub fn signature_help() {
    let Some(id) = with(|w| w.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
    signature_help_for(id, caret);
}

fn signature_help_for(id: DocumentId, caret: EditorPosition) {
    let sequence = lsp(|lsp| { lsp.hover += 1; lsp.hover });
    let anchor = lsp(|lsp| lsp.last_anchor).unwrap_or_else(|| Bounds::new(point(px(280.0), px(110.0)), size(px(8.0), px(20.0))));
    let pos = Position::new(caret.line, caret.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else { return; };
        let _ = landed.await;
        let Ok(Ok(Some(help))) = background(async move { manager.signature_help(&uri, pos).await }).await else { return; };
        if lsp(|lsp| lsp.hover) != sequence { return; }
        let Some(sig) = help.signatures.get(help.active_signature).or_else(|| help.signatures.first()) else { return; };
        let mut blocks = vec![Block::CodeBlock { language: None, code: sig.label.clone() }];
        if let Some(idx) = help.current_parameter()
            && let Some(param) = sig.parameters.get(idx)
        {
            let mut detail = format!("Active parameter: {}", param.label);
            if let Some(doc) = &param.documentation {
                let plain = doc.to_plain();
                if !plain.is_empty() {
                    detail.push_str(" — ");
                    detail.push_str(&plain);
                }
            }
            blocks.push(Block::Paragraph(vec![scratch_lsp::Inline::Text(detail)]));
        }
        if let Some(doc) = &sig.documentation {
            blocks.extend(clip(doc.clone(), HOVER_LIMIT / 2));
        }
        global::<HoverCard>().set(HoverCard {
            open: true,
            document: Some(id),
            anchor,
            problems: Vec::new(),
            blocks,
        });
    });
}

fn ranges_overlap(a: Range, b: Range) -> bool {
    !(a.end < b.start || b.end < a.start)
}

fn code_action_requested(id: DocumentId, request: EditorCodeActionRequest) {
    let range = Range::new(
        request.range.start.line,
        request.range.start.column,
        request.range.end.line,
        request.range.end.column,
    );
    let diags: Vec<Diagnostic> = lsp(|lsp| {
        lsp.docs.get(&id).map(|doc| {
            doc.diagnostics.iter().filter(|d| ranges_overlap(d.range, range)).cloned().collect()
        }).unwrap_or_default()
    });
    if let Some(b) = request.bounds {
        lsp(|lsp| { lsp.last_anchor = Some(b.page_bounds); });
    }
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            state::notice("No language server active for code actions.");
            return;
        };
        let _ = landed.await;
        let uri_clone = uri.clone();
        let manager_req = manager.clone();
        let Ok(Ok(actions)) = background(async move { manager_req.code_actions(&uri_clone, range, &diags).await }).await else {
            state::notice("No code actions available.");
            return;
        };
        if actions.is_empty() {
            state::notice("No code actions available at caret.");
            return;
        }
        let items: Vec<MenuItem> = actions.into_iter().map(|action| {
            let title = action.title.clone();
            let disabled = action.disabled_reason.is_some();
            let mgr = manager.clone();
            let doc_uri = uri.clone();
            MenuItem::new(title).disabled(disabled).on_select(move || {
                let mgr = mgr.clone();
                let doc_uri = doc_uri.clone();
                let act: CodeAction = action.clone();
                Owner::detached().run(|| {
                    spawn(async move {
                        let Ok(Ok(applied)) = background(async move { mgr.apply_code_action(&doc_uri, &act).await }).await else {
                            state::notice("Could not apply code action.");
                            return;
                        };
                        apply_applied_edit(applied);
                    });
                });
            })
        }).collect();
        if let Some(bounds) = request.bounds {
            drop(popup_menu_at(items, point(bounds.page_bounds.origin.x, bounds.page_bounds.origin.y + bounds.page_bounds.size.height)));
        } else {
            let _ = popup_menu(items).await;
        }
    });
}

fn resolve_prepare_rename(
    prep: std::result::Result<Option<scratch_lsp::PrepareRename>, scratch_lsp::LspError>,
    fallback_word: &str,
) -> Option<String> {
    let candidate = match prep {
        Ok(Some(p)) => p
            .placeholder
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback_word.to_owned()),
        Err(scratch_lsp::LspError::Unsupported(_)) if !fallback_word.is_empty() => {
            fallback_word.to_owned()
        }
        _ => return None,
    };
    (!candidate.is_empty()).then_some(candidate)
}

/// Opens the Rename Symbol sheet (`F2`) for the identifier at the caret.
pub fn rename_symbol() {
    let Some(id) = with(|w| w.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
    let word = {
        let selected = api.selected_text().trim().to_owned();
        if !selected.is_empty() {
            selected
        } else {
            let text = api.value();
            text.split('\n')
                .nth(caret.line as usize)
                .and_then(|line| super::find::word_at(line, caret.column))
                .unwrap_or_default()
        }
    };
    let pos = Position::new(caret.line, caret.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            state::notice("No language server active for rename.");
            return;
        };
        let _ = landed.await;
        let Ok(prep) = background(async move { manager.prepare_rename(&uri, pos).await }).await else {
            state::notice("This symbol cannot be renamed.");
            return;
        };
        let Some(initial) = resolve_prepare_rename(prep, &word) else {
            state::notice("This symbol cannot be renamed.");
            return;
        };
        global::<RenameSheet>().set(RenameSheet {
            open: true,
            doc: Some(id),
            position: pos,
            original: initial.clone(),
            draft: initial,
        });
    });
}

fn commit_rename() {
    let sheet = global::<RenameSheet>().with_untracked(|s| s.clone());
    global::<RenameSheet>().update(|s| s.open = false);
    let Some(id) = sheet.doc else { return; };
    let new_name = sheet.draft.trim().to_owned();
    if new_name.is_empty() || new_name == sheet.original { return; }
    let pos = sheet.position;
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            state::notice("No language server active for rename.");
            return;
        };
        let _ = landed.await;
        let m1 = manager.clone();
        let u1 = uri.clone();
        let ws_edit = match background(async move { m1.rename(&u1, pos, &new_name).await }).await {
            Ok(Ok(edit)) => edit,
            _ => {
                state::notice("Rename is not supported at this symbol.");
                return;
            }
        };
        if ws_edit.is_empty() { return; }
        let m2 = manager.clone();
        if let Ok(Ok(applied)) = background(async move { m2.apply_workspace_edit(&ws_edit).await }).await {
            apply_applied_edit(applied);
        }
    });
}

/// Formats the active document via its language server (`⌃⇧I`).
pub fn format_document() {
    let Some(id) = with(|w| w.active) else { return; };
    let (tab_size, insert_spaces) = preferences::settings().with_untracked(|p| (
        p.text_editing.indentation.tab_width.max(1),
        p.text_editing.indentation.soft_tabs,
    ));
    let options = FormattingOptions {
        tab_size,
        insert_spaces,
        trim_trailing_whitespace: false,
        insert_final_newline: false,
    };
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else {
            state::notice("No language server active for formatting.");
            return;
        };
        let _ = landed.await;
        let edits_before = lsp(|lsp| lsp.docs.get(&id).map(|d| d.sync.edits));
        let Ok(Ok(mut edits)) = background(async move { manager.format(&uri, options).await }).await else {
            state::notice("Formatting is not supported by this language server.");
            return;
        };
        if edits.is_empty() { return; }
        let edits_after = lsp(|lsp| lsp.docs.get(&id).map(|d| d.sync.edits));
        if edits_before != edits_after {
            return;
        }
        edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
        if let Some(api) = state::editor_api(id) {
            api.edit(edits.iter().map(editor_edit).collect());
        }
    });
}

fn apply_applied_edit(applied: AppliedEdit) {
    let uri_map: Vec<(DocumentId, String, Option<PathBuf>)> = with(|w| {
        w.documents.iter().map(|d| (d.document.id, String::new(), state::saved_path(&d.document))).collect()
    });
    for file_edit in applied.edits {
        let matched = lsp(|lsp| {
            lsp.docs.iter().find(|(_, d)| d.uri == file_edit.uri).map(|(id, _)| *id)
        }).or_else(|| {
            let target_path = scratch_lsp::uri_to_path(&file_edit.uri).ok()?;
            uri_map.iter().find(|(_, _, p)| p.as_ref() == Some(&target_path)).map(|(id, _, _)| *id)
        });
        let Some(doc_id) = matched else { continue; };
        let Some(api) = state::editor_api(doc_id) else { continue; };
        let new_text = file_edit.new_text.clone();
        if !file_edit.edits.is_empty() {
            let mut edits = file_edit.edits;
            edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
            api.edit(edits.iter().map(editor_edit).collect());
        } else {
            let cur = api.value();
            if cur != new_text {
                api.set_value(new_text.clone());
            }
        }
        lsp(|lsp| {
            if let Some(doc) = lsp.docs.get_mut(&doc_id) {
                doc.sync.record(&doc.language, &new_text);
            }
        });
    }
}

/// Opens the LSP Server Status modal sheet (Diagnostics > LSP Server Status…).
pub fn open_status_sheet() {
    global::<StatusSheetOpen>().set(StatusSheetOpen(true));
}

fn rename_sheet_view() -> impl IntoElement {
    let sheet = global::<RenameSheet>();
    let close = || global::<RenameSheet>().update(|s| s.open = false);
    nsh_base::dialog()
        .animation(ModalAnimation::None)
        .label("Rename Symbol")
        .on_cancel(close)
        .flex_1()
        .child(view().absolute().inset_0().bg(design_tokens::scrim).on_press(close))
        .child(view().flex_1().items_center().pt(px(56.0)).child(
            view()
                .w(px(360.0))
                .p(px(18.0))
                .gap(px(10.0))
                .rounded(px(design_tokens::radius_xl()))
                .bg(design_tokens::background)
                .border_1()
                .border_color(design_tokens::border)
                .shadow_lg()
                .shadow_color(design_tokens::shadow())
                .child(type_subhead().semibold().apply(text("Rename Symbol")).text_color(design_tokens::foreground))
                .child(input()
                    .value(move || sheet.with(|s| s.draft.clone()))
                    .auto_focus(true)
                    .placeholder("New symbol name")
                    .on_change(move |v: &str| sheet.update(|s| s.draft = v.to_owned()))
                    .on_submit(move |_: &str| commit_rename())
                    .h(px(28.0)).px_2().text_sm()
                    .font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                    .bg(design_tokens::field).text_color(design_tokens::foreground)
                    .border_1().border_color(design_tokens::input).rounded(px(design_tokens::radius())))
                .child(view().flex_row().justify_end().gap(px(8.0)).pt(px(4.0))
                    .child(button("Cancel", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), close))
                    .child(button("Rename", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Sm), commit_rename))),
        ))
}

fn status_sheet_view() -> impl IntoElement {
    let close = || global::<StatusSheetOpen>().set(StatusSheetOpen(false));
    nsh_base::dialog()
        .animation(ModalAnimation::None)
        .label("Language Server Status")
        .on_cancel(close)
        .flex_1()
        .child(view().absolute().inset_0().bg(design_tokens::scrim).on_press(close))
        .child(view().flex_1().items_center().pt(px(44.0)).child(
            view()
                .w(px(520.0))
                .p(px(18.0))
                .gap(px(10.0))
                .rounded(px(design_tokens::radius_xl()))
                .bg(design_tokens::background)
                .border_1()
                .border_color(design_tokens::border)
                .shadow_lg()
                .shadow_color(design_tokens::shadow())
                .child(view().flex_row().items_center().justify_between()
                    .child(type_subhead().semibold().apply(text("Language Server Status")).text_color(design_tokens::foreground))
                    .child(type_caption().apply(text(move || {
                        let count = global::<ServerStatuses>().with(|s| s.0.values().filter(|e| e.ready).count());
                        format!("{count} running")
                    })).text_color(design_tokens::muted_foreground)))
                .child(scroll().max_h(px(320.0)).gap(px(6.0)).child(dynamic(move || {
                    let entries: Vec<ServerStatusEntry> = global::<ServerStatuses>()
                        .with(|all| all.0.values().cloned().collect());
                    view().flex_col().gap(px(6.0)).children(entries.into_iter().map(|entry| {
                        let tone = if entry.ready {
                            Tone::Success
                        } else if entry.error {
                            Tone::Danger
                        } else {
                            Tone::Muted
                        };
                        let icon = if entry.ready {
                            IconName::SuccessCircle
                        } else if entry.error {
                            IconName::ErrorCircle
                        } else {
                            IconName::InfoCircle
                        };
                        let detail = entry.detail.clone();
                        let has_detail = detail.is_some();
                        view().p(px(8.0)).gap(px(4.0)).rounded(px(design_tokens::radius())).bg(design_tokens::card)
                            .border_1().border_color(design_tokens::border)
                            .child(view().flex_row().items_center().gap(px(6.0))
                                .child(glyph(icon, IconSize::Xs, tone))
                                .child(type_body().medium().apply(text(entry.name)).text_color(design_tokens::foreground))
                                .child(type_caption().apply(text(entry.languages)).flex_1().min_w(px(0.0)).text_color(design_tokens::muted_foreground))
                                .child(badge(SharedString::from(entry.badge), BadgeOptions::new())))
                            .child(when(move || has_detail, {
                                let d = detail.unwrap_or_default();
                                move || type_caption().apply(text(d.clone())).select_text().text_color(design_tokens::muted_foreground)
                            }))
                    })).into_element()
                })))
                .child(view().flex_row().justify_end().pt(px(4.0))
                    .child(button("Done", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Sm), close))),
        ))
}

fn hide_hover() {
    lsp(|lsp| lsp.hover += 1);
    let card = global::<HoverCard>();
    if card.with_untracked(|card| card.open) { card.update(|card| card.open = false); }
}

/// The first blocks of `text`, about `limit` characters of them.
fn clip(text: RichText, limit: usize) -> Vec<Block> {
    let mut used = 0;
    let mut blocks = Vec::new();
    for block in text.blocks {
        if used >= limit { break; }
        used += RichText { blocks: vec![block.clone()] }.to_plain().chars().count();
        blocks.push(block);
    }
    blocks
}

fn block_view(block: Block) -> AnyElement {
    let foreground = move || design_tokens::popover_foreground();
    let plain = |content: &[scratch_lsp::Inline]| scratch_lsp::markup::inlines_to_plain(content);
    match block {
        Block::CodeBlock { code, .. } => view().px_2().py_1().rounded(px(design_tokens::radius_sm())).bg(design_tokens::muted())
            .child(type_code_caption().apply(text(code)).text_color(foreground)).into_element(),
        Block::Heading { content, .. } => type_body().semibold().apply(text(plain(&content))).text_color(foreground).into_element(),
        Block::Paragraph(content) => type_body().apply(text(plain(&content))).text_color(foreground).into_element(),
        Block::Quote(content) => type_body().apply(text(plain(&content))).text_color(design_tokens::muted_foreground).into_element(),
        Block::List { ordered, items } => view().gap_1().children(items.iter().enumerate().map(|(index, item)| {
            let marker = if ordered { format!("{}.", index + 1) } else { "•".to_owned() };
            type_body().apply(text(format!("{marker} {}", plain(item)))).text_color(foreground)
        })).into_element(),
        Block::Rule => separator(SeparatorOrientation::Horizontal).into_element(),
    }
}

fn severity_label(severity: EditorDiagnosticSeverity) -> &'static str {
    match severity {
        EditorDiagnosticSeverity::Error => "Error",
        EditorDiagnosticSeverity::Warning => "Warning",
        EditorDiagnosticSeverity::Info => "Information",
        EditorDiagnosticSeverity::Hint => "Hint",
    }
}

/// The severity icons of DESIGN 8.6.2.
fn severity_glyph(severity: EditorDiagnosticSeverity) -> (IconName, Tone) {
    match severity {
        EditorDiagnosticSeverity::Error => (IconName::ErrorCircle, Tone::Danger),
        EditorDiagnosticSeverity::Warning => (IconName::WarningTriangle, Tone::Warning),
        EditorDiagnosticSeverity::Info => (IconName::InfoCircle, Tone::Info),
        EditorDiagnosticSeverity::Hint => (IconName::InfoCircle, Tone::Muted),
    }
}

// Conversions. Both sides count UTF-16 code units on 0-based lines, so a
// position maps field to field.

fn editor_position(position: Position) -> EditorPosition { EditorPosition::new(position.line, position.character) }

fn editor_range(range: Range) -> EditorRange { EditorRange::new(editor_position(range.start), editor_position(range.end)) }

fn editor_edit(edit: &TextEdit) -> EditorEdit { EditorEdit::new(editor_range(edit.range), edit.new_text.clone()) }

fn to_editor_diagnostic(diagnostic: &Diagnostic, own_uri: &str) -> EditorDiagnostic {
    let mut out = EditorDiagnostic::default();
    out.range = editor_range(diagnostic.range);
    out.severity = match diagnostic.severity {
        Severity::Error => EditorDiagnosticSeverity::Error,
        Severity::Warning => EditorDiagnosticSeverity::Warning,
        Severity::Information => EditorDiagnosticSeverity::Info,
        Severity::Hint => EditorDiagnosticSeverity::Hint,
    };
    out.message = SharedString::from(diagnostic.message.clone());
    out.source = diagnostic.source.clone().map(SharedString::from);
    out.code = diagnostic.code.clone().map(SharedString::from);
    out.code_description = diagnostic.code_href.clone().map(EditorDiagnosticCodeDescription::new);
    if diagnostic.unnecessary { out.tags.push(EditorDiagnosticTag::Unnecessary); }
    if diagnostic.deprecated { out.tags.push(EditorDiagnosticTag::Deprecated); }
    out.related_information = diagnostic.related.iter().map(|related| {
        let uri = (related.location.uri != own_uri).then(|| SharedString::from(related.location.uri.clone()));
        EditorDiagnosticRelatedInformation::new(EditorLocation::new(uri, editor_range(related.location.range)), related.message.clone())
    }).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_follow_the_manager_counter() {
        let mut sync = SyncState::default();
        assert_eq!(sync.record("python", "a"), 1);
        assert_eq!(sync.record("python", "a"), 1, "the same text is not a new version");
        sync.edited();
        assert_eq!(sync.record("python", "ab"), 2);
        assert_eq!(sync.record("plain", "ab"), 3, "a language change is a new version");
    }

    #[test]
    fn stale_diagnostics_are_dropped() {
        let mut sync = SyncState::default();
        assert!(!sync.accepts(None), "nothing was sent yet");
        sync.record("python", "a");
        sync.edited();
        sync.record("python", "ab");
        assert!(!sync.accepts(Some(1)), "an older version");
        assert!(sync.accepts(Some(2)));
        assert!(sync.accepts(None), "servers may leave the version out");
        sync.edited();
        assert!(sync.pending());
        assert!(!sync.accepts(Some(2)), "newer edits wait to be synced");
    }

    #[test]
    fn debounce_syncs_only_after_the_last_edit() {
        let mut sync = SyncState::default();
        let first = sync.edited();
        let second = sync.edited();
        assert_ne!(first, sync.edits);
        assert_eq!(second, sync.edits);
        sync.record("python", "x");
        assert!(!sync.pending());
    }

    #[test]
    fn positions_map_field_to_field() {
        let range = editor_range(Range::new(1, 2, 3, 4));
        assert_eq!(range.start, EditorPosition::new(1, 2));
        assert_eq!(range.end, EditorPosition::new(3, 4));
        let edit = editor_edit(&TextEdit { range: Range::new(0, 0, 0, 0), new_text: "import os\n".into() });
        assert_eq!(edit.text.as_ref(), "import os\n");
    }

    #[test]
    fn untitled_documents_get_a_virtual_uri() {
        let id = DocumentId::nil();
        assert_eq!(uri_of(id, None), "untitled:00000000-0000-0000-0000-000000000000");
        assert_eq!(uri_of(id, Some(PathBuf::from("/tmp/a b.py"))), "file:///tmp/a%20b.py");
    }

    #[test]
    fn query_start_counts_back_from_the_caret() {
        let mut request = EditorCompletionRequest::default();
        request.position = EditorPosition::new(2, 7);
        request.offset = 30;
        request.query_start = 27;
        assert_eq!(query_start(&request), EditorPosition::new(2, 4));
    }

    #[test]
    fn completion_items_keep_their_edits_and_expand_snippets() {
        let caret = EditorPosition::new(0, 5);
        let query = EditorPosition::new(0, 3);
        let mut item = CompletionItem { label: "print".into(), kind: Some(CompletionKind::Function), ..CompletionItem::default() };
        item.text_edit = Some(TextEdit { range: Range::new(0, 3, 0, 5), new_text: "print(${1:value})".into() });
        item.insert_format = InsertFormat::Snippet;
        let shown = to_editor_item(&item, caret, query);
        assert_eq!(shown.kind, Some(EditorCompletionKind::Function));
        assert_eq!(shown.range, None, "the query's own range is left to the editor");
        assert_eq!(shown.insert_text.as_deref(), Some("print(value)"));

        item.text_edit = Some(TextEdit { range: Range::new(0, 2, 0, 5), new_text: ".print".into() });
        item.insert_format = InsertFormat::PlainText;
        let shown = to_editor_item(&item, caret, query);
        assert_eq!(shown.range, Some(EditorRange::new(EditorPosition::new(0, 2), caret)));

        let plain = to_editor_item(&CompletionItem { label: "len".into(), ..CompletionItem::default() }, caret, query);
        assert_eq!(plain.insert_text, None);
    }

    #[test]
    fn diagnostics_convert_with_tags_and_related_places() {
        let mut diagnostic = Diagnostic::new(Range::new(0, 0, 0, 3), Severity::Warning, "unused");
        diagnostic.unnecessary = true;
        diagnostic.code = Some("W1".into());
        diagnostic.related.push(scratch_lsp::RelatedInformation {
            location: scratch_lsp::Location { uri: "untitled:x".into(), range: Range::new(1, 0, 1, 1) },
            message: "here".into(),
        });
        let shown = to_editor_diagnostic(&diagnostic, "untitled:x");
        assert_eq!(shown.severity, EditorDiagnosticSeverity::Warning);
        assert_eq!(shown.tags, vec![EditorDiagnosticTag::Unnecessary]);
        assert_eq!(shown.code.as_deref(), Some("W1"));
        assert_eq!(shown.related_information[0].location.uri, None, "the document's own place");
    }

    #[test]
    fn additional_edits_need_unchanged_lines() {
        assert!(lines_unchanged("import a\nx = 1\n", "import a\nx = 12\n", 0));
        assert!(!lines_unchanged("import a\nx\n", "import b\nx\n", 0));
    }

    #[test]
    fn hover_text_is_clipped() {
        let text = RichText::markdown(&"word ".repeat(10).replace(' ', "\n\n"));
        assert!(clip(text, 8).len() < 10);
    }

    #[test]
    fn prepare_rename_respects_rejection_and_unsupported() {
        assert_eq!(
            resolve_prepare_rename(Ok(None), "if"),
            None,
            "server rejecting prepareRename must abort rename"
        );
        assert_eq!(
            resolve_prepare_rename(
                Err(scratch_lsp::LspError::Server {
                    code: -32600,
                    message: "cannot rename".into()
                }),
                "foo"
            ),
            None
        );
        assert_eq!(
            resolve_prepare_rename(
                Err(scratch_lsp::LspError::Unsupported("prepareRename".into())),
                "foo"
            ),
            Some("foo".to_owned())
        );
        assert_eq!(
            resolve_prepare_rename(
                Ok(Some(scratch_lsp::PrepareRename {
                    range: Range::new(0, 0, 0, 3),
                    placeholder: Some("bar".into()),
                })),
                "foo"
            ),
            Some("bar".to_owned())
        );
    }

    #[test]
    fn sync_state_record_after_edit_clears_pending() {
        let mut sync = SyncState::default();
        sync.record("rust", "let x = 1;");
        sync.edited();
        assert!(sync.pending());
        let ver = sync.record("rust", "let y = 1;");
        assert!(!sync.pending());
        assert!(sync.accepts(Some(ver)));
    }
}
