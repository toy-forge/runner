//! The navigator's state: a `Copy` struct of signals around the pure
//! [`FileTree`]. Views read it; only its methods write it.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::{
    model::{
        ClickMods, Disclosure, Draft, EntryId, Expand, FileTree, Kind, NavCommand, RootHistory,
        Row, Selection, ViewOptions, edit_filter, fsops, path_id,
    },
    watch::{Debounce, DirWatcher},
};

/// Height of a navigator row in points (drag and drop maps y to a row with it).
pub const ROW_H: f32 = 24.0;

/// The flattened rows, compared by pointer so a signal of 50 000 rows is cheap to `set`.
#[derive(Clone)]
pub struct Shown(pub Rc<Vec<Row>>);

impl PartialEq for Shown {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Numbers the HUD shows.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metrics {
    /// Rows shown.
    pub rows: usize,
    /// Last flatten (model -> rows), microseconds.
    pub flatten_us: u64,
    /// Entries known to the model.
    pub known: usize,
    /// Last directory load (worker thread + apply), milliseconds.
    pub load_ms: f64,
    /// Rebuilds so far.
    pub rebuilds: u64,
    /// Directories watched.
    pub watched: usize,
    /// Batches of FS events applied.
    pub fs_batches: u64,
}

/// An in-flight in-app drag.
#[derive(Debug, Clone, PartialEq)]
pub struct DragState {
    /// Dragged entries (the selection, or the row under the pointer).
    pub ids: Vec<EntryId>,
    /// Label near the pointer.
    pub label: SharedString,
    /// Pointer in the window's coordinates.
    pub x: f32,
    /// Pointer in the window's coordinates.
    pub y: f32,
    /// Option held: copy instead of move.
    pub copy: bool,
}

/// What the app does with an `Open` of a file.
pub type OpenHandler = Rc<dyn Fn(PathBuf)>;

/// What the app does with "Search N Items...".
pub type SelectionHandler = Rc<dyn Fn(Vec<PathBuf>)>;

/// The navigator's state.
#[derive(Clone, Copy)]
pub struct Store {
    /// The model.
    pub tree: Stored<FileTree>,
    /// Visible rows.
    pub rows: Signal<Shown>,
    /// Selection and cursor.
    pub selection: Signal<Selection>,
    /// The row being renamed (a draft row for a new item).
    pub renaming: Signal<Option<EntryId>>,
    /// A pending new file/folder row.
    pub draft: Signal<Option<Draft>>,
    /// Type-to-filter text.
    pub filter: Signal<String>,
    /// Root history.
    pub history: Signal<RootHistory>,
    /// Sort/hidden options.
    pub options: Signal<ViewOptions>,
    /// The document open in the editor.
    pub current_doc: Signal<Option<EntryId>>,
    /// Drag in flight.
    pub drag: Signal<Option<DragState>>,
    /// Row a drag would drop into.
    pub drop_target: Signal<Option<EntryId>>,
    /// The list has the keyboard focus.
    pub focused: Signal<bool>,
    /// One-line status / last error.
    pub status: Signal<SharedString>,
    /// HUD numbers.
    pub metrics: Signal<Metrics>,
    /// The list element (scrolling, measuring).
    pub list: ElementRef,
    /// The focusable root of the navigator (key bindings live while it has the focus).
    pub root: ElementRef,
    /// Rows wholly in view (first, last), as the list reports.
    pub visible: Stored<Option<(usize, usize)>>,
    /// Scroll offset y.
    pub scroll_y: Stored<f32>,
    /// Page position of the list's top-left, measured when a drag starts.
    pub list_origin: Stored<Option<(f32, f32, f32)>>,
    /// Paths copied with Copy.
    pub clipboard: Stored<Vec<PathBuf>>,
    /// Owner of the component that made the store; every task runs under it.
    owner: Owner,
    open: Stored<Option<OpenHandler>>,
    open_new_window: Stored<Option<OpenHandler>>,
    search_folder: Stored<Option<OpenHandler>>,
    search_selection: Stored<Option<SelectionHandler>>,
    go_to_folder: Stored<Option<Rc<dyn Fn()>>>,
    new_template: Stored<Option<OpenHandler>>,
    watcher: Stored<Option<DirWatcher>>,
    watch_task: Stored<Option<Spawned<()>>>,
    spring: Stored<Option<Spawned<()>>>,
    first_press: Stored<Option<(EntryId, Instant)>>,
}

impl Store {
    /// A store over `tree`, loading its root. Call inside a component body (the
    /// signals belong to the calling owner).
    pub fn new(mut tree: FileTree) -> Self {
        let root = tree.begin_root_load();
        let root_path: PathBuf = root.to_path_buf();
        let store = Self {
            tree: stored(tree),
            rows: signal_with(Shown(Rc::new(Vec::new())), SignalEquals::Never),
            selection: signal(Selection::default()),
            renaming: signal(None),
            draft: signal(None),
            filter: signal(String::new()),
            history: signal(RootHistory::new(root_path)),
            options: signal(ViewOptions::default()),
            current_doc: signal(None),
            drag: signal(None),
            drop_target: signal(None),
            focused: signal(false),
            status: signal(SharedString::default()),
            metrics: signal(Metrics::default()),
            list: element_ref(),
            root: element_ref(),
            visible: stored(None),
            scroll_y: stored(0.0),
            list_origin: stored(None),
            clipboard: stored(Vec::new()),
            owner: Owner::current(),
            open: stored(None),
            open_new_window: stored(None),
            search_folder: stored(None),
            search_selection: stored(None),
            go_to_folder: stored(None),
            new_template: stored(None),
            watcher: stored(None),
            watch_task: stored(None),
            spring: stored(None),
            first_press: stored(None),
        };
        store.load(root);
        store
    }

