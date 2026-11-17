//! The sidebar's project Search view (DESIGN.md 8.4.2): where and what to
//! search, a find bar, the grouped results outline, and a progress strip.
//! Matching, walking, encodings and cancellation are `scratch-search`'s
//! project engine (`spawn_search` streams `SearchEvent`s); this module owns
//! the view state and applies the events in batches.
//!
//! Motion inventory (DESIGN.md 9.4, "Sidebar"): rows appear as events stream
//! in without animation (a log, like the console); group disclosure and row
//! hovers use the fast color wash; switching to or from the navigator is
//! instant. The progress strip appears and disappears with the search.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use nsh_base::input;
use nsh_core::element::ImageSrc;
use nsh_core::element::desktop::{MenuItem, popup_menu_at};
use nsh_core::prelude::*;
use scratch_search::matcher::{MatchMode, Matcher};
use scratch_search::project::{
    DocumentSnapshot, ProjectMatch, SearchEvent, SearchMode, SearchOptions, SearchOutcome, SearchRequest, SearchSource,
    spawn_search,
};
use scratch_search::utf16_to_byte;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::files;
use super::navigator;
use super::preferences;
use super::state;

// ------------------------------------------------------------------ model

/// What the Where popup names.
#[derive(Debug, Clone, PartialEq)]
enum Where {
    /// The file navigator's root.
    Root,
    /// A folder picked with Browse….
    Folder(PathBuf),
    /// The items selected in the file navigator.
    Selection(Vec<PathBuf>),
    /// Every open document, from its live text.
    OpenDocuments,
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl Where {
    fn label(&self) -> String {
        match self {
            Where::Root => format!("Folder “{}”", folder_name(&navigator::root_path())),
            Where::Folder(path) => format!("Folder “{}”", folder_name(path)),
            Where::Selection(paths) if paths.len() == 1 => "Selected Item".to_owned(),
            Where::Selection(paths) => format!("{} Selected Items", paths.len()),
            Where::OpenDocuments => "All Open Documents".to_owned(),
        }
    }
}

/// What a search looks at (the `what` popup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    Both,
    Contents,
    Filenames,
}

impl What {
    fn label(self) -> &'static str {
        match self {
            What::Both => "File Contents + Filenames",
            What::Contents => "File Contents",
            What::Filenames => "Filenames",
        }
    }
    fn mode(self) -> SearchMode {
        match self {
            What::Both => SearchMode::ContentsAndFilenames,
            What::Contents => SearchMode::Contents,
            What::Filenames => SearchMode::Filenames,
        }
    }
}

/// One file's matches, a collapsible group in the outline.
#[derive(Debug, Clone, PartialEq)]
struct Group {
    path: PathBuf,
    name: String,
    dir: String,
    collapsed: bool,
    matches: Vec<ProjectMatch>,
}

/// A visible row: a group header or one of its matches.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Header(usize),
    Match(usize, usize),
}

/// The result groups, compared by pointer so a growing list republishes cheaply.
#[derive(Clone, Default)]
struct Groups(Rc<Vec<Group>>);

impl PartialEq for Groups {
    fn eq(&self, other: &Self) -> bool { Rc::ptr_eq(&self.0, &other.0) }
}

/// The search view's state.
#[derive(Clone, PartialEq)]
struct Search {
    shown_where: Where,
    what: What,
    query: String,
    ignore_case: bool,
    regex: bool,
    subdirectories: bool,
    hidden: bool,
    running: bool,
    status: String,
    groups: Groups,
    truncated: bool,
    total: usize,
    /// Bumped to rebuild the query field (focus/selection).
    generation: u64,
    focus_field: bool,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            shown_where: Where::Root,
            what: What::Both,
            query: String::new(),
            ignore_case: true,
            regex: false,
            subdirectories: true,
            hidden: false,
            running: false,
            status: String::new(),
            groups: Groups::default(),
            truncated: false,
            total: 0,
            generation: 0,
            focus_field: false,
        }
    }
}

fn search() -> Signal<Search> { global::<Search>() }

