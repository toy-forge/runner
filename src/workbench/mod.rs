//! ScratchRunner's workbench: the window's layout, the documents and their
//! editors, and the wiring of the feature modules.
//!
//! Feature modules can use [`active_document`], [`document_text`],
//! [`editor_for`], [`choose_language`], [`run_document`] and [`stop_document`].
//! All calls belong on the NSH engine thread. The editor reference is valid
//! only while its tab remains open. A run always uses the current buffer.

mod bindings;
mod chrome;
mod compound;
mod console;
mod debugger;
mod diagnostics;
mod docs;
mod edit;
mod execution;
mod find;
mod files;
mod help;
mod lsp;
mod native;
mod navigator;
mod persistence;
mod preferences;
mod quick_open;
mod run_settings;
mod search_sidebar;
mod state;
mod theme;

use std::time::Duration;

use nsh_base::{Editor, editor, resizable_panel, resizable_panel_group, theme};
use nsh_core::desktop::MenuRole;
use nsh_core::element::desktop::{self, MenuItem};
use nsh_core::geometry::{Axis, Points, point};
use nsh_core::prelude::*;
use scratch_config::Paths;
use scratch_document::DocumentId;
use scratch_language::LanguageRegistry;

use self::state::{display, with};

/// The currently selected document, or `None` when no tab is open.
pub fn active_document() -> Option<DocumentId> { with(|workspace| workspace.active) }

/// A copy of the document's current editor buffer, if the tab exists.
pub fn document_text(id: DocumentId) -> Option<String> { state::text(id) }

/// The NSH editor API for an open, mounted tab.
pub fn editor_for(id: DocumentId) -> Option<std::rc::Rc<nsh_base::EditorApi>> { state::editor_api(id) }

/// Chooses a registered language for a document.
pub fn choose_language(id: DocumentId, language: &str) { state::set_language(id, language); }

/// Runs the selected document with its current buffer and language.
pub fn run_document() { execution::run_active(); }

/// Stops the selected document's run, if any.
pub fn stop_document() { execution::stop_active(); }

#[derive(Clone, Default, PartialEq)]
struct EditorFocus(u64);

/// An editor took the focus. Called from `lsp::attach`, which owns the editor's focus handler.
pub(super) fn editor_focused() { global::<EditorFocus>().update(|focus| focus.0 += 1); }

/// Native desktop root, registered by `src/lib.rs` and `nsh.json`.
#[component]
pub fn ScratchRunner() -> impl IntoElement {
    start();
    navigator::install();
    docs::install();
    theme::install();
    install_menu();
    install_keys();
    install_autosave();
    chrome::install();
    // The family list is an AppKit query (main thread only); warm it now
    // so the preferences font popup and the editor's family fallback have
    // it when they need it.
    native::warm_font_cache();
    // Preferences > Themes > Match Appearance to Theme.
    effect(|| {
        let separate = preferences::settings().with(|p| p.themes.use_separate_color_themes);
        if theme::match_appearance() && !separate {
            set_color_scheme(if theme::theme_is_dark() { Appearance::Dark } else { Appearance::Light });
        } else {
            set_color_scheme(Appearance::System);
        }
    });
    // Preferences > General > Show all filename extensions.
    effect(|| {
        let _ = preferences::settings().with(|p| p.general.documents.show_all_extensions);
        state::publish();
    });
    desktop::on_app_quit(|| {
        let snapshot = state::snapshot();
        let waiting = with(|workspace| {
            workspace.runner.cancel_all();
            workspace.journal.as_ref().and_then(|journal| journal.flush(workspace.generation, snapshot).ok())
        });
        async move {
            lsp::shutdown().await;
            if let Some(waiting) = waiting {
                let _ = blocking(move || waiting.recv_timeout(Duration::from_secs(3))).await;
            }
        }
    });
    view().flex_1().min_h(px(0.0)).bg(scratch_ui_kit::tokens::window).text_color(move || theme().foreground)
        .child(chrome::title_band())
        .child(when(move || display().with(|view| view.toolbar_visible), chrome::toolbar))
        .child(when(move || display().with(|view| view.tab_bar_visible), chrome::tab_bar))
        .child(resizable_panel_group()
            .axis(Axis::Horizontal)
            .handle_class("bg-border")
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(resizable_panel()
                .size(move || Some(px(display().with(|view| view.sidebar_width))))
                .size_range(px(scratch_ui_kit::tokens::sidebar_min_w())..px(scratch_ui_kit::tokens::sidebar_max_w()))
                .visible(move || display().with(|view| view.sidebar_visible))
                .on_resize(|size: &Points| {
                    display().update(|view| view.sidebar_width = size.0);
                    with(|workspace| workspace.changed());
                })
                .child(view().flex_1().h_full()
                    .macos_backdrop({
                        let mut backdrop = nsh_core::MacOsBackdrop::default();
                        backdrop.material = nsh_core::MacOsBackdropMaterial::Sidebar;
                        backdrop.blending = nsh_core::MacOsBackdropBlending::BehindWindow;
                        backdrop.state = nsh_core::MacOsBackdropState::FollowsWindowActiveState;
                        backdrop
                    })
                    .bg(scratch_ui_kit::tokens::sidebar)
                    .child(navigator::sidebar())))
            .child(resizable_panel()
                .min_size(px(scratch_ui_kit::tokens::editor_min_w()))
                .child(view().flex_1().min_w(px(0.0)).min_h(px(0.0)).flex_col()
                    .child(workspace_panels())
                    .child(chrome::status_bar())))
            .child(resizable_panel()
                .size(move || Some(px(display().with(|view| {
                    if view.docs_expanded {
                        (view.docs_width * 1.6).clamp(360.0, 600.0)
                    } else {
                        view.docs_width
                    }
                }))))
                .size_range(px(scratch_ui_kit::tokens::docs_min_w())..px(600.0))
                .visible(move || display().with(|view| view.docs_visible))
                .on_resize(|size: &Points| {
                    display().update(|view| {
                        view.docs_width = size.0;
                        view.docs_expanded = false;
                    });
                    with(|workspace| workspace.changed());
                })
                .child(docs::panel())))
        .child(find::overlay())
        .child(edit::overlay())
        .child(diagnostics::overlay())
        .child(quick_open::overlay())
        .child(preferences::overlay())
        .child(run_settings::overlay())
        .child(lsp::overlay())
        .child(chrome::overlay())
        .child(debugger::overlay())
        .child(search_sidebar::overlay())
        .child(navigator::overlay())
        .child(help::overlay())
}

