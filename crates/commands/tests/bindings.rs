//! Key-binding tests: default table integrity, shipped sets, `.crBindings` fixtures,
//! reserved keys and the runtime key map.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use scratch_commands::bindings::{
    DEFAULT_SET_NAME, GLOBAL_SET_NAME, KeyMap, VIM_SET_NAME, builtin_set, default_set, global_set, vim_set,
};
use scratch_commands::command::{CommandDef, CommandTable, Scope};
use scratch_commands::menu::MenuModel;
use scratch_commands::shortcut::{NshError, Shortcut, Stroke};
use scratch_commands::{CommandContext, CommandId, ids};

const FIXTURE_DIR: &str = "/Applications/CodeRunner.app/Contents/SharedSupport/KeyBindings";

fn table() -> &'static CommandTable {
    CommandTable::global()
}

// ---- the static command table -------------------------------------------------------------

#[test]
fn every_default_table_shortcut_parses() {
    let mut count = 0;
    for def in table().iter() {
        for key in def.default_key_strings() {
            Shortcut::parse_crbindings(key).unwrap_or_else(|e| panic!("{}: {key:?}: {e}", def.id));
            count += 1;
        }
        if let Some(first) = def.default_key_strings().next() {
            let parsed = def.primary_shortcut().unwrap_or_else(|e| panic!("{}: {e}", def.id));
            assert!(parsed.is_some());
            assert_eq!(parsed.unwrap(), Shortcut::parse_crbindings(first).unwrap());
        }
    }
    assert!(count > 100, "only {count} default shortcuts");
}

#[test]
fn command_ids_and_binding_names_are_unique() {
    let mut ids_seen = HashSet::new();
    let mut names_seen = HashMap::new();
    for def in table().iter() {
        assert!(ids_seen.insert(def.id.to_string()), "duplicate command id {}", def.id);
        if let Some(name) = def.binding_name() {
            assert!(names_seen.insert(name, def.id.to_string()).is_none(), "duplicate binding name {name:?}");
        }
    }
}

#[test]
fn no_duplicate_default_shortcuts_without_scope_separation() {
    // Documented allowed overlaps: none in the menu-bar defaults. Two commands may share a
    // shortcut only when their scopes do not overlap (App overlaps everything; Editor and
    // Sidebar do not overlap). Role items (Quit, Hide, Minimize, Full Screen) are platform
    // items and are never installed as NSH shortcuts.
    let mut by_shortcut: HashMap<Shortcut, Vec<&'static CommandDef>> = HashMap::new();
    for def in table().iter() {
        for shortcut in def.default_shortcuts() {
            by_shortcut.entry(shortcut).or_default().push(def);
        }
    }
    let mut clashes = Vec::new();
    for (shortcut, defs) in &by_shortcut {
        for (i, a) in defs.iter().enumerate() {
            for b in &defs[i + 1..] {
                if a.scope.overlaps(b.scope) {
                    clashes.push(format!("{shortcut:?}: {} and {}", a.id, b.id));
                }
            }
        }
    }
    assert!(clashes.is_empty(), "duplicate defaults: {clashes:#?}");
}

#[test]
fn reserved_keys_are_rejected_and_roles_own_them() {
    let reserved = [
        ("cmd+q", ids::APP_QUIT.clone()),
        ("cmd+h", ids::APP_HIDE.clone()),
        ("alt+cmd+h", ids::APP_HIDE_OTHERS.clone()),
        ("cmd+m", ids::WINDOW_MINIMIZE.clone()),
        ("ctrl+cmd+f", ids::VIEW_FULL_SCREEN.clone()),
    ];
    for (text, command) in reserved {
        let stroke = Stroke::parse_nsh(text).unwrap();
        assert!(matches!(stroke.check_nsh(), Err(NshError::Reserved(_))), "{text}");
        assert!(matches!(Shortcut::single(stroke).to_nsh(), Err(NshError::Reserved(_))), "{text}");
        let def = table().get(&command).unwrap_or_else(|| panic!("{command} exists"));
        assert!(def.role.is_some(), "{command} must be a platform role");
        assert_eq!(def.default_shortcuts().first(), Some(&Shortcut::single(stroke)), "{command}");
    }
    // Every other single-stroke default is installable through NSH.
    for def in table().iter() {
        if def.role.is_some_and(|r| {
            matches!(
                r,
                scratch_commands::Role::Quit
                    | scratch_commands::Role::Hide
                    | scratch_commands::Role::HideOthers
                    | scratch_commands::Role::Minimize
                    | scratch_commands::Role::FullScreen
            )
        }) {
            continue;
        }
        for shortcut in def.default_shortcuts() {
            if !shortcut.is_chord() {
                assert!(shortcut.to_nsh().is_ok(), "{} ({}) is not installable: {:?}", def.id, def.title, shortcut.to_nsh());
            }
        }
    }
}

