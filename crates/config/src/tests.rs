//! Crate-level tests: defaults, schema completeness, get/set, I/O, migration
//! and CodeRunner import.

use std::collections::BTreeSet;

use toml::{Table, Value};

use crate::access::{lookup, values_equal};
use crate::cr_import::convert_and_validate;
use crate::*;

fn leaves(table: &Table, path: &str, out: &mut Vec<String>, unknown: &mut Vec<String>) {
    for (key, value) in table {
        let id = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
        if PrefSchema::get(&id).is_some_and(|s| s.is_value()) {
            out.push(id);
        } else if let Value::Table(inner) = value {
            leaves(inner, &id, out, unknown);
        } else {
            unknown.push(id);
        }
    }
}

// ------------------------------------------------------------- defaults

#[test]
fn defaults_match_the_research() {
    let p = Preferences::default();
    // Report 03 and verdicts C6/C7 of report 10.
    assert!(!p.text_editing.indentation.soft_tabs, "C6: tabs by default");
    assert_eq!(p.text_editing.indentation.tab_width, 4);
    assert!(p.themes.use_separate_color_themes, "C7: separate themes by default");
    assert_eq!(p.themes.color_theme_light, "ScratchRunner Light");
    assert_eq!(p.themes.color_theme_dark, "ScratchRunner Dark");
    assert_eq!(p.themes.resolve(true), "ScratchRunner Dark");
    assert_eq!(p.themes.resolve(false), "ScratchRunner Light");
    assert!(!p.shell.run_as_login_shell, "C14: login shell off");
    assert!(p.shell.automatically_determine_path, "C14: PATH determined automatically");
    assert_eq!(p.shell.shell, "/bin/bash");
    assert_eq!(p.shell.path, "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
    assert_eq!(p.shell.environment["TERM"], "dumb");
    assert_eq!(p.shell.environment["LC_CTYPE"], "UTF-8");
    assert_eq!(p.key_bindings.set, "Default");
    assert_eq!(p.general.tabs.insert_new_tabs, InsertNewTabs::AfterExisting);
    assert_eq!(p.general.console.position, ConsolePosition::Bottom);
    assert!(p.general.console.hide_automatically);
    assert_eq!(p.general.documents.file_encoding, "utf-8");
    assert!(!p.general.documents.ask_to_save_when_quitting);
    assert_eq!(p.text_editing.appearance.line_height, 1.2);
    assert_eq!(p.text_editing.appearance.insertion_point.style, InsertionPointStyle::Bar);
    assert_eq!(
        p.text_editing.appearance.alternate_insertion_point.style,
        InsertionPointStyle::Block
    );
    assert_eq!(p.text_editing.appearance.page_guide.column, 80);
    assert_eq!(
        p.text_editing.appearance.invisibles.mode,
        InvisiblesMode::SelectionAndNonconformingIndentation
    );
    assert_eq!(p.text_editing.appearance.invisibles.characters, "\\t \\n\\r\u{a0}");
    assert_eq!(p.text_editing.behavior.kill_ring_size, 7);
    assert_eq!(p.text_editing.completion.word_length, 2);
    assert_eq!(p.text_editing.diagnostics.delay, 1.5);
    assert_eq!(p.debugger.variables_sort_mode, VariablesSortMode::Natural);
    assert_eq!(p.printing.font_size, 8);
    assert_eq!(p.docs_global_count(), 5);
    assert_eq!(p.advanced.extra_swift_flags, "-sdk macosx");
    assert_eq!(p.advanced.clang.extra_flags, "-Wno-unused-command-line-argument");
    assert_eq!(
        p.text_editing.behavior.multiple_selections_event_masks,
        vec![393_216, 1_048_576, 524_288]
    );
}

impl Preferences {
    fn docs_global_count(&self) -> usize {
        self.languages.docs.global_entries.len()
    }
}

// --------------------------------------------------------------- schema

#[test]
fn every_field_has_a_schema_entry_and_back() {
    let table = Preferences::default().to_table();
    let (mut found, mut unknown) = (Vec::new(), Vec::new());
    leaves(&table, "", &mut found, &mut unknown);
    assert!(unknown.is_empty(), "fields without a schema entry: {unknown:?}");
    let fields: BTreeSet<_> = found.iter().map(String::as_str).collect();
    let specs: BTreeSet<_> = PrefSchema::values().map(|s| s.id).collect();
    assert_eq!(
        specs.difference(&fields).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "schema entries without a field"
    );
    assert_eq!(
        fields.difference(&specs).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "fields without a schema entry"
    );
    // Every leaf is reachable: ids are unique.
    let all: Vec<_> = PrefSchema::all().map(|s| s.id).collect();
    assert_eq!(all.len(), all.iter().collect::<BTreeSet<_>>().len(), "duplicate ids");
}

#[test]
fn schema_defaults_equal_struct_defaults() {
    let defaults = Preferences::default();
    for spec in PrefSchema::values() {
        let got = defaults.get(spec.id).unwrap_or_else(|| panic!("{} missing", spec.id));
        assert!(
            values_equal(&got, &spec.default_value()),
            "{}: struct default {got:?} != schema default {:?}",
            spec.id,
            spec.default_value()
        );
    }
}

#[test]
fn schema_entries_are_well_formed() {
    let mut cr_keys = BTreeSet::new();
    for spec in PrefSchema::all() {
        if let Some(parent) = spec.parent {
            assert!(PrefSchema::get(parent).is_some(), "{}: parent {parent} missing", spec.id);
        }
        if let Some(key) = spec.cr_key {
            assert!(cr_keys.insert(key), "duplicate cr_key {key}");
        }
        if spec.in_ui {
            assert!(
                spec.label.is_some()
                    || spec.layout == Layout::SameRowAsParent
                    || spec.layout == Layout::InlineInParentLabel
                    || matches!(
                        spec.control,
                        ControlKind::Radio(_) | ControlKind::Custom(_) | ControlKind::Text { multiline: true, .. }
                    ),
                "{} has a control but no label",
                spec.id
            );
        }
        if let Some(label) = spec.label
            && label.contains("%@")
            && spec.layout != Layout::InlineInOwnLabel
        {
            assert!(
                PrefSchema::children(spec.id).any(|c| c.layout == Layout::InlineInParentLabel),
                "{}: label has %@ but nothing is inlined",
                spec.id
            );
        }
        if spec.layout == Layout::InlineInParentLabel {
            let parent = PrefSchema::get(spec.parent.expect("inline child has a parent")).unwrap();
            assert!(parent.label.is_some_and(|l| l.contains("%@")), "{}", spec.id);
        }
        match spec.control {
            ControlKind::Popup(options) | ControlKind::Radio(options) => {
                let default = spec.default_value();
                assert!(
                    options.iter().any(|o| !o.separator && values_equal(&o.value.to_value(), &default)),
                    "{}: default not among options",
                    spec.id
                );
            }
            ControlKind::Number { min, max, .. } => {
                let d = match spec.default_value() {
                    Value::Integer(i) => i as f64,
                    Value::Float(f) => f,
                    other => panic!("{}: number default {other:?}", spec.id),
                };
                assert!((min..=max).contains(&d), "{}: default out of range", spec.id);
            }
            ControlKind::Font { size_id } => assert!(PrefSchema::get(size_id).is_some()),
            _ => {}
        }
        // Every entry of a visible pane belongs to it.
        assert!(spec.in_ui || spec.pane != Pane::Hidden || spec.is_value());
    }
}

#[test]
fn schema_lookup_helpers() {
    assert_eq!(PrefSchema::resolve("tab_width").unwrap().id, "text_editing.indentation.tab_width");
    assert!(PrefSchema::resolve("enabled").is_none(), "ambiguous bare key");
    assert_eq!(PrefSchema::by_cr_key("DefaultTabModeWidth").unwrap().key(), "tab_width");
    assert_eq!(PrefSchema::by_cr_key("Font").unwrap().id, "text_editing.appearance.font.family");
    assert_eq!(
        PrefSchema::categories(Pane::TextEditing),
        vec![
            Some("Appearance"),
            Some("Indentation"),
            Some("Behavior"),
            Some("Completion"),
            Some("Diagnostics")
        ]
    );
    assert_eq!(
        PrefSchema::categories(Pane::General),
        vec![
            Some("Documents"),
            Some("Tabs"),
            Some("Console"),
            Some("File Navigator"),
            Some("Notifications"),
            None
        ]
    );
    assert_eq!(Pane::TextEditing.window_title(), "Preferences \u{2013} Text Editing");
    assert_eq!(Pane::from_identifier("KeyBindings"), Some(Pane::KeyBindings));
    let width = PrefSchema::get("text_editing.indentation.tab_width").unwrap();
    assert_eq!(width.layout, Layout::SameRowAsParent);
    assert_eq!(width.options().len(), 5);
    let sparkle = PrefSchema::get("general.updates.check_automatically").unwrap();
    assert!(!sparkle.in_ui);
    assert!(PrefSchema::get("general.documents.ask_to_save_when_quitting").unwrap().requires_restart);
    assert_eq!(
        PrefSchema::get("text_editing.indentation.soft_tabs").unwrap().help,
        Some("Indentation can be customized on a per-language basis under the Languages tab.")
    );
    let insertion = PrefSchema::get("text_editing.appearance.insertion_point.animate").unwrap();
    assert_eq!(insertion.parent, Some("text_editing.appearance.insertion_point.blink"));
    assert_eq!(insertion.gate, Gate::WhenOn);
    assert_eq!(PrefSchema::get("shell.path").unwrap().gate, Gate::WhenOff);
    assert_eq!(PrefSchema::get("shell.automatically_determine_path").unwrap().options().len(), 2);
    assert_eq!(
        PrefSchema::get("text_editing.completion.word_length").unwrap().label_without_placeholder().unwrap(),
        "Display completions after typing characters"
    );
}

// --------------------------------------------------------------- get/set

#[test]
fn get_and_set_by_id() {
    let mut p = Preferences::default();
    assert_eq!(p.get("tab_width"), Some(Value::Integer(4)));
    assert_eq!(p.get("general.tabs.insert_new_tabs"), Some(Value::String("after_existing".into())));
    p.set("tab_width", Value::Integer(8)).unwrap();
    assert_eq!(p.text_editing.indentation.tab_width, 8);
    p.set("general.tabs.insert_new_tabs", Value::String("after_selected".into())).unwrap();
    assert_eq!(p.general.tabs.insert_new_tabs, InsertNewTabs::AfterSelected);
    // An integer for a float preference is coerced.
    p.set("text_editing.appearance.line_height", Value::Integer(2)).unwrap();
    assert_eq!(p.text_editing.appearance.line_height, 2.0);
    p.set("shell.environment", Value::Table(
        [("A".to_owned(), Value::String("1".into()))].into_iter().collect(),
    )).unwrap();
    assert_eq!(p.shell.environment.len(), 1);
    assert!(!p.is_default("tab_width"));
    p.reset("tab_width").unwrap();
    assert!(p.is_default("tab_width"));
}

#[test]
fn set_validates() {
    let mut p = Preferences::default();
    let before = p.clone();
    assert!(matches!(p.set("nope", Value::Boolean(true)), Err(PrefError::Unknown(_))));
    assert!(matches!(
        p.set("tab_width", Value::Integer(5)),
        Err(PrefError::NotAnOption { .. })
    ));
    assert!(matches!(
        p.set("tab_width", Value::String("4".into())),
        Err(PrefError::NotAnOption { .. })
    ));
    assert!(matches!(
        p.set("text_editing.appearance.line_height", Value::Float(500.0)),
        Err(PrefError::OutOfRange { .. })
    ));
    assert!(matches!(
        p.set("text_editing.appearance.line_height", Value::String("x".into())),
        Err(PrefError::WrongType { .. })
    ));
    assert!(matches!(
        p.set("text_editing.behavior.kill_ring_size", Value::Float(1.5)),
        Err(PrefError::WrongType { .. })
    ));
    assert!(matches!(
        p.set("general.documents.autosave", Value::Integer(1)),
        Err(PrefError::WrongType { .. })
    ));
    assert!(matches!(
        p.set("general.action.edit_file_type_associations", Value::Boolean(true)),
        Err(PrefError::NotAValue(_))
    ));
    assert!(matches!(
        p.set("shell.environment", Value::Table([("A".to_owned(), Value::Integer(1))].into_iter().collect())),
        Err(PrefError::Invalid { .. })
    ));
    assert!(matches!(
        p.set("misc.suppressed_alerts", Value::Boolean(true)),
        Err(PrefError::WrongType { .. })
    ));
    assert_eq!(p, before, "a rejected value changes nothing");
    assert_eq!(p.get("general.action.edit_file_type_associations"), None);
}

#[test]
fn diff_lists_changed_ids() {
    let old = Preferences::default();
    let mut new = old.clone();
    assert!(Preferences::diff(&old, &new).is_empty());
    new.text_editing.appearance.wrap_lines = false;
    new.shell.run_as_login_shell = true;
    new.themes.color_theme_dark = "Other".into();
    new.text_editing.appearance.line_height = 1.5;
    let mut changed = Preferences::diff(&old, &new);
    changed.sort_unstable();
    assert_eq!(
        changed,
        vec![
            "shell.run_as_login_shell",
            "text_editing.appearance.line_height",
            "text_editing.appearance.wrap_lines",
            "themes.color_theme_dark",
        ]
    );
}

// ------------------------------------------------------------------ I/O

fn sample() -> Preferences {
    let mut p = Preferences::default();
    p.text_editing.indentation.soft_tabs = true;
    p.text_editing.indentation.tab_width = 2;
    p.text_editing.appearance.font.family = "JetBrains Mono".into();
    p.text_editing.appearance.font.size = 13.5;
    p.text_editing.appearance.alternate_insertion_point.style = InsertionPointStyle::Underline;
    p.general.tabs.insert_new_tabs = InsertNewTabs::BeforeExisting;
    p.shell.environment.insert("FOO".into(), "bar baz".into());
    p.shell.arguments = vec!["-x".into()];
    p.themes.use_separate_color_themes = false;
    p.themes.color_theme = "Solarized".into();
    p.misc.suppressed_alerts.insert("run-while-running".into(), true);
    p.languages.docs.global_entries.pop();
    p.languages.docs.state.insert(
        "X".into(),
        DocsState { transparency: true, expects_redirect_from_search_results: true, zoom_level: Some(1.25) },
    );
    p.text_editing.behavior.comment_characters_by_scope.insert(
        "text.plain".into(),
        [("line".to_owned(), "//".to_owned())].into_iter().collect(),
    );
    p
}

#[test]
fn round_trips_through_text() {
    let p = sample();
    let text = p.render(None);
    let loaded = Loaded::parse(&text);
    assert_eq!(loaded.warnings, vec![], "{text}");
    assert_eq!(loaded.prefs, p, "{text}");
    // The full export round-trips too.
    let full = Loaded::parse(&p.to_toml_full());
    assert_eq!(full.warnings, vec![]);
    assert_eq!(full.prefs, p);
    // And so do the defaults, through the table form.
    let d = Preferences::default();
    assert_eq!(Preferences::from_table(d.to_table()).unwrap(), d);
}

#[test]
fn default_preferences_render_sparse() {
    let text = Preferences::default().render(None);
    assert!(text.contains("version = 1"));
    assert!(!text.contains("tab_width"), "{text}");
    let changed = {
        let mut p = Preferences::default();
        p.text_editing.indentation.tab_width = 8;
        p.render(None)
    };
    assert!(changed.contains("[text_editing.indentation]"), "{changed}");
    assert!(changed.contains("tab_width = 8"), "{changed}");
    assert!(!changed.contains("soft_tabs"));
}

#[test]
fn saving_preserves_comments_and_unknown_keys() {
    let existing = "# my settings\nversion = 1\n\n[text_editing.indentation]\n# width of a tab\ntab_width = 3 # three\n\n[lsp]\nenabled = true\n";
    let loaded = Loaded::parse(existing);
    assert_eq!(loaded.warnings, vec![]);
    assert_eq!(loaded.prefs.text_editing.indentation.tab_width, 3);
    assert_eq!(loaded.extra["lsp"]["enabled"], Value::Boolean(true));
    let mut prefs = loaded.prefs;
    prefs.text_editing.indentation.tab_width = 6;
    prefs.text_editing.appearance.wrap_lines = false;
    let out = prefs.render(Some(existing));
    assert!(out.contains("# my settings"), "{out}");
    assert!(out.contains("# width of a tab"), "{out}");
    assert!(out.contains("tab_width = 6 # three"), "{out}");
    assert!(out.contains("[lsp]\nenabled = true"), "{out}");
    assert!(out.contains("wrap_lines = false"), "{out}");
    // A value equal to the default but already in the file stays.
    let mut back = prefs.clone();
    back.text_editing.indentation.tab_width = 4;
    assert!(back.render(Some(&out)).contains("tab_width = 4 # three"));
    // Unchanged preferences leave the text byte for byte alone.
    let again = Loaded::parse(&out).prefs.render(Some(&out));
    assert_eq!(again, out);
}

#[test]
fn save_writes_atomically_and_backs_up_an_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::in_dir(dir.path());
    let file = paths.settings();
    let p = sample();
    p.save(&file).unwrap();
    let loaded = Loaded::load(&file);
    assert_eq!(loaded.prefs, p);
    assert!(loaded.file_used);

    std::fs::write(&file, "[broken\nvalue = ").unwrap();
    let broken = Loaded::load(&file);
    assert_eq!(broken.prefs, Preferences::default());
    assert_eq!(broken.warnings[0].kind, WarningKind::SyntaxError);
    assert!(broken.warning().unwrap().contains("settings.toml was ignored"));
    p.save(&file).unwrap();
    let backup = std::fs::read_to_string(dir.path().join("Application Support/ScratchRunner/settings.toml.bak")).unwrap();
    assert!(backup.starts_with("[broken"));
    assert_eq!(Loaded::load(&file).prefs, p);
    let leftovers = std::fs::read_dir(file.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 2, "settings.toml and its backup only");
}

