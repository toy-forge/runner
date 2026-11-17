//! Resolving binding sets into runtime lookups: [`KeyMap`].
//!
//! Precedence, highest first: the active set, the `_Global_` set, the menu-bar default
//! shortcuts. A binding that targets a menu command replaces that command's default shortcut
//! (an empty key equivalent removes it). Bindings to a key table (`compoundCommandDictionary`)
//! contribute one lookup per table key.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use super::BindingSet;
use crate::command::{CommandId, CommandTable, Scope};
use crate::compound::{CommandLibrary, CommandList, CompoundCommand, Root};
use crate::menu::{MenuModel, ShortcutSource};
use crate::shortcut::{Shortcut, ShortcutError};

/// Where a lookup sends the key.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A menu, editor or window command.
    Command(CommandId),
    /// A command list of a compound command, ready for `Machine::start`.
    CompoundList {
        /// Display name (binding title, or the table key for dictionary entries).
        title: String,
        /// The list.
        list: Arc<CommandList>,
    },
}

/// Which layer produced an entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// A menu-bar default from the command table.
    Default,
    /// The selected binding set.
    Set(String),
    /// The `_Global_` compound-command set.
    GlobalSet(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("Default"),
            Source::Set(name) => write!(f, "Set {name:?}"),
            Source::GlobalSet(name) => write!(f, "Global {name:?}"),
        }
    }
}

/// One resolved binding.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyMapEntry {
    /// The keystroke or chord.
    pub shortcut: Shortcut,
    /// What to do.
    pub target: Target,
    /// Where it came from.
    pub source: Source,
    /// Focus area in which it is active.
    pub scope: Scope,
    /// Human-readable name (binding title or command title).
    pub title: String,
}

impl KeyMapEntry {
    /// A short description for conflict reports.
    pub fn label(&self) -> String {
        format!("{}: {}", self.source, self.title)
    }
}

/// A problem found while resolving a set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMapIssue {
    /// A key equivalent does not parse.
    InvalidKey {
        /// The binding's display name.
        title: String,
        /// The key string.
        key: String,
        /// Why it is invalid.
        error: String,
    },
    /// An editor/window command name is not in the command table.
    UnknownCommand {
        /// The binding's display name.
        title: String,
        /// The command name.
        command: String,
    },
    /// A menu path matches no static menu item.
    UnknownMenuPath {
        /// The binding's display name.
        title: String,
        /// The path.
        path: String,
    },
    /// A menu path points into a runtime-populated menu (Open Recent, Language ...).
    DynamicMenuPath {
        /// The binding's display name.
        title: String,
        /// The path.
        path: String,
    },
    /// A compound command does not parse.
    InvalidCompound {
        /// The binding's display name.
        title: String,
        /// Why.
        error: String,
    },
    /// A binding has both `command` and compound code.
    BothTargets {
        /// The binding's display name.
        title: String,
    },
    /// A binding has neither a command nor compound code.
    NoTarget {
        /// The binding's display name.
        title: String,
    },
    /// A key-table compound command also has its own key equivalent.
    UnsupportedCompoundTrigger {
        /// The binding's display name.
        title: String,
    },
}

impl fmt::Display for KeyMapIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyMapIssue::InvalidKey { title, key, error } => write!(f, "{title}: invalid key {key:?}: {error}"),
            KeyMapIssue::UnknownCommand { title, command } => write!(f, "{title}: unknown command {command:?}"),
            KeyMapIssue::UnknownMenuPath { title, path } => write!(f, "{title}: menu path {path:?} does not exist"),
            KeyMapIssue::DynamicMenuPath { title, path } => {
                write!(f, "{title}: {path:?} is in a runtime-populated menu and cannot be bound statically")
            }
            KeyMapIssue::InvalidCompound { title, error } => write!(f, "{title}: {error}"),
            KeyMapIssue::BothTargets { title } => write!(f, "{title}: has both a command and compound code"),
            KeyMapIssue::NoTarget { title } => write!(f, "{title}: has neither a command nor compound code"),
            KeyMapIssue::UnsupportedCompoundTrigger { title } => {
                write!(f, "{title}: a key-table compound command cannot also have a key equivalent")
            }
        }
    }
}

