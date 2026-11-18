//! The navigator's native context menus (`popup_menu_at`).

use nsh_core::{
    element::desktop::{MenuItem, popup_menu_at},
    prelude::*,
};

use super::store::Store;
use crate::model::{EntryId, Kind, ancestor_chain};

/// Builds the CodeRunner file-navigator context menu (report 01, 8.1, "Context menu").
///
/// `row` is the right-clicked row (None: empty space, acts on the root).
pub fn navigator_menu(store: Store, row: Option<EntryId>) -> Vec<MenuItem> {
    let rows = store.rows_now();
    let picked = row
        .and_then(|id| rows.iter().find(|r| r.entry.id == id))
        .map(|r| r.entry.clone());
    let count = store.selection.with_untracked(|s| s.len());
    let has_sel = picked.is_some();
    let is_dir = picked.as_ref().is_some_and(|e| e.is_dir());
    let multi = count > 1;
    let has_clip = !store.clipboard.get().is_empty();
    let opts = store.options.get_untracked();
    let name = picked.as_ref().map(|e| e.name.to_string());

    let search_label = match (&name, multi) {
        (_, true) => format!("Search {count} Items..."),
        (Some(n), false) => format!("Search \"{n}\"..."),
        _ => "Search...".to_owned(),
    };
    let terminal_label = if has_sel {
        "Open Terminal Window Here"
    } else {
        "Open Terminal Window"
    };
    let path_for_actions = picked.as_ref().map(|e| e.path.to_path_buf());

    let mut items: Vec<MenuItem> = Vec::new();
    items.push(MenuItem::new("Open").disabled(!has_sel).on_select({
        let id = row;
        move || {
            if let Some(id) = id {
                store.activate(id);
            }
        }
    }));
    items.push(
        MenuItem::new("Open in New Window")
            .disabled(!has_sel)
            .on_select({
                let p = path_for_actions.clone();
                move || {
                    if let Some(p) = p.clone() {
                        store.trigger_open_new_window(p);
                    }
                }
            }),
    );
    items.push(
        MenuItem::new("Open with Finder")
            .disabled(!has_sel)
            .on_select({
                let p = path_for_actions.clone();
                move || {
                    if let Some(p) = p.clone() {
                        store.task(async move {
                            let _ = open_path(p).await;
                        });
                    }
                }
            }),
    );
    items.push(MenuItem::new(terminal_label).on_select({
        let p = path_for_actions.clone();
        move || {
            let dir = match &p {
                Some(p) if p.is_dir() => p.clone(),
                Some(p) => p
                    .parent()
                    .map(|x| x.to_path_buf())
                    .unwrap_or_else(|| store.root_path()),
                None => store.root_path(),
            };
            let _ = std::process::Command::new("open")
                .args(["-a", "Terminal"])
                .arg(dir)
                .spawn();
        }
    }));
    items.push(MenuItem::separator());
    items.push(MenuItem::new(search_label).on_select({
        let p = path_for_actions.clone();
        move || {
            if multi {
                store.trigger_search_selection(store.selected_paths());
                return;
            }
            let dir = match &p {
                Some(p) if p.is_dir() => p.clone(),
                Some(p) => p
                    .parent()
                    .map(|x| x.to_path_buf())
                    .unwrap_or_else(|| store.root_path()),
                None => store.root_path(),
            };
            store.trigger_search_folder(dir);
        }
    }));
    items.push(MenuItem::separator());
    items.push(MenuItem::new("New File").on_select(move || store.begin_new(Kind::File)));
    items.push(
        MenuItem::new("New Folder")
            .shortcut("mod+shift+n")
            .on_select(move || store.begin_new(Kind::Dir)),
    );
    items.push(MenuItem::new("Go to Folder...").on_select(move || {
        if !store.trigger_go_to_folder() {
            store.task(async move {
                let picked = pick_paths(nsh_core::file_dialog::OpenDialog::folders()).await;
                if let Ok(Some(v)) = picked
                    && let Some(p) = v.first()
                {
                    store.go(p);
                }
            });
        }
    }));
    items.push(MenuItem::new("Refresh").on_select(move || {
        let dirs = store.tree.with(|t| t.loaded_dirs());
        store.refresh_dirs(dirs);
    }));
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("Copy")
            .disabled(!has_sel)
            .on_select(move || store.copy_selected()),
    );
    items.push(
        MenuItem::new("Paste")
            .disabled(!has_clip)
            .on_select(move || store.paste()),
    );
    items.push(
        MenuItem::new("Rename")
            .disabled(!has_sel || multi)
            .shortcut("enter")
            .on_select({
                let id = row;
                move || {
                    if let Some(id) = id {
                        store.begin_rename(id);
                    }
                }
            }),
    );
    items.push(
        MenuItem::new("Duplicate")
            .disabled(!has_sel)
            .on_select(move || store.duplicate_selected()),
    );
    items.push(
        MenuItem::new("Delete")
            .disabled(!has_sel)
            .shortcut("mod+backspace")
            .on_select(move || store.trash_selected()),
    );
    items.push(MenuItem::separator());
    items.push(MenuItem::submenu(
        "View Options",
        [
            MenuItem::new("Show Hidden Files")
                .checked(opts.show_hidden)
                .on_select(move || {
                    store.set_options(|o| o.show_hidden = !o.show_hidden);
                }),
            MenuItem::new("Sort Folders on Top")
                .checked(opts.folders_first)
                .on_select(move || {
                    store.set_options(|o| o.folders_first = !o.folders_first);
                }),
        ],
    ));
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("Show in Finder")
            .disabled(!has_sel)
            .on_select({
                let p = path_for_actions.clone();
                move || {
                    if let Some(p) = p.clone() {
                        store.task(async move {
                            let _ = reveal_path(p).await;
                        });
                    }
                }
            }),
    );
    items.push(MenuItem::new("Copy Path").disabled(!has_sel).on_select({
        let p = path_for_actions;
        move || {
            if let Some(p) = &p {
                set_clipboard(p.display().to_string());
            }
        }
    }));
    let _ = is_dir;
    items
}

