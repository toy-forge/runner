//! The window's chrome: the toolbar with the language, the run group and the
//! panel toggles; the tab strip; and the status bar with the cursor position.
//! Controls are the app's ui-kit primitives (DESIGN.md 5 to 7).
//!
//! Motion inventory (DESIGN.md 9):
//! - Toolbar buttons: hover wash and press scale (ui-kit).
//! - Panel toggles: the selected capsule changes at once, as the panel does.
//! - Tabs: the selected pill follows the selection (`tabs()` indicator); the
//!   close control fades in on hover.
//! - Status bar: text updates in place, no animation.

use nsh_base::{EditorPosition, EditorSelection, PopoverEvent, popover, tab, tabs};
use nsh_core::desktop::WindowAttributes;
use nsh_core::element::desktop::{MenuItem, current_window, popup_menu, popup_menu_at};
use nsh_core::prelude::*;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::debugger;
use super::diagnostics;
use super::execution;
use super::files;
use super::preferences;
use super::run_settings;
use super::state::{self, display, with};
use scratch_document::DocumentId;

/// Placeholder root overlay slot (status bar popovers anchor directly to their buttons).
pub(super) fn overlay() -> impl IntoElement {
    view()
}

/// The cursor position and selection count of the active editor, 1-based, for the status bar.
#[derive(Clone, PartialEq, Default)]
struct CursorPosition {
    line: u32,
    column: u32,
    selected_chars: usize,
    selected_cols: Option<usize>,
}

#[derive(Clone, PartialEq, Default)]
struct SymbolPopoverState {
    open: bool,
    filter: String,
}

/// The name the window reports to the system, so a change is sent once.
#[derive(Clone, PartialEq, Default)]
struct WindowTitle(String);

/// Whether the window was narrow at the last check, so the documentation
/// panel is closed only when it crosses the threshold.
#[derive(Clone, PartialEq, Default)]
struct NarrowWindow(bool);

/// Starts reactive cursor/title observers. Called once from the root.
pub fn install() {
    effect(|| {
        let _ = display().with(|view| view.active);
        refresh_cursor();
    });
    // Below 720 pt the documentation panel closes (DESIGN 2.2). A user who
    // reopens it keeps it until the window narrows again.
    effect(move || {
        let narrow = window().width.0 < 720.0;
        let current = global::<NarrowWindow>();
        if current.get_untracked().0 != narrow {
            current.set(NarrowWindow(narrow));
            if narrow {
                display().update(|view| view.docs_visible = false);
            }
        }
    });
    // The window title follows the document, as a document app's does. The
    // hidden title bar itself is set once, in `start`.
    let window = current_window();
    effect(move || {
        let next = display().with(|view| {
            view.active
                .and_then(|id| view.tabs.iter().find(|tab| tab.id == id))
                .map(|tab| tab.title.clone())
                .unwrap_or_else(|| "ScratchRunner".to_owned())
        });
        let current = global::<WindowTitle>();
        if current.get_untracked().0 != next {
            current.set(WindowTitle(next.clone()));
            let mut attributes = WindowAttributes::default();
            attributes.title = Some(next);
            window.set(attributes);
        }
    });
}

fn selection_char_count(text: &str, selections: &[EditorSelection]) -> usize {
    selections
        .iter()
        .map(|sel| {
            if sel.anchor == sel.head {
                return 0;
            }
            let (start, end) = if (sel.anchor.line, sel.anchor.column) <= (sel.head.line, sel.head.column) {
                (sel.anchor, sel.head)
            } else {
                (sel.head, sel.anchor)
            };
            let line_offset = |target_line: u32, target_col: u32| -> usize {
                let mut offset = 0usize;
                for (idx, line) in text.split('\n').enumerate() {
                    if idx == target_line as usize {
                        return offset + scratch_search::utf16_to_byte(line, target_col as usize);
                    }
                    offset += line.len() + 1;
                }
                text.len()
            };
            let a = line_offset(start.line, start.column);
            let b = line_offset(end.line, end.column);
            let lo = a.min(b).min(text.len());
            let hi = a.max(b).min(text.len());
            text[lo..hi].chars().count()
        })
        .sum()
}

/// Called by the mounted editor whenever its selection changes.
pub(super) fn selection_changed(id: DocumentId, selections: &[EditorSelection]) {
    let active = with(|workspace| workspace.active);
    if active != Some(id) { return; }
    let head = selections.last().or_else(|| selections.first()).map(|s| s.head);
    let selected_chars = if selections.iter().all(|s| s.anchor == s.head) {
        0
    } else {
        state::text(id)
            .map(|text| selection_char_count(&text, selections))
            .unwrap_or(0)
    };
    let selected_cols = if selections.len() == 1
        && selections[0].anchor != selections[0].head
        && selections[0].anchor.line == selections[0].head.line
    {
        Some(selections[0].anchor.column.abs_diff(selections[0].head.column) as usize)
    } else {
        None
    };
    let next = head.map_or(
        CursorPosition { line: 1, column: 1, selected_chars: 0, selected_cols: None },
        |pos| CursorPosition { line: pos.line + 1, column: pos.column + 1, selected_chars, selected_cols },
    );
    let current = global::<CursorPosition>();
    if current.get_untracked() != next {
        current.set(next);
    }
}