#[test]
fn missing_file_is_silent_and_empty_file_is_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = Loaded::load(&dir.path().join("settings.toml"));
    assert_eq!(loaded.warning(), None);
    assert!(!loaded.file_used);
    assert_eq!(loaded.prefs, Preferences::default());
    let empty = Loaded::parse("");
    assert_eq!(empty.prefs, Preferences::default());
    assert_eq!(empty.warnings, vec![]);
}

#[test]
fn invalid_values_fall_back_one_by_one() {
    let text = r#"
version = 1
[text_editing.indentation]
tab_width = 5
soft_tabs = true
bogus = 1
[text_editing.appearance]
line_height = "tall"
wrap_lines = false
[general.tabs]
insert_new_tabs = "sideways"
[themes]
color_theme_dark = "Night"
[shell]
environment = { A = 1 }
[future_section]
x = 1
"#;
    let loaded = Loaded::parse(text);
    let p = &loaded.prefs;
    assert_eq!(p.text_editing.indentation.tab_width, 4, "invalid -> default");
    assert!(p.text_editing.indentation.soft_tabs, "valid neighbours are kept");
    assert!(!p.text_editing.appearance.wrap_lines);
    assert_eq!(p.text_editing.appearance.line_height, 1.2);
    assert_eq!(p.general.tabs.insert_new_tabs, InsertNewTabs::AfterExisting);
    assert_eq!(p.themes.color_theme_dark, "Night");
    assert_eq!(p.shell.environment, default_environment());
    let ids: Vec<_> = loaded.warnings.iter().filter_map(|w| w.id.as_deref()).collect();
    for expected in [
        "text_editing.indentation.tab_width",
        "text_editing.appearance.line_height",
        "general.tabs.insert_new_tabs",
        "shell.environment",
        "text_editing.indentation.bogus",
    ] {
        assert!(ids.contains(&expected), "{expected} not in {ids:?}");
    }
    assert_eq!(
        loaded.warnings.iter().find(|w| w.id.as_deref() == Some("text_editing.indentation.bogus")).unwrap().kind,
        WarningKind::UnknownKey
    );
    assert!(loaded.extra.contains_key("future_section"));
}