#[test]
fn every_menu_item_has_a_command() {
    let menu = MenuModel::standard();
    let mut dynamic = HashSet::new();
    fn collect(entries: &[scratch_commands::menu::MenuEntry], kinds: &mut HashSet<String>) {
        for e in entries {
            match e {
                scratch_commands::menu::MenuEntry::Dynamic(k) => {
                    kinds.insert(format!("{k:?}"));
                }
                scratch_commands::menu::MenuEntry::Submenu(s) => collect(&s.entries, kinds),
                _ => {}
            }
        }
    }
    for m in menu.menus() {
        collect(&m.entries, &mut dynamic);
    }
    for item in menu.items() {
        assert!(table().get(&item.command).is_some(), "menu item {} has no command", item.path.join("/"));
    }
    for kind in ["OpenRecent", "NewFromTemplate", "Languages", "SyntaxModes", "TextEncodings", "WindowList", "DiagnosticsLanguages"] {
        assert!(dynamic.contains(kind), "missing dynamic menu {kind}: {dynamic:?}");
    }
    assert_eq!(menu.find("MainMenu/File/Save").len(), 1);
    assert_eq!(menu.find("MainMenu/CodeRunner/Preferences...").len(), 1);
    assert!(menu.find("MainMenu/Find/Find Mode/*").len() >= 6);
}

#[test]
fn command_state_is_context_driven() {
    let ctx2 = CommandContext { tab_count: 2, ..CommandContext::new() };
    assert_eq!(table().get(&ids::FILE_CLOSE).unwrap().state(&ctx2).title, "Close Tab");
    let ctx1 = CommandContext::new();
    assert_eq!(table().get(&ids::FILE_CLOSE).unwrap().state(&ctx1).title, "Close Window");
    let compiling = CommandContext { compile_only: true, ..CommandContext::new() };
    assert_eq!(table().get(&ids::RUN_BUILD).unwrap().state(&compiling).title, "Compile");
    let undo = CommandContext { can_undo: true, undo_action_name: Some("Typing".into()), ..CommandContext::new() };
    assert_eq!(table().get(&ids::EDIT_UNDO).unwrap().state(&undo).title, "Undo Typing");
    let browsing = CommandContext { version_browsing: true, ..CommandContext::new() };
    assert!(!table().get(&ids::FILE_SAVE).unwrap().state(&browsing).enabled);
    let checked = CommandContext { sidebar_visible: true, ..CommandContext::new() };
    assert!(!table().get(&ids::VIEW_TOGGLE_SIDEBAR).unwrap().state(&checked).checked);
    assert_eq!(table().get(&ids::VIEW_TOGGLE_SIDEBAR).unwrap().state(&checked).title, "Hide File Navigator");
}

// ---- the shipped sets ---------------------------------------------------------------------

#[test]
fn default_set_is_complete_and_resolves() {
    let set = default_set();
    assert_eq!(set.title, DEFAULT_SET_NAME);
    assert!(set.read_only);
    assert_eq!(set.bindings.len(), 30);
    assert!(set.validate().is_empty(), "{:?}", set.validate());
    let menu = MenuModel::standard();
    for b in &set.bindings {
        let name = b.display_name();
        assert!(b.shortcut().unwrap().is_some(), "{name} has no key");
        match (&b.command, &b.compound_code) {
            (Some(command), None) if command.starts_with("MainMenu/") => {
                assert!(!menu.find(&command["MainMenu/".len()..]).is_empty(), "{name}: {command} does not resolve");
            }
            (Some(command), None) => {
                assert!(
                    table().by_name(command).is_some() || table().get(&CommandId::new(command)).is_some(),
                    "{name}: unknown command {command:?}"
                );
            }
            (None, Some(_code)) => {
                assert!(b.parse_compound().is_some(), "{name}");
            }
            _ => panic!("{name}: bad target combination"),
        }
    }
    let km = KeyMap::build(&menu, Some(&set), None);
    assert!(km.issues().is_empty(), "{:#?}", km.issues());
    assert!(km.conflicts().is_empty(), "{:#?}", km.conflicts());
    // The first binding is the ⌥⌘E then h chord.
    let chord = set.bindings[0].shortcut().unwrap().unwrap();
    assert!(chord.is_chord());
    assert_eq!(chord.strokes().len(), 2);
    assert_eq!(chord.display(), "⌥⌘E H");
}

