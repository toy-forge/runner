//! The menu bar and context menus as data: [`MenuModel`].
//!
//! [`MenuModel::standard`] is the ScratchRunner menu bar in CodeRunner 4.6 order
//! (App, File, Edit, Find, Text, Run, Diagnostics, View, Window, Help). Entries refer to commands
//! by [`CommandId`]; titles, enablement and shortcuts come from the [`CommandTable`] and the
//! active key bindings when [`MenuModel::resolve`] builds the tree the UI turns into NSH
//! `MenuItem`s.
//!
//! * **Alternates** ([`MenuItem::alternate`]) are Option-revealed variants of the preceding item.
//! * **Dynamic placeholders** ([`DynamicKind`]) are filled at resolve time by a
//!   [`DynamicMenuProvider`] (Open Recent, New from Template, Language, Syntax Mode, Text
//!   Encoding, per-language diagnostics, the Window list ...).
//! * **Fragments** are named injection points (`"file.end"`, `"view.end"`, see
//!   [`MenuModel::fragment_names`]) other crates fill with [`MenuModel::inject`].

use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::command::{CommandId, CommandTable, Role};
use crate::context::CommandContext;
use crate::shortcut::Shortcut;
use crate::table::{APP_NAME, ids};

/// A runtime-populated part of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DynamicKind {
    /// System Services submenu content (platform; usually left to the OS).
    Services,
    /// File > New from Template: templates nested by language.
    NewFromTemplate,
    /// File > Open Recent: recent documents.
    OpenRecent,
    /// Run > Language: the languages (grouped when the preference says so), check on current.
    Languages,
    /// Text > Syntax Mode: all grammar names, check on the document's grammar.
    SyntaxModes,
    /// Text > Text Encoding: encodings, check on the document's.
    TextEncodings,
    /// Window: one entry per open document, check on the key window.
    WindowList,
    /// Diagnostics: per-language items.
    DiagnosticsLanguages,
}

/// One entry of a menu.
#[derive(Debug, Clone)]
pub enum MenuEntry {
    /// A command.
    Item(MenuItem),
    /// A separator line.
    Separator,
    /// A submenu.
    Submenu(Submenu),
    /// A runtime-populated placeholder.
    Dynamic(DynamicKind),
    /// A named injection point; see [`MenuModel::inject`].
    Fragment(Cow<'static, str>),
}

/// A command in a menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// The command.
    pub command: CommandId,
    /// Shown only while Option (or the alternate modifier) is held, replacing the previous item.
    pub alternate: bool,
}

/// A submenu.
#[derive(Debug, Clone)]
pub struct Submenu {
    /// Title.
    pub title: Cow<'static, str>,
    /// Entries.
    pub entries: Vec<MenuEntry>,
    /// Visibility rule (`None` = visible).
    pub visible: Option<fn(&CommandContext) -> bool>,
}

/// A top-level menu.
#[derive(Debug, Clone)]
pub struct Menu {
    /// Stable id (`"file"`).
    pub id: &'static str,
    /// Title shown in the menu bar.
    pub title: Cow<'static, str>,
    /// Entries.
    pub entries: Vec<MenuEntry>,
}

/// The kinds of context menu / popup menu that are built from commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContextMenuKind {
    /// Right-click in the editor.
    Editor,
    /// Right-click in the file navigator.
    FileNavigator,
    /// The documentation view's gear menu.
    Docs,
    /// Right-click in the debugger variables view.
    DebugVariables,
    /// Right-click on a tab (Run / Stop).
    Tab,
    /// The Dock menu.
    Dock,
}

/// The complete menu description.
#[derive(Debug, Clone)]
pub struct MenuModel {
    menus: Vec<Menu>,
    context: BTreeMap<ContextMenuKind, Vec<MenuEntry>>,
    fragments: BTreeMap<String, Vec<MenuEntry>>,
}

// ---- builders ------------------------------------------------------------------------------

fn item(command: CommandId) -> MenuEntry {
    MenuEntry::Item(MenuItem { command, alternate: false })
}

fn alt(command: CommandId) -> MenuEntry {
    MenuEntry::Item(MenuItem { command, alternate: true })
}

fn sep() -> MenuEntry {
    MenuEntry::Separator
}

fn sub(title: &'static str, entries: Vec<MenuEntry>) -> MenuEntry {
    MenuEntry::Submenu(Submenu { title: Cow::Borrowed(title), entries, visible: None })
}

fn dynamic(kind: DynamicKind) -> MenuEntry {
    MenuEntry::Dynamic(kind)
}

fn frag(name: &'static str) -> MenuEntry {
    MenuEntry::Fragment(Cow::Borrowed(name))
}

fn menu(id: &'static str, title: &'static str, entries: Vec<MenuEntry>) -> Menu {
    Menu { id, title: Cow::Borrowed(title), entries }
}

/// Names of the fragment injection points of the standard model.
const FRAGMENTS: &[&str] = &[
    "app.end", "file.new", "file.end", "edit.end", "find.end", "text.end", "run.end", "diagnostics.end", "view.end", "window.end", "help.end",
];