#[test]
fn wrong_shape_of_a_nested_value_is_rejected() {
    let loaded = Loaded::parse("[languages.docs]\nglobal_entries = [ { name = 3 } ]\n");
    assert_eq!(loaded.prefs.languages.docs.global_entries.len(), 5);
    assert_eq!(loaded.warnings.len(), 1);
}

// ------------------------------------------------------------ migration

#[test]
fn migrates_the_first_scratchrunner_format() {
    let legacy = r#"
[general]
restore_session = true
default_language = "python"
theme = "dark"

[editor]
font_family = "Menlo"
font_size = 15
line_numbers = false
soft_wrap = true
tab_size = 2
auto_close_brackets = false

[execution]
timeout_seconds = 5

[runtimes.python]
command = "python3"
"#;
    let loaded = Loaded::parse(legacy);
    assert_eq!(loaded.migrated_from, Some(0));
    assert_eq!(loaded.warnings, vec![], "{:?}", loaded.warnings);
    let p = &loaded.prefs;
    assert_eq!(p.languages.current_language, "python");
    assert_eq!(p.text_editing.appearance.font.size, 15.0);
    assert!(!p.text_editing.appearance.line_numbers.enabled);
    assert!(p.text_editing.appearance.wrap_lines);
    assert_eq!(p.text_editing.indentation.tab_width, 2);
    assert!(!p.text_editing.behavior.character_pairs.close_enabled);
    assert!(loaded.extra.contains_key("execution"));
    assert!(loaded.extra.contains_key("runtimes"));
    // Saving rewrites the legacy keys in place and stamps the version.
    let out = loaded.prefs.render(Some(legacy));
    assert!(out.contains("version = 1"), "{out}");
    assert!(!out.contains("[editor]"), "{out}");
    assert!(!out.contains("restore_session"), "{out}");
    assert!(out.contains("[runtimes.python]"), "{out}");
    let again = Loaded::parse(&out);
    assert_eq!(again.migrated_from, None);
    assert_eq!(again.warnings, vec![]);
    assert_eq!(again.prefs, loaded.prefs);
}

