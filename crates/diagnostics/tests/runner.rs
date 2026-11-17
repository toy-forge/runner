//! End-to-end: jobs against real processes. Tests that need an external tool
//! skip visibly (`SKIPPED: ...` on stderr) when it is missing.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use scratch_diagnostics::*;
use scratch_language::{DiagnosticDef, DiagnosticKind, DiagnosticOptions, LanguageDef, LanguageRegistry, OutputMode, UpdateMode};

fn registry() -> LanguageRegistry {
    LanguageRegistry::builtin()
}

fn lang(id: &str) -> Arc<LanguageDef> {
    Arc::new(registry().get(id).unwrap_or_else(|| panic!("language {id}")).clone())
}

fn builtin_def(id: &str, diag: &str) -> DiagnosticDef {
    registry()
        .get(id)
        .unwrap()
        .diagnostics
        .iter()
        .find(|d| d.id == diag)
        .unwrap_or_else(|| panic!("{id}/{diag}"))
        .clone()
}

fn command_def(command: &str, mode: OutputMode, regex: Option<&str>) -> DiagnosticDef {
    DiagnosticDef {
        id: "test".into(),
        uuid: None,
        name: "Test".into(),
        kind: DiagnosticKind::Command,
        update: UpdateMode { on_edit: true, on_run: false, manual: false },
        options: DiagnosticOptions::default(),
        output_mode: mode,
        regex: regex.map(str::to_owned),
        output_script: None,
        command: Some(command.to_owned()),
        flags: None,
        key_equivalent: None,
    }
}

fn have(tool: &str) -> bool {
    scratch_diagnostics::exec::find_program(tool, &std::env::var("PATH").unwrap_or_default()).is_some()
}

macro_rules! need {
    ($($tool:expr),+) => {
        $(if !have($tool) {
            eprintln!("SKIPPED: {} is not installed", $tool);
            return;
        })+
    };
}

fn run(job: &DiagnosticJob) -> JobOutput {
    run_job(job, &CancelToken::new()).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn command_runs_on_a_temp_copy_of_the_unsaved_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("note.txt");
    std::fs::write(&real, "on disk\n").unwrap();
    std::fs::write(dir.path().join("sibling.txt"), "s").unwrap();
    let buffer = "line one\nline two\nTODO three\n";
    // Prints the buffer's line count, lists the directory and reports where TODO is.
    let def = command_def(
        "printf '%s:3: warning: todo in %s\\n' $filename \"$(wc -l < $filename | tr -d ' ') lines, $(ls | tr '\\n' ' ')\"",
        OutputMode::Unix,
        None,
    );
    let job = DiagnosticJob::new(lang("plain-text"), def, buffer).file(&real);
    let out = run(&job);
    assert_eq!(out.diagnostics.len(), 1, "{}", out.raw_output);
    assert!(out.raw_output.contains("3 lines"), "ran on the buffer, not the file: {}", out.raw_output);
    assert!(out.raw_output.contains("sibling.txt"), "siblings are linked: {}", out.raw_output);
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "on disk\n", "the real file is untouched");
    assert!(!out.aliases.is_empty());
    let (here, foreign) = out.annotations(buffer, Some(&real));
    assert!(foreign.is_empty());
    assert_eq!(here.len(), 1);
    assert_eq!(&buffer[here[0].range.clone().unwrap()], "TODO three");
    assert!(here[0].whole_line);
    assert_eq!(here[0].source.as_deref(), Some("test"));
    assert!(!out.aliases[0].exists(), "the temp directory is gone after the job");
}

#[test]
fn saved_buffer_runs_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("a.txt");
    std::fs::write(&real, "x\n").unwrap();
    let def = command_def("pwd", OutputMode::Unix, None);
    let out = run(&DiagnosticJob::new(lang("plain-text"), def, "x\n").file(&real).saved(true));
    let pwd = std::fs::canonicalize(out.raw_output.trim()).unwrap();
    assert_eq!(pwd, std::fs::canonicalize(dir.path()).unwrap());
    assert!(out.aliases.is_empty());
}

