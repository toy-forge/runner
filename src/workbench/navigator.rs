//! The left sidebar (DESIGN.md 8.4): the file navigator of `scratch-navigator`
//! (`chrome::header` + `navigator` + `chrome::footer`) and the project Search
//! view ([`super::search_sidebar`]), switched in place by the header's search
//! button. The store is made once by [`install`] under the root's owner, so
//! hiding the sidebar keeps the expanded folders, the history and the filter.
//!
//! Preferences: the root is `ui.file_view_directory` (`~` expands to the home
//! folder, written back when the store's root changes); `general.file_navigator`
//! decides hidden files and folders on top. The row of the active document is
//! marked through `Store::set_current_doc`.
//!
//! Motion inventory (DESIGN.md 9.4, "Sidebar"): the crate's rows handle their
//! own hover, disclosure, drag and context-menu motion; switching between the
//! navigator and the search view is instant (a state change, not an event).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nsh_base::input;
use nsh_core::prelude::*;
use scratch_navigator::model::{FileTree, FsSource};
use scratch_navigator::ui::store::Store;
use scratch_navigator::ui::{chrome, navigator as tree_view};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::files;
use super::preferences;
use super::search_sidebar;
use super::state;

thread_local! {
    /// The navigator's store, created under the root's owner by [`install`].
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// The home folder.
pub(super) fn home() -> PathBuf { std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from) }

/// Expands a stored root (`~`, `~/x`, an absolute path); empty is the home folder.
fn expand_home(raw: &str) -> PathBuf {
    let raw = raw.trim();
    match raw.strip_prefix('~') {
        _ if raw.is_empty() => home(),
        Some(rest) if rest.is_empty() || rest == "/" => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(raw),
    }
}

/// How a root is stored: under the home folder as `~/…`.
fn store_root(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~/".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The root the preferences name.
fn configured_root() -> PathBuf {
    expand_home(&preferences::settings().with_untracked(|p| p.ui.file_view_directory.clone()))
}

/// The navigator's store.
pub(super) fn store() -> Option<Store> { STORE.with(|s| *s.borrow()) }

/// The navigator's current root folder.
pub(super) fn root_path() -> PathBuf {
    store().map_or_else(configured_root, |store| store.root_path())
}

/// The paths selected in the navigator, for the Search view's `Selected Items`.
pub(super) fn selected_paths() -> Vec<PathBuf> {
    store().map_or_else(Vec::new, |store| store.selected_paths())
}

/// Makes `path` the navigator's root; the preferences follow through the
/// history effect in [`install`].
#[allow(dead_code)]
pub(super) fn go_to(path: &Path) {
    if let Some(store) = store() { store.go(path); }
}

/// Reveals `path` in the tree, expanding its ancestors.
pub(super) fn reveal(path: &Path) {
    if let Some(store) = store() { store.reveal(path); }
}

/// Shows the file navigator or the search view.
pub(super) fn show_search(on: bool) {
    state::display().update(|view| {
        view.sidebar_search = on;
        if on { view.sidebar_visible = true; }
    });
}

/// Toggles the search view (the header's search button and View > Search in Files).
pub(super) fn toggle_search() {
    let on = !state::display().with(|view| view.sidebar_search);
    show_search(on);
    if on { search_sidebar::focus_query(); }
}

#[derive(Clone, Default, PartialEq)]
struct GoToFolderSheet {
    open: bool,
    draft: String,
    error: Option<String>,
}

fn open_go_to_folder_sheet() {
    global::<GoToFolderSheet>().set(GoToFolderSheet {
        open: true,
        draft: store_root(&root_path()),
        error: None,
    });
}

fn create_template_file_in(dir: PathBuf) {
    let base_dir = if dir.is_dir() {
        dir
    } else {
        dir.parent().map(Path::to_path_buf).unwrap_or_else(root_path)
    };
    let soft_tabs = preferences::settings().with_untracked(|p| p.text_editing.indentation.soft_tabs);
    let tab_width = preferences::settings().with_untracked(|p| p.text_editing.indentation.tab_width as u8);
    let (def_id, ext, indent) = state::with(|w| {
        let lang = w
            .active
            .and_then(|id| w.doc(id).map(|d| d.document.language.clone()));
        let Some(registry) = w.registry.as_ref() else {
            return (None, "txt".to_owned(), scratch_language::IndentStyle::default());
        };
        let def = lang
            .as_deref()
            .and_then(|id| registry.get(id))
            .unwrap_or_else(|| registry.default_language());
        let ext = def.default_extension().unwrap_or("txt").to_owned();
        let indent = def.effective_indent(scratch_language::IndentStyle { soft_tabs, width: tab_width });
        (Some(def.id.clone()), ext, indent)
    });
    let mut index = 1usize;
    let target = loop {
        let name = if index == 1 {
            format!("untitled.{ext}")
        } else {
            format!("untitled-{index}.{ext}")
        };
        let candidate = base_dir.join(name);
        if !candidate.exists() {
            break candidate;
        }
        index += 1;
    };
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("untitled.{ext}"));
    let content = state::with(|w| {
        let (Some(registry), Some(def_id)) = (w.registry.as_ref(), def_id.as_deref()) else {
            return String::new();
        };
        registry
            .new_document(def_id, &scratch_language::TemplateContext::for_file(&file_name, indent))
            .text
    });
    if let Err(error) = std::fs::write(&target, content) {
        state::notice(format!("Could not create file: {error}"));
        return;
    }
    if let Some(s) = store() {
        s.refresh_dirs(vec![Arc::from(base_dir.as_path())]);
    }
    spawn(files::open_path(target));
}

