//! The session: which windows and tabs were open and how they were laid out,
//! so the app comes back the way it was left.
//!
//! `session.json` ([`Session`]) holds a list of [`WindowState`]s, each with
//! its frame, panel visibility and sizes, sidebar state and tabs
//! ([`TabState`]: document id, file, per-tab view state). The format is
//! versioned ([`SESSION_VERSION`]); version 1 (a single window) is migrated
//! on read, unknown fields are ignored, and a missing or damaged file is the
//! empty session with a warning ([`Session::load`]).
//! [`Session::prune_unavailable`] drops tabs whose files are gone.
//!
//! `window-state.json` ([`WindowDefaults`]) holds what new windows start
//! with (the last used frame and panel sizes), plus the geometry helpers
//! [`cascade_frame`] and [`clamp_to_screens`].
//!
//! Tab ordering rules (where a new tab goes per the *Insert New Tabs*
//! preference, which tab is selected after a close, ⌘1..⌘0, ⌘{ ⌘}) are methods of
//! [`WindowState`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use scratch_config::{ConsolePosition, InsertNewTabs, write_atomic};
use serde::{Deserialize, Serialize};

use crate::DocumentId;
use crate::history::NavigationHistory;
use crate::state::ViewState;

/// The format version written by this build.
pub const SESSION_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Geometry

/// A window frame in points, as the platform reports it; restored verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Frame {
    /// Left edge.
    pub x: f64,
    /// Top edge (or bottom, in whatever convention the platform layer uses
    /// consistently).
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Frame {
    /// A frame.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The right edge.
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    /// The bottom edge (`y + height`).
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn overlap_area(&self, other: &Frame) -> f64 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        w * h
    }
}

/// The minimum size CodeRunner's main window allows (content 500x300).
pub const MIN_WINDOW_SIZE: (f64, f64) = (500.0, 300.0);

/// The frame of the `index`-th new window: `base` moved down and right by
/// `step` per index, wrapping back to the base after ten windows.
pub fn cascade_frame(base: Frame, index: usize, step: f64) -> Frame {
    let k = (index % 10) as f64 * step;
    Frame {
        x: base.x + k,
        y: base.y + k,
        ..base
    }
}

/// Brings a restored `frame` onto the available `screens`: the size is kept
/// (at least [`MIN_WINDOW_SIZE`], at most the screen) and the window is moved
/// so it lies fully on the screen it overlaps most (the first screen when it
/// overlaps none). Without screens the frame is returned unchanged.
pub fn clamp_to_screens(frame: Frame, screens: &[Frame]) -> Frame {
    let Some(screen) = screens
        .iter()
        .max_by(|a, b| {
            frame
                .overlap_area(a)
                .partial_cmp(&frame.overlap_area(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
    else {
        return frame;
    };
    let width = frame.width.max(MIN_WINDOW_SIZE.0).min(screen.width);
    let height = frame.height.max(MIN_WINDOW_SIZE.1).min(screen.height);
    let x = frame
        .x
        .clamp(screen.x, (screen.right() - width).max(screen.x));
    let y = frame
        .y
        .clamp(screen.y, (screen.bottom() - height).max(screen.y));
    Frame {
        x,
        y,
        width,
        height,
    }
}

// ---------------------------------------------------------------------------
// Window parts

/// A side or bottom panel: shown or not, and its size (width for side panels,
/// height for a bottom console) in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PanelState {
    /// The panel is shown.
    pub visible: bool,
    /// Its size, `None` for the default.
    pub size: Option<f32>,
}

impl PanelState {
    /// A shown panel with the default size.
    pub const fn shown() -> Self {
        Self {
            visible: true,
            size: None,
        }
    }
}

/// What the sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarMode {
    /// The file navigator.
    #[default]
    Files,
    /// Project search.
    Search,
}

/// The sidebar of one window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    /// The file navigator's root folder.
    pub root: Option<PathBuf>,
    /// Which sidebar view is selected.
    pub mode: SidebarMode,
    /// Previously visited roots, oldest first (the navigator's back list).
    pub root_history: Vec<PathBuf>,
    /// Expanded folders, to restore the outline.
    pub expanded: Vec<PathBuf>,
    /// The filter field's text.
    pub filter: String,
    /// The search view's query.
    pub search_query: String,
}

/// Panel visibility and sizes of a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chrome {
    /// The file navigator.
    pub sidebar: PanelState,
    /// The console of the selected document (each document keeps its own in
    /// [`TabState::console`]; this is the window default for new tabs).
    pub console: PanelState,
    /// Where the console sits.
    pub console_position: ConsolePosition,
    /// The documentation view.
    pub docs: PanelState,
    /// The tab bar is shown.
    pub tab_bar_visible: bool,
    /// The toolbar is shown.
    pub toolbar_visible: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            sidebar: PanelState::shown(),
            console: PanelState::default(),
            console_position: ConsolePosition::Bottom,
            docs: PanelState::default(),
            tab_bar_visible: true,
            toolbar_visible: true,
        }
    }
}

