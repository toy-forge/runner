//! Compile stages, the compile-script protocol, compile-only and debug mode, and
//! preflight, with real toolchains (each test skips visibly when its toolchain
//! is missing).

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use scratch_language::{CompileSpec, LanguageDef, ShellSpec};
use scratch_runner::{
    DeveloperDir, Phase, RunError, RunEvent, RunMode, RunRequest, RunSettings, RunStatus, Snapshot,
};

fn stage_events(outcome: &Outcome) -> Vec<String> {
    outcome
        .events()
        .filter_map(|e| match e {
            RunEvent::StageStarted(info) => Some(format!("start:{}", info.name)),
            RunEvent::StageFinished(done) => Some(format!("end:{}:{}", done.name, done.success)),
            _ => None,
        })
        .collect()
}

#[test]
fn c_is_compiled_then_run_and_the_build_stays_out_of_the_source_directory() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("hello.c");
    std::fs::write(
        &file,
        "#include <stdio.h>\n#include <unistd.h>\nint main(int argc, char **argv) {\n  printf(\"hello %d %d\\n\", argc, isatty(1));\n  fprintf(stderr, \"to stderr\\n\");\n  return 0;\n}\n",
    )
    .unwrap();
    let settings = RunSettings {
        arguments: "a b".into(),
        ..RunSettings::default()
    };
    let outcome = run(request("c", Snapshot::saved(&file), root.path()).with_settings(settings));
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{:?} {}",
        outcome.summary,
        outcome.compile_stderr()
    );
    assert!(
        outcome.stdout().contains("hello 3 1"),
        "{:?}",
        outcome.stdout()
    );
    assert_eq!(outcome.stderr().trim(), "to stderr");
    assert_eq!(
        stage_events(&outcome),
        ["start:Compile", "end:Compile:true"]
    );
    // StageStarted comes before the compile output and Started after it.
    let kinds: Vec<&str> = outcome
        .events()
        .map(|e| match e {
            RunEvent::Preparing => "prep",
            RunEvent::StageStarted(_) => "stage+",
            RunEvent::StageFinished(_) => "stage-",
            RunEvent::Started(_) => "started",
            RunEvent::Finished(_) => "done",
            RunEvent::Output { .. } => "out",
            _ => "other",
        })
        .collect();
    assert_eq!(&kinds[..3], ["prep", "stage+", "stage-"], "{kinds:?}");
    assert!(kinds.contains(&"started"));
    // The executable went to the temporary directory, not next to the source.
    let names: Vec<String> = std::fs::read_dir(project.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["hello.c"], "{names:?}");
}

#[test]
fn compiler_output_can_go_next_to_the_source_when_asked() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("beside.c");
    std::fs::write(&file, "int main(void){return 0;}\n").unwrap();
    let mut shell = shell();
    shell.compiler_output_in_temp_dir = false;
    let outcome = run(request("c", Snapshot::saved(&file), root.path()).with_shell(shell));
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{}",
        outcome.compile_stderr()
    );
    assert!(project.path().join("beside").exists());
}

#[test]
fn a_compile_error_stops_before_the_program_and_shows_the_compiler_output() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "c",
        Snapshot::named_text("int main(void) { return x; }\n", "broken"),
        root.path(),
    ));
    match &outcome.summary.status {
        RunStatus::CompileFailed {
            stage,
            exit_code,
            signal,
        } => {
            assert_eq!(stage, "Compile");
            assert_eq!(*exit_code, Some(1));
            assert_eq!(*signal, None);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(outcome.summary.phase, Phase::Compile);
    assert_eq!(outcome.summary.status_text(), "Compile Error");
    let errors = outcome.compile_stderr();
    assert!(
        errors.contains("error:") && errors.contains("broken.c:1"),
        "{errors}"
    );
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));
    assert_eq!(
        stage_events(&outcome),
        ["start:Compile", "end:Compile:false"]
    );
    assert_eq!(outcome.stdout(), "");
}

