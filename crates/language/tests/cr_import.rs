//! Import of CodeRunner's own `.crLanguage` bundles. The app is used as a
//! read-only fixture; the tests skip visibly when it is not installed.

use std::path::{Path, PathBuf};

use scratch_language::{
    CompilePlan, CompileSpec, DiagnosticKind, LanguageRegistry, RunContext, Severity, Vars,
    from_cr_language_plist, import_cr_language_bundle,
};

const LANGUAGES: &str = "/Applications/CodeRunner.app/Contents/SharedSupport/Languages";

fn bundles() -> Option<Vec<PathBuf>> {
    let dir = Path::new(LANGUAGES);
    if !dir.is_dir() {
        eprintln!("SKIPPED: {LANGUAGES} not found (CodeRunner is not installed)");
        return None;
    }
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "crLanguage"))
        .collect();
    bundles.sort();
    Some(bundles)
}

fn bundle(name: &str) -> Option<PathBuf> {
    let path = Path::new(LANGUAGES).join(format!("{name}.crLanguage"));
    if path.is_dir() {
        Some(path)
    } else {
        eprintln!("SKIPPED: {} not found", path.display());
        None
    }
}

#[test]
fn all_bundled_languages_import_and_agree_with_our_definitions() {
    let Some(bundles) = bundles() else { return };
    assert_eq!(bundles.len(), 29);
    let ours = LanguageRegistry::builtin();
    for path in bundles {
        let imported =
            import_cr_language_bundle(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let theirs = &imported.def;
        let errors: Vec<_> = theirs
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{}: {errors:?}", theirs.name);

        let uuid = theirs.uuid.as_deref().expect("every bundle has a uuid");
        let mine = ours
            .by_uuid(uuid)
            .unwrap_or_else(|| panic!("{} ({uuid}) has no counterpart", theirs.name));
        assert_eq!(mine.name, theirs.name, "{uuid}");
        assert_eq!(mine.grammar, theirs.grammar, "{}", theirs.name);
        assert_eq!(mine.console.kind, theirs.console.kind, "{}", theirs.name);
        assert_eq!(
            mine.console.switchable, theirs.console.switchable,
            "{}",
            theirs.name
        );
        assert_eq!(mine.console.live, theirs.console.live, "{}", theirs.name);
        assert_eq!(
            mine.requires_external_components, theirs.requires_external_components,
            "{}",
            theirs.name
        );
        assert_eq!(
            mine.custom_tab_mode, theirs.custom_tab_mode,
            "{}",
            theirs.name
        );
        assert_eq!(mine.soft_tabs, theirs.soft_tabs, "{}", theirs.name);
        assert_eq!(mine.tab_width, theirs.tab_width, "{}", theirs.name);
        assert_eq!(
            mine.output_encoding, theirs.output_encoding,
            "{}",
            theirs.name
        );
        // (URLs are ours: https and current addresses.)
        let installer = |l: &scratch_language::LanguageDef| {
            l.installer
                .as_ref()
                .map(|i| (i.executable.clone(), i.name.clone()))
        };
        if theirs.installer.is_some() {
            assert_eq!(installer(mine), installer(theirs), "{}", theirs.name);
        }
        assert_eq!(
            mine.errors.keys().collect::<Vec<_>>(),
            theirs.errors.keys().collect::<Vec<_>>(),
            "{}",
            theirs.name
        );
        assert_eq!(
            mine.has_compile_step(),
            theirs.has_compile_step(),
            "{}",
            theirs.name
        );
        assert_eq!(
            mine.bundle_version, theirs.bundle_version,
            "{}",
            theirs.name
        );
        if mine.id != "rust" {
            assert_eq!(mine.debugger, theirs.debugger, "{}", theirs.name);
        }
        // Their extension list is ours or a subset of ours, same default.
        assert_eq!(
            mine.extensions.first(),
            theirs.extensions.first(),
            "{}",
            theirs.name
        );
        for ext in &theirs.extensions {
            assert!(mine.extensions.contains(ext), "{} lacks .{ext}", mine.name);
        }
        // Default template: the same names/extensions where CodeRunner has templates.
        let names = |l: &scratch_language::LanguageDef| {
            l.templates
                .iter()
                .map(|t| {
                    (
                        t.name.clone(),
                        t.extension.clone(),
                        t.default,
                        t.name_on_creation,
                    )
                })
                .collect::<Vec<_>>()
        };
        if matches!(
            theirs.id.as_str(),
            "c" | "cpp" | "go" | "python" | "rust" | "objective-c" | "swift" | "csharp-mono"
        ) {
            assert_eq!(names(mine), names(theirs), "{}", theirs.name);
        }
        // Their compile scripts are kept verbatim as shell specs.
        match &theirs.run.compile {
            CompileSpec::Shell(shell) => {
                assert!(shell.script.contains("#!/bin/bash"), "{}", theirs.name)
            }
            CompileSpec::None => {}
            other => panic!("{other:?}"),
        }
        // Diagnostics: same entries by name and kind (our CodeRunner-derived defs).
        for theirs_d in &theirs.diagnostics {
            if let Some(mine_d) = mine.diagnostics.iter().find(|d| d.name == theirs_d.name) {
                // (Python's custom parser script is replaced by a highlight-only regex.)
                if !(mine.id == "python" && theirs_d.name == "Console Text") {
                    assert_eq!(
                        mine_d.kind, theirs_d.kind,
                        "{}/{}",
                        mine.name, theirs_d.name
                    );
                }
                assert_eq!(
                    mine_d.update.bits(),
                    theirs_d.update.bits(),
                    "{}/{}",
                    mine.name,
                    theirs_d.name
                );
            }
        }
    }
}

#[test]
fn go_imports_with_its_compile_script_and_diagnostics() {
    let Some(path) = bundle("Go") else { return };
    let imported = import_cr_language_bundle(&path).unwrap();
    let go = imported.def;
    assert_eq!(go.id, "go");
    assert_eq!(go.name, "Go");
    assert_eq!(go.extensions, ["go"]);
    assert_eq!(go.run.command, "$compiler");
    assert_eq!(go.debugger.as_deref(), Some("lldb"));
    assert!(go.requires_external_components);
    let installer = go.installer.as_ref().unwrap();
    assert_eq!(
        (installer.executable.as_str(), installer.name.as_str()),
        ("go", "the Go compiler")
    );
    let CompileSpec::Shell(shell) = &go.run.compile else {
        panic!("expected a shell compile")
    };
    assert!(shell.script.contains("go build"));
    assert_eq!(go.diagnostics.len(), 1);
    assert_eq!(go.diagnostics[0].kind, DiagnosticKind::CompileScript);
    assert_eq!(go.diagnostics[0].options.bits(), 4);
    assert_eq!(go.diagnostics[0].update.bits(), 3);
    assert_eq!(go.templates.len(), 1);
    assert!(go.templates[0].default);
    assert_eq!(
        go.templates[0].text,
        "package main\n\nfunc main() {\n\t<##>\n}"
    );
    assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
}

#[test]
fn python_imports_docs_and_custom_script_diagnostics() {
    let Some(path) = bundle("Python") else { return };
    let imported = import_cr_language_bundle(&path).unwrap();
    let py = imported.def;
    assert_eq!(py.run.command, "python3 $filename");
    assert_eq!(py.debugger.as_deref(), Some("pdb"));
    assert!(matches!(py.run.compile, CompileSpec::None));
    assert_eq!(py.docs.len(), 2);
    assert_eq!(
        py.docs[0].url,
        "https://docs.python.org/3/search.html?q={query}"
    );
    assert_eq!(py.docs[1].kind, scratch_language::DocType::Bookmark);
    assert_eq!(py.diagnostics.len(), 2);
    let syntax = &py.diagnostics[0];
    assert_eq!(syntax.kind, DiagnosticKind::Command);
    assert!(syntax.update.on_edit && !syntax.update.on_run);
    assert_eq!(
        syntax.output_mode,
        scratch_language::OutputMode::CustomScript
    );
    let console = &py.diagnostics[1];
    assert_eq!(console.kind, DiagnosticKind::ConsoleText);
    assert!(
        console.output_script.is_some(),
        "the parser script is carried along"
    );
}

#[test]
fn c_imports_flags_and_regex_diagnostics() {
    let Some(path) = bundle("C") else { return };
    let c = import_cr_language_bundle(&path).unwrap().def;
    assert_eq!(c.extensions, ["c", "h"]);
    assert_eq!(c.run.flags, "-std=c11");
    assert_eq!(c.diagnostics[0].kind, DiagnosticKind::Clang);
    assert!(regex_ok(c.diagnostics[0].regex.as_deref().unwrap()));
    assert_eq!(c.templates.iter().filter(|t| t.default).count(), 1);
    // `h` is shared with C++ and Objective-C: imports do not decide it.
    assert_eq!(c.default_for, Vec::<String>::new());
    assert!(c.group_name() == "C");
}

fn regex_ok(pattern: &str) -> bool {
    regex::Regex::new(pattern).is_ok()
}

#[test]
fn importing_into_a_registry_replaces_the_bundled_language() {
    let Some(path) = bundle("Go") else { return };
    let mut registry = LanguageRegistry::builtin();
    let before = registry.get("go").unwrap().clone();
    let (id, warnings) = registry.import_cr_language(&path).unwrap();
    assert_eq!(id, "go", "the uuid belongs to the bundled Go");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(registry.is_overridden("go"));
    assert!(registry.get("go").unwrap().builtin);
    assert!(matches!(
        registry.get("go").unwrap().run.compile,
        CompileSpec::Shell(_)
    ));
    // The imported shell-protocol language still plans a run.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("main.go"), "package main").unwrap();
    let plan = registry
        .get("go")
        .unwrap()
        .plan_run(&RunContext::new(Vars::for_source(tmp.path(), "main.go")))
        .unwrap();
    let CompilePlan::Shell(shell) = &plan.compile else {
        panic!()
    };
    assert_eq!(shell.cwd, tmp.path());
    assert_eq!(
        shell.env.get("CR_FILENAME").map(String::as_str),
        Some("main.go")
    );
    let argv = plan
        .run_argv("/bin/bash", false, Some("note\n/tmp/out/main\n"))
        .unwrap();
    assert_eq!(argv, ["/tmp/out/main"]);
    // Reset brings our definition back.
    registry.reset("go").unwrap();
    assert_eq!(registry.get("go").unwrap(), &before);
}

#[test]
fn importing_an_unknown_language_adds_it() {
    let Some(path) = bundle("Lua") else { return };
    // Pretend the user changed the uuid: a custom copy.
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("Mine.crLanguage");
    std::fs::create_dir_all(copy.join("Scripts")).unwrap();
    let plist = std::fs::read_to_string(path.join("Language.plist")).unwrap();
    let plist = plist.replace(
        "0C785A85-9668-4B43-AE5F-54B0D104D222",
        "FFFFFFFF-0000-0000-0000-000000000000",
    );
    let plist = plist.replace("<string>Lua</string>", "<string>Lua (mine)</string>");
    std::fs::write(copy.join("Language.plist"), plist).unwrap();
    let mut registry = LanguageRegistry::builtin();
    let (id, _) = registry.import_cr_language(&copy).unwrap();
    assert_eq!(id, "lua-mine");
    assert!(!registry.get("lua-mine").unwrap().builtin);
    assert_eq!(registry.get("lua-mine").unwrap().group_name(), "Lua");
}

#[test]
fn the_plist_alone_converts_too() {
    let Some(path) = bundle("Ruby") else { return };
    let bytes = std::fs::read(path.join("Language.plist")).unwrap();
    let imported = from_cr_language_plist(&bytes).unwrap();
    assert_eq!(imported.def.run.command, "ruby $filename");
    assert_eq!(imported.def.diagnostics[0].kind, DiagnosticKind::Command);
}