#[test]
fn environment_and_path_come_from_the_caller() {
    let bin = tempfile::tempdir().unwrap();
    let tool = bin.path().join("fakelint");
    std::fs::write(&tool, "#!/bin/sh\necho \"$1:2:5: error: fake lint says $MY_VAR [$CR_DIAGNOSTICS]\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let def = command_def("fakelint $filename", OutputMode::Unix, None);
    let env = RunnerEnv::default()
        .with_var("MY_VAR", "hello")
        .with_path(format!("{}:/usr/bin:/bin", bin.path().display()));
    let job = DiagnosticJob::new(lang("plain-text"), def.clone(), "a\nb\n").env(env);
    let out = run(&job);
    assert_eq!(out.diagnostics.len(), 1, "{}", out.raw_output);
    assert_eq!(out.diagnostics[0].message, "fake lint says hello [1]");
    // Without that PATH the tool is missing.
    let job = DiagnosticJob::new(lang("plain-text"), def, "a\n").env(RunnerEnv::default().with_path("/usr/bin:/bin"));
    match run_job(&job, &CancelToken::new()) {
        Err(RunError::ToolMissing(t)) => assert_eq!(t.program, "fakelint"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn missing_tool_offers_homebrew() {
    let def = command_def("shellcheck -f gcc $filename", OutputMode::Unix, None);
    let job = DiagnosticJob::new(lang("shell"), def, "echo $x\n").env(RunnerEnv::default().with_path("/usr/bin:/bin"));
    match run_job(&job, &CancelToken::new()) {
        Err(RunError::ToolMissing(t)) => {
            assert_eq!(t.install_command().as_deref(), Some("brew install shellcheck"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn timeout_and_cancel_clean_up() {
    let root = tempfile::tempdir().unwrap();
    let def = command_def("sleep 30", OutputMode::Unix, None);
    let job = DiagnosticJob::new(lang("plain-text"), def.clone(), "x")
        .timeout(Duration::from_millis(300))
        .temp_root(root.path());
    let started = Instant::now();
    assert_eq!(run_job(&job, &CancelToken::new()).unwrap_err(), RunError::Timeout(Duration::from_millis(300)));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0, "no temp dirs left behind");

    let runner = DiagnosticsRunner::new();
    let job = DiagnosticJob::new(lang("plain-text"), def, "x").temp_root(root.path());
    let handle = runner.spawn(JobToken(7), job);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(runner.running_count(), 1);
    handle.cancel();
    let result = runner.recv_timeout(Duration::from_secs(10)).expect("a result");
    assert_eq!(result.token, JobToken(7));
    assert_eq!(result.outcome.unwrap_err(), RunError::Cancelled);
    assert_eq!(runner.running_count(), 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn custom_script_json_protocol() {
    let mut def = command_def("printf 'raw output\\n'", OutputMode::CustomScript, None);
    def.output_script = Some(
        "#!/bin/sh\nread first\necho \"[{\\\"message\\\": \\\"script saw: $first\\\", \\\"type\\\": \\\"warning\\\", \\\"location\\\": {\\\"line\\\": 2, \\\"column\\\": 3, \\\"endColumn\\\": 5}, \\\"fixes\\\": [{\\\"location\\\": {\\\"line\\\": 2, \\\"column\\\": 3, \\\"endColumn\\\": 5}, \\\"replacement\\\": \\\"ZZ\\\"}]}]\"\n"
            .to_owned(),
    );
    let buffer = "abcdef\nghijkl\n";
    let out = run(&DiagnosticJob::new(lang("plain-text"), def.clone(), buffer));
    assert_eq!(out.diagnostics.len(), 1, "{out:?}");
    let (here, _) = out.annotations(buffer, None);
    assert_eq!(here[0].message, "script saw: raw output");
    assert_eq!(here[0].severity, Severity::Warning);
    assert_eq!(&buffer[here[0].range.clone().unwrap()], "ij");
    assert_eq!(here[0].fixes[0].title, "Replace 'ij' with 'ZZ'");
    // A broken script is reported, not swallowed.
    def.output_script = Some("#!/bin/sh\necho 'oops' >&2\nexit 3\n".into());
    let err = run_job(&DiagnosticJob::new(lang("plain-text"), def.clone(), buffer), &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, RunError::Script(m) if m.contains("oops")), "{err:?}");
    def.output_script = Some("#!/bin/sh\necho 'not json'\n".into());
    let err = run_job(&DiagnosticJob::new(lang("plain-text"), def.clone(), buffer), &CancelToken::new()).unwrap_err();
    assert!(matches!(err, RunError::Parse(ParseError::Json(_))), "{err:?}");
    // An uuid reference without the script text is an error, with a custom path it works.
    def.output_script = Some("0123-uuid".into());
    let err = run_job(&DiagnosticJob::new(lang("plain-text"), def.clone(), buffer), &CancelToken::new()).unwrap_err();
    assert!(matches!(err, RunError::Script(_)));
    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(script.path(), "#!/bin/sh\necho '[{\"message\":\"external\"}]'\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut job = DiagnosticJob::new(lang("plain-text"), def, buffer);
    job.script_path = Some(script.path().to_path_buf());
    let out = run(&job);
    assert_eq!(out.diagnostics[0].message, "external");
}

#[test]
fn checkstyle_mode() {
    let def = command_def(
        "printf '<checkstyle><file name=\"x\"><error line=\"2\" column=\"1\" severity=\"warning\" message=\"hmm\" source=\"r.x\"/></file></checkstyle>'",
        OutputMode::CheckStyle,
        None,
    );
    let out = run(&DiagnosticJob::new(lang("plain-text"), def, "a\nb\n"));
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].severity, Severity::Warning);
    assert_eq!(out.diagnostics[0].message_id.as_deref(), Some("r.x"));
}

#[test]
fn fallback_to_known_formats_when_the_regex_finds_nothing() {
    let cmd = "printf 'Traceback (most recent call last):\\n  File \"%s\", line 2, in <module>\\n    boom()\\nNameError: name boom is not defined\\n' $filename; exit 1";
    let def = command_def(cmd, OutputMode::Regex, Some(r"^NOPE(?<line>\d+): (?<message>.*)$"));
    let out = run(&DiagnosticJob::new(lang("python"), def.clone(), "x = 1\nboom()\n"));
    assert!(out.used_fallback);
    assert_eq!(out.diagnostics.len(), 1);
    assert!(out.diagnostics[0].runtime);
    let mut job = DiagnosticJob::new(lang("python"), def, "x = 1\nboom()\n");
    job.fallback_auto = false;
    assert!(run(&job).diagnostics.is_empty());
}

#[test]
fn test_run_reports_raw_output_and_count() {
    let def = command_def(
        "printf 'L1: first\\nL2: second\\n'",
        OutputMode::Regex,
        Some(r"^L(?<line>\d+): (?<message>.*)$"),
    );
    let job = DiagnosticJob::new(lang("plain-text"), def, "a\nb\nc\n");
    let report = perform_test_run(&job, &CancelToken::new());
    assert!(report.error.is_none());
    assert_eq!(report.raw_output, "L1: first\nL2: second\n");
    assert_eq!(report.annotations.len(), 2);
    assert_eq!(report.summary(), "The diagnostics test run produced 2 diagnostics.");
    // A bad regex is an error in the report, not a panic.
    let bad = command_def("echo x", OutputMode::Regex, Some("("));
    let report = perform_test_run(&DiagnosticJob::new(lang("plain-text"), bad, "a\n"), &CancelToken::new());
    assert!(matches!(report.error, Some(RunError::Parse(ParseError::Regex(_)))));
}

#[test]
fn console_kinds_interpret_given_output() {
    let python = lang("python");
    let console = Arc::new(registry().get("shell").unwrap().clone());
    let def = console.diagnostics.iter().find(|d| d.id == "console-text").unwrap().clone();
    let job = DiagnosticJob::new(console.clone(), def, "")
        .console_output("x.sh:3:7: error: unexpected token\n")
        .file("/work/x.sh");
    let out = run(&job);
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].origin, Origin::Console);
    assert_eq!(out.diagnostics[0].source.as_deref(), Some("console-text"));
    assert_eq!(out.highlights().len(), 1);
    // Highlight only.
    let hl = python.diagnostics.iter().find(|d| d.id == "console-text").unwrap().clone();
    assert_eq!(hl.kind, DiagnosticKind::ConsoleHighlight);
    let out = run(&DiagnosticJob::new(python, hl, "").console_output("  File \"a.py\", line 3, in f\n"));
    assert!(out.highlight_only);
    assert_eq!(out.highlights().len(), 1);
}

#[test]
fn builtin_checkers_through_the_runner() {
    let reg = registry();
    let (defs, entries) = entries_for(reg.get("json").unwrap(), &DiagnosticsPrefs::default(), &DocumentDiagnosticsData::default());
    assert!(entries.iter().all(|e| e.enabled));
    let json = lang("json");
    let syntax = defs.iter().find(|d| d.kind == DiagnosticKind::InvalidSyntax).unwrap().clone();
    let out = run(&DiagnosticJob::new(json.clone(), syntax, "{\n  \"a\": 1,\n}\n"));
    assert_eq!(out.diagnostics.len(), 1);
    let conflict = defs.iter().find(|d| d.kind == DiagnosticKind::Conflict).unwrap().clone();
    let text = "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\n";
    let out = run(&DiagnosticJob::new(json, conflict, text));
    let (here, _) = out.annotations(text, None);
    assert_eq!(here[0].fixes.len(), 3);
}

// ----------------------------------------------------------- real toolchains

#[test]
fn clang_diagnostic_with_fixits() {
    need!("clang");
    let c = lang("c");
    let def = builtin_def("c", "clang");
    let buffer = "#include <stdio.h>\nint main(void) {\n    int x = 1;\n    if (x = 2) { return 0; }\n    printf(\"%d\\n\", x)\n    return 1;\n}\n";
    let out = run(&DiagnosticJob::new(c, def, buffer).file("/nonexistent/dir/main.c"));
    let (here, foreign) = out.annotations(buffer, Some(Path::new("/nonexistent/dir/main.c")));
    assert!(foreign.is_empty(), "{foreign:#?}");
    let errors: Vec<_> = here.iter().filter(|a| a.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 1, "{here:#?}\n{}", out.raw_output);
    assert!(errors[0].message.contains("expected ';'"));
    let mut set = AnnotationSet::new();
    set.replace_source("clang", Origin::Document, here);
    let fixed = set.all_fixes().apply_to(buffer);
    assert!(fixed.contains("printf(\"%d\\n\", x);"), "{fixed}");
    assert!(set.counts().warnings >= 1, "-Wparentheses is on by default for assignments in conditions");
    assert!(set.visible().iter().any(|a| a.message_id.as_deref() == Some("-Wparentheses")));
}

#[test]
fn clang_finds_local_headers_next_to_the_real_file() {
    need!("clang");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("util.h"), "int util(void);\n").unwrap();
    let real = dir.path().join("main.c");
    let buffer = "#include \"util.h\"\nint main(void) { return util(); }\n";
    let out = run(&DiagnosticJob::new(lang("c"), builtin_def("c", "clang"), buffer).file(&real));
    assert!(out.diagnostics.is_empty(), "{}", out.raw_output);
}

#[test]
fn python_syntax_diagnostic() {
    need!("python3");
    let def = builtin_def("python", "syntax");
    let buffer = "def f(x):\n    return x +\n\nprint(f(1))\n";
    let out = run(&DiagnosticJob::new(lang("python"), def.clone(), buffer));
    let (here, _) = out.annotations(buffer, None);
    assert_eq!(here.len(), 1, "{}", out.raw_output);
    assert_eq!(here[0].severity, Severity::Error);
    assert!(here[0].message.starts_with("SyntaxError"));
    assert_eq!(here[0].line(&LineIndex::new(buffer)), Some(1));
    let ok = run(&DiagnosticJob::new(lang("python"), def, "print('fine')\n"));
    assert!(ok.diagnostics.is_empty());
}

#[test]
fn node_check_and_bash_n_via_fallback_and_unix() {
    if have("node") {
        let def = builtin_def("javascript-node", "node-syntax");
        let buffer = "const a = 1;\nfoo bar;\n";
        let out = run(&DiagnosticJob::new(lang("javascript-node"), def, buffer));
        let (here, _) = out.annotations(buffer, None);
        assert_eq!(here.len(), 1, "{}", out.raw_output);
        assert_eq!(&buffer[here[0].range.clone().unwrap()], "bar");
        assert!(out.used_fallback, "node's format is not file:line:col");
    } else {
        eprintln!("SKIPPED: node is not installed");
    }
    let def = builtin_def("shell", "syntax");
    let buffer = "if true; then\n  echo hi\n";
    let out = run(&DiagnosticJob::new(lang("shell"), def, buffer));
    let (here, _) = out.annotations(buffer, None);
    assert_eq!(here.len(), 1, "{}", out.raw_output);
    assert!(here[0].message.contains("syntax error"));
}

#[test]
fn compile_script_diagnostics_for_compiled_languages() {
    // (language, diagnostic, file name, buffer, tool, text expected under the first error)
    let cases: [(&str, &str, &str, &str, &str, &str); 4] = [
        ("go", "compile", "main.go", "package main\n\nfunc main() {\n\tundefinedCall()\n}\n", "go", "undefinedCall"),
        ("rust", "compile", "main.rs", "fn main() {\n    let y: i32 = \"a\";\n    println!(\"{}\", y);\n}\n", "rustc", "\"a\""),
        ("swift", "compile", "main.swift", "let x: Int = \"hello\"\n", "swiftc", "\"hello\""),
        ("java", "compile", "Main.java", "public class Main {\n    public static void main(String[] a) {\n        int x = \"s\";\n    }\n}\n", "javac", "\"s\""),
    ];
    for (id, diag, file, buffer, tool, under) in cases {
        if !have(tool) {
            eprintln!("SKIPPED: {tool} is not installed ({id})");
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join(file);
        let out = match run_job(
            &DiagnosticJob::new(lang(id), builtin_def(id, diag), buffer).file(&real),
            &CancelToken::new(),
        ) {
            Ok(o) => o,
            Err(RunError::ToolMissing(t)) => {
                eprintln!("SKIPPED: {id}: {} missing", t.program);
                continue;
            }
            Err(e) => panic!("{id}: {e}"),
        };
        let (here, foreign) = out.annotations(buffer, Some(&real));
        let first_error = here
            .iter()
            .find(|a| a.severity == Severity::Error)
            .unwrap_or_else(|| panic!("{id}: no error in {:#?}\n{}", out.diagnostics, out.raw_output));
        assert_eq!(&buffer[first_error.range.clone().unwrap()], under, "{id}\n{}", out.raw_output);
        assert!(foreign.is_empty(), "{id}: {foreign:#?}");
    }
}

#[test]
fn rust_human_format_is_found_by_the_fallback() {
    need!("rustc");
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("main.rs");
    let buffer = "fn main() {\n    println!(\"{}\", nope);\n}\n";
    let out = run(&DiagnosticJob::new(lang("rust"), builtin_def("rust", "compile"), buffer).file(&real));
    assert!(out.used_fallback, "{}", out.raw_output);
    let (here, _) = out.annotations(buffer, Some(&real));
    assert_eq!(here.len(), 1);
    assert_eq!(here[0].message_id.as_deref(), Some("E0425"));
    assert_eq!(&buffer[here[0].range.clone().unwrap()], "nope");
}

// -------------------------------------------------- scheduler + runner glue

#[test]
fn scheduler_drives_the_runner_end_to_end() {
    let language = lang("plain-text");
    let def = command_def("printf '%s:1:1: error: bad [E1]\\n' $filename", OutputMode::Unix, None);
    let mut def_cancel = def.clone();
    def_cancel.options = DiagnosticOptions { cancel_on_errors: true, ..DiagnosticOptions::default() };
    def_cancel.update = UpdateMode { on_edit: false, on_run: true, manual: false };
    def_cancel.id = "on-run".into();
    let defs = [def.clone(), def_cancel.clone()];
    let entries: Vec<Entry> = defs.iter().map(|d| Entry::from_def(d, d.update, true)).collect();
    let mut scheduler = Scheduler::new(SchedulerConfig {
        delay: Duration::from_millis(60),
        ..SchedulerConfig::default()
    });
    scheduler.set_entries(entries);
    let woken = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&woken);
    let runner = DiagnosticsRunner::new().with_notifier(move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    let mut set = AnnotationSet::new();
    let buffer = "hello\n";

    let start_all = |runner: &DiagnosticsRunner, plan: Plan, scheduler: &mut Scheduler| {
        for t in plan.cancel {
            runner.cancel(t);
        }
        for req in plan.start {
            let d = defs.iter().find(|d| d.id == req.entry_id).unwrap().clone();
            let mut job = DiagnosticJob::new(language.clone(), d, buffer);
            job.generation = req.generation;
            runner.spawn(req.token, job);
        }
        let _ = scheduler;
    };
    let apply = |runner: &DiagnosticsRunner, scheduler: &mut Scheduler, set: &mut AnnotationSet| -> usize {
        let mut applied = 0;
        while let Some(result) = runner.recv_timeout(Duration::from_secs(10)) {
            let mut new = Counts::default();
            let ok = result.outcome.is_ok();
            let accept = scheduler.job_finished(result.token, JobOutcome::default()).accept;
            if let (true, Ok(out)) = (accept, &result.outcome) {
                let (here, _) = out.annotations(buffer, None);
                new = set.replace_source(&out.source, Origin::Document, here).new;
                applied += 1;
            }
            let _ = (ok, new);
            if !scheduler.is_busy() {
                break;
            }
        }
        applied
    };

    // Typing: nothing runs until the debounce delay passed.
    let t0 = Instant::now();
    scheduler.text_changed(t0);
    assert!(scheduler.poll(t0).is_empty());
    let plan = scheduler.poll(t0 + Duration::from_millis(100));
    assert_eq!(plan.start.len(), 1);
    start_all(&runner, plan, &mut scheduler);
    assert_eq!(apply(&runner, &mut scheduler, &mut set), 1);
    assert_eq!(set.counts().errors, 1);
    assert!(woken.load(Ordering::SeqCst) >= 1, "the notifier woke the UI");

    // Run: the on-run entry produces an error that is *new for its source*, so the run is cancelled.
    let plan = scheduler.before_run(Instant::now());
    assert_eq!(plan.start.len(), 1);
    let token = plan.start[0].token;
    start_all(&runner, plan, &mut scheduler);
    let result = runner.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(result.token, token);
    let out = result.outcome.unwrap();
    let (here, _) = out.annotations(buffer, None);
    let report = set.replace_source(&out.source, Origin::Document, here);
    assert_eq!(report.new.errors, 1);
    scheduler.job_finished(token, JobOutcome { new: report.new, ok: true });
    match scheduler.run_gate(Instant::now()).state {
        RunGate::Cancel(reason) => assert_eq!(reason.severity, Severity::Error),
        other => panic!("{other:?}"),
    }
    // The same text again: the error is dedupe-collapsed (same id from both sources).
    assert_eq!(set.counts().errors, 1);
}

#[test]
fn many_parallel_jobs_do_not_interfere() {
    let runner = DiagnosticsRunner::new();
    let def = command_def("printf '%s:1:1: warning: w\\n' $filename; cat $filename >&2", OutputMode::Unix, None);
    let language = lang("plain-text");
    for i in 0..12 {
        let job = DiagnosticJob::new(language.clone(), def.clone(), format!("buffer {i}\n"));
        runner.spawn(JobToken(i), job);
    }
    let mut seen = Vec::new();
    while seen.len() < 12 {
        let r = runner.recv_timeout(Duration::from_secs(20)).expect("result");
        let out = r.outcome.unwrap();
        assert!(out.raw_output.contains(&format!("buffer {}", r.token.0)), "{}", out.raw_output);
        seen.push(r.token.0);
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..12).collect::<Vec<_>>());
}