/// The editors and the console. The console sits below the editors, or on
/// their right; one group serves both, so the editors keep their state.
fn workspace_panels() -> impl IntoElement {
    resizable_panel_group()
        .axis(move || if console::console_on_right() { Axis::Horizontal } else { Axis::Vertical })
        .handle_class("bg-border")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(resizable_panel().min_size(px(120.0)).child(view().flex_1().min_h(px(0.0)).min_w(px(0.0)).flex_col()
            .child(find::bar())
            .child(editors())))
        .child(resizable_panel()
            .size(move || Some(px(display().with(|view| if console::console_on_right() { view.console_width } else { view.console_height }))))
            .size_range(px(96.0)..px(900.0))
            .visible(move || display().with(|view| view.console_visible))
            .on_resize(|size: &Points| {
                display().update(|view| if console::console_on_right() { view.console_width = size.0 } else { view.console_height = size.0 });
                with(|workspace| workspace.changed());
            })
            .child(resizable_panel_group()
                .axis(Axis::Horizontal)
                .handle_class("bg-border")
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .child(resizable_panel()
                    .size(move || Some(px(195.0)))
                    .size_range(px(140.0)..px(380.0))
                    .visible(debugger::pane_visible)
                    .child(debugger::stack_pane()))
                .child(resizable_panel()
                    .size(move || Some(px(230.0)))
                    .size_range(px(150.0)..px(460.0))
                    .visible(debugger::pane_visible)
                    .child(debugger::variables_pane()))
                .child(resizable_panel()
                    .min_size(px(180.0))
                    .child(console::panel()))))
}

