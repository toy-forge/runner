//! Command identity and metadata: [`CommandId`], [`CommandDef`], [`ItemState`] and the
//! lookup facade [`CommandTable`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::context::CommandContext;
use crate::shortcut::{Shortcut, ShortcutError};

/// Stable identifier of a command.
///
/// Static commands are dotted ids such as `"file.save"` declared in [`crate::ids`]. Commands
/// created at run time carry an argument after a colon: `"language.set:<uuid>"`,
/// `"window.select:<number>"`; see [`CommandId::with_arg`] and [`CommandId::parts`].
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(Cow<'static, str>);

impl CommandId {
    /// A static id (usable in `const` items).
    pub const fn from_static(id: &'static str) -> CommandId {
        CommandId(Cow::Borrowed(id))
    }

    /// An id from any string (parsed from a file, or built at run time).
    pub fn new(id: impl Into<String>) -> CommandId {
        CommandId(Cow::Owned(id.into()))
    }

    /// A dynamic command: `base` plus an argument, `base:arg`.
    pub fn with_arg(base: &str, arg: &str) -> CommandId {
        CommandId(Cow::Owned(format!("{base}:{arg}")))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Splits `base:arg` into `("base", Some("arg"))`; static ids give `(id, None)`.
    pub fn parts(&self) -> (&str, Option<&str>) {
        match self.0.split_once(':') {
            Some((b, a)) => (b, Some(a)),
            None => (&self.0, None),
        }
    }

    /// The id of the editor command `name` (`editor.moveLeft`).
    pub fn editor(name: &str) -> CommandId {
        CommandId::new(format!("editor.{name}"))
    }
}

impl fmt::Debug for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommandId({})", self.0)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CommandId {
    fn from(s: &str) -> CommandId {
        CommandId::new(s)
    }
}

/// What a command is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    /// An item of the menu bar (listed under "Menu Items" in the Key Bindings pane).
    Menu,
    /// An editor command by name ("Commands" tab): cursor movement, line editing ...
    Editor,
    /// A window command reachable only by name (`selectTab1`).
    Window,
    /// An item that only appears in context menus / popups.
    Context,
}

/// Where a shortcut is active; two commands may share a shortcut only when their scopes are
/// different and neither is [`Scope::App`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Everywhere (menu bar key equivalents).
    App,
    /// Focus in the editor.
    Editor,
    /// Focus in the file navigator.
    Sidebar,
    /// Focus in the console.
    Console,
    /// Focus in the documentation view.
    Docs,
    /// Focus in the debugger variables view.
    Debugger,
}

impl Scope {
    /// Whether a shortcut in `self` can fire at the same time as one in `other`.
    pub fn overlaps(self, other: Scope) -> bool {
        self == other || self == Scope::App || other == Scope::App
    }
}

/// A menu item the platform provides and labels itself (NSH `MenuRole`): the UI should use the
/// role rather than a custom item, and must not bind its (reserved) shortcut itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// About panel.
    About,
    /// Hide the app (⌘H).
    Hide,
    /// Hide other apps (⌥⌘H).
    HideOthers,
    /// Quit (⌘Q).
    Quit,
    /// Minimize (⌘M).
    Minimize,
    /// Zoom.
    Zoom,
    /// Enter/exit full screen (⌃⌘F).
    FullScreen,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Paste.
    Paste,
    /// Select all.
    SelectAll,
}

/// Enable / check / title state of a menu item for one [`CommandContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemState<'a> {
    /// The item can be chosen.
    pub enabled: bool,
    /// The item shows a check mark.
    pub checked: bool,
    /// The item is shown at all (alternates and companions can hide).
    pub visible: bool,
    /// The title to show (possibly dynamic).
    pub title: Cow<'a, str>,
}

/// Predicate over the context.
pub type Predicate = fn(&CommandContext) -> bool;
/// Dynamic title rule.
pub type TitleFn = for<'a> fn(&'a CommandContext) -> Cow<'a, str>;

/// Static metadata of one command.
#[derive(Clone)]
pub struct CommandDef {
    /// Stable id.
    pub id: CommandId,
    /// Default title (menus show [`CommandDef::state`]'s title).
    pub title: &'static str,
    /// What the command is.
    pub kind: CommandKind,
    /// CodeRunner key-equivalent strings (empty = none), first one is shown in menus.
    keys: [&'static str; 2],
    /// Name used by `.crBindings` `command` entries and compound commands ("" = not bindable by name).
    name: &'static str,
    /// Where the shortcut is active.
    pub scope: Scope,
    /// Platform menu role, when the platform labels the item.
    pub role: Option<Role>,
    /// Enabled rule (`None` = always enabled).
    pub enabled: Option<Predicate>,
    /// Check-mark rule (`None` = not a check item).
    pub checked: Option<Predicate>,
    /// Visibility rule (`None` = always visible).
    pub visible: Option<Predicate>,
    /// Dynamic title rule (`None` = static title).
    pub title_fn: Option<TitleFn>,
    /// Excluded from the key-binding editor (dynamic / system items).
    pub excluded_from_editor: bool,
}

impl CommandDef {
    /// A menu command with defaults: always enabled, no shortcut.
    pub const fn new(id: &'static str, title: &'static str) -> CommandDef {
        CommandDef {
            id: CommandId::from_static(id),
            title,
            kind: CommandKind::Menu,
            keys: ["", ""],
            name: "",
            scope: Scope::App,
            role: None,
            enabled: None,
            checked: None,
            visible: None,
            title_fn: None,
            excluded_from_editor: false,
        }
    }

    /// Sets the primary default shortcut (CodeRunner key-equivalent string).
    pub const fn key(mut self, k: &'static str) -> CommandDef {
        self.keys[0] = k;
        self
    }