#[test]
fn compile_only_reports_the_program_without_starting_it() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("ran");
    let source = format!(
        "#include <stdio.h>\nint main(void){{ FILE *f = fopen(\"{}\", \"w\"); fclose(f); return 0; }}\n",
        marker.display()
    );
    let req = request("c", Snapshot::text(source), root.path()).with_mode(RunMode::CompileOnly);
    let outcome = run(req);
    assert_eq!(
        outcome.summary.status,
        RunStatus::Compiled,
        "{}",
        outcome.compile_stderr()
    );
    assert_eq!(outcome.summary.status_text(), "Compile Succeeded");
    assert!(outcome.summary.status.is_success());
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));
    assert!(!marker.exists(), "the program must not have run");
    let info = outcome
        .events()
        .find_map(|e| match e {
            RunEvent::Compiled(info) => Some(info.clone()),
            _ => None,
        })
        .expect("Compiled event");
    assert_eq!(info.pid, None);
    assert!(
        info.compiler
            .as_deref()
            .is_some_and(|c| c.ends_with("/main")),
        "{:?}",
        info.compiler
    );
    assert_eq!(info.argv[0], info.compiler.clone().unwrap());
    assert_eq!(info.env["CR_FILENAME"], "main.c");
    assert!(info.file.ends_with("main.c"));
}

#[test]
fn compile_only_of_an_interpreted_language_is_a_no_op_success() {
    let root = tempfile::tempdir().unwrap();
    let req =
        request("python", Snapshot::text("print(1)"), root.path()).with_mode(RunMode::CompileOnly);
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::Compiled);
    assert!(stage_events(&outcome).is_empty());
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));
}

#[test]
fn debug_mode_compiles_with_debug_info_and_hands_over_the_program() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let req = request(
        "c",
        Snapshot::text("#include <stdio.h>\nint main(void){puts(\"x\");return 0;}\n"),
        root.path(),
    )
    .with_mode(RunMode::Debug);
    let outcome = run(req);
    assert_eq!(
        outcome.summary.status,
        RunStatus::Compiled,
        "{}",
        outcome.compile_stderr()
    );
    let info = outcome
        .events()
        .find_map(|e| match e {
            RunEvent::Compiled(info) => Some(info.clone()),
            _ => None,
        })
        .expect("Compiled event");
    assert_eq!(info.env["CR_DEBUGGING"], "1");
    let started = outcome.events().find_map(|e| match e {
        RunEvent::StageStarted(i) => Some(i.command.clone()),
        _ => None,
    });
    assert!(started.unwrap().contains(" -g"), "debug flag missing");
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));
    // The executable is gone with the run directory, but the argv, cwd and env
    // are everything a debugger needs.
    assert!(info.cwd.ends_with("src"));
}

#[test]
fn rust_go_node_and_friends() {
    let root = tempfile::tempdir().unwrap();
    if !skip_without(&["rustc"]) {
        let outcome = run(request(
            "rust",
            Snapshot::named_text(
                "fn main() { println!(\"hi from rust\"); eprintln!(\"rust err\"); }\n",
                "My Prog",
            ),
            root.path(),
        ));
        assert_eq!(
            outcome.summary.status,
            RunStatus::Exit(0),
            "{}",
            outcome.compile_stderr()
        );
        assert!(outcome.stdout().contains("hi from rust"));
        assert_eq!(outcome.stderr().trim(), "rust err");
        let broken = run(request(
            "rust",
            Snapshot::text("fn main() { let x: i32 = \"s\"; }\n"),
            root.path(),
        ));
        assert!(matches!(
            broken.summary.status,
            RunStatus::CompileFailed { .. }
        ));
        assert!(
            broken.compile_stderr().contains("mismatched types"),
            "{}",
            broken.compile_stderr()
        );
    }
    if !skip_without(&["go"]) {
        let outcome = run(request(
            "go",
            Snapshot::text(
                "package main\nimport \"fmt\"\nfunc main() { fmt.Println(\"hi from go\") }\n",
            ),
            root.path(),
        ));
        assert_eq!(
            outcome.summary.status,
            RunStatus::Exit(0),
            "{}",
            outcome.compile_stderr()
        );
        assert!(outcome.stdout().contains("hi from go"));
    }
    if !skip_without(&["node"]) {
        let outcome = run(request(
            "javascript-node",
            Snapshot::text("console.log('hi from node', process.stdout.isTTY)\n"),
            root.path(),
        ));
        assert_eq!(outcome.summary.status, RunStatus::Exit(0));
        assert!(
            outcome.stdout().contains("hi from node true"),
            "{:?}",
            outcome.stdout()
        );
    }
}