    /// Runs `fut` on the engine thread under the store's own owner.
    ///
    /// A plain `spawn` from an event handler belongs to the owner the handler was made under;
    /// when that is a `when` branch that goes away (the rename input), the task is cancelled
    /// with it. Tasks of the store must outlive the rows, so they run under the component's owner.
    pub fn task(&self, fut: impl std::future::Future<Output = ()> + 'static) -> Spawned<()> {
        self.owner.run(|| spawn(fut))
    }

    /// Sets what opening a file does.
    pub fn on_open(&self, f: impl Fn(PathBuf) + 'static) {
        self.open.replace(Some(Rc::new(f)));
    }

    /// Sets what "Open in New Window" does.
    pub fn on_open_new_window(&self, f: impl Fn(PathBuf) + 'static) {
        self.open_new_window.replace(Some(Rc::new(f)));
    }

    /// Sets what "Search in Folder…" does.
    pub fn on_search_folder(&self, f: impl Fn(PathBuf) + 'static) {
        self.search_folder.replace(Some(Rc::new(f)));
    }

    /// Sets what "Search N Items..." does for a multi-selection.
    pub fn on_search_selection(&self, f: impl Fn(Vec<PathBuf>) + 'static) {
        self.search_selection.replace(Some(Rc::new(f)));
    }

    /// Sets a custom handler for "Go to Folder…" (returns `true` if handled).
    pub fn on_go_to_folder(&self, f: impl Fn() + 'static) {
        self.go_to_folder.replace(Some(Rc::new(f)));
    }

    /// Sets what creating a file from the active language template does.
    pub fn on_new_from_template(&self, f: impl Fn(PathBuf) + 'static) {
        self.new_template.replace(Some(Rc::new(f)));
    }

    pub fn trigger_open_new_window(&self, path: PathBuf) {
        let cb = self.open_new_window.with(|o| o.clone());
        if let Some(f) = cb {
            f(path);
        } else {
            self.status.set("Open in New Window is not available".into());
        }
    }

    pub fn trigger_search_folder(&self, path: PathBuf) {
        let cb = self.search_folder.with(|o| o.clone());
        if let Some(f) = cb {
            f(path);
        } else {
            self.status
                .set("Search...: switches the sidebar to the search tab".into());
        }
    }

    pub fn trigger_search_selection(&self, paths: Vec<PathBuf>) {
        let cb = self.search_selection.with(|o| o.clone());
        if let Some(f) = cb {
            f(paths);
        } else if let Some(first) = paths.into_iter().next() {
            let dir = if first.is_dir() {
                first
            } else {
                first
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.root_path())
            };
            self.trigger_search_folder(dir);
        }
    }

    pub fn trigger_go_to_folder(&self) -> bool {
        let cb = self.go_to_folder.with(|o| o.clone());
        if let Some(f) = cb {
            f();
            true
        } else {
            false
        }
    }

    pub fn trigger_new_from_template(&self, dir: PathBuf) -> bool {
        let cb = self.new_template.with(|o| o.clone());
        if let Some(f) = cb {
            f(dir);
            true
        } else {
            false
        }
    }

    // ---- model plumbing -------------------------------------------------