thread_local! {
    /// The cancellation token of the running search.
    static CANCEL: RefCell<Option<scratch_search::CancelToken>> = const { RefCell::new(None) };
    /// The draining task's generation; an old drain exits on a mismatch.
    static DRAIN: Cell<u64> = const { Cell::new(0) };
}

/// Focuses the query field (the header's search button and View > Search in Files).
pub(super) fn focus_query() {
    search().update(|s| {
        s.generation += 1;
        s.focus_field = true;
    });
}

/// Runs a search for `query` in `source` at once (context-menu items and tests).
#[allow(dead_code)]
pub(super) fn find_now(query: &str, source: SearchSource) {
    search().update(|s| s.query = query.to_owned());
    start(source);
}

// ------------------------------------------------------------------ engine

fn source_of(where_: &Where) -> SearchSource {
    match where_ {
        Where::Root => SearchSource::Folder(navigator::root_path()),
        Where::Folder(path) => SearchSource::Folder(path.clone()),
        Where::Selection(paths) => SearchSource::SelectedItems(paths.clone()),
        Where::OpenDocuments => SearchSource::OpenDocuments(state::with(|workspace| {
            workspace
                .documents
                .iter()
                .map(|doc| {
                    DocumentSnapshot::new(
                        doc.title.clone(),
                        state::saved_path(&doc.document),
                        state::text(doc.document.id).unwrap_or_else(|| doc.text.clone()),
                    )
                })
                .collect()
        })),
    }
}

