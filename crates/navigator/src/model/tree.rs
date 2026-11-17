//! The lazily loaded file tree and its flattening into rows.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::Arc,
};

use super::{
    entry::{DirSource, Entry, EntryId, EntryRef, Kind, path_id},
    sort::ViewOptions,
};

/// What is known about one directory's children.
#[derive(Debug, Clone)]
pub enum DirState {
    /// A load was requested and has not finished.
    Loading,
    /// Sorted children (hidden ones included; filtered when flattening).
    Loaded(Arc<Vec<EntryRef>>),
    /// Listing failed (permissions, vanished).
    Failed(Arc<str>),
}

/// How a row's disclosure chevron looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disclosure {
    /// A file: no chevron.
    None,
    /// A closed folder.
    Collapsed,
    /// An open folder.
    Expanded,
    /// An open folder whose listing is in flight.
    Loading,
    /// An open folder whose listing failed.
    Failed,
}

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The entry (for a draft row: a placeholder).
    pub entry: EntryRef,
    /// Nesting level, 0 for the root's children.
    pub depth: u16,
    /// Chevron state.
    pub disclosure: Disclosure,
    /// An inline "new file/new folder" row: the name is being typed.
    pub draft: bool,
}

/// A pending new file/folder: shown as a row at the top of `parent`'s children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The folder it will be created in.
    pub parent: EntryId,
    /// File or folder.
    pub kind: Kind,
}

/// Result of asking to expand a folder.
#[derive(Debug, PartialEq, Eq)]
pub enum Expand {
    /// Children are known (or the entry is a file).
    Ready,
    /// The caller must list this directory and call [`FileTree::finish_load`].
    Load(Arc<Path>),
}

#[derive(Debug)]
struct Dir {
    state: DirState,
}

/// The tree model: a root, per-directory listings loaded on demand and the
/// expanded set. Pure data: no signals, no threads.
#[derive(Debug)]
pub struct FileTree {
    source: Arc<dyn DirSource>,
    root: EntryRef,
    options: ViewOptions,
    dirs: HashMap<EntryId, Dir>,
    expanded: HashSet<EntryId>,
    known: HashMap<EntryId, EntryRef>,
    filter: String,
    /// Directories that must be listed because an earlier root left them
    /// expanded (see [`FileTree::set_root`]); drained by the caller.
    pending_loads: Vec<Arc<Path>>,
}

impl FileTree {
    /// A tree over `source` rooted at `root` (nothing loaded yet).
    pub fn new(source: Arc<dyn DirSource>, root: impl AsRef<Path>) -> Self {
        let root = Entry::new(Arc::<Path>::from(root.as_ref()), Kind::Dir);
        let mut known = HashMap::new();
        known.insert(root.id, root.clone());
        Self {
            source,
            root,
            options: ViewOptions::default(),
            dirs: HashMap::new(),
            expanded: HashSet::new(),
            known,
            filter: String::new(),
            pending_loads: Vec::new(),
        }
    }

    /// The directory source (to list on a worker thread).
    pub fn source(&self) -> Arc<dyn DirSource> {
        self.source.clone()
    }

    /// The root entry.
    pub fn root(&self) -> &EntryRef {
        &self.root
    }

    /// Re-roots: drops all listings, keeps the expanded set (it is keyed by
    /// absolute path, so folders expanded earlier open again, as CodeRunner's
    /// `expandedPaths` do). Returns the path to load.
    pub fn set_root(&mut self, root: impl AsRef<Path>) -> Arc<Path> {
        self.root = Entry::new(Arc::<Path>::from(root.as_ref()), Kind::Dir);
        self.dirs.clear();
        self.known.clear();
        self.known.insert(self.root.id, self.root.clone());
        self.filter.clear();
        self.pending_loads.clear();
        self.dirs.insert(
            self.root.id,
            Dir {
                state: DirState::Loading,
            },
        );
        self.root.path.clone()
    }

    /// Marks the root as loading and returns its path (first load).
    pub fn begin_root_load(&mut self) -> Arc<Path> {
        self.pending_loads.clear();
        self.dirs.insert(
            self.root.id,
            Dir {
                state: DirState::Loading,
            },
        );
        self.root.path.clone()
    }

