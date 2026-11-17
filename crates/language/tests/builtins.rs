//! Integrity tests of the embedded built-in definitions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use scratch_language::{
    CompilePlan, CompileSpec, ConsoleType, DetectInput, DetectReason, DiagnosticKind, IndentStyle,
    LanguageDef, LanguageRegistry, NSH_BUILTIN_EDITOR_LANGUAGES, OutputMode, ResolvedCommand,
    RunContext, TemplateContext, Vars, group_templates, is_nsh_builtin_editor_language,
    render_template,
};

fn registry() -> LanguageRegistry {
    LanguageRegistry::builtin()
}

#[test]
fn ids_names_and_uuids_are_unique() {
    let r = registry();
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut uuids = BTreeSet::new();
    for def in r.all() {
        assert!(ids.insert(def.id.clone()), "duplicate id {}", def.id);
        assert!(
            names.insert(def.name.to_lowercase()),
            "duplicate name {}",
            def.name
        );
        if let Some(uuid) = &def.uuid {
            assert!(uuids.insert(uuid.to_lowercase()), "duplicate uuid {uuid}");
        }
    }
    assert_eq!(r.all().len(), 34);
}

#[test]
fn every_builtin_validates_without_errors_or_warnings() {
    for def in registry().all() {
        let issues = def.validate();
        assert!(issues.is_empty(), "{}: {issues:?}", def.id);
        assert!(def.builtin);
    }
}

#[test]
fn every_extension_has_exactly_one_default_language() {
    let r = registry();
    let mut claims: BTreeMap<String, Vec<&LanguageDef>> = BTreeMap::new();
    for def in r.all() {
        for ext in &def.extensions {
            claims.entry(ext.clone()).or_default().push(def);
        }
    }
    for (ext, claimants) in &claims {
        let defaults: Vec<&str> = claimants
            .iter()
            .filter(|l| l.default_for.contains(ext))
            .map(|l| l.id.as_str())
            .collect();
        if claimants.len() == 1 {
            assert!(defaults.len() <= 1);
        } else {
            assert_eq!(
                defaults.len(),
                1,
                ".{ext} is claimed by {:?} and needs exactly one default_for, has {defaults:?}",
                claimants.iter().map(|l| &l.id).collect::<Vec<_>>()
            );
        }
        // And detection agrees.
        let name = format!("file.{ext}");
        let detected = r.detect(&DetectInput::new().path(Path::new(&name)));
        assert_eq!(detected.reason, DetectReason::Extension, ".{ext}");
        let expected = if claimants.len() == 1 {
            claimants[0].id.as_str()
        } else {
            defaults[0]
        };
        assert_eq!(detected.language.id, expected, ".{ext}");
        assert_eq!(detected.candidates.len(), claimants.len() - 1, ".{ext}");
    }
    // The shared extensions of CodeRunner.
    assert_eq!(
        r.detect(&DetectInput::new().path(Path::new("a.h")))
            .language
            .id,
        "c"
    );
}

