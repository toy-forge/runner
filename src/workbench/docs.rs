//! The documentation panel (DESIGN.md 8.8): the right pane, toggled by ⇧⌘I
//! or the toolbar's `sidebar-right` button.
//!
//! Header row 1: a "Search Docs" field, the Automatic Lookups eye toggle and
//! the actions gear. Header row 2: a chip per documentation source of the
//! active language. The page area is a `web_view` bound to the current URL;
//! a language without sources shows the `No Documentation Sources` empty
//! state, no document the `No Documentation` one.
//!
//! Typing looks up after a 400 ms debounce (DESIGN 8.8); Return is not
//! required. Searching substitutes `{query}`/`{lang}` into the source's URL
//! template (`DocSource::url_for`); a Bookmark chip loads its fixed URL.
//! Automatic Lookups polls the active editor's selection while toggled on
//! and looks up what it finds.

use std::time::Duration;

use nsh_core::element::desktop::{MenuItem, popup_menu};
use scratch_language::DocSource;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::{preferences, state};

/// The chips' row height (DESIGN 8.8 header row 2).
const CHIPS_H: f32 = 28.0;

/// Panel state, global so it survives the panel's `when` remount.
#[derive(Clone, Default, PartialEq)]
struct Docs {
    /// The draft in the search field.
    draft: String,
    /// The committed lookup text.
    query: String,
    /// The URL the web view shows ("" = nothing loaded).
    url: String,
    /// The selected source chip, by name.
    source: String,
    /// Automatic Lookups (the eye): look up the editor's selection.
    auto: bool,
    /// The last auto-looked-up text (dedupes the poll).
    looked_up: String,
    /// The debounce generation of `draft` changes.
    generation: u64,
}

fn docs() -> Signal<Docs> { global::<Docs>() }

/// The enabled sources of the active document's language.
fn sources() -> Vec<DocSource> {
    state::with(|workspace| {
        let Some(id) = workspace.active else { return Vec::new() };
        let Some(doc) = workspace.doc(id) else { return Vec::new() };
        workspace.registry.as_ref().and_then(|registry| registry.get(&doc.document.language)).map(|language| {
            language.docs.iter().filter(|source| source.enabled).cloned().collect()
        }).unwrap_or_default()
    })
}

/// The active language's display name (for `{lang}` in URLs).
fn language_name() -> String {
    state::with(|workspace| {
        let id = workspace.active?;
        workspace.registry.as_ref()?.get(&workspace.doc(id)?.document.language).map(|language| language.name.clone())
    }).unwrap_or_default()
}

/// The selected source (`Some(name)` preferred, else the first).
fn current_source(list: &[DocSource]) -> Option<DocSource> {
    let name = docs().with_untracked(|docs| docs.source.clone());
    list.iter().find(|source| source.name == name).cloned().or_else(|| list.first().cloned())
}

/// A lookup of `query` in the current source; Bookmark sources always load
/// their fixed URL.
fn lookup(query: &str) {
    let list = sources();
    let Some(source) = current_source(&list) else { return; };
    let lang = language_name();
    let query = if matches!(source.kind, scratch_language::DocType::Bookmark) { "" } else { query };
    let url = source.url_for(query, &lang);
    docs().update(|docs| { docs.url = url; docs.query = query.to_owned(); });
}

/// "Look Up Current Symbol" (⌘I): looks up the selection — or the word at
/// the caret — in the documentation panel, opening it when hidden.
pub fn look_up_current_symbol() {
    let Some(id) = state::with(|workspace| workspace.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let selected = api.selected_text().trim().to_owned();
    let query = if selected.is_empty() {
        let text = api.value();
        let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
        let Some(line) = text.split('\n').nth(caret.line as usize) else { return; };
        match super::find::word_at(line, caret.column) { Some(word) => word, None => return }
    } else {
        selected
    };
    state::display().update(|view| view.docs_visible = true);
    docs().update(|docs| { docs.looked_up = query.clone(); docs.draft = query.clone(); });
    lookup(&query);
}

/// Commits `draft` to a lookup after a 400 ms quiet spell.
fn schedule_lookup() {
    let at = docs().with_untracked(|docs| docs.generation);
    spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (generation, draft) = docs().with_untracked(|docs| (docs.generation, docs.draft.clone()));
        if generation == at { lookup(&draft); }
    });
}

/// The Automatic Lookups poll: every 600 ms, look up the active editor's
/// selected text when it changed.
fn install_auto_lookup() {
    spawn(async {
        loop {
            tokio::time::sleep(Duration::from_millis(600)).await;
            let enabled = docs().with_untracked(|docs| docs.auto);
            let visible = state::display().with_untracked(|view| view.docs_visible);
            if !enabled || !visible { continue; }
            let selection = state::with(|workspace| workspace.active)
                .and_then(state::editor_api)
                .map(|api| api.selected_text())
                .unwrap_or_default();
            let word = selection.trim().to_owned();
            let last = docs().with_untracked(|docs| docs.looked_up.clone());
            if !word.is_empty() && word != last && word.chars().count() <= 200 {
                docs().update(|docs| { docs.looked_up = word.clone(); docs.draft = word.clone(); });
                lookup(&word);
            }
        }
    });
}

/// Mounts the panel's shared state and the auto-lookup poll once at root.
pub fn install() {
    let _ = docs();
    install_auto_lookup();
}