    /// Recomputes the rows from the model.
    pub fn rebuild(&self) {
        let t0 = Instant::now();
        let draft = self.draft.get_untracked();
        let (rows, known) = self
            .tree
            .with(|t| (t.visible(draft.as_ref()), t.known_count()));
        let flatten_us = t0.elapsed().as_micros() as u64;
        let n = rows.len();
        self.selection.update(|s| s.retain_visible(&rows));
        self.rows.set(Shown(Rc::new(rows)));
        self.metrics.update(|m| {
            m.rows = n;
            m.flatten_us = flatten_us;
            m.known = known;
            m.rebuilds += 1;
        });
    }

    /// The current rows (untracked).
    pub fn rows_now(&self) -> Rc<Vec<Row>> {
        self.rows.with_untracked(|s| s.0.clone())
    }

    /// Lists `path` on a worker thread, then applies it.
    pub fn load(&self, path: Arc<Path>) {
        let store = *self;
        let source = self.tree.with(|t| t.source());
        self.task(async move {
            let t0 = Instant::now();
            let p = path.clone();
            let result = blocking(move || source.list(&p)).await;
            let result = match result {
                Ok(r) => r,
                Err(e) => Err(std::io::Error::other(e.to_string())),
            };
            store.tree.with(|t| t.finish_load(&path, result));
            // Folders that were expanded under a previous root and are met again.
            let pending = store.tree.with(|t| t.drain_pending_loads());
            store
                .metrics
                .update(|m| m.load_ms = t0.elapsed().as_secs_f64() * 1000.0);
            store.rebuild();
            store.sync_watches();
            for p in pending {
                store.load(p);
            }
        });
    }

    /// Same as [`Store::load`] for a refresh of an already loaded folder.
    pub fn refresh_dirs(&self, dirs: Vec<Arc<Path>>) {
        for d in dirs {
            self.load(d);
        }
    }

    /// Expands or collapses a folder.
    pub fn toggle(&self, id: EntryId) {
        let r = self.tree.with(|t| t.toggle(id));
        if let Expand::Load(p) = r {
            self.load(p);
        }
        self.rebuild();
        self.sync_watches();
    }

    /// Expands a folder.
    pub fn expand(&self, id: EntryId) {
        let r = self.tree.with(|t| t.expand(id));
        if let Expand::Load(p) = r {
            self.load(p);
        }
        self.rebuild();
        self.sync_watches();
    }

    /// Collapses a folder.
    pub fn collapse(&self, id: EntryId) {
        self.tree.with(|t| t.collapse(id));
        self.rebuild();
        self.sync_watches();
    }

    /// Changes sort/hidden options.
    pub fn set_options(&self, f: impl FnOnce(&mut ViewOptions)) {
        let mut o = self.options.get_untracked();
        f(&mut o);
        self.options.set(o);
        self.tree.with(|t| t.set_options(o));
        self.rebuild();
    }

    /// Sets the filter text (type-to-filter and the footer field).
    pub fn set_filter(&self, text: String) {
        self.filter.set(text.clone());
        self.tree.with(|t| t.set_filter(text));
        self.rebuild();
    }

    // ---- root history ---------------------------------------------------

    fn reroot(&self, path: &Path) {
        let p = self.tree.with(|t| t.set_root(path));
        self.selection.set(Selection::default());
        self.filter.set(String::new());
        self.renaming.set(None);
        self.draft.set(None);
        self.rebuild();
        self.load(p);
        self.status.set(SharedString::default());
    }

    /// Navigates to `path` (pushes history).
    pub fn go(&self, path: &Path) {
        if self.history.with_untracked(|h| h.current() == path) {
            return;
        }
        self.history.update(|h| {
            h.go(path);
        });
        self.reroot(path);
    }

    /// Back button.
    pub fn back(&self) {
        let mut target = None;
        self.history
            .update(|h| target = h.back().map(Path::to_path_buf));
        if let Some(p) = target {
            self.reroot(&p);
        }
    }

    /// Forward button.
    pub fn forward(&self) {
        let mut target = None;
        self.history
            .update(|h| target = h.forward().map(Path::to_path_buf));
        if let Some(p) = target {
            self.reroot(&p);
        }
    }

    /// Cmd+Up.
    pub fn go_parent(&self) {
        let mut moved = false;
        self.history.update(|h| moved = h.go_parent());
        if moved {
            let p = self.history.with_untracked(|h| h.current().to_path_buf());
            self.reroot(&p);
        }
    }

