//! [`CommandContext`]: the pure snapshot of application state that decides whether a
//! command is enabled, checked, visible and how its title reads.
//!
//! The UI fills one in (cheaply, from its signals) whenever a menu is about to open or
//! a palette is built, then asks [`crate::CommandDef::state`] / [`crate::MenuModel::resolve`].

use std::borrow::Cow;

/// Where the console pane is docked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsolePosition {
    /// Below the editor.
    #[default]
    Bottom,
    /// To the right of the editor.
    Right,
}

/// What Go Back / Go Forward would navigate to; selects the dynamic menu caption.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NavTarget {
    /// Nothing: the item is disabled and reads plain "Go Back" / "Go Forward".
    #[default]
    None,
    /// An earlier / later caret location in the same document.
    CaretLocation,
    /// The search results.
    SearchResults,
    /// Another tab or document, by name.
    Document(String),
}

/// How the Find bar matches text (Find > Find Mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FindMode {
    /// Contains the text (`NSTextFinderMatchingTypeContains`).
    #[default]
    Text,
    /// Regular expression.
    RegularExpression,
    /// Full word.
    Word,
    /// Word prefix.
    WordPrefix,
    /// Word suffix.
    WordSuffix,
    /// Ignoring whitespace.
    IgnoringWhitespace,
}

/// When diagnostics refresh for the current language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiagnosticsUpdate {
    /// Only on request.
    Manually,
    /// While editing.
    OnEdit,
    /// When the document is run.
    #[default]
    OnRun,
}