/// How two bindings clash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// The same key or chord.
    SameShortcut,
    /// One shortcut is the beginning of the other (`j` and `j j`).
    Prefix,
}

/// Two enabled bindings that can fire in overlapping scopes with the same keys.
///
/// Shadowing a menu-bar default from a set is not a conflict: the set resolves first. The
/// menu-bar defaults themselves are checked for duplicates by the command-table tests.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// The shortcut of the first binding.
    pub shortcut: Shortcut,
    /// The other shortcut (different only for [`ConflictKind::Prefix`]).
    pub other: Shortcut,
    /// The kind of clash.
    pub kind: ConflictKind,
    /// The first binding.
    pub first: String,
    /// The second binding.
    pub second: String,
    /// Their scopes.
    pub scopes: (Scope, Scope),
}

/// What a key sequence currently means.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    /// Nothing is bound.
    None,
    /// A longer binding starts with these strokes; wait for the next key.
    Prefix,
    /// A binding matched.
    Match(KeyMapEntry),
}

impl Lookup {
    /// The matched entry, if any.
    pub fn entry(&self) -> Option<&KeyMapEntry> {
        match self {
            Lookup::Match(e) => Some(e),
            _ => None,
        }
    }
}

/// The resolved key map of one window/application state.
#[derive(Debug, Clone)]
pub struct KeyMap {
    entries: Vec<KeyMapEntry>,
    overrides: HashMap<CommandId, Option<Shortcut>>,
    defaults: HashMap<CommandId, Shortcut>,
    issues: Vec<KeyMapIssue>,
    conflicts: Vec<Conflict>,
    library: Arc<CommandLibrary>,
}

fn key_error(binding: &super::Binding, e: ShortcutError) -> KeyMapIssue {
    KeyMapIssue::InvalidKey {
        title: binding.display_name(),
        key: binding.key_equivalent.clone().unwrap_or_default(),
        error: e.to_string(),
    }
}

fn is_reserved_role(id: &CommandId, shortcut: &Shortcut) -> bool {
    CommandTable::global().get(id).is_some_and(|d| d.role.is_some())
        && shortcut.strokes().iter().any(|s| s.reserved_reason().is_some())
}

fn scope_of(id: &CommandId) -> Scope {
    CommandTable::global().get(id).map_or(Scope::App, |d| d.scope)
}

fn is_dynamic_path(path: &str) -> bool {
    let p = path.strip_prefix("MainMenu/").unwrap_or(path);
    p.starts_with("File/New from Template")
        || p.starts_with("File/Open Recent")
        || p.starts_with("File/Revert To")
        || p.starts_with("Run/Language")
        || p.starts_with("Text/Syntax Mode")
        || p.starts_with("Text/Text Encoding")
}

