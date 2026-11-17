//! Key bindings: where the menus and `on_key`s resolve their shortcuts from,
//! and the `SetStore`-backed editing the Key Bindings pane does.
//!
//! A binding set ([`BindingSet`], a CodeRunner `.crBindings` file) maps key
//! equivalents to `MainMenu/…` menu paths, editor command names (`selectTab3`)
//! or compound commands. The app's items are [`COMMANDS`]: each carries the
//! path a binding to it stores — the path of the matching item of the
//! standard menu model, so real `.crBindings` files interoperate — plus the
//! command names that also mean it. [`keys_for`] resolves an item's effective
//! keystroke: the active set's binding, else the item's default.

use std::cell::RefCell;
use std::path::PathBuf;

use nsh_core::prelude::*;
use scratch_commands::bindings::{Binding, BindingSet, SetStore};
use scratch_commands::shortcut::{Shortcut, Stroke};
use scratch_config::Paths;

use super::preferences;
use super::state;

/// Bumped when a set or its selection changes: menus and `on_key`s re-resolve.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct BindingsRevision(pub u64);

pub fn revision() -> Signal<BindingsRevision> { global() }

pub fn bump() { revision().update(|r| r.0 += 1); }

// ---------------------------------------------------------------- commands

/// A menu item or workbench command a binding can override.
#[derive(PartialEq)]
pub struct Bound {
    /// Stable id; a `CommandId` of the command table where there is one.
    pub id: &'static str,
    /// The `command` a binding to this item stores (`MainMenu/…`, a menu path
    /// of the standard menu model, or an editor command name like `selectTab3`).
    pub path: &'static str,
    /// Other `command` spellings a binding to this item may use.
    pub names: &'static [&'static str],
    /// The menu the pane's Menu Items list groups it under.
    pub menu: &'static str,
    /// The title the row and a new binding's `title` show.
    pub title: &'static str,
    /// The default keystroke, NSH text (`""` is unbound by default).
    pub keys: &'static str,
}

const fn bound(id: &'static str, path: &'static str, names: &'static [&'static str], menu: &'static str, title: &'static str, keys: &'static str) -> Bound {
    Bound { id, path, names, menu, title, keys }
}