/// What the research (CodeRunner 4.6 language list) records
/// for the 29 bundled languages.
#[allow(clippy::type_complexity)]
const CODERUNNER_TABLE: &[(
    &str,         // id
    &str,         // name
    &str,         // uuid
    &[&str],      // extensions CodeRunner lists
    &str,         // grammar
    Option<&str>, // debugger
    ConsoleType,  // console type
    bool,         // requires external components
)] = &[
    (
        "applescript",
        "AppleScript",
        "AF9F60E4-632E-40AB-93D8-6A414F37DC9B",
        &["applescript", "scpt"],
        "source.applescript",
        None,
        ConsoleType::Terminal,
        false,
    ),
    (
        "csharp-script",
        "C# (.NET Script)",
        "A65D203A-FC0C-4F0C-A080-41863424566E",
        &["csx"],
        "source.cs",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "csharp-dotnet",
        "C# (.NET)",
        "E141E12A-2879-4253-B8B5-5FB47C3DBC8F",
        &["cs"],
        "source.cs",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "csharp-mono",
        "C# (Mono)",
        "9A37339F-AB15-4DDD-BF09-4C54AB627DA4",
        &["cs"],
        "source.cs",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "cpp",
        "C++",
        "EC55AB59-7920-4862-B7EB-963CD4BB928A",
        &["cpp", "cc", "cp", "cxx", "c++", "h", "hh", "hpp", "h++"],
        "source.c++",
        Some("lldb"),
        ConsoleType::Terminal,
        false,
    ),
    (
        "c",
        "C",
        "F158B03F-4059-4FBE-95FB-F101FC3C2097",
        &["c", "h"],
        "source.c",
        Some("lldb"),
        ConsoleType::Terminal,
        false,
    ),
    (
        "css",
        "CSS",
        "B78D595F-94B7-470B-BE66-C3BF31DE4E4F",
        &["css"],
        "source.css",
        None,
        ConsoleType::WebResource,
        false,
    ),
    (
        "go",
        "Go",
        "31392064-DD21-4114-9BEB-7FA18B503CD8",
        &["go"],
        "source.go",
        Some("lldb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "haskell",
        "Haskell",
        "ED86A00E-F6AE-4094-A125-7E6117B9D0DC",
        &["hs"],
        "source.haskell",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "html",
        "HTML",
        "FFE53747-E6FA-41A9-AD88-B6E70A7D2DBE",
        &["html", "htm", "shtml", "xhtml", "tmpl", "tpl"],
        "text.html.basic",
        None,
        ConsoleType::Web,
        false,
    ),
    (
        "java",
        "Java",
        "123A91F7-DC75-4DA4-AC58-2D9FBC35F8C5",
        &["java"],
        "source.java",
        Some("jdb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "javascript-browser",
        "JavaScript (Browser)",
        "BDBEA569-54FA-4E18-9E33-35F90DDECE1A",
        &["js"],
        "source.js",
        None,
        ConsoleType::WebResource,
        false,
    ),
    (
        "javascript-jquery",
        "JavaScript (jQuery)",
        "B2BAD283-A431-45E6-B53C-8FCE8081C0B3",
        &["js"],
        "source.js.jquery",
        None,
        ConsoleType::WebResource,
        false,
    ),
    (
        "javascript-node",
        "JavaScript (Node.js)",
        "C871AFED-ADA2-4CDA-955A-68B11C6AA5B5",
        &["js"],
        "source.js.node",
        Some("node"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "kotlin",
        "Kotlin",
        "A9B2B786-309E-45AD-B55D-78C59418ED2A",
        &["kt", "kts"],
        "source.Kotlin",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "latex",
        "LaTeX",
        "1D9475F2-F52D-4F8B-AD10-257A00223011",
        &["tex"],
        "text.tex.latex",
        None,
        ConsoleType::Pdf,
        true,
    ),
    (
        "lua",
        "Lua",
        "0C785A85-9668-4B43-AE5F-54B0D104D222",
        &["lua"],
        "source.lua",
        None,
        ConsoleType::Terminal,
        true,
    ),
    (
        "markdown",
        "Markdown",
        "210B9720-0015-445F-919F-8BF207AEF0FD",
        &["md", "markdown"],
        "text.html.markdown",
        None,
        ConsoleType::Web,
        false,
    ),
    (
        "objective-cpp",
        "Objective-C++",
        "54152989-9931-4123-8B23-584872677F75",
        &["mm", "h"],
        "source.objc++",
        Some("lldb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "objective-c",
        "Objective-C",
        "AE688137-77E1-44B1-8D36-92AFF8C442CA",
        &["m", "h"],
        "source.objc",
        Some("lldb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "perl",
        "Perl",
        "64A50F8B-F915-49CE-BD6A-0E5305190918",
        &["pl"],
        "source.perl",
        Some("perldebug"),
        ConsoleType::Terminal,
        false,
    ),
    (
        "php",
        "PHP",
        "DC909571-5FC3-4625-90FD-A59B3171788D",
        &["php"],
        "text.html.php",
        Some("xdebug"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "python",
        "Python",
        "3CB7EE93-841D-41F6-96EE-4C8E8C6927E4",
        &["py"],
        "source.python",
        Some("pdb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "ruby",
        "Ruby",
        "966788B5-C2C2-444C-AB8C-C97862C77C6C",
        &["rb"],
        "source.ruby",
        None,
        ConsoleType::Terminal,
        false,
    ),
    // Rust: lldb, not the bundled gdb (research 10, C19).
    (
        "rust",
        "Rust",
        "834D5121-761E-4B58-B40B-82FA581EB2AE",
        &["rs"],
        "source.rust",
        Some("lldb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "shell",
        "Shell Script",
        "673C976D-FCA5-4110-A78F-5751A98CC481",
        &["sh"],
        "source.shell",
        Some("bashdb"),
        ConsoleType::Terminal,
        false,
    ),
    (
        "swift",
        "Swift",
        "8EB61A41-AAC1-4C23-B110-2E2C8B5E1162",
        &["swift"],
        "source.swift",
        Some("lldb"),
        ConsoleType::Terminal,
        true,
    ),
    (
        "typescript-browser",
        "TypeScript (Browser)",
        "01AE4478-7C08-46DC-9369-946EEE11083C",
        &["ts"],
        "source.ts",
        None,
        ConsoleType::WebResource,
        true,
    ),
    (
        "typescript-node",
        "TypeScript (Node.js)",
        "8CF92C94-BDD9-4CAB-B52D-271639671821",
        &["ts"],
        "source.ts",
        None,
        ConsoleType::Terminal,
        true,
    ),
];

#[test]
fn the_29_coderunner_languages_carry_their_facts() {
    let r = registry();
    assert_eq!(CODERUNNER_TABLE.len(), 29);
    for (id, name, uuid, extensions, grammar, debugger, console, external) in CODERUNNER_TABLE {
        let def = r.get(id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(&def.name, name, "{id}");
        assert_eq!(def.uuid.as_deref(), Some(*uuid), "{id}");
        assert_eq!(&def.grammar, grammar, "{id}");
        assert_eq!(def.debugger.as_deref(), *debugger, "{id}");
        assert_eq!(def.console.kind, *console, "{id}");
        assert_eq!(def.requires_external_components, *external, "{id}");
        assert_eq!(
            def.extensions.first(),
            extensions.first().map(|e| &**e).map(str::to_owned).as_ref(),
            "{id}: first extension is the default"
        );
        for ext in *extensions {
            assert!(def.extensions.iter().any(|e| e == ext), "{id} lacks .{ext}");
        }
        assert!(def.enabled, "{id}");
        assert!(r.by_uuid(uuid).is_some());
    }
}

#[test]
fn coderunner_flags_tabs_and_console_options() {
    let r = registry();
    let flags = |id: &str| r.get(id).unwrap().run.flags.clone();
    assert_eq!(flags("c"), "-std=c11");
    assert_eq!(flags("cpp"), "-std=c++11");
    assert_eq!(flags("objective-c"), "-fobjc-arc");
    assert_eq!(flags("objective-cpp"), "-fobjc-arc");
    assert_eq!(flags("latex"), "-file-line-error");
    assert_eq!(flags("python"), "");
    for def in r.all() {
        if def.id == "haskell" || def.id == "yaml" {
            assert!(def.custom_tab_mode && def.soft_tabs, "{}", def.id);
        } else {
            assert!(!def.custom_tab_mode, "{}", def.id);
        }
    }
    assert!(r.get("html").unwrap().console.live);
    assert!(
        r.get("markdown").unwrap().console.live && r.get("markdown").unwrap().console.switchable
    );
    assert!(r.get("php").unwrap().console.switchable);
    assert!(!r.get("javascript-browser").unwrap().console.live);
    assert_eq!(
        r.get("markdown").unwrap().console.renderer.as_deref(),
        Some("markdown")
    );
}

#[test]
fn compile_script_languages_match_coderunner() {
    // CompilerScript = true in CodeRunner (research 04, section 4.A).
    let compiled = [
        "csharp-dotnet",
        "csharp-mono",
        "cpp",
        "c",
        "go",
        "haskell",
        "java",
        "kotlin",
        "latex",
        "objective-cpp",
        "objective-c",
        "rust",
        "swift",
        "typescript-browser",
        "typescript-node",
    ];
    let r = registry();
    for def in r.all() {
        assert_eq!(
            def.has_compile_step(),
            compiled.contains(&def.id.as_str()),
            "{}",
            def.id
        );
        // built-ins never need a shell compile script
        assert!(
            !matches!(def.run.compile, CompileSpec::Shell(_)),
            "{}",
            def.id
        );
    }
}

#[test]
fn installer_hints_and_errors() {
    let r = registry();
    let hint = |id: &str| {
        r.get(id)
            .unwrap()
            .installer
            .clone()
            .map(|i| (i.executable, i.name))
    };
    assert_eq!(hint("go"), Some(("go".into(), "the Go compiler".into())));
    assert_eq!(hint("python"), Some(("python3".into(), "Python 3".into())));
    assert_eq!(
        hint("rust"),
        Some(("rust".into(), "the Rust compiler".into()))
    );
    assert_eq!(hint("haskell"), Some(("haskell".into(), "Haskell".into())));
    assert_eq!(hint("latex"), Some(("latex".into(), "MacTeX".into())));
    assert!(
        r.get("go")
            .unwrap()
            .exit_status_message(127)
            .unwrap()
            .contains("the Go compiler")
    );
    for id in ["csharp-script", "javascript-node", "lua", "php"] {
        assert!(r.get(id).unwrap().errors.contains_key("127"), "{id}");
    }
}

#[test]
fn default_templates_match_coderunner() {
    let with_default = [
        "c",
        "cpp",
        "objective-c",
        "objective-cpp",
        "go",
        "haskell",
        "html",
        "java",
        "kotlin",
        "latex",
        "perl",
        "php",
        "python",
        "ruby",
        "rust",
        "shell",
        "csharp-script",
        "csharp-mono",
    ];
    let r = registry();
    for def in r.all() {
        let has = def.templates.iter().any(|t| t.default);
        assert_eq!(has, with_default.contains(&def.id.as_str()), "{}", def.id);
        assert!(def.templates.iter().filter(|t| t.default).count() <= 1);
        for t in &def.templates {
            assert!(!t.extension.is_empty() && !t.name.is_empty(), "{}", def.id);
            if t.default {
                assert!(
                    !t.name_on_creation,
                    "{}: default templates do not ask for a name",
                    def.id
                );
            }
        }
    }
    // Languages without templates create an empty file.
    for id in [
        "applescript",
        "css",
        "javascript-node",
        "lua",
        "markdown",
        "typescript-node",
    ] {
        assert!(r.get(id).unwrap().templates.is_empty(), "{id}");
    }
    let ctx = TemplateContext::new("Untitled.java", "1/2/26", "u", "U", IndentStyle::default());
    assert_eq!(
        r.new_document("java", &ctx).text,
        "class Untitled {\n\tpublic static void main(String[] args) {\n\t\t\n\t}\n}"
    );
    assert_eq!(r.new_document("java", &ctx).caret, Some((2, 2)));
    assert_eq!(r.new_document("shell", &ctx).text, "#!/bin/bash\n\n");
    // multi-file templates
    let cpp = r.templates_for("cpp");
    let class = cpp.iter().find(|g| g.name == "Class").unwrap();
    assert_eq!(
        class
            .files
            .iter()
            .map(|f| f.extension.as_str())
            .collect::<Vec<_>>(),
        ["h", "cpp"]
    );
    assert!(class.name_on_creation);
    let pair = r.templates_for("c");
    assert_eq!(
        pair.iter()
            .find(|g| g.name == "Header-C Pair")
            .unwrap()
            .files
            .len(),
        2
    );
    assert_eq!(group_templates(&r.get("html").unwrap().templates).len(), 3);
}

#[test]
fn every_template_renders_with_every_indent_style() {
    for def in registry().all() {
        for template in &def.templates {
            for indent in [
                IndentStyle::default(),
                IndentStyle {
                    soft_tabs: true,
                    width: 2,
                },
            ] {
                let ctx = TemplateContext::new(
                    &format!("Thing.{}", template.extension),
                    "1/2/26",
                    "u",
                    "Full Name",
                    indent,
                );
                let out = render_template(&template.text, &ctx);
                assert!(
                    !out.text.contains("___"),
                    "{} / {}: {:?}",
                    def.id,
                    template.name,
                    out.text
                );
                assert!(!out.text.contains("<##>"), "{} / {}", def.id, template.name);
                if indent.soft_tabs {
                    assert!(!out.text.contains('\t'), "{} / {}", def.id, template.name);
                }
            }
        }
    }
}

#[test]
fn diagnostics_are_well_formed() {
    for def in registry().all() {
        for d in &def.diagnostics {
            match d.kind {
                DiagnosticKind::Command => {
                    assert!(
                        d.command.as_deref().is_some_and(|c| !c.is_empty()),
                        "{}/{}",
                        def.id,
                        d.id
                    );
                }
                DiagnosticKind::CompileScript => assert!(
                    def.has_compile_step() || d.output_mode == OutputMode::Unix,
                    "{}/{}",
                    def.id,
                    d.id
                ),
                _ => {}
            }
            if let Some(pattern) = &d.regex {
                let regex = regex_lite_check(pattern)
                    .unwrap_or_else(|e| panic!("{}/{}: {e}", def.id, d.id));
                let names: BTreeSet<&str> = regex.capture_names().flatten().collect();
                assert!(
                    names.contains("line") || names.contains("offset"),
                    "{}/{} regex has no line/offset group",
                    def.id,
                    d.id
                );
                if d.kind != DiagnosticKind::ConsoleHighlight {
                    assert!(
                        names.contains("message"),
                        "{}/{} regex has no message group",
                        def.id,
                        d.id
                    );
                }
                assert!(
                    names.contains("file")
                        || d.kind != DiagnosticKind::ConsoleText
                        || def.id == "php"
                        || def.id == "perl"
                );
            }
            if d.output_mode == OutputMode::Regex {
                assert!(
                    d.regex.is_some(),
                    "{}/{}: regex mode without a regex",
                    def.id,
                    d.id
                );
            }
            assert!(
                !d.update.manual || d.kind == DiagnosticKind::Command,
                "{}/{}",
                def.id,
                d.id
            );
        }
    }
}

fn regex_lite_check(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(pattern)
}

#[test]
fn diagnostic_regexes_match_real_compiler_output() {
    let r = registry();
    let find = |lang: &str, id: &str| {
        let d = r
            .get(lang)
            .unwrap()
            .diagnostics
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .clone();
        regex::Regex::new(d.regex.as_deref().unwrap()).unwrap()
    };
    let clang = find("c", "clang");
    let caps = clang
        .captures("main.c:3:5: error: use of undeclared identifier 'x'")
        .unwrap();
    assert_eq!(
        (&caps["file"], &caps["line"], &caps["column"]),
        ("main.c", "3", "5")
    );
    assert_eq!(&caps["message"], "error: use of undeclared identifier 'x'");

    let cs = find("csharp-dotnet", "console-text");
    let caps = cs
        .captures("Program.cs(12,9): error CS1002: ; expected")
        .unwrap();
    assert_eq!(
        (&caps["line"], &caps["column"], &caps["messageID"]),
        ("12", "9", "CS1002")
    );
    assert!(caps.name("error").is_some() && caps.name("warning").is_none());
    assert_eq!(&caps["message"], "; expected");

    let ts = find("typescript-node", "compile");
    let caps = ts
        .captures("a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.")
        .unwrap();
    assert_eq!(&caps["messageID"], "TS2322");

    let rust = find("rust", "console-text");
    let caps = rust
        .captures("src/main.rs:2:5: 2:6 error: expected one of")
        .unwrap();
    assert_eq!((&caps["file"], &caps["line"]), ("src/main.rs", "2"));

    let php = find("php", "syntax");
    let caps = php
        .captures("Parse error: syntax error, unexpected end of file in /tmp/a.php on line 4")
        .unwrap();
    assert_eq!((&caps["file"], &caps["line"]), ("/tmp/a.php", "4"));

    let perl = find("perl", "syntax");
    let caps = perl
        .captures("syntax error at x.pl line 3, near \"foo bar\"")
        .unwrap();
    assert_eq!(
        (&caps["file"], &caps["line"], &caps["code"]),
        ("x.pl", "3", "foo bar")
    );

    let shellcheck = find("shell", "shellcheck");
    let caps = shellcheck
        .captures("x.sh:3:8: warning: Quote this to prevent word splitting. [SC2086]")
        .unwrap();
    assert!(caps.name("warning").is_some());

    let py = find("python", "syntax");
    let text = "Traceback (most recent call last):\n  File \"<string>\", line 1, in <module>\n  File \"x.py\", line 3\n    foo bar\n        ^^^\nSyntaxError: invalid syntax\n";
    let caps = py.captures(text).unwrap();
    assert_eq!((&caps["file"], &caps["line"]), ("x.py", "3"));
    assert_eq!(&caps["message"], "SyntaxError: invalid syntax");

    let lua = find("lua", "console-text");
    let caps = lua
        .captures("lua: x.lua:5: attempt to call a nil value")
        .unwrap();
    assert_eq!((&caps["file"], &caps["line"]), ("x.lua", "5"));
}

#[test]
fn editor_syntax_names_are_known_to_nsh_or_registrable() {
    for def in registry().all() {
        if is_nsh_builtin_editor_language(&def.editor_syntax) {
            assert!(def.editor_registration().is_none());
        } else {
            let reg = def.editor_registration().unwrap_or_else(|| {
                panic!("{}: unknown editor syntax {}", def.id, def.editor_syntax)
            });
            assert!(!reg.syntax_hint.is_empty());
            assert!(
                matches!(def.editor_syntax.as_str(), "AppleScript" | "LaTeX"),
                "{}",
                def.id
            );
        }
    }
}

/// Checks `NSH_BUILTIN_EDITOR_LANGUAGES` against the pinned NSH source when it
/// is checked out (skips visibly otherwise).
#[test]
fn nsh_language_list_matches_the_nsh_source() {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        std::env::var("NSH_SRC").unwrap_or_default(),
        format!(
            "{home}/.cargo/git/checkouts/nsh-9218b667fc8c8325/7befd58/crates/nsh-base/src/editor/language.rs"
        ),
    ];
    let Some(path) = candidates
        .iter()
        .find(|p| !p.is_empty() && Path::new(p).is_file())
    else {
        eprintln!("SKIPPED: NSH source not found (set NSH_SRC to nsh-base/src/editor/language.rs)");
        return;
    };
    let source = std::fs::read_to_string(path).unwrap();
    let mut names: Vec<String> = Vec::new();
    for line in source.lines() {
        if let Some(rest) = line.trim().strip_prefix("name: \"")
            && let Some(name) = rest.split('"').next()
            && !name.is_empty()
        {
            names.push(name.to_owned());
        }
    }
    assert!(
        names.len() > 40,
        "parsed only {} names from {path}",
        names.len()
    );
    for name in &names {
        assert!(
            NSH_BUILTIN_EDITOR_LANGUAGES.contains(&name.as_str()),
            "NSH knows `{name}` but NSH_BUILTIN_EDITOR_LANGUAGES does not list it"
        );
    }
    for name in NSH_BUILTIN_EDITOR_LANGUAGES {
        assert!(
            names.iter().any(|n| n == name),
            "`{name}` is listed but not in the NSH source"
        );
    }
}

/// Plans a run for every runnable built-in on a real sample file and checks
/// the shape of the result.
#[test]
fn every_runnable_builtin_plans_a_run() {
    let r = registry();
    let tmp = tempfile::tempdir().unwrap();
    let mut planned = 0;
    for def in r.all() {
        if !def.is_runnable() {
            continue;
        }
        let dir = tmp.path().join(&def.id);
        std::fs::create_dir_all(&dir).unwrap();
        let ext = def.default_extension().unwrap();
        let file = if def.id == "java" {
            "Sample.java".to_owned()
        } else {
            format!("sample.{ext}")
        };
        let ctx = TemplateContext::new(&file, "1/2/26", "u", "U", IndentStyle::default());
        let text = r.new_document(&def.id, &ctx).text;
        std::fs::write(
            dir.join(&file),
            if text.is_empty() {
                "x".to_owned()
            } else {
                text
            },
        )
        .unwrap();
        if def.id == "csharp-dotnet" {
            std::fs::write(dir.join("p.csproj"), "<Project/>").unwrap();
        }
        let out = tmp.path().join("out");
        let vars = Vars::for_source(&dir, &file).with_tmpdir(&out);
        let plan = def
            .plan_run(&RunContext::new(vars))
            .unwrap_or_else(|e| panic!("{}: {e}", def.id));

        match &plan.compile {
            CompilePlan::None => assert!(!def.has_compile_step(), "{}", def.id),
            CompilePlan::Steps(steps) => {
                assert!(!steps.is_empty(), "{}", def.id);
                for step in steps {
                    assert!(!step.program.is_empty());
                    assert!(
                        step.args.iter().all(|a| !a.contains('$')),
                        "{}: unexpanded {:?}",
                        def.id,
                        step.args
                    );
                    assert!(step.cwd.starts_with(tmp.path()), "{}", def.id);
                }
            }
            CompilePlan::Shell(_) => panic!("{} uses a shell compile script", def.id),
        }
        if def.run.command.trim().is_empty() {
            assert!(def.console.renderer.is_some(), "{}", def.id);
            continue;
        }
        let resolved = plan
            .resolve_command(None)
            .unwrap_or_else(|e| panic!("{}: {e}", def.id));
        match &resolved {
            ResolvedCommand::Argv(argv) => {
                assert!(!argv[0].is_empty());
                assert!(
                    argv.iter()
                        .all(|a| !a.contains('$') || a.contains("<script") || a.contains("jquery")),
                    "{}: {argv:?}",
                    def.id
                );
            }
            ResolvedCommand::Shell(command) => {
                assert!(
                    matches!(def.id.as_str(), "css"),
                    "{} unexpectedly needs a shell: {command}",
                    def.id
                );
            }
        }
        planned += 1;
    }
    assert!(planned >= 28, "planned {planned}");
}

#[test]
fn spot_check_plans() {
    let r = registry();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let out = dir.join("out");
    let plan_of = |id: &str, file: &str, flags: Option<&str>, debugging: bool| {
        let mut ctx = RunContext::new(Vars::for_source(dir, file).with_tmpdir(&out));
        ctx.compile_flags = flags.map(str::to_owned);
        ctx.debugging = debugging;
        r.get(id).unwrap().plan_run(&ctx).unwrap()
    };
    let steps = |plan: &scratch_language::RunPlan| match &plan.compile {
        CompilePlan::Steps(steps) => steps.clone(),
        other => panic!("{other:?}"),
    };
    let o = out.to_string_lossy().into_owned();

    std::fs::write(dir.join("main.c"), "int main(){}").unwrap();
    let plan = plan_of("c", "main.c", None, true);
    let s = &steps(&plan)[0];
    assert_eq!(s.via, ["xcrun"]);
    assert_eq!(s.program, "clang");
    assert_eq!(
        s.args[..4],
        [
            "-o".to_owned(),
            format!("{o}/main"),
            "main.c".into(),
            "-std=c11".into()
        ]
    );
    assert_eq!(s.args.last().map(String::as_str), Some("-g"));
    assert_eq!(
        plan.run_argv("/bin/bash", false, None).unwrap(),
        [format!("{o}/main")]
    );

    std::fs::write(dir.join("main.go"), "package main").unwrap();
    let plan = plan_of("go", "main.go", None, true);
    let s = &steps(&plan)[0];
    assert_eq!(s.program, "go");
    assert_eq!(
        s.args,
        [
            "build",
            "-o",
            &format!("{o}/main"),
            "main.go",
            "-gcflags",
            "-N -l"
        ]
    );

    std::fs::write(dir.join("a b.rs"), "fn main(){}").unwrap();
    let plan = plan_of("rust", "a b.rs", None, false);
    let s = &steps(&plan)[0];
    assert_eq!(
        s.args,
        [
            "-o",
            &format!("{o}/a b"),
            "--crate-name",
            "a_b",
            "a b.rs",
            "--edition",
            "2021"
        ]
    );

    std::fs::write(dir.join("t.ts"), "let x: number = 1;").unwrap();
    let plan = plan_of("typescript-node", "t.ts", None, false);
    assert_eq!(
        plan.run_argv("/bin/bash", false, None).unwrap(),
        ["node", &format!("{o}/t.build/t.js")]
    );
    let plan = plan_of("typescript-browser", "t.ts", None, false);
    assert_eq!(
        plan.run_argv("/bin/bash", false, None).unwrap(),
        [
            "echo",
            &format!("<script src=\"file://{o}/t.build/t.js\"></script>")
        ]
    );

    std::fs::write(dir.join("p.py"), "print(1)").unwrap();
    let mut ctx = RunContext::new(Vars::for_source(dir, "p.py"));
    ctx.arguments = vec!["--flag".into(), "two words".into()];
    let plan = r.get("python").unwrap().plan_run(&ctx).unwrap();
    assert_eq!(
        plan.run_argv("/bin/bash", false, None).unwrap(),
        ["python3", "p.py", "--flag", "two words"]
    );

    // The CSS run command needs a shell, quoted like CodeRunner does.
    std::fs::write(dir.join("it's.css"), "a{}").unwrap();
    let plan = plan_of("css", "it's.css", None, false);
    assert_eq!(
        plan.resolve_command(None).unwrap(),
        ResolvedCommand::Shell("echo \"<style>\"; cat 'it'\\''s.css'; echo \"</style>\"".into())
    );

    // Single file in a directory of Swift files: only main.swift compiles them all.
    std::fs::write(dir.join("main.swift"), "print(1)").unwrap();
    std::fs::write(dir.join("util.swift"), "").unwrap();
    let s = steps(&plan_of("swift", "main.swift", None, false))[0].clone();
    assert_eq!(s.via, ["xcrun", "-sdk", "macosx"]);
    assert_eq!(
        s.args[2..4],
        ["main.swift".to_owned(), "util.swift".to_owned()]
    );
    let s = steps(&plan_of("swift", "util.swift", None, false))[0].clone();
    assert_eq!(s.args[2..3], ["util.swift".to_owned()]);
    assert_eq!(s.args.len(), 3);
}

#[test]
fn dotnet_needs_a_project() {
    let r = registry();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("p.cs"), "").unwrap();
    let err = r
        .get("csharp-dotnet")
        .unwrap()
        .plan_run(&RunContext::new(Vars::for_source(tmp.path(), "p.cs")))
        .unwrap_err();
    assert!(err.to_string().contains("dotnet new console"), "{err}");
}

#[test]
fn java_in_a_package_directory_builds_from_the_source_root() {
    let r = registry();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("src/com/acme");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("App.java"),
        "package com.acme;\npublic class App { public static void main(String[] a) {} }",
    )
    .unwrap();
    let out = tmp.path().join("out");
    let plan = r
        .get("java")
        .unwrap()
        .plan_run(&RunContext::new(
            Vars::for_source(&dir, "App.java").with_tmpdir(&out),
        ))
        .unwrap();
    let CompilePlan::Steps(steps) = &plan.compile else {
        panic!()
    };
    assert_eq!(steps[0].cwd, tmp.path().join("src"));
    assert_eq!(
        steps[0].args.last().map(String::as_str),
        Some("com/acme/App.java")
    );
    assert!(
        steps[0]
            .args
            .windows(2)
            .any(|w| w == ["-encoding", "UTF-8"])
    );
    let argv = plan.run_argv("/bin/bash", false, None).unwrap();
    assert_eq!(argv[0], "java");
    assert_eq!(argv.last().map(String::as_str), Some("com.acme.App"));
}

#[test]
fn defaults_for_new_user_languages() {
    // "Create a new language": everything empty, nothing runnable.
    let def = LanguageDef::new("mine", "Mine");
    assert!(!def.is_runnable());
    assert_eq!(def.grammar, "text.plain");
    assert_eq!(def.tab_width, 4);
    assert!(def.validate().is_empty());
}

#[test]
fn every_builtin_is_detected_by_its_own_templates_or_shebang_or_name() {
    let r = registry();
    // languages whose default template is distinctive enough to be recognised
    let expect = [
        ("c", "c"),
        ("cpp", "cpp"),
        ("go", "go"),
        ("rust", "rust"),
        ("java", "java"),
        ("kotlin", "kotlin"),
        ("haskell", "haskell"),
        ("html", "html"),
        ("latex", "latex"),
        ("php", "php"),
        ("python", "python"),
        ("ruby", "ruby"),
        ("perl", "perl"),
        ("shell", "shell"),
        ("csharp-mono", "csharp-dotnet"),
        ("objective-c", "objective-c"),
    ];
    let ctx = TemplateContext::new("x", "1/2/26", "u", "U", IndentStyle::default());
    for (id, expected) in expect {
        let text = r.new_document(id, &ctx).text;
        let guess = r
            .guess_from_text(&text)
            .unwrap_or_else(|| panic!("{id}: no guess for {text:?}"));
        assert_eq!(guess.language.id, expected, "{id}: {text:?}");
    }
}