impl MenuModel {
    /// The standard ScratchRunner menu bar.
    pub fn standard() -> MenuModel {
        let mut menus = vec![
            menu(
                "app",
                APP_NAME,
                vec![
                    item(ids::APP_ABOUT),
                    item(ids::APP_CHECK_UPDATES),
                    sep(),
                    item(ids::APP_PREFERENCES),
                    sep(),
                    sub("Services", vec![dynamic(DynamicKind::Services)]),
                    sep(),
                    item(ids::APP_HIDE),
                    item(ids::APP_HIDE_OTHERS),
                    item(ids::APP_SHOW_ALL),
                    frag("app.end"),
                    sep(),
                    item(ids::APP_QUIT),
                ],
            ),
            menu(
                "file",
                "File",
                vec![
                    item(ids::FILE_NEW_WINDOW),
                    item(ids::FILE_NEW_TAB),
                    sub("New from Template", vec![dynamic(DynamicKind::NewFromTemplate)]),
                    item(ids::FILE_NEW_FROM_CLIPBOARD),
                    alt(ids::FILE_NEW_FROM_CLIPBOARD_ALT),
                    frag("file.new"),
                    item(ids::FILE_OPEN),
                    sub("Open Recent", vec![dynamic(DynamicKind::OpenRecent), sep(), item(ids::FILE_CLEAR_RECENT)]),
                    item(ids::FILE_OPEN_QUICKLY),
                    item(ids::FILE_GO_TO_CURRENT_DOCUMENT),
                    sep(),
                    item(ids::FILE_CLOSE),
                    item(ids::FILE_CLOSE_WINDOW),
                    item(ids::FILE_CLOSE_OTHER_TABS),
                    item(ids::FILE_CLOSE_WITHOUT_SAVING),
                    sep(),
                    item(ids::FILE_SAVE),
                    item(ids::FILE_DUPLICATE),
                    item(ids::FILE_RENAME),
                    item(ids::FILE_MOVE_TO),
                    item(ids::FILE_SAVE_AS),
                    item(ids::FILE_REVERT),
                    sub("Revert To", vec![item(ids::FILE_REVERT_LAST_SAVED), item(ids::FILE_BROWSE_VERSIONS)]),
                    sep(),
                    item(ids::FILE_PAGE_SETUP),
                    item(ids::FILE_PRINT),
                    frag("file.end"),
                ],
            ),
            menu(
                "edit",
                "Edit",
                vec![
                    item(ids::EDIT_UNDO),
                    item(ids::EDIT_REDO),
                    sep(),
                    item(ids::EDIT_CUT),
                    item(ids::EDIT_COPY),
                    item(ids::EDIT_COPY_WITH_COLORS),
                    item(ids::EDIT_PASTE),
                    item(ids::EDIT_PASTE_PRESERVE),
                    item(ids::EDIT_DELETE),
                    item(ids::EDIT_SELECT_ALL),
                    sep(),
                    sub(
                        "Spelling and Grammar",
                        vec![
                            item(ids::EDIT_SPELL_PANEL),
                            item(ids::EDIT_SPELL_CHECK_NOW),
                            sep(),
                            item(ids::EDIT_SPELL_TYPING),
                            item(ids::EDIT_SPELL_GRAMMAR),
                            item(ids::EDIT_SPELL_AUTO),
                        ],
                    ),
                    sub("Transformations", vec![item(ids::EDIT_UPPERCASE), item(ids::EDIT_LOWERCASE), item(ids::EDIT_CAPITALIZE)]),
                    sub("Speech", vec![item(ids::EDIT_START_SPEAKING), item(ids::EDIT_STOP_SPEAKING)]),
                    frag("edit.end"),
                ],
            ),
            menu(
                "find",
                "Find",
                vec![
                    item(ids::FIND_FIND),
                    item(ids::FIND_REPLACE),
                    item(ids::FIND_NEXT),
                    item(ids::FIND_PREVIOUS),
                    item(ids::FIND_SELECT_NEXT),
                    item(ids::FIND_SELECT_PREVIOUS),
                    item(ids::FIND_SELECT_ALL),
                    item(ids::FIND_SELECT_WORD),
                    item(ids::FIND_PATTERN),
                    sep(),
                    sub(
                        "Find Mode",
                        vec![
                            item(ids::FIND_MODE_TEXT),
                            item(ids::FIND_MODE_REGEX),
                            item(ids::FIND_MODE_WORD),
                            item(ids::FIND_MODE_PREFIX),
                            item(ids::FIND_MODE_SUFFIX),
                            item(ids::FIND_MODE_WHITESPACE),
                            sep(),
                            item(ids::FIND_IGNORE_CASE),
                            item(ids::FIND_WRAP_AROUND),
                        ],
                    ),
                    sep(),
                    item(ids::FIND_SELECT_NEXT_RESULT),
                    item(ids::FIND_SELECT_PREVIOUS_RESULT),
                    item(ids::FIND_SELECT_ALL_RESULTS),
                    item(ids::FIND_SELECT_ALL_RESULTS_IN_SELECTION),
                    sep(),
                    item(ids::FIND_REPLACE_AND_NEXT),
                    item(ids::FIND_REPLACE_AND_PREVIOUS),
                    item(ids::FIND_REPLACE_ALL_IN_SELECTION),
                    item(ids::FIND_REPLACE_ALL),
                    sep(),
                    item(ids::FIND_JUMP_NEXT_SELECTION),
                    item(ids::FIND_JUMP_PREVIOUS_SELECTION),
                    sep(),
                    item(ids::FIND_IN_FILES),
                    item(ids::FIND_SELECTED_IN_FILES),
                    item(ids::FIND_SELECTED_IN_DOCUMENT),
                    frag("find.end"),
                ],
            ),
            menu(
                "text",
                "Text",
                vec![
                    item(ids::TEXT_COMPLETE),
                    item(ids::TEXT_LOOK_UP),
                    sep(),
                    item(ids::TEXT_AUTO_INDENT),
                    item(ids::TEXT_SHIFT_RIGHT),
                    item(ids::TEXT_SHIFT_LEFT),
                    item(ids::TEXT_TOGGLE_COMMENT),
                    sep(),
                    item(ids::TEXT_SELECT_COLUMN_UP),
                    item(ids::TEXT_SELECT_COLUMN_DOWN),
                    item(ids::TEXT_SPLIT_LINES),
                    item(ids::TEXT_SPLIT_WORDS),
                    item(ids::TEXT_SINGLE_SELECTION),
                    item(ids::TEXT_INSERT_SELECTION_NUMBER),
                    sep(),
                    item(ids::TEXT_SELECT_LINE),
                    item(ids::TEXT_SELECT_WORD),
                    item(ids::TEXT_BALANCE),
                    alt(ids::TEXT_BALANCE_IN),
                    alt(ids::TEXT_BALANCE_RIGHT),
                    alt(ids::TEXT_BALANCE_LEFT),
                    sub(
                        "Surround Selection With",
                        vec![
                            item(ids::TEXT_SURROUND_SINGLE),
                            item(ids::TEXT_SURROUND_DOUBLE),
                            item(ids::TEXT_SURROUND_PARENS),
                            item(ids::TEXT_SURROUND_BRACES),
                            item(ids::TEXT_SURROUND_BRACKETS),
                            sep(),
                            item(ids::TEXT_SURROUND_CUSTOM),
                        ],
                    ),
                    sep(),
                    item(ids::TEXT_DUPLICATE_LINE),
                    item(ids::TEXT_DELETE_LINE),
                    alt(ids::TEXT_DELETE_LINE_UP),
                    item(ids::TEXT_MOVE_LINE_UP),
                    item(ids::TEXT_MOVE_LINE_DOWN),
                    item(ids::TEXT_GO_TO_LINE),
                    sep(),
                    sub(
                        "Editor Options",
                        vec![
                            item(ids::OPT_SHOW_INVISIBLES),
                            item(ids::OPT_SHOW_LINE_NUMBERS),
                            item(ids::OPT_SHOW_PAGE_GUIDE),
                            item(ids::OPT_WRAP_LINES),
                            item(ids::OPT_SCROLL_PAST_END),
                            item(ids::OPT_HIGHLIGHT_LINE),
                            sep(),
                            item(ids::OPT_TEXT_PREFERENCES),
                        ],
                    ),
                    item(ids::TEXT_SHOW_COLOR_SCOPES),
                    sep(),
                    sub("Font", vec![item(ids::TEXT_SHOW_FONTS), sep(), item(ids::TEXT_FONT_BIGGER), item(ids::TEXT_FONT_SMALLER)]),
                    sub("Text Encoding", vec![item(ids::TEXT_ENCODING_AUTO), sep(), dynamic(DynamicKind::TextEncodings)]),
                    sub("Syntax Mode", vec![dynamic(DynamicKind::SyntaxModes), sep(), item(ids::TEXT_IMPORT_SYNTAX)]),
                    frag("text.end"),
                ],
            ),
            menu(
                "run",
                "Run",
                vec![
                    item(ids::RUN_RUN),
                    alt(ids::RUN_RUN_THIS_DOCUMENT),
                    item(ids::RUN_RUN_AND_DEBUG),
                    item(ids::RUN_RUN_WITHOUT_DEBUGGING),
                    item(ids::RUN_BUILD),
                    item(ids::RUN_STOP),
                    sep(),
                    sub(
                        "Debugger",
                        vec![
                            item(ids::DEBUG_TOGGLE_BREAKPOINT),
                            item(ids::DEBUG_CLEAR_BREAKPOINTS),
                            sep(),
                            item(ids::DEBUG_PAUSE),
                            item(ids::DEBUG_STEP_OVER),
                            item(ids::DEBUG_STEP_IN),
                            item(ids::DEBUG_STEP_OUT),
                        ],
                    ),
                    sep(),
                    item(ids::RUN_EDIT_SETTINGS),
                    item(ids::RUN_CLEAR_CONSOLE),
                    sep(),
                    sub("Language", vec![item(ids::RUN_NO_LANGUAGE), dynamic(DynamicKind::Languages)]),
                    item(ids::RUN_SELECT_LANGUAGE),
                    item(ids::RUN_EDIT_LANGUAGES),
                    frag("run.end"),
                ],
            ),
            menu(
                "diagnostics",
                "Diagnostics",
                vec![
                    item(ids::DIAG_APPLY_NEXT_FIX),
                    alt(ids::DIAG_APPLY_ALL_FIXES),
                    sep(),
                    item(ids::DIAG_NEXT),
                    alt(ids::DIAG_NEXT_ERROR),
                    item(ids::DIAG_PREVIOUS),
                    alt(ids::DIAG_PREVIOUS_ERROR),
                    sep(),
                    item(ids::DIAG_IGNORED_FOR_DOCUMENT),
                    item(ids::DIAG_IGNORED),
                    sep(),
                    item(ids::DIAG_ENABLE_FOR_LANGUAGE),
                    item(ids::DIAG_DISABLE_FOR_DOCUMENT),
                    item(ids::DIAG_RUN_NOW),
                    sub("Update Diagnostics", vec![item(ids::DIAG_UPDATE_MANUALLY), item(ids::DIAG_UPDATE_ON_EDIT), item(ids::DIAG_UPDATE_ON_RUN)]),
                    item(ids::DIAG_CLEAR),
                    dynamic(DynamicKind::DiagnosticsLanguages),
                    frag("diagnostics.end"),
                    sep(),
                    item(ids::DIAG_PREFERENCES),
                ],
            ),
            menu(
                "view",
                "View",
                vec![
                    item(ids::VIEW_TOGGLE_TAB_BAR),
                    sep(),
                    item(ids::VIEW_TOGGLE_SIDEBAR),
                    item(ids::VIEW_TOGGLE_CONSOLE),
                    item(ids::VIEW_TOGGLE_DOCS),
                    sep(),
                    item(ids::VIEW_HIDE_CONSOLE_AUTO),
                    sub("Console Position", vec![item(ids::VIEW_CONSOLE_BOTTOM), item(ids::VIEW_CONSOLE_RIGHT)]),
                    sep(),
                    item(ids::VIEW_TOGGLE_TOOLBAR),
                    item(ids::VIEW_CUSTOMIZE_TOOLBAR),
                    frag("view.end"),
                    sep(),
                    item(ids::VIEW_FULL_SCREEN),
                ],
            ),
            menu(
                "window",
                "Window",
                vec![
                    item(ids::WINDOW_MINIMIZE),
                    item(ids::WINDOW_ZOOM),
                    sep(),
                    item(ids::NAV_FORWARD),
                    item(ids::NAV_BACK),
                    sep(),
                    item(ids::WINDOW_PREV_TAB),
                    item(ids::WINDOW_NEXT_TAB),
                    item(ids::WINDOW_MOVE_TAB_LEFT),
                    item(ids::WINDOW_MOVE_TAB_RIGHT),
                    item(ids::WINDOW_MOVE_TAB_TO_NEW_WINDOW),
                    item(ids::WINDOW_MERGE_ALL),
                    frag("window.end"),
                    sep(),
                    item(ids::WINDOW_BRING_ALL_TO_FRONT),
                    sep(),
                    dynamic(DynamicKind::WindowList),
                ],
            ),
            menu(
                "help",
                "Help",
                vec![
                    item(ids::HELP_HELP),
                    sep(),
                    item(ids::HELP_WEBSITE),
                    item(ids::HELP_SUPPORT),
                    sep(),
                    item(ids::HELP_ATTRIBUTIONS),
                    frag("help.end"),
                ],
            ),
        ];

        // The "Revert to Saved" item and the "Revert To" submenu are alternatives (Versions on/off).
        if let Some(file) = menus.iter_mut().find(|m| m.id == "file") {
            for e in &mut file.entries {
                if let MenuEntry::Submenu(s) = e
                    && s.title == "Revert To"
                {
                    s.visible = Some(|c| c.autosave_versions);
                }
            }
        }

        let mut context = BTreeMap::new();
        context.insert(
            ContextMenuKind::Editor,
            vec![
                item(ids::CTX_EDIT_TOKEN),
                item(ids::TEXT_LOOK_UP),
                item(ids::CTX_SEARCH_DOCS_PROVIDER),
                item(ids::CTX_SEARCH_WEB),
                sep(),
                item(ids::CTX_COPY_LINK),
                item(ids::CTX_TRANSLATE),
                sep(),
                item(ids::EDIT_CUT),
                item(ids::EDIT_COPY),
                item(ids::EDIT_PASTE),
                sep(),
                item(ids::TEXT_COMPLETE),
                sep(),
                item(ids::TEXT_AUTO_INDENT),
                item(ids::TEXT_SHIFT_RIGHT),
                item(ids::TEXT_SHIFT_LEFT),
                item(ids::TEXT_TOGGLE_COMMENT),
                sep(),
                item(ids::TEXT_SHOW_COLOR_SCOPES),
                sep(),
                item(ids::DEBUG_TOGGLE_BREAKPOINT),
            ],
        );
        context.insert(
            ContextMenuKind::FileNavigator,
            vec![
                item(ids::NAV_OPEN),
                item(ids::NAV_OPEN_NEW_WINDOW),
                item(ids::NAV_OPEN_FINDER),
                item(ids::NAV_OPEN_TERMINAL),
                sep(),
                item(ids::NAV_SEARCH),
                sep(),
                item(ids::NAV_NEW_FILE),
                item(ids::NAV_NEW_FOLDER),
                item(ids::NAV_GO_TO_FOLDER),
                item(ids::NAV_REFRESH),
                sep(),
                item(ids::NAV_COPY),
                item(ids::NAV_PASTE),
                item(ids::NAV_RENAME),
                item(ids::NAV_DUPLICATE),
                item(ids::NAV_DELETE),
                sep(),
                sub("View Options", vec![item(ids::NAV_SHOW_HIDDEN), item(ids::NAV_SORT_FOLDERS_FIRST)]),
                sep(),
                item(ids::NAV_SHOW_IN_FINDER),
            ],
        );
        context.insert(
            ContextMenuKind::Docs,
            vec![
                item(ids::DOCS_OPEN_IN_BROWSER),
                item(ids::DOCS_COPY_URL),
                sep(),
                item(ids::DOCS_BOOKMARK),
                item(ids::DOCS_DEFINE_CONFIG),
                item(ids::DOCS_EDIT_CONFIGS),
                sep(),
                item(ids::DOCS_GO_TO_URL),
                item(ids::DOCS_FIND),
                item(ids::DOCS_RELOAD),
                sep(),
                item(ids::DOCS_STYLIZE),
                sub("Zoom", vec![item(ids::DOCS_ZOOM_IN), item(ids::DOCS_ZOOM_OUT), sep(), item(ids::DOCS_ZOOM_RESET)]),
            ],
        );
        context.insert(
            ContextMenuKind::DebugVariables,
            vec![
                item(ids::DEBUGVAR_PRINT),
                item(ids::DEBUGVAR_COPY),
                item(ids::DEBUGVAR_EDIT),
                item(ids::DEBUGVAR_WATCH),
                sep(),
                sub("View", vec![item(ids::DEBUGVAR_SHOW_TYPES), item(ids::DEBUGVAR_SHOW_ALL)]),
                sub("Sort By", vec![item(ids::DEBUGVAR_SORT_NAME), item(ids::DEBUGVAR_SORT_NATURAL), item(ids::DEBUGVAR_SORT_REVERSE)]),
                sep(),
                item(ids::DEBUGVAR_HOVER),
                item(ids::DEBUGVAR_ENABLE),
            ],
        );
        context.insert(ContextMenuKind::Tab, vec![item(ids::RUN_RUN), item(ids::RUN_STOP)]);
        context.insert(ContextMenuKind::Dock, vec![item(ids::DOCK_NEW_FILE), item(ids::FILE_NEW_WINDOW), sep(), dynamic(DynamicKind::WindowList)]);

        MenuModel { menus, context, fragments: BTreeMap::new() }
    }