fn refresh_cursor() {
    let active = with(|workspace| workspace.active);
    if let Some(id) = active {
        if let Some(api) = state::editor_api(id) {
            selection_changed(id, &api.selections());
            return;
        }
    }
    let current = global::<CursorPosition>();
    let reset = CursorPosition { line: 1, column: 1, selected_chars: 0, selected_cols: None };
    if current.get_untracked() != reset {
        current.set(reset);
    }
}

/// Shows or hides the tab bar (View > Hide Tab Bar, ⌥⇧⌘T).
pub fn toggle_tab_bar() {
    motion::animate_panel();
    display().update(|view| view.tab_bar_visible = !view.tab_bar_visible);
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
}

/// Shows or hides the navigator.
pub fn toggle_navigator() {
    motion::animate_panel();
    display().update(|view| view.sidebar_visible = !view.sidebar_visible);
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
}

/// Shows the console at the bottom or on the right, or hides it when it is
/// already there.
pub fn toggle_console_at(right: bool) {
    motion::animate_panel();
    display().update(|view| {
        if view.console_visible && view.console_right == right {
            view.console_visible = false;
        } else {
            view.console_visible = true;
            view.console_right = right;
        }
    });
    with(|workspace| workspace.changed());
}

/// Moves the console to `right` and shows it (View > Console Position).
pub fn set_console_position(right: bool) {
    motion::animate_panel();
    display().update(|view| {
        view.console_visible = true;
        view.console_right = right;
    });
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
    super::preferences::write(|prefs| {
        prefs.general.console.position = if right {
            scratch_config::ConsolePosition::Right
        } else {
            scratch_config::ConsolePosition::Bottom
        };
    });
}

/// Shows or hides the console where it is.
pub fn toggle_console() {
    motion::animate_panel();
    display().update(|view| view.console_visible = !view.console_visible);
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
}

/// "Hide/Show Toolbar" (View menu).
pub fn toggle_toolbar() {
    motion::animate_panel();
    display().update(|view| view.toolbar_visible = !view.toolbar_visible);
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
}

/// Flips the "Automatically Hide Console" preference.
pub fn toggle_console_auto_hide() {
    super::preferences::write(|prefs| {
        prefs.general.console.hide_automatically = !prefs.general.console.hide_automatically;
    });
}

/// The title band (DESIGN 8.2): the document's language tile and name,
/// centered between the traffic lights. It replaces the system title bar, so
/// the window names the document before any tab is read. The band is the drag
/// region; nothing in it is interactive.
pub fn title_band() -> impl IntoElement {
    let active = move || {
        display().with(|view| {
            view.active
                .and_then(|id| view.tabs.iter().find(|tab| tab.id == id))
                .cloned()
        })
    };
    view()
        .h(px(tokens::titlebar_h()))
        .w_full()
        .flex_row()
        .items_center()
        .justify_center()
        .bg(tokens::toolbar)
        .opacity(move || if display().with(|view| view.window_focused) { 1.0_f32 } else { 0.78_f32 })
        .window_drag(true)
        .child(dynamic(move || {
            let (title, dirty, language) = match active() {
                Some(tab) => (tab.title, tab.dirty, tab.language_id),
                None => ("ScratchRunner".to_owned(), false, "plain-text".to_owned()),
            };
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .max_w(move || px((window().width.0 - 180.0).clamp(140.0, 560.0)))
                .child(language_glyph_tile(&language))
                .child(
                    type_body()
                        .medium()
                        .apply(text(title))
                        .text_color(tokens::foreground)
                        .number_of_lines(1)
                        .truncate(),
                )
                .child(when(move || dirty, move || {
                    type_body()
                        .apply(text(" – Edited"))
                        .text_color(tokens::muted_foreground)
                        .flex_shrink(0.0)
                }))
                .into_element()
        }))
}

/// The toolbar: the language, the run group, and the panel toggles.
pub fn toolbar() -> impl IntoElement {
    view().h(px(40.0)).w_full().flex_row().items_center().gap_2().px_3()
        .border_b_1().border_color(tokens::toolbar_border).bg(tokens::toolbar)
        .opacity(move || if display().with(|view| view.window_focused) { 1.0_f32 } else { 0.78_f32 })
        .child(when(move || !display().with(|view| view.loading),
            || view().w(px(200.0)).min_w(px(120.0)).max_w(px(240.0)).child(language_popup())))
        .child(run_group())
        .child(view().flex_1())
        .child(when(move || window().width.0 >= 620.0, history_group))
        .child(when(move || window().width.0 >= 560.0, || {
            icon_button(IconName::Gear, "Preferences", IconButtonOptions::new().shortcut("⌥⌘,"), preferences::open)
        }))
        .child(panel_toggles())
}