/// The bindable commands, in the pane's display order. Paths name the matching
/// items of the standard menu model (a real `.crBindings` written against
/// CodeRunner's menus lands on the same commands).
pub const COMMANDS: &[Bound] = &[
    bound("app.preferences", "MainMenu/ScratchRunner/Preferences…", &[], "ScratchRunner", "Settings…", "mod+,"),
    bound("run.editLanguages", "MainMenu/Run/Edit Languages…", &[], "ScratchRunner", "Edit Languages…", "mod+alt+,"),

    bound("file.newWindow", "MainMenu/File/New Window", &[], "File", "New Window", "mod+n"),
    bound("file.newTab", "MainMenu/File/New Tab", &[], "File", "New Tab", "mod+t"),
    bound("sr.newFromLanguage", "MainMenu/File/New Document from Language…", &[], "File", "New Document from Language…", ""),
    bound("file.open", "MainMenu/File/Open…", &[], "File", "Open…", "mod+o"),
    bound("file.openQuickly", "MainMenu/File/Open Quickly…", &[], "File", "Open Quickly…", "mod+shift+o"),
    bound("file.save", "MainMenu/File/Save", &[], "File", "Save", "mod+s"),
    bound("file.saveAs", "MainMenu/File/Save As…", &[], "File", "Save As…", "mod+shift+s"),
    bound("file.close", "MainMenu/File/Close Tab", &[], "File", "Close Tab", "mod+w"),
    bound("file.closeOtherTabs", "MainMenu/File/Close Other Tabs", &[], "File", "Close Other Tabs", "mod+alt+w"),
    bound("file.closeWindow", "MainMenu/File/Close Window", &[], "File", "Close Window", "mod+shift+w"),
    bound("file.newFromClipboard", "MainMenu/File/New from Clipboard", &[], "File", "New from Clipboard", "ctrl+cmd+v"),
    bound("file.goToCurrentDocument", "MainMenu/File/Go to Current Document", &[], "File", "Go to Current Document", "alt+cmd+j"),
    bound("file.duplicate", "MainMenu/File/Duplicate", &[], "File", "Duplicate", ""),
    bound("file.rename", "MainMenu/File/Rename…", &[], "File", "Rename…", ""),
    bound("file.moveTo", "MainMenu/File/Move To…", &[], "File", "Move To…", ""),
    bound("file.revertToSaved", "MainMenu/File/Revert to Saved", &[], "File", "Revert to Saved", ""),
    bound("file.pageSetup", "MainMenu/File/Page Setup…", &[], "File", "Page Setup…", "mod+shift+p"),
    bound("file.print", "MainMenu/File/Print…", &[], "File", "Print…", "mod+p"),
    bound("file.closeWithoutSaving", "MainMenu/File/Close Tab Without Saving", &[], "File", "Close Tab Without Saving", "ctrl+alt+cmd+w"),

    bound("find.find", "MainMenu/Find/Find…", &[], "Find", "Find…", "mod+f"),
    bound("find.findAndReplace", "MainMenu/Find/Find and Replace…", &[], "Find", "Find and Replace…", "mod+alt+f"),
    bound("find.next", "MainMenu/Find/Find Next", &[], "Find", "Find Next", "mod+g"),
    bound("find.previous", "MainMenu/Find/Find Previous", &[], "Find", "Find Previous", "mod+shift+g"),
    bound("find.selectNext", "MainMenu/Find/Find and Select Next", &["selectNextOccurrence"], "Find", "Find and Select Next", "mod+e"),
    bound("find.selectPrevious", "MainMenu/Find/Find and Select Previous", &["selectPreviousOccurrence"], "Find", "Find and Select Previous", "mod+shift+e"),
    bound("find.selectAll", "MainMenu/Find/Find and Select All", &["selectAllOccurrences"], "Find", "Find and Select All", "cmd+ctrl+e"),
    bound("find.selectWord", "MainMenu/Find/Find and Select Word", &[], "Find", "Find and Select Word", "mod+alt+shift+e"),
    bound("find.useSelection", "MainMenu/Find/Use Selection for Find", &[], "Find", "Use Selection for Find", ""),
    bound("find.pattern", "MainMenu/Find/Find Pattern…", &[], "Find", "Find Pattern…", "cmd+ctrl+alt+p"),
    bound("find.selectNextResult", "MainMenu/Find/Select Next Result", &[], "Find", "Select Next Result", "mod+alt+g"),
    bound("find.selectPreviousResult", "MainMenu/Find/Select Previous Result", &[], "Find", "Select Previous Result", "mod+alt+shift+g"),
    bound("find.selectAllResults", "MainMenu/Find/Select All Results", &[], "Find", "Select All Results", ""),
    bound("find.selectAllResultsInSelection", "MainMenu/Find/Select All Results in Selection", &[], "Find", "Select All Results in Selection", ""),
    bound("find.replaceAndFindNext", "MainMenu/Find/Replace and Find Next", &[], "Find", "Replace and Find Next", ""),
    bound("find.replaceAndFindPrevious", "MainMenu/Find/Replace and Find Previous", &[], "Find", "Replace and Find Previous", ""),
    bound("find.replaceAllInSelection", "MainMenu/Find/Replace All in Selection", &[], "Find", "Replace All in Selection", ""),
    bound("find.replaceAll", "MainMenu/Find/Replace All", &[], "Find", "Replace All", ""),
    bound("find.jumpNextSelection", "MainMenu/Find/Jump to Next Selection", &[], "Find", "Jump to Next Selection", "mod+j"),
    bound("find.jumpPreviousSelection", "MainMenu/Find/Jump to Previous Selection", &[], "Find", "Jump to Previous Selection", "mod+shift+j"),
    bound("find.searchSelectedFileSystem", "MainMenu/Find/Search for Selected Text in File System", &[], "Find", "Search for Selected Text in File System", "mod+shift+f"),
    bound("find.searchSelectedDocument", "MainMenu/Find/Search for Selected Text in Current Document", &[], "Find", "Search for Selected Text in Current Document", "cmd+ctrl+shift+f"),
    bound("text.goToLine", "MainMenu/Text/Go to Line…", &[], "Find", "Go to Line…", "mod+l"),
    bound("find.searchInFileSystem", "MainMenu/Find/Search in File System…", &[], "Find", "Search in Files…", "mod+alt+shift+f"),

    bound("text.autoIndentSelection", "MainMenu/Text/Auto-Indent Selection", &[], "Text", "Auto-Indent Selection", "ctrl+i"),
    bound("text.shiftRight", "MainMenu/Text/Shift Right", &["shiftRightSelection"], "Text", "Shift Right", "mod+]"),
    bound("text.shiftLeft", "MainMenu/Text/Shift Left", &["shiftLeftSelection"], "Text", "Shift Left", "mod+["),
    bound("text.toggleComment", "MainMenu/Text/Un/Comment Lines", &["toggleCommentedSelection"], "Text", "Un/Comment Lines", "mod+/"),
    bound("text.selectLine", "MainMenu/Text/Select Line", &["selectParagraph"], "Text", "Select Line", "ctrl+s"),
    bound("text.duplicateLine", "MainMenu/Text/Duplicate Line", &["duplicateLine"], "Text", "Duplicate Line", "mod+d"),
    bound("text.deleteLine", "MainMenu/Text/Delete Line", &["deleteLine"], "Text", "Delete Line", "mod+shift+backspace"),
    bound("text.moveLineUp", "MainMenu/Text/Swap Lines Up", &["moveLineUp"], "Text", "Swap Lines Up", "mod+alt+up"),
    bound("text.moveLineDown", "MainMenu/Text/Swap Lines Down", &["moveLineDown"], "Text", "Swap Lines Down", "mod+alt+down"),
    bound("text.showFonts", "MainMenu/Text/Font/Show Fonts", &[], "Text", "Show Fonts", "mod+shift+t"),
    bound("text.font.bigger", "MainMenu/Text/Font/Bigger", &["increaseFontSize"], "Text", "Bigger", "mod+plus"),
    bound("text.font.smaller", "MainMenu/Text/Font/Smaller", &["decreaseFontSize"], "Text", "Smaller", "mod+minus"),
    bound("sr.actualSize", "MainMenu/Text/Font/Actual Size", &[], "Text", "Actual Size", ""),
    bound("run.selectLanguage", "MainMenu/Run/Select Language…", &[], "Text", "Select Language…", "mod+alt+l"),
    bound("text.options.showInvisibles", "MainMenu/Text/Editor Options/Show Invisibles", &[], "Text", "Show Invisibles", "mod+alt+i"),
    bound("text.showCompletions", "MainMenu/Text/Show Completions", &["complete"], "Text", "Show Completions", "ctrl+space"),
    bound("text.lookupSymbol", "MainMenu/Text/Look Up Current Symbol", &[], "Text", "Look Up Current Symbol", "mod+i"),
    bound("text.selectColumnUp", "MainMenu/Text/Select Column Up", &["addCaretAbove"], "Text", "Select Column Up", "ctrl+shift+up"),
    bound("text.selectColumnDown", "MainMenu/Text/Select Column Down", &["addCaretBelow"], "Text", "Select Column Down", "ctrl+shift+down"),
    bound("text.splitSelectionByLines", "MainMenu/Text/Split Selection By Lines", &["splitSelectionIntoLines"], "Text", "Split Selection By Lines", "ctrl+shift+return"),
    bound("text.splitSelectionByWords", "MainMenu/Text/Split Selection By Words", &[], "Text", "Split Selection By Words", "ctrl+alt+shift+return"),
    bound("text.showColorScopes", "MainMenu/Text/Show Color Scopes", &[], "Text", "Show Color Scopes", "ctrl+cmd+p"),
    bound("text.surround.singleQuotes", "MainMenu/Text/Surround Selection With/Single Quotes", &[], "Text", "Surround With Single Quotes", "cmd+'"),
    bound("text.surround.doubleQuotes", "MainMenu/Text/Surround Selection With/Double Quotes", &[], "Text", "Surround With Double Quotes", "cmd+shift+'"),
    bound("text.surround.parentheses", "MainMenu/Text/Surround Selection With/Parentheses", &[], "Text", "Surround With Parentheses", "cmd+shift+9"),
    bound("text.surround.braces", "MainMenu/Text/Surround Selection With/Braces", &[], "Text", "Surround With Braces", "ctrl+cmd+shift+["),
    bound("text.surround.brackets", "MainMenu/Text/Surround Selection With/Brackets", &[], "Text", "Surround With Brackets", "alt+cmd+["),
    bound("text.singleSelection", "MainMenu/Text/Single Selection", &["singleSelection"], "Text", "Single Selection", "cmd+ctrl+j"),
    bound("text.selectWord", "MainMenu/Text/Select Word", &["selectWord"], "Text", "Select Word", "ctrl+w"),
    bound("text.selectToDelimiters", "MainMenu/Text/Select to Delimiters", &[], "Text", "Select to Delimiters", "ctrl+alt+up"),
    bound("text.selectToDelimitersInwards", "MainMenu/Text/Select to Delimiters Inwards", &[], "Text", "Select to Delimiters Inwards", "ctrl+alt+down"),
    bound("text.selectToDelimitersRight", "MainMenu/Text/Select to Delimiters Right", &[], "Text", "Select to Delimiters Right", "ctrl+alt+right"),
    bound("text.selectToDelimitersLeft", "MainMenu/Text/Select to Delimiters Left", &[], "Text", "Select to Delimiters Left", "ctrl+alt+left"),
    bound("text.insertSelectionNumber", "MainMenu/Text/Insert Selection Number", &[], "Text", "Insert Selection Number", "alt+cmd+1"),
    bound("text.options.lineNumbers", "MainMenu/Text/Editor Options/Show Line Numbers", &[], "Text", "Show Line Numbers", ""),
    bound("text.options.wrapLines", "MainMenu/Text/Editor Options/Wrap Lines", &[], "Text", "Wrap Lines", ""),
    bound("text.options.currentLine", "MainMenu/Text/Editor Options/Highlight Current Line", &[], "Text", "Highlight Current Line", ""),
    bound("text.options.pageGuide", "MainMenu/Text/Editor Options/Show Page Guide", &[], "Text", "Show Page Guide", "mod+alt+p"),
    bound("text.options.scrollPastEnd", "MainMenu/Text/Editor Options/Allow Scroll Past End", &[], "Text", "Allow Scroll Past End", ""),
    bound("text.preferences", "MainMenu/Text/Editor Options/Text Editing Preferences…", &[], "Text", "Text Editing Preferences…", ""),

    bound("view.toggleFileNavigator", "MainMenu/View/Show File Navigator", &[], "View", "Show File Navigator", "cmd+shift+h"),
    bound("view.toggleToolbar", "MainMenu/View/Show Toolbar", &[], "View", "Show/Hide Toolbar", "alt+cmd+t"),
    bound("view.toggleTabBar", "MainMenu/View/Show Tab Bar", &[], "View", "Show/Hide Tab Bar", "alt+shift+cmd+t"),
    bound("view.consolePosition.bottom", "MainMenu/View/Console Position/Bottom", &[], "View", "Show Console at Bottom", ""),
    bound("view.consolePosition.right", "MainMenu/View/Console Position/Right", &[], "View", "Show Console on Right", ""),
    bound("view.toggleConsole", "MainMenu/View/Show Console", &[], "View", "Show/Hide Console", "mod+shift+c"),
    bound("view.toggleDocumentation", "MainMenu/View/Show Documentation View", &[], "View", "Show Documentation", "mod+shift+i"),

    bound("run.run", "MainMenu/Run/Run", &[], "Run", "Run", "mod+r"),
    bound("run.stop", "MainMenu/Run/Stop", &[], "Run", "Stop", "mod+."),
    bound("run.runAndDebug", "MainMenu/Run/Run and Debug", &[], "Run", "Run and Debug", "mod+alt+r"),
    bound("run.runWithoutDebugging", "MainMenu/Run/Run Without Debugging", &[], "Run", "Run Without Debugging", "cmd+ctrl+r"),
    bound("run.runDiagnostics", "MainMenu/Run/Run Diagnostics", &[], "Run", "Run Diagnostics", "mod+b"),
    bound("sr.stopDebugging", "MainMenu/Run/Stop Debugging", &[], "Run", "Stop Debugging", ""),
    bound("debug.toggleBreakpoint", "MainMenu/Run/Debugging/Toggle Breakpoint at Current Line", &[], "Run", "Toggle Breakpoint", "mod+\\"),
    bound("debug.clearBreakpoints", "MainMenu/Run/Debugging/Clear Breakpoints", &[], "Run", "Clear Breakpoints", "mod+y"),
    bound("debug.pause", "MainMenu/Run/Debugging/Pause Execution", &[], "Run", "Pause Execution", "ctrl+1"),
    bound("debug.stepOver", "MainMenu/Run/Debugging/Step Over", &[], "Run", "Step Over", "ctrl+2"),
    bound("debug.stepIn", "MainMenu/Run/Debugging/Step In", &[], "Run", "Step In", "ctrl+3"),
    bound("debug.stepOut", "MainMenu/Run/Debugging/Step Out", &[], "Run", "Step Out", "ctrl+4"),
    bound("run.editRunSettings", "MainMenu/Run/Edit Run Settings…", &[], "Run", "Edit Run Settings…", "mod+shift+r"),
    bound("run.clearConsole", "MainMenu/Run/Clear Console", &[], "Run", "Clear Console", "mod+k"),

    bound("diag.applyNextFix", "MainMenu/Diagnostics/Apply Next Fix", &[], "Diagnostics", "Apply Next Fix", "mod+alt+;"),
    bound("diag.applyAllFixes", "MainMenu/Diagnostics/Apply All Fixes", &[], "Diagnostics", "Apply All Fixes", "mod+alt+shift+;"),
    bound("diag.nextDiagnostic", "MainMenu/Diagnostics/Jump To Next Diagnostic", &[], "Diagnostics", "Jump To Next Diagnostic", "mod+alt+right"),
    bound("diag.nextError", "MainMenu/Diagnostics/Jump To Next Error", &[], "Diagnostics", "Jump To Next Error", "mod+alt+shift+right"),
    bound("diag.previousDiagnostic", "MainMenu/Diagnostics/Jump To Previous Diagnostic", &[], "Diagnostics", "Jump To Previous Diagnostic", "mod+alt+left"),
    bound("diag.previousError", "MainMenu/Diagnostics/Jump To Previous Error", &[], "Diagnostics", "Jump To Previous Error", "mod+alt+shift+left"),
    bound("diag.ignoredForDocument", "MainMenu/Diagnostics/Ignored Messages For Document…", &[], "Diagnostics", "Ignored Messages For Document…", ""),
    bound("diag.ignoredMessages", "MainMenu/Diagnostics/Ignored Messages…", &[], "Diagnostics", "Ignored Messages…", ""),
    bound("diag.preferences", "MainMenu/Diagnostics/Diagnostics Preferences…", &[], "Diagnostics", "Diagnostics Preferences…", ""),

    bound("window.selectPreviousTab", "MainMenu/Window/Select Previous Tab", &[], "Window", "Select Previous Tab", "mod+shift+["),
    bound("window.selectNextTab", "MainMenu/Window/Select Next Tab", &[], "Window", "Select Next Tab", "mod+shift+]"),
    bound("window.goBack", "MainMenu/Window/Go Back", &[], "Window", "Go Back", "cmd+ctrl+left"),
    bound("window.goForward", "MainMenu/Window/Go Forward", &[], "Window", "Go Forward", "cmd+ctrl+right"),
    bound("window.moveTabLeft", "MainMenu/Window/Move Tab Left", &[], "Window", "Move Tab Left", "mod+alt+shift+["),
    bound("window.moveTabRight", "MainMenu/Window/Move Tab Right", &[], "Window", "Move Tab Right", "mod+alt+shift+]"),
    bound("edit.deleteForward", "MainMenu/Edit/Delete", &[], "Edit", "Delete", ""),
    bound("window.selectTab1", "selectTab1", &[], "Window", "Tab 1", "mod+1"),
    bound("window.selectTab2", "selectTab2", &[], "Window", "Tab 2", "mod+2"),
    bound("window.selectTab3", "selectTab3", &[], "Window", "Tab 3", "mod+3"),
    bound("window.selectTab4", "selectTab4", &[], "Window", "Tab 4", "mod+4"),
    bound("window.selectTab5", "selectTab5", &[], "Window", "Tab 5", "mod+5"),
    bound("window.selectTab6", "selectTab6", &[], "Window", "Tab 6", "mod+6"),
    bound("window.selectTab7", "selectTab7", &[], "Window", "Tab 7", "mod+7"),
    bound("window.selectTab8", "selectTab8", &[], "Window", "Tab 8", "mod+8"),
    bound("window.selectTab9", "selectTab9", &[], "Window", "Tab 9", "mod+9"),
    bound("window.selectTab10", "selectTab10", &[], "Window", "Tab 10", "mod+0"),

    bound("help.help", "MainMenu/Help/ScratchRunner Help", &[], "Help", "ScratchRunner Help", "mod+shift+/"),
];