impl KeyMap {
    /// Resolves the default shortcuts, `active` and the `_Global_` set.
    pub fn build(menu: &MenuModel, active: Option<&BindingSet>, global: Option<&BindingSet>) -> KeyMap {
        let mut map = KeyMap {
            entries: Vec::new(),
            overrides: HashMap::new(),
            defaults: HashMap::new(),
            issues: Vec::new(),
            conflicts: Vec::new(),
            library: Arc::new(CommandLibrary::new()),
        };
        let mut claimed: HashSet<CommandId> = HashSet::new();

        // 1. Menu-bar defaults.
        for def in CommandTable::global().iter() {
            let defaults = def.default_shortcuts();
            if let Some(first) = defaults.first() {
                map.defaults.insert(def.id.clone(), first.clone());
            }
            for shortcut in defaults {
                map.entries.push(KeyMapEntry {
                    shortcut,
                    target: Target::Command(def.id.clone()),
                    source: Source::Default,
                    scope: def.scope,
                    title: def.title.to_owned(),
                });
            }
        }

        // 2. Selected set, then 3. global compounds.
        if let Some(set) = active {
            map.apply_set(menu, set, false, &mut claimed);
            map.library = BindingSet::library_of(&[set]);
        }
        if let Some(set) = global {
            map.apply_set(menu, set, true, &mut claimed);
            let sets: Vec<&BindingSet> = active.into_iter().chain(std::iter::once(set)).collect();
            map.library = BindingSet::library_of(&sets);
        }

        // Drop default entries for commands whose shortcut a set took over.
        if !claimed.is_empty() {
            map.entries.retain(|e| match &e.target {
                Target::Command(id) => !matches!(e.source, Source::Default) || !claimed.contains(id),
                Target::CompoundList { .. } => true,
            });
        }

        // Precedence order for lookups: active set, global set, menu defaults.
        map.entries.sort_by_key(|e| matches!(e.source, Source::Default));
        map.find_conflicts();
        map
    }

    fn apply_set(&mut self, menu: &MenuModel, set: &BindingSet, global: bool, claimed: &mut HashSet<CommandId>) {
        let source = if global { Source::GlobalSet(set.title.clone()) } else { Source::Set(set.title.clone()) };
        for binding in set.bindings.iter().filter(|b| b.enabled) {
            let name = binding.display_name();
            if binding.compound_code.is_some() {
                if binding.command.is_some() {
                    self.issues.push(KeyMapIssue::BothTargets { title: name });
                }
                self.apply_compound(binding, source.clone());
                continue;
            }
            let Some(command) = &binding.command else {
                self.issues.push(KeyMapIssue::NoTarget { title: name });
                continue;
            };
            let Some((target, scope)) = self.resolve_command(menu, binding, command) else { continue };
            let shortcut = match binding.shortcut() {
                Ok(s) => s,
                Err(e) => {
                    self.issues.push(key_error(binding, e));
                    continue;
                }
            };
            let Target::Command(id) = &target else { continue };
            if global && claimed.contains(id) {
                continue; // the selected set wins over the global set
            }
            claimed.insert(id.clone());
            if let Some(sc) = &shortcut {
                self.overrides.insert(id.clone(), Some(sc.clone()));
                self.entries.push(KeyMapEntry {
                    shortcut: sc.clone(),
                    target,
                    source: source.clone(),
                    scope,
                    title: name,
                });
            } else {
                self.overrides.insert(id.clone(), None);
            }
        }
    }

    fn apply_compound(&mut self, binding: &super::Binding, source: Source) {
        let name = binding.display_name();
        let code = binding.compound_code.as_deref().unwrap_or_default();
        let command = match CompoundCommand::parse(code) {
            Ok(c) => c,
            Err(e) => {
                self.issues.push(KeyMapIssue::InvalidCompound { title: name, error: e.to_string() });
                return;
            }
        };
        match &command.root {
            Root::List(list) => {
                let shortcut = match binding.shortcut() {
                    Ok(s) => s,
                    Err(e) => {
                        self.issues.push(key_error(binding, e));
                        return;
                    }
                };
                let Some(shortcut) = shortcut else { return };
                self.entries.push(KeyMapEntry {
                    shortcut,
                    target: Target::CompoundList { title: binding.title.clone().unwrap_or_else(|| name.clone()), list: list.clone() },
                    source,
                    scope: Scope::Editor,
                    title: name,
                });
            }
            Root::Entries(table) => {
                if binding.key_equivalent.as_deref().is_some_and(|k| !k.is_empty()) {
                    self.issues.push(KeyMapIssue::UnsupportedCompoundTrigger { title: name.clone() });
                }
                for entry in &table.entries {
                    if entry.key == "any" {
                        continue;
                    }
                    match Shortcut::parse_crbindings(&entry.key) {
                        Ok(shortcut) => self.entries.push(KeyMapEntry {
                            shortcut,
                            target: Target::CompoundList {
                                title: binding.title.clone().unwrap_or_else(|| entry.key.clone()),
                                list: entry.list.clone(),
                            },
                            source: source.clone(),
                            scope: Scope::Editor,
                            title: name.clone(),
                        }),
                        Err(e) => self.issues.push(KeyMapIssue::InvalidKey {
                            title: name.clone(),
                            key: entry.key.clone(),
                            error: e.to_string(),
                        }),
                    }
                }
            }
        }
    }

