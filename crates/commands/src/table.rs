//! The static command table: every command of the ScratchRunner menu bar, the editor command
//! vocabulary, window commands and context-menu-only commands.
//!
//! Shortcut strings are CodeRunner key equivalents (`@` Cmd, `~` Option, `^` Control, `$` Shift,
//! upper-case letter = Shift, U+F700.. function keys); see [`crate::shortcut`]. The menu
//! structure lives in [`crate::menu`], which refers to the ids defined here.

use std::borrow::Cow;

use crate::command::{CommandDef, CommandKind, Role, Scope};
use crate::context::{CommandContext as C, ConsolePosition, DiagnosticsUpdate, FindMode, NavTarget};

/// The product name used in titles.
pub const APP_NAME: &str = "ScratchRunner";

// ---- predicates -------------------------------------------------------------------------

/// Reusable enable / check predicates.
mod p {
    use super::*;

    pub fn window(c: &C) -> bool {
        c.has_window
    }
    /// A document exists and the window is not browsing versions.
    pub fn doc(c: &C) -> bool {
        c.has_document && !c.version_browsing
    }
    pub fn live(c: &C) -> bool {
        !c.version_browsing
    }
    /// Editor commands that need keyboard focus in an editable document.
    pub fn editing(c: &C) -> bool {
        doc(c) && c.is_editable && c.editor_focused
    }
    /// Editor commands that need keyboard focus in the editor.
    pub fn editor(c: &C) -> bool {
        doc(c) && c.editor_focused
    }
    pub fn selection(c: &C) -> bool {
        doc(c) && c.has_selection
    }
    pub fn multi_tab(c: &C) -> bool {
        c.has_window && c.tab_count > 1
    }
    pub fn can_undo(c: &C) -> bool {
        c.can_undo
    }
    pub fn can_redo(c: &C) -> bool {
        c.can_redo
    }
    pub fn paste(c: &C) -> bool {
        c.can_paste && c.is_editable && doc(c)
    }
    pub fn delete(c: &C) -> bool {
        selection(c) && c.is_editable
    }
    pub fn revert(c: &C) -> bool {
        doc(c) && !c.is_untitled && c.is_dirty
    }
    pub fn run(c: &C) -> bool {
        doc(c) && c.run_enabled
    }
    pub fn run_debug(c: &C) -> bool {
        run(c) && c.has_debugger
    }
    pub fn run_alt(c: &C) -> bool {
        run(c) && c.run_target_is_other
    }
    pub fn stop(c: &C) -> bool {
        c.is_running || c.is_debugging
    }
    pub fn lint(c: &C) -> bool {
        doc(c) && c.can_lint
    }
    pub fn debugging(c: &C) -> bool {
        c.is_debugging
    }
    pub fn stepping(c: &C) -> bool {
        c.is_debugging && c.debugger_paused
    }
    pub fn breakpoints(c: &C) -> bool {
        doc(c) && c.has_breakpoints
    }
    pub fn apply_fix(c: &C) -> bool {
        doc(c) && c.can_apply_fix
    }
    pub fn next_diag(c: &C) -> bool {
        doc(c) && c.can_jump_next_diagnostic
    }
    pub fn prev_diag(c: &C) -> bool {
        doc(c) && c.can_jump_previous_diagnostic
    }
    pub fn next_err(c: &C) -> bool {
        doc(c) && c.can_jump_next_error
    }
    pub fn prev_err(c: &C) -> bool {
        doc(c) && c.can_jump_previous_error
    }
    pub fn merge(c: &C) -> bool {
        c.window_count >= 2
    }
    pub fn go_back(c: &C) -> bool {
        c.go_back != NavTarget::None
    }
    pub fn go_forward(c: &C) -> bool {
        c.go_forward != NavTarget::None
    }
    pub fn token(c: &C) -> bool {
        doc(c) && c.has_highlighted_token
    }
    pub fn nav_selection(c: &C) -> bool {
        c.navigator_has_selection
    }
    pub fn has_recent(c: &C) -> bool {
        c.has_recent_documents
    }
    pub fn no_versions(c: &C) -> bool {
        !c.autosave_versions
    }
    pub fn clear_visible(c: &C) -> bool {
        // The ⇧⌘W companion is redundant while ⌘W already reads "Close Window".
        c.tab_count > 1 || c.tab_bar_hidden
    }
    // ---- check marks
    pub fn mode_text(c: &C) -> bool {
        c.find_mode == FindMode::Text
    }
    pub fn mode_regex(c: &C) -> bool {
        c.find_mode == FindMode::RegularExpression
    }
    pub fn mode_word(c: &C) -> bool {
        c.find_mode == FindMode::Word
    }
    pub fn mode_prefix(c: &C) -> bool {
        c.find_mode == FindMode::WordPrefix
    }
    pub fn mode_suffix(c: &C) -> bool {
        c.find_mode == FindMode::WordSuffix
    }
    pub fn mode_ws(c: &C) -> bool {
        c.find_mode == FindMode::IgnoringWhitespace
    }
    pub fn ignore_case(c: &C) -> bool {
        c.find_ignore_case
    }
    pub fn wrap_around(c: &C) -> bool {
        c.find_wrap_around
    }
    pub fn show_invisibles(c: &C) -> bool {
        c.show_invisibles
    }
    pub fn show_line_numbers(c: &C) -> bool {
        c.show_line_numbers
    }
    pub fn show_page_guide(c: &C) -> bool {
        c.show_page_guide
    }
    pub fn wrap_lines(c: &C) -> bool {
        c.wrap_lines
    }
    pub fn scroll_past_end(c: &C) -> bool {
        c.allow_scroll_past_end
    }
    pub fn highlight_line(c: &C) -> bool {
        c.highlight_current_line
    }
    pub fn hide_console_auto(c: &C) -> bool {
        c.hide_console_automatically
    }
    pub fn console_bottom(c: &C) -> bool {
        c.console_position == ConsolePosition::Bottom
    }
    pub fn console_right(c: &C) -> bool {
        c.console_position == ConsolePosition::Right
    }
    pub fn spell_typing(c: &C) -> bool {
        c.spelling_check_while_typing
    }
    pub fn spell_grammar(c: &C) -> bool {
        c.spelling_check_grammar
    }
    pub fn spell_auto(c: &C) -> bool {
        c.spelling_correct_automatically
    }
    pub fn diag_enabled(c: &C) -> bool {
        c.diagnostics_enabled
    }
    pub fn upd_manual(c: &C) -> bool {
        c.diagnostics_update == DiagnosticsUpdate::Manually
    }
    pub fn upd_edit(c: &C) -> bool {
        c.diagnostics_update == DiagnosticsUpdate::OnEdit
    }
    pub fn upd_run(c: &C) -> bool {
        c.diagnostics_update == DiagnosticsUpdate::OnRun
    }
}