    /// Sets a second default shortcut.
    pub const fn key2(mut self, k: &'static str) -> CommandDef {
        self.keys[1] = k;
        self
    }

    /// Sets the binding name.
    pub const fn name(mut self, n: &'static str) -> CommandDef {
        self.name = n;
        self
    }

    /// Sets the kind.
    pub const fn kind(mut self, k: CommandKind) -> CommandDef {
        self.kind = k;
        self
    }

    /// Sets the scope.
    pub const fn scope(mut self, s: Scope) -> CommandDef {
        self.scope = s;
        self
    }

    /// Sets the platform role.
    pub const fn role(mut self, r: Role) -> CommandDef {
        self.role = Some(r);
        self
    }

    /// Sets the enabled rule.
    pub const fn enabled(mut self, p: Predicate) -> CommandDef {
        self.enabled = Some(p);
        self
    }

    /// Sets the check rule.
    pub const fn checked(mut self, p: Predicate) -> CommandDef {
        self.checked = Some(p);
        self
    }

    /// Sets the visibility rule.
    pub const fn visible(mut self, p: Predicate) -> CommandDef {
        self.visible = Some(p);
        self
    }

    /// Sets the dynamic title rule.
    pub const fn title_fn(mut self, f: TitleFn) -> CommandDef {
        self.title_fn = Some(f);
        self
    }

    /// Excludes the command from the key-binding editor.
    pub const fn hidden_in_editor(mut self) -> CommandDef {
        self.excluded_from_editor = true;
        self
    }

    /// The binding name (`insertNewlineBelow`), if the command has one.
    pub fn binding_name(&self) -> Option<&'static str> {
        (!self.name.is_empty()).then_some(self.name)
    }

    /// The default shortcuts, parsed. Invalid strings are skipped (a test guarantees there are none).
    pub fn default_shortcuts(&self) -> Vec<Shortcut> {
        self.keys.iter().filter(|k| !k.is_empty()).filter_map(|k| Shortcut::parse_crbindings(k).ok()).collect()
    }

    /// The default shortcut strings as written in the table.
    pub fn default_key_strings(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.keys.iter().copied().filter(|k| !k.is_empty())
    }

    /// Parses the first default shortcut, reporting errors.
    pub fn primary_shortcut(&self) -> Result<Option<Shortcut>, ShortcutError> {
        match self.keys[0] {
            "" => Ok(None),
            k => Shortcut::parse_crbindings(k).map(Some),
        }
    }

    /// Enabled / checked / visible / title for `ctx`.
    pub fn state<'a>(&self, ctx: &'a CommandContext) -> ItemState<'a> {
        ItemState {
            enabled: self.enabled.is_none_or(|p| p(ctx)),
            checked: self.checked.is_some_and(|p| p(ctx)),
            visible: self.visible.is_none_or(|p| p(ctx)),
            title: match self.title_fn {
                Some(f) => f(ctx),
                None => Cow::Borrowed(self.title),
            },
        }
    }
}

impl fmt::Debug for CommandDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandDef").field("id", &self.id).field("title", &self.title).finish()
    }
}

/// The static table of all commands.
///
/// ```
/// use scratch_commands::{CommandTable, ids, CommandContext};
/// let table = CommandTable::global();
/// let save = table.get(&ids::FILE_SAVE).expect("known command");
/// assert_eq!(save.title, "Save");
/// assert_eq!(save.primary_shortcut().unwrap().unwrap().display(), "⌘S");
/// let ctx = CommandContext::new();
/// assert!(save.state(&ctx).enabled);
/// ```
pub struct CommandTable {
    defs: Vec<&'static CommandDef>,
    by_id: HashMap<&'static str, usize>,
    by_name: HashMap<&'static str, usize>,
}

impl CommandTable {
    /// The process-wide table.
    pub fn global() -> &'static CommandTable {
        static TABLE: OnceLock<CommandTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            let defs: Vec<&'static CommandDef> = crate::table::all_defs().collect();
            let mut by_id = HashMap::new();
            let mut by_name = HashMap::new();
            for (i, d) in defs.iter().enumerate() {
                by_id.insert(d.id.as_str_static(), i);
                if let Some(n) = d.binding_name() {
                    by_name.entry(n).or_insert(i);
                }
            }
            CommandTable { defs, by_id, by_name }
        })
    }

    /// All commands in table order.
    pub fn iter(&self) -> impl Iterator<Item = &'static CommandDef> + '_ {
        self.defs.iter().copied()
    }

    /// Number of commands.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// Whether the table is empty (never).
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Looks a command up by id. Dynamic ids (`base:arg`) look up `base`.
    pub fn get(&self, id: &CommandId) -> Option<&'static CommandDef> {
        let key = id.parts().0;
        self.by_id.get(key).map(|&i| self.defs[i])
    }

    /// Looks a command up by its binding name (`insertNewlineBelow`, `selectTab3`, `undo`).
    pub fn by_name(&self, name: &str) -> Option<&'static CommandDef> {
        self.by_name.get(name).map(|&i| self.defs[i])
    }

    /// Commands of one kind.
    pub fn of_kind(&self, kind: CommandKind) -> impl Iterator<Item = &'static CommandDef> + '_ {
        self.iter().filter(move |d| d.kind == kind)
    }
}

impl CommandId {
    /// Static id text; only valid for ids made with [`CommandId::from_static`].
    pub(crate) fn as_str_static(&self) -> &'static str {
        match &self.0 {
            Cow::Borrowed(s) => s,
            // Table ids are always static; leak-free fallback for the impossible case.
            Cow::Owned(_) => "",
        }
    }
}