    fn resolve_command(&mut self, menu: &MenuModel, binding: &super::Binding, command: &str) -> Option<(Target, Scope)> {
        let name = binding.display_name();
        let path_like = command.starts_with("MainMenu/") || command.contains('/');
        if path_like {
            let path = command.strip_prefix("MainMenu/").unwrap_or(command);
            if is_dynamic_path(path) {
                self.issues.push(KeyMapIssue::DynamicMenuPath { title: name, path: command.to_owned() });
                return None;
            }
            let matches = menu.find(path);
            let Some(info) = matches.first() else {
                self.issues.push(KeyMapIssue::UnknownMenuPath { title: name, path: command.to_owned() });
                return None;
            };
            let id = info.command.clone();
            let scope = scope_of(&id);
            return Some((Target::Command(id), scope));
        }
        if let Some(def) = CommandTable::global().by_name(command) {
            return Some((Target::Command(def.id.clone()), def.scope));
        }
        // Also accept a static command id (`navigator.refresh`) for context-menu commands,
        // which have no binding name.
        let id = CommandId::new(command);
        if let Some(def) = CommandTable::global().get(&id) {
            return Some((Target::Command(def.id.clone()), def.scope));
        }
        self.issues.push(KeyMapIssue::UnknownCommand { title: name, command: command.to_owned() });
        None
    }

    fn find_conflicts(&mut self) {
        let mut conflicts = Vec::new();
        for (i, a) in self.entries.iter().enumerate() {
            for b in &self.entries[i + 1..] {
                if !a.scope.overlaps(b.scope) {
                    continue;
                }
                // A set binding shadowing a menu-bar default is the normal way to override a
                // shortcut (the set is resolved first); only clashes inside/between sets and
                // duplicate defaults are reported.
                if matches!(a.source, Source::Default) != matches!(b.source, Source::Default) {
                    continue;
                }
                let kind = if a.shortcut == b.shortcut {
                    Some(ConflictKind::SameShortcut)
                } else if a.shortcut.has_prefix(b.shortcut.strokes()) || b.shortcut.has_prefix(a.shortcut.strokes()) {
                    Some(ConflictKind::Prefix)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    conflicts.push(Conflict {
                        shortcut: a.shortcut.clone(),
                        other: b.shortcut.clone(),
                        kind,
                        first: a.label(),
                        second: b.label(),
                        scopes: (a.scope, b.scope),
                    });
                }
            }
        }
        self.conflicts = conflicts;
    }

    /// Every resolved entry, in precedence order (active set, global set, defaults).
    pub fn entries(&self) -> &[KeyMapEntry] {
        &self.entries
    }

    /// Problems found while resolving (invalid keys, unknown menu paths ...).
    pub fn issues(&self) -> &[KeyMapIssue] {
        &self.issues
    }

    /// Clashing enabled bindings.
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// The compound commands of the active and global sets.
    pub fn library(&self) -> &Arc<CommandLibrary> {
        &self.library
    }