/// The bound command `id` names, if there is one.
pub fn command(id: &str) -> Option<&'static Bound> {
    COMMANDS.iter().find(|b| b.id == id)
}

// ------------------------------------------------------------------- store

/// The sets directory, under the app-data directory like `Paths`.
pub fn store() -> Option<SetStore> {
    std::env::var_os("SCRATCHRUNNER_DATA_DIR")
        .map(|dir| SetStore::new(Paths::in_dir(dir).key_bindings()))
        .or_else(SetStore::application_default)
}

/// The name of the set the preferences select.
pub fn selected_set_name() -> String {
    preferences::settings().with_untracked(|p| p.key_bindings.set.clone())
}

/// The active set: the user's file when present, else the shipped one.
pub fn active_set() -> BindingSet {
    store()
        .and_then(|store| store.load_set(&selected_set_name()).ok())
        .unwrap_or_else(scratch_commands::bindings::default_set)
}

/// The global set (the user's `_Global_` file or the shipped one).
pub fn global_set() -> BindingSet {
    store().and_then(|store| store.load_global().ok()).unwrap_or_else(scratch_commands::bindings::global_set)
}

/// The sets of the "Key Bindings Set" popup: shipped sets, then the user's.
pub fn sets() -> Vec<BindingSet> {
    store().and_then(|store| store.list_sets().ok()).unwrap_or_else(scratch_commands::bindings::builtin_sets)
}