/// Modal sheet owned by the navigator ("Go to Folder…"), mounted once at root.
pub fn overlay() -> impl IntoElement {
    when(move || global::<GoToFolderSheet>().with(|s| s.open), || {
        let sheet = global::<GoToFolderSheet>();
        let close = || global::<GoToFolderSheet>().update(|s| s.open = false);
        let go = move || {
            let raw = sheet.with_untracked(|s| s.draft.clone());
            let target = expand_home(&raw);
            if target.is_dir() {
                global::<GoToFolderSheet>().update(|s| { s.open = false; s.error = None; });
                go_to(&target);
            } else {
                sheet.update(|s| s.error = Some("Folder does not exist.".to_owned()));
            }
        };
        nsh_core::prelude::overlay()
            .inset_0()
            .child(view().absolute().inset_0().bg(tokens::scrim).on_press(close))
            .child(view().flex_1().items_center().pt(px(48.0)).child(
                view()
                    .w(px(420.0)).p(px(18.0)).gap(px(10.0))
                    .rounded(px(tokens::radius_xl())).bg(tokens::background)
                    .border_1().border_color(tokens::border)
                    .shadow_lg().shadow_color(tokens::shadow())
                    .child(type_subhead().semibold().apply(text("Go to Folder")).text_color(tokens::foreground))
                    .child(input()
                        .value(move || sheet.with(|s| s.draft.clone()))
                        .auto_focus(true)
                        .placeholder("~/path/to/folder")
                        .on_change(move |v: &str| sheet.update(|s| { s.draft = v.to_owned(); s.error = None; }))
                        .on_submit(move |_: &str| go())
                        .placeholder_color(|| color(tokens::muted_foreground()))
                        .caret_color(|| color(tokens::primary()))
                        .h(px(28.0)).px_2().text_sm()
                        .bg(tokens::field).text_color(tokens::foreground)
                        .border_1().border_color(tokens::input).rounded(px(tokens::radius())))
                    .child(when(move || sheet.with(|s| s.error.is_some()), move || {
                        type_caption().apply(text(move || sheet.with(|s| s.error.clone().unwrap_or_default()))).text_color(tokens::destructive)
                    }))
                    .child(view().flex_row().items_center().gap(px(8.0))
                        .child(button("Browse…", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), || {
                            spawn(async move {
                                if let Ok(Some(picked)) = pick_paths(nsh_core::file_dialog::OpenDialog::folders()).await
                                    && let Some(path) = picked.into_iter().next()
                                {
                                    global::<GoToFolderSheet>().update(|s| { s.open = false; s.error = None; });
                                    go_to(&path);
                                }
                            });
                        }))
                        .child(view().flex_1())
                        .child(button("Cancel", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), close))
                        .child(button("Go", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Sm), go))),
            ))
    })
}