#[test]
fn migration_chain_runs_in_order() {
    use toml_edit::DocumentMut;
    fn one(doc: &mut DocumentMut) -> Result<(), String> {
        doc["steps"] = toml_edit::value("1");
        Ok(())
    }
    fn two(doc: &mut DocumentMut) -> Result<(), String> {
        let steps = doc["steps"].as_str().ok_or("step one did not run")?.to_owned();
        doc["steps"] = toml_edit::value(format!("{steps}2"));
        Ok(())
    }
    fn fail(_: &mut DocumentMut) -> Result<(), String> {
        Err("boom".into())
    }
    let chain = [Migration { from: 1, run: two }, Migration { from: 0, run: one }];
    let mut doc: DocumentMut = "x = 1".parse().unwrap();
    assert_eq!(migrate_document(&mut doc, &chain, 2).unwrap(), Some(0));
    assert_eq!(doc["steps"].as_str(), Some("12"));
    assert_eq!(doc["version"].as_integer(), Some(2));
    assert_eq!(migrate_document(&mut doc, &chain, 2).unwrap(), None);
    let mut start_at_one: DocumentMut = "version = 1\nsteps = \"a\"".parse().unwrap();
    migrate_document(&mut start_at_one, &chain, 2).unwrap();
    assert_eq!(start_at_one["steps"].as_str(), Some("a2"));
    let mut failing: DocumentMut = "".parse().unwrap();
    let err = migrate_document(&mut failing, &[Migration { from: 0, run: fail }], 1).unwrap_err();
    assert!(err.contains("boom"));
}