    /// The root path.
    pub fn root_path(&self) -> PathBuf {
        self.history.with_untracked(|h| h.current().to_path_buf())
    }

    // ---- selection and keys --------------------------------------------

    /// A press on row `id` (modifiers and click count from the event).
    pub fn press_row(&self, id: EntryId, mods: ClickMods, clicks: u32) {
        let rows = self.rows_now();
        let was_selected = self
            .selection
            .with_untracked(|s| s.contains(id) && s.len() == 1);
        self.selection.update(|s| s.click(&rows, id, mods));
        let plain = !mods.shift && !mods.cmd;
        if clicks >= 2 && plain {
            self.first_press.replace(None);
            self.activate(id);
        } else if plain && was_selected && clicks == 1 {
            // A slow second click on the selected row starts a rename (Finder, Xcode).
            let slow = self.first_press.with(|p| {
                p.is_some_and(|(pid, at)| {
                    pid == id
                        && at.elapsed() > Duration::from_millis(550)
                        && at.elapsed() < Duration::from_millis(1500)
                })
            });
            if slow {
                self.first_press.replace(None);
                self.begin_rename(id);
                return;
            }
            self.first_press.replace(Some((id, Instant::now())));
        } else {
            self.first_press.replace(Some((id, Instant::now())));
        }
    }

    /// Double click / Return on an opened item: folders toggle, files open.
    pub fn activate(&self, id: EntryId) {
        let Some(e) = self.tree.with(|t| t.entry(id)) else {
            return;
        };
        if e.is_dir() {
            self.toggle(id);
        } else {
            self.open_path(&e.path);
        }
    }

    /// Opens a file in the editor.
    pub fn open_path(&self, path: &Path) {
        let handler = self.open.get();
        if let Some(h) = handler {
            h(path.to_path_buf());
        }
        self.status.set(format!("Open {}", path.display()).into());
    }

    fn cursor_row(&self, rows: &[Row]) -> Option<usize> {
        let c = self.selection.with_untracked(|s| s.cursor)?;
        crate::model::select::index_of(rows, c)
    }

    /// Keeps the cursor row on screen.
    pub fn scroll_cursor_into_view(&self) {
        let rows = self.rows_now();
        let Some(i) = self.cursor_row(&rows) else {
            return;
        };
        match self.visible.get() {
            Some((first, last)) if (first..=last).contains(&i) => {}
            Some((first, _)) => {
                let at = if i < first { 0.0 } else { 1.0 };
                self.list
                    .scroll_to_index(i as u32, Percentage(at), px(0.0), false);
            }
            None => self
                .list
                .scroll_to_index(i as u32, Percentage(0.5), px(0.0), false),
        }
    }

    /// Runs a keyboard command (report 10 G30).
    pub fn nav(&self, cmd: NavCommand) {
        let rows = self.rows_now();
        match cmd {
            NavCommand::Up { extend } => self.selection.update(|s| s.step(&rows, -1, extend)),
            NavCommand::Down { extend } => self.selection.update(|s| s.step(&rows, 1, extend)),
            NavCommand::PageUp => self.selection.update(|s| s.step(&rows, -10, false)),
            NavCommand::PageDown => self.selection.update(|s| s.step(&rows, 10, false)),
            NavCommand::First => self.selection.update(|s| s.jump(&rows, false, false)),
            NavCommand::Last => self.selection.update(|s| s.jump(&rows, true, false)),
            NavCommand::SelectAll => self.selection.update(|s| {
                if let (Some(f), Some(l)) = (rows.first(), rows.last()) {
                    s.anchor = Some(f.entry.id);
                    s.extend_to(&rows, l.entry.id);
                }
            }),
            NavCommand::Expand => {
                if let Some(i) = self.cursor_row(&rows) {
                    let r = &rows[i];
                    match r.disclosure {
                        Disclosure::Collapsed => self.expand(r.entry.id),
                        Disclosure::Expanded => {
                            if let Some(child) = rows.get(i + 1).filter(|c| c.depth > r.depth) {
                                self.selection.update(|s| s.select_only(child.entry.id));
                            }
                        }
                        _ => {}
                    }
                }
            }
            NavCommand::Collapse => {
                if let Some(i) = self.cursor_row(&rows) {
                    let r = &rows[i];
                    if r.disclosure == Disclosure::Expanded
                        && self.filter.with_untracked(String::is_empty)
                    {
                        self.collapse(r.entry.id);
                    } else if r.depth > 0 {
                        // The parent row: nearest row above with a smaller depth.
                        if let Some(p) = rows[..i].iter().rev().find(|p| p.depth < r.depth) {
                            self.selection.update(|s| s.select_only(p.entry.id));
                        }
                    }
                }
            }
            NavCommand::Rename => {
                if let Some(i) = self.cursor_row(&rows) {
                    self.begin_rename(rows[i].entry.id);
                }
            }
            NavCommand::Open => {
                if let Some(i) = self.cursor_row(&rows) {
                    let e = rows[i].entry.clone();
                    if e.is_dir() {
                        self.go(&e.path);
                    } else {
                        self.open_path(&e.path);
                    }
                }
            }
            NavCommand::GoParent => self.go_parent(),
            NavCommand::Trash => self.trash_selected(),
            NavCommand::NewFolder => self.begin_new(Kind::Dir),
            NavCommand::Escape => {
                if self.renaming.get_untracked().is_some() {
                    self.cancel_edit();
                } else if !self.filter.with_untracked(String::is_empty) {
                    self.set_filter(String::new());
                } else {
                    self.selection.update(|s| s.clear());
                }
            }
            NavCommand::Type(_) | NavCommand::Backspace => {
                let cur = self.filter.get_untracked();
                if let Some(next) = edit_filter(&cur, cmd) {
                    self.set_filter(next);
                }
            }
        }
        if matches!(
            cmd,
            NavCommand::Up { .. }
                | NavCommand::Down { .. }
                | NavCommand::PageUp
                | NavCommand::PageDown
                | NavCommand::First
                | NavCommand::Last
                | NavCommand::Collapse
                | NavCommand::Expand
        ) {
            self.scroll_cursor_into_view();
        }
    }