    /// The top-level menus in menu-bar order.
    pub fn menus(&self) -> &[Menu] {
        &self.menus
    }

    /// The entries of a context menu.
    pub fn context_menu(&self, kind: ContextMenuKind) -> &[MenuEntry] {
        self.context.get(&kind).map_or(&[], Vec::as_slice)
    }

    /// The names of the fragment injection points.
    pub fn fragment_names() -> &'static [&'static str] {
        FRAGMENTS
    }

    /// Fills the injection point `name` with `entries` (replacing earlier content).
    /// Returns `false` (and does nothing) when the model has no such point.
    pub fn inject(&mut self, name: &str, entries: Vec<MenuEntry>) -> bool {
        if !FRAGMENTS.contains(&name) && !self.fragment_exists(name) {
            return false;
        }
        self.fragments.insert(name.to_owned(), entries);
        true
    }

    /// Adds a new fragment point at the end of the top-level menu `menu_id` (before nothing else
    /// moves) so other crates can create their own; returns `false` when the menu is unknown.
    pub fn add_fragment(&mut self, menu_id: &str, name: &'static str) -> bool {
        match self.menus.iter_mut().find(|m| m.id == menu_id) {
            Some(m) => {
                m.entries.push(MenuEntry::Fragment(Cow::Borrowed(name)));
                true
            }
            None => false,
        }
    }

    fn fragment_exists(&self, name: &str) -> bool {
        fn walk(entries: &[MenuEntry], name: &str) -> bool {
            entries.iter().any(|e| match e {
                MenuEntry::Fragment(n) => n == name,
                MenuEntry::Submenu(s) => walk(&s.entries, name),
                _ => false,
            })
        }
        self.menus.iter().any(|m| walk(&m.entries, name)) || self.context.values().any(|v| walk(v, name))
    }

    /// Every static command item with its menu path (`["File", "Save"]`), alternates included,
    /// in menu order. Fragments injected with [`MenuModel::inject`] are included.
    pub fn items(&self) -> Vec<MenuItemInfo> {
        let mut out = Vec::new();
        for m in &self.menus {
            self.collect(&m.entries, &mut vec![m.title.to_string()], &mut out);
        }
        out
    }

    fn collect(&self, entries: &[MenuEntry], path: &mut Vec<String>, out: &mut Vec<MenuItemInfo>) {
        for e in entries {
            match e {
                MenuEntry::Item(i) => {
                    let title = CommandTable::global().get(&i.command).map_or_else(|| i.command.to_string(), |d| d.title.to_owned());
                    let mut p = path.clone();
                    p.push(title);
                    out.push(MenuItemInfo { command: i.command.clone(), path: p, alternate: i.alternate });
                }
                MenuEntry::Submenu(s) => {
                    path.push(s.title.to_string());
                    self.collect(&s.entries, path, out);
                    path.pop();
                }
                MenuEntry::Fragment(n) => {
                    if let Some(f) = self.fragments.get(n.as_ref()) {
                        self.collect(f, path, out);
                    }
                }
                MenuEntry::Separator | MenuEntry::Dynamic(_) => {}
            }
        }
    }

    /// The menu path of a command, `"File/Save"` (first occurrence).
    pub fn path_of(&self, command: &CommandId) -> Option<String> {
        self.items().into_iter().find(|i| &i.command == command).map(|i| i.path.join("/"))
    }

    /// Finds the commands a `.crBindings` / `selectMenuItem:` menu path refers to.
    ///
    /// `pattern` may start with `MainMenu/`, uses `/` between submenu names and the item,
    /// and supports the wildcards `*` and `?` (within one path component). A pattern without
    /// `/` matches the item title in any menu. Titles compare with `...` equal to `…`.
    /// Results are in menu-bar order, so the first is what `selectMenuItem:` picks.
    pub fn find(&self, pattern: &str) -> Vec<MenuItemInfo> {
        let pattern = pattern.strip_prefix("MainMenu/").unwrap_or(pattern);
        let parts: Vec<&str> = pattern.split('/').collect();
        self.items()
            .into_iter()
            .filter(|i| {
                if parts.len() == 1 {
                    i.path.last().is_some_and(|t| wildcard_match(parts[0], t))
                } else {
                    i.path.len() == parts.len()
                        && parts.iter().zip(&i.path).enumerate().all(|(n, (pat, seg))| {
                            wildcard_match(pat, seg) || n == 0 && is_app_menu_alias(pat) && seg == APP_NAME
                        })
                }
            })
            .collect()
    }

    /// Resolves the model for `ctx`: evaluates enable/check/title state, fills in shortcuts from
    /// `shortcuts`, expands dynamic placeholders and fragments, and tidies separators.
    pub fn resolve(&self, ctx: &CommandContext, shortcuts: &dyn ShortcutSource, dynamic: &dyn DynamicMenuProvider) -> Vec<ResolvedMenu> {
        let r = Resolver { model: self, ctx, shortcuts, dynamic };
        self.menus
            .iter()
            .map(|m| ResolvedMenu { id: m.id, title: m.title.to_string(), entries: tidy(r.entries(&m.entries)) })
            .collect()
    }

    /// Resolves one context menu (see [`MenuModel::resolve`]).
    pub fn resolve_context(
        &self,
        kind: ContextMenuKind,
        ctx: &CommandContext,
        shortcuts: &dyn ShortcutSource,
        dynamic: &dyn DynamicMenuProvider,
    ) -> Vec<ResolvedEntry> {
        let r = Resolver { model: self, ctx, shortcuts, dynamic };
        tidy(r.entries(self.context_menu(kind)))
    }
}