fn language_popup() -> impl IntoElement {
    // The items are built at press time from a snapshot: rebuild the popup
    // when the document or the registry changes so the checkmark and the
    // `None` entry stay right.
    dynamic(move || {
        global::<state::WorkspaceRevision>().get();
        global::<state::RegistryRevision>().get();
        popup_button("Choose the language of this document", current_language, super::edit::language_items(), PopupButtonOptions::new(), |_open: bool| {})
            .into_element()
    })
}

/// The language of the active tab, for the popup's value (`Syntax: X`).
fn current_language() -> SharedString {
    display().with(|view| {
        let name = view.active
            .and_then(|id| view.tabs.iter().find(|tab| tab.id == id))
            .map(|tab| tab.language.clone());
        SharedString::from(format!("Syntax: {}", name.unwrap_or_else(|| "None".to_owned())))
    })
}

/// The active document's run state (`Signal<bool>` under the tab).
fn active_running() -> bool {
    with(|workspace| workspace.active.and_then(|id| workspace.doc(id)).map(|doc| doc.running.get()).unwrap_or(false))
}

/// The active document has run settings of its own (the wrench turns Primary).
fn custom_run_settings() -> bool {
    with(|workspace| {
        workspace.active.and_then(|id| workspace.doc(id))
            .map(|doc| doc.document.run != scratch_document::RunSettings::default()).unwrap_or(false)
    })
}

/// Whether the active language has something to run: a run command, or an
/// in-process renderer (Markdown's preview).
pub(super) fn can_run() -> bool {
    with(|workspace| {
        let Some(id) = workspace.active else { return false; };
        let Some(doc) = workspace.doc(id) else { return false; };
        let Some(registry) = workspace.registry.as_ref() else { return false; };
        let Some(def) = registry.get(&doc.document.language) else { return false; };
        !def.run.command.trim().is_empty() || def.console.renderer.is_some()
    })
}

fn run_primary_clicked() {
    execution::run_active();
}

