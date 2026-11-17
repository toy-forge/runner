//! Parses real tool output (captured from clang, swiftc, rustc, go, python,
//! node, javac, bash, ruby, perl, tsc, osacompile, java; paths rewritten to
//! `/work`) with the parsers and places the result in the source buffer.

use std::path::Path;

use scratch_diagnostics::parse::{self, parse_auto};
use scratch_diagnostics::{
    AnnotationSet, DocumentContext, Location, Origin, ParseOptions, Severity, resolve_all,
};
use scratch_language::{LanguageRegistry, OutputMode};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn opts() -> ParseOptions {
    ParseOptions::default()
}

/// The text of the first annotation's range after placing in `source`.
fn placed(diags: &[scratch_diagnostics::Diagnostic], source: &str, path: &str) -> Vec<(Severity, String, String)> {
    let ctx = DocumentContext::new(source).path(Path::new(path));
    let (here, _) = resolve_all(diags, &ctx);
    here.iter()
        .map(|a| {
            (
                a.severity,
                a.message.clone(),
                a.range.clone().map(|r| source[r].to_owned()).unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn clang_with_parseable_fixits() {
    let out = fixture("clang-main-c.txt");
    let d = parse::parse_unix(&out, &opts());
    assert_eq!(d.len(), 1, "{d:#?}");
    assert_eq!(d[0].message, "expected ';' at end of declaration");
    assert_eq!(d[0].location, Some(Location::line_col(3, 19)));
    assert_eq!(d[0].fixes.len(), 1, "the fix-it line after the caret frame");
    let source = "#include <stdio.h>\nint main(void) {\n    int unused = 3\n    printf(\"%d\\n\", undefined_var);\n    return 0;\n}\n";
    let ctx = DocumentContext::new(source).path(Path::new("/work/main.c"));
    let (here, _) = resolve_all(&d, &ctx);
    assert_eq!(here[0].fixes[0].title, "Insert ';'");
    let mut set = AnnotationSet::new();
    set.replace_source("clang", Origin::Document, here);
    assert!(set.all_fixes().apply_to(source).contains("int unused = 3;\n"));
}

#[test]
fn swiftc_new_style_frames_are_ignored() {
    let out = fixture("swiftc-main.txt");
    let d = parse::parse_unix(&out, &opts());
    assert_eq!(d.len(), 2, "{d:#?}");
    assert_eq!(d[0].message, "cannot convert value of type 'String' to specified type 'Int'");
    assert_eq!(d[1].location, Some(Location::line_col(2, 7)));
    let source = "let x: Int = \"hello\"\nprint(y)\n";
    let p = placed(&d, source, "/work/main.swift");
    assert_eq!(p[0].2, "\"hello\"");
    assert_eq!(p[1].2, "y");
}

#[test]
fn rustc_human_and_short() {
    let human = fixture("rustc-human.txt");
    let d = parse::parse_rustc(&human, &opts());
    assert_eq!(d.len(), 1, "{d:#?}");
    assert_eq!(d[0].message_id.as_deref(), Some("E0425"));
    let source = "fn main() {\n    let y = 1;\n    println!(\"{}\", x);\n}\n";
    let p = placed(&d, source, "/work/main.rs");
    assert_eq!(p[0].2, "x");
    let short = fixture("rustc-short.txt");
    let d = parse::parse_unix(&short, &opts());
    assert_eq!(d.len(), 1, "{d:#?}");
    assert_eq!(d[0].message_id.as_deref(), Some("E0425"));
    assert_eq!(placed(&d, source, "/work/main.rs")[0].2, "x");
    // The all-formats parser handles both and does not double count.
    assert_eq!(parse_auto(&human, &opts()).len(), 1);
    assert_eq!(parse_auto(&short, &opts()).len(), 1);
}

#[test]
fn go_build_and_panic() {
    let out = fixture("go-build.txt");
    let d = parse::parse_unix(&out, &opts());
    assert_eq!(d.len(), 2);
    let source = "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tvar y int\n\tfmt.Println(x)\n}\n";
    let p = placed(&d, source, "/work/main.go");
    assert_eq!(p[0], (Severity::Error, "declared and not used: y".into(), "y".into()));
    assert_eq!(p[1].2, "x");
    let panic = fixture("go-panic.txt");
    let d = parse::parse_go_panic(&panic, &opts());
    assert_eq!(d.len(), 1);
    assert!(d[0].runtime);
    assert_eq!(d[0].file.as_deref(), Some(Path::new("/work/panic.go")));
    assert_eq!(d[0].children[0].message, "'main.get' called here");
    assert_eq!(d[0].children[0].location.as_ref().and_then(Location::start_line), Some(6));
}

#[test]
fn python_syntax_error_from_compile_wrapper() {
    let out = fixture("python-syntax.txt");
    let d = parse::parse_python(&out, &opts());
    assert_eq!(d.len(), 1, "{d:#?}");
    assert!(!d[0].runtime, "a syntax error is not a runtime error");
    assert_eq!(d[0].message, "SyntaxError: invalid syntax");
    assert_eq!(d[0].file.as_deref(), Some(Path::new("bad.py")));
    let source = "def f(x):\n    return x +\n\nprint(f(1))\n";
    let p = placed(&d, source, "/work/bad.py");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].2, "+", "the caret points just past the `+`; the previous character is underlined");
}

#[test]
fn python_runtime_traceback() {
    let out = fixture("python-traceback.txt");
    let d = parse::parse_python(&out, &opts());
    assert_eq!(d.len(), 1);
    assert!(d[0].runtime);
    assert_eq!(d[0].children.len(), 2);
    let source = "def helper(x):\n    return 1 / x\n\ndef main():\n    helper(0)\n\nmain()\n";
    let ctx = DocumentContext::new(source).path(Path::new("/work/run.py"));
    let (here, _) = resolve_all(&d, &ctx);
    let a = &here[0];
    assert_eq!(&source[a.range.clone().unwrap()], "1 / x", "3.13 carets cover the failing expression");
    assert_eq!(a.children.len(), 2);
    assert_eq!(a.children[0].message, "'helper' called here");
    assert_eq!(a.children[1].message, "'main' called here");
    let r = |c: &scratch_diagnostics::Annotation| c.range.clone().map(|r| source[r].to_owned());
    assert_eq!(r(&a.children[0]).as_deref(), Some("    helper(0)"));
}

#[test]
fn node_check_and_runtime() {
    let out = fixture("node-check.txt");
    let d = parse::parse_node(&out, &opts());
    assert_eq!(d.len(), 1);
    assert!(!d[0].runtime);
    assert_eq!(d[0].message, "SyntaxError: Unexpected identifier 'bar'");
    let source = "const a = 1;\nfoo bar;\n";
    assert_eq!(placed(&d, source, "/work/bad.js")[0].2, "bar");
    // The unix mode finds nothing in node's format; the all-formats parser does.
    assert!(parse::parse_unix(&out, &opts()).is_empty());
    assert_eq!(parse_auto(&out, &opts()).len(), 1);

    let rt = fixture("node-runtime.txt");
    let d = parse::parse_node(&rt, &opts());
    assert_eq!(d.len(), 1);
    assert!(d[0].runtime);
    assert_eq!(d[0].children.len(), 2);
    assert_eq!(d[0].children[0].message, "'inner' called here");
    assert_eq!(d[0].children[1].message, "'outer' called here");
    let source = "function inner() { throw new Error('boom'); }\nfunction outer() { inner(); }\nouter();\n";
    let p = placed(&d, source, "/work/run.js");
    assert_eq!(p[0].2, "throw", "the caret marks the start of the throw statement");
}

#[test]
fn javac_with_symbol_details() {
    let out = fixture("javac-main.txt");
    let d = parse::parse_unix(&out, &opts());
    assert_eq!(d.len(), 2, "{d:#?}");
    assert_eq!(d[1].message, "cannot find symbol");
    assert_eq!(d[1].children.len(), 2);
    assert_eq!(d[1].children[1].message, "location: class Main");
    let source = "public class Main {\n    public static void main(String[] args) {\n        int x = \"s\";\n        foo();\n    }\n}\n";
    let p = placed(&d, source, "/work/Main.java");
    assert_eq!(p[0].2, "\"s\"", "the caret column gives the range start");
    assert_eq!(p[1].2, "foo");
}

#[test]
fn java_exception_frames() {
    let out = fixture("java-exception.txt");
    let d = parse::parse_java(&out, &opts());
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].message, "java.lang.IllegalStateException: boom");
    let source = "public class Run {\n    static void fail() { throw new IllegalStateException(\"boom\"); }\n    public static void main(String[] args) { fail(); }\n}\n";
    let ctx = DocumentContext::new(source).path(Path::new("/work/Run.java"));
    let (here, _) = resolve_all(&d, &ctx);
    assert_eq!(here.len(), 1, "Run.java:2 is this document");
    assert!(here[0].runtime);
    assert_eq!(here[0].children[0].message, "'Run.fail' called here");
}

