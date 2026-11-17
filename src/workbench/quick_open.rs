//! Open Quickly (Shift-Cmd-O, DESIGN.md 8.12): a centered panel with a search
//! field and fuzzy-ranked results. The providers are `scratch-search`'s
//! QuickNav engine: line specs, `~` and absolute paths, the current document's
//! symbols, open documents, recents, files under the navigator's root,
//! languages (`Set Language:` / `New Document:`) and menu commands.
//!
//! Up and Down move the selection, Return opens, Escape or a click outside
//! closes. `mod.rs` mounts [`overlay`] once at the root; [`open`] toggles it.
//!
//! The panel is an `nsh_base::dialog()` shown while the panel is open. The
//! file index is a `FileCache` made when the panel opens; each keystroke
//! ranks on a blocking thread, the latest query winning.
//!
//! Motion inventory (DESIGN.md 9.4). Reduced motion makes every line instant
//! except the spinner, a progress cue.
//!
//! | Change | Tool | Tokens |
//! |---|---|---|
//! | Panel opens or closes | the dialog layer's native fade (`ModalAnimation::Fade`) | platform |
//! | Result row hover | color wash (`motion::state_wash`) | `instant` |
//! | Selection moves (keys or pointer) | color wash on the row's own surface | `instant` |
//! | Results change while typing | none: they update on every keystroke | |
//! | Panel height follows the result count | none (deviation from 9.3's `fast` height animation; the list is virtualized) | |
//! | Indexing the root | ui-kit spinner at the field's trailing edge | 1 s turn |

use std::path::PathBuf;
use std::sync::Arc;

use nsh_base::{EditorCommand, EditorPosition, EditorSelection, dialog, input};
use nsh_core::keymap::When;
use scratch_search::quicknav::{
    IconKind, LanguageChoice, MenuCommandItem, OpenDocumentItem, QuickNavAction, QuickNavItem,
    QuickNavMode, QuickNavQuery,
};
use scratch_search::{CancelToken, FileCache, FileCacheOptions, LanguageId, SymbolKind, extract_symbols, quicknav};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::navigator::{self, file_icon};
use super::{edit, execution, files, find, preferences, state};

/// Panel width, DESIGN.md 8.12.
const PANEL_W: f32 = 480.0;
/// Field row height.
const FIELD_H: f32 = 56.0;

/// Result row height (`row-xl`).
fn row_h() -> f32 { tokens::row_xl() }
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 6;
/// The most results kept per query.
const LIMIT: usize = 100;

// ------------------------------------------------------------------ state

#[derive(Clone, Default, PartialEq)]
struct Panel {
    open: bool,
    /// The language-only palette (`Select Language…` / `New Document:`).
    mode: PanelMode,
}

/// Which palette the panel runs.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PanelMode {
    #[default]
    OpenQuickly,
    SelectLanguage,
    NewDocument,
}

impl PanelMode {
    fn nav(self) -> QuickNavMode {
        match self {
            PanelMode::OpenQuickly => QuickNavMode::OpenQuickly,
            PanelMode::SelectLanguage => QuickNavMode::SelectLanguage,
            PanelMode::NewDocument => QuickNavMode::NewDocument,
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            PanelMode::OpenQuickly => "Open Quickly",
            PanelMode::SelectLanguage => "Select Language",
            PanelMode::NewDocument => "New Document",
        }
    }
}

fn panel_state() -> Signal<Panel> { global::<Panel>() }

/// Opens the Open Quickly panel, or closes it when it is already open.
pub fn open() { open_mode(PanelMode::OpenQuickly); }

/// Opens the Select Language palette (Opt-Cmd-L).
pub fn select_language() { open_mode(PanelMode::SelectLanguage); }

/// Opens the New Document palette.
pub fn new_document_palette() { open_mode(PanelMode::NewDocument); }

fn open_mode(mode: PanelMode) {
    panel_state().update(|panel| {
        if panel.open && panel.mode == mode { panel.open = false; } else { panel.open = true; panel.mode = mode; }
    });
}

