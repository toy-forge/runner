//! The documentation panel (DESIGN.md 8.8): the right pane, toggled by ⇧⌘I
//! or the toolbar's `sidebar-right` button.
//!
//! Header row 1: Back, Forward, a "Search Docs" field, Automatic Lookups eye
//! toggle and the actions gear. Under header: optional 2pt indeterminate
//! progress bar and collapsible Find On Page bar. Header row 2: a chip per
//! documentation source of the active language plus enabled global sources.
//! Page area: `web_view` with JS heading extraction & optional theme reader
//! stylization, or offline/empty fallback. Footer (24pt): expand grabber,
//! URL host caption, and Table of Contents menu.

use std::time::Duration;

use nsh_core::element::WebNavigation;
use nsh_core::element::desktop::{MenuItem, popup_menu};
use scratch_config::{DocKind, DocSource as ConfigDocSource};
use scratch_language::{DocSource, DocType};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::{preferences, state};

/// The chips' row height (DESIGN 8.8 header row 2).
const CHIPS_H: f32 = 28.0;
/// Footer bar height matching the window's bottom chrome band.
const FOOTER_H: f32 = 24.0;

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
    /// Whether the web view is currently loading.
    loading: bool,
    /// Back/forward history flags reported by WKWebView.
    can_back: bool,
    can_forward: bool,
    /// Offline / load error message from `on_error`.
    error: Option<String>,
    /// Whether the Find On Page strip is open.
    find_open: bool,
    /// Query inside Find On Page.
    find_query: String,
    /// Whether "Stylize Web Content" reader CSS is injected.
    stylize: bool,
    /// Table of contents headings `(level 1..=3, id, text)` from loaded page.
    headings: Vec<(u8, String, String)>,
}

fn docs() -> Signal<Docs> { global::<Docs>() }

/// Enabled sources of the active language merged with enabled Global Docs,
/// plus the name of the preferred global source when `use_preferred_global_docs` is on.
fn sources_and_preferred() -> (Vec<DocSource>, Option<String>) {
    let mut list: Vec<DocSource> = state::with(|workspace| {
        let Some(id) = workspace.active else { return Vec::new() };
        let Some(doc) = workspace.doc(id) else { return Vec::new() };
        workspace
            .registry
            .as_ref()
            .and_then(|registry| registry.get(&doc.document.language))
            .map(|language| language.docs.iter().filter(|s| s.enabled).cloned().collect())
            .unwrap_or_default()
    });
    if !state::with(|w| w.active.is_some()) {
        return (list, None);
    }

    let preferred_name = preferences::settings().with(|prefs| {
        let docs_prefs = &prefs.languages.docs;
        let mut preferred = None;
        for global_src in docs_prefs.global_entries.iter().filter(|g| g.enabled) {
            if docs_prefs.use_preferred_global_docs
                && global_src.id == docs_prefs.preferred_global_docs_id
            {
                preferred = Some(global_src.name.clone());
            }
            if !list.iter().any(|existing| existing.name == global_src.name) {
                list.push(DocSource {
                    name: global_src.name.clone(),
                    kind: match global_src.kind {
                        DocKind::Search => DocType::Search,
                        DocKind::Bookmark => DocType::Bookmark,
                        DocKind::TextFind => DocType::TextFind,
                    },
                    url: global_src.url.clone(),
                    post: (!global_src.post.is_empty()).then(|| global_src.post.clone()),
                    enabled: true,
                    scope: None,
                    css: None,
                    js: None,
                });
            }
        }
        preferred
    });
    (list, preferred_name)
}