fn start() {
    // The app draws DESIGN 8.2's title band itself, so the document name and
    // the traffic lights share the top 32 pt. A hidden title bar changes the
    // window, which can re-mount the root; set it once per process.
    thread_local! {
        static TITLE_BAR_SET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if !TITLE_BAR_SET.with(|set| set.replace(true)) {
        let mut attributes = nsh_core::desktop::WindowAttributes::default();
        let mut bar = nsh_core::desktop::TitleBarOptions::default();
        bar.style = nsh_core::desktop::TitleBarStyle::Hidden;
        bar.height = Some(Points(scratch_ui_kit::tokens::titlebar_h()));
        attributes.title_bar = Some(bar);
        let win = desktop::current_window();
        win.set(attributes);
        win.on_focus_change(|focused| {
            display().update(|view| view.window_focused = focused);
        });
        win.on_bounds_change(|bounds| {
            global::<state::WindowBoundsSignal>().set(state::WindowBoundsSignal(Some(persistence::SavedBounds {
                x: bounds.origin.x.0,
                y: bounds.origin.y.0,
                width: bounds.size.width.0,
                height: bounds.size.height.0,
            })));
            with(|workspace| workspace.changed());
        });
        native::enable_webview_developer_extras();
        let _ = scratch_runner::warm_up();
    }
    let mut new_window = false;
    let mut open_args: Vec<std::path::PathBuf> = Vec::new();
    let mut handoff_args: Vec<std::path::PathBuf> = Vec::new();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--new-window" => new_window = true,
            "--open" => {
                if let Some(p) = iter.next() {
                    open_args.push(std::path::PathBuf::from(p));
                }
            }
            "--handoff" => {
                if let Some(p) = iter.next() {
                    handoff_args.push(std::path::PathBuf::from(p));
                }
            }
            _ => {}
        }
    }
    let paths = std::env::var_os("SCRATCHRUNNER_DATA_DIR").map(Paths::in_dir).or_else(Paths::system);
    let Some(paths) = paths else {
        display().update(|view| { view.loading = false; view.notice = "No application data directory; recovery is unavailable.".into(); });
        return;
    };
    preferences::load(&paths);
    spawn(async move {
        let loaded = blocking({ let paths = paths.clone(); move || {
            let mut registry = LanguageRegistry::builtin();
            let language_error = if paths.languages().exists() {
                match registry.load_user_dir(paths.languages()) {
                    Ok(report) if !report.errors.is_empty() => Some(format!("{} language definitions could not be loaded.", report.errors.len())),
                    Err(error) => Some(format!("Could not load languages: {error}")),
                    _ => None,
                }
            } else { None };
            let (snapshot, warning) = if new_window {
                (persistence::Snapshot::default(), None)
            } else {
                persistence::restore(&paths)
            };
            let mut handoffs = Vec::new();
            for handoff in &handoff_args {
                if let Ok(bytes) = std::fs::read(handoff) {
                    if let Ok(doc) = serde_json::from_slice::<persistence::SavedDocument>(&bytes) {
                        handoffs.push(doc);
                    }
                    let _ = std::fs::remove_file(handoff);
                }
            }
            (registry, snapshot, warning.or(language_error), handoffs)
        }}).await;
        let Ok((registry, snapshot, warning, handoffs)) = loaded else {
            display().update(|view| { view.loading = false; view.notice = "Recovery could not start.".into(); });
            return;
        };
        with(|workspace| {
            workspace.registry = Some(registry);
            workspace.journal = if new_window && handoffs.is_empty() { None } else { Some(persistence::Journal::new(&paths)) };
            workspace.paths = Some(paths);
        });
        global::<state::RegistryRevision>().update(|rev| rev.0 += 1);
        let has_session_dir = !new_window && with(|workspace| workspace.paths.as_ref().is_some_and(|paths| {
            let dir = paths.root().join("WorkbenchSessions");
            dir.exists() && std::fs::read_dir(dir).ok().is_some_and(|mut entries| entries.next().is_some())
        }));
        let tab_bar_hidden = preferences::settings().with_untracked(|p| p.general.tabs.tab_bar_hidden);
        display().update(|view| {
            view.loading = false;
            view.console_visible = snapshot.console_visible;
            view.console_height = snapshot.console_height.clamp(96.0, 600.0);
            view.console_right = snapshot.console_right;
            view.console_width = snapshot.console_width.clamp(240.0, 900.0);
            view.sidebar_visible = snapshot.sidebar_visible;
            view.sidebar_width = snapshot.sidebar_width.clamp(180.0, 400.0);
            view.docs_visible = snapshot.docs_visible;
            view.docs_width = snapshot.docs_width.clamp(240.0, 600.0);
            view.toolbar_visible = snapshot.toolbar_visible;
            view.tab_bar_visible = if !has_session_dir && !new_window { !tab_bar_hidden } else { snapshot.tab_bar_visible };
            if let Some(warning) = warning { view.notice = warning; }
        });
        if !new_window {
            if let Some(bounds) = snapshot.window_bounds {
                if bounds.width >= 500.0 && bounds.height >= 320.0 {
                    global::<state::WindowBoundsSignal>().set(state::WindowBoundsSignal(Some(bounds)));
                    desktop::current_window().set_bounds(nsh_core::desktop::WindowFrame {
                        x: Some(px(bounds.x)),
                        y: Some(px(bounds.y)),
                        width: Some(px(bounds.width)),
                        height: Some(px(bounds.height)),
                        animated: Some(false),
                    });
                }
            }
        }
        let selected = snapshot.active;
        for document in snapshot.documents { state::add(document); }
        let keep_snapshot_active = handoffs.is_empty() && open_args.is_empty();
        for document in handoffs { state::add(document); }
        for path in open_args { files::open_path(path).await; }
        let found = if keep_snapshot_active {
            selected.filter(|id| with(|workspace| workspace.doc(*id).is_some()))
        } else {
            None
        };
        if let Some(id) = found { state::activate(id); }
        else if with(|workspace| workspace.documents.is_empty()) {
            if !has_session_dir && !new_window { state::new_first_launch_document(); } else { state::new_document(); }
        }
        else { state::publish(); }
    });
}

fn install_autosave() {
    set_interval(Duration::from_millis(800), || {
        // Fonts picked in the system font panel (queued on the main
        // thread by the font manager's `changeFont:` target).
        for (family, size) in native::take_font_picks() {
            preferences::settings().update(|p| {
                let font = &mut p.text_editing.appearance.font;
                font.family = family.clone();
                font.size = size;
            });
            preferences::save();
        }
        let results = with(|workspace| {
            let mut results = Vec::new();
            if let Some(journal) = &workspace.journal { while let Ok(result) = journal.results.try_recv() { results.push(result); } }
            results
        });
        for (generation, result) in results {
            if let Err(error) = result {
                state::notice(format!("Recovery save failed: {error}. Retrying."));
                with(|workspace| { if workspace.queued == generation { workspace.queued = 0; } });
            }
        }
        let generation = with(|workspace| (workspace.journal.is_some() && workspace.generation != workspace.queued).then_some(workspace.generation));
        if let Some(generation) = generation {
            let snapshot = state::snapshot();
            let result = with(|workspace| {
                let result = workspace.journal.as_ref().expect("journal checked above").write(generation, snapshot);
                if result.is_ok() { workspace.queued = generation; }
                result
            });
            if let Err(error) = result { state::notice(format!("Recovery save failed: {error}")); }
        }
    });
}

