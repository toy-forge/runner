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
    EditorCompletionAcceptEvent, EditorEdit, EditorCompletionItem, EditorCompletionKind,
    EditorCompletionRequest, EditorCompletionTrigger,
};
use nsh_base::text_view::PartStyle;
use nsh_base::{
    EditorDiagnostic, EditorDiagnosticCodeDescription, EditorDiagnosticRelatedInformation,
    EditorDiagnosticSeverity, EditorDiagnosticTag, EditorElement, EditorHoverEvent,
    EditorLocation, EditorPosition, EditorRange, EditorStyles, PopoverEvent, popover,
};
use nsh_core::element::{Style, StyleRefinement};
use nsh_core::overlay::Placement;
use nsh_core::PointerEvents;
use scratch_ui_kit::prelude::*;
use scratch_document::DocumentId;
use scratch_lsp::{
    Block, CompletionItem, CompletionKind, CompletionTrigger, Diagnostic, InsertFormat, Manager,
    ManagerConfig, ManagerEvent, MessageLevel, Position, Range, RichText, ServerCatalog,
    ServerStatus, Severity, TextEdit,
};
use tokio::sync::{mpsc, oneshot};

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
}

#[derive(Default)]
struct Lsp {
    sender: Option<mpsc::UnboundedSender<Command>>,
    manager: Option<Manager>,
    docs: HashMap<DocumentId, Doc>,
    /// Servers that are not installed or are turned off: their documents are not synced.
    quiet: HashSet<String>,
    hover: u64,
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

// Public hooks.

/// Starts the manager and its event pump, and binds Ctrl-Space and Option-Escape
/// to completion. Called once from the root.
pub fn install() {
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
        if global::<Focused>().get().0.is_none() { return; }
        super::bindings::on("text.showCompletions", complete_focused);
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
    spawn(async move {
        let Some(edits) = edits else { sync_now(id); return; };
        timer(SYNC_DELAY).await;
        let due = lsp(|lsp| lsp.docs.get(&id).is_some_and(|doc| doc.sync.edits == edits));
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
    if global::<Focused>().with_untracked(|focused| focused.0 == Some(id)) { global::<Focused>().set(Focused(None)); }
}

/// The LSP diagnostics of a document (for the Diagnostics menu's merged
/// navigation; console annotations live in `doc.annotations`).
pub fn diagnostics(id: DocumentId) -> Vec<EditorDiagnostic> {
    global::<Diagnostics>().with(|all| all.0.get(&id).cloned().unwrap_or_default())
}

/// Wires an editor to its document's language server: completion as a word or
/// a trigger character is typed, the hover card, and the diagnostics.
pub fn attach(editor: EditorElement, id: DocumentId) -> EditorElement {
    let diagnostics = computed(move || {
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
        .on_selection_change(|_: &Vec<nsh_base::EditorSelection>| hide_hover())
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

/// The hover card, mounted once at the root: a popover (DESIGN 7.20, 8.6.3)
/// placed against the hovered word through an invisible trigger laid over it.
pub fn overlay() -> impl IntoElement {
    let card = global::<HoverCard>();
    let anchor = move || card.with(|card| card.anchor);
    nsh_core::prelude::overlay()
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
            .popup_style(|s| s.max_w(px(320.0)).p(px(12.0)).gap_2().rounded(px(design_tokens::radius_lg()))
                .bg(design_tokens::popover()).border_1().border_color(design_tokens::border())
                .shadow_md().shadow_color(design_tokens::shadow()))
            .popup_starting_style(motion::popup_starting)
            .popup_ending_style(motion::popup_ending)
            .trigger(view().w_full().h_full().focusable(false))
            .child(hover_content(card)))
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
    styles.diagnostic_error = underline(design_tokens::diag_error());
    styles.diagnostic_warning = underline(design_tokens::diag_warning());
    styles.diagnostic_info = underline(design_tokens::diag_info());
    styles.diagnostic_hint = underline(design_tokens::diag_hint());
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
        let doc = lsp.docs.entry(id).or_insert_with(|| Doc { uri: uri.clone(), language: language.clone(), sync: SyncState::default(), completions: Vec::new() });
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
                let converted = diagnostics.iter().map(|diagnostic| to_editor_diagnostic(diagnostic, &uri)).collect();
                global::<Diagnostics>().update(|all| { all.0.insert(id, converted); });
            }
            ManagerEvent::Status { key, name, status } => {
                let quiet = matches!(status, ServerStatus::NotInstalled { .. } | ServerStatus::Disabled);
                let was_quiet = lsp(|lsp| if quiet { !lsp.quiet.insert(key.clone()) } else { false });
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
        ServerStatus::Disabled => return None,
        ServerStatus::NotInstalled { hint } => format!("{name} is not installed. {hint}"),
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
    let sequence = lsp(|lsp| { lsp.hover += 1; lsp.hover });
    let problems: Vec<_> = event.diagnostics.iter().map(|diagnostic| (diagnostic.severity, diagnostic.message.to_string())).collect();
    let card = HoverCard { open: true, document: Some(id), anchor: event.page_bounds, problems, blocks: Vec::new() };
    if !card.is_empty() { global::<HoverCard>().set(card.clone()); }
    let position = Position::new(event.position.line, event.position.column);
    spawn(async move {
        let Some((landed, manager, uri)) = sync_now(id) else { return; };
        let _ = landed.await;
        let answer = background(async move { manager.hover(&uri, position).await }).await;
        if lsp(|lsp| lsp.hover) != sequence { return; }
        let Ok(Ok(Some(hover))) = answer else { return; };
        let blocks = clip(hover.contents, HOVER_LIMIT);
        if blocks.is_empty() { return; }
        global::<HoverCard>().set(HoverCard { blocks, ..card });
    });
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
}