/// One tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    /// The document the tab shows.
    pub id: DocumentId,
    /// The tab's name (kept to label the tab before the document loads).
    pub title: String,
    /// The file (for an untitled document, its file in the Unsaved directory).
    pub path: Option<PathBuf>,
    /// The document is untitled.
    pub untitled: bool,
    /// The language in effect.
    pub language: String,
    /// The user chose the language.
    pub language_locked: bool,
    /// Selections, scroll and folds.
    pub view: ViewState,
    /// This tab's console panel.
    pub console: PanelState,
    /// This tab's documentation panel (`None`: follow the window).
    pub docs: Option<PanelState>,
}

impl Default for TabState {
    fn default() -> Self {
        Self {
            id: DocumentId::nil(),
            title: String::new(),
            path: None,
            untitled: false,
            language: String::new(),
            language_locked: false,
            view: ViewState::default(),
            console: PanelState::default(),
            docs: None,
        }
    }
}

impl TabState {
    /// A tab for `document` with its current view state.
    pub fn for_document(document: &crate::Document) -> Self {
        Self {
            id: document.id,
            title: document.title(),
            path: document.path.clone(),
            untitled: document.untitled,
            language: document.language.clone(),
            language_locked: document.language_locked,
            view: document.view.clone(),
            console: PanelState::default(),
            docs: None,
        }
    }
}

/// One window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// The frame; `None` lets the platform place (cascade) the window.
    pub frame: Option<Frame>,
    /// The window is in full screen.
    pub fullscreen: bool,
    /// Panels and bars.
    pub chrome: Chrome,
    /// The sidebar.
    pub sidebar: SidebarState,
    /// The tabs, in order.
    pub tabs: Vec<TabState>,
    /// The selected tab's document.
    pub active: Option<DocumentId>,
    /// The Back/Forward history of the window.
    pub history: NavigationHistory,
}

impl WindowState {
    /// The position of the selected tab (the first when `active` is unset or
    /// dangling).
    pub fn active_index(&self) -> Option<usize> {
        if self.tabs.is_empty() {
            return None;
        }
        Some(self.active.and_then(|id| self.index_of(id)).unwrap_or(0))
    }

    /// The position of the tab for `id`.
    pub fn index_of(&self, id: DocumentId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Inserts `tab` where the *Insert New Tabs* preference says and selects
    /// it. Returns its index.
    pub fn insert_tab(&mut self, tab: TabState, policy: InsertNewTabs) -> usize {
        let index = tab_insert_index(policy, self.tabs.len(), self.active_index());
        self.active = Some(tab.id);
        self.tabs.insert(index, tab);
        index
    }

    /// Removes the tab for `id` and selects the neighbour: the tab that moves
    /// into its place (the one to the right), else the new last tab. Returns
    /// the removed tab. Selection is unchanged when another tab was closed.
    pub fn remove_tab(&mut self, id: DocumentId) -> Option<TabState> {
        let index = self.index_of(id)?;
        let was_active = self.active_index() == Some(index);
        let tab = self.tabs.remove(index);
        self.history.forget_document(id);
        if was_active {
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|t| t.id);
        } else if self.active == Some(id) {
            self.active = None;
        }
        Some(tab)
    }

    /// Selects the tab for `id`; returns whether it exists.
    pub fn select(&mut self, id: DocumentId) -> bool {
        let exists = self.index_of(id).is_some();
        if exists {
            self.active = Some(id);
        }
        exists
    }