fn install_menu() {
    fn item(title: &str, id: &str, action: impl Fn() + 'static) -> MenuItem {
        let keys = bindings::keys_for(id);
        let item = MenuItem::new(title);
        if keys.is_empty() { item.on_select(action) } else { item.shortcut(&keys).on_select(action) }
    }
    // Rebuilt when the key bindings, the recents list, the language
    // registry, the document set, or the preferences change: menu items show
    // the recorded keys, Open Recent lists the latest paths, the language and
    // template submenus enumerate the registry, and the Diagnostics menu
    // reflects the active document's language and diagnostics state.
    effect(|| {
        global::<bindings::BindingsRevision>().get();
        global::<state::RecentsRevision>().get();
        global::<state::RegistryRevision>().get();
        global::<state::WorkspaceRevision>().get();
        preferences::settings().get();
        native::disallow_window_tabbing();
        native::set_min_window_size(500.0, 320.0);
        desktop::set_application_menu(Some(vec![
            MenuItem::submenu("ScratchRunner", [
                MenuItem::new("About ScratchRunner").on_select(help::open_about),
                MenuItem::separator(),
                item("Preferences…", "app.preferences", preferences::open),
                // No updater ships with ScratchRunner: the item stays visible
                // and disabled, as in CodeRunner with updates off.
                MenuItem::new("Check for Updates…").disabled(true),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Hide), MenuItem::role(MenuRole::HideOthers), MenuItem::role(MenuRole::ShowAll),
                MenuItem::separator(), MenuItem::role(MenuRole::Quit),
            ]),
            MenuItem::submenu("File", [
                item("New Tab", "file.newTab", state::new_document),
                item("New Window", "file.newWindow", files::new_window),
                item("New Document from Language…", "sr.newFromLanguage", quick_open::new_document_palette),
                new_from_template_menu(),
                item("New from Clipboard", "file.newFromClipboard", files::new_from_clipboard),
                item("Open…", "file.open", files::open_dialog),
                open_recent_menu(),
                item("Open Quickly…", "file.openQuickly", quick_open::open),
                item("Go to Current Document", "file.goToCurrentDocument", files::go_to_current_document),
                MenuItem::separator(), item("Close Tab", "file.close", files::close_active),
                item("Close Window", "file.closeWindow", native::close_window),
                item("Close Other Tabs", "file.closeOtherTabs", files::close_other_tabs),
                // CodeRunner names this item after what it will close.
                item(if display().with(|view| view.tabs.len() > 1) { "Close Tab Without Saving" } else { "Close Document Without Saving" },
                    "file.closeWithoutSaving", files::close_active_without_saving),
                MenuItem::separator(), item("Save", "file.save", || files::save_active(false)),
                item("Save As…", "file.saveAs", || files::save_active(true)),
                item("Duplicate", "file.duplicate", files::duplicate_active),
                item("Rename…", "file.rename", files::rename_active),
                item("Move To…", "file.moveTo", files::move_active),
                item("Revert to Saved", "file.revertToSaved", files::revert_active),
                MenuItem::separator(),
                MenuItem::submenu("Share", [
                    MenuItem::new("Mail").on_select(|| files::share_active("mail")),
                    MenuItem::new("Messages").on_select(|| files::share_active("message")),
                    MenuItem::new("AirDrop").on_select(|| files::share_active("airdrop")),
                ]),
                MenuItem::separator(),
                item("Page Setup…", "file.pageSetup", native::page_setup),
                item("Print…", "file.print", files::print_active),
            ]),
            MenuItem::submenu("Edit", [
                MenuItem::role(MenuRole::Undo), MenuItem::role(MenuRole::Redo), MenuItem::separator(),
                MenuItem::role(MenuRole::Cut), MenuItem::role(MenuRole::Copy),
                MenuItem::new("Copy with Colors").on_select(edit::copy_with_colors),
                MenuItem::role(MenuRole::Paste),
                MenuItem::new("Paste and Preserve Formatting").on_select(|| edit::run(nsh_base::EditorCommand::Paste)),
                item("Delete", "edit.deleteForward", || edit::run(nsh_base::EditorCommand::DeleteForward)),
                MenuItem::role(MenuRole::SelectAll),
                MenuItem::separator(),
                item("Kill to Line End", "edit.killToLineEnd", edit::kill_to_line_end),
                item("Yank", "edit.yank", edit::yank),
                MenuItem::separator(),
                // AppKit appends AutoFill / Start Dictation / Emoji & Symbols
                // itself; declaring them here would duplicate the tail.
                MenuItem::submenu("Spelling and Grammar", [
                    MenuItem::new("Show Spelling and Grammar").on_select(native::show_spelling_panel),
                    MenuItem::new("Check Document Now").on_select(native::check_spelling_now),
                    MenuItem::separator(),
                    MenuItem::new("Check Spelling While Typing").on_select(native::toggle_spell_checking),
                    MenuItem::new("Check Grammar With Spelling").on_select(native::toggle_grammar_checking),
                    MenuItem::new("Correct Spelling Automatically").on_select(native::toggle_auto_correction),
                ]),
                MenuItem::submenu("Transformations", [
                    MenuItem::new("Make Upper Case").on_select(|| edit::run(nsh_base::EditorCommand::TransformToUppercase)),
                    MenuItem::new("Make Lower Case").on_select(|| edit::run(nsh_base::EditorCommand::TransformToLowercase)),
                    MenuItem::new("Capitalize").on_select(|| edit::run(nsh_base::EditorCommand::TransformToTitleCase)),
                ]),
                MenuItem::submenu("Speech", [
                    MenuItem::new("Start Speaking").on_select(edit::start_speaking),
                    MenuItem::new("Stop Speaking").disabled(!native::is_speaking()).on_select(native::stop_speaking),
                ]),
            ]),
            MenuItem::submenu("Find", find::menu_items()),
            MenuItem::submenu("Text", edit::menu_items()),
            MenuItem::submenu("Run", {
                let (any_running, compiles) = with(|workspace| {
                    (workspace.documents.iter().any(|doc| doc.running.get()),
                     workspace.active.and_then(|id| workspace.doc(id))
                        .and_then(|doc| workspace.registry.as_ref().and_then(|r| r.get(&doc.document.language)))
                        .is_some_and(|def| def.has_compile_step()))
                });
                let can_run = chrome::can_run();
                // CodeRunner titles the diagnostics item "Compile" for
                // languages with a compile script (⌘B then means compile).
                let diagnostics_title = if compiles { "Compile" } else { "Run Diagnostics" };
                vec![
                item("Run", "run.run", execution::run_active).disabled(any_running || !can_run),
                item("Run and Debug", "run.runAndDebug", debugger::run_in_debugger).disabled(any_running || !can_run),
                item("Run Without Debugging", "run.runWithoutDebugging", execution::run_without_debugging).disabled(any_running || !can_run),
                item("Run Selection", "run.runSelection", execution::run_selection).disabled(any_running || !can_run),
                item(diagnostics_title, "run.runDiagnostics", execution::compile_or_diagnostics),
                item("Stop", "run.stop", execution::stop_active).disabled(!any_running),
                MenuItem::separator(),
                MenuItem::submenu("Debugger", [
                    item("Toggle Breakpoint at Current Line", "debug.toggleBreakpoint", debugger::toggle_breakpoint_at_caret),
                    item("Clear Breakpoints", "debug.clearBreakpoints", debugger::clear_breakpoints),
                    MenuItem::separator(),
                    item("Pause Execution", "debug.pause", debugger::pause_or_continue),
                    item("Step Over", "debug.stepOver", debugger::step_over),
                    item("Step In", "debug.stepIn", debugger::step_in),
                    item("Step Out", "debug.stepOut", debugger::step_out),
                    item("Stop Debugging", "sr.stopDebugging", debugger::stop),
                ]),
                MenuItem::separator(), item("Edit Run Settings…", "run.editRunSettings", run_settings::open),
                item("Clear Console", "run.clearConsole", execution::clear_active),
                MenuItem::separator(),
                MenuItem::submenu("Language", edit::language_items()),
                item("Select Language…", "run.selectLanguage", quick_open::select_language),
                item("Edit Languages…", "run.editLanguages", || preferences::open_pane(preferences::Category::Languages)),
                ]
            }),
            MenuItem::submenu("View", {
                let prefs = preferences::settings().get_untracked();
                let (sidebar_visible, console_visible, console_right, docs_visible, toolbar_visible, tab_bar_visible) = state::display()
                    .with_untracked(|view| (
                        view.sidebar_visible,
                        view.console_visible,
                        view.console_right,
                        view.docs_visible,
                        view.toolbar_visible,
                        view.tab_bar_visible,
                    ));
                let keyed = |title: &str, id: &str, action: fn()| {
                    let item = MenuItem::new(title).on_select(action);
                    let keys = bindings::keys_for(id);
                    if keys.is_empty() { item } else { item.shortcut(&keys) }
                };
                vec![
                    keyed(if tab_bar_visible { "Hide Tab Bar" } else { "Show Tab Bar" }, "view.toggleTabBar", chrome::toggle_tab_bar),
                    keyed(if sidebar_visible { "Hide File Navigator" } else { "Show File Navigator" }, "view.toggleFileNavigator", chrome::toggle_navigator),
                    keyed(if console_visible { "Hide Console" } else { "Show Console" }, "view.toggleConsole", chrome::toggle_console),
                    keyed(if docs_visible { "Hide Documentation View" } else { "Show Documentation View" }, "view.toggleDocumentation", chrome::toggle_docs),
                    MenuItem::separator(),
                    MenuItem::new("Automatically Hide Console")
                        .checked(prefs.general.console.hide_automatically)
                        .on_select(chrome::toggle_console_auto_hide),
                    MenuItem::submenu("Console Position", [
                        MenuItem::new("Bottom").checked(!console_right).on_select(|| chrome::set_console_position(false)),
                        MenuItem::new("Right").checked(console_right).on_select(|| chrome::set_console_position(true)),
                    ]),
                    MenuItem::separator(),
                    keyed(if toolbar_visible { "Hide Toolbar" } else { "Show Toolbar" }, "view.toggleToolbar", chrome::toggle_toolbar),
                    // The system appends Enter Full Screen ⌃⌘F itself —
                    // a custom item can't carry the OS-reserved keystroke.
                ]
            }),
            MenuItem::submenu("Diagnostics", diagnostics::menu_items()),
            {
                // CodeRunner's Window menu: system window actions first, then
                // history and tab navigation, then the open documents (the
                // Tab 1-9 shortcuts stay bound even without menu items).
                let mut window_items = vec![
                    // The Minimize/Zoom roles mark this submenu as the app's
                    // windows menu — the system then injects Minimize All /
                    // Zoom All alternates plus Fill, Center, Move & Resize,
                    // Full Screen Tile and Remove Window from Set (and ⌘M
                    // comes with the Minimize role, a keystroke custom
                    // bindings can't carry).
                    MenuItem::role(MenuRole::Minimize),
                    MenuItem::role(MenuRole::Zoom),
                    MenuItem::separator(),
                    item("Go Forward", "window.goForward", || state::nav_step(false)),
                    item("Go Back", "window.goBack", || state::nav_step(true)),
                    MenuItem::separator(),
                    item("Select Previous Tab", "window.selectPreviousTab", || state::step_tab(-1)),
                    item("Select Next Tab", "window.selectNextTab", || state::step_tab(1)),
                    item("Move Tab Left", "window.moveTabLeft", || state::move_active_tab(-1)),
                    item("Move Tab Right", "window.moveTabRight", || state::move_active_tab(1)),
                    MenuItem::new("Move Tab to New Window").disabled(display().with_untracked(|view| view.tabs.len() < 2)).on_select(files::move_active_tab_to_new_window),
                    MenuItem::new("Merge All Windows").on_select(native::merge_all_windows),
                    MenuItem::separator(),
                    MenuItem::new("Bring All to Front").on_select(native::bring_all_to_front),
                    MenuItem::new("Arrange in Front").on_select(native::arrange_in_front),
                    MenuItem::separator(),
                ];
                window_items.extend(with(|workspace| workspace.documents.iter().map(|doc| {
                    let id = doc.document.id;
                    let title = doc.title.clone();
                    MenuItem::new(title).checked(workspace.active == Some(id)).on_select(move || state::activate(id))
                }).collect::<Vec<_>>()));
                MenuItem::submenu("Window", window_items)
            },
            MenuItem::submenu("Help", [
                item("ScratchRunner Help", "help.help", help::open_help),
                MenuItem::new("CodeRunner Documentation").on_select(|| open_site("https://coderunnerapp.com/docs")),
                MenuItem::separator(),
                MenuItem::new("Visit CodeRunner Website").on_select(|| open_site("https://coderunnerapp.com")),
                MenuItem::new("Visit CodeRunner Support Page").on_select(|| open_site("https://coderunnerapp.com/support")),
                MenuItem::separator(),
                MenuItem::new("Software Attributions…").on_select(attributions),
            ]),
        ]));
    });
}