#[test]
fn java_unsaved_is_named_after_its_class_and_run_with_the_classpath() {
    if skip_without(&["javac", "java"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "public class Hello {\n  public static void main(String[] args) { System.out.println(\"hi from java \" + args.length); }\n}\n";
    let settings = RunSettings {
        arguments: "x y".into(),
        ..RunSettings::default()
    };
    let outcome = run(request(
        "java",
        Snapshot::named_text(source, "Untitled"),
        root.path(),
    )
    .with_settings(settings));
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{} {:?}",
        outcome.compile_stderr(),
        outcome.summary
    );
    assert!(
        outcome.stdout().contains("hi from java 2"),
        "{:?} {:?}",
        outcome.stdout(),
        outcome.stderr()
    );
}

#[test]
fn swift_unsaved_runs_as_main_swift() {
    if skip_without(&["swiftc"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "swift",
        Snapshot::text("print(\"hi from swift\")\n"),
        root.path(),
    ));
    if matches!(
        outcome.summary.status,
        RunStatus::Error(RunError::DeveloperToolsMissing { .. })
    ) {
        eprintln!("SKIP: no developer tools");
        return;
    }
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{}",
        outcome.compile_stderr()
    );
    assert!(outcome.stdout().contains("hi from swift"));
}

#[test]
fn compile_cancel_and_compile_timeout() {
    let root = tempfile::tempdir().unwrap();
    // A compile script that hangs: cancel it during the Compile stage.
    let language = script_language(
        "#!/bin/bash\necho compiling 1>&2\nsleep 300 &\nwait\n",
        "echo never",
    );
    let (handle, rx) =
        scratch_runner::start_channel(script_request(&language, root.path())).unwrap();
    let mut seen = Vec::new();
    loop {
        let message = rx.recv_timeout(Duration::from_secs(30)).expect("event");
        let is_stage = matches!(message.event, RunEvent::StageStarted(_));
        seen.push(message);
        if is_stage {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    handle.cancel();
    seen.extend(wait_rest(&rx));
    let Some(RunEvent::Finished(summary)) = seen.last().map(|m| m.event.clone()) else {
        panic!()
    };
    assert_eq!(summary.status, RunStatus::Cancelled);
    assert_eq!(summary.phase, Phase::Compile);
    assert_eq!(summary.status_text(), "Compile Stopped");
    assert!(started.elapsed() < Duration::from_secs(8));

    // Compile timeout.
    let mut req = script_request(&language, root.path());
    req.options.limits.compile_timeout = Some(Duration::from_millis(500));
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::TimedOut);
    assert_eq!(outcome.summary.phase, Phase::Compile);
}

fn script_language(script: &str, command: &str) -> LanguageDef {
    let mut language = LanguageDef::new("mine", "Mine");
    language.extensions = vec!["mine".into()];
    language.run.command = command.to_owned();
    language.run.flags = "-DDEFAULT".to_owned();
    language.run.compile = CompileSpec::Shell(ShellSpec {
        script: script.to_owned(),
    });
    language
}

fn script_request(language: &LanguageDef, root: &Path) -> RunRequest {
    RunRequest::new(language, Snapshot::text("source text\n"))
        .with_shell(shell())
        .with_options(options(root))
}

#[test]
fn the_compile_script_protocol() {
    let root = tempfile::tempdir().unwrap();
    // cwd = source dir, $1..$n = flags, CR_* env, last stdout line = $compiler.
    let script = r#"#!/bin/bash
echo "cwd=$PWD" 1>&2
echo "args=$# [$1] [$2]" 1>&2
echo "file=$CR_FILENAME" 1>&2
echo "noise on stdout"
out="$CR_SUGGESTED_OUTPUT_FILE.artifact"
{ echo "built from $(cat "$CR_FILENAME")"; echo "flags $*"; } > "$out"
echo "$out"
"#;
    let language = script_language(script, "cat $compiler");
    let mut req = script_request(&language, root.path());
    req.settings.compile_flags = Some(r#"-x "two words""#.into());
    let outcome = run(req);
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{}",
        outcome.compile_stderr()
    );
    let log = outcome.compile_stderr();
    assert!(log.contains("cwd="), "{log}");
    assert!(log.contains("/src"), "{log}");
    assert!(log.contains("args=2 [-x] [two words]"), "{log}");
    assert!(log.contains("file=main.mine"), "{log}");
    let out = outcome.stdout();
    assert!(out.contains("built from source text"), "{out:?}");
    assert!(out.contains("flags -x two words"), "{out:?}");
    // Stdout of a successful script is not shown (only the last line is used).
    assert!(!out.contains("noise on stdout"));
    assert_eq!(
        stage_events(&outcome),
        ["start:Compile", "end:Compile:true"]
    );

    // The language's default flags apply without an override.
    let script = "#!/bin/bash\necho \"[$*]\" 1>&2\necho /bin/echo\n";
    let language = script_language(script, "$compiler ran");
    let outcome = run(script_request(&language, root.path()));
    assert!(
        outcome.compile_stderr().contains("[-DDEFAULT]"),
        "{}",
        outcome.compile_stderr()
    );
    assert!(outcome.stdout().contains("ran"), "{:?}", outcome.stdout());
}

#[test]
fn a_failing_compile_script_shows_stdout_and_stderr_as_errors() {
    let root = tempfile::tempdir().unwrap();
    let language = script_language(
        "#!/bin/bash\necho 'out text'\necho 'err text' 1>&2\nexit 3\n",
        "echo not reached",
    );
    let outcome = run(script_request(&language, root.path()));
    assert!(matches!(
        outcome.summary.status,
        RunStatus::CompileFailed {
            exit_code: Some(3),
            ..
        }
    ));
    let errors = outcome.compile_stderr();
    assert!(
        errors.contains("err text") && errors.contains("out text"),
        "{errors}"
    );
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));
}