    // ---- inline rename and new items -----------------------------------

    /// Starts renaming the row `id` (an input replaces its label).
    pub fn begin_rename(&self, id: EntryId) {
        if self.tree.with(|t| t.entry(id)).is_some() {
            self.renaming.set(Some(id));
        }
    }

    /// Leaves rename/new mode without changes.
    pub fn cancel_edit(&self) {
        self.renaming.set(None);
        // The input held the focus; take it back or the key bindings stay unbound.
        self.root.focus();
        if self.draft.get_untracked().is_some() {
            self.draft.set(None);
            self.rebuild();
        }
    }

    /// The folder new items go into: the selected folder, else the selected
    /// file's folder, else the root.
    fn target_dir(&self) -> crate::model::EntryRef {
        let rows = self.rows_now();
        let sel = self.selection.with_untracked(|s| s.cursor);
        let from_sel = sel
            .and_then(|c| rows.iter().find(|r| r.entry.id == c))
            .map(|r| r.entry.clone());
        match from_sel {
            Some(e) if e.is_dir() => e,
            Some(e) => e
                .path
                .parent()
                .and_then(|p| self.tree.with(|t| t.entry(path_id(p))))
                .unwrap_or_else(|| self.tree.with(|t| t.root().clone())),
            None => self.tree.with(|t| t.root().clone()),
        }
    }

    /// New File / New Folder: expands the target folder and shows an input row.
    pub fn begin_new(&self, kind: Kind) {
        let dir = self.target_dir();
        let is_root = self.tree.with(|t| t.root().id == dir.id);
        if !is_root && !self.tree.with(|t| t.is_expanded(dir.id)) {
            self.expand(dir.id);
        }
        self.draft.set(Some(Draft {
            parent: dir.id,
            kind,
        }));
        self.rebuild();
        let rows = self.rows_now();
        if let Some(i) = rows.iter().position(|r| r.draft) {
            let id = rows[i].entry.id;
            self.renaming.set(Some(id));
            self.list
                .scroll_to_index(i as u32, Percentage(0.3), px(0.0), false);
        }
    }

    /// The text of a rename input was confirmed (Return or blur).
    pub fn commit_edit(&self, id: EntryId, text: &str) {
        if self.renaming.get_untracked() != Some(id) {
            return;
        }
        self.renaming.set(None);
        let rows = self.rows_now();
        let Some(row) = rows.iter().find(|r| r.entry.id == id).cloned() else {
            self.cancel_edit();
            return;
        };
        let text = text.trim().to_owned();
        if row.draft {
            let parent = row
                .entry
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            self.draft.set(None);
            if text.is_empty() {
                self.rebuild();
                return;
            }
            let r = if row.entry.is_dir() {
                fsops::create_dir(&parent, &text)
            } else {
                fsops::create_file(&parent, &text)
            };
            self.finish_fs(r, &parent);
        } else if !text.is_empty() && text != *row.entry.name {
            let r = fsops::rename(&row.entry.path, &text, fsops::Conflict::Fail);
            match r {
                Err(fsops::FsError::Exists(_)) => {
                    self.ask_replace(row.entry.path.to_path_buf(), text)
                }
                other => {
                    let parent = row
                        .entry
                        .path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default();
                    self.finish_fs(other, &parent);
                }
            }
        }
    }