/// Opens `url` in the browser (the Help menu's links).
fn open_site(url: &'static str) {
    spawn(async move { let _ = open_url(url).await; });
}

/// Help > Software Attributions…: the third-party software notice.
fn attributions() {
    spawn(async {
        let _ = nsh_base::alert_dialog()
            .title("Software Attributions")
            .description(
                "ScratchRunner is built on Native Shell (nsh) and open-source \
                 crates: syntect (syntax highlighting), encoding_rs, serde, \
                 tokio, tracing, uuid, and the Rust ecosystem.\n\n\
                 Language definitions, key bindings, and feature behavior are \
                 modeled after CodeRunner by Nikolai Krill.",
            )
            .action("ok", "OK")
            .show()
            .await;
    });
}

/// File > New from Template: every template group of every registered
/// language ("Language — Template"); picking one creates its files as new
/// documents. The list is fixed at menu-build time.
fn new_from_template_menu() -> MenuItem {
    let entries = with(|workspace| {
        workspace
            .registry
            .as_ref()
            .map(|registry| {
                let mut all: Vec<(String, String, String)> = Vec::new();
                for language in registry.all() {
                    for group in registry.templates_for(&language.id) {
                        let name = if group.name.is_empty() { language.name.clone() } else { group.name.to_owned() };
                        all.push((language.id.clone(), group.name.to_owned(), format!("{} — {}", language.name, name)));
                    }
                }
                all.sort_by(|a, b| a.2.cmp(&b.2));
                all
            })
            .unwrap_or_default()
    });
    MenuItem::submenu(
        "New from Template",
        entries
            .iter()
            .map(|(language, template, title)| {
                let (language, template) = (language.clone(), template.clone());
                MenuItem::new(title.clone())
                    .on_select(move || state::new_document_from_template(&language, &template))
            })
            .collect::<Vec<_>>(),
    )
}