/// The enabled sources of the active document's language plus global entries.
fn sources() -> Vec<DocSource> {
    sources_and_preferred().0
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
    let query = if matches!(source.kind, DocType::Bookmark) { "" } else { query };
    let url = source.url_for(query, &lang);
    docs().update(|docs| {
        docs.url = url;
        docs.query = query.to_owned();
        docs.error = None;
        docs.headings.clear();
    });
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
    state::with(|w| w.changed());
    let (_, preferred) = sources_and_preferred();
    docs().update(|docs| {
        docs.looked_up = query.clone();
        docs.draft = query.clone();
        if let Some(pref) = preferred {
            docs.source = pref;
        }
    });
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
    let auto_enabled = preferences::settings().with_untracked(|p| p.languages.docs.autolookup_enabled);
    docs().update(|d| d.auto = auto_enabled);
    install_auto_lookup();
}

/// The panel: header, progress indicator, optional find bar, chips, page, and 24pt TOC footer.
pub fn panel() -> impl IntoElement {
    let web_ref = element_ref();
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).flex_col().bg(tokens::background)
        .child(header(web_ref.clone()))
        .child(when(move || docs().with(|d| d.loading && !d.url.is_empty()), || {
            progress_bar(0.5, ProgressOptions::new().height(2.0).indeterminate(true))
        }))
        .child(when(move || docs().with(|d| d.find_open), {
            let r = web_ref.clone();
            move || find_bar(r.clone())
        }))
        .child(chips())
        .child(view().h(px(tokens::hairline())).bg(tokens::border))
        .child(page(web_ref.clone()))
        .child(footer_bar(web_ref))
}

fn header(web_ref: ElementRef) -> impl IntoElement {
    let r_back = web_ref.clone();
    let r_fwd = web_ref.clone();
    let can_back = computed(move || docs().with(|d| d.can_back && !d.url.is_empty()));
    let can_fwd = computed(move || docs().with(|d| d.can_forward && !d.url.is_empty()));
    let auto_lookup = computed(move || docs().with(|d| d.auto));
    view().h(px(40.0)).flex_shrink(0.0).flex_row().items_center().gap_1().px_2()
        .child(dynamic(move || {
            let can_back = can_back.get();
            let r = r_back.clone();
            icon_button(IconName::ChevronLeft, "Back", IconButtonOptions::new().size(ControlSize::Sm).disabled(!can_back), move || r.go_back()).into_element()
        }))
        .child(dynamic(move || {
            let can_fwd = can_fwd.get();
            let r = r_fwd.clone();
            icon_button(IconName::ChevronRight, "Forward", IconButtonOptions::new().size(ControlSize::Sm).disabled(!can_fwd), move || r.go_forward()).into_element()
        }))
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
            let auto = auto_lookup.get();
            icon_button(IconName::Eye, "Automatic Lookups", IconButtonOptions::new().size(ControlSize::Sm).selected(auto),
                move || {
                    let next = !docs().with_untracked(|d| d.auto);
                    docs().update(|d| d.auto = next);
                    preferences::write(|p| {
                        p.languages.docs.autolookup_enabled = next;
                    });
                }).into_element()
        }))
        .child(icon_button(IconName::Gear, "Actions", IconButtonOptions::new().size(ControlSize::Sm), move || {
            let r = web_ref.clone();
            spawn(async move { let _ = popup_menu(actions_menu(r)).await; });
        }))
}