/// Creates the navigator's store once, under the root component's owner, and
/// keeps it in sync with the preferences and the active document.
pub fn install() {
    let store = Store::new(FileTree::new(Arc::new(FsSource), configured_root()));
    store.on_open(|path| {
        spawn(files::open_path(path));
    });
    store.on_open_new_window(|path| {
        super::native::spawn_window(&[path]);
    });
    store.on_search_folder(search_sidebar::search_in_folder);
    store.on_search_selection(search_sidebar::search_in_selection);
    store.on_go_to_folder(open_go_to_folder_sheet);
    store.on_new_from_template(create_template_file_in);
    store.start_watching();
    STORE.with(|s| s.replace(Some(store)));

    // Preferences -> view options.
    effect(move || {
        let (hidden, folders) = preferences::settings()
            .with(|p| (p.general.file_navigator.show_hidden_files, p.general.file_navigator.sort_folders_on_top));
        let cur = store.options.get_untracked();
        if cur.show_hidden != hidden || cur.folders_first != folders {
            store.set_options(|o| {
                o.show_hidden = hidden;
                o.folders_first = folders;
            });
        }
    });
    // View options -> preferences.
    effect(move || {
        let opts = store.options.get();
        let (cur_hidden, cur_folders) = preferences::settings().with_untracked(|p| {
            (p.general.file_navigator.show_hidden_files, p.general.file_navigator.sort_folders_on_top)
        });
        if cur_hidden != opts.show_hidden || cur_folders != opts.folders_first {
            preferences::settings().update(|p| {
                p.general.file_navigator.show_hidden_files = opts.show_hidden;
                p.general.file_navigator.sort_folders_on_top = opts.folders_first;
            });
            preferences::save();
        }
    });
    // Preferences -> root (only when the preference changed outside the store).
    effect(move || {
        let root = expand_home(&preferences::settings().with(|p| p.ui.file_view_directory.clone()));
        if store.root_path() != root { store.go(&root); }
    });
    // Store -> preferences (Go to Folder, the path popup, back/forward).
    effect(move || {
        let current = store.history.with(|h| h.current().to_path_buf());
        let configured = configured_root();
        if current != configured {
            preferences::settings().update(|p| p.ui.file_view_directory = store_root(&current));
            preferences::save();
        }
    });
    // The active document's row.
    effect(move || {
        let path = state::display()
            .with(|view| view.active)
            .and_then(|id| state::with(|workspace| workspace.doc(id).and_then(|doc| state::saved_path(&doc.document))));
        store.set_current_doc(path.as_deref());
    });
}

/// The sidebar's contents: file navigator or project search.
pub fn sidebar() -> impl IntoElement {
    let store = store();
    let search = move || state::display().with(|view| view.sidebar_search);
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(tokens::sidebar)
        .text_color(tokens::sidebar_foreground)
        .child(when(move || store.is_some() && !search(), move || {
            let store = store.expect("checked above");
            view().flex_1().min_h(px(0.0)).flex_col()
                .child(chrome::header(store, toggle_search))
                .child(tree_view::navigator(store))
                .child(chrome::footer(store))
                .into_element()
        }))
        .child(when(search, search_sidebar::sidebar))
}

/// The icon of a file, by its extension (DESIGN.md 6.4): QuickNav and search
/// results use it; the tree has its own glyphs.
pub(super) fn file_icon(name: &str) -> IconName {
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tiff" | "bmp" | "ico" | "icns" | "heic" | "svg" => IconName::FileImage,
        "md" | "markdown" | "txt" | "rtf" | "log" | "csv" | "tsv" | "rst" | "tex" => IconName::FileText,
        "zip" | "gz" | "tar" | "tgz" | "7z" | "dmg" | "pdf" | "o" | "a" | "so" | "dylib" | "class" | "jar" | "wasm" | "bin" | "exe" => IconName::FileBinary,
        "sh" | "bash" | "zsh" | "fish" | "py" | "pyw" | "rb" | "pl" | "php" | "lua" | "tcl" | "r" | "js" | "mjs" | "cjs"
        | "jsx" | "ts" | "tsx" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "m" | "mm" | "swift" | "go" | "rs" | "java"
        | "kt" | "kts" | "scala" | "cs" | "fs" | "hs" | "ml" | "erl" | "ex" | "exs" | "clj" | "lisp" | "scm" | "d"
        | "zig" | "dart" | "f" | "f90" | "pas" | "applescript" | "scpt" | "sql" | "html" | "htm" | "css" | "scss"
        | "less" | "xml" | "json" | "yaml" | "yml" | "toml" | "ini" | "plist" | "vue" | "svelte" => IconName::FileCode,
        _ => IconName::File,
    }
}