/// Where a static menu item sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemInfo {
    /// The command.
    pub command: CommandId,
    /// Menu path: top menu title, submenu titles, item title.
    pub path: Vec<String>,
    /// Whether the item is an Option-revealed alternate.
    pub alternate: bool,
}

impl MenuItemInfo {
    /// The path as used by `.crBindings`: `MainMenu/File/Save`.
    pub fn binding_path(&self) -> String {
        format!("MainMenu/{}", self.path.join("/"))
    }
}

fn is_app_menu_alias(s: &str) -> bool {
    s == "CodeRunner" || s == APP_NAME
}

// ---- wildcard matching ---------------------------------------------------------------------

/// Normalises a menu title for comparison: `...` becomes `…`.
fn norm(s: &str) -> String {
    s.replace("...", "…")
}

/// Matches `text` against `pattern` where `*` is any run of characters and `?` any one
/// character; `...` and `…` are equivalent and matching is exact otherwise.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = norm(pattern).chars().collect();
    let t: Vec<char> = norm(text).chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ---- resolution ----------------------------------------------------------------------------

/// Supplies the effective shortcut of a command (default table, or the active key bindings).
pub trait ShortcutSource {
    /// The shortcut to show / install for `id`.
    fn shortcut_for(&self, id: &CommandId) -> Option<Shortcut>;
}