#[test]
fn vim_set_parses_builds_and_looks_up() {
    let vim = vim_set();
    assert_eq!(vim.title, VIM_SET_NAME);
    assert!(vim.read_only);
    assert!(vim.bindings.len() >= 60, "only {} bindings", vim.bindings.len());
    assert!(vim.validate().is_empty(), "{:?}", vim.validate());
    for b in &vim.bindings {
        assert!(b.shortcut().is_ok(), "{}: {:?}", b.display_name(), b.shortcut());
        if let Some(result) = b.parse_compound() {
            assert!(result.is_ok(), "{}: {}", b.display_name(), result.unwrap_err());
        }
    }
    let menu = MenuModel::standard();
    let km = KeyMap::build(&menu, Some(&vim), None);
    assert!(km.issues().is_empty(), "{:#?}", km.issues());
    assert!(km.conflicts().is_empty(), "{:#?}", km.conflicts());

    let j = Stroke::parse_crbindings("j").unwrap();
    let entry = km.lookup(Scope::Editor, &[j]).entry().cloned().expect("j is bound");
    assert_eq!(entry.title, "Move Down");
    let esc = Stroke::parse_crbindings("\u{1b}").unwrap();
    let entry = km.lookup(Scope::Editor, &[esc]).entry().cloned().expect("Esc is bound");
    assert_eq!(entry.title, "Exit Insert Mode (Esc)");
    // Plain keys are editor-scoped, not app-scoped.
    assert!(km.lookup(Scope::Sidebar, &[j]).entry().is_none());
}

#[test]
fn global_set_parses_and_contributes_key_table_entries() {
    let global = global_set();
    assert_eq!(global.title, GLOBAL_SET_NAME);
    assert_eq!(global.bindings.len(), 9);
    for b in &global.bindings {
        assert!(b.parse_compound().unwrap().is_ok(), "{}", b.display_name());
    }
    let menu = MenuModel::standard();
    let km = KeyMap::build(&menu, None, Some(&global));
    assert!(km.issues().is_empty(), "{:#?}", km.issues());
    for key in ["{", "}", "[", "]", "`", "\u{7f}", "="] {
        let stroke = Stroke::parse_crbindings(key).unwrap();
        assert!(!km.lookup_all(Scope::Editor, &[stroke]).is_empty(), "{key} missing");
    }
}

#[test]
fn vim_and_global_overlaps_are_runtime_separated() {
    // The Vim set and the global compound commands both bind some typing keys. That is
    // deliberate: the Vim commands start with `testNotEditable` (insert mode falls through to
    // the global auto-pairing commands), and the global tests reject normal-mode positions.
    // The key map reports them as conflicts; the UI must offer the matches in order
    // (`lookup_all`) until one consumes the key.
    let menu = MenuModel::standard();
    let km = KeyMap::build(&menu, Some(&vim_set()), Some(&global_set()));
    assert!(km.issues().is_empty(), "{:#?}", km.issues());
    let brace = Stroke::parse_crbindings("{").unwrap();
    let all = km.lookup_all(Scope::Editor, &[brace]);
    assert!(all.len() >= 2, "expected Vim and global braces: {all:#?}");
    assert!(km.conflicts().iter().any(|c| c.shortcut == Shortcut::single(brace)));
}

#[test]
fn builtin_lookup_by_name() {
    assert_eq!(builtin_set("Default").unwrap().title, DEFAULT_SET_NAME);
    assert_eq!(builtin_set("Vim").unwrap().title, VIM_SET_NAME);
    assert_eq!(builtin_set("_Global_").unwrap().title, GLOBAL_SET_NAME);
    assert!(builtin_set("Nope").is_none());
}

// ---- the real CodeRunner files (read-only fixtures) ----------------------------------------

fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = Path::new(FIXTURE_DIR).join(name);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            eprintln!("SKIP: fixture {} is not readable: {e}", path.display());
            None
        }
    }
}