/// Makes `name` the active set.
pub fn select_set(name: &str) {
    preferences::write(|p| p.key_bindings.set = name.to_owned());
    bump();
}

// ---------------------------------------------------------------- resolve

/// Whether `binding` overrides `bound` (by menu path or a command name).
fn matches(b: &Binding, bound: &Bound) -> bool {
    !b.is_compound()
        && (b.command.as_deref() == Some(bound.path)
            || bound.names.iter().any(|name| b.command.as_deref() == Some(*name)))
}

/// The active set's binding for `bound`, if there is one.
pub fn binding_of<'a>(set: &'a BindingSet, bound: &Bound) -> Option<&'a Binding> {
    set.bindings.iter().find(|b| matches(b, bound))
}

/// The keystroke `id` fires on, NSH text for `on_key`/`MenuItem::shortcut`;
/// `""` when unbound. A binding wins over the default; a disabled binding lets
/// the default back through; a key NSH cannot bind (a chord) binds nothing.
pub fn keys_for(id: &str) -> String {
    let Some(bound) = command(id) else { return String::new() };
    match binding_of(&active_set(), bound) {
        Some(b) if !b.enabled => bound.keys.to_owned(),
        Some(b) => match b.shortcut() {
            Ok(Some(shortcut)) => shortcut.to_nsh().unwrap_or_default(),
            Ok(None) => String::new(),
            Err(_) => bound.keys.to_owned(),
        },
        None => bound.keys.to_owned(),
    }
}