    /// Selects the tab at `index` (⌘1..⌘9 are indexes 0..8, ⌘0 is 9);
    /// returns whether it exists.
    pub fn select_index(&mut self, index: usize) -> bool {
        match self.tabs.get(index) {
            Some(tab) => {
                self.active = Some(tab.id);
                true
            }
            None => false,
        }
    }

    /// Selects the next tab (⌘}), wrapping around. Returns the new selection.
    pub fn select_next(&mut self) -> Option<DocumentId> {
        self.step_selection(1)
    }

    /// Selects the previous tab (⌘{), wrapping around.
    pub fn select_previous(&mut self) -> Option<DocumentId> {
        self.step_selection(-1)
    }

    fn step_selection(&mut self, delta: isize) -> Option<DocumentId> {
        let current = self.active_index()?;
        let count = self.tabs.len() as isize;
        let next = (current as isize + delta).rem_euclid(count) as usize;
        let id = self.tabs[next].id;
        self.active = Some(id);
        Some(id)
    }

    /// Moves the selected tab one place left (⌥⌘{); false at the edge.
    pub fn move_active_left(&mut self) -> bool {
        match self.active_index() {
            Some(i) if i > 0 => {
                self.tabs.swap(i, i - 1);
                true
            }
            _ => false,
        }
    }

    /// Moves the selected tab one place right (⌥⌘}); false at the edge.
    pub fn move_active_right(&mut self) -> bool {
        match self.active_index() {
            Some(i) if i + 1 < self.tabs.len() => {
                self.tabs.swap(i, i + 1);
                true
            }
            _ => false,
        }
    }

    /// Moves a tab to a new position (drag to reorder); `to` is the index in
    /// the list after the tab is taken out.
    pub fn move_tab(&mut self, id: DocumentId, to: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let tab = self.tabs.remove(from);
        let to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        true
    }
}

/// Where a new tab goes: after the last (`AfterExisting`), first
/// (`BeforeExisting`) or right after the selected one (`AfterSelected`).
pub fn tab_insert_index(policy: InsertNewTabs, count: usize, selected: Option<usize>) -> usize {
    match policy {
        InsertNewTabs::AfterExisting => count,
        InsertNewTabs::BeforeExisting => 0,
        InsertNewTabs::AfterSelected => selected.map_or(count, |i| (i + 1).min(count)),
    }
}

// ---------------------------------------------------------------------------
// Session

/// Everything restored at launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// The format version.
    pub version: u32,
    /// The windows, back to front is not recorded: the front window is
    /// [`Session::active_window`].
    pub windows: Vec<WindowState>,
    /// The index of the frontmost window.
    pub active_window: Option<usize>,
    /// The interpreter the user last picked for each language.
    pub runtimes: BTreeMap<String, String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            windows: Vec::new(),
            active_window: None,
            runtimes: BTreeMap::new(),
        }
    }
}

/// The shape of version 1 files (one window, flat).
#[derive(Debug, Deserialize)]
#[serde(default)]
struct SessionV1 {
    tabs: Vec<TabV1>,
    active: Option<DocumentId>,
    output_visible: bool,
    output_height: f32,
    runtimes: BTreeMap<String, String>,
}