#[test]
fn newer_format_loads_with_a_warning() {
    let loaded = Loaded::parse("version = 99\n[general.tabs]\nprefer_tabs = false\n");
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].kind, WarningKind::NewerVersion);
    assert!(!loaded.prefs.general.tabs.prefer_tabs);
}

// ------------------------------------------------------- CodeRunner import

fn defaults_plist() -> Option<plist::Value> {
    let path = std::path::Path::new("/Applications/CodeRunner.app/Contents/Resources/Defaults.plist");
    if !path.exists() {
        eprintln!("skipped: CodeRunner's Defaults.plist is not installed");
        return None;
    }
    match plist::Value::from_file(path) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("skipped: Defaults.plist unreadable: {e}");
            None
        }
    }
}

/// `(key, value, is_button)` of every item, children included.
fn plist_items(root: &plist::Value) -> Vec<(String, Option<plist::Value>, bool)> {
    fn walk(item: &plist::Value, out: &mut Vec<(String, Option<plist::Value>, bool)>) {
        let Some(dict) = item.as_dictionary() else { return };
        let button = dict.get("type").and_then(|v| v.as_string()) == Some("button");
        if let Some(id) = dict.get("identifier").and_then(|v| v.as_string()) {
            let key = id.rsplit('/').next().unwrap_or(id).to_owned();
            out.push((key, dict.get("value").cloned(), button));
        }
        if let Some(children) = dict.get("children").and_then(|v| v.as_array()) {
            for child in children {
                walk(child, out);
            }
        }
    }
    let mut out = Vec::new();
    for list in ["General", "Text", "All"] {
        if let Some(items) = root.as_dictionary().and_then(|d| d.get(list)).and_then(|v| v.as_array()) {
            for item in items {
                walk(item, &mut out);
            }
        }
    }
    out
}