// The handlers installed for bound commands, by `Bound::id` — what a
// compound command's `execute`/`selectMenuItem:` dispatches.
thread_local! { static HANDLERS: RefCell<std::collections::HashMap<String, std::rc::Rc<dyn Fn()>>> = RefCell::new(std::collections::HashMap::new()); }

/// Registers `handler` for `id`'s effective keystroke when bound. Call inside
/// an `effect` that reads [`revision`]: a re-run re-registers the new keys.
pub fn on(id: &str, handler: impl Fn() + 'static) {
    register(id, handler);
    let keys = keys_for(id);
    if !keys.is_empty() {
        let run = HANDLERS.with(|h| h.borrow().get(id).cloned());
        if let Some(run) = run {
            on_key(&keys, move |_| run());
        }
    }
}

/// As [`on`], with [`KeyOptions`] (`When::Always`, `repeat`) for the binding.
pub fn on_with(id: &str, options: KeyOptions, handler: impl Fn() + 'static) {
    register(id, handler);
    let keys = keys_for(id);
    if !keys.is_empty() {
        let run = HANDLERS.with(|h| h.borrow().get(id).cloned());
        if let Some(run) = run {
            on_key_with(&keys, options, move |_| run());
        }
    }
}

/// Registers `handler` for `id` without binding a key — for commands the
/// editor binds itself (Shift Right/Left) and dispatch-only commands.
pub fn register(id: &str, handler: impl Fn() + 'static) {
    HANDLERS.with(|h| h.borrow_mut().insert(id.to_owned(), std::rc::Rc::new(handler) as std::rc::Rc<dyn Fn()>));
}