// ---- dynamic titles ---------------------------------------------------------------------

/// Dynamic title functions.
mod t {
    use super::*;

    fn lit(s: &'static str) -> Cow<'static, str> {
        Cow::Borrowed(s)
    }

    pub fn new_from_clipboard(c: &C) -> Cow<'_, str> {
        lit(if c.prefer_tabs { "New Tab from Clipboard" } else { "New Window from Clipboard" })
    }
    pub fn new_from_clipboard_alt(c: &C) -> Cow<'_, str> {
        lit(if c.prefer_tabs { "New Window from Clipboard" } else { "New Tab from Clipboard" })
    }
    /// ⌘W: Close Tab (several tabs) / Close Document (tab bar hidden) / Close Window.
    pub fn close(c: &C) -> Cow<'_, str> {
        lit(if c.tab_count > 1 {
            "Close Tab"
        } else if c.tab_bar_hidden {
            "Close Document"
        } else {
            "Close Window"
        })
    }
    pub fn close_without_saving(c: &C) -> Cow<'_, str> {
        lit(if c.tab_count <= 1 && c.tab_bar_hidden { "Close Document Without Saving" } else { "Close Tab Without Saving" })
    }
    pub fn undo(c: &C) -> Cow<'_, str> {
        match &c.undo_action_name {
            Some(n) if !n.is_empty() => Cow::Owned(format!("Undo {n}")),
            _ => lit("Undo"),
        }
    }
    pub fn redo(c: &C) -> Cow<'_, str> {
        match &c.redo_action_name {
            Some(n) if !n.is_empty() => Cow::Owned(format!("Redo {n}")),
            _ => lit("Redo"),
        }
    }
    pub fn search_selected_in_document(c: &C) -> Cow<'_, str> {
        match &c.document_name {
            Some(n) if c.editor_focused => Cow::Owned(format!("Search for Selected Text in \"{n}\"")),
            _ => lit("Search for Selected Text in Current Document"),
        }
    }
    pub fn look_up(c: &C) -> Cow<'_, str> {
        match &c.selected_word {
            Some(w) if !w.is_empty() => Cow::Owned(format!("Look up \"{w}\" in the docs")),
            _ => lit("Look Up Current Symbol"),
        }
    }
    pub fn run_this_document(c: &C) -> Cow<'_, str> {
        let prefix = if c.has_run_before { "Run Again " } else { "Run " };
        match &c.document_name {
            Some(n) => Cow::Owned(format!("{prefix}\"{n}\"")),
            None => lit(if c.has_run_before { "Run Again" } else { "Run" }),
        }
    }
    /// Run Diagnostics (⌘B) reads "Compile" for compile-script languages.
    pub fn build(c: &C) -> Cow<'_, str> {
        lit(if c.compile_only { "Compile" } else { "Run Diagnostics" })
    }
    pub fn pause(c: &C) -> Cow<'_, str> {
        lit(if c.debugger_paused { "Continue Execution" } else { "Pause Execution" })
    }
    pub fn ignored_for_document(c: &C) -> Cow<'_, str> {
        match &c.document_name {
            Some(n) => Cow::Owned(format!("Ignored Messages For \"{n}\"…")),
            None => lit("Ignored Messages For Document…"),
        }
    }
    pub fn enable_diagnostics(c: &C) -> Cow<'_, str> {
        match &c.language_name {
            Some(n) => Cow::Owned(format!("Enable Diagnostics For \"{n}\"")),
            None => lit("Enable Diagnostics For Language"),
        }
    }
    pub fn disable_diagnostics_doc(c: &C) -> Cow<'_, str> {
        match &c.document_name {
            Some(n) => Cow::Owned(format!("Disable Diagnostics For Document \"{n}\"")),
            None => lit("Disable Diagnostics For Document"),
        }
    }
    pub fn toggle_tab_bar(c: &C) -> Cow<'_, str> {
        lit(if c.tab_bar_hidden { "Show Tab Bar" } else { "Hide Tab Bar" })
    }
    pub fn toggle_sidebar(c: &C) -> Cow<'_, str> {
        lit(if c.sidebar_visible { "Hide File Navigator" } else { "Show File Navigator" })
    }
    pub fn toggle_console(c: &C) -> Cow<'_, str> {
        lit(if c.console_visible { "Hide Console" } else { "Show Console" })
    }
    pub fn toggle_docs(c: &C) -> Cow<'_, str> {
        lit(if c.docs_visible { "Hide Documentation View" } else { "Show Documentation View" })
    }
    pub fn toggle_toolbar(c: &C) -> Cow<'_, str> {
        lit(if c.toolbar_visible { "Hide Toolbar" } else { "Show Toolbar" })
    }
    pub fn full_screen(c: &C) -> Cow<'_, str> {
        lit(if c.is_full_screen { "Exit Full Screen" } else { "Enter Full Screen" })
    }
    pub fn go_back(c: &C) -> Cow<'_, str> {
        match &c.go_back {
            NavTarget::None => lit("Go Back"),
            NavTarget::CaretLocation => lit("Go Back to Previous Caret Location"),
            NavTarget::SearchResults => lit("Go Back to Search Results"),
            NavTarget::Document(n) => Cow::Owned(format!("Go Back to Tab or Document \"{n}\"")),
        }
    }
    pub fn go_forward(c: &C) -> Cow<'_, str> {
        match &c.go_forward {
            NavTarget::None => lit("Go Forward"),
            NavTarget::CaretLocation => lit("Go Forward to Next Caret Location"),
            NavTarget::SearchResults => lit("Go Forward to Search Results"),
            NavTarget::Document(n) => Cow::Owned(format!("Go Forward to Tab or Document \"{n}\"")),
        }
    }
    pub fn edit_token(c: &C) -> Cow<'_, str> {
        match &c.selected_word {
            Some(w) if !w.is_empty() => Cow::Owned(format!("Edit \"{w}\"")),
            _ => lit("Edit Token"),
        }
    }
}

// ---- the menu commands ------------------------------------------------------------------