impl Default for SessionV1 {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            output_visible: false,
            output_height: 180.0,
            runtimes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TabV1 {
    id: DocumentId,
    #[serde(default)]
    title: String,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    language: String,
    #[serde(default)]
    language_locked: bool,
    #[serde(default)]
    cursor_line: u32,
    #[serde(default)]
    cursor_column: u32,
}

impl From<SessionV1> for Session {
    fn from(old: SessionV1) -> Self {
        let tabs: Vec<TabState> = old
            .tabs
            .into_iter()
            .map(|t| {
                let mut view = ViewState::default();
                view.set_caret(crate::Position::new(t.cursor_line, t.cursor_column));
                TabState {
                    id: t.id,
                    title: t.title,
                    untitled: t.path.is_none(),
                    path: t.path,
                    language: t.language,
                    language_locked: t.language_locked,
                    view,
                    ..TabState::default()
                }
            })
            .collect();
        let window = WindowState {
            chrome: Chrome {
                console: PanelState {
                    visible: old.output_visible,
                    size: Some(old.output_height),
                },
                ..Chrome::default()
            },
            active: old.active,
            tabs,
            ..WindowState::default()
        };
        let has_window = !window.tabs.is_empty();
        Session {
            version: SESSION_VERSION,
            windows: if has_window { vec![window] } else { Vec::new() },
            active_window: has_window.then_some(0),
            runtimes: old.runtimes,
        }
    }
}

impl Session {
    /// Parses session text of any version this build knows; unknown fields
    /// are ignored, so a newer file loses nothing this build understands.
    ///
    /// # Errors
    ///
    /// The JSON error when the text is not a session at all.
    pub fn parse(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        let mut session = if version < 2 {
            Session::from(serde_json::from_value::<SessionV1>(value)?)
        } else {
            serde_json::from_value::<Session>(value)?
        };
        session.normalize();
        Ok(session)
    }

    /// Reads the session at `path`. A missing or unreadable session is the
    /// empty one; the reason is returned alongside for the caller to log.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match fs::read(path) {
            Ok(bytes) => match Self::parse(&bytes) {
                Ok(session) => (session, None),
                Err(error) => (
                    Self::default(),
                    Some(format!("session.json was ignored: {error}")),
                ),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!("session.json could not be read: {error}")),
            ),
        }
    }

    /// Writes the session to `path` atomically.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, &bytes)
    }

    /// Repairs what a damaged or hand-edited file can get wrong: current
    /// version, an `active_window` and per-window `active` that exist, no
    /// duplicate document ids across windows.
    pub fn normalize(&mut self) {
        self.version = SESSION_VERSION;
        let mut seen = std::collections::HashSet::new();
        for window in &mut self.windows {
            window.tabs.retain(|t| seen.insert(t.id));
            if window.active.is_none_or(|id| window.index_of(id).is_none()) {
                window.active = window.tabs.first().map(|t| t.id);
            }
        }
        if self.active_window.is_none_or(|i| i >= self.windows.len()) {
            self.active_window = (!self.windows.is_empty()).then_some(0);
        }
    }

    /// Every document id in the session.
    pub fn document_ids(&self) -> Vec<DocumentId> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter().map(|t| t.id))
            .collect()
    }

    /// Drops the tabs for which `is_available` is false (a file that was
    /// deleted, an untitled document whose recovery failed), and windows left
    /// without tabs *and* without a sidebar root. Returns the dropped tabs.
    pub fn prune_unavailable(&mut self, is_available: impl Fn(&TabState) -> bool) -> Vec<TabState> {
        let mut dropped = Vec::new();
        for window in &mut self.windows {
            let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut window.tabs)
                .into_iter()
                .partition(|t| is_available(t));
            window.tabs = keep;
            for tab in &gone {
                window.history.forget_document(tab.id);
            }
            dropped.extend(gone);
        }
        let frontmost = self.active_window;
        let mut kept_before_frontmost = 0usize;
        let mut frontmost_kept = false;
        let mut index = 0usize;
        self.windows.retain(|w| {
            let keep = !w.tabs.is_empty() || w.sidebar.root.is_some();
            match frontmost {
                Some(front) if index == front => frontmost_kept = keep,
                Some(front) if index < front && keep => kept_before_frontmost += 1,
                _ => {}
            }
            index += 1;
            keep
        });
        self.active_window = frontmost_kept.then_some(kept_before_frontmost);
        self.normalize();
        dropped
    }
}

// ---------------------------------------------------------------------------
// Defaults for new windows

/// `window-state.json`: what a new window starts with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowDefaults {
    /// The format version.
    pub version: u32,
    /// The frame of the window that was frontmost when the app last ran.
    pub last_frame: Option<Frame>,
    /// Panel state a new window starts with.
    pub chrome: Chrome,
    /// Cascade offset in points between new windows.
    pub cascade_step: f64,
    /// How many windows were opened since launch is not stored; this is the
    /// size new windows get when no frame is known.
    pub default_size: (f64, f64),
}