/// File > Open Recent: the last-opened paths, most recent first.
fn open_recent_menu() -> MenuItem {
    let paths = state::recents();
    let mut items: Vec<MenuItem> = paths.iter().map(|path| {
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let path = path.clone();
        MenuItem::new(title).on_select(move || {
            let path = path.clone();
            spawn(async move { files::open_path(path).await; });
        })
    }).collect();
    if !items.is_empty() { items.push(MenuItem::separator()); }
    items.push(MenuItem::new("Clear Menu").disabled(paths.is_empty())
        .on_select(state::clear_recents));
    MenuItem::submenu("Open Recent", items)
}

fn install_keys() {
    diagnostics::install();
    find::install();
    edit::install();
    lsp::install();
    // Re-registered when the key bindings change: the pane's records apply at once.
    effect(|| {
        global::<bindings::BindingsRevision>().get();
        bindings::on("file.newWindow", files::new_window);
        bindings::on("file.newTab", state::new_document);
        bindings::on("file.open", files::open_dialog);
        bindings::on("file.openQuickly", quick_open::open);
        bindings::on("window.goBack", || state::nav_step(true));
        bindings::on("window.goForward", || state::nav_step(false));
        bindings::on("view.toggleDocumentation", chrome::toggle_docs);
        // view.enterFullScreen is system-provided (no bindable keystroke).
        bindings::on("app.preferences", preferences::open);
        bindings::on("run.editRunSettings", run_settings::open);
        bindings::on("file.save", || files::save_active(false));
        bindings::on("file.saveAs", || files::save_active(true));
        bindings::on("file.print", files::print_active);
        bindings::on("file.close", files::close_active);
        bindings::on("run.run", execution::run_active);
        bindings::on("run.stop", execution::stop_active);
        bindings::on("run.clearConsole", execution::clear_active);
        bindings::on("view.consolePosition.bottom", || chrome::toggle_console_at(false));
        bindings::on("view.consolePosition.right", || chrome::toggle_console_at(true));
        bindings::on("view.toggleFileNavigator", chrome::toggle_navigator);
        bindings::on("find.searchInFileSystem", navigator::toggle_search);
        bindings::on("run.editLanguages", || preferences::open_pane(preferences::Category::Languages));
        bindings::on("sr.newFromLanguage", quick_open::new_document_palette);
        bindings::on("run.runAndDebug", debugger::run_in_debugger);
        bindings::on("run.runWithoutDebugging", execution::run_without_debugging);
        bindings::on("run.runSelection", execution::run_selection);
        bindings::on("sr.stopDebugging", debugger::stop);
        bindings::on("debug.toggleBreakpoint", debugger::toggle_breakpoint_at_caret);
        bindings::on("debug.clearBreakpoints", debugger::clear_breakpoints);
        bindings::on("debug.pause", debugger::pause_or_continue);
        bindings::on("debug.stepOver", debugger::step_over);
        bindings::on("debug.stepIn", debugger::step_in);
        bindings::on("debug.stepOut", debugger::step_out);
        bindings::on("view.toggleConsole", chrome::toggle_console);
        bindings::on("view.toggleToolbar", chrome::toggle_toolbar);
        bindings::on("view.toggleTabBar", chrome::toggle_tab_bar);
        bindings::on("file.closeWindow", native::close_window);
        bindings::on("file.closeOtherTabs", files::close_other_tabs);
        bindings::on("file.closeWithoutSaving", files::close_active_without_saving);
        bindings::on("file.newFromClipboard", files::new_from_clipboard);
        bindings::on("file.goToCurrentDocument", files::go_to_current_document);
        bindings::on("text.splitSelectionByWords", edit::split_selection_by_words);
        bindings::on("text.insertSelectionNumber", edit::insert_selection_number);
        bindings::on("text.showColorScopes", edit::show_color_scopes);
        bindings::on("text.surround.singleQuotes", || edit::surround("'", "'"));
        bindings::on("text.surround.doubleQuotes", || edit::surround("\"", "\""));
        bindings::on("text.surround.parentheses", || edit::surround("(", ")"));
        bindings::on("text.surround.braces", || edit::surround("{", "}"));
        bindings::on("text.surround.brackets", || edit::surround("[", "]"));
        bindings::on("file.duplicate", files::duplicate_active);
        bindings::on("file.rename", files::rename_active);
        bindings::on("file.moveTo", files::move_active);
        bindings::on("file.pageSetup", native::page_setup);
        bindings::on("file.revertToSaved", files::revert_active);
        bindings::on("run.selectLanguage", quick_open::select_language);
        bindings::on("window.selectPreviousTab", || state::step_tab(-1));
        bindings::on("window.selectNextTab", || state::step_tab(1));
        bindings::on("window.moveTabLeft", || state::move_active_tab(-1));
        bindings::on("window.moveTabRight", || state::move_active_tab(1));
        bindings::on("help.help", help::open_help);
        for n in 1..=10 {
            bindings::on(&format!("window.selectTab{n}"), move || {
                let id = display().with_untracked(|view| view.tabs.get(n - 1).map(|tab| tab.id));
                if let Some(id) = id { state::activate(id); }
            });
        }
    });
    // Compound-command triggers follow bindings, editor focus, read-only locks,
    // and find-field focus independently of menu-shortcut registration.
    effect(|| {
        global::<bindings::BindingsRevision>().get();
        let _ = global::<EditorFocus>().get();
        compound::refresh();
    });
    // The editor registers Cmd-. on focus. Register after each focus change so
    // the app's Stop command wins the editor's code-action binding.
    effect(|| {
        global::<bindings::BindingsRevision>().get();
        if global::<EditorFocus>().get().0 > 0 { bindings::on("run.stop", execution::stop_active); }
    });
}