fn start(source: SearchSource) {
    let (query, what, ignore_case, regex, subdirectories, hidden) =
        search().with_untracked(|s| (s.query.clone(), s.what, s.ignore_case, s.regex, s.subdirectories, s.hidden));
    if query.is_empty() {
        return;
    }
    let mode = if regex { MatchMode::Regex } else { MatchMode::Text };
    let matcher = match Matcher::simple(&query, mode, ignore_case) {
        Ok(matcher) => matcher,
        Err(error) => {
            search().update(|s| s.status = format!("Invalid pattern: {error}"));
            return;
        }
    };
    let excluded = preferences::settings().with_untracked(|p| {
        p.ui
            .search_excluded_directories
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
    });
    let mut request = SearchRequest::new(source, matcher);
    request.mode = what.mode();
    request.options = SearchOptions { subdirectories, hidden, binary: false, excluded_directories: excluded, ..SearchOptions::default() };
    let handle = spawn_search(request);
    CANCEL.with(|c| c.replace(Some(handle.token().clone())));
    DRAIN.with(|d| d.set(d.get() + 1));
    let generation = DRAIN.with(|d| d.get());
    search().update(|s| {
        s.running = true;
        s.status = "Searching…".to_owned();
        s.groups = Groups::default();
        s.truncated = false;
        s.total = 0;
    });
    spawn(async move {
        loop {
            let mut done = false;
            while let Ok(event) = handle.events().try_recv() {
                done |= apply(event);
            }
            if done || DRAIN.with(|d| d.get()) != generation {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    });
}

/// Applies one engine event; returns true for `Done`.
fn apply(event: SearchEvent) -> bool {
    match event {
        SearchEvent::FileStart { path, .. } => {
            search().update(|s| {
                let groups = Rc::make_mut(&mut s.groups.0);
                if !groups.iter().any(|g| g.path == path) {
                    groups.push(Group::new(path));
                }
            });
            false
        }
        SearchEvent::Match { path, matched, .. } => {
            search().update(|s| {
                s.total += 1;
                let groups = Rc::make_mut(&mut s.groups.0);
                match groups.iter_mut().find(|g| g.path == path) {
                    Some(group) => group.matches.push(matched),
                    None => {
                        let mut group = Group::new(path);
                        group.matches.push(matched);
                        groups.push(group);
                    }
                }
            });
            false
        }
        SearchEvent::Progress(progress) => {
            search().update(|s| {
                s.status = match progress.current {
                    Some(current) => format!("Searching {}…", folder_name(&current)),
                    None => format!("Searching {} files…", progress.files_searched),
                };
            });
            false
        }
        SearchEvent::Done(done) => {
            CANCEL.with(|c| c.replace(None));
            search().update(|s| {
                s.running = false;
                s.truncated = done.truncated;
                s.status = match done.outcome {
                    SearchOutcome::Cancelled => "Stopped".to_owned(),
                    SearchOutcome::Failed => "Search failed".to_owned(),
                    _ => match done.matches {
                        0 => "No Results".to_owned(),
                        1 => "1 match".to_owned(),
                        n => format!("{n} matches"),
                    },
                };
                if !done.errors.is_empty() {
                    s.status = format!("{} · {} error(s)", s.status, done.errors.len());
                }
            });
            true
        }
    }
}

impl Group {
    fn new(path: PathBuf) -> Group {
        Group {
            name: folder_name(&path),
            dir: path.parent().map_or_else(String::new, pretty),
            path,
            collapsed: false,
            matches: Vec::new(),
        }
    }
}

fn stop() {
    CANCEL.with(|c| {
        if let Some(token) = c.borrow().as_ref() {
            token.cancel();
        }
    });
}

/// A folder as it reads in a subtitle: `~/…` under the home folder.
fn pretty(path: &Path) -> String {
    match path.strip_prefix(navigator::home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

// ------------------------------------------------------------------ rows

fn rows_of(groups: &[Group]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        rows.push(Row::Header(index));
        if !group.collapsed {
            for m in 0..group.matches.len() {
                rows.push(Row::Match(index, m));
            }
        }
    }
    rows
}

fn open_match(group: usize, at: usize) {
    let Some((path, line)) = search().with_untracked(|s| {
        s.groups.0.get(group).and_then(|g| g.matches.get(at).map(|m| (g.path.clone(), m.display_line())))
    }) else {
        return;
    };
    let owner = Owner::current();
    owner.run(|| { spawn(files::open_path_at(path, line)); });
}

// ------------------------------------------------------------------ views

/// The search view: header, find bar, results, progress, footer.
pub fn sidebar() -> impl IntoElement {
    view().flex_1().min_h(px(0.0)).flex_col().child(header()).child(find_bar()).child(results()).child(progress()).child(footer())
}

/// A view that opens a native menu below itself on press (the popup look of
/// the navigator's own header button).
fn menu_trigger(label: &'static str, items: impl Fn() -> Vec<MenuItem> + 'static, child: impl IntoElement) -> impl IntoElement {
    view()
        .role(AccessibilityRole::Button)
        .label(label)
        .focusable(true)
        .h(px(24.0))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .px(px(6.0))
        .rounded(px(tokens::radius()))
        .hover(|s| s.bg(tokens::sidebar_accent))
        .active(|s| s.opacity(0.7))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
        .when(!motion::reduced(), motion::state_wash)
        .when(!motion::reduced(), motion::press)
        .active(|s| motion::press_scale(s, true))
        .on_press(move |e: &PressEvent| {
            let at = point(e.page_position.x - e.position.x, e.page_position.y - e.position.y + px(26.0));
            drop(popup_menu_at(items(), at));
        })
        .child(child)
        .child(glyph(IconName::ChevronDown, IconSize::Xs, Tone::Muted))
}

fn header() -> impl IntoElement {
    let where_label = computed(|| search().with(|s| SharedString::from(s.shown_where.label())));
    view().h(px(32.0)).flex_shrink(0.0).flex_row().items_center().gap(px(6.0)).px(px(8.0))
        .border_b_1().border_color(tokens::sidebar_border)
        .child(text("Search").text_sm().font_weight(FontWeight::MEDIUM).text_color(tokens::sidebar_foreground))
        .child(view().flex_1().min_w(px(0.0)).child(menu_trigger("Where to search", where_menu,
            view().flex_1().min_w(px(0.0)).flex_row().items_center().gap(px(5.0))
                .child(glyph(IconName::Folder, IconSize::Md, Tone::Primary).flex_shrink(0.0))
                .child(text(move || where_label.get()).flex_1().min_w(px(0.0)).number_of_lines(1).text_sm().font_weight(FontWeight::MEDIUM)
                    .text_color(tokens::sidebar_foreground)))))
        .child(icon_button(IconName::Search, "Hide the search view", IconButtonOptions::new().size(ControlSize::Sm).selected(true),
            || navigator::show_search(false)))
}

fn where_menu() -> Vec<MenuItem> {
    let set = |w: Where| move || search().update(|s| s.shown_where = w.clone());
    let mut items = vec![
        MenuItem::new(Where::Root.label()).on_select(set(Where::Root)),
        MenuItem::new("Browse…").on_select(|| {
            spawn(async move {
                if let Ok(Some(paths)) = pick_paths(nsh_core::file_dialog::OpenDialog::folders()).await
                    && let Some(path) = paths.into_iter().next()
                {
                    search().update(|s| s.shown_where = Where::Folder(path));
                }
            });
        }),
        MenuItem::separator(),
        MenuItem::new("All Open Documents").on_select(set(Where::OpenDocuments)),
    ];
    let selected = navigator::selected_paths();
    if !selected.is_empty() {
        items.push(MenuItem::new(Where::Selection(selected.clone()).label()).on_select(set(Where::Selection(selected))));
    }
    items
}

fn find_bar() -> impl IntoElement {
    let generation = computed(|| search().with(|s| s.generation));
    let what_label = computed(|| search().with(|s| SharedString::from(s.what.label())));
    view().flex_shrink(0.0).flex_col().gap(px(6.0)).px(px(8.0)).py(px(8.0))
        .border_b_1().border_color(tokens::sidebar_border)
        .child(view().flex_row().items_center().gap(px(6.0))
            .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
                let _ = generation.get();
                let (initial, focus) = search().with_untracked(|s| (s.query.clone(), s.focus_field));
                query_field(initial, focus).into_element()
            }))))
        .child(view().flex_row().items_center().gap(px(4.0))
            .child(menu_trigger("What to search", what_menu,
                text(move || what_label.get()).text_xs().number_of_lines(1).text_color(tokens::sidebar_foreground)))
            .child(view().flex_1())
            .child(menu_trigger("Search Options", options_menu, glyph(IconName::Ellipsis, IconSize::Sm, Tone::Muted)))
            .child(button("Search", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Sm),
                || start(search().with_untracked(|s| source_of(&s.shown_where))))))
}