/// [`ShortcutSource`] that returns the table defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultShortcuts;

impl ShortcutSource for DefaultShortcuts {
    /// The shipped shortcut, except for platform-reserved role items (`Quit`, `Hide`,
    /// `Minimize`, full screen), whose shortcut the platform draws and handles itself.
    fn shortcut_for(&self, id: &CommandId) -> Option<Shortcut> {
        let def = CommandTable::global().get(id)?;
        let shortcut = def.default_shortcuts().into_iter().next()?;
        if def.role.is_some() && shortcut.strokes().iter().any(|s| s.reserved_reason().is_some()) {
            None
        } else {
            Some(shortcut)
        }
    }
}

/// One entry returned by a [`DynamicMenuProvider`].
#[derive(Debug, Clone, PartialEq)]
pub enum DynamicEntry {
    /// A selectable item.
    Item(DynamicItem),
    /// A separator.
    Separator,
    /// A nested submenu.
    Submenu {
        /// Submenu title.
        title: String,
        /// Its entries.
        entries: Vec<DynamicEntry>,
    },
}

/// A runtime menu item (a recent file, a language, an encoding ...).
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicItem {
    /// The command to run: typically [`CommandId::with_arg`].
    pub command: CommandId,
    /// Title.
    pub title: String,
    /// Check mark.
    pub checked: bool,
    /// Enabled.
    pub enabled: bool,
    /// Optional shortcut.
    pub shortcut: Option<Shortcut>,
}