/// Runs the handler bound command `id` was installed with, if any.
// Unused while the pinned nsh has no key events for the compound machine.
#[allow(dead_code)]
pub fn dispatch(id: &str) -> bool {
    let handler = HANDLERS.with(|h| h.borrow().get(id).cloned());
    match handler {
        Some(handler) => { handler(); true }
        None => false,
    }
}

/// Runs the bound command `command` names — a `MainMenu/…` path or one of a
/// command's `names` (`selectTab3`, `shiftRight`); for compound commands'
/// `execute`/`selectMenuItem:`.
// Unused while the pinned nsh has no key events for the compound machine.
#[allow(dead_code)]
pub fn dispatch_command(command: &str) -> bool {
    COMMANDS.iter()
        .find(|bound| bound.path == command || bound.names.contains(&command))
        .is_some_and(|bound| dispatch(bound.id))
}

/// Whether the command's effective key is active (the row's checkbox).
pub fn key_enabled(set: &BindingSet, bound: &Bound) -> bool {
    match binding_of(set, bound) {
        Some(b) if !b.enabled => !bound.keys.is_empty(),
        Some(b) => match b.shortcut() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => !bound.keys.is_empty(),
        },
        None => !bound.keys.is_empty(),
    }
}

/// The glyph text a row shows for its binding or default (`""` = no key).
pub fn display_of(set: &BindingSet, bound: &Bound) -> String {
    match binding_of(set, bound) {
        Some(b) if b.enabled => match b.shortcut() {
            Ok(Some(shortcut)) => shortcut.display(),
            Ok(None) => String::new(),
            Err(_) => bound_default_display(bound),
        },
        _ => bound_default_display(bound),
    }
}