fn query_field(initial: String, focus: bool) -> impl IntoElement {
    let length = initial.encode_utf16().count() as u32;
    input()
        .default_value(initial)
        .selected_range((0, length))
        .auto_focus(focus)
        .placeholder("Find")
        .label("Search text")
        .submit_behavior(SubmitBehavior::Submit)
        .on_change(|text: &str| search().update(|s| s.query = text.to_owned()))
        .on_submit(|_: &str| start(search().with_untracked(|s| source_of(&s.shown_where))))
        .placeholder_color(|| color(tokens::muted_foreground()))
        .caret_color(|| color(tokens::primary()))
        .h(px(24.0))
        .w_full()
        .px_2()
        .text_sm()
        .rounded(px(tokens::radius()))
        .bg(tokens::field)
        .border_1()
        .border_color(tokens::input)
        .text_color(tokens::foreground)
}

fn what_menu() -> Vec<MenuItem> {
    let current = search().with_untracked(|s| s.what);
    [What::Both, What::Contents, What::Filenames]
        .iter()
        .map(|what| {
            let what = *what;
            MenuItem::new(what.label()).checked(what == current).on_select(move || search().update(|s| s.what = what))
        })
        .collect()
}

fn options_menu() -> Vec<MenuItem> {
    let (ignore_case, regex, subdirectories, hidden) =
        search().with_untracked(|s| (s.ignore_case, s.regex, s.subdirectories, s.hidden));
    vec![
        MenuItem::new("Ignore Case").checked(ignore_case).on_select(|| search().update(|s| s.ignore_case = !s.ignore_case)),
        MenuItem::new("Regular Expression").checked(regex).on_select(|| search().update(|s| s.regex = !s.regex)),
        MenuItem::separator(),
        MenuItem::new("Search Subdirectories").checked(subdirectories).on_select(|| search().update(|s| s.subdirectories = !s.subdirectories)),
        MenuItem::new("Hidden Files").checked(hidden).on_select(|| search().update(|s| s.hidden = !s.hidden)),
    ]
}