/// The panel: header, chips and the page.
pub fn panel() -> impl IntoElement {
    view().flex_1().min_h(px(0.0)).bg(tokens::background)
        .child(header())
        .child(chips())
        .child(view().h(px(tokens::hairline())).bg(tokens::border))
        .child(page())
}

fn header() -> impl IntoElement {
    view().h(px(40.0)).flex_shrink(0.0).flex_row().items_center().gap_1p5().px_2()
        .child(view().flex_1().min_w(px(0.0)).child(search_field("Search Docs",
            move || SharedString::from(docs().with(|docs| docs.draft.clone())),
            move || None::<SharedString>,
            move |value: &str| {
                docs().update(|docs| { docs.draft = value.to_owned(); docs.generation += 1; });
                schedule_lookup();
            },
            move || docs().update(|docs| { docs.draft.clear(); docs.query.clear(); }),
            SearchFieldOptions::new().size(ControlSize::Md))))
        .child(dynamic(move || {
            icon_button(IconName::Eye, "Automatic Lookups", IconButtonOptions::new().size(ControlSize::Sm).selected(docs().with(|docs| docs.auto)),
                move || docs().update(|docs| docs.auto = !docs.auto)).into_element()
        }))
        .child(icon_button(IconName::Gear, "Actions", IconButtonOptions::new().size(ControlSize::Sm), || {
            spawn(async { let _ = popup_menu(actions_menu()).await; });
        }))
}

/// The actions menu (`gear`): browser, clipboard and navigation actions.
fn actions_menu() -> Vec<MenuItem> {
    let url = docs().with_untracked(|docs| docs.url.clone());
    let open = url.clone();
    let copy = url.clone();
    vec![
        MenuItem::new("Open in Browser").disabled(url.is_empty()).on_select(move || {
            let url = open.clone();
            spawn(async move { let _ = open_url(url).await; });
        }),
        MenuItem::new("Copy Current URL").disabled(url.is_empty()).on_select(move || set_clipboard(copy.clone())),
        MenuItem::separator(),
        MenuItem::new("Reload Page").disabled(url.is_empty()).on_select(reload),
        MenuItem::separator(),
        MenuItem::new("Hide Documentation").on_select(super::chrome::toggle_docs),
    ]
}

/// Reloads by bouncing the URL binding through empty.
fn reload() {
    let url = docs().with_untracked(|docs| docs.url.clone());
    if url.is_empty() { return; }
    spawn(async move {
        docs().update(|docs| docs.url.clear());
        next_frame().await;
        docs().update(|docs| docs.url = url);
    });
}

fn chips() -> impl IntoElement {
    view().h(px(CHIPS_H)).flex_shrink(0.0).flex_row().items_center().gap_1().px_2()
        .child(dynamic(move || {
            let list = sources();
            let selected = current_source(&list).map(|source| source.name).unwrap_or_default();
            let mut row = view().flex_row().items_center().gap_1();
            for source in list {
                let name = source.name.clone();
                let is_selected = name == selected;
                let chip = view().rounded(px(tokens::radius_xl() * 2.0))
                    .bg(move || if is_selected { tokens::capsule_selected() } else { AdaptiveColor::TRANSPARENT })
                    .child(button(SharedString::from(name.clone()),
                        ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm),
                        move || {
                            docs().update(|docs| docs.source = name.clone());
                            let query = docs().with_untracked(|docs| docs.query.clone());
                            lookup(&query);
                        }));
                row = row.child(chip);
            }
            row.into_element()
        }))
}

/// The page: a `web_view` of the loaded URL, or the right empty state.
fn page() -> impl IntoElement {
    view().flex_1().min_h(px(0.0)).child(dynamic(page_body))
}

fn page_body() -> AnyElement {
    let url = docs().with(|docs| docs.url.clone());
    if !url.is_empty() {
        return web_view(move || docs().with(|docs| docs.url.clone()))
            .flex_1().min_h(px(0.0)).into_element();
    }
    if !state::with(|workspace| workspace.active.is_some()) {
        return empty_state(IconName::BookOpen, "No Document", "Open a document to look up its documentation.", None::<(&'static str, fn())>);
    }
    if sources().is_empty() {
        return empty_state(IconName::BookOpen, "No Documentation Sources",
            "This language has no documentation sources configured.",
            Some(("Configure…", || preferences::open_pane(preferences::Category::Languages))));
    }
    empty_state(IconName::BookOpen, "Search Documentation",
        "Type above to search this language's documentation sources, or pick a source below.",
        None::<(&'static str, fn())>)
}

/// A centered empty state: icon, title, message, optional button.
fn empty_state(
    icon: IconName,
    title: &'static str,
    message: &'static str,
    action: Option<(&'static str, impl Fn() + 'static)>,
) -> AnyElement {
    let mut body = view().flex_1().min_h(px(0.0)).items_center().justify_center().gap_2().px_6()
        .child(glyph(icon, IconSize::Xl, Tone::Muted))
        .child(type_subhead().medium().apply(text(title)).text_color(tokens::foreground))
        .child(type_caption().apply(text(message)).text_color(tokens::muted_foreground));
    if let Some((label, action)) = action {
        body = body.child(button(label, ButtonOptions::new().size(ControlSize::Sm), action));
    }
    body.into_element()
}