#[test]
fn compile_script_status_127_and_69_have_their_messages() {
    let root = tempfile::tempdir().unwrap();
    let mut language = script_language("#!/bin/bash\nexit 127\n", "echo x");
    language.installer = Some(scratch_language::InstallerHint {
        executable: "mine".into(),
        name: "the Mine compiler".into(),
        url: "https://example.com/mine".into(),
    });
    let outcome = run(script_request(&language, root.path()));
    let message = outcome.summary.message.expect("installer hint");
    assert!(
        message.contains("the Mine compiler") && message.contains("https://example.com/mine"),
        "{message}"
    );
    language
        .errors
        .insert("127".into(), "custom 127 text".into());
    let outcome = run(script_request(&language, root.path()));
    assert_eq!(outcome.summary.message.as_deref(), Some("custom 127 text"));

    let language = script_language("#!/bin/bash\nexit 69\n", "echo x");
    let outcome = run(script_request(&language, root.path()));
    assert!(
        outcome
            .summary
            .message
            .expect("license text")
            .contains("License")
    );
}

#[test]
fn a_compile_script_without_output_leaves_compiler_empty() {
    let root = tempfile::tempdir().unwrap();
    let language = script_language("#!/bin/bash\nexit 0\n", "echo compiler=[$compiler]");
    let outcome = run(script_request(&language, root.path()));
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    assert!(
        outcome.stdout().contains("compiler=[]"),
        "{:?}",
        outcome.stdout()
    );
}

