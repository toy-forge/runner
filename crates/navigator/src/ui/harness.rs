//! A tiny root that mounts the navigator beside text readouts, for headless tests
//! (`tests/navigator.rs`) and as a usage example.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use nsh_core::prelude::*;

use super::{chrome, navigator, store::Store};
use crate::model::{DirSource, FileTree};

/// What the harness mounts (set before starting the engine; tests serialize on it).
pub static CONFIG: Mutex<Option<(Arc<dyn DirSource>, PathBuf)>> = Mutex::new(None);

/// The harness: `navigator` in a 300 x 500 box and text readouts
/// (`rows:`, `sel:`, `cursor:`, `filter:`, `status:`, `opened:`, `editing:`).
#[component]
pub fn Harness() -> impl IntoElement {
    let (source, root) = CONFIG
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| (Arc::new(crate::model::FsSource), PathBuf::from("/")));
    let store = Store::new(FileTree::new(source, root));
    let opened = signal(String::new());
    store.on_open(move |p| opened.set(p.display().to_string()));
    store.start_watching();
    let cursor = move || {
        let c = store.selection.with(|s| s.cursor);
        store.rows.with(|r| {
            c.and_then(|c| {
                r.0.iter()
                    .find(|r| r.entry.id == c)
                    .map(|r| r.entry.name.to_string())
            })
            .unwrap_or_default()
        })
    };
    view()
        .flex_1()
        .flex_row()
        .child(
            view()
                .w(px(300.0))
                .h(px(500.0))
                .child(chrome::header(store, || {}))
                .child(navigator::navigator(store))
                .child(chrome::footer(store)),
        )
        .child(
            view()
                .child(text(move || {
                    format!("rows:{}", store.rows.with(|r| r.0.len()))
                }))
                .child(text(move || {
                    format!("sel:{}", store.selection.with(|s| s.len()))
                }))
                .child(text(move || format!("cursor:{}", cursor())))
                .child(text(move || format!("filter:{}", store.filter.get())))
                .child(text(move || format!("status:{}", store.status.get())))
                .child(text(move || format!("opened:{}", opened.get())))
                .child(text(move || {
                    format!("editing:{}", store.renaming.get().is_some())
                }))
                .child(text(move || {
                    format!(
                        "drag:{}",
                        store
                            .drag
                            .with(|d| d.as_ref().map(|d| d.label.to_string()).unwrap_or_default())
                    )
                }))
                .child(text(move || {
                    format!(
                        "root:{}",
                        store.history.with(|h| h.current().display().to_string())
                    )
                }))
                .child(text(move || {
                    format!("batches:{}", store.metrics.with(|m| m.fs_batches))
                }))
                .child(text(move || {
                    format!("drop:{}", store.drop_target.with(|d| d.is_some()))
                })),
        )
}
