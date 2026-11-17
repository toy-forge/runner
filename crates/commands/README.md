# scratch-commands

ScratchRunner's command registry, menu model and key-binding system. Pure Rust: no `nsh-*`
dependency, so it builds and tests without the UI framework.

## What is here

| Module | Contents |
|---|---|
| `command` | `CommandId`, `CommandDef`, `CommandTable::global()`, `ItemState`, `Role` (platform menu items), `Scope` (App/Editor/Sidebar/Console/Docs/Debugger). |
| `context` | `CommandContext`: plain state snapshot used to compute enabled/checked/visible/title. |
| `table` | The static command table (`ids::FILE_SAVE`, …), predicates, dynamic titles and the editor command vocabulary. |
| `menu` | `MenuModel::standard()`: the CodeRunner-4.6-order menu bar, context menus, alternates, dynamic placeholders (`DynamicKind`), fragment injection, `resolve()` for native menus, wildcard `find()`. |
| `shortcut` | `Stroke`/`Shortcut`: CodeRunner `@~^$` key equivalents (with U+F8F0 chord separator), NSH `mod+shift+x` text, Mac glyphs, reserved-key validation. |
| `bindings` | `BindingSet` (`.crBindings` plist, JSON, TOML), shipped `Default`/`Vim`/`_Global_` sets, `SetStore` (user directory), `KeyMap` (effective shortcuts, lookup, conflicts), `KeyBindings.dict` import. |
| `compound` | The compound-command language: parser (`CompoundCommand`), resumable interpreter (`Machine`), `Host` trait, `FakeHost`, selected command library, registers. |
| `fuzzy` | `fuzzy_match(query, text)` with scores and highlight positions. |
| `palette` | `palette_items(...)`: QuickNav `mainMenu` rows (title, shortcut subtitle, menu path). |

## Usage

```rust
use scratch_commands::bindings::{default_set, KeyMap};
use scratch_commands::menu::DefaultShortcuts;
use scratch_commands::{CommandContext, MenuModel, Scope, Stroke};

let menu = MenuModel::standard();
let set = default_set();
let keys = KeyMap::build(&menu, Some(&set), None);
assert!(keys.issues().is_empty() && keys.conflicts().is_empty());

let strokes = [Stroke::parse_crbindings("@s").unwrap()];
let entry = keys.lookup(Scope::Editor, &strokes).entry().cloned().expect("Save");
assert_eq!(entry.title, "Save");

let resolved = menu.resolve(&CommandContext::new(), &keys, &scratch_commands::menu::NoDynamicEntries);
assert_eq!(resolved[0].title, "ScratchRunner");
```

`KeyMap` implements `menu::ShortcutSource`, so pass it to `MenuModel::resolve` and
`palette::palette_items` to show the user's effective shortcuts. Menu items with a
`CommandDef::role` (`Quit`, `Hide`, `Minimize`, `Full Screen`, …) must use the platform role and
must not install their key equivalent; use `Role` instead.

## Key-binding files

* `.crBindings`: CodeRunner XML property list `{ title, readOnly, keyBindings[] }`, binding
  `{ identifier, keyEquivalent, command | compoundCommandCode, title? }`. The reader ignores the
  `compoundCommandArray`/`compoundCommandDictionary` caches and tolerates the raw control
  characters CodeRunner writes (Escape, Backspace); the writer regenerates the caches.
* Our JSON/TOML: `{ title, read_only, bindings[] }` with `key`, `title`, `command`, `compound`,
  `enabled`. The compound source uses the same language as `.crBindings`. CodeRunner's
  `.crBindings` has no enabled flag, so a disabled binding loses that state when exported to
  plist; keep such sets in JSON/TOML.
* `KeyBindings.dict`: old-style ASCII, XML or binary plist dictionaries. Imported as the
  editable "macOS Key Bindings" set; selectors with arguments and nested dictionaries are
  skipped with warnings.
* User sets live in `~/Library/Application Support/ScratchRunner/KeyBindings/`; the global set
  is `_Global_.crBindings`. Shipped sets are read-only; duplicate to edit.

## Key-map resolution

Precedence: active set, `_Global_` set, menu-bar defaults. A set binding to a menu command
replaces that command's default shortcut; an empty key equivalent removes it. `lookup` returns
the first match; `lookup_all` returns every match so the UI can run a compound command, and if
its top-level tests fail (pass-through) offer the next. A chord prefix returns `Lookup::Prefix`
until enough keys arrive; entries are ordered by precedence, so put a longer chord before a
shorter binding that shares its first stroke. `conflicts()` reports same-shortcut and prefix
clashes inside and between sets (a set shadowing a menu default is not a clash).

The menu-bar defaults have no overlapping duplicate shortcuts. The Vim and `_Global_` sets do
deliberately bind some of the same typing keys; their compound commands use `testIsEditable` /
`testNotEditable` and text tests so only one applies, and the UI resolves them through
`lookup_all`. Set bindings shadowing menu defaults are not reported as conflicts.

## Compound commands

`CompoundCommand::parse` accepts JSON with `//` comments and trailing commas. `Machine` runs a
list against any `Host` (text + selection + editor commands + menu items) and is resumable:
`start`/`start_command`, `feed_key` for object-keyed waits, `cancel`. `FakeHost` is the test
host. See `compound/mod.rs` for semantics (failure scopes, break/fail, call/include, registers).

## Tests

`cargo test -p scratch-commands` covers the command table, menu order and dynamic placeholders,
every shipped/default binding (parsing, duplicate detection, NSH reserved keys), `.crBindings`
round-trips against the real CodeRunner files when installed (skipped visibly otherwise), Vim
and global compounds running on `FakeHost`, `KeyBindings.dict` (including the system file),
set storage, conflicts and fuzzy/palette matching.