impl DynamicItem {
    /// An enabled, unchecked item without shortcut.
    pub fn new(command: CommandId, title: impl Into<String>) -> DynamicItem {
        DynamicItem { command, title: title.into(), checked: false, enabled: true, shortcut: None }
    }
}

/// Fills [`DynamicKind`] placeholders.
pub trait DynamicMenuProvider {
    /// The entries for `kind` given the current context; empty when there is nothing to show.
    fn entries(&self, kind: DynamicKind, ctx: &CommandContext) -> Vec<DynamicEntry>;
}

/// A [`DynamicMenuProvider`] that provides nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDynamicEntries;

impl DynamicMenuProvider for NoDynamicEntries {
    fn entries(&self, _kind: DynamicKind, _ctx: &CommandContext) -> Vec<DynamicEntry> {
        Vec::new()
    }
}

/// A resolved top-level menu.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMenu {
    /// Stable id.
    pub id: &'static str,
    /// Title.
    pub title: String,
    /// Entries.
    pub entries: Vec<ResolvedEntry>,
}

/// A resolved entry: everything the UI needs to create a native item.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedEntry {
    /// A command item.
    Item(ResolvedItem),
    /// A separator.
    Separator,
    /// A submenu.
    Submenu {
        /// Title.
        title: String,
        /// False when it would be empty.
        enabled: bool,
        /// Entries.
        entries: Vec<ResolvedEntry>,
    },
}