thread_local! {
    static RUN_MENU_LONG_PRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn run_group() -> impl IntoElement {
    capsule_group("Run", CapsuleGroupOptions::new(), [
        dynamic(move || {
            // While a run is active the Run control is a spinner that stops
            // the run — CodeRunner's running state. The `active` read keeps
            // the dynamic subscribed across document switches.
            let _ = display().with(|view| view.active);
            if active_running() {
                view().on_context_menu(|e: &ContextMenuEvent| { drop(popup_menu_at(run_menu(), e.page_position)); })
                    .on_press(|_: &PressEvent| execution::stop_active())
                    .role(AccessibilityRole::Button)
                    .label("Stop")
                    .focusable(true)
                    .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
                    .child(spinner(IconSize::Sm, Tone::Muted, "Stop")).into_element()
            } else {
                let label = move || SharedString::from(if can_run() { "Run" } else { "No run command for this language" });
                view().relative().group()
                    .on_context_menu(|e: &ContextMenuEvent| { drop(popup_menu_at(run_menu(), e.page_position)); })
                    .on_long_press(|e: &PressEvent| {
                        RUN_MENU_LONG_PRESSED.with(|flag| flag.set(true));
                        let pos = e.page_position;
                        Owner::detached().run(|| {
                            spawn(async move {
                                let _ = popup_menu_at(run_menu(), pos).await;
                                RUN_MENU_LONG_PRESSED.with(|flag| flag.set(false));
                            });
                        });
                    })
                    .child(state_button(IconName::Run, label, IconButtonOptions::new().shortcut("⌘R"), move || active_running() || !can_run(), || {
                        if RUN_MENU_LONG_PRESSED.with(|flag| flag.replace(false)) {
                            return;
                        }
                        run_primary_clicked();
                    }))
                    .child(view().absolute().bottom(px(1.0)).right(px(1.0))
                        .pointer_events(PointerEvents::None)
                        .opacity(0.0)
                        .group_hover(|s| s.opacity(0.85))
                        .child(glyph_at(IconName::ChevronDown, 8.0, Tone::Muted)))
                    .into_element()
            }
        }).into_element(),
        state_button(IconName::Stop, "Stop", IconButtonOptions::new().shortcut("⌘."), || !active_running(), execution::stop_active).into_element(),
        dynamic(move || {
            let custom = custom_run_settings();
            icon_button(IconName::Wrench, if custom { "Custom run settings in effect. Click to edit…" } else { "Run Settings" },
                IconButtonOptions::new().shortcut("⇧⌘R").selected(custom), run_settings::open)
        }).into_element(),
    ])
}

/// The Run button's pull-down menu (CodeRunner's MenuButton): the Run
/// menu's run commands — enabled states mirror it.
fn run_menu() -> Vec<MenuItem> {
    let running = active_running();
    let compiles = with(|workspace| {
        workspace.active.and_then(|id| workspace.doc(id))
            .and_then(|doc| workspace.registry.as_ref().and_then(|r| r.get(&doc.document.language)))
            .is_some_and(|def| def.has_compile_step())
    });
    let can_run = can_run();
    vec![
        MenuItem::new("Run").disabled(running || !can_run).on_select(execution::run_active),
        MenuItem::new("Run and Debug").disabled(running || !can_run || !debugger_available()).on_select(debugger::run_in_debugger),
        MenuItem::new("Run Without Debugging").disabled(running || !can_run).on_select(execution::run_without_debugging),
        MenuItem::new("Run Selection").disabled(running || !can_run).on_select(execution::run_selection),
        MenuItem::new(if compiles { "Compile" } else { "Run Diagnostics" }).on_select(execution::compile_or_diagnostics),
    ]
}

/// Whether the active document's language has a debugger adapter.
fn debugger_available() -> bool {
    with(|workspace| workspace.active.and_then(|id| workspace.doc(id))
        .and_then(|doc| workspace.registry.as_ref().and_then(|registry| registry.get(&doc.document.language)))
        .is_some_and(|def| scratch_debugger::AdapterRegistry::builtin().for_language(&def.id).is_some()))
}

/// Toolbar Back/Forward (caret history, DESIGN 8.3).
fn history_group() -> impl IntoElement {
    capsule_group("Caret History", CapsuleGroupOptions::new(), [
        state_button(IconName::ChevronLeft, "Go Back to Previous Caret Location", IconButtonOptions::new().shortcut("⌃⌘←"),
            || !display().with(|view| view.can_go_back), || state::nav_step(true)).into_element(),
        state_button(IconName::ChevronRight, "Go Forward to Next Caret Location", IconButtonOptions::new().shortcut("⌃⌘→"),
            || !display().with(|view| view.can_go_forward), || state::nav_step(false)).into_element(),
    ])
}

fn panel_toggles() -> impl IntoElement {
    capsule_group("Panels", CapsuleGroupOptions::new(), [
        toggle(|| IconName::SidebarLeft, "Hide or show the Navigator", Some("⇧⌘H"), || display().with(|view| view.sidebar_visible), toggle_navigator),
        toggle(|| if super::console::console_on_right() { IconName::ConsoleRight } else { IconName::ConsoleBottom },
            "Hide or show the Console", Some("⇧⌘C"), || display().with(|view| view.console_visible), toggle_console),
        toggle(|| IconName::SidebarRight, "Hide or show the Documentation View", Some("⇧⌘I"), || display().with(|view| view.docs_visible), toggle_docs),
    ])
}

/// Toggles the documentation view (the right panel).
pub fn toggle_docs() {
    let opening = display().with(|view| !view.docs_visible);
    // A user may reopen it after the automatic close, but not so narrow that
    // the 240 pt panel would crush the 240 pt editor.
    if opening && window().width.0 < 680.0 {
        state::notice("Needs a wider window.");
        return;
    }
    motion::animate_panel();
    display().update(|view| view.docs_visible = !view.docs_visible);
    with(|workspace| workspace.changed());
    super::state::bump_workspace_rev();
}

/// An IconButton whose disabled look follows `disabled` (the two variants
/// swap under `when`, which also re-subscribes on state changes). The label
/// is reactive so a disabled control can say why it is disabled.
fn state_button(
    icon: IconName,
    label: impl IntoReactive<SharedString> + Clone + 'static,
    options: IconButtonOptions,
    disabled: impl Fn() -> bool + Clone + 'static,
    action: impl Fn() + Clone + 'static,
) -> AnyElement {
    let disabled_now = disabled.clone();
    let action_now = action.clone();
    let on_label = label.clone();
    let off_label = label;
    view()
        .child(when(disabled, move || icon_button(icon, on_label.clone(), options.disabled(true), action.clone())))
        .child(when(move || !disabled_now(), move || icon_button(icon, off_label.clone(), options.disabled(false), action_now.clone())))
        .into_element()
}

/// A toggle button whose selected look follows `on`; the icon may be dynamic.
fn toggle(
    icon: impl Fn() -> IconName + Clone + 'static,
    label: &'static str,
    shortcut: Option<&'static str>,
    on: impl Fn() -> bool + Clone + 'static,
    action: impl Fn() + Clone + 'static,
) -> AnyElement {
    dynamic(move || {
        let icon = icon();
        let (on, action) = (on.clone(), action.clone());
        let (on_now, action_now) = (on.clone(), action.clone());
        let options = move |selected: bool| {
            let options = IconButtonOptions::new().selected(selected);
            shortcut.map_or(options, |shortcut| options.shortcut(shortcut))
        };
        view()
            .child(when(on, move || icon_button(icon, label, options(true), action.clone())))
            .child(when(move || !on_now(), move || icon_button(icon, label, options(false), action_now.clone())))
            .into_element()
    }).into_element()
}

/// The tab strip (DESIGN 8.5): `[language tile][name][dot|close]` per tab, a
/// New Tab button, and the overflow menu listing every tab.
pub fn tab_bar() -> impl IntoElement {
    view().h(px(32.0)).w_full().flex_row().items_center().gap_1().px_2()
        .border_b_1().border_color(tokens::border).bg(tokens::tab_bar)
        .opacity(move || if display().with(|view| view.window_focused) { 1.0_f32 } else { 0.78_f32 })
        .child(view().flex_1().min_w(px(0.0)).h_full().flex_row().items_center()
            .child(tabs().flex_row().items_center().gap_1().flex_shrink(1.0).min_w(px(0.0)).overflow_hidden()
                .indicator_style(|s| s.top_1().bottom_1().rounded_md().bg(tokens::tab_active))
                .child(each(move || display().with(|view| view.tabs.clone()), |tab: &state::Tab| tab.id, |row, _| {
                    let id = row.get_untracked().id;
                    view().group().on_context_menu(move |e: &ContextMenuEvent| { drop(popup_menu_at(tab_menu(id), e.page_position)); })
                    .child(
                    tab().selected(move || display().with(|view| view.active == Some(id)))
                        .on_press(move || state::activate(id))
                        .accessibility_label(move || Some(SharedString::from(row.with(tab_title_tip))))
                        .h(px(26.0)).flex_row().px_2().gap_1p5().items_center().rounded(move || px(tokens::radius()))
                        .hover(|s| s.bg(tokens::tab_hover))
                        .child(dynamic(move || {
                            // The tile follows the tab's language row, not a
                            // capture: a `set_language` on the document repaints
                            // it (row subscribes to the tab's data).
                            let language_id = row.with(|tab| tab.language_id.clone());
                            if with(|workspace| workspace.doc(id).map(|doc| doc.running.get()).unwrap_or(false)) {
                                spinner(IconSize::Xs, Tone::Muted, "Running").into_element()
                            } else {
                                language_glyph_tile(&language_id).into_element()
                            }
                        }))
                        .child(text(move || row.with(|tab| tab.title.clone())).text_sm()
                            .text_color(move || if display().with(|view| view.active == Some(id)) { tokens::tab_active_foreground() } else { tokens::tab_foreground() })
                            .max_w(px(160.0)).truncate())
                        .child(view().relative().w(px(16.0)).h(px(16.0)).items_center().justify_center().flex_shrink(0.0)
                            .child(when(move || row.with(|tab| tab.dirty), || {
                                view().w(px(6.0)).h(px(6.0)).rounded_full().bg(tokens::foreground)
                                    .group_hover(|s| s.opacity(0.0))
                            }))
                            .child(view().absolute().inset_0().items_center().justify_center()
                                .opacity(0.0)
                                .style_when(move || !row.with(|tab| tab.dirty) && display().with(|view| view.active == Some(id)), |s| s.opacity(0.65))
                                .group_hover(|s| s.opacity(1.0))
                                .child(icon_button(IconName::Close, "Close Tab", IconButtonOptions::new().size(ControlSize::Xs),
                                    move || files::close(id))))))
                })))
            .child(view().flex_1().h_full().min_w(px(12.0))
                .on_press(|e: &PressEvent| {
                    if e.clicks >= 2 {
                        state::new_document();
                    }
                })))
        .child(icon_button(IconName::Plus, "Create a new tab", IconButtonOptions::new().size(ControlSize::Sm).shortcut("⌘T"), state::new_document))
        .child(icon_button(IconName::ChevronDown, "Show all tabs", IconButtonOptions::new().size(ControlSize::Sm),
            || { spawn(async { let _ = popup_menu(tab_list_menu()).await; }); }))
}

/// The tab's tooltip: the full path, or `Unsaved document`.
fn tab_title_tip(tab: &state::Tab) -> String {
    state::with(|workspace| workspace.doc(tab.id).and_then(|doc| state::saved_path(&doc.document)))
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Unsaved document".to_owned())
}

/// A tab's context menu (DESIGN 8.5): Run and Stop only.
fn tab_menu(id: DocumentId) -> Vec<MenuItem> {
    let running = with(|workspace| workspace.doc(id).map(|doc| doc.running.get()).unwrap_or(false));
    vec![
        MenuItem::new("Run").disabled(running).on_select(move || { state::activate(id); execution::run_active(); }),
        MenuItem::new("Stop").disabled(!running).on_select(move || { state::activate(id); execution::stop_active(); }),
    ]
}

/// The overflow menu: every tab, checked when selected.
fn tab_list_menu() -> Vec<MenuItem> {
    let (tabs, active) = display().with_untracked(|view| (view.tabs.clone(), view.active));
    tabs.into_iter().map(|tab| {
        let id = tab.id;
        MenuItem::new(&tab.title).checked(active == Some(id)).on_select(move || state::activate(id))
    }).collect()
}

/// The status bar (DESIGN 8.6.5): the run state and notice on the left; the
/// symbol, indentation and position status popups on the right.
pub fn status_bar() -> impl IntoElement {
    view().h(px(24.0)).w_full().flex_row().items_center().gap_3().px_3()
        .border_t_1().border_color(tokens::editor_bar_border).bg(tokens::editor_bar)
        .text_color(tokens::editor_bar_foreground)
        .child(dynamic(run_status))
        .child(dynamic(diagnostics_badge))
        .child(text(move || display().with(|view| view.notice.clone()))
            .text_xs().text_color(tokens::editor_bar_foreground).flex_1().min_w(px(0.0)).truncate())
        .child(symbol_button())
        .child(indentation_button())
        .child(position_button())
}

/// Clickable error/warning diagnostic counts for the active document.
fn diagnostics_badge() -> AnyElement {
    let _ = display().with(|view| view.active);
    let _ = global::<state::WorkspaceRevision>().get();
    let counts = with(|workspace| {
        let doc = workspace.active.and_then(|id| workspace.doc(id))?;
        let _ = doc.diag_rev.get();
        Some(doc.annotations.counts())
    }).unwrap_or_default();
    if counts.errors == 0 && counts.warnings == 0 {
        return view().into_element();
    }
    nsh_base::button()
        .accessibility_label("Jump to next diagnostic")
        .flex_row().items_center().gap_1p5().h(px(18.0)).px_1p5().rounded(px(tokens::radius_sm()))
        .cursor_pointer()
        .hover(|s| s.bg(tokens::accent))
        .on_press(move || diagnostics::jump(true, counts.errors > 0))
        .when(counts.errors > 0, |el| {
            el.child(view().flex_row().items_center().gap_0p5()
                .child(glyph(IconName::ErrorCircle, IconSize::Xs, Tone::Danger))
                .child(text(counts.errors.to_string()).text_xs().text_color(tokens::editor_bar_foreground)))
        })
        .when(counts.warnings > 0, |el| {
            el.child(view().flex_row().items_center().gap_0p5()
                .child(glyph(IconName::WarningTriangle, IconSize::Xs, Tone::Warning))
                .child(text(counts.warnings.to_string()).text_xs().text_color(tokens::editor_bar_foreground)))
        })
        .into_element()
}

/// The active document's run state item: a spinner while running with live
/// CPU/memory telemetry, then the outcome's icon, status, and "Show Console".
fn run_status() -> AnyElement {
    // The active read keeps this dynamic subscribed to document switches.
    let _ = display().with(|view| view.active);
    let console_visible = display().with(|view| view.console_visible);
    let (status, running, stats, exit_info) = with(|workspace| {
        workspace.active.and_then(|id| workspace.doc(id))
            .map(|doc| (doc.status.get(), doc.running.get(), doc.run_stats.get(), doc.exit_info.get()))
            .unwrap_or(("Ready".to_owned(), false, None, None))
    });
    if !running && status == "Ready" && exit_info.is_none() { return view().into_element(); }
    let appear = motion().entering(motion::fade_in_rise(motion::fast()));
    let mut row = view().flex_row().items_center().gap_1p5().flex_shrink(0.0);
    if running {
        row = row.child(spinner(IconSize::Xs, Tone::Muted, "Running"));
        let label = match stats {
            Some((cpu, rss)) if rss > 0 => format!("{status} · {cpu:.0}% CPU · {:.1} MB", rss as f64 / (1024.0 * 1024.0)),
            Some((cpu, _)) if cpu > 0.0 => format!("{status} · {cpu:.0}% CPU"),
            _ => status,
        };
        row = row.child(text(label).text_xs().text_color(tokens::editor_bar_foreground));
    } else {
        let lower = status.to_lowercase();
        let (icon, tone) = match &exit_info {
            Some(info) if info.canceled => (IconName::StopCircle, Tone::Muted),
            Some(info) if info.code == Some(0) => (IconName::SuccessCircle, Tone::Success),
            Some(_) => (IconName::ErrorCircle, Tone::Danger),
            None if lower.contains("succeeded") || lower.contains("ended") => (IconName::SuccessCircle, Tone::Success),
            None if lower.contains("fail") || lower.contains("error") => (IconName::ErrorCircle, Tone::Danger),
            None if lower.contains("stopped") || lower.contains("disconnected") => (IconName::StopCircle, Tone::Muted),
            None => (IconName::InfoCircle, Tone::Muted),
        };
        row = row.child(glyph(icon, IconSize::Xs, tone))
            .child(text(status).text_xs().text_color(tokens::editor_bar_foreground));
        if !console_visible {
            row = row.child(status_button("Show Console", false, toggle_console));
        }
    }
    row.bind_motion(&appear).into_element()
}

/// A status item that opens a native menu (or runs `action`) on press: the
/// label's text, an optional `chevrons-up-down`, hover wash.
fn status_button(
    label: impl IntoReactive<SharedString> + Clone + 'static,
    menu: bool,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    let shown = label.clone().into_reactive();
    let a11y = label.into_reactive();
    let mut item = nsh_base::button()
        .accessibility_label(move || Some(a11y.get()))
        .flex_row().items_center().gap_1().h(px(20.0)).px_1p5().rounded(px(tokens::radius_sm()))
        .cursor_pointer()
        .hover(|s| s.bg(tokens::accent))
        .active(|s| s.bg(tokens::capsule_pressed))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
        .when(!motion::reduced(), motion::state_wash)
        .when(!motion::reduced(), motion::press)
        .active(|s| motion::press_scale(s, true))
        .on_press(action)
        .child(text(move || shown.get()).text_xs().text_color(tokens::editor_bar_foreground).number_of_lines(1));
    if menu { item = item.child(glyph(IconName::ChevronsUpDown, IconSize::Xs, Tone::Muted)); }
    item
}

/// The symbol at the caret (`permute` slot): the deepest symbol on or above
/// the caret's line.
fn symbol_at_caret() -> Option<scratch_search::Symbol> {
    let line = global::<CursorPosition>().get().line;
    super::quick_open::active_symbols().into_iter()
        .filter(|symbol| symbol.line < line)
        .max_by_key(|symbol| (symbol.line, symbol.level))
}

fn symbol_button() -> impl IntoElement {
    let state = global::<SymbolPopoverState>();
    popover()
        .open(move || Some(state.with(|s| s.open)))
        .on_open_change(move |event: &PopoverEvent| {
            if let PopoverEvent::OpenChange(open, _) = event {
                state.update(|s| {
                    s.open = *open;
                    if !open { s.filter.clear(); }
                });
            }
        })
        .placement(Placement::Top)
        .offset(px(6.0))
        .margin(px(8.0))
        .popup_style(|s| {
            s.w(px(280.0))
                .max_h(px(320.0))
                .p_2()
                .gap_1p5()
                .rounded(px(tokens::radius_lg()))
                .bg(tokens::popover())
                .border_1()
                .border_color(tokens::border())
                .shadow_md()
                .shadow_color(tokens::shadow())
        })
        .popup_starting_style(motion::popup_starting)
        .popup_ending_style(motion::popup_ending)
        .trigger(status_button(
            move || SharedString::from(symbol_at_caret().map(|symbol| symbol.name).unwrap_or_else(|| "No Symbols".to_owned())),
            true,
            move || {
                state.update(|s| {
                    s.open = !s.open;
                    if s.open { s.filter.clear(); }
                });
            },
        ))
        .child(view().flex_col().gap_1p5().w_full()
            .child(search_field(
                "Filter symbols…",
                move || SharedString::from(state.with(|s| s.filter.clone())),
                move || None,
                move |q: &str| state.update(|s| s.filter = q.to_owned()),
                move || state.update(|s| s.filter.clear()),
                SearchFieldOptions::new().size(ControlSize::Sm),
            ))
            .child(scroll().max_h(px(250.0)).w_full()
                .child(dynamic(move || {
                    if !state.with(|s| s.open) {
                        return view().into_element();
                    }
                    let query = state.with(|s| s.filter.to_lowercase());
                    let filtered: Vec<_> = super::quick_open::active_symbols()
                        .into_iter()
                        .filter(|sym| query.is_empty() || sym.name.to_lowercase().contains(&query))
                        .take(120)
                        .collect();
                    if filtered.is_empty() {
                        return view().p_3().items_center().justify_center()
                            .child(text("No matching symbols").text_xs().text_color(tokens::muted_foreground))
                            .into_element();
                    }
                    view().flex_col().gap_0p5().children(filtered.into_iter().map(|sym| {
                        let (line, column) = (sym.line, sym.column);
                        nsh_base::button()
                            .w_full().h(px(tokens::row_sm())).px_2().flex_row().items_center().gap_2()
                            .rounded(px(tokens::radius_sm()))
                            .cursor_pointer()
                            .hover(|s| s.bg(tokens::accent))
                            .on_press(move || {
                                state.update(|s| { s.open = false; s.filter.clear(); });
                                jump_to_position(line, column);
                            })
                            .child(text(sym.kind.label()).text_xs().text_color(tokens::muted_foreground).w(px(58.0)).truncate())
                            .child(text(sym.name).text_xs().text_color(tokens::popover_foreground).flex_1().min_w(px(0.0)).truncate())
                            .child(text(format!("L{}", line + 1)).text_xs().text_color(tokens::muted_foreground))
                    })).into_element()
                }))))
}

/// Moves the active caret to 0-based `line`/`column` (symbol jumps).
fn jump_to_position(line: u32, column: u32) {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    state::record_jump();
    if let Some(api) = state::editor_api(id) {
        let at = EditorPosition::new(line, column);
        api.set_selections(vec![EditorSelection::caret(at)]);
        api.reveal_position(at, true);
        state::focus(id);
    }
}

/// Converts leading indentation of every line in the active document to spaces or tabs.
fn convert_indentation(to_spaces: bool) {
    let Some(id) = with(|workspace| workspace.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    let width = preferences::settings().with_untracked(|s| s.text_editing.indentation.tab_width).max(1) as usize;
    let source = api.value();
    let mut out = String::with_capacity(source.len());
    for (idx, segment) in source.split_inclusive('\n').enumerate() {
        let _ = idx;
        let (line_body, ending) = if let Some(stripped) = segment.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = segment.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (segment, "")
        };
        let mut cols = 0usize;
        let mut split_at = 0usize;
        for (byte_idx, ch) in line_body.char_indices() {
            match ch {
                ' ' => {
                    cols += 1;
                    split_at = byte_idx + 1;
                }
                '\t' => {
                    let rem = cols % width;
                    cols += width - rem;
                    split_at = byte_idx + 1;
                }
                _ => break,
            }
        }
        if to_spaces {
            for _ in 0..cols { out.push(' '); }
        } else {
            for _ in 0..(cols / width) { out.push('\t'); }
            for _ in 0..(cols % width) { out.push(' '); }
        }
        out.push_str(&line_body[split_at..]);
        out.push_str(ending);
    }
    if out != source {
        api.set_value(out);
    }
    preferences::write(move |prefs| prefs.text_editing.indentation.soft_tabs = to_spaces);
}

/// The indentation popup: `Tabs: 4` / `Spaces: 4`.
fn indentation_button() -> impl IntoElement {
    status_button(
        move || SharedString::from({
            let indentation = preferences::settings().with(|settings| settings.text_editing.indentation.clone());
            format!("{}: {}", if indentation.soft_tabs { "Spaces" } else { "Tabs" }, indentation.tab_width)
        }),
        true,
        || { spawn(async { let _ = popup_menu(indentation_menu()).await; }); })
}

/// The indentation menu (DESIGN 8.6.5): Tabs/Spaces, widths, and conversion.
fn indentation_menu() -> Vec<MenuItem> {
    let (soft_tabs, width) = preferences::settings()
        .with_untracked(|settings| (settings.text_editing.indentation.soft_tabs, settings.text_editing.indentation.tab_width));
    let mut items = vec![
        MenuItem::new("Indent Using").disabled(true),
        MenuItem::new("Tabs").checked(!soft_tabs).on_select(|| preferences::write(|prefs| prefs.text_editing.indentation.soft_tabs = false)),
        MenuItem::new("Spaces").checked(soft_tabs).on_select(|| preferences::write(|prefs| prefs.text_editing.indentation.soft_tabs = true)),
        MenuItem::separator(),
        MenuItem::new("Size").disabled(true),
    ];
    for size in [2u32, 3, 4, 6, 8] {
        items.push(MenuItem::new(size.to_string()).checked(width == size)
            .on_select(move || preferences::write(move |prefs| prefs.text_editing.indentation.tab_width = size)));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new("Convert Indentation to Spaces").on_select(|| convert_indentation(true)));
    items.push(MenuItem::new("Convert Indentation to Tabs").on_select(|| convert_indentation(false)));
    items
}

/// The position item: `Line 9, Column 44`; opens Go to Line on press.
fn position_button() -> impl IntoElement {
    status_button(
        move || {
            let position = global::<CursorPosition>().get();
            let use_cols = preferences::settings().with(|p| p.advanced.selection_label_uses_columns);
            if position.selected_chars > 0 {
                if use_cols && let Some(cols) = position.selected_cols {
                    let unit = if cols == 1 { "col" } else { "cols" };
                    SharedString::from(format!("Line {}, Column {} ({cols} {unit})", position.line, position.column))
                } else {
                    SharedString::from(format!("Line {}, Column {} ({} sel)", position.line, position.column, position.selected_chars))
                }
            } else {
                SharedString::from(format!("Line {}, Column {}", position.line, position.column))
            }
        },
        false,
        super::find::go_to_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_char_count_handles_utf16_surrogates() {
        let text = "a😀b\ncd";
        let emoji = EditorSelection::new(EditorPosition::new(0, 1), EditorPosition::new(0, 3));
        assert_eq!(selection_char_count(text, &[emoji]), 1);
        let to_b = EditorSelection::new(EditorPosition::new(0, 1), EditorPosition::new(0, 4));
        assert_eq!(selection_char_count(text, &[to_b]), 2);
        let across_lines = EditorSelection::new(EditorPosition::new(0, 3), EditorPosition::new(1, 2));
        assert_eq!(selection_char_count(text, &[across_lines]), 4);
    }
}