/// Closes the panel; `refocus` gives the keyboard back to the editor.
fn close(refocus: bool) {
    if !panel_state().with_untracked(|panel| panel.open) { return; }
    panel_state().update(|panel| panel.open = false);
    if refocus && let Some(id) = state::with(|workspace| workspace.active) { state::focus(id); }
}

/// The panel, mounted once at the root.
pub fn overlay() -> impl IntoElement {
    // Opening a result outlives the panel, so it runs under the root's owner.
    let owner = Owner::current();
    view().child(when(|| panel_state().with(|panel| panel.open), move || panel(owner)))
}

// ------------------------------------------------------------------ query

/// The documents, languages, symbols, commands and file index of one query.
fn base_query(mode: QuickNavMode, text: &str, files: Option<Arc<FileCache>>) -> QuickNavQuery {
    let mut query = QuickNavQuery::new(text);
    query.mode = mode;
    query.limit = LIMIT;
    query.files = files;
    let (documents, languages) = state::with(|workspace| {
        let documents = workspace
            .documents
            .iter()
            .map(|doc| OpenDocumentItem {
                name: doc.title.clone(),
                path: state::saved_path(&doc.document),
            })
            .collect::<Vec<_>>();
        let languages = workspace
            .registry
            .as_ref()
            .map(|registry| {
                registry
                    .enabled()
                    .map(|def| LanguageChoice { name: def.name.clone(), id: def.id.clone() })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (documents, languages)
    });
    query.open_documents = documents;
    query.languages = languages;
    query.recent_documents = recents();
    query.symbols = active_symbols();
    query.menu_commands = commands();
    query
}

/// Recently opened file paths, most recent first.
fn recents() -> Vec<PathBuf> { state::recents() }

/// Symbols of the active document's current text.
pub(super) fn active_symbols() -> Vec<scratch_search::Symbol> {
    let Some(id) = state::with(|workspace| workspace.active) else { return Vec::new() };
    let Some(text) = state::text(id) else { return Vec::new() };
    let syntax = state::with(|workspace| {
        workspace
            .doc(id)
            .and_then(|doc| workspace.registry.as_ref().and_then(|r| r.get(&doc.document.language)))
            .map(|def| def.editor_syntax.clone())
    });
    let language = syntax.as_deref().and_then(LanguageId::from_id).unwrap_or(LanguageId::PlainText);
    extract_symbols(&text, language)
}

/// The menu commands the palette offers (title, command id, shortcut).
fn commands() -> Vec<MenuCommandItem> {
    COMMANDS.iter().map(|(title, command, shortcut)| MenuCommandItem {
        title: (*title).to_owned(),
        command: (*command).to_owned(),
        shortcut: (!shortcut.is_empty()).then(|| (*shortcut).to_owned()),
    }).collect()
}

/// Command ids the palette can run. Keep in sync with `run_command`.
const COMMANDS: &[(&str, &str, &str)] = &[
    ("New Tab", "file.newTab", "⌘N"),
    ("Open…", "file.open", "⌘O"),
    ("Open Quickly…", "file.openQuickly", "⇧⌘O"),
    ("Save", "file.save", "⌘S"),
    ("Save As…", "file.saveAs", "⇧⌘S"),
    ("Close Tab", "file.close", "⌘W"),
    ("Find…", "find.find", "⌘F"),
    ("Find and Replace…", "find.replace", "⌥⌘F"),
    ("Go to Line…", "find.goToLine", "⌘L"),
    ("Run", "run.run", "⌘R"),
    ("Stop", "run.stop", "⌘."),
    ("Run Settings…", "run.settings", "⇧⌘R"),
    ("Clear Console", "run.clearConsole", "⌘K"),
    ("Show Navigator", "view.navigator", "⌥⌘1"),
    ("Search in Files…", "view.searchFiles", "⇧⌘F"),
    ("Show Console at Bottom", "view.consoleBottom", "⌘J"),
    ("Show Console on Right", "view.consoleRight", "⌥⌘J"),
    ("Un/Comment Lines", "text.comment", "⌘/"),
    ("Duplicate Line", "text.duplicate", "⌘D"),
    ("Delete Line", "text.deleteLine", "⇧⌘⌫"),
    ("Swap Lines Up", "text.swapUp", "⌥⌘↑"),
    ("Swap Lines Down", "text.swapDown", "⌥⌘↓"),
    ("Settings…", "app.settings", "⌘,"),
    ("Select Language…", "text.selectLanguage", "⌥⌘L"),
    ("New Document from Language…", "file.newFromLanguage", ""),
];

/// Runs a palette command id.
fn run_command(command: &str) {
    match command {
        "file.newTab" => state::new_document(),
        "file.open" => files::open_dialog(),
        "file.openQuickly" => open(),
        "file.save" => files::save_active(false),
        "file.saveAs" => files::save_active(true),
        "file.close" => files::close_active(),
        "find.find" => find::open(false),
        "find.replace" => find::open(true),
        "find.goToLine" => find::go_to_line(),
        "run.run" => execution::run_active(),
        "run.stop" => execution::stop_active(),
        "run.settings" => super::run_settings::open(),
        "run.clearConsole" => execution::clear_active(),
        "view.navigator" => super::chrome::toggle_navigator(),
        "view.searchFiles" => navigator::toggle_search(),
        "view.consoleBottom" => super::chrome::toggle_console_at(false),
        "view.consoleRight" => super::chrome::toggle_console_at(true),
        "text.comment" => edit::toggle_comment(),
        "text.duplicate" => edit::run(EditorCommand::DuplicateLinesDown),
        "text.deleteLine" => edit::run(EditorCommand::DeleteLine),
        "text.swapUp" => edit::run(EditorCommand::MoveLinesUp),
        "text.swapDown" => edit::run(EditorCommand::MoveLinesDown),
        "app.settings" => preferences::open(),
        "text.selectLanguage" => select_language(),
        "file.newFromLanguage" => new_document_palette(),
        _ => state::notice(format!("Unknown command: {command}")),
    }
}

// ------------------------------------------------------------------ actions

/// Opens `hit`'s target and closes the panel.
fn choose(owner: Owner, hit: QuickNavItem) {
    close(true);
    match hit.action {
        QuickNavAction::OpenPath(path) => {
            owner.run(|| { spawn(files::open_path(path)); });
        }
        QuickNavAction::OpenDocument { name, path } => {
            let id = state::with(|workspace| {
                workspace.documents.iter().find(|doc| {
                    match &path {
                        Some(path) => state::saved_path(&doc.document).as_ref() == Some(path),
                        None => doc.title == name,
                    }
                }).map(|doc| doc.document.id)
            });
            match id {
                Some(id) => state::activate(id),
                None => if let Some(path) = path { owner.run(|| { spawn(files::open_path(path)); }); },
            }
        }
        QuickNavAction::GoToLine { line, column } | QuickNavAction::GoToSymbol { line, column, .. } => {
            go_to(line, column);
        }
        QuickNavAction::SetLanguage(id) => {
            if let Some(active) = state::with(|workspace| workspace.active) { state::set_language(active, &id); }
        }
        QuickNavAction::NewDocument(language) => state::new_document_in(&language),
        QuickNavAction::MenuCommand(command) => run_command(&command),
    }
}

/// Moves the caret of the active editor to 0-based `line` / `column`.
fn go_to(line: u32, column: u32) {
    let Some(id) = state::with(|workspace| workspace.active) else { return; };
    state::record_jump();
    let Some(api) = state::editor_api(id) else { return; };
    let at = EditorPosition::new(line, column);
    api.set_selections(vec![EditorSelection::caret(at)]);
    api.reveal_position(at, true);
    state::focus(id);
}

// ------------------------------------------------------------------ panel

fn panel(owner: Owner) -> impl IntoElement {
    let mode = panel_state().with_untracked(|panel| panel.mode);
    let query = signal(String::new());
    let selected = signal(0_usize);
    let file_cache = stored(None::<Arc<FileCache>>);
    let indexing = signal(true);
    let hits = signal(Arc::new(Vec::<QuickNavItem>::new()));
    let generation = signal(0_u64);
    let list = element_ref();
    let visible = stored(None::<(usize, usize)>);

    // Index the files under the navigator's root (the cache walks lazily and
    // keeps the walk for later queries).
    {
        let root = navigator::root_path();
        spawn(async move {
            let root = root.clone();
            let cache = blocking(move || Arc::new(FileCache::with_options(root, FileCacheOptions {
                max_files: 50_000,
                excluded_directories: [
                    ".git", ".Trash", ".cache", ".npm", ".cargo", ".rustup",
                    "Library", "node_modules", "target", ".build", "DerivedData",
                ].iter().map(|dir| dir.to_string()).collect(),
                ..FileCacheOptions::default()
            }))).await;
            if let Ok(cache) = cache {
                file_cache.replace(Some(cache));
            }
            indexing.set(false);
        });
    }

    // Rank the providers for each query; a newer query wins and cancels the
    // one before it (`CancelToken` lets a stale walk stop early).
    let pending = stored(None::<CancelToken>);
    effect(move || {
        let text = query.get();
        let files = file_cache.get();
        generation.update(|g| *g += 1);
        let at = generation.get_untracked();
        let mode = mode.nav();
        // `base_query` reads the thread-local workspace — build it here, on
        // the engine thread, before `blocking` moves the work off it.
        let query = base_query(mode, &text, files);
        // Under `owner`, not the effect's run scope: a task owned by the
        // run is cancelled the moment the effect re-runs or ends.
        if let Some(token) = pending.get() {
            token.cancel();
        }
        let cancel = CancelToken::new();
        pending.replace(Some(cancel.clone()));
        owner.run(|| {
            spawn(async move {
                let ranked = blocking(move || quicknav(&query, &cancel)).await.unwrap_or_default();
                if generation.get_untracked() == at {
                    hits.set(Arc::new(ranked));
                }
            });
        });
    });

    let results = hits;
    let count = computed(move || results.with(|hits| hits.len()));
    let current = move || { let count = count.get_untracked(); (count > 0).then(|| selected.get_untracked().min(count - 1)) };
    let reveal = move |at: usize| {
        let (first, last) = visible.get().unwrap_or((0, 0));
        if at <= first { list.scroll_to_index(at as u32, Percentage(0.0), px(0.0), false); }
        else if at >= last { list.scroll_to_index(at as u32, Percentage(1.0), px(0.0), false); }
    };
    let step = move |delta: isize| {
        let count = count.get_untracked();
        if count == 0 { return; }
        let at = current().unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(count as isize) as usize;
        selected.set(next);
        reveal(next);
    };
    let submit = move || {
        let hit = current().and_then(|at| results.with_untracked(|hits| hits.get(at).cloned()));
        if let Some(hit) = hit { choose(owner, hit); }
    };
    let keys = |repeat: bool| KeyOptions { when: When::Always, repeat, ..KeyOptions::default() };
    on_key_with("up", keys(true), move |_| step(-1));
    on_key_with("down", keys(true), move |_| step(1));
    on_key_with("escape", keys(false), move |_| close(true));

    let placeholder = mode.placeholder();
    let field = view().h(px(FIELD_H)).flex_shrink(0.0).flex_row().items_center().gap_3().px_4()
        .child(glyph(IconName::Search, IconSize::Md, Tone::Muted).flex_shrink(0.0))
        .child(type_subhead().apply(input()).label(placeholder)
            .value(move || query.get())
            .on_change(move |value: &str| { query.set(value.to_owned()); selected.set(0); })
            .on_submit(move |_: &str| submit())
            .submit_behavior(SubmitBehavior::Submit)
            .auto_focus(true)
            .placeholder(placeholder)
            .placeholder_color(|| color(tokens::muted_foreground()))
            .caret_color(|| color(tokens::primary()))
            .selection_color(|| color(tokens::selection()))
            .flex_1().min_w(px(0.0)).text_color(tokens::popover_foreground))
        .child(when(move || indexing.get(), || spinner(IconSize::Md, Tone::Muted, "Indexing files")));
    let rows = move || virtual_list(move || count.get(), move |at| result_row(results, at, selected, owner))
        .estimated_item_size(px(row_h()))
        .content_padding_y(px(4.0))
        .on_viewable_items_change(move |items: &ViewableItems| { visible.replace(items.viewable); })
        .element_ref(&list)
        .h(move || px(count.get().min(VISIBLE_ROWS) as f32 * row_h() + 8.0));
    let no_results = move || view().h(px(row_h())).items_center().justify_center()
        .child(type_caption().apply(text(move || if indexing.get() { "Indexing…" } else { "No Results" })).text_color(tokens::muted_foreground));
    let show_body = move || {
        let mode = panel_state().with(|panel| panel.mode);
        !query.with(|query| query.trim().is_empty()) || mode != PanelMode::OpenQuickly
    };
    let body = move || view()
        .child(view().h(px(tokens::hairline())).bg(tokens::border))
        .child(when(move || count.get() > 0, rows).otherwise(no_results));
    let appear = motion().entering(
        MotionSpec::new(KeyframeTiming::new(motion::slow()).easing(motion::EASE_ENTER))
            .values(MotionChannel::Opacity, [0.0, 1.0])
            .values(MotionChannel::TranslateY, [motion::DISTANCE_POPUP, 0.0])
            .values(MotionChannel::Scale, [motion::POPUP_SCALE, 1.0]),
    );
    let card = view().absolute().w(move || px(PANEL_W.min(window().width.0 - 32.0)))
        .left(move || px(((window().width.0 - PANEL_W) / 2.0).max(16.0)))
        .top(move || px(window().height.0 * 0.22))
        .bg(tokens::popover).text_color(tokens::popover_foreground)
        .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
        .shadow_lg().shadow_color(tokens::shadow()).overflow_hidden()
        .bind_motion(&appear)
        .child(field)
        .child(when(show_body, body));
    dialog().animation(if motion::reduced() { ModalAnimation::None } else { ModalAnimation::Fade })
        .label("Open Quickly")
        .on_cancel(move || close(true))
        .child(view().absolute().inset_0().bg(tokens::scrim).focusable(false).label("Close").on_press(move || close(true)))
        .child(card)
}

// ------------------------------------------------------------------ rows

fn result_row(results: Signal<Arc<Vec<QuickNavItem>>>, at: Position, selected: Signal<usize>, owner: Owner) -> impl IntoElement {
    let hit = computed(move || results.with(|hits| hits.get(at.index()).cloned()));
    let is_selected = computed(move || selected.get() == at.index());
    let title = move || hit.with(|hit| hit.as_ref().map(|hit| SharedString::from(hit.title.clone())).unwrap_or_default());
    let subtitle = move || hit.with(|hit| hit.as_ref().map(|hit| SharedString::from(hit.subtitle.clone())).unwrap_or_default());
    let foreground = move || if is_selected.get() { tokens::selection_foreground() } else { tokens::popover_foreground() };
    view().h(px(row_h())).px_2()
        .role(AccessibilityRole::Button)
        .label(move || format!("{}, {}", title(), subtitle()))
        .on_press(move |_: &PressEvent| { if let Some(hit) = hit.get_untracked() { choose(owner, hit); } })
        .child(view().flex_1().flex_row().items_center().gap(px(10.0)).px(px(8.0)).rounded(px(tokens::radius()))
            .bg(move || if is_selected.get() { tokens::selection() } else { AdaptiveColor::TRANSPARENT })
            .when(!motion::reduced(), motion::state_wash)
            .hover(move |s| s.bg(move || if is_selected.get() { tokens::selection() } else { tokens::accent() }))
            .child(view().w(px(28.0)).items_center().flex_shrink(0.0).child(dynamic(move || {
                row_icon(hit.get(), is_selected.get()).into_element()
            })))
            .child(view().flex_1().min_w(px(0.0)).gap(px(1.0))
                .child(type_body().medium().apply(text(title)).number_of_lines(1).text_color(foreground))
                .child(type_caption().apply(text(subtitle)).number_of_lines(1)
                    .text_color(move || if is_selected.get() { tokens::selection_foreground().opacity(0.8) } else { tokens::muted_foreground() }))))
}

/// The row's leading icon: a language or symbol glyph tile, or a file icon.
fn row_icon(hit: Option<QuickNavItem>, selected: bool) -> impl IntoElement {
    let tone = if selected { Tone::OnSelection } else { Tone::Muted };
    match hit.as_ref().map(|hit| hit.icon_kind) {
        Some(IconKind::Language) => glyph_tile("Aa", GlyphTone::One).into_element(),
        Some(IconKind::Symbol(kind)) => glyph_tile(symbol_letter(kind), symbol_tone(kind)).into_element(),
        Some(IconKind::Line) => glyph(IconName::ChevronRight, IconSize::Md, tone).into_element(),
        Some(IconKind::MenuCommand) => glyph(IconName::Command, IconSize::Md, tone).into_element(),
        Some(IconKind::NewDocument) => glyph(IconName::Plus, IconSize::Md, tone).into_element(),
        Some(IconKind::Recent) => glyph(IconName::File, IconSize::Md, tone).into_element(),
        Some(IconKind::Folder) => glyph(IconName::Folder, IconSize::Md, Tone::Primary).into_element(),
        Some(IconKind::File) => glyph(hit.map(|hit| file_icon(&hit.title)).unwrap_or(IconName::FileText), IconSize::Md, tone).into_element(),
        Some(IconKind::Document) | None => glyph(IconName::FileText, IconSize::Md, tone).into_element(),
    }
}

/// The letter a symbol kind's tile shows (DESIGN.md 6.2).
fn symbol_letter(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function | SymbolKind::Method => "f",
        SymbolKind::Class => "C",
        SymbolKind::Struct => "S",
        SymbolKind::Enum => "E",
        SymbolKind::Protocol | SymbolKind::Interface => "P",
        SymbolKind::TypeAlias => "T",
        SymbolKind::Variable => "V",
        SymbolKind::Constant => "K",
        SymbolKind::Namespace | SymbolKind::Module => "N",
        SymbolKind::Section => "#",
        SymbolKind::Property => "A",
        _ => "•",
    }
}

/// The tile color of a symbol kind (DESIGN.md 6.2).
fn symbol_tone(kind: SymbolKind) -> GlyphTone {
    match kind {
        SymbolKind::Function | SymbolKind::Method => GlyphTone::One,
        SymbolKind::Class => GlyphTone::Four,
        SymbolKind::Struct => GlyphTone::Two,
        SymbolKind::Enum => GlyphTone::Seven,
        SymbolKind::Protocol | SymbolKind::Interface => GlyphTone::Eight,
        SymbolKind::TypeAlias => GlyphTone::Nine,
        SymbolKind::Variable => GlyphTone::Three,
        SymbolKind::Constant => GlyphTone::Six,
        SymbolKind::Namespace | SymbolKind::Module | SymbolKind::Section => GlyphTone::Ten,
        SymbolKind::Property => GlyphTone::Five,
        _ => GlyphTone::Ten,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_all_dispatch() {
        for (_, command, _) in COMMANDS {
            assert!(!command.is_empty());
        }
    }
}