/// Everything menu/command state depends on. Plain data; build with `..Default::default()`.
///
/// `Default` is the state of an app with one empty document window in a quiet state
/// (see [`CommandContext::new`]); all flags false unless documented.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandContext {
    // ---- windows, tabs, documents -------------------------------------------------
    /// A document window exists and is key.
    pub has_window: bool,
    /// The key window shows a document.
    pub has_document: bool,
    /// Number of open document windows.
    pub window_count: usize,
    /// Tabs in the key window.
    pub tab_count: usize,
    /// The in-window tab bar is hidden.
    pub tab_bar_hidden: bool,
    /// New documents open as tabs rather than windows ("prefer tabs").
    pub prefer_tabs: bool,
    /// The document has never been saved (no file).
    pub is_untitled: bool,
    /// The document has unsaved changes.
    pub is_dirty: bool,
    /// The window is browsing document versions: most commands are disabled.
    pub version_browsing: bool,
    /// Display name of the current document (for `Run "name"` etc.).
    pub document_name: Option<String>,
    /// Autosave & Versions is on: File shows the "Revert To" submenu instead of "Revert to Saved".
    pub autosave_versions: bool,
    /// Open Recent has entries.
    pub has_recent_documents: bool,

    // ---- editor -------------------------------------------------------------------
    /// The editor view has keyboard focus.
    pub editor_focused: bool,
    /// The document can be edited.
    pub is_editable: bool,
    /// The selection is not empty.
    pub has_selection: bool,
    /// More than one selection exists.
    pub has_multiple_selections: bool,
    /// Undo is possible.
    pub can_undo: bool,
    /// Redo is possible.
    pub can_redo: bool,
    /// Action name of the next undo ("Typing"), for "Undo Typing".
    pub undo_action_name: Option<String>,
    /// Action name of the next redo.
    pub redo_action_name: Option<String>,
    /// The pasteboard holds text.
    pub can_paste: bool,
    /// The word under the caret or selection (for `Look up "word" in the docs`).
    pub selected_word: Option<String>,
    /// The word at the caret has token highlighting (enables `Edit "word"`).
    pub has_highlighted_token: bool,

    // ---- find ---------------------------------------------------------------------
    /// Current find matching mode.
    pub find_mode: FindMode,
    /// Find ignores case.
    pub find_ignore_case: bool,
    /// Find wraps around.
    pub find_wrap_around: bool,

    // ---- run / debug --------------------------------------------------------------
    /// The language/document can be run.
    pub run_enabled: bool,
    /// A process started by Run is alive.
    pub is_running: bool,
    /// The language compiles instead of running (`Compile` title for Run Diagnostics...).
    pub compile_only: bool,
    /// The language has a debugger configured.
    pub has_debugger: bool,
    /// A debug session is active.
    pub is_debugging: bool,
    /// The debuggee is paused.
    pub debugger_paused: bool,
    /// The document has breakpoints.
    pub has_breakpoints: bool,
    /// The run target differs from this document (enables the alternate `Run "doc"`).
    pub run_target_is_other: bool,
    /// The document has been run before (`Run Again`).
    pub has_run_before: bool,
    /// The language has per-language run settings to edit.
    pub can_edit_run_settings: bool,

    // ---- diagnostics --------------------------------------------------------------
    /// Diagnostics can be run for this document.
    pub can_lint: bool,
    /// There is a fix to apply.
    pub can_apply_fix: bool,
    /// There is a next diagnostic.
    pub can_jump_next_diagnostic: bool,
    /// There is a previous diagnostic.
    pub can_jump_previous_diagnostic: bool,
    /// There is a next error.
    pub can_jump_next_error: bool,
    /// There is a previous error.
    pub can_jump_previous_error: bool,
    /// Diagnostics are enabled for the document's language.
    pub diagnostics_enabled: bool,
    /// Current update mode.
    pub diagnostics_update: DiagnosticsUpdate,

    // ---- view ---------------------------------------------------------------------
    /// File navigator (left sidebar) is visible.
    pub sidebar_visible: bool,
    /// Console is visible.
    pub console_visible: bool,
    /// Documentation view (right pane) is visible.
    pub docs_visible: bool,
    /// Toolbar is visible.
    pub toolbar_visible: bool,
    /// The window is full screen.
    pub is_full_screen: bool,
    /// Console docking position.
    pub console_position: ConsolePosition,
    /// Preference: hide the console automatically.
    pub hide_console_automatically: bool,
    /// Editor option: show invisible characters.
    pub show_invisibles: bool,
    /// Editor option: show line numbers.
    pub show_line_numbers: bool,
    /// Editor option: show the page guide.
    pub show_page_guide: bool,
    /// Editor option: wrap lines.
    pub wrap_lines: bool,
    /// Editor option: allow scrolling past the end.
    pub allow_scroll_past_end: bool,
    /// Editor option: highlight the current line.
    pub highlight_current_line: bool,
    /// Spelling: check while typing.
    pub spelling_check_while_typing: bool,
    /// Spelling: check grammar with spelling.
    pub spelling_check_grammar: bool,
    /// Spelling: correct automatically.
    pub spelling_correct_automatically: bool,

    // ---- navigation ---------------------------------------------------------------
    /// What Go Back goes to.
    pub go_back: NavTarget,
    /// What Go Forward goes to.
    pub go_forward: NavTarget,

    // ---- misc ---------------------------------------------------------------------
    /// The current document's language name.
    pub language_name: Option<String>,
    /// A sidebar file/folder is selected (file navigator context commands).
    pub navigator_has_selection: bool,
}

impl CommandContext {
    /// A typical context: one window with one editable document, editor focused.
    pub fn new() -> CommandContext {
        CommandContext {
            has_window: true,
            has_document: true,
            window_count: 1,
            tab_count: 1,
            autosave_versions: true,
            is_editable: true,
            editor_focused: true,
            sidebar_visible: true,
            toolbar_visible: true,
            show_line_numbers: true,
            wrap_lines: true,
            allow_scroll_past_end: true,
            highlight_current_line: true,
            show_invisibles: true,
            hide_console_automatically: true,
            find_wrap_around: true,
            find_ignore_case: true,
            run_enabled: true,
            ..CommandContext::default()
        }
    }

    /// `name` as `Prefix "doc"` when a document name is known, else `Prefix` + `fallback`.
    pub fn with_doc_name<'a>(&'a self, prefix: &str, suffix: &str, fallback: &'a str) -> Cow<'a, str> {
        match &self.document_name {
            Some(n) => Cow::Owned(format!("{prefix}\"{n}\"{suffix}")),
            None => Cow::Borrowed(fallback),
        }
    }
}