#[test]
fn every_defaults_plist_key_is_mapped_and_equal() {
    let Some(root) = defaults_plist() else { return };
    let items = plist_items(&root);
    assert!(items.len() > 150, "only {} items", items.len());
    // Brand-bearing defaults differ on purpose; Font is set in code.
    const EXCEPTIONS: &[&str] = &[
        "Font",
        "AutosaveDocumentsDirectory",
        "Environment",
        "ColorTheme",
        "ColorThemeLight",
        "ColorThemeDark",
    ];
    let defaults = Preferences::default();
    let mut checked = 0;
    for (key, value, button) in items {
        if button {
            continue; // actions: general/updates, text/diagnostics/auto-fixes, ...
        }
        let spec = PrefSchema::by_cr_key(&key).unwrap_or_else(|| panic!("no mapping for CodeRunner key {key}"));
        let Some(value) = value else { continue };
        if EXCEPTIONS.contains(&key.as_str()) {
            continue;
        }
        let converted = convert_and_validate(spec, &value)
            .unwrap_or_else(|| panic!("{key}: plist value {value:?} not accepted by {}", spec.id));
        let ours = defaults.get(spec.id).unwrap();
        assert!(values_equal(&converted, &ours), "{key}: plist {converted:?} != default {ours:?} ({})", spec.id);
        checked += 1;
    }
    assert!(checked > 140, "checked only {checked}");
}

#[test]
fn defaults_plist_option_lists_match_the_schema() {
    let Some(root) = defaults_plist() else { return };
    let Some(items) = root.as_dictionary().and_then(|d| d.get("Text")).and_then(|v| v.as_array()) else { return };
    for item in items {
        let Some(dict) = item.as_dictionary() else { continue };
        let (Some(id), Some(options)) = (
            dict.get("identifier").and_then(|v| v.as_string()),
            dict.get("options").and_then(|v| v.as_array()),
        ) else {
            continue;
        };
        let key = id.rsplit('/').next().unwrap();
        let spec = PrefSchema::by_cr_key(key).unwrap();
        let titles: Vec<_> = options
            .iter()
            .map(|o| o.as_dictionary().and_then(|d| d.get("title")).and_then(|t| t.as_string()).unwrap_or(""))
            .collect();
        let ours: Vec<_> = spec.options().iter().map(|o| o.title).collect();
        assert_eq!(titles, ours, "{key}");
        let label = dict.get("title").and_then(|v| v.as_string());
        assert_eq!(label, spec.label, "{key}: label");
    }
}