/// Shows the navigator's context menu for `row` at `at` (window coordinates).
pub fn show_navigator_menu(store: Store, row: Option<EntryId>, at: Point<Points>) {
    if let Some(id) = row {
        let already = store.selection.with_untracked(|s| s.contains(id));
        if !already {
            let rows = store.rows_now();
            store
                .selection
                .update(|s| s.click(&rows, id, Default::default()));
        }
    }
    let items = navigator_menu(store, row);
    drop(popup_menu_at(items, at));
}

/// The path popup: the root's ancestor chain, then a separator and recents.
pub fn path_menu(store: Store) -> Vec<MenuItem> {
    let root = store.root_path();
    let mut items: Vec<MenuItem> = ancestor_chain(&root)
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let p = c.path.clone();
            MenuItem::new(c.label)
                .checked(i == 0)
                .on_select(move || store.go(&p))
        })
        .collect();
    items.push(MenuItem::separator());
    items.push(MenuItem::new("Choose Folder...").on_select(move || {
        store.task(async move {
            let picked = pick_paths(nsh_core::file_dialog::OpenDialog::folders()).await;
            if let Ok(Some(v)) = picked
                && let Some(p) = v.first()
            {
                store.go(p);
            }
        });
    }));
    items
}

/// Long-press menu of the back/forward buttons.
pub fn history_menu(store: Store, back: bool) -> Vec<MenuItem> {
    let list: Vec<std::path::PathBuf> = store.history.with_untracked(|h| {
        if back {
            h.back_list().map(|p| p.to_path_buf()).collect()
        } else {
            h.forward_list().map(|p| p.to_path_buf()).collect()
        }
    });
    list.into_iter()
        .map(|p| {
            let label = crate::model::display_name(&p);
            MenuItem::new(label).on_select(move || store.go(&p))
        })
        .collect()
}