#[test]
fn xcode_shims_are_not_started_without_developer_tools() {
    if !Path::new("/usr/bin/clang").exists() {
        eprintln!("SKIP: no /usr/bin/clang");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "c",
        Snapshot::text("int main(void){return 0;}\n"),
        root.path(),
    );
    req.options.developer_dir = DeveloperDir::Assume(None);
    // Only the system tools on PATH, so that no other clang is found.
    let mut shell = shell();
    shell.path = scratch_runner::PathSetting::Custom("/usr/bin:/bin".into());
    req.shell = shell;
    let outcome = run(req);
    match &outcome.summary.status {
        RunStatus::Error(RunError::DeveloperToolsMissing { program, message }) => {
            assert!(
                program.contains("xcrun") || program.contains("clang"),
                "{program}"
            );
            assert!(message.contains("xcode-select --install"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !outcome.has(|e| matches!(e, RunEvent::StageStarted(_))),
        "nothing may start"
    );
    assert_eq!(outcome.summary.status_text(), "Run Failed");

    // An interpreter shim: python3 with no developer tools and no other Python.
    if Path::new("/usr/bin/python3").exists()
        && scratch_runner::preflight::is_xcode_shim(Path::new("/usr/bin/python3"))
    {
        let mut req = request("python", Snapshot::text("print(1)\n"), root.path());
        req.options.developer_dir = DeveloperDir::Assume(None);
        req.shell.path = scratch_runner::PathSetting::Custom("/usr/bin:/bin".into());
        let outcome = run(req);
        match &outcome.summary.status {
            // A real Python in the usual install locations is found instead.
            RunStatus::Exit(0) => {
                assert!(outcome.has(|e| matches!(e, RunEvent::Started(info) if !info.argv[0].starts_with("/usr/bin/"))));
            }
            RunStatus::Error(RunError::PythonMissing { message }) => {
                assert!(message.contains("python.org"), "{message}");
                assert!(message.contains("xcode-select --install"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !outcome.has(
                |e| matches!(e, RunEvent::Started(info) if info.argv[0] == "/usr/bin/python3")
            )
        );
    }
}

#[test]
fn a_missing_compiler_is_a_typed_error_with_the_steps_own_hint() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "go",
        Snapshot::text("package main\nfunc main(){}\n"),
        root.path(),
    );
    req.shell.path = scratch_runner::PathSetting::Custom("/nonexistent-dir".into());
    let outcome = run(req);
    match &outcome.summary.status {
        RunStatus::Error(RunError::MissingTool { program, message }) => {
            assert!(program.contains("go"), "{program}");
            assert!(!message.is_empty());
        }
        // go may be found through the language's own path_append (/opt/homebrew/bin).
        RunStatus::Exit(0) => eprintln!("go found through the language's PATH additions"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_build_directory_is_removed_with_the_run() {
    if skip_without(&["clang"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "c",
        Snapshot::text("int main(void){return 0;}\n"),
        root.path(),
    ));
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    let left: Vec<_> = std::fs::read_dir(root.path().join("ScratchRunner"))
        .map(|d| d.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn keep_temp_leaves_the_run_directory_for_inspection() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request("shell", Snapshot::text("echo kept\n"), root.path());
    req.options.keep_temp = true;
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    let kept: Vec<_> = std::fs::read_dir(root.path().join("ScratchRunner"))
        .unwrap()
        .collect();
    assert_eq!(kept.len(), 1);
    // And the stale sweep can remove it.
    assert_eq!(
        scratch_runner::sweep_stale_runs(Some(root.path()), Duration::ZERO),
        1
    );
}