impl Default for WindowDefaults {
    fn default() -> Self {
        Self {
            version: 1,
            last_frame: None,
            chrome: Chrome::default(),
            cascade_step: 22.0,
            default_size: (1100.0, 600.0),
        }
    }
}

impl WindowDefaults {
    /// Reads `path`; a missing or damaged file gives the defaults and, for a
    /// damaged one, a warning.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => (value, None),
                Err(error) => (
                    Self::default(),
                    Some(format!("window-state.json was ignored: {error}")),
                ),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!("window-state.json could not be read: {error}")),
            ),
        }
    }

    /// Writes `path` atomically.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(
            path,
            &serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )
    }

    /// The frame for the `index`-th window opened without a saved frame:
    /// the last frame (or a default-sized one at `origin`) cascaded.
    pub fn new_window_frame(&self, index: usize, origin: (f64, f64)) -> Frame {
        let base = self.last_frame.unwrap_or(Frame::new(
            origin.0,
            origin.1,
            self.default_size.0,
            self.default_size.1,
        ));
        cascade_frame(base, index, self.cascade_step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str) -> TabState {
        TabState {
            id: DocumentId::new(),
            title: title.into(),
            ..TabState::default()
        }
    }

    fn window(titles: &[&str]) -> WindowState {
        let mut w = WindowState::default();
        for t in titles {
            w.insert_tab(tab(t), InsertNewTabs::AfterExisting);
        }
        w
    }

    fn titles(w: &WindowState) -> Vec<&str> {
        w.tabs.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn round_trips_windows_and_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let mut a = window(&["a.py", "b.py"]);
        a.frame = Some(Frame::new(10.0, 20.0, 900.0, 700.0));
        a.chrome.console = PanelState {
            visible: true,
            size: Some(240.0),
        };
        a.chrome.console_position = ConsolePosition::Right;
        a.sidebar.root = Some(PathBuf::from("/work"));
        a.sidebar.mode = SidebarMode::Search;
        a.tabs[0].view.set_caret(crate::Position::new(4, 2));
        let b = window(&["c.go"]);
        let session = Session {
            windows: vec![a, b],
            active_window: Some(1),
            runtimes: BTreeMap::from([("python".to_owned(), "/opt/venv/bin/python".to_owned())]),
            ..Session::default()
        };
        session.save(&path).unwrap();
        let (loaded, warning) = Session::load(&path);
        assert_eq!(warning, None);
        assert_eq!(loaded, session);
    }

    #[test]
    fn migrates_version_one() {
        let id = DocumentId::new();
        let v1 = format!(
            r#"{{"version":1,"tabs":[{{"id":"{id}","title":"Untitled-1","path":null,"language":"shell","language_locked":true,"cursor_line":4,"cursor_column":2}}],"active":"{id}","output_visible":true,"output_height":210.0,"runtimes":{{"python":"/p"}}}}"#
        );
        let session = Session::parse(v1.as_bytes()).unwrap();
        assert_eq!(session.version, SESSION_VERSION);
        assert_eq!(session.windows.len(), 1);
        let window = &session.windows[0];
        assert_eq!(window.tabs[0].language, "shell");
        assert!(window.tabs[0].untitled && window.tabs[0].language_locked);
        assert_eq!(window.tabs[0].view.caret(), crate::Position::new(4, 2));
        assert_eq!(
            window.chrome.console,
            PanelState {
                visible: true,
                size: Some(210.0)
            }
        );
        assert_eq!(window.active, Some(id));
        assert_eq!(session.runtimes["python"], "/p");
        assert_eq!(session.active_window, Some(0));
    }

    #[test]
    fn empty_version_one_has_no_windows() {
        let session = Session::parse(br#"{"version":1,"tabs":[]}"#).unwrap();
        assert!(session.windows.is_empty() && session.active_window.is_none());
    }

    #[test]
    fn unknown_fields_and_future_versions_are_tolerated() {
        let json = r#"{"version":9,"windows":[{"tabs":[],"future":true}],"extra":1}"#;
        let session = Session::parse(json.as_bytes()).unwrap();
        assert_eq!(session.windows.len(), 1);
        assert_eq!(session.version, SESSION_VERSION);
    }

    #[test]
    fn unreadable_and_missing_sessions_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let (session, warning) = Session::load(&path);
        assert_eq!((session, warning), (Session::default(), None));
        fs::write(&path, b"not json").unwrap();
        let (session, warning) = Session::load(&path);
        assert_eq!(session, Session::default());
        assert!(warning.unwrap().contains("session.json was ignored"));
    }

    #[test]
    fn normalize_repairs_dangling_selection_and_duplicate_documents() {
        let mut w1 = window(&["a", "b"]);
        let dup = w1.tabs[0].clone();
        let mut w2 = window(&["c"]);
        w2.tabs.push(dup); // same document in two windows
        w1.active = Some(DocumentId::new()); // dangling
        let mut session = Session {
            windows: vec![w1, w2],
            active_window: Some(7),
            ..Session::default()
        };
        session.normalize();
        assert_eq!(session.windows[1].tabs.len(), 1, "duplicate dropped");
        assert_eq!(
            session.windows[0].active,
            Some(session.windows[0].tabs[0].id)
        );
        assert_eq!(session.active_window, Some(0));
    }

    #[test]
    fn prune_drops_unavailable_tabs_and_empty_windows() {
        let a = window(&["keep.py", "gone.py"]);
        let b = window(&["gone2.py"]);
        let mut c = window(&["gone3.py"]);
        c.sidebar.root = Some(PathBuf::from("/proj"));
        let mut session = Session {
            windows: vec![a, b, c],
            active_window: Some(1),
            ..Session::default()
        };
        let dropped = session.prune_unavailable(|t| t.title.starts_with("keep"));
        assert_eq!(dropped.len(), 3);
        assert_eq!(
            session.windows.len(),
            2,
            "the window without tabs or sidebar root is gone"
        );
        assert_eq!(titles(&session.windows[0]), ["keep.py"]);
        assert!(session.windows[1].tabs.is_empty() && session.windows[1].sidebar.root.is_some());
        assert_eq!(session.active_window, Some(0));
    }

    #[test]
    fn insert_policy_places_new_tabs() {
        assert_eq!(
            tab_insert_index(InsertNewTabs::AfterExisting, 3, Some(0)),
            3
        );
        assert_eq!(
            tab_insert_index(InsertNewTabs::BeforeExisting, 3, Some(2)),
            0
        );
        assert_eq!(
            tab_insert_index(InsertNewTabs::AfterSelected, 3, Some(0)),
            1
        );
        assert_eq!(
            tab_insert_index(InsertNewTabs::AfterSelected, 3, Some(2)),
            3
        );
        assert_eq!(tab_insert_index(InsertNewTabs::AfterSelected, 0, None), 0);

        let mut w = window(&["a", "b", "c"]);
        w.select_index(0);
        w.insert_tab(tab("x"), InsertNewTabs::AfterSelected);
        assert_eq!(titles(&w), ["a", "x", "b", "c"]);
        assert_eq!(w.active, Some(w.tabs[1].id), "the new tab is selected");
        w.insert_tab(tab("y"), InsertNewTabs::BeforeExisting);
        assert_eq!(titles(&w), ["y", "a", "x", "b", "c"]);
    }

    #[test]
    fn closing_selects_the_neighbour() {
        let mut w = window(&["a", "b", "c"]);
        let ids: Vec<_> = w.tabs.iter().map(|t| t.id).collect();
        w.select(ids[1]);
        w.remove_tab(ids[1]);
        assert_eq!(w.active, Some(ids[2]), "the tab to the right takes over");
        w.remove_tab(ids[2]);
        assert_eq!(
            w.active,
            Some(ids[0]),
            "the last tab when there is none to the right"
        );
        // closing another tab keeps the selection
        let mut w = window(&["a", "b", "c"]);
        let ids: Vec<_> = w.tabs.iter().map(|t| t.id).collect();
        w.select(ids[0]);
        w.remove_tab(ids[2]);
        assert_eq!(w.active, Some(ids[0]));
        w.remove_tab(ids[0]);
        w.remove_tab(ids[1]);
        assert_eq!(w.active, None);
        assert_eq!(w.active_index(), None);
    }

    #[test]
    fn next_previous_wrap_and_numbered_selection() {
        let mut w = window(&["a", "b", "c"]);
        let ids: Vec<_> = w.tabs.iter().map(|t| t.id).collect();
        w.select(ids[2]);
        assert_eq!(w.select_next(), Some(ids[0]));
        assert_eq!(w.select_previous(), Some(ids[2]));
        assert!(w.select_index(1) && w.active == Some(ids[1]));
        assert!(!w.select_index(9));
        assert_eq!(WindowState::default().select_next(), None);
    }

    #[test]
    fn moving_tabs() {
        let mut w = window(&["a", "b", "c"]);
        let ids: Vec<_> = w.tabs.iter().map(|t| t.id).collect();
        w.select(ids[0]);
        assert!(!w.move_active_left());
        assert!(w.move_active_right());
        assert_eq!(titles(&w), ["b", "a", "c"]);
        assert!(w.move_tab(ids[2], 0));
        assert_eq!(titles(&w), ["c", "b", "a"]);
        assert!(w.move_tab(ids[2], 99));
        assert_eq!(titles(&w), ["b", "a", "c"]);
        assert!(!w.move_tab(DocumentId::new(), 0));
    }

    #[test]
    fn cascading_wraps() {
        let base = Frame::new(100.0, 100.0, 800.0, 600.0);
        assert_eq!(cascade_frame(base, 0, 22.0), base);
        assert_eq!(
            cascade_frame(base, 2, 22.0),
            Frame::new(144.0, 144.0, 800.0, 600.0)
        );
        assert_eq!(cascade_frame(base, 10, 22.0), base);
    }

    #[test]
    fn clamping_keeps_windows_on_screen() {
        let main = Frame::new(0.0, 0.0, 1440.0, 900.0);
        let second = Frame::new(1440.0, 0.0, 1920.0, 1080.0);
        // fully inside: unchanged
        let inside = Frame::new(100.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_to_screens(inside, &[main, second]), inside);
        // hanging off the right edge of the main screen
        let off = Frame::new(1300.0, 100.0, 800.0, 600.0);
        let clamped = clamp_to_screens(off, &[main]);
        assert_eq!((clamped.x, clamped.right()), (640.0, 1440.0));
        // on a screen that no longer exists: lands on the first one
        let lost = Frame::new(5000.0, 3000.0, 800.0, 600.0);
        let moved = clamp_to_screens(lost, &[main]);
        assert!(moved.x >= 0.0 && moved.right() <= 1440.0 && moved.bottom() <= 900.0);
        // too big: shrunk to the screen, never below the minimum
        let huge = Frame::new(0.0, 0.0, 5000.0, 4000.0);
        let shrunk = clamp_to_screens(huge, &[main]);
        assert_eq!((shrunk.width, shrunk.height), (1440.0, 900.0));
        let tiny = clamp_to_screens(Frame::new(0.0, 0.0, 10.0, 10.0), &[main]);
        assert_eq!((tiny.width, tiny.height), MIN_WINDOW_SIZE);
        assert_eq!(clamp_to_screens(inside, &[]), inside);
        // picks the screen with the most overlap
        let straddle = Frame::new(1300.0, 100.0, 800.0, 600.0);
        let on_second = clamp_to_screens(straddle, &[main, second]);
        assert!(on_second.x >= 1440.0 || on_second.right() <= 1440.0);
    }

    #[test]
    fn window_defaults_round_trip_and_cascade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window-state.json");
        let mut defaults = WindowDefaults::default();
        assert_eq!(
            defaults.new_window_frame(0, (50.0, 60.0)),
            Frame::new(50.0, 60.0, 1100.0, 600.0)
        );
        defaults.last_frame = Some(Frame::new(10.0, 10.0, 824.0, 636.0));
        assert_eq!(defaults.new_window_frame(1, (0.0, 0.0)).x, 32.0);
        defaults.save(&path).unwrap();
        let (loaded, warning) = WindowDefaults::load(&path);
        assert_eq!((loaded, warning), (defaults, None));
        fs::write(&path, b"{").unwrap();
        let (fallback, warning) = WindowDefaults::load(&path);
        assert_eq!(fallback, WindowDefaults::default());
        assert!(warning.is_some());
    }
}