fn results() -> impl IntoElement {
    let rows = computed(|| search().with(|s| Rc::new(rows_of(&s.groups.0))));
    view().flex_1().min_h(px(0.0)).child(
        when(move || search().with(|s| s.groups.0.is_empty() && !s.running), || {
            view().flex_1().items_center().justify_center().p_4().flex_col().gap_2()
                .child(glyph(IconName::Search, IconSize::Xl, Tone::Muted))
                .child(type_subhead().apply(text("Search")).text_color(tokens::sidebar_foreground))
                .child(type_caption().apply(text("Search for text in files, folders and open documents."))
                    .text_color(tokens::muted_foreground).text_center())
                .into_element()
        })
        .otherwise(move || {
            virtual_list(move || rows.with(|r| r.len()), move |at| {
                match rows.with(|r| r.get(at.index()).cloned()) {
                    Some(Row::Header(g)) => header_row(g).into_element(),
                    Some(Row::Match(g, m)) => match_row(g, m).into_element(),
                    None => view().into_element(),
                }
            })
            .estimated_item_size(px(28.0))
            .flex_1()
            .min_h(px(0.0))
            .into_element()
        }),
    )
}

/// The match/group context menu (native).
fn result_menu(group: usize, at: Option<usize>) -> Vec<MenuItem> {
    let target = search().with_untracked(|s| {
        s.groups.0.get(group).map(|g| {
            (
                g.path.clone(),
                at.and_then(|at| g.matches.get(at)).map(|m| m.display_line()),
            )
        })
    });
    let Some((path, line)) = target else { return Vec::new() };
    let copy = path.clone();
    let nav = path.clone();
    let finder = path.clone();
    vec![
        MenuItem::new("Open").on_select(move || {
            let owner = Owner::current();
            let line = line.unwrap_or(1);
            owner.run(|| { spawn(files::open_path_at(path.clone(), line)); });
        }),
        MenuItem::new("Copy Path").on_select(move || set_clipboard(copy.display().to_string())),
        MenuItem::separator(),
        MenuItem::new("Show in File Navigator").on_select(move || {
            navigator::show_search(false);
            navigator::reveal(&nav);
        }),
        MenuItem::new("Show in Finder").on_select(move || {
            let path = finder.clone();
            spawn(async move {
                if let Err(error) = reveal_path(path).await {
                    state::notice(format!("Couldn't show the item in Finder: {error}"));
                }
            });
        }),
    ]
}

fn header_row(group: usize) -> impl IntoElement {
    let data = move || {
        search().with(|s| {
            s.groups
                .0
                .get(group)
                .map(|g| (g.name.clone(), g.dir.clone(), g.matches.len(), g.collapsed))
                .unwrap_or_default()
        })
    };
    view().h(px(28.0)).flex_row().items_center().gap(px(6.0)).px(px(8.0))
        .role(AccessibilityRole::Button)
        .label(move || data().0)
        .hover(|s| s.bg(tokens::sidebar_accent))
        .on_press(move |_: &PressEvent| {
            search().update(|s| {
                if let Some(g) = Rc::make_mut(&mut s.groups.0).get_mut(group) {
                    g.collapsed = !g.collapsed;
                }
            });
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            drop(popup_menu_at(result_menu(group, None), e.page_position));
        })
        .child(when(move || data().3, || glyph(IconName::ChevronRight, IconSize::Xs, Tone::Muted).into_element())
            .otherwise(|| glyph(IconName::ChevronDown, IconSize::Xs, Tone::Muted).into_element()))
        .child(svg(move || ImageSrc::from(navigator::file_icon(&data().0).asset())).size(px(tokens::icon_md()))
            .text_color(tokens::muted_foreground))
        .child(view().flex_1().min_w(px(0.0))
            .child(text(move || SharedString::from(data().0)).text_sm().font_weight(FontWeight::MEDIUM).number_of_lines(1)
                .text_color(tokens::sidebar_foreground))
            .child(text(move || SharedString::from(data().1)).text_xs().number_of_lines(1)
                .text_color(tokens::muted_foreground)))
        .child(badge(move || SharedString::from(data().2.to_string()), BadgeOptions::new().variant(BadgeVariant::Count)))
}