/// A resolved menu item.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedItem {
    /// The command.
    pub command: CommandId,
    /// Title for the current context.
    pub title: String,
    /// Effective shortcut.
    pub shortcut: Option<Shortcut>,
    /// Enabled.
    pub enabled: bool,
    /// Check mark.
    pub checked: bool,
    /// False when the rule hides the item.
    pub visible: bool,
    /// Option-revealed alternate of the previous item.
    pub alternate: bool,
    /// The platform role to use instead of a custom item, if any.
    pub role: Option<Role>,
}

struct Resolver<'a> {
    model: &'a MenuModel,
    ctx: &'a CommandContext,
    shortcuts: &'a dyn ShortcutSource,
    dynamic: &'a dyn DynamicMenuProvider,
}

impl Resolver<'_> {
    fn entries(&self, entries: &[MenuEntry]) -> Vec<ResolvedEntry> {
        let mut out = Vec::new();
        for e in entries {
            match e {
                MenuEntry::Item(i) => {
                    if let Some(r) = self.item(i) {
                        out.push(ResolvedEntry::Item(r));
                    }
                }
                MenuEntry::Separator => out.push(ResolvedEntry::Separator),
                MenuEntry::Submenu(s) => {
                    if s.visible.is_some_and(|v| !v(self.ctx)) {
                        continue;
                    }
                    let inner = tidy(self.entries(&s.entries));
                    out.push(ResolvedEntry::Submenu { title: s.title.to_string(), enabled: !inner.is_empty(), entries: inner });
                }
                MenuEntry::Dynamic(k) => out.extend(self.dynamic_entries(self.dynamic.entries(*k, self.ctx))),
                MenuEntry::Fragment(n) => {
                    if let Some(f) = self.model.fragments.get(n.as_ref()) {
                        out.extend(self.entries(f));
                    }
                }
            }
        }
        out
    }

    fn item(&self, i: &MenuItem) -> Option<ResolvedItem> {
        let def = CommandTable::global().get(&i.command)?;
        let st = def.state(self.ctx);
        Some(ResolvedItem {
            command: i.command.clone(),
            title: st.title.into_owned(),
            shortcut: self.shortcuts.shortcut_for(&i.command),
            enabled: st.enabled,
            checked: st.checked,
            visible: st.visible,
            alternate: i.alternate,
            role: def.role,
        })
    }

    fn dynamic_entries(&self, entries: Vec<DynamicEntry>) -> Vec<ResolvedEntry> {
        entries
            .into_iter()
            .map(|e| match e {
                DynamicEntry::Separator => ResolvedEntry::Separator,
                DynamicEntry::Item(d) => ResolvedEntry::Item(ResolvedItem {
                    command: d.command,
                    title: d.title,
                    shortcut: d.shortcut,
                    enabled: d.enabled,
                    checked: d.checked,
                    visible: true,
                    alternate: false,
                    role: None,
                }),
                DynamicEntry::Submenu { title, entries } => {
                    let inner = self.dynamic_entries(entries);
                    ResolvedEntry::Submenu { title, enabled: !inner.is_empty(), entries: inner }
                }
            })
            .collect()
    }
}

