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

/// Creates the navigator's store once, under the root component's owner, and
/// keeps it in sync with the preferences and the active document.
pub fn install() {
    let store = Store::new(FileTree::new(Arc::new(FsSource), configured_root()));
    store.on_open(|path| {
        spawn(files::open_path(path));
    });
    store.start_watching();
    STORE.with(|s| s.replace(Some(store)));

    // Preferences -> view options.
    effect(move || {
        let (hidden, folders) = preferences::settings()
            .with(|p| (p.general.file_navigator.show_hidden_files, p.general.file_navigator.sort_folders_on_top));
        store.set_options(|o| {
            o.show_hidden = hidden;
            o.folders_first = folders;
        });
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