#[test]
fn shell_ruby_perl_applescript_tsc() {
    let sh = parse::parse_unix(&fixture("bash-n.txt"), &opts());
    assert_eq!(sh.len(), 1);
    assert_eq!(sh[0].location, Some(Location::line(3)));
    assert_eq!(sh[0].message, "syntax error: unexpected end of file");

    let rb = parse::parse_unix(&fixture("ruby-c.txt"), &opts());
    assert_eq!(rb.len(), 1);
    assert_eq!(rb[0].location, Some(Location::line(2)));
    let rt = parse::parse_ruby(&fixture("ruby-runtime.txt"), &opts());
    assert_eq!(rt.len(), 1);
    assert_eq!(rt[0].message, "ZeroDivisionError: divided by 0");
    assert_eq!(rt[0].children.len(), 2);

    let pl = parse_auto(&fixture("perl-c.txt"), &opts());
    assert_eq!(pl.len(), 1, "{pl:#?}");
    assert_eq!(pl[0].location, Some(Location::line(1)));

    let osa = parse::parse_unix(&fixture("osacompile.txt"), &opts());
    assert_eq!(osa.len(), 1);
    assert_eq!(osa[0].message_id, None);
    assert!(osa[0].message.starts_with("Expected expression"));

    let reg = LanguageRegistry::builtin();
    let ts = reg.get("typescript-node").unwrap();
    let compile = ts.diagnostics.iter().find(|d| d.id == "compile").unwrap();
    assert_eq!(compile.output_mode, OutputMode::Regex);
    let d = parse::interpret(&fixture("tsc.txt"), compile.output_mode, compile.regex.as_deref(), &opts()).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].message_id.as_deref(), Some("TS2322"));
    assert_eq!(d[0].location, Some(Location::line_col(1, 7)));
    let source = "const n: number = \"s\";\n";
    assert_eq!(placed(&d, source, "/work/bad.ts")[0].2, "n");
}