    /// The effective shortcut of a menu command (binding override or default), for display.
    ///
    /// Platform-reserved role shortcuts (`Quit ⌘Q`, `Hide ⌘H`, `Minimize ⌘M`, full screen) are
    /// returned as `None`: the item must be created with its [`crate::Role`], which draws and
    /// handles the standard shortcut itself and must not be handed to NSH as a custom key.
    pub fn menu_shortcut(&self, command: &CommandId) -> Option<Shortcut> {
        let shortcut = if let Some(v) = self.overrides.get(command) {
            v.clone()
        } else {
            let (base, arg) = command.parts();
            if arg.is_some() && let Some(v) = self.overrides.get(&CommandId::new(base)) {
                v.clone()
            } else if let Some(s) = self.defaults.get(command) {
                Some(s.clone())
            } else if arg.is_some() {
                self.defaults.get(&CommandId::new(base)).cloned()
            } else {
                None
            }
        };
        match shortcut {
            Some(s) if is_reserved_role(command, &s) => None,
            other => other,
        }
    }

    /// The shipped default shortcut of a command, for the Reset button.
    pub fn default_shortcut(&self, command: &CommandId) -> Option<Shortcut> {
        self.defaults.get(command).cloned().or_else(|| self.defaults.get(&CommandId::new(command.parts().0)).cloned())
    }

    /// Looks `strokes` up with keyboard focus in `focus`. Entries of [`Scope::App`] are active
    /// everywhere; a chord prefix stops the lookup (and the caller should wait for more keys).
    pub fn lookup(&self, focus: Scope, strokes: &[crate::shortcut::Stroke]) -> Lookup {
        if strokes.is_empty() {
            return Lookup::None;
        }
        for entry in &self.entries {
            if entry.scope != Scope::App && entry.scope != focus {
                continue;
            }
            let entry_strokes = entry.shortcut.strokes();
            if entry_strokes == strokes {
                return Lookup::Match(entry.clone());
            }
            if entry.shortcut.has_prefix(strokes) {
                return Lookup::Prefix;
            }
        }
        Lookup::None
    }

    /// All bindings for exactly `strokes`, highest precedence first. The caller tries them in
    /// order until one consumes the key (a compound command whose tests fail passes through and
    /// the next binding is offered).
    pub fn lookup_all(&self, focus: Scope, strokes: &[crate::shortcut::Stroke]) -> Vec<KeyMapEntry> {
        self.entries
            .iter()
            .filter(|e| (e.scope == Scope::App || e.scope == focus) && e.shortcut.strokes() == strokes)
            .cloned()
            .collect()
    }

    /// [`KeyMap::lookup`] for one stroke.
    pub fn lookup_stroke(&self, focus: Scope, stroke: crate::shortcut::Stroke) -> Lookup {
        self.lookup(focus, std::slice::from_ref(&stroke))
    }
}