fn editors() -> impl IntoElement {
    view().flex_1().min_h(px(0.0)).bg(scratch_ui_kit::tokens::editor_bg)
        .child(when(move || display().with(|view| view.loading), || text("Restoring documents…").p_4().text_color(scratch_ui_kit::tokens::muted_foreground)))
        .child(when(move || display().with(|view| !view.loading && view.tabs.is_empty()), || {
            view().flex_1().items_center().justify_center().gap_2()
                .child(text("No Open Document").text_base().font_weight(FontWeight::MEDIUM).text_color(scratch_ui_kit::tokens::muted_foreground))
                .child(text("Create a new tab (⌘T) or open a file (⌘O).").text_xs().text_color(scratch_ui_kit::tokens::muted_foreground))
                .child(view().flex_row().gap_2().pt_2()
                    .child(scratch_ui_kit::primitives::button("New Tab", scratch_ui_kit::primitives::ButtonOptions::new().size(scratch_ui_kit::primitives::ControlSize::Sm), state::new_document))
                    .child(scratch_ui_kit::primitives::button("Open…", scratch_ui_kit::primitives::ButtonOptions::new().size(scratch_ui_kit::primitives::ControlSize::Sm), files::open_dialog)))
        }))
        .child(each(move || display().with(|view| view.tabs.clone()), |tab: &state::Tab| tab.id, |row, _| {
            let tab = row.get_untracked();
            let id = tab.id;
            let source = with(|workspace| workspace.doc(id).map(|doc| doc.text.clone()).unwrap_or_default());
            let handle = component_ref::<Editor>();
            with(|workspace| { if let Some(doc) = workspace.doc_mut(id) { doc.editor = Some(handle); } });
            let settings = preferences::settings();
            view().flex_1().min_h(px(0.0)).style_when(move || display().with(|view| view.active != Some(id)), |s| s.hidden())
                .child(lsp::attach(editor().default_value(source)
                    .language(move || Some(SharedString::from(row.with(|tab| tab.syntax.clone()))))
                    .accessibility_label(format!("Editor for {}", tab.title))
                    .line_numbers(move || settings.with(|settings| settings.text_editing.appearance.line_numbers.enabled))
                    .soft_wrap(move || settings.with(|settings| settings.text_editing.appearance.wrap_lines))
                    .tab_size(move || Some(settings.with(|settings| settings.text_editing.indentation.tab_width.max(1))))
                    .hard_tabs(move || Some(!settings.with(|settings| settings.text_editing.indentation.soft_tabs)))
                    .auto_close(move || settings.with(|settings| settings.text_editing.behavior.character_pairs.close_enabled))
                    .auto_indent(move || settings.with(|settings| settings.text_editing.indentation.auto_indent_while_typing))
                    .folding(true)
                    .indent_guides(true)
                    .font_size(move || px(settings.with(|settings| settings.text_editing.appearance.font.size) as f32))
                    .font_family(move || Some(SharedString::from(settings.with(|settings| {
                        // A family that is not installed (a stale import)
                        // would render as the engine's fallback face.
                        native::font_family_or_default(&settings.text_editing.appearance.font.family, scratch_config::defaults::FONT_FAMILY)
                    }))))
                    .line_height(move || px(settings.with(|settings| settings.text_editing.appearance.line_height_points()) as f32))
                    .show_whitespaces(move || settings.with(|settings| settings.text_editing.appearance.invisibles.show))
                    .active_line(move || settings.with(|settings| settings.text_editing.appearance.highlight_current_line))
                    .rulers(move || {
                        let guide = settings.with(|settings| settings.text_editing.appearance.page_guide.clone());
                        if guide.show { vec![guide.column] } else { vec![] }
                    })
                    .scroll_beyond_last_line(move || {
                        nsh_base::EditorScrollBeyond::from(
                            settings.with(|settings| settings.text_editing.appearance.allow_scroll_past_end),
                        )
                    })
                    .highlighting(false)
                    .read_only(move || compound::locked(id))
                    .decorations(move || theme::decorations(id))
                    .gutter_decorations_width(px(5.0))
                    .on_line_number_press(move |line: &u32| state::toggle_breakpoint(id, *line))
                    .on_context_menu(move |e: &nsh_base::EditorContextMenuEvent| {
                        drop(desktop::popup_menu_at(edit::context_menu_for(id, e.position.line), point(e.x, e.y)));
                    })
                    .search(find::editor_search)
                    .on_find_with_selection(move |query: &String| find::use_selection_text(query))
                    .on_search_change(move |result: &nsh_base::EditorSearchResult| find::search_changed(id, result))
                    .on_selection_change(move |selections: &Vec<nsh_base::EditorSelection>| chrome::selection_changed(id, selections))
                    .component_ref(&handle).flex_1().min_h(px(0.0))
                    .on_change(move |text: &SharedString| state::edited(id, text))
                    , id))
        }))
}