#[test]
fn defaults_plist_labels_match_the_schema() {
    let Some(root) = defaults_plist() else { return };
    for (key, label) in plist_labels(&root) {
        let Some(spec) = PrefSchema::by_cr_key(&key) else { continue };
        if spec.id.ends_with(".action") {
            continue;
        }
        assert_eq!(Some(label.as_str()), spec.label, "{key}");
    }
}

fn plist_labels(root: &plist::Value) -> Vec<(String, String)> {
    fn walk(item: &plist::Value, out: &mut Vec<(String, String)>) {
        let Some(dict) = item.as_dictionary() else { return };
        let button = dict.get("type").and_then(|v| v.as_string()) == Some("button");
        if let (Some(id), Some(title)) = (
            dict.get("identifier").and_then(|v| v.as_string()),
            dict.get("title").and_then(|v| v.as_string()),
        ) {
            // Skip Font (composite) and buttons (their identifiers are categories).
            if !button && !id.ends_with("/Font") {
                out.push((id.rsplit('/').next().unwrap().to_owned(), title.to_owned()));
            }
        }
        for child in dict.get("children").and_then(|v| v.as_array()).into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    for list in ["General", "Text", "All"] {
        for item in root.as_dictionary().and_then(|d| d.get(list)).and_then(|v| v.as_array()).into_iter().flatten() {
            walk(item, &mut out);
        }
    }
    out
}

fn font_blob(name: &str, size: f64) -> plist::Value {
    use plist::{Dictionary, Uid, Value as P};
    let mut font = Dictionary::new();
    font.insert("NSName".into(), P::Uid(Uid::new(2)));
    font.insert("NSSize".into(), P::Real(size));
    let mut root = Dictionary::new();
    root.insert(
        "$objects".into(),
        P::Array(vec![P::String("$null".into()), P::Dictionary(font), P::String(name.into())]),
    );
    let mut bytes = Vec::new();
    plist::to_writer_binary(&mut bytes, &P::Dictionary(root)).unwrap();
    P::Data(bytes)
}

#[test]
fn imports_known_keys_and_reports_the_rest() {
    use plist::{Dictionary, Value as P};
    let mut dict = Dictionary::new();
    dict.insert("DefaultTabModeSoftTabs".into(), P::Boolean(true));
    dict.insert("DefaultTabModeWidth".into(), P::Integer(2.into()));
    dict.insert("InsertNewTabs".into(), P::Integer(2.into()));
    dict.insert("ConsolePosition".into(), P::Integer(1.into()));
    dict.insert("InvisiblesMode".into(), P::Integer(4.into()));
    dict.insert("FileEncoding".into(), P::Integer(0x9400_0100_i64.into()));
    dict.insert("LineHeight".into(), P::Integer(2.into()));
    dict.insert("ColorTheme".into(), P::String("Twilight".into()));
    dict.insert("ColorThemeDark".into(), P::String("Twilight".into()));
    dict.insert("ColorThemeLight".into(), P::String("CodeRunner Light".into()));
    dict.insert("MatchAppearanceToTheme".into(), P::Boolean(true));
    dict.insert("FontLigatures".into(), P::Boolean(true));
    dict.insert("Font".into(), font_blob("JetBrainsMono-Regular", 12.0));
    dict.insert("Environment".into(), {
        let mut env = Dictionary::new();
        env.insert("TERM_PROGRAM".into(), P::String("CodeRunner".into()));
        env.insert("FOO".into(), P::String("1".into()));
        P::Dictionary(env)
    });
    dict.insert("CommentCharactersByScope".into(), {
        let mut inner = Dictionary::new();
        inner.insert("line".into(), P::String("//".into()));
        let mut outer = Dictionary::new();
        outer.insert("text.plain".into(), P::Dictionary(inner));
        P::Dictionary(outer)
    });
    dict.insert("DocsState".into(), {
        let mut state = Dictionary::new();
        state.insert("expectsRedirectFromSearchResults".into(), P::Boolean(true));
        let mut outer = Dictionary::new();
        outer.insert("ID-1".into(), P::Dictionary(state));
        P::Dictionary(outer)
    });
    dict.insert("InvisibleCharacters".into(), P::Integer(3.into())); // wrong type
    dict.insert("DefaultTabModeWidth2".into(), P::Boolean(true)); // unknown
    dict.insert("CurrentLanguageUUID".into(), P::String("31392064".into())); // not imported
    dict.insert("NSWindow Frame MainWindow".into(), P::String("0 0 1 1".into()));
    dict.insert("AutosaveDocumentsDirectory".into(), P::String("$(APPLICATION_SUPPORT)/CodeRunner/Unsaved".into()));
    let report = import_from_cr_defaults(&dict);
    let p = &report.prefs;
    assert!(p.text_editing.indentation.soft_tabs);
    assert_eq!(p.text_editing.indentation.tab_width, 2);
    assert_eq!(p.general.tabs.insert_new_tabs, InsertNewTabs::AfterSelected);
    assert_eq!(p.general.console.position, ConsolePosition::Right);
    assert_eq!(p.text_editing.appearance.invisibles.mode, InvisiblesMode::Selection);
    assert_eq!(p.general.documents.file_encoding, "utf-16le");
    assert_eq!(p.text_editing.appearance.line_height, 2.0);
    assert_eq!(p.themes.color_theme, "Twilight");
    assert_eq!(p.themes.color_theme_light, "ScratchRunner Light", "CodeRunner's default maps over");
    assert!(p.themes.match_appearance_to_theme);
    assert!(p.text_editing.appearance.font.ligatures);
    assert_eq!(p.text_editing.appearance.font.family, "JetBrainsMono-Regular");
    assert_eq!(p.text_editing.appearance.font.size, 12.0);
    assert_eq!(p.shell.environment["TERM_PROGRAM"], "ScratchRunner");
    assert_eq!(p.shell.environment["FOO"], "1");
    assert_eq!(p.text_editing.behavior.comment_characters_by_scope["text.plain"]["line"], "//");
    assert!(p.languages.docs.state["ID-1"].expects_redirect_from_search_results);
    assert_eq!(p.languages.current_language, "", "ids of CodeRunner's languages are not imported");
    assert_eq!(p.advanced.autosave_documents_directory, "$(APPLICATION_SUPPORT)/ScratchRunner/Unsaved");
    assert!(report.imported.contains(&"text_editing.indentation.tab_width"));
    assert!(report.skipped.iter().any(|s| s.key == "InvisibleCharacters"));
    assert!(report.skipped.iter().any(|s| s.key == "CurrentLanguageUUID"));
    assert!(report.unmapped.contains(&"NSWindow Frame MainWindow".to_owned()));
    assert!(report.unmapped.contains(&"DefaultTabModeWidth2".to_owned()));
}

#[test]
fn imports_the_global_docs_list() {
    let Some(root) = defaults_plist() else { return };
    let (_, value, _) = plist_items(&root)
        .into_iter()
        .find(|(k, _, _)| k == "GlobalDocs")
        .expect("GlobalDocs in Defaults.plist");
    let mut dict = plist::Dictionary::new();
    dict.insert("GlobalDocs".into(), value.unwrap());
    let report = import_from_cr_defaults(&dict);
    assert_eq!(report.prefs.languages.docs.global_entries, default_global_docs());
}

#[test]
fn imports_the_real_preferences_when_present() {
    let Some(path) = cr_preferences_path().filter(|p| p.exists()) else {
        eprintln!("skipped: CodeRunner has no preferences file here");
        return;
    };
    let dict = match read_cr_preferences(&path) {
        Ok(dict) => dict,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let report = import_from_cr_defaults(&dict);
    // Whatever the user has, the result is a valid, self-consistent tree.
    assert_eq!(Preferences::from_table(report.prefs.to_table()).unwrap(), report.prefs);
    assert!(report.skipped.iter().all(|s| !s.key.is_empty()));
}

#[test]
fn table_lookup_helper() {
    let t = Preferences::default().to_table();
    assert!(lookup(&t, "text_editing.appearance.font.size").is_some());
    assert!(lookup(&t, "text_editing.nope").is_none());
}

#[test]
fn saving_unchanged_preferences_leaves_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let mut prefs = Preferences::default();
    prefs.text_editing.indentation.tab_width = 2;
    prefs.save(&path).unwrap();
    // A comment the user wrote, and a modification time from the past.
    let text = format!("# mine\n{}", std::fs::read_to_string(&path).unwrap());
    std::fs::write(&path, &text).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
    prefs.save(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), old);
    prefs.text_editing.indentation.tab_width = 8;
    prefs.save(&path).unwrap();
    assert_eq!(Loaded::load(&path).prefs.text_editing.indentation.tab_width, 8);
    assert!(std::fs::read_to_string(&path).unwrap().starts_with("# mine\n"));
}

#[test]
fn line_height_in_points() {
    let mut appearance = Appearance::default();
    appearance.font.size = 10.0;
    appearance.line_height = 1.5;
    assert_eq!(appearance.line_height_points(), 15.0);
    appearance.line_height = 0.5;
    assert_eq!(appearance.line_height_points(), 10.0, "never tighter than the font");
    appearance.line_height = f64::NAN;
    assert_eq!(appearance.line_height_points(), 10.0);
}