fn find_bar(web_ref: ElementRef) -> impl IntoElement {
    let r_prev = web_ref.clone();
    let r_next = web_ref.clone();
    let run_find = move |web: &ElementRef, backwards: bool| {
        let q = docs().with_untracked(|d| d.find_query.clone());
        if q.is_empty() { return; }
        let quoted = serde_json::to_string(&q).unwrap_or_else(|_| "\"\"".to_owned());
        web.inject_java_script(format!(
            "window.find({quoted}, false, {backwards}, true, false, true, false);"
        ));
    };
    view().h(px(30.0)).flex_shrink(0.0).flex_row().items_center().gap_1().px_2()
        .border_b_1().border_color(tokens::border).bg(tokens::editor_bar)
        .child(view().flex_1().min_w(px(0.0)).child(search_field(
            "Find On Page",
            move || SharedString::from(docs().with(|d| d.find_query.clone())),
            move || None::<SharedString>,
            move |value: &str| docs().update(|d| d.find_query = value.to_owned()),
            move || docs().update(|d| d.find_query.clear()),
            SearchFieldOptions::new().size(ControlSize::Sm),
        )))
        .child(icon_button(IconName::ChevronUp, "Previous Match", IconButtonOptions::new().size(ControlSize::Sm), move || {
            run_find(&r_prev, true);
        }))
        .child(icon_button(IconName::ChevronDown, "Next Match", IconButtonOptions::new().size(ControlSize::Sm), move || {
            run_find(&r_next, false);
        }))
        .child(icon_button(IconName::Close, "Close Find On Page", IconButtonOptions::new().size(ControlSize::Sm), || {
            docs().update(|d| d.find_open = false);
        }))
}

/// The actions menu (`gear`): browser, clipboard, find, bookmarking and configuration.
fn actions_menu(web_ref: ElementRef) -> Vec<MenuItem> {
    let (url, stylize) = docs().with_untracked(|docs| (docs.url.clone(), docs.stylize));
    let open = url.clone();
    let copy = url.clone();
    let bookmark_url = url.clone();
    let r_rel = web_ref.clone();
    let r_stylize = web_ref;
    vec![
        MenuItem::new("Open in Browser").disabled(url.is_empty()).on_select(move || {
            let url = open.clone();
            spawn(async move { let _ = open_url(url).await; });
        }),
        MenuItem::new("Copy Current URL").disabled(url.is_empty()).on_select(move || set_clipboard(copy.clone())),
        MenuItem::separator(),
        MenuItem::new("Reload Page").disabled(url.is_empty()).on_select(move || reload(&r_rel)),
        MenuItem::new("Find On Page…").disabled(url.is_empty()).on_select(|| {
            docs().update(|d| d.find_open = true);
        }),
        MenuItem::new("Bookmark This Page…").disabled(url.is_empty()).on_select(move || {
            bookmark_url_entry(&bookmark_url);
        }),
        MenuItem::separator(),
        MenuItem::new("Stylize Web Content").checked(stylize).on_select(move || {
            let next = !docs().with_untracked(|d| d.stylize);
            docs().update(|d| d.stylize = next);
            r_stylize.inject_java_script(doc_injected_script(next));
        }),
        MenuItem::new("Define Docs Configuration…").on_select(|| {
            preferences::open_pane(preferences::Category::Languages);
        }),
        MenuItem::separator(),
        MenuItem::new("Hide Documentation").on_select(super::chrome::toggle_docs),
    ]
}

fn bookmark_url_entry(url: &str) {
    if url.trim().is_empty() { return; }
    let title = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('?')
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let short_name: String = title.chars().take(36).collect();
    let name = if short_name.is_empty() { "Bookmarked Docs".to_owned() } else { short_name };
    preferences::write(|p| {
        if !p.languages.docs.global_entries.iter().any(|g| g.url == url) {
            p.languages.docs.global_entries.push(ConfigDocSource {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.clone(),
                kind: DocKind::Bookmark,
                url: url.to_owned(),
                post: String::new(),
                enabled: true,
            });
        }
    });
    state::notice(format!("Bookmarked \"{name}\" in Global Documentation Sources"));
}

/// Reloads the web view via its `ElementRef`, clearing any stale error banner.
fn reload(web_ref: &ElementRef) {
    let url = docs().with_untracked(|docs| docs.url.clone());
    if url.is_empty() { return; }
    docs().update(|d| {
        d.loading = true;
        d.error = None;
    });
    web_ref.reload();
}

