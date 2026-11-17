# scratch-config

Storage layout and preferences for ScratchRunner. No UI dependencies.

| Piece | Where |
|---|---|
| `Paths` | `~/Library/Application Support/ScratchRunner/{settings.toml, Languages/, Themes/, KeyBindings/, Debuggers/, Docs/, Unsaved/, session.json, window-state.json, recent.json, logs/}` and `~/Library/Caches/ScratchRunner/`. `Paths::system()`, `Paths::in_dir(tmp)` for tests, `ensure_dirs()`. |
| `write_atomic` | temp file + fsync + rename |
| `Preferences` | typed `serde(default)` tree: `general`, `text_editing`, `languages`, `themes`, `key_bindings`, `shell`, `advanced`, plus no-pane `debugger`, `printing`, `ui`, `misc` |
| `PrefSchema` / `PrefSpec` | static table of every preference (pane, category, section, label, help, control, default, parent/gate, layout, restart, `cr_key`) in display order |
| get/set/diff | `prefs.get("tab_width")`, `prefs.set(id, PrefValue)` (validated), `Preferences::diff(&old, &new)`, `reset`, `is_default` |
| I/O | `Loaded::load/parse` (never fails, per-key warnings, `extra` for foreign tables), `Preferences::save/render` (toml_edit: keeps comments and unknown keys, sparse), `migrate_document` + `SETTINGS_VERSION` |
| Import | `import_from_cr_defaults(&plist::Dictionary)`, `read_cr_preferences(path)` (read-only) |

## Conventions

* Ids are the dotted TOML path (`text_editing.indentation.tab_width`); a bare key is accepted when unique.
* Enums are snake_case strings in TOML (`insert_new_tabs = "after_selected"`); the schema popup options carry CodeRunner's integer in `PopupOption::cr`, used by the import.
* `settings.toml` is sparse: only non-default values (and keys already in the file) are written.
* Defaults equal CodeRunner's `Defaults.plist`, except brand-bearing values (`AutosaveDocumentsDirectory`, `TERM_PROGRAM`, the default theme names, mapped on import); the test `every_defaults_plist_key_is_mapped_and_equal` checks this when CodeRunner is installed.
* Per-language settings, templates, docs entries, themes and key-binding sets are files in their own directories (owned by other crates); only the global defaults are here.