    fn finish_fs(&self, r: Result<PathBuf, fsops::FsError>, parent: &Path) {
        match r {
            Ok(path) => {
                self.status.set(format!("{}", path.display()).into());
                self.refresh_dirs(vec![Arc::from(parent)]);
                // Select the new path once the listing arrives: ids are path hashes.
                let id = path_id(&path);
                self.selection.update(|s| s.select_only(id));
            }
            Err(e) => {
                self.status.set(format!("Error: {e}").into());
                self.rebuild();
            }
        }
    }

    /// "A file or folder with the same name already exists in this directory.
    /// Replacing it will overwrite its current contents." (report 01)
    fn ask_replace(&self, from: PathBuf, new_name: String) {
        let store = *self;
        self.task(async move {
            let answer = alert_dialog()
                .title(format!("\"{new_name}\" already exists"))
                .description("A file or folder with the same name already exists in this directory. Replacing it will overwrite its current contents.")
                .cancel("keep", "Don't Replace")
                .destructive("replace", "Replace")
                .show()
                .await;
            if matches!(answer, Ok(r) if r.action.as_deref() == Some("replace")) {
                let parent = from.parent().map(Path::to_path_buf).unwrap_or_default();
                let r = fsops::rename(&from, &new_name, fsops::Conflict::Replace);
                store.finish_fs(r, &parent);
            }
        });
    }

    // ---- trash, duplicate, copy/paste ----------------------------------