#[test]
fn rust_panic_new_format() {
    let d = parse::parse_rust_panic(&fixture("rust-panic.txt"), &opts());
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].message, "panicked: index out of bounds: the len is 3 but the index is 5");
    assert_eq!(d[0].location, Some(Location::Lines { line: 3, column: Some(21), end_line: None, end_column: None, unit: scratch_diagnostics::ColumnUnit::Chars }));
}

#[test]
fn language_regexes_parse_the_real_output_of_their_tools() {
    // (language, diagnostic id, fixture, expected number of diagnostics)
    let reg = LanguageRegistry::builtin();
    let cases = [
        ("c", "clang", "clang-main-c.txt", 1),
        ("go", "compile", "go-build.txt", 2),
        ("rust", "compile", "rustc-short.txt", 1),
        ("applescript", "syntax", "osacompile.txt", 1),
        ("typescript-node", "compile", "tsc.txt", 1),
    ];
    for (lang, id, file, expected) in cases {
        let def = reg.get(lang).unwrap().diagnostics.iter().find(|d| d.id == id).unwrap();
        let d = parse::interpret(&fixture(file), def.output_mode, def.regex.as_deref(), &opts()).unwrap();
        assert_eq!(d.len(), expected, "{lang}/{id}: {d:#?}");
    }
}

#[test]
fn placed_console_annotations_form_a_set() {
    let out = format!("{}{}", fixture("go-build.txt"), fixture("go-panic.txt"));
    let d = parse_auto(&out, &ParseOptions::console(Some("go")));
    assert_eq!(d.iter().filter(|d| d.runtime).count(), 1);
    assert!(d.iter().all(|d| d.origin == Origin::Console));
    let source = "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tvar y int\n\tfmt.Println(x)\n}\n";
    let ctx = DocumentContext::new(source).path(Path::new("/work/main.go"));
    let (here, foreign) = resolve_all(&d, &ctx);
    assert_eq!(here.len(), 2);
    assert_eq!(foreign.len(), 1, "the panic is about panic.go");
    let mut set = AnnotationSet::new();
    set.replace_source("console", Origin::Console, here);
    assert_eq!(set.counts().errors, 2);
}

#[test]
fn clang_fixits_notes_and_ranges() {
    let out = fixture("clang-fixits.txt");
    let source = fixture("clang-fixits.c");
    let d = parse::parse_unix(&out, &opts());
    assert_eq!(d.len(), 2, "{d:#?}");
    // Warning: two alternative fixes taken from its notes, message id from the option tag.
    assert_eq!(d[0].severity, Severity::Warning);
    assert_eq!(d[0].message_id.as_deref(), Some("-Wparentheses"));
    assert_eq!(d[0].children.len(), 2);
    assert_eq!(d[0].fixes.len(), 2);
    assert_eq!(d[0].fixes[0].edits.len(), 2, "both parentheses are one fix");
    // Error: a single fix-it.
    assert_eq!(d[1].fixes.len(), 1);

    let ctx = DocumentContext::new(&source).path(Path::new("/work/fx2.c"));
    let (here, _) = resolve_all(&d, &ctx);
    assert_eq!(&source[here[0].range.clone().unwrap()], "x = 2", "the ~~^~~ range, not just the caret");
    let titles: Vec<&str> = here[0].fixes.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["Fix", "Replace '=' with '=='"]);
    assert_eq!(here[1].fixes[0].title, "Insert ';'");

    let mut set = AnnotationSet::new();
    set.replace_source("clang", Origin::Document, here);
    // Apply All Fixes only applies unambiguous fixes: the `;`.
    let all = set.all_fixes();
    assert_eq!(all.titles, ["Insert ';'"]);
    assert!(all.apply_to(&source).contains("return 1;"));
    // Choosing the second alternative of the warning.
    let id = set.visible()[0].id;
    let eq = set.fix_plan(id, 1).unwrap().apply_to(&source);
    assert!(eq.contains("if (x == 2)"), "{eq}");
    let parens = set.fix_plan(id, 0).unwrap().apply_to(&source);
    assert!(parens.contains("if ((x = 2))"), "{parens}");
}