/// A match's line split into (before, match, after), plus its 1-based line.
fn match_data(group: usize, at: usize) -> Option<(String, String, String, u32)> {
    search().with(|s| {
        let group = s.groups.0.get(group)?;
        let m = group.matches.get(at)?;
        let raw = &m.line_text;
        let start = utf16_to_byte(raw, m.column as usize).min(raw.len());
        let end = utf16_to_byte(raw, (m.column + m.length) as usize).min(raw.len()).max(start);
        Some((raw[..start].trim_start().to_owned(), raw[start..end].to_owned(), raw[end..].to_owned(), m.display_line()))
    })
}

fn match_row(group: usize, at: usize) -> impl IntoElement {
    let data = move || match_data(group, at).unwrap_or_default();
    let before = move || SharedString::from(data().0);
    let matched = move || SharedString::from(data().1);
    let after = move || SharedString::from(data().2);
    let line = move || SharedString::from(data().3.to_string());
    view().h(px(28.0)).flex_row().items_center().gap(px(6.0)).px(px(8.0))
        .role(AccessibilityRole::Button)
        .label(move || format!("{}, line {}", data().1, data().3))
        .hover(|s| s.bg(tokens::sidebar_accent))
        .on_press(move |_: &PressEvent| open_match(group, at))
        .on_context_menu(move |e: &ContextMenuEvent| {
            drop(popup_menu_at(result_menu(group, Some(at)), e.page_position));
        })
        .child(text(line).text_xs().tabular_nums().w(px(32.0)).text_right()
            .text_color(tokens::muted_foreground))
        .child(view().flex_1().min_w(px(0.0)).flex_row().items_center().overflow_hidden()
            .child(text(before).text_xs().number_of_lines(1).font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                .text_color(tokens::muted_foreground))
            .child(text(matched).text_xs().number_of_lines(1).font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                .bg(tokens::match_highlight)
                .text_color(tokens::sidebar_foreground))
            .child(text(after).text_xs().number_of_lines(1).font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                .text_color(tokens::sidebar_foreground)))
}

fn progress() -> impl IntoElement {
    view().child(when(move || search().with(|s| s.running), || {
        view().h(px(28.0)).flex_shrink(0.0).flex_row().items_center().gap(px(8.0)).px(px(8.0))
            .border_t_1().border_color(tokens::sidebar_border)
            .child(spinner(IconSize::Xs, Tone::Muted, "Searching"))
            .child(text(move || search().with(|s| SharedString::from(s.status.clone()))).text_xs()
                .text_color(tokens::muted_foreground).flex_1().min_w(px(0.0)).number_of_lines(1))
            .child(button("Stop", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Xs), stop))
            .into_element()
    }))
}

fn footer() -> impl IntoElement {
    view().h(px(24.0)).flex_shrink(0.0).flex_row().items_center().gap(px(8.0)).px(px(8.0))
        .border_t_1().border_color(tokens::sidebar_border)
        .child(text(move || {
            search().with(|s| {
                if s.running {
                    SharedString::from(s.status.clone())
                } else {
                    match s.total {
                        0 => SharedString::from(""),
                        1 => SharedString::from("1 match"),
                        n => SharedString::from(format!("{n}{} matches", if s.truncated { "+" } else { "" })),
                    }
                }
            })
        }).text_xs().text_color(tokens::muted_foreground).flex_1().number_of_lines(1))
        .child(button("Done", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Xs), || {
            navigator::show_search(false)
        }))
}