impl ShortcutSource for KeyMap {
    fn shortcut_for(&self, id: &CommandId) -> Option<Shortcut> {
        self.menu_shortcut(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::Binding;
    use crate::ids;
    use crate::shortcut::Stroke;

    fn set(name: &str, bindings: Vec<Binding>) -> BindingSet {
        BindingSet { title: name.to_owned(), read_only: false, bindings }
    }

    fn stroke(s: &str) -> Stroke {
        Stroke::parse_crbindings(s).unwrap()
    }

    #[test]
    fn defaults_are_looked_up() {
        let menu = MenuModel::standard();
        let map = KeyMap::build(&menu, None, None);
        let Lookup::Match(e) = map.lookup(Scope::Editor, &[stroke("@s")]) else { panic!("missing save") };
        assert_eq!(e.target, Target::Command(ids::FILE_SAVE.clone()));
    }

    #[test]
    fn set_overrides_and_suppresses_menu_defaults() {
        let menu = MenuModel::standard();
        let active = set("Mine", vec![Binding::command(None, "@s", "MainMenu/File/Save")]);
        let map = KeyMap::build(&menu, Some(&active), None);
        assert_eq!(map.menu_shortcut(&ids::FILE_SAVE).unwrap(), Shortcut::parse_crbindings("@s").unwrap());
        let empty = Binding { key_equivalent: Some(String::new()), ..Binding::command(None, "@s", "MainMenu/File/Save") };
        let map = KeyMap::build(&menu, Some(&set("Mine", vec![empty])), None);
        assert!(map.menu_shortcut(&ids::FILE_SAVE).is_none());
        assert!(map.lookup(Scope::Editor, &[stroke("@s")]).entry().is_none());
    }

    #[test]
    fn duplicate_shortcuts_conflict_but_focus_separates() {
        let menu = MenuModel::standard();
        let active = set(
            "Mine",
            vec![
                Binding::command(None, "~@k", "navigator.refresh"),
                Binding::command(None, "~@k", "MainMenu/File/Open…"),
            ],
        );
        let map = KeyMap::build(&menu, Some(&active), None);
        assert_eq!(map.conflicts().len(), 1);
        assert_eq!(map.conflicts()[0].kind, ConflictKind::SameShortcut);
        // The navigator command only fires with sidebar focus.
        assert!(map.lookup(Scope::Editor, &[stroke("~@k")]).entry().is_some());
        assert!(map.lookup(Scope::Sidebar, &[stroke("~@k")]).entry().is_some());
    }

    #[test]
    fn prefix_conflicts_are_reported() {
        let menu = MenuModel::standard();
        let active = set(
            "Mine",
            vec![
                Binding::compound("one", "j", r#"["beep"]"#),
                Binding::compound("two", "j\u{F8F0}j", r#"["beep"]"#),
            ],
        );
        let map = KeyMap::build(&menu, Some(&active), None);
        assert!(map.conflicts().iter().any(|c| c.kind == ConflictKind::Prefix));
        // Earlier entries win: the exact `j` binding precedes the chord here.
        assert!(matches!(map.lookup(Scope::Editor, &[stroke("j")]), Lookup::Match(_)));
        let reversed = set(
            "Mine",
            vec![
                Binding::compound("two", "j\u{F8F0}j", r#"["beep"]"#),
                Binding::compound("one", "j", r#"["beep"]"#),
            ],
        );
        let map = KeyMap::build(&menu, Some(&reversed), None);
        assert_eq!(map.lookup(Scope::Editor, &[stroke("j")]), Lookup::Prefix);
    }

    #[test]
    fn key_tables_contribute_entries() {
        let menu = MenuModel::standard();
        let table = set(
            "Global",
            vec![Binding { key_equivalent: Some(String::new()), ..Binding::compound("Brace", "", r#"{"{": ["insertText:", "{}"], "\r": ["beep"]}"#) }],
        );
        let map = KeyMap::build(&menu, None, Some(&table));
        assert!(map.lookup(Scope::Editor, &[stroke("{")]).entry().is_some());
        assert!(map.lookup(Scope::Editor, &[stroke("\r")]).entry().is_some());
        assert!(map.lookup(Scope::Editor, &[stroke("x")]).entry().is_none());
        assert!(map.issues().is_empty());
    }

    #[test]
    fn unknown_names_are_issues() {
        let menu = MenuModel::standard();
        let active = set("Mine", vec![Binding::command(None, "@9", "MainMenu/File/Does Not Exist"), Binding::command(None, "@8", "noSuchCommand")]);
        let map = KeyMap::build(&menu, Some(&active), None);
        assert_eq!(map.issues().len(), 2);
    }

    #[test]
    fn reserved_role_shortcuts_are_not_handed_to_the_ui() {
        let menu = MenuModel::standard();
        let map = KeyMap::build(&menu, None, None);
        assert!(map.menu_shortcut(&ids::APP_QUIT).is_none());
        assert!(map.menu_shortcut(&ids::APP_HIDE).is_none());
        assert!(map.menu_shortcut(&ids::VIEW_FULL_SCREEN).is_none());
        assert!(map.menu_shortcut(&ids::FILE_SAVE).is_some());
    }

    #[test]
    fn sidebar_and_editor_scopes_are_independent() {
        assert!(!Scope::Sidebar.overlaps(Scope::Editor));
        assert!(Scope::App.overlaps(Scope::Editor));
    }
}