    /// Directories that were expanded before [`FileTree::set_root`] and are
    /// met again while walking a fresh listing: the caller must list them
    /// (and call [`FileTree::finish_load`]) to restore the expansion.
    pub fn drain_pending_loads(&mut self) -> Vec<Arc<Path>> {
        std::mem::take(&mut self.pending_loads)
    }

    /// Current view options.
    pub fn options(&self) -> ViewOptions {
        self.options
    }

    /// Changes the view options (re-sorts every loaded listing).
    pub fn set_options(&mut self, options: ViewOptions) {
        self.options = options;
        for dir in self.dirs.values_mut() {
            if let DirState::Loaded(list) = &dir.state {
                let mut v: Vec<EntryRef> = list.as_ref().clone();
                options.sort(&mut v);
                dir.state = DirState::Loaded(Arc::new(v));
            }
        }
    }

    /// The type-to-filter text.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Sets the filter text.
    pub fn set_filter(&mut self, text: impl Into<String>) {
        self.filter = text.into();
    }

    /// Looks an entry up by id (any entry seen so far).
    pub fn entry(&self, id: EntryId) -> Option<EntryRef> {
        self.known.get(&id).cloned()
    }

    /// The loaded, sorted children of `id` (hidden ones included).
    pub fn listing(&self, id: EntryId) -> Option<Arc<Vec<EntryRef>>> {
        match self.dirs.get(&id).map(|d| &d.state) {
            Some(DirState::Loaded(l)) => Some(l.clone()),
            _ => None,
        }
    }

    /// The state of the directory `id`, if requested.
    pub fn dir_state(&self, id: EntryId) -> Option<&DirState> {
        self.dirs.get(&id).map(|d| &d.state)
    }

    /// The parent folder entry of `id`, if known.
    pub fn parent_of(&self, id: EntryId) -> Option<EntryRef> {
        let e = self.known.get(&id)?;
        let parent = e.path.parent()?;
        self.known.get(&path_id(parent)).cloned()
    }

    /// Whether the folder is expanded.
    pub fn is_expanded(&self, id: EntryId) -> bool {
        self.expanded.contains(&id)
    }

    /// All expanded ids (for persistence).
    pub fn expanded(&self) -> &HashSet<EntryId> {
        &self.expanded
    }

    /// Expands a folder; asks the caller to load it when needed.
    pub fn expand(&mut self, id: EntryId) -> Expand {
        let Some(entry) = self.known.get(&id).cloned() else {
            return Expand::Ready;
        };
        if !entry.is_dir() {
            return Expand::Ready;
        }
        self.expanded.insert(id);
        if self.dirs.contains_key(&id) {
            return Expand::Ready;
        }
        self.dirs.insert(
            id,
            Dir {
                state: DirState::Loading,
            },
        );
        Expand::Load(entry.path.clone())
    }

    /// Collapses a folder (its listing stays cached).
    pub fn collapse(&mut self, id: EntryId) {
        self.expanded.remove(&id);
    }

    /// Expands or collapses.
    pub fn toggle(&mut self, id: EntryId) -> Expand {
        if self.is_expanded(id) {
            self.collapse(id);
            Expand::Ready
        } else {
            self.expand(id)
        }
    }

    /// Stores a finished listing. A failed refresh of an already loaded
    /// directory keeps the old listing.
    pub fn finish_load(&mut self, dir: &Path, result: io::Result<Vec<EntryRef>>) {
        let id = path_id(dir);
        match result {
            Ok(mut list) => {
                self.options.sort(&mut list);
                // Children that were expanded before a re-root: mark them loading and
                // hand their paths to the caller, or the restored chevron would never open.
                let mut pending = Vec::new();
                for e in &list {
                    self.known.insert(e.id, e.clone());
                    if e.is_dir() && self.expanded.contains(&e.id) && !self.dirs.contains_key(&e.id)
                    {
                        pending.push(e.path.clone());
                    }
                }
                for p in &pending {
                    self.dirs.insert(
                        path_id(p),
                        Dir {
                            state: DirState::Loading,
                        },
                    );
                }
                self.pending_loads.extend(pending);
                self.dirs.insert(
                    id,
                    Dir {
                        state: DirState::Loaded(Arc::new(list)),
                    },
                );
            }
            Err(err) => {
                let had_listing = matches!(
                    self.dirs.get(&id),
                    Some(Dir {
                        state: DirState::Loaded(_)
                    })
                );
                if !had_listing {
                    self.dirs.insert(
                        id,
                        Dir {
                            state: DirState::Failed(err.to_string().into()),
                        },
                    );
                }
            }
        }
    }