fn chips() -> impl IntoElement {
    view().h(px(CHIPS_H)).flex_shrink(0.0).flex_row().items_center().gap_1().px_2().overflow_hidden()
        .child(dynamic(move || {
            let _ = state::display().with(|v| (v.active, v.tabs.clone()));
            let _ = global::<state::RegistryRevision>().get();
            let _ = preferences::settings().with(|p| p.languages.docs.global_entries.clone());
            let selected_name = docs().with(|d| d.source.clone());
            let list = sources();
            let selected = list
                .iter()
                .find(|s| s.name == selected_name)
                .or_else(|| list.first())
                .map(|s| s.name.clone())
                .unwrap_or_default();
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

fn doc_injected_script(stylize: bool) -> String {
    let active_source = current_source(&sources());
    let custom_css = active_source
        .as_ref()
        .and_then(|s| s.css.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let custom_js = active_source
        .as_ref()
        .and_then(|s| s.js.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let stylize_js = if stylize {
        let bg = color(tokens::background()).to_hex();
        let fg = color(tokens::foreground()).to_hex();
        let link = color(tokens::link()).to_hex();
        let code_bg = color(tokens::muted()).to_hex();
        let border = color(tokens::border()).to_hex();
        let css = format!(
            "html,body{{background:{bg} !important;color:{fg} !important;}} \
             a{{color:{link} !important;}} \
             pre,code{{background:{code_bg} !important;border-color:{border} !important;color:{fg} !important;}} \
             th,td,hr,blockquote{{border-color:{border} !important;}}"
        );
        let quoted_css = serde_json::to_string(&css).unwrap_or_else(|_| "\"\"".to_owned());
        format!(
            "var st=document.getElementById('__sr_stylize');\
             if(!st){{st=document.createElement('style');st.id='__sr_stylize';document.head&&document.head.appendChild(st);}}\
             st.textContent={quoted_css};"
        )
    } else {
        "var st=document.getElementById('__sr_stylize');if(st)st.remove();".to_owned()
    };

    let source_css_js = if let Some(css) = custom_css {
        let quoted = serde_json::to_string(css).unwrap_or_else(|_| "\"\"".to_owned());
        format!(
            "var uc=document.getElementById('__sr_user_css');\
             if(!uc){{uc=document.createElement('style');uc.id='__sr_user_css';document.head&&document.head.appendChild(uc);}}\
             uc.textContent={quoted};"
        )
    } else {
        "var uc=document.getElementById('__sr_user_css');if(uc)uc.remove();".to_owned()
    };

    let source_js_snippet = if let Some(js) = custom_js {
        format!("try{{{js}}}catch(_){{}}")
    } else {
        String::new()
    };

    format!(
        "(function(){{\
          try {{\
            {stylize_js}\
            {source_css_js}\
            {source_js_snippet}\
            var els=document.querySelectorAll('h1,h2,h3');\
            var items=[];\
            for(var i=0;i<els.length&&items.length<60;i++){{\
              var el=els[i];\
              var txt=(el.textContent||'').trim().replace(/\\s+/g,' ');\
              if(!txt)continue;\
              if(!el.id)el.id='cr-toc-'+i;\
              var lv=parseInt(el.tagName.substring(1),10)||1;\
              items.push({{level:lv,id:el.id,text:txt.slice(0,80)}});\
            }}\
            if(window.nsh&&window.nsh.postMessage){{\
              window.nsh.postMessage(JSON.stringify({{type:'toc',items:items}}));\
            }}\
          }}catch(_){{}}\
        }})();"
    )
}

fn handle_doc_message(data: &str) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(data) else { return; };
    if val.get("type").and_then(|v| v.as_str()) != Some("toc") { return; }
    let Some(arr) = val.get("items").and_then(|v| v.as_array()) else { return; };
    let headings: Vec<(u8, String, String)> = arr
        .iter()
        .filter_map(|item| {
            let level = item.get("level")?.as_u64()?.clamp(1, 3) as u8;
            let id = item.get("id")?.as_str()?.to_owned();
            let text = item.get("text")?.as_str()?.to_owned();
            Some((level, id, text))
        })
        .collect();
    docs().update(|d| d.headings = headings);
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PageMode {
    Error { message: String, url: String },
    WebView,
    NoDocument,
    NoSources,
    Empty,
}

fn compute_page_mode(has_active_doc: bool, url: &str, error: Option<&str>, has_sources: bool) -> PageMode {
    if let Some(message) = error {
        PageMode::Error {
            message: message.to_owned(),
            url: url.to_owned(),
        }
    } else if !url.is_empty() {
        PageMode::WebView
    } else if !has_active_doc {
        PageMode::NoDocument
    } else if !has_sources {
        PageMode::NoSources
    } else {
        PageMode::Empty
    }
}

/// The page: a `web_view` of the loaded URL, offline/error fallback, or the right empty state.
fn page(web_ref: ElementRef) -> impl IntoElement {
    let mode = computed(move || {
        let _ = state::display().with(|d| (d.active, d.tabs.len()));
        let _ = global::<state::RegistryRevision>().get();
        let (url, err) = docs().with(|docs| (docs.url.clone(), docs.error.clone()));
        let active = state::with(|workspace| workspace.active.is_some());
        let has_sources = !sources().is_empty();
        compute_page_mode(active, &url, err.as_deref(), has_sources)
    });
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).child(dynamic(move || {
        let current = mode.get();
        untracked(|| page_body(current, web_ref.clone()))
    }))
}

fn page_body(mode: PageMode, web_ref: ElementRef) -> AnyElement {
    match mode {
        PageMode::Error { message, url } => {
            let open_url_copy = url.clone();
            let r_retry = web_ref;
            view().flex_1().min_h(px(0.0)).items_center().justify_center().gap_2().px_6()
                .child(glyph(IconName::WifiOff, IconSize::Xl, Tone::Muted))
                .child(type_subhead().medium().apply(text("Could Not Load Documentation")).text_color(tokens::foreground))
                .child(type_caption().apply(text(message)).text_color(tokens::muted_foreground))
                .child(view().flex_row().gap_2().pt_1()
                    .child(button("Retry", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), move || reload(&r_retry)))
                    .child(when(move || !open_url_copy.is_empty(), {
                        let target = url.clone();
                        move || {
                            let t = target.clone();
                            button("Open in Browser", ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm), move || {
                                let t = t.clone();
                                spawn(async move { let _ = open_url(t).await; });
                            })
                        }
                    })))
                .into_element()
        }
        PageMode::WebView => {
            web_view(move || docs().with(|docs| docs.url.clone()))
                .element_ref(&web_ref)
                .injected_java_script(move || doc_injected_script(docs().with(|d| d.stylize)))
                .on_load_start(|nav: &WebNavigation| {
                    docs().update(|d| {
                        d.loading = true;
                        d.error = None;
                        d.can_back = nav.can_go_back;
                        d.can_forward = nav.can_go_forward;
                    });
                })
                .on_load(|nav: &WebNavigation| {
                    docs().update(|d| {
                        d.loading = false;
                        d.can_back = nav.can_go_back;
                        d.can_forward = nav.can_go_forward;
                    });
                })
                .on_navigation_state_change(|nav: &WebNavigation| {
                    docs().update(|d| {
                        d.loading = nav.loading;
                        d.can_back = nav.can_go_back;
                        d.can_forward = nav.can_go_forward;
                    });
                })
                .on_error(|msg: &str| {
                    let message = msg.to_owned();
                    docs().update(|d| {
                        d.loading = false;
                        d.error = Some(message);
                    });
                })
                .on_message(handle_doc_message)
                .flex_1().min_h(px(0.0)).into_element()
        }
        PageMode::NoDocument => {
            empty_state(IconName::BookOpen, "No Document", "Open a document to look up its documentation.", None::<(&'static str, fn())>)
        }
        PageMode::NoSources => {
            empty_state(
                IconName::BookOpen,
                "No Documentation Sources",
                "This language has no documentation sources configured.",
                Some(("Configure…", || preferences::open_pane(preferences::Category::Languages))),
            )
        }
        PageMode::Empty => {
            empty_state(
                IconName::BookOpen,
                "Search Documentation",
                "Type above to search this language's documentation sources, or pick a source below.",
                None::<(&'static str, fn())>,
            )
        }
    }
}

/// Bottom 24pt bar: expand/collapse grabber, current URL host caption, and Table of Contents menu.
fn footer_bar(web_ref: ElementRef) -> impl IntoElement {
    let headings_count = computed(move || docs().with(|d| d.headings.len()));
    view().h(px(FOOTER_H)).flex_shrink(0.0).flex_row().items_center().gap_1p5().px_2()
        .border_t_1().border_color(tokens::editor_bar_border).bg(tokens::editor_bar)
        .child(dynamic(move || {
            let expanded = state::display().with(|d| d.docs_expanded);
            icon_button(
                if expanded { IconName::ChevronsRight } else { IconName::ChevronsLeft },
                if expanded { "Restore Documentation Width" } else { "Expand Documentation" },
                IconButtonOptions::new().size(ControlSize::Sm),
                || {
                    motion::animate_panel();
                    state::display().update(|d| d.docs_expanded = !d.docs_expanded);
                },
            ).into_element()
        }))
        .child(view().flex_1().min_w(px(0.0)).overflow_hidden().child(type_caption().apply(
            text(move || {
                let url = docs().with(|d| d.url.clone());
                if url.is_empty() {
                    return "Ready".to_owned();
                }
                url.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .unwrap_or(&url)
                    .to_owned()
            })
            .number_of_lines(1u32)
            .text_overflow(nsh_core::EllipsizeMode::Tail)
            .text_color(tokens::muted_foreground),
        )))
        .child(dynamic(move || {
            let count = headings_count.get();
            let r = web_ref.clone();
            button(
                if count == 0 { SharedString::from("Contents") } else { SharedString::from(format!("Contents ({count})")) },
                ButtonOptions::new()
                    .variant(ButtonVariant::Ghost)
                    .size(ControlSize::Sm)
                    .icon(IconName::ChevronsUpDown)
                    .disabled(count == 0),
                move || {
                    let headings = docs().with_untracked(|d| d.headings.clone());
                    let r = r.clone();
                    let items: Vec<MenuItem> = headings
                        .into_iter()
                        .map(|(level, id, title)| {
                            let indent = "  ".repeat(level.saturating_sub(1) as usize);
                            let r_item = r.clone();
                            MenuItem::new(format!("{indent}{title}")).on_select(move || {
                                let quoted = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".to_owned());
                                r_item.inject_java_script(format!(
                                    "document.getElementById({quoted})?.scrollIntoView({{behavior:'smooth',block:'start'}});"
                                ));
                            })
                        })
                        .collect();
                    if !items.is_empty() {
                        spawn(async move { let _ = popup_menu(items).await; });
                    }
                },
            ).into_element()
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_mode_stays_webview_across_non_structural_updates() {
        assert_eq!(
            compute_page_mode(true, "https://doc.rust-lang.org", None, true),
            PageMode::WebView
        );
        assert_eq!(
            compute_page_mode(false, "", None, false),
            PageMode::NoDocument
        );
        assert_eq!(
            compute_page_mode(true, "", None, false),
            PageMode::NoSources
        );
        assert_eq!(
            compute_page_mode(true, "", None, true),
            PageMode::Empty
        );
        assert_eq!(
            compute_page_mode(true, "https://doc.rust-lang.org", Some("offline"), true),
            PageMode::Error {
                message: "offline".to_owned(),
                url: "https://doc.rust-lang.org".to_owned()
            }
        );
    }
}