macro_rules! commands {
    ($( $konst:ident : $id:literal , $title:literal $( . $m:ident ( $($a:expr),* ) )* ; )*) => {
        /// Constants for every static command id.
        pub mod ids {
            use crate::command::CommandId;
            $( #[doc = $id] pub const $konst: CommandId = CommandId::from_static($id); )*
        }
        static MENU_DEFS: &[CommandDef] = &[ $( CommandDef::new($id, $title) $( .$m($($a),*) )* ),* ];
    };
}

commands! {
    // ---- ScratchRunner (application) menu
    APP_ABOUT: "app.about", "About ScratchRunner".role(Role::About);
    APP_CHECK_UPDATES: "app.checkForUpdates", "Check for Updates…";
    APP_PREFERENCES: "app.preferences", "Preferences…".key("@,");
    APP_HIDE: "app.hide", "Hide ScratchRunner".key("@h").role(Role::Hide);
    APP_HIDE_OTHERS: "app.hideOthers", "Hide Others".key("~@h").role(Role::HideOthers);
    APP_SHOW_ALL: "app.showAll", "Show All";
    APP_QUIT: "app.quit", "Quit ScratchRunner".key("@q").role(Role::Quit);

    // ---- File
    FILE_NEW_WINDOW: "file.newWindow", "New Window".key("@n").enabled(p::live);
    FILE_NEW_TAB: "file.newTab", "New Tab".key("@t");
    FILE_NEW_FROM_CLIPBOARD: "file.newFromClipboard", "New Tab from Clipboard".key("^@v").title_fn(t::new_from_clipboard);
    FILE_NEW_FROM_CLIPBOARD_ALT: "file.newFromClipboardAlt", "New Window from Clipboard".key("^@V").title_fn(t::new_from_clipboard_alt);
    FILE_OPEN: "file.open", "Open…".key("@o");
    FILE_CLEAR_RECENT: "file.clearRecent", "Clear Menu".enabled(p::has_recent);
    FILE_OPEN_QUICKLY: "file.openQuickly", "Open Quickly…".key("@O");
    FILE_GO_TO_CURRENT_DOCUMENT: "file.goToCurrentDocument", "Go to Current Document".key("~@j").enabled(p::doc);
    FILE_CLOSE: "file.close", "Close Window".key("@w").title_fn(t::close).enabled(p::window);
    FILE_CLOSE_WINDOW: "file.closeWindow", "Close Window".key("@W").enabled(p::window).visible(p::clear_visible);
    FILE_CLOSE_OTHER_TABS: "file.closeOtherTabs", "Close Other Tabs".key("~@w").enabled(p::multi_tab);
    FILE_CLOSE_WITHOUT_SAVING: "file.closeTabWithoutSaving", "Close Tab Without Saving".key("^~@w").title_fn(t::close_without_saving).enabled(p::doc);
    FILE_SAVE: "file.save", "Save".key("@s").enabled(p::doc);
    FILE_DUPLICATE: "file.duplicate", "Duplicate".enabled(p::doc);
    FILE_RENAME: "file.rename", "Rename…".enabled(p::doc);
    FILE_MOVE_TO: "file.moveTo", "Move To…".enabled(p::doc);
    FILE_SAVE_AS: "file.saveAs", "Save As…".key("$@S").enabled(p::doc);
    FILE_REVERT: "file.revert", "Revert to Saved".enabled(p::revert).visible(p::no_versions);
    FILE_REVERT_LAST_SAVED: "file.revertToLastSaved", "Last Saved Version".enabled(p::revert).hidden_in_editor();
    FILE_BROWSE_VERSIONS: "file.browseVersions", "Browse All Versions…".enabled(p::doc).hidden_in_editor();
    FILE_PAGE_SETUP: "file.pageSetup", "Page Setup…".key("$@P").enabled(p::doc);
    FILE_PRINT: "file.print", "Print…".key("@p").enabled(p::doc);

    // ---- Edit
    EDIT_UNDO: "edit.undo", "Undo".key("@z").role(Role::Undo).name("undo").title_fn(t::undo).enabled(p::can_undo);
    EDIT_REDO: "edit.redo", "Redo".key("$@Z").role(Role::Redo).name("redo").title_fn(t::redo).enabled(p::can_redo);
    EDIT_CUT: "edit.cut", "Cut".key("@x").role(Role::Cut).name("cut").enabled(p::delete);
    EDIT_COPY: "edit.copy", "Copy".key("@c").role(Role::Copy).name("copy").enabled(p::selection);
    EDIT_COPY_WITH_COLORS: "edit.copyWithColors", "Copy with Colors".key("~@c").enabled(p::selection);
    EDIT_PASTE: "edit.paste", "Paste".key("@v").role(Role::Paste).name("paste").enabled(p::paste);
    EDIT_PASTE_PRESERVE: "edit.pasteAndPreserveFormatting", "Paste and Preserve Formatting".key("~@V").name("pasteAndPreserveFormatting").enabled(p::paste);
    EDIT_DELETE: "edit.delete", "Delete".name("delete").enabled(p::delete);
    EDIT_SELECT_ALL: "edit.selectAll", "Select All".key("@a").role(Role::SelectAll).name("selectAll").enabled(p::doc);
    EDIT_SPELL_PANEL: "edit.spelling.showPanel", "Show Spelling and Grammar".key("@:").enabled(p::editing);
    EDIT_SPELL_CHECK_NOW: "edit.spelling.checkNow", "Check Document Now".key("@;").enabled(p::editing);
    EDIT_SPELL_TYPING: "edit.spelling.whileTyping", "Check Spelling While Typing".checked(p::spell_typing).enabled(p::doc);
    EDIT_SPELL_GRAMMAR: "edit.spelling.grammar", "Check Grammar With Spelling".checked(p::spell_grammar).enabled(p::doc);
    EDIT_SPELL_AUTO: "edit.spelling.autoCorrect", "Correct Spelling Automatically".checked(p::spell_auto).enabled(p::doc);
    EDIT_UPPERCASE: "edit.transform.uppercase", "Make Upper Case".name("uppercaseWord").enabled(p::editing);
    EDIT_LOWERCASE: "edit.transform.lowercase", "Make Lower Case".name("lowercaseWord").enabled(p::editing);
    EDIT_CAPITALIZE: "edit.transform.capitalize", "Capitalize".name("capitalizeWord").enabled(p::editing);
    EDIT_START_SPEAKING: "edit.speech.start", "Start Speaking".enabled(p::doc);
    EDIT_STOP_SPEAKING: "edit.speech.stop", "Stop Speaking";

    // ---- Find
    FIND_FIND: "find.find", "Find…".key("@f").enabled(p::doc);
    FIND_REPLACE: "find.findAndReplace", "Find and Replace…".key("~@f").enabled(p::doc);
    FIND_NEXT: "find.next", "Find Next".key("@g").enabled(p::doc);
    FIND_PREVIOUS: "find.previous", "Find Previous".key("$@G").enabled(p::doc);
    FIND_SELECT_NEXT: "find.selectNext", "Find and Select Next".key("@e").enabled(p::editor);
    FIND_SELECT_PREVIOUS: "find.selectPrevious", "Find and Select Previous".key("@E").enabled(p::editor);
    FIND_SELECT_ALL: "find.selectAll", "Find and Select All".key("^@e").enabled(p::editor);
    FIND_SELECT_WORD: "find.selectWord", "Find and Select Word".key("~@E").enabled(p::editor);
    FIND_PATTERN: "find.pattern", "Find Pattern…".key("^~@p").enabled(p::doc);
    FIND_MODE_TEXT: "find.mode.text", "Text".name("setTextFinderMatchingType").checked(p::mode_text).enabled(p::doc);
    FIND_MODE_REGEX: "find.mode.regularExpression", "Regular Expression".checked(p::mode_regex).enabled(p::doc);
    FIND_MODE_WORD: "find.mode.word", "Word".checked(p::mode_word).enabled(p::doc);
    FIND_MODE_PREFIX: "find.mode.wordPrefix", "Word Prefix".checked(p::mode_prefix).enabled(p::doc);
    FIND_MODE_SUFFIX: "find.mode.wordSuffix", "Word Suffix".checked(p::mode_suffix).enabled(p::doc);
    FIND_MODE_WHITESPACE: "find.mode.ignoringWhitespace", "Ignoring Whitespace".checked(p::mode_ws).enabled(p::doc);
    FIND_IGNORE_CASE: "find.ignoreCase", "Ignore Case".name("toggleTextFinderIgnoreCase").checked(p::ignore_case).enabled(p::doc);
    FIND_WRAP_AROUND: "find.wrapAround", "Wrap Around".name("toggleTextFinderWrapAround").checked(p::wrap_around).enabled(p::doc);
    FIND_SELECT_NEXT_RESULT: "find.selectNextResult", "Select Next Result".key("~@g").enabled(p::editor);
    FIND_SELECT_PREVIOUS_RESULT: "find.selectPreviousResult", "Select Previous Result".key("~@G").enabled(p::editor);
    FIND_SELECT_ALL_RESULTS: "find.selectAllResults", "Select All Results".enabled(p::editor);
    FIND_SELECT_ALL_RESULTS_IN_SELECTION: "find.selectAllResultsInSelection", "Select All Results in Selection".enabled(p::selection);
    FIND_REPLACE_AND_NEXT: "find.replaceAndFindNext", "Replace and Find Next".key("@*").enabled(p::editing);
    FIND_REPLACE_AND_PREVIOUS: "find.replaceAndFindPrevious", "Replace and Find Previous".enabled(p::editing);
    FIND_REPLACE_ALL_IN_SELECTION: "find.replaceAllInSelection", "Replace All in Selection".enabled(p::selection);
    FIND_REPLACE_ALL: "find.replaceAll", "Replace All".key("^~@*").enabled(p::editing);
    FIND_JUMP_NEXT_SELECTION: "find.jumpToNextSelection", "Jump to Next Selection".key("@j").name("centerSelectionInVisibleArea").enabled(p::editor);
    FIND_JUMP_PREVIOUS_SELECTION: "find.jumpToPreviousSelection", "Jump to Previous Selection".key("@J").name("centerPreviousSelectionInVisibleArea").enabled(p::editor);
    FIND_IN_FILES: "find.searchInFileSystem", "Search in File System…".key("~@F").enabled(p::window);
    FIND_SELECTED_IN_FILES: "find.searchSelectedInFileSystem", "Search for Selected Text in File System".key("@F").enabled(p::window);
    FIND_SELECTED_IN_DOCUMENT: "find.searchSelectedInDocument", "Search for Selected Text in Current Document".key("^@F").title_fn(t::search_selected_in_document).enabled(p::doc);

    // ---- Text
    TEXT_COMPLETE: "text.showCompletions", "Show Completions".key("\u{1b}").name("codeComplete").enabled(p::editing);
    TEXT_LOOK_UP: "text.lookUpSymbol", "Look Up Current Symbol".key("@i").name("lookUpSymbolInDocs").title_fn(t::look_up).enabled(p::doc);
    TEXT_AUTO_INDENT: "text.autoIndent", "Auto-Indent Selection".key("^i").name("autoIndent").enabled(p::editing);
    TEXT_SHIFT_RIGHT: "text.shiftRight", "Shift Right".key("@]").name("shiftRightSelection").enabled(p::editing);
    TEXT_SHIFT_LEFT: "text.shiftLeft", "Shift Left".key("@[").name("shiftLeftSelection").enabled(p::editing);
    TEXT_TOGGLE_COMMENT: "text.toggleComment", "Un/Comment Lines".key("@/").name("toggleCommentedSelection").enabled(p::editing);
    TEXT_SELECT_COLUMN_UP: "text.selectColumnUp", "Select Column Up".key("^$\u{F700}").name("selectColumnUp").enabled(p::editor);
    TEXT_SELECT_COLUMN_DOWN: "text.selectColumnDown", "Select Column Down".key("^$\u{F701}").name("selectColumnDown").enabled(p::editor);
    TEXT_SPLIT_LINES: "text.splitSelectionByLines", "Split Selection By Lines".key("^$\r").name("splitSelectionByLines").enabled(p::editor);
    TEXT_SPLIT_WORDS: "text.splitSelectionByWords", "Split Selection By Words".key("^~$\r").name("splitSelectionByWords").enabled(p::editor);
    TEXT_SINGLE_SELECTION: "text.singleSelection", "Single Selection".key("^j").name("singleSelection").enabled(p::editor);
    TEXT_INSERT_SELECTION_NUMBER: "text.insertSelectionNumber", "Insert Selection Number".key("~@1").name("insertSelectionNumber").enabled(p::editing);
    TEXT_SELECT_LINE: "text.selectLine", "Select Line".key("^s").name("selectParagraph").enabled(p::editor);
    TEXT_SELECT_WORD: "text.selectWord", "Select Word".key("^w").name("selectWord").enabled(p::editor);
    TEXT_BALANCE: "text.balanceDelimiters", "Select to Delimiters".key("^~\u{F700}").name("balanceDelimiters").enabled(p::editor);
    TEXT_BALANCE_IN: "text.balanceDelimitersInwards", "Select to Delimiters Inwards".key("^~\u{F701}").name("balanceDelimitersInwards").enabled(p::editor);
    TEXT_BALANCE_RIGHT: "text.balanceDelimitersRight", "Select to Delimiters Right".key("^~\u{F703}").name("balanceDelimitersRight").enabled(p::editor);
    TEXT_BALANCE_LEFT: "text.balanceDelimitersLeft", "Select to Delimiters Left".key("^~\u{F702}").name("balanceDelimitersLeft").enabled(p::editor);
    TEXT_SURROUND_SINGLE: "text.surround.singleQuotes", "Single Quotes".key("@'").name("singleQuoteSelection").enabled(p::editing);
    TEXT_SURROUND_DOUBLE: "text.surround.doubleQuotes", "Double Quotes".key("@\"").name("doubleQuoteSelection").enabled(p::editing);
    TEXT_SURROUND_PARENS: "text.surround.parentheses", "Parentheses".key("@(").name("parenthesizeSelection").enabled(p::editing);
    TEXT_SURROUND_BRACES: "text.surround.braces", "Braces".key("^@{").name("braceSelection").enabled(p::editing);
    TEXT_SURROUND_BRACKETS: "text.surround.brackets", "Brackets".key("~@[").name("bracketSelection").enabled(p::editing);
    TEXT_SURROUND_CUSTOM: "text.surround.custom", "Custom…".key("@L").name("surroundSelectionWithCustomString").enabled(p::editing);
    TEXT_DUPLICATE_LINE: "text.duplicateLine", "Duplicate Line".key("@d").name("duplicateLine").enabled(p::editing);
    TEXT_DELETE_LINE: "text.deleteLine", "Delete Line".key("$@\u{8}").name("deleteLine").enabled(p::editing);
    TEXT_DELETE_LINE_UP: "text.deleteLineUp", "Delete Line Up".key("~$@\u{8}").name("deleteLineUp").enabled(p::editing);
    TEXT_MOVE_LINE_UP: "text.moveLineUp", "Swap Lines Up".key("~@\u{F700}").name("moveLineUp").enabled(p::editing);
    TEXT_MOVE_LINE_DOWN: "text.moveLineDown", "Swap Lines Down".key("~@\u{F701}").name("moveLineDown").enabled(p::editing);
    TEXT_GO_TO_LINE: "text.goToLine", "Go to Line…".key("@l").name("goToLine").enabled(p::doc);
    OPT_SHOW_INVISIBLES: "text.options.showInvisibles", "Show Invisibles".key("~@i").checked(p::show_invisibles).enabled(p::doc);
    OPT_SHOW_LINE_NUMBERS: "text.options.showLineNumbers", "Show Line Numbers".checked(p::show_line_numbers).enabled(p::doc);
    OPT_SHOW_PAGE_GUIDE: "text.options.showPageGuide", "Show Page Guide".key("~@p").checked(p::show_page_guide).enabled(p::doc);
    OPT_WRAP_LINES: "text.options.wrapLines", "Wrap Lines".checked(p::wrap_lines).enabled(p::doc);
    OPT_SCROLL_PAST_END: "text.options.allowScrollPastEnd", "Allow Scroll Past End".checked(p::scroll_past_end).enabled(p::doc);
    OPT_HIGHLIGHT_LINE: "text.options.highlightCurrentLine", "Highlight Current Line".checked(p::highlight_line).enabled(p::doc);
    OPT_TEXT_PREFERENCES: "text.options.preferences", "Text Editing Preferences…";
    TEXT_SHOW_COLOR_SCOPES: "text.showColorScopes", "Show Color Scopes".key("^@p").enabled(p::editor);
    TEXT_SHOW_FONTS: "text.font.showFonts", "Show Fonts".key("@T");
    TEXT_FONT_BIGGER: "text.font.bigger", "Bigger".key("@+").name("increaseFontSize").enabled(p::window);
    TEXT_FONT_SMALLER: "text.font.smaller", "Smaller".key("@-").name("decreaseFontSize").enabled(p::window);
    TEXT_ENCODING_AUTO: "text.encoding.automatic", "Automatic (UTF-8)".hidden_in_editor();
    TEXT_IMPORT_SYNTAX: "text.syntax.import", "Import Syntax File…".hidden_in_editor();

    // ---- Run
    RUN_RUN: "run.run", "Run".key("@r").enabled(p::run);
    RUN_RUN_THIS_DOCUMENT: "run.runThisDocument", "Run".key("^~@r").title_fn(t::run_this_document).enabled(p::run_alt);
    RUN_RUN_AND_DEBUG: "run.runAndDebug", "Run and Debug".key("~@r").enabled(p::run_debug);
    RUN_RUN_WITHOUT_DEBUGGING: "run.runWithoutDebugging", "Run Without Debugging".key("^@r").enabled(p::run);
    RUN_BUILD: "run.build", "Run Diagnostics".key("@b").title_fn(t::build).enabled(p::lint);
    RUN_STOP: "run.stop", "Stop".key("@.").enabled(p::stop);
    DEBUG_TOGGLE_BREAKPOINT: "debug.toggleBreakpoint", "Toggle Breakpoint at Current Line".key("@\\").enabled(p::editor);
    DEBUG_CLEAR_BREAKPOINTS: "debug.clearBreakpoints", "Clear Breakpoints".key("@y").enabled(p::breakpoints);
    DEBUG_PAUSE: "debug.pause", "Pause Execution".key("^1").title_fn(t::pause).enabled(p::debugging);
    DEBUG_STEP_OVER: "debug.stepOver", "Step Over".key("^2").enabled(p::stepping);
    DEBUG_STEP_IN: "debug.stepIn", "Step In".key("^3").enabled(p::stepping);
    DEBUG_STEP_OUT: "debug.stepOut", "Step Out".key("^4").enabled(p::stepping);
    RUN_EDIT_SETTINGS: "run.editRunSettings", "Edit Run Settings…".key("$@R").enabled(p::doc);
    RUN_CLEAR_CONSOLE: "run.clearConsole", "Clear Console".key("@k").enabled(p::doc);
    RUN_NO_LANGUAGE: "run.language.none", "None".enabled(p::doc).hidden_in_editor();
    RUN_SELECT_LANGUAGE: "run.selectLanguage", "Select Language…".key("~@l").enabled(p::doc);
    RUN_EDIT_LANGUAGES: "run.editLanguages", "Edit Languages…".key("~@,");

    // ---- Diagnostics
    DIAG_APPLY_NEXT_FIX: "diagnostics.applyNextFix", "Apply Next Fix".key("~@;").enabled(p::apply_fix);
    DIAG_APPLY_ALL_FIXES: "diagnostics.applyAllFixes", "Apply All Fixes".key("~$@;").enabled(p::apply_fix);
    DIAG_NEXT: "diagnostics.jumpToNext", "Jump To Next Diagnostic".key("~@\u{F703}").enabled(p::next_diag);
    DIAG_NEXT_ERROR: "diagnostics.jumpToNextError", "Jump To Next Error".key("~$@\u{F703}").enabled(p::next_err);
    DIAG_PREVIOUS: "diagnostics.jumpToPrevious", "Jump To Previous Diagnostic".key("~@\u{F702}").enabled(p::prev_diag);
    DIAG_PREVIOUS_ERROR: "diagnostics.jumpToPreviousError", "Jump To Previous Error".key("~$@\u{F702}").enabled(p::prev_err);
    DIAG_IGNORED_FOR_DOCUMENT: "diagnostics.ignoredForDocument", "Ignored Messages For Document…".title_fn(t::ignored_for_document).enabled(p::doc);
    DIAG_IGNORED: "diagnostics.ignoredGlobal", "Ignored Messages…";
    DIAG_PREFERENCES: "diagnostics.preferences", "Diagnostics Preferences…";
    DIAG_ENABLE_FOR_LANGUAGE: "diagnostics.enableForLanguage", "Enable Diagnostics For Language".title_fn(t::enable_diagnostics).checked(p::diag_enabled).enabled(p::lint);
    DIAG_DISABLE_FOR_DOCUMENT: "diagnostics.disableForDocument", "Disable Diagnostics For Document".title_fn(t::disable_diagnostics_doc).enabled(p::lint);
    DIAG_RUN_NOW: "diagnostics.runNow", "Run Diagnostic Now".enabled(p::lint);
    DIAG_UPDATE_MANUALLY: "diagnostics.update.manually", "Manually".checked(p::upd_manual).enabled(p::lint);
    DIAG_UPDATE_ON_EDIT: "diagnostics.update.onEdit", "On Edit".checked(p::upd_edit).enabled(p::lint);
    DIAG_UPDATE_ON_RUN: "diagnostics.update.onRun", "On Run".checked(p::upd_run).enabled(p::lint);
    DIAG_CLEAR: "diagnostics.clear", "Clear Diagnostics".enabled(p::lint);

    // ---- View
    VIEW_TOGGLE_TAB_BAR: "view.toggleTabBar", "Hide Tab Bar".key("~@T").title_fn(t::toggle_tab_bar).enabled(p::window);
    VIEW_TOGGLE_SIDEBAR: "view.toggleFileNavigator", "Show File Navigator".key("@H").title_fn(t::toggle_sidebar).enabled(p::window);
    VIEW_TOGGLE_CONSOLE: "view.toggleConsole", "Show Console".key("@C").title_fn(t::toggle_console).enabled(p::window);
    VIEW_TOGGLE_DOCS: "view.toggleDocumentation", "Show Documentation View".key("@I").title_fn(t::toggle_docs).enabled(p::window);
    VIEW_HIDE_CONSOLE_AUTO: "view.hideConsoleAutomatically", "Automatically Hide Console".checked(p::hide_console_auto);
    VIEW_CONSOLE_BOTTOM: "view.consolePosition.bottom", "Bottom".checked(p::console_bottom).enabled(p::doc);
    VIEW_CONSOLE_RIGHT: "view.consolePosition.right", "Right".checked(p::console_right).enabled(p::doc);
    VIEW_TOGGLE_TOOLBAR: "view.toggleToolbar", "Show Toolbar".key("~@t").title_fn(t::toggle_toolbar).enabled(p::window);
    VIEW_CUSTOMIZE_TOOLBAR: "view.customizeToolbar", "Customize Toolbar…".enabled(p::window);
    VIEW_FULL_SCREEN: "view.toggleFullScreen", "Enter Full Screen".key("^@f").role(Role::FullScreen).title_fn(t::full_screen).enabled(p::window);

    // ---- Window
    WINDOW_MINIMIZE: "window.minimize", "Minimize".key("@m").role(Role::Minimize).enabled(p::window);
    WINDOW_ZOOM: "window.zoom", "Zoom".role(Role::Zoom).enabled(p::window);
    NAV_FORWARD: "window.goForward", "Go Forward".key("^@\u{F703}").title_fn(t::go_forward).enabled(p::go_forward);
    NAV_BACK: "window.goBack", "Go Back".key("^@\u{F702}").title_fn(t::go_back).enabled(p::go_back);
    WINDOW_PREV_TAB: "window.selectPreviousTab", "Select Previous Tab".key("@{").enabled(p::multi_tab);
    WINDOW_NEXT_TAB: "window.selectNextTab", "Select Next Tab".key("@}").enabled(p::multi_tab);
    WINDOW_MOVE_TAB_LEFT: "window.moveTabLeft", "Move Tab Left".key("~@{").enabled(p::multi_tab);
    WINDOW_MOVE_TAB_RIGHT: "window.moveTabRight", "Move Tab Right".key("~@}").enabled(p::multi_tab);
    WINDOW_MOVE_TAB_TO_NEW_WINDOW: "window.moveTabToNewWindow", "Move Tab to New Window".key("~@n").enabled(p::multi_tab);
    WINDOW_MERGE_ALL: "window.mergeAllWindows", "Merge All Windows".enabled(p::merge);
    WINDOW_BRING_ALL_TO_FRONT: "window.bringAllToFront", "Bring All to Front";

    // ---- Help
    HELP_HELP: "help.help", "ScratchRunner Help".key("@?");
    HELP_WEBSITE: "help.website", "Visit ScratchRunner Website";
    HELP_SUPPORT: "help.support", "Visit ScratchRunner Support Page";
    HELP_ATTRIBUTIONS: "help.attributions", "Software Attributions";

    // ---- Context menus and popups (not in the menu bar)
    CTX_EDIT_TOKEN: "context.editToken", "Edit Token".kind(CommandKind::Context).scope(Scope::Editor).title_fn(t::edit_token).enabled(p::token);
    CTX_SEARCH_DOCS_PROVIDER: "context.searchWithDocsProvider", "Search in Documentation".kind(CommandKind::Context).scope(Scope::Editor).enabled(p::selection);
    CTX_SEARCH_WEB: "context.searchWeb", "Search Google".kind(CommandKind::Context).scope(Scope::Editor).enabled(p::selection);
    CTX_COPY_LINK: "context.copyLink", "Copy Link".kind(CommandKind::Context).scope(Scope::Editor);
    CTX_TRANSLATE: "context.translate", "Translate".kind(CommandKind::Context).scope(Scope::Editor).enabled(p::selection);
    NAV_OPEN: "navigator.open", "Open".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_OPEN_NEW_WINDOW: "navigator.openInNewWindow", "Open in New Window".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_OPEN_FINDER: "navigator.openWithFinder", "Open with Finder".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_OPEN_TERMINAL: "navigator.openTerminalHere", "Open Terminal Window Here".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_SEARCH: "navigator.search", "Search…".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_NEW_FILE: "navigator.newFile", "New File".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_NEW_FOLDER: "navigator.newFolder", "New Folder".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_GO_TO_FOLDER: "navigator.goToFolder", "Go to Folder…".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_REFRESH: "navigator.refresh", "Refresh".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_COPY: "navigator.copy", "Copy".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_PASTE: "navigator.paste", "Paste".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_RENAME: "navigator.rename", "Rename".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_DUPLICATE: "navigator.duplicate", "Duplicate".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_DELETE: "navigator.delete", "Delete".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    NAV_SHOW_HIDDEN: "navigator.showHiddenFiles", "Show Hidden Files".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_SORT_FOLDERS_FIRST: "navigator.sortFoldersOnTop", "Sort Folders on Top".kind(CommandKind::Context).scope(Scope::Sidebar);
    NAV_SHOW_IN_FINDER: "navigator.showInFinder", "Show in Finder".kind(CommandKind::Context).scope(Scope::Sidebar).enabled(p::nav_selection);
    DOCS_OPEN_IN_BROWSER: "docs.openInBrowser", "Open in Browser".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_COPY_URL: "docs.copyCurrentURL", "Copy Current URL".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_BOOKMARK: "docs.bookmarkPage", "Bookmark This Page…".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_DEFINE_CONFIG: "docs.defineConfiguration", "Define Docs Configuration…".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_EDIT_CONFIGS: "docs.editConfigurations", "Edit Docs Configurations".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_GO_TO_URL: "docs.goToURL", "Go to URL…".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_FIND: "docs.find", "Find…".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_RELOAD: "docs.reload", "Reload Page".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_STYLIZE: "docs.stylizeWebContent", "Stylize Web Content".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_ZOOM_IN: "docs.zoomIn", "Zoom In".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_ZOOM_OUT: "docs.zoomOut", "Zoom Out".kind(CommandKind::Context).scope(Scope::Docs);
    DOCS_ZOOM_RESET: "docs.zoomReset", "Actual Size".kind(CommandKind::Context).scope(Scope::Docs);
    DEBUGVAR_PRINT: "debugVariable.printDescription", "Print Description".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_COPY: "debugVariable.copy", "Copy".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_EDIT: "debugVariable.editValue", "Edit Value…".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_WATCH: "debugVariable.watch", "Watch".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_SHOW_TYPES: "debugVariable.showTypes", "Show Types".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_SHOW_ALL: "debugVariable.showAllVariables", "Show All Variables".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_SORT_NAME: "debugVariable.sortByName", "Name".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_SORT_NATURAL: "debugVariable.sortByNaturalOrder", "Natural Order".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_SORT_REVERSE: "debugVariable.sortByReverseNaturalOrder", "Reverse Natural Order".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_HOVER: "debugVariable.showValuesOnHover", "Show Values on Hover".kind(CommandKind::Context).scope(Scope::Debugger);
    DEBUGVAR_ENABLE: "debugVariable.enableVariablesView", "Enable Variables View".kind(CommandKind::Context).scope(Scope::Debugger);
    DOCK_NEW_FILE: "dock.newFile", "New File".kind(CommandKind::Context);
}

// ---- window commands by name -------------------------------------------------------------

macro_rules! tab_commands {
    ($( $n:literal : $title:literal ; )*) => {
        static TAB_DEFS: &[CommandDef] = &[
            $( CommandDef::new(concat!("window.selectTab", $n), $title).kind(CommandKind::Window).name(concat!("selectTab", $n)).enabled(p::window), )*
            CommandDef::new("window.selectDocument", "Select Document").kind(CommandKind::Window).name("selectDocument").enabled(p::window),
        ];
    };
}

tab_commands! {
    "1": "Window ‣ Tab 1"; "2": "Window ‣ Tab 2"; "3": "Window ‣ Tab 3"; "4": "Window ‣ Tab 4"; "5": "Window ‣ Tab 5";
    "6": "Window ‣ Tab 6"; "7": "Window ‣ Tab 7"; "8": "Window ‣ Tab 8"; "9": "Window ‣ Tab 9"; "10": "Window ‣ Tab 10";
}

// ---- the editor command vocabulary ------------------------------------------------------

macro_rules! editor_commands {
    ($( $name:literal $title:literal ; )*) => {
        static EDITOR_DEFS: &[CommandDef] = &[
            $( CommandDef::new(concat!("editor.", $name), $title).kind(CommandKind::Editor).name($name).scope(Scope::Editor), )*
        ];
    };
}

editor_commands! {
    // custom editor commands
    "insertNewlineBelow" "Insert Newline Below";
    "insertNewlineBelowAndSelect" "Insert Newline Below and Select";
    "complete" "Complete";
    "insertIndentation" "Insert Indentation";
    "insertTabCharacter" "Insert Tab Character";
    "insertNewlineCharacter" "Insert Newline Character";
    "insertZeroIndexedSelectionNumber" "Insert Zero-Indexed Selection Number";
    "moveSubwordLeft" "Move Subword Left";
    "moveSubwordLeftAndModifySelection" "Select Subword Left";
    "moveSubwordRight" "Move Subword Right";
    "moveSubwordRightAndModifySelection" "Select Subword Right";
    "moveParagraphDown" "Move Paragraph Down";
    "moveParagraphDownAndModifySelection" "Select Paragraph Down";
    "moveParagraphUp" "Move Paragraph Up";
    "moveParagraphUpAndModifySelection" "Select Paragraph Up";
    "moveToBeginningOfVisibleArea" "Move to Beginning of Visible Area";
    "moveToBeginningOfVisibleAreaAndModifySelection" "Select to Beginning of Visible Area";
    "moveToCenterOfVisibleArea" "Move to Center of Visible Area";
    "moveToCenterOfVisibleAreaAndModifySelection" "Select to Center of Visible Area";
    "moveToEndOfVisibleArea" "Move to End of Visible Area";
    "moveToEndOfVisibleAreaAndModifySelection" "Select to End of Visible Area";
    "collapseSelectionToStart" "Collapse Selection to Start";
    "collapseSelectionToEnd" "Collapse Selection to End";
    "collapseSelection" "Collapse Selection";
    "setSelectionMovingEdgeToStart" "Set Selection Moving Edge to Start";
    "setSelectionMovingEdgeToEnd" "Set Selection Moving Edge to End";
    "showFindIndicator" "Show Find Indicator";
    "showFindIndicatorForBalancedCharacter" "Show Find Indicator for Balanced Character";
    "breakUndoCoalescing" "Break Undo Coalescing";
    "pasteAndSelect" "Paste and Select";
    "scrollSelectionToTop" "Scroll Selection to Top";
    "scrollSelectionToCenter" "Scroll Selection to Center";
    "scrollSelectionToBottom" "Scroll Selection to Bottom";
    "yankAndSelect" "Yank and Select";
    "moveToBeginningOfIndentation" "Move to Beginning of Indentation";
    "moveToEndOfIndentation" "Move to End of Indentation";
    "beep" "Beep";
    "makeEditable" "Make Editable";
    "makeNotEditable" "Make Not Editable";
    "noop" "No Operation";
    "debug" "Debug";
    "testSelectionMovingEdgeAtStart" "Test Selection Moving Edge At Start";
    "testSelectionMovingEdgeAtEnd" "Test Selection Moving Edge At End";
    "testSelectedCharacterIsBalanced" "Test Selected Character Is Balanced";
    "testSelectedCharacterNotBalanced" "Test Selected Character Not Balanced";
    "testHasMultipleSelections" "Test Has Multiple Selections";
    "testHasSingleSelection" "Test Has Single Selection";
    "testIsSourceEditor" "Test Is Source Editor";
    "testNotSourceEditor" "Test Not Source Editor";
    "testIsEditable" "Test Is Editable";
    "testNotEditable" "Test Not Editable";
    // standard NSResponder movement and editing
    "moveForward" "Move Forward";
    "moveBackward" "Move Backward";
    "moveLeft" "Move Left";
    "moveRight" "Move Right";
    "moveUp" "Move Up";
    "moveDown" "Move Down";
    "moveForwardAndModifySelection" "Select Forward";
    "moveBackwardAndModifySelection" "Select Backward";
    "moveLeftAndModifySelection" "Select Left";
    "moveRightAndModifySelection" "Select Right";
    "moveUpAndModifySelection" "Select Up";
    "moveDownAndModifySelection" "Select Down";
    "moveWordForward" "Move Word Forward";
    "moveWordBackward" "Move Word Backward";
    "moveWordLeft" "Move Word Left";
    "moveWordRight" "Move Word Right";
    "moveWordForwardAndModifySelection" "Select Word Forward";
    "moveWordBackwardAndModifySelection" "Select Word Backward";
    "moveWordLeftAndModifySelection" "Select Word Left";
    "moveWordRightAndModifySelection" "Select Word Right";
    "moveToBeginningOfLine" "Move to Beginning of Line";
    "moveToEndOfLine" "Move to End of Line";
    "moveToBeginningOfParagraph" "Move to Beginning of Paragraph";
    "moveToEndOfParagraph" "Move to End of Paragraph";
    "moveToBeginningOfDocument" "Move to Beginning of Document";
    "moveToEndOfDocument" "Move to End of Document";
    "moveToLeftEndOfLine" "Move to Left End of Line";
    "moveToRightEndOfLine" "Move to Right End of Line";
    "moveToBeginningOfLineAndModifySelection" "Select to Beginning of Line";
    "moveToEndOfLineAndModifySelection" "Select to End of Line";
    "moveToLeftEndOfLineAndModifySelection" "Select to Left End of Line";
    "moveToRightEndOfLineAndModifySelection" "Select to Right End of Line";
    "moveToBeginningOfParagraphAndModifySelection" "Select to Beginning of Paragraph";
    "moveToEndOfParagraphAndModifySelection" "Select to End of Paragraph";
    "moveToBeginningOfDocumentAndModifySelection" "Select to Beginning of Document";
    "moveToEndOfDocumentAndModifySelection" "Select to End of Document";
    "moveParagraphForwardAndModifySelection" "Select Paragraph Forward";
    "moveParagraphBackwardAndModifySelection" "Select Paragraph Backward";
    "pageUp" "Page Up";
    "pageDown" "Page Down";
    "pageUpAndModifySelection" "Select Page Up";
    "pageDownAndModifySelection" "Select Page Down";
    "transpose" "Transpose";
    "transposeWords" "Transpose Words";
    "selectLine" "Select Visual Line";
    "indent" "Indent";
    "insertBacktab" "Insert Backtab";
    "insertNewline" "Insert Newline";
    "insertParagraphSeparator" "Insert Paragraph Separator";
    "insertNewlineIgnoringFieldEditor" "Insert Newline Ignoring Field Editor";
    "insertTabIgnoringFieldEditor" "Insert Tab Ignoring Field Editor";
    "insertLineBreak" "Insert Line Break";
    "insertContainerBreak" "Insert Container Break";
    "insertSingleQuoteIgnoringSubstitution" "Insert Single Quote Ignoring Substitution";
    "insertDoubleQuoteIgnoringSubstitution" "Insert Double Quote Ignoring Substitution";
    "changeCaseOfLetter" "Change Case of Letter";
    "deleteForward" "Delete Forward";
    "deleteBackward" "Delete Backward";
    "deleteBackwardByDecomposingPreviousCharacter" "Delete Backward by Decomposing Previous Character";
    "deleteWordForward" "Delete Word Forward";
    "deleteWordBackward" "Delete Word Backward";
    "deleteToBeginningOfLine" "Delete to Beginning of Line";
    "deleteToEndOfLine" "Delete to End of Line";
    "deleteToEndOfParagraph" "Delete to End of Paragraph";
}

/// Iterates every command definition (menu, window, editor).
pub(crate) fn all_defs() -> impl Iterator<Item = &'static CommandDef> {
    MENU_DEFS.iter().chain(TAB_DEFS.iter()).chain(EDITOR_DEFS.iter())
}