fn bound_default_display(bound: &Bound) -> String {
    Stroke::parse_nsh(bound.keys).ok().map(|s| s.display()).unwrap_or_default()
}

// ------------------------------------------------------------------ mutate

/// A set-file name not yet taken ("Name Copy", "Name Copy 2", …).
fn unique_set_title(store: &SetStore, base: &str) -> String {
    let names = store.list_sets().unwrap_or_default().into_iter().map(|s| s.title).collect::<Vec<_>>();
    (1..).map(|n| if n == 1 { base.to_owned() } else { format!("{base} {n}") })
        .find(|name| !names.contains(name))
        .expect("a free set title")
}

/// `f` mutates the active set; a read-only shipped set is duplicated first and
/// the copy becomes the selected set. Saves and bumps afterwards.
pub fn mutate_set(f: impl FnOnce(&mut BindingSet)) {
    let Some(store) = store() else {
        state::notice("No key-bindings directory.".to_owned());
        return;
    };
    let mut set = store.load_set(&selected_set_name()).unwrap_or_else(|_| scratch_commands::bindings::default_set());
    if set.read_only {
        let title = unique_set_title(&store, &format!("{} Copy", set.title));
        set = set.duplicate(&title);
        preferences::write(|p| p.key_bindings.set = title);
    }
    f(&mut set);
    match store.save_set(&set) {
        Ok(_) => bump(),
        Err(error) => state::notice(format!("Key bindings could not be saved: {error}")),
    }
}

/// `f` mutates the global set (`_Global_.crBindings` is always writable).
pub fn mutate_global(f: impl FnOnce(&mut BindingSet)) {
    let Some(store) = store() else {
        state::notice("No key-bindings directory.".to_owned());
        return;
    };
    let Ok(mut set) = store.load_global() else {
        state::notice("The global key-binding set could not be read.".to_owned());
        return;
    };
    f(&mut set);
    match store.save_global(&set) {
        Ok(_) => bump(),
        Err(error) => state::notice(format!("Key bindings could not be saved: {error}")),
    }
}

/// Records `shortcut` on `bound` (`None` unbinds it): updates the matching
/// binding — by command name, then menu path — or inserts one.
// Unused while the pinned nsh has no key events for the recorder.
#[allow(dead_code)]
pub fn record(bound: &Bound, shortcut: Option<Shortcut>) {
    mutate_set(|set| {
        if let Some(b) = set.bindings.iter_mut().find(|b| matches(b, bound)) {
            b.set_shortcut(shortcut.as_ref());
            b.enabled = true;
        } else {
            let mut b = Binding::command(Some(bound.title), "", bound.path);
            b.set_shortcut(shortcut.as_ref());
            set.bindings.push(b);
        }
    });
}

/// The row's checkbox: `on` restores the default (or revives a disabled
/// binding's recorded key); `off` unbinds the item.
pub fn set_key_enabled(bound: &Bound, on: bool) {
    mutate_set(|set| {
        let index = set.bindings.iter().position(|b| matches(b, bound));
        match (index, on) {
            (Some(i), true) => {
                let b = &set.bindings[i];
                if b.enabled && b.key_equivalent.as_deref().unwrap_or("").is_empty() {
                    set.bindings.remove(i);
                } else {
                    set.bindings[i].enabled = true;
                }
            }
            (Some(i), false) => {
                let b = &mut set.bindings[i];
                b.enabled = true;
                b.key_equivalent = None;
            }
            (None, true) => {}
            (None, false) => set.bindings.push(Binding::command(Some(bound.title), "", bound.path)),
        }
    });
}