/// Removes leading, trailing and doubled separators, ignoring invisible items.
fn tidy(entries: Vec<ResolvedEntry>) -> Vec<ResolvedEntry> {
    let mut out: Vec<ResolvedEntry> = Vec::with_capacity(entries.len());
    // Alternates and hidden items do not count as content between separators.
    for e in entries {
        match &e {
            ResolvedEntry::Separator => {
                if matches!(out.last(), None | Some(ResolvedEntry::Separator)) {
                    continue;
                }
            }
            ResolvedEntry::Item(i) if !i.visible && !i.alternate => continue,
            _ => {}
        }
        out.push(e);
    }
    while matches!(out.last(), Some(ResolvedEntry::Separator)) {
        out.pop();
    }
    out
}

impl ResolvedEntry {
    /// Visits every item in this entry (depth first).
    pub fn for_each_item(&self, f: &mut dyn FnMut(&ResolvedItem)) {
        match self {
            ResolvedEntry::Item(i) => f(i),
            ResolvedEntry::Separator => {}
            ResolvedEntry::Submenu { entries, .. } => entries.iter().for_each(|e| e.for_each_item(f)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("Find*", "Find Next"));
        assert!(wildcard_match("Find ?ext", "Find Next"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("F*d*t", "Find Next"));
        assert!(!wildcard_match("Find", "Find Next"));
        assert!(wildcard_match("Find...", "Find…"));
        assert!(!wildcard_match("Find?", "Find"));
        assert!(wildcard_match("", ""));
        assert!(wildcard_match("a*", "a"));
    }

    #[test]
    fn top_level_order() {
        let m = MenuModel::standard();
        let titles: Vec<_> = m.menus().iter().map(|m| m.title.as_ref()).collect();
        assert_eq!(titles, [APP_NAME, "File", "Edit", "Find", "Text", "Run", "Diagnostics", "View", "Window", "Help"]);
    }

    struct Provider;

    impl DynamicMenuProvider for Provider {
        fn entries(&self, kind: DynamicKind, _ctx: &CommandContext) -> Vec<DynamicEntry> {
            match kind {
                DynamicKind::OpenRecent => vec![
                    DynamicEntry::Item(DynamicItem::new(CommandId::with_arg("file.openRecent", "a.txt"), "a.txt")),
                    DynamicEntry::Separator,
                    DynamicEntry::Item(DynamicItem::new(ids::FILE_CLEAR_RECENT.clone(), "Clear Menu")),
                ],
                DynamicKind::WindowList => vec![DynamicEntry::Item(DynamicItem::new(CommandId::with_arg("window.select", "1"), "Doc"))],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn resolve_fills_dynamic_menus_and_tidies_separators() {
        let model = MenuModel::standard();
        let ctx = CommandContext::new();
        let menus = model.resolve(&ctx, &DefaultShortcuts, &Provider);
        assert_eq!(menus.len(), 10);
        let file = menus.iter().find(|m| m.id == "file").unwrap();
        let mut titles = Vec::new();
        file.entries.iter().for_each(|e| e.for_each_item(&mut |i| titles.push(i.title.clone())));
        assert!(titles.contains(&"a.txt".to_owned()));
        assert!(titles.contains(&"Save".to_owned()));
        // The implicit "Revert to Saved" (no Versions) vs the "Revert To" submenu.
        let has_revert_to = file.entries.iter().any(|e| matches!(e, ResolvedEntry::Submenu { title, .. } if title == "Revert To"));
        assert!(has_revert_to, "Autosave & Versions is on by default");
        assert!(!titles.contains(&"Revert to Saved".to_owned()));
        let window = menus.iter().find(|m| m.id == "window").unwrap();
        let mut window_titles = Vec::new();
        window.entries.iter().for_each(|e| e.for_each_item(&mut |i| window_titles.push(i.title.clone())));
        assert!(window_titles.contains(&"Doc".to_owned()));
    }

    #[test]
    fn fragments_can_be_injected_and_added() {
        let mut model = MenuModel::standard();
        assert!(model.inject("file.end", vec![item(ids::HELP_HELP)]));
        assert!(!model.inject("nope", vec![]));
        assert!(model.add_fragment("help", "help.extra"));
        assert!(model.inject("help.extra", vec![item(ids::HELP_WEBSITE)]));
        let paths: Vec<String> = model.items().into_iter().map(|i| i.path.join("/")).collect();
        assert!(paths.contains(&"File/ScratchRunner Help".to_owned()));
        assert!(paths.contains(&"Help/Visit ScratchRunner Website".to_owned()));
    }

    #[test]
    fn alternate_items_are_marked() {
        let model = MenuModel::standard();
        let items = model.items();
        assert!(items.iter().any(|i| i.command == ids::RUN_RUN_THIS_DOCUMENT && i.alternate));
        assert!(items.iter().any(|i| i.command == ids::FILE_NEW_FROM_CLIPBOARD && !i.alternate));
    }
}