    /// Loads `dir` synchronously (tests, benchmarks).
    pub fn load_now(&mut self, dir: &Path) {
        let r = self.source.list(dir);
        self.finish_load(dir, r);
    }

    /// Loads and expands everything below `id` synchronously (benchmarks).
    pub fn expand_all_now(&mut self, id: EntryId) {
        if let Expand::Load(p) = self.expand(id) {
            self.load_now(&p);
        }
        let kids: Vec<EntryId> = match self.dirs.get(&id).map(|d| &d.state) {
            Some(DirState::Loaded(l)) => l.iter().filter(|e| e.is_dir()).map(|e| e.id).collect(),
            _ => Vec::new(),
        };
        for k in kids {
            self.expand_all_now(k);
        }
    }

    /// Directories that are loaded (the watcher's relevant set), as paths.
    pub fn loaded_dirs(&self) -> Vec<Arc<Path>> {
        self.dirs
            .iter()
            .filter(|(_, d)| matches!(d.state, DirState::Loaded(_)))
            .filter_map(|(id, _)| self.known.get(id).map(|e| e.path.clone()))
            .collect()
    }

    /// Given changed paths (from the watcher), the loaded directories that
    /// must be listed again: the parent of each changed path, and the path
    /// itself when it is a loaded directory.
    pub fn dirs_to_refresh<'a>(
        &self,
        changed: impl IntoIterator<Item = &'a Path>,
    ) -> Vec<Arc<Path>> {
        let mut out: Vec<Arc<Path>> = Vec::new();
        let mut seen = HashSet::new();
        for p in changed {
            for cand in [p.parent(), Some(p)].into_iter().flatten() {
                let id = path_id(cand);
                if matches!(
                    self.dirs.get(&id),
                    Some(Dir {
                        state: DirState::Loaded(_)
                    })
                ) && seen.insert(id)
                    && let Some(e) = self.known.get(&id) {
                        out.push(e.path.clone());
                    }
            }
        }
        out
    }

    /// Folders that must be listed (and expanded) so `target` becomes visible.
    pub fn reveal(&mut self, target: &Path) -> Vec<Arc<Path>> {
        let mut loads = Vec::new();
        let Ok(rel) = target.strip_prefix(&self.root.path) else {
            return loads;
        };
        let mut cur = self.root.path.to_path_buf();
        let comps: Vec<_> = rel.components().collect();
        // Every ancestor folder of the target, not the target itself.
        let take = comps.len().saturating_sub(1);
        let mut chain = vec![self.root.id];
        for c in comps.iter().take(take) {
            cur.push(c);
            chain.push(path_id(&cur));
        }
        for id in chain {
            if let Expand::Load(p) = self.expand_known_or_pending(id) {
                loads.push(p);
            }
        }
        loads
    }

    fn expand_known_or_pending(&mut self, id: EntryId) -> Expand {
        if self.known.contains_key(&id) {
            self.expand(id)
        } else {
            Expand::Ready
        }
    }

    fn visible_children<'a>(
        &self,
        dir: &'a [EntryRef],
    ) -> impl Iterator<Item = &'a EntryRef> + use<'a> {
        let show_hidden = self.options.show_hidden;
        dir.iter().filter(move |e| show_hidden || !e.hidden)
    }

    /// Flattens the expanded tree into rows (the filter applies when set).
    pub fn visible(&self, draft: Option<&Draft>) -> Vec<Row> {
        let mut out = Vec::new();
        if self.filter.is_empty() {
            self.walk(self.root.id, 0, draft, &mut out);
        } else {
            let needle = self.filter.to_lowercase();
            self.walk_filtered(self.root.id, 0, &needle, &mut out);
        }
        out
    }

    fn draft_row(&self, parent: &EntryRef, kind: Kind, depth: u16) -> Row {
        let placeholder = Entry::new(Arc::<Path>::from(parent.path.join("\u{0}draft")), kind);
        Row {
            entry: placeholder,
            depth,
            disclosure: Disclosure::None,
            draft: true,
        }
    }

    fn walk(&self, dir_id: EntryId, depth: u16, draft: Option<&Draft>, out: &mut Vec<Row>) {
        let Some(Dir { state }) = self.dirs.get(&dir_id) else {
            return;
        };
        if let (Some(d), Some(parent)) = (draft, self.known.get(&dir_id))
            && d.parent == dir_id {
                out.push(self.draft_row(parent, d.kind, depth));
            }
        let DirState::Loaded(list) = state else {
            return;
        };
        for e in self.visible_children(list) {
            let (disclosure, open) = if e.is_dir() {
                let open = self.expanded.contains(&e.id);
                let d = match (open, self.dirs.get(&e.id).map(|d| &d.state)) {
                    (false, _) => Disclosure::Collapsed,
                    (true, Some(DirState::Loading)) | (true, None) => Disclosure::Loading,
                    (true, Some(DirState::Failed(_))) => Disclosure::Failed,
                    (true, Some(DirState::Loaded(_))) => Disclosure::Expanded,
                };
                (d, open)
            } else {
                (Disclosure::None, false)
            };
            out.push(Row {
                entry: e.clone(),
                depth,
                disclosure,
                draft: false,
            });
            if open {
                self.walk(e.id, depth + 1, draft, out);
            }
        }
    }

    /// Filtered flattening: only loaded folders are searched; a folder shows
    /// when it or a loaded descendant matches.
    fn walk_filtered(&self, dir_id: EntryId, depth: u16, needle: &str, out: &mut Vec<Row>) -> bool {
        let Some(Dir {
            state: DirState::Loaded(list),
        }) = self.dirs.get(&dir_id)
        else {
            return false;
        };
        let mut any = false;
        for e in self.visible_children(list) {
            let matches = e.name.to_lowercase().contains(needle);
            if e.is_dir() {
                let mark = out.len();
                let child_match = self.walk_filtered(e.id, depth + 1, needle, out);
                if matches || child_match {
                    out.insert(
                        mark,
                        Row {
                            entry: e.clone(),
                            depth,
                            disclosure: if child_match {
                                Disclosure::Expanded
                            } else {
                                Disclosure::Collapsed
                            },
                            draft: false,
                        },
                    );
                    any = true;
                }
            } else if matches {
                out.push(Row {
                    entry: e.clone(),
                    depth,
                    disclosure: Disclosure::None,
                    draft: false,
                });
                any = true;
            }
        }
        any
    }

    /// Number of entries known (cached listings).
    pub fn known_count(&self) -> usize {
        self.known.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::{FsSource, SYNTH_ROOT, SynthSource};

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", " ".repeat(r.depth as usize), r.entry.name))
            .collect()
    }

    fn synth_tree() -> FileTree {
        let mut t = FileTree::new(Arc::new(SynthSource::nested_50k()), SYNTH_ROOT);
        let root = t.begin_root_load();
        t.load_now(&root);
        t
    }

    #[test]
    fn lazy_children_load_only_when_expanded() {
        let mut t = synth_tree();
        let rows = t.visible(None);
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[0].disclosure, Disclosure::Collapsed);
        let id = rows[0].entry.id;
        let Expand::Load(p) = t.expand(id) else {
            panic!("needs a load")
        };
        // Before the listing arrives the folder shows as loading and has no children.
        assert_eq!(t.visible(None)[0].disclosure, Disclosure::Loading);
        t.load_now(&p);
        let rows = t.visible(None);
        assert_eq!(rows.len(), 20 + 25);
        assert_eq!(rows[0].disclosure, Disclosure::Expanded);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(
            t.known_count(),
            1 + 20 + 25,
            "nothing below an unexpanded folder is listed"
        );
    }

    #[test]
    fn collapse_keeps_the_cache_and_expand_is_instant() {
        let mut t = synth_tree();
        let id = t.visible(None)[0].entry.id;
        if let Expand::Load(p) = t.expand(id) {
            t.load_now(&p);
        }
        t.collapse(id);
        assert_eq!(t.visible(None).len(), 20);
        assert_eq!(t.expand(id), Expand::Ready);
        assert_eq!(t.visible(None).len(), 45);
    }

    #[test]
    fn flatten_a_fully_expanded_50k_tree() {
        let mut t = synth_tree();
        let root = t.root().id;
        t.expand_all_now(root);
        let rows = t.visible(None);
        assert_eq!(rows.len(), 20 + 20 * 25 + 50_000);
    }

    #[test]
    fn filter_keeps_ancestors_of_matches_and_searches_loaded_folders_only() {
        let mut t = synth_tree();
        let first = t.visible(None)[0].entry.id;
        let Expand::Load(p) = t.expand(first) else {
            panic!()
        };
        t.load_now(&p);
        t.set_filter("pkg_03");
        let rows = t.visible(None);
        assert_eq!(names(&rows), ["module_00", " pkg_03"]);
        t.set_filter("zzz");
        assert!(t.visible(None).is_empty());
    }

    #[test]
    fn options_sort_folders_first_and_hide_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.txt", "a.txt", ".hidden", "file10", "file2"] {
            std::fs::write(dir.path().join(n), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("zdir")).unwrap();
        let mut t = FileTree::new(Arc::new(FsSource), dir.path());
        let root = t.begin_root_load();
        t.load_now(&root);
        assert_eq!(
            names(&t.visible(None)),
            ["a.txt", "b.txt", "file2", "file10", "zdir"]
        );
        t.set_options(ViewOptions {
            show_hidden: true,
            folders_first: true,
        });
        assert_eq!(
            names(&t.visible(None)),
            ["zdir", ".hidden", "a.txt", "b.txt", "file2", "file10"]
        );
    }

    #[test]
    fn draft_row_is_first_child_of_its_parent() {
        let mut t = synth_tree();
        let first = t.visible(None)[0].entry.id;
        let Expand::Load(p) = t.expand(first) else {
            panic!()
        };
        t.load_now(&p);
        let rows = t.visible(Some(&Draft {
            parent: first,
            kind: Kind::Dir,
        }));
        assert!(rows[1].draft && rows[1].depth == 1);
        assert_eq!(rows.len(), 20 + 25 + 1);
    }

    #[test]
    fn expanded_folders_are_restored_after_a_reroot() {
        let mut t = synth_tree();
        let module = t.visible(None)[0].entry.id;
        let Expand::Load(p) = t.expand(module) else {
            panic!("module needs a load")
        };
        t.load_now(&p);
        let pkg = t.visible(None)[1].entry.id;
        let Expand::Load(p) = t.expand(pkg) else {
            panic!("pkg needs a load")
        };
        t.load_now(&p);
        assert_eq!(t.visible(None).len(), 20 + 25 + 100);
        let root = t.root().path.clone();
        t.set_root(&*root);
        let root = t.begin_root_load();
        t.load_now(&root);
        let mut pending = t.drain_pending_loads();
        assert_eq!(pending.len(), 1, "module_00 was expanded before the reroot");
        t.load_now(&pending.remove(0));
        pending = t.drain_pending_loads();
        assert_eq!(pending.len(), 1, "pkg_00 was expanded before the reroot");
        t.load_now(&pending.remove(0));
        assert_eq!(t.visible(None).len(), 20 + 25 + 100);
    }

    #[test]
    fn reveal_expands_every_ancestor() {
        let mut t = synth_tree();
        let target = Path::new("/synth/module_03/pkg_07/file_00042.json");
        let loads = t.reveal(target);
        assert_eq!(loads.len(), 1, "module_03 is known, pkg_07 is not yet");
        t.load_now(&loads[0]);
        let loads = t.reveal(target);
        assert_eq!(loads.len(), 1);
        t.load_now(&loads[0]);
        assert!(t.visible(None).iter().any(|r| &*r.entry.path == target));
    }

    #[test]
    fn refresh_targets_loaded_parents_only() {
        let t = synth_tree();
        let changed = [
            Path::new("/synth/module_00/new.txt"),
            Path::new("/synth/new.txt"),
        ];
        let dirs = t.dirs_to_refresh(changed.iter().copied());
        assert_eq!(dirs.len(), 1);
        assert_eq!(&*dirs[0], Path::new("/synth"));
    }
}