    /// Paths of the selected rows.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let rows = self.rows_now();
        self.selection.with_untracked(|s| {
            rows.iter()
                .filter(|r| s.contains(r.entry.id) && !r.draft)
                .map(|r| r.entry.path.to_path_buf())
                .collect()
        })
    }

    /// Cmd+Backspace: confirm, then move to the Trash.
    pub fn trash_selected(&self) {
        let rows = self.rows_now();
        let picked: Vec<_> = self.selection.with_untracked(|s| {
            rows.iter()
                .filter(|r| s.contains(r.entry.id) && !r.draft)
                .map(|r| r.entry.clone())
                .collect()
        });
        if picked.is_empty() {
            return;
        }
        let (title, desc) = if picked.len() == 1 {
            let kind = if picked[0].is_dir() { "folder" } else { "file" };
            (
                format!(
                    "Are you sure you want to delete the {kind} \"{}\"?",
                    picked[0].name
                ),
                format!("The {kind} will be moved to the Trash."),
            )
        } else {
            (
                format!(
                    "Are you sure you want to delete the {} selected items?",
                    picked.len()
                ),
                "The items will be moved to the Trash.".to_owned(),
            )
        };
        let store = *self;
        self.task(async move {
            let answer = alert_dialog()
                .title(title)
                .description(desc)
                .cancel("cancel", "Cancel")
                .destructive("delete", "Move to Trash")
                .show()
                .await;
            if !matches!(answer, Ok(r) if r.action.as_deref() == Some("delete")) {
                return;
            }
            let mut parents = Vec::new();
            for e in picked {
                match move_to_trash(e.path.to_path_buf()).await {
                    Ok(_) => parents.extend(e.path.parent().map(Arc::<Path>::from)),
                    Err(err) => store.status.set(format!("Trash failed: {err}").into()),
                }
            }
            parents.dedup();
            store.refresh_dirs(parents);
        });
    }

    /// Duplicate in the context menu.
    pub fn duplicate_selected(&self) {
        for p in self.selected_paths() {
            let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
            let r = fsops::duplicate(&p);
            self.finish_fs(r, &parent);
        }
    }

    /// Copy: remember the paths and put their text on the system clipboard.
    pub fn copy_selected(&self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        set_clipboard(text);
        self.clipboard.replace(paths);
    }

    /// Paste into the target folder (copies, uniquifying names).
    pub fn paste(&self) {
        let dir = self.target_dir();
        let mut err = None;
        for p in self.clipboard.get() {
            if let Err(e) = fsops::copy_into(&p, &dir.path, fsops::Conflict::Uniquify) {
                err = Some(e.to_string());
            }
        }
        if let Some(e) = err {
            self.status.set(format!("Paste failed: {e}").into());
        }
        self.refresh_dirs(vec![dir.path.clone()]);
    }

    // ---- drag and drop --------------------------------------------------

    /// Row index under a window y (None above/below the rows).
    pub fn row_at_page_y(&self, page_y: f32) -> Option<usize> {
        let (_, top, _) = self.list_origin.get()?;
        let y = page_y - top + self.scroll_y.get();
        (y >= 0.0).then(|| (y / ROW_H) as usize)
    }

    /// The folder a drop on row `i` lands in (the row itself when a folder, else its parent).
    fn drop_dir_for(&self, rows: &[Row], i: Option<usize>) -> Option<crate::model::EntryRef> {
        match i.and_then(|i| rows.get(i)) {
            Some(r) if r.entry.is_dir() && !r.draft => Some(r.entry.clone()),
            Some(r) => r
                .entry
                .path
                .parent()
                .and_then(|p| self.tree.with(|t| t.entry(path_id(p)))),
            None => Some(self.tree.with(|t| t.root().clone())),
        }
    }

    /// Begins an in-app drag from the row under `page_y`.
    pub fn drag_begin(&self, page: (f32, f32), copy: bool) {
        let rows = self.rows_now();
        let Some(i) = self.row_at_page_y(page.1).filter(|i| *i < rows.len()) else {
            return;
        };
        let id = rows[i].entry.id;
        let selected = self.selection.with_untracked(|s| s.contains(id));
        if !selected {
            self.selection.update(|s| s.select_only(id));
        }
        let ids = self.selection.with_untracked(|s| s.in_order(&rows));
        let label = if ids.len() > 1 {
            format!("{} items", ids.len())
        } else {
            rows[i].entry.name.to_string()
        };
        self.drag.set(Some(DragState {
            ids,
            label: label.into(),
            x: page.0,
            y: page.1,
            copy,
        }));
    }

    /// The pointer moved during a drag.
    pub fn drag_move(&self, page: (f32, f32), copy: bool) {
        let Some(mut d) = self.drag.get_untracked() else {
            return;
        };
        d.x = page.0;
        d.y = page.1;
        d.copy = copy;
        let rows = self.rows_now();
        let idx = self.row_at_page_y(page.1);
        let target = idx
            .and_then(|i| rows.get(i))
            .and_then(|_| self.drop_dir_for(&rows, idx));
        // Dropping onto the dragged rows themselves is not a target.
        let target = target.filter(|t| !d.ids.contains(&t.id));
        let tid = target.as_ref().map(|t| t.id);
        if self.drop_target.get_untracked() != tid {
            self.drop_target.set(tid);
            self.spring.replace(None);
            // Spring-loaded folders: hovering a closed folder opens it.
            if let (Some(t), Some(i)) = (target, idx)
                && rows
                    .get(i)
                    .is_some_and(|r| r.entry.id == t.id && r.disclosure == Disclosure::Collapsed)
            {
                let store = *self;
                let id = t.id;
                let task = self.task(async move {
                    timer(Duration::from_millis(700)).await;
                    if store.drop_target.get_untracked() == Some(id) {
                        store.expand(id);
                    }
                });
                self.spring.replace(Some(task));
            }
        }
        self.drag.set(Some(d));
    }

    /// The pointer was released: move (or copy) into the target.
    pub fn drag_end(&self) {
        let d = self.drag.get_untracked();
        let target = self.drop_target.get_untracked();
        self.drag.set(None);
        self.drop_target.set(None);
        self.spring.replace(None);
        if let (Some(d), Some(target)) = (d, target) {
            self.move_ids(&d.ids, target, d.copy);
        }
    }

    /// The gesture was cancelled.
    pub fn drag_cancel(&self) {
        self.drag.set(None);
        self.drop_target.set(None);
        self.spring.replace(None);
    }

    /// Moves or copies entries into the folder `target`.
    pub fn move_ids(&self, ids: &[EntryId], target: EntryId, copy: bool) {
        let Some(dir) = self.tree.with(|t| t.entry(target)) else {
            return;
        };
        let mut refresh: Vec<Arc<Path>> = vec![dir.path.clone()];
        let mut err = None;
        for id in ids {
            let Some(e) = self.tree.with(|t| t.entry(*id)) else {
                continue;
            };
            let r = if copy {
                fsops::copy_into(&e.path, &dir.path, fsops::Conflict::Uniquify)
            } else {
                fsops::move_into(&e.path, &dir.path, fsops::Conflict::Fail)
            };
            match r {
                Ok(_) if !copy => refresh.extend(e.path.parent().map(Arc::<Path>::from)),
                Ok(_) => {}
                Err(x) => err = Some(x.to_string()),
            }
        }
        if let Some(e) = err {
            self.status
                .set(format!("{}: {e}", if copy { "Copy failed" } else { "Move failed" }).into());
        }
        if !self.tree.with(|t| t.is_expanded(dir.id)) {
            self.expand(dir.id);
        }
        refresh.dedup();
        self.refresh_dirs(refresh);
    }

    /// Files dropped from Finder (`file:` URLs) into `target` (a folder row, or the root).
    pub fn drop_urls(&self, urls: &[String], target: Option<EntryId>) {
        let dir = match target.and_then(|t| self.tree.with(|tree| tree.entry(t))) {
            Some(e) if e.is_dir() => e,
            Some(e) => e
                .path
                .parent()
                .and_then(|p| self.tree.with(|t| t.entry(path_id(p))))
                .unwrap_or_else(|| self.tree.with(|t| t.root().clone())),
            None => self.tree.with(|t| t.root().clone()),
        };
        let mut n = 0;
        let mut err = None;
        for u in urls {
            let Some(p) = fsops::path_from_file_url(u) else {
                continue;
            };
            match fsops::copy_into(&p, &dir.path, fsops::Conflict::Uniquify) {
                Ok(_) => n += 1,
                Err(e) => err = Some(e.to_string()),
            }
        }
        self.status.set(
            match err {
                Some(e) => format!("Drop failed: {e}"),
                None => format!("Copied {n} item(s) into {}", dir.name),
            }
            .into(),
        );
        self.drop_target.set(None);
        self.refresh_dirs(vec![dir.path.clone()]);
    }

    // ---- current document, reveal --------------------------------------

    /// Marks the document open in the editor (its row is highlighted).
    pub fn set_current_doc(&self, path: Option<&Path>) {
        self.current_doc.set(path.map(path_id));
    }

    /// Go to Current Document (Opt+Cmd+J): expands its ancestors, selects and scrolls to it.
    pub fn reveal(&self, path: &Path) {
        let loads = self.tree.with(|t| t.reveal(path));
        for p in loads {
            self.load(p);
        }
        self.rebuild();
        let id = path_id(path);
        let rows = self.rows_now();
        if let Some(i) = crate::model::select::index_of(&rows, id) {
            self.selection.update(|s| s.select_only(id));
            self.list
                .scroll_to_index(i as u32, Percentage(0.5), px(0.0), true);
        } else {
            // Not listed yet: select by id; the listing arriving rebuilds the rows.
            self.selection.update(|s| s.select_only(id));
        }
    }

    // ---- file system watching ------------------------------------------

    /// Starts the FSEvents watcher and the task that applies its batches.
    pub fn start_watching(&self) {
        let (watcher, mut rx) = match DirWatcher::new(Debounce::default()) {
            Ok(w) => w,
            Err(e) => {
                self.status.set(format!("Watching failed: {e}").into());
                return;
            }
        };
        self.watcher.replace(Some(watcher));
        self.sync_watches();
        let store = *self;
        let task = self.task(async move {
            while let Some(batch) = rx.recv().await {
                let dirs = store
                    .tree
                    .with(|t| t.dirs_to_refresh(batch.iter().map(PathBuf::as_path)));
                store.metrics.update(|m| m.fs_batches += 1);
                store.refresh_dirs(dirs);
            }
        });
        self.watch_task.replace(Some(task));
    }

    /// Watches the root and every expanded loaded folder.
    pub fn sync_watches(&self) {
        let dirs: Vec<Arc<Path>> = self.tree.with(|t| {
            let mut v = vec![t.root().path.clone()];
            v.extend(
                t.loaded_dirs()
                    .into_iter()
                    .filter(|p| t.is_expanded(path_id(p)) && *p != t.root().path),
            );
            v
        });
        let n = self.watcher.with(|w| {
            w.as_mut().map(|w| {
                w.sync(dirs.iter().map(|d| d.as_ref()));
                w.len()
            })
        });
        if let Some(n) = n {
            self.metrics.update(|m| m.watched = n);
        }
    }
}