/// Removes the binding claiming `bound` (its default key comes back).
pub fn reset(bound: &Bound) {
    mutate_set(|set| set.bindings.retain(|b| !matches(b, bound)));
}

/// `f` mutates the binding `identifier` of the active (or the global) set.
pub fn mutate_binding(identifier: &str, global: bool, f: impl FnOnce(&mut Binding)) {
    let apply = |set: &mut BindingSet| {
        if let Some(b) = set.bindings.iter_mut().find(|b| b.identifier == identifier) { f(b); }
    };
    if global { mutate_global(apply) } else { mutate_set(apply) }
}

/// Adds a compound command binding to the active (or global) set; returns its
/// identifier for the pane to select.
pub fn add_compound(global: bool) -> String {
    let mut identifier = String::new();
    let apply = |set: &mut BindingSet| {
        let binding = Binding::compound("New Command", "", "[\n  \n]");
        identifier = binding.identifier.clone();
        set.bindings.push(binding);
    };
    if global { mutate_global(apply) } else { mutate_set(apply) }
    identifier
}

/// Adds an editor-command binding to the active set; returns its identifier.
pub fn add_command() -> String {
    let mut identifier = String::new();
    mutate_set(|set| {
        let binding = Binding::command(Some("New Binding"), "", "insertText");
        identifier = binding.identifier.clone();
        set.bindings.push(binding);
    });
    identifier
}

/// Removes the binding `identifier` of the active (or global) set.
pub fn delete_binding(identifier: &str, global: bool) {
    let apply = |set: &mut BindingSet| set.bindings.retain(|b| b.identifier != identifier);
    if global { mutate_global(apply) } else { mutate_set(apply) }
}

// -------------------------------------------------------------- set actions

/// Duplicates the set `name` and selects the copy; returns the new title.
pub fn duplicate_set(name: &str) -> Option<String> {
    let store = store()?;
    let source = store.load_set(name).ok()?;
    let title = unique_set_title(&store, &format!("{name} Copy"));
    let copy = source.duplicate(&title);
    match store.save_set(&copy) {
        Ok(_) => { select_set(&title); Some(title) }
        Err(error) => { state::notice(format!("Not duplicated: {error}")); None }
    }
}

/// Renames the user set `name`; the active set follows.
pub fn rename_set(name: &str, next: &str) {
    let Some(store) = store() else { return };
    let next = next.trim();
    if next.is_empty() || next == name { return; }
    let mut set = match store.load_set(name) {
        Ok(set) if !set.read_only => set,
        _ => { state::notice("A built-in set cannot be renamed.".to_owned()); return; }
    };
    set.title = next.to_owned();
    if let Err(error) = store.save_set(&set) {
        state::notice(format!("Not renamed: {error}"));
        return;
    }
    let _ = store.delete_set(name);
    select_set(next);
}

/// Deletes the user set `name` and selects Default.
pub fn delete_set(name: &str) {
    let Some(store) = store() else { return };
    match store.delete_set(name) {
        Ok(()) => select_set(scratch_commands::bindings::DEFAULT_SET_NAME),
        Err(error) => state::notice(format!("Not deleted: {error}")),
    }
}

/// Imports a `.crBindings`, `.json`, `.toml` or `KeyBindings.dict` file into
/// the store and selects the imported set.
pub fn import(path: PathBuf) {
    let Some(store) = store() else { return };
    match store.import(&path) {
        Ok((set, warnings)) => {
            for warning in warnings { state::notice(warning); }
            select_set(&set.title);
        }
        Err(error) => state::notice(format!("Not imported: {error}")),
    }
}

/// Writes the set `name` to `path` (the extension picks the format).
pub fn export(name: &str, path: PathBuf) {
    let Some(store) = store() else { return };
    let Ok(set) = store.load_set(name) else { return };
    if let Err(error) = store.export(&set, &path) {
        state::notice(format!("Not exported: {error}"));
    }
}

/// Reveals the sets directory in the Finder.
pub fn show_in_finder() {
    if let Some(store) = store() {
        let _ = store.ensure_dir();
        let _ = std::process::Command::new("open").arg(store.dir()).spawn();
    }
}

// The shortcut recorder (`stroke_of` on a `KeyEvent`) is gone: the pinned nsh
// exposes no raw key events.