#[test]
fn crbindings_fixtures_roundtrip() {
    for (file, title, read_only, count) in
        [("Default.crBindings", "Default", true, 30), ("_Global_.crBindings", "_Global_", false, 9), ("Vim.crBindings", "Vim", true, 96)]
    {
        let Some(bytes) = fixture(file) else { continue };
        let set = scratch_commands::bindings::BindingSet::from_plist_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{file} failed to parse: {e}"));
        assert_eq!(set.title, title);
        assert_eq!(set.read_only, read_only);
        assert_eq!(set.bindings.len(), count, "{file}");
        for b in &set.bindings {
            assert!(!b.identifier.is_empty(), "{file}: missing identifier");
            assert!(b.shortcut().is_ok(), "{file}: {:?}", b.shortcut());
            if b.compound_code.is_some() {
                assert!(b.parse_compound().is_some(), "{file}: {} has no parsable code", b.display_name());
            }
        }
        let written = set.to_plist_bytes().unwrap_or_else(|e| panic!("{file} failed to write: {e}"));
        let again = scratch_commands::bindings::BindingSet::from_plist_bytes(&written)
            .unwrap_or_else(|e| panic!("{file} rewritten file failed to parse: {e}"));
        assert_eq!(again, set, "{file} did not round-trip");
    }
}

#[test]
fn default_fixture_uses_chords_and_control_characters() {
    let Some(bytes) = fixture("Default.crBindings") else { return };
    let set = scratch_commands::bindings::BindingSet::from_plist_bytes(&bytes).unwrap();
    let chord = set.bindings[0].shortcut().unwrap().unwrap();
    assert!(chord.is_chord());
    assert_eq!(chord.strokes()[1], Stroke::parse_crbindings("h").unwrap());
    let newline_below = set
        .bindings
        .iter()
        .find(|b| b.command.as_deref() == Some("insertNewlineBelow"))
        .and_then(|b| b.shortcut().unwrap())
        .expect("insertNewlineBelow");
    assert_eq!(newline_below.strokes()[0].key, scratch_commands::shortcut::Key::Enter);
}

// ---- the authored compounds run -------------------------------------------------------------

use std::sync::Arc;

use scratch_commands::compound::{Host, Machine, Registers, Status, fake::FakeHost};

fn vim_compound(title: &str) -> scratch_commands::compound::CompoundCommand {
    let set = vim_set();
    set.bindings
        .iter()
        .find(|b| b.title.as_deref() == Some(title))
        .and_then(|b| b.parse_compound())
        .unwrap_or_else(|| panic!("no Vim command {title:?}"))
        .unwrap_or_else(|e| panic!("{title}: {e}"))
}

#[test]
fn vim_delete_character_runs() {
    let vim = vim_set();
    let library = Arc::new(vim.library());
    let cmd = vim_compound("Delete Character");
    let mut host = FakeHost::new("abc");
    host.set_editable(false);
    let mut regs = Registers::new();
    let mut machine = Machine::new(library);
    assert_eq!(machine.start_command(&cmd, None, &mut host, &mut regs), Status::Consumed);
    assert_eq!(host.text_string(), "bc");
}

#[test]
fn vim_visual_selects_a_character() {
    let vim = vim_set();
    let library = Arc::new(vim.library());
    let cmd = vim_compound("Visual");
    let mut host = FakeHost::new("abc");
    host.set_editable(false);
    let mut regs = Registers::new();
    let mut machine = Machine::new(library);
    assert_eq!(machine.start_command(&cmd, None, &mut host, &mut regs), Status::Consumed);
    assert_eq!(host.sel().range(), 0..1);
}

#[test]
fn vim_count_repeats_the_motion() {
    let vim = vim_set();
    let library = Arc::new(vim.library());
    let mut host = FakeHost::new("abcdef");
    host.set_editable(false);
    let mut regs = Registers::new();
    let mut machine = Machine::new(library);
    let count = vim_compound("Count 3");
    assert_eq!(machine.start_command(&count, None, &mut host, &mut regs), Status::Consumed);
    let motion = vim_compound("Move Right");
    assert_eq!(machine.start_command(&motion, None, &mut host, &mut regs), Status::Consumed);
    assert_eq!(host.caret(), 3);
}

#[test]
fn global_brace_pair_and_return_run() {
    let global = global_set();
    let binding = global
        .bindings
        .iter()
        .find(|b| b.title.as_deref() == Some("Defer insertion of matching } at end of line"))
        .expect("brace binding");
    let command = binding.parse_compound().unwrap().unwrap();
    let library = Arc::new(global.library());
    let mut host = FakeHost::new("x\n");
    host.set_caret(1);
    host.scopes.push(("source".into(), 0..2));
    let mut regs = Registers::new();
    let mut machine = Machine::new(library);

    assert_eq!(machine.start_command(&command, Some("{"), &mut host, &mut regs), Status::Consumed);
    assert_eq!(host.text_string(), "x{\n");
    assert_eq!(host.caret(), 2);

    assert_eq!(machine.start_command(&command, Some("\r"), &mut host, &mut regs), Status::Consumed);
    assert_eq!(host.text_string(), "x{\n}\n");
}
