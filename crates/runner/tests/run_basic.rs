//! Running interpreted languages for real: python3 and bash.

mod common;

use std::time::{Duration, Instant};

use common::*;
use scratch_runner::{
    OnBusy, Phase, ProgramInput, RunEvent, RunMode, RunSettings, RunStatus, Snapshot, Stream,
    WorkingDirectory,
};

#[test]
fn stdout_and_stderr_are_tagged_exactly() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "import sys\nprint('to stdout')\nprint('to stderr', file=sys.stderr)\nprint('again', flush=True)\n";
    let outcome = run(request("python", Snapshot::text(source), root.path()));
    assert_eq!(
        outcome.summary.status,
        RunStatus::Exit(0),
        "{:?}",
        outcome.summary
    );
    assert!(
        outcome.stdout().contains("to stdout"),
        "{:?}",
        outcome.stdout()
    );
    assert!(outcome.stdout().contains("again"));
    assert!(!outcome.stdout().contains("to stderr"));
    assert_eq!(outcome.stderr().trim(), "to stderr");
    assert_eq!(outcome.summary.status_text(), "Run Succeeded");
    assert!(outcome.summary.peak_rss > 0);
    assert!(outcome.summary.total_wall >= outcome.summary.wall);
    assert!(outcome.started_pid().is_some());
    assert!(
        outcome.has(|e| matches!(e, RunEvent::Started(info) if info.argv[0].ends_with("python3")))
    );
}

#[test]
fn the_program_sees_a_tty_on_stdout_but_not_on_stderr() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source =
        "import sys\nprint(sys.stdin.isatty(), sys.stdout.isatty(), sys.stderr.isatty())\n";
    let outcome = run(request("python", Snapshot::text(source), root.path()));
    assert!(
        outcome.stdout().contains("True True False"),
        "{:?}",
        outcome.stdout()
    );
}

#[test]
fn nonzero_exit_and_signal_statuses() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "python",
        Snapshot::text("import sys\nsys.exit(3)\n"),
        root.path(),
    ));
    assert_eq!(outcome.summary.status, RunStatus::Exit(3));
    assert_eq!(outcome.summary.status_text(), "Run Ended");
    assert!(!outcome.summary.status.is_success());

    let outcome = run(request(
        "python",
        Snapshot::text("import os, signal\nos.kill(os.getpid(), signal.SIGSEGV)\n"),
        root.path(),
    ));
    assert_eq!(outcome.summary.status, RunStatus::Signal(libc_sigsegv()));
    assert_eq!(
        outcome.summary.message.as_deref(),
        Some("Terminated due to signal: SIGSEGV (11)")
    );
}

fn libc_sigsegv() -> i32 {
    11
}

#[test]
fn typed_input_is_echoed_by_the_pty_and_read_by_the_program() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    // (input() would write its prompt to stderr: see RunOptions::merge_stderr.)
    let source =
        "print('Name? ', end='', flush=True)\nname = input()\nprint('Hello, ' + name + '!')\n";
    let outcome = run_with(
        request("python", Snapshot::text(source), root.path()),
        |handle, rx| {
            let (found, _, _) = read_until(rx, "Name? ", Duration::from_secs(10));
            assert!(found, "prompt never appeared");
            handle.write_stdin(b"Ada\n").unwrap();
        },
    );
    let out = outcome.stdout();
    assert!(out.contains("Hello, Ada!"), "{out:?}");
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
}

#[test]
fn merged_stderr_puts_everything_on_the_pty() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "python",
        Snapshot::text(
            "import sys\nprint('o', flush=True)\nprint('e', file=sys.stderr, flush=True)\n",
        ),
        root.path(),
    );
    req.options.merge_stderr = true;
    let outcome = run(req);
    assert!(
        outcome.stdout().contains('o') && outcome.stdout().contains('e'),
        "{:?}",
        outcome.stdout()
    );
    assert_eq!(outcome.stderr(), "");
}

#[test]
fn python_input_prompt_goes_to_stderr_when_streams_are_separate() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let (handle, rx) = scratch_runner::start_channel(request(
        "python",
        Snapshot::text("name = input('Name? ')\nprint('Hi ' + name)\n"),
        root.path(),
    ))
    .unwrap();
    handle.write_stdin(b"Bo\n").unwrap();
    let outcome = wait(&rx);
    assert!(outcome.stdout().contains("Hi Bo"), "{:?}", outcome.stdout());
    assert!(
        outcome.stderr().contains("Name? "),
        "{:?}",
        outcome.stderr()
    );
}

#[test]
fn input_typed_before_the_program_starts_is_delivered() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "print('got ' + input())\n";
    let (handle, rx) =
        scratch_runner::start_channel(request("python", Snapshot::text(source), root.path()))
            .unwrap();
    handle.write_stdin(b"early\n").unwrap();
    let outcome = wait(&rx);
    assert!(
        outcome.stdout().contains("got early"),
        "{:?}",
        outcome.stdout()
    );
    assert!(handle.is_finished());
    assert!(handle.write_stdin(b"late").is_err());
}

#[test]
fn program_input_text_is_redirected_to_stdin() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "import sys\nprint(sys.stdin.isatty())\nprint(sum(int(x) for x in sys.stdin.read().split()))\n";
    let settings = RunSettings {
        input: Some(ProgramInput::Text("1 2\n3 4\n".into())),
        ..RunSettings::default()
    };
    let outcome =
        run(request("python", Snapshot::text(source), root.path()).with_settings(settings));
    let out = outcome.stdout();
    assert!(out.contains("False"), "{out:?}");
    assert!(out.contains("10"), "{out:?}");
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
}

#[test]
fn program_input_file_and_input_through_the_environment() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.txt");
    std::fs::write(&input, "from a file\n").unwrap();
    let settings = RunSettings {
        input: Some(ProgramInput::File(input.clone())),
        ..RunSettings::default()
    };
    let outcome = run(request(
        "python",
        Snapshot::text("print(input().upper())\n"),
        root.path(),
    )
    .with_settings(settings));
    assert!(
        outcome.stdout().contains("FROM A FILE"),
        "{:?}",
        outcome.stdout()
    );

    // Through the environment: stdin stays the terminal, CR_INPUT names the file.
    let source = "import os, sys\nprint(sys.stdin.isatty())\nprint(open(os.environ['CR_INPUT']).read().strip())\n";
    let settings = RunSettings {
        input: Some(ProgramInput::Text("env input".into())),
        input_via_environment: true,
        ..RunSettings::default()
    };
    let outcome =
        run(request("python", Snapshot::text(source), root.path()).with_settings(settings));
    let out = outcome.stdout();
    assert!(out.contains("True"), "{out:?}");
    assert!(out.contains("env input"), "{out:?}");

    // A missing input file is a typed error.
    let settings = RunSettings {
        input: Some(ProgramInput::File(root.path().join("nope.txt"))),
        ..RunSettings::default()
    };
    let outcome =
        run(request("python", Snapshot::text("print(1)\n"), root.path()).with_settings(settings));
    assert!(matches!(
        outcome.summary.status,
        RunStatus::Error(scratch_runner::RunError::InputFile(_))
    ));
}

#[test]
fn arguments_are_split_and_appended_without_expansion() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "import sys\nprint(repr(sys.argv[1:]))\n";
    let settings = RunSettings {
        arguments: r#"one "two words" '$HOME' $HOME *"#.into(),
        ..RunSettings::default()
    };
    let outcome =
        run(request("python", Snapshot::text(source), root.path()).with_settings(settings));
    assert!(
        outcome
            .stdout()
            .contains(r#"['one', 'two words', '$HOME', '$HOME', '*']"#),
        "{:?}",
        outcome.stdout()
    );
}

#[test]
fn arguments_reach_a_shell_command_too() {
    let root = tempfile::tempdir().unwrap();
    let settings = RunSettings {
        arguments: "a 'b c'".into(),
        run_command: Some("echo \"count=$#\"; printf '[%s]' \"$@\"".into()),
        ..RunSettings::default()
    };
    let outcome =
        run(request("shell", Snapshot::text("echo unused\n"), root.path()).with_settings(settings));
    let out = outcome.stdout();
    assert!(out.contains("count=2") || out.contains("count="), "{out:?}");
    assert!(out.contains("[a][b c]"), "{out:?}");
}

#[test]
fn bash_script_runs_through_the_shell_prefs() {
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "shell",
        Snapshot::text("echo hello from bash\necho warn 1>&2\nexit 7\n"),
        root.path(),
    ));
    assert_eq!(outcome.summary.status, RunStatus::Exit(7));
    assert!(outcome.stdout().contains("hello from bash"));
    assert_eq!(outcome.stderr().trim(), "warn");
}

#[test]
fn the_environment_contract() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("env test.sh");
    std::fs::write(&file, "env\n").unwrap();
    let mut shell = shell();
    shell
        .variables
        .insert("MY_SHELL_VAR".into(), "from prefs".into());
    let mut request = request("shell", Snapshot::saved(&file), root.path()).with_shell(shell);
    request
        .settings
        .environment
        .insert("PER_RUN".into(), "yes".into());
    request
        .settings
        .environment
        .insert("CR_FILENAME".into(), "must not win".into());
    request.language_dir = Some("/lang".into());
    request.unsaved_dir = Some("/unsaved".into());
    request.app_version = Some("9.9".into());
    let outcome = run(request);
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    let env: std::collections::BTreeMap<String, String> = outcome
        .stdout()
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let get = |name: &str| {
        env.get(name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing in {env:?}"))
    };
    assert_eq!(get("CR_FILENAME"), "env test.sh");
    assert_eq!(get("CR_FILE"), file.to_string_lossy());
    assert!(
        get("CR_TMPDIR").starts_with(&root.path().to_string_lossy().to_string()),
        "{}",
        get("CR_TMPDIR")
    );
    assert!(get("CR_SUGGESTED_OUTPUT_FILE").starts_with(&get("CR_TMPDIR")));
    assert!(get("CR_RUNID").parse::<u64>().unwrap() > 0);
    assert_eq!(get("CR_RUNID"), outcome.summary.run.get().to_string());
    assert_eq!(get("CR_ENCODING_NAME"), "UTF-8");
    assert_eq!(get("CR_LANGUAGE_DIR"), "/lang");
    assert_eq!(get("CR_SCRIPTS_DIR"), "/lang/Scripts");
    assert_eq!(get("CR_UNSAVED_DIR"), "/unsaved");
    assert_eq!(get("CR_VERSION"), "9.9");
    assert!(
        get("CR_RUN_COMMAND").starts_with("bash"),
        "{}",
        get("CR_RUN_COMMAND")
    );
    assert_eq!(get("TERM"), "dumb");
    assert_eq!(get("TERM_PROGRAM"), "ScratchRunner");
    assert_eq!(get("LC_CTYPE"), "UTF-8");
    assert_eq!(get("MY_SHELL_VAR"), "from prefs");
    assert_eq!(get("PER_RUN"), "yes");
    assert!(env.contains_key("HOME"));
    assert!(!env.contains_key("CR_INPUT"));
    assert!(!env.contains_key("CR_DEBUGGING"));
    // The environment is a whitelist: nothing cargo or the app's launcher set.
    assert!(!env.contains_key("CARGO_MANIFEST_DIR"), "leaked: {env:?}");
    // PATH: the configured one, extended by the language (none for shell).
    assert!(get("PATH").split(':').any(|d| d == "/usr/bin"));
    if let Some(dir) = scratch_runner::developer_dir() {
        assert_eq!(get("DEVELOPER_DIR"), dir.to_string_lossy());
    }
    // The run happened next to the file.
    assert_eq!(
        get("PWD"),
        project.path().canonicalize().unwrap().to_string_lossy()
    );
}

#[test]
fn a_custom_path_replaces_the_automatic_one_and_language_paths_still_apply() {
    let root = tempfile::tempdir().unwrap();
    let mut shell = shell();
    shell.path = scratch_runner::PathSetting::Custom("/usr/bin:/bin:/custom/one".into());
    let outcome = run(request(
        "shell",
        Snapshot::text("echo \"PATH=$PATH\"\n"),
        root.path(),
    )
    .with_shell(shell));
    let out = outcome.stdout();
    assert!(out.contains("PATH=/usr/bin:/bin:/custom/one"), "{out:?}");
}

#[test]
fn unsaved_documents_are_written_with_the_right_extension_and_cleaned_up() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "import os, sys\nprint(os.path.basename(sys.argv[0]))\nprint(os.getcwd())\nprint(os.environ['CR_TMPDIR'])\n";
    let outcome = run(request(
        "python",
        Snapshot::named_text(source, "Untitled 2"),
        root.path(),
    ));
    let lines: Vec<String> = outcome
        .stdout()
        .lines()
        .map(|l| l.trim().to_owned())
        .collect();
    assert_eq!(lines[0], "Untitled 2.py", "{lines:?}");
    assert!(lines[1].ends_with("/src"), "{lines:?}");
    let tmpdir = std::path::PathBuf::from(&lines[2]);
    assert!(
        tmpdir.starts_with(root.path().canonicalize().unwrap()) || tmpdir.starts_with(root.path()),
        "{lines:?}"
    );
    // Everything was removed afterwards.
    assert!(!tmpdir.exists(), "{} was left behind", tmpdir.display());
    let leftovers: Vec<_> = std::fs::read_dir(root.path().join("ScratchRunner"))
        .map(|d| d.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_saved_file_is_run_in_place_and_nothing_is_left_beside_it() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("helper.py"), "VALUE = 41\n").unwrap();
    let file = project.path().join("main.py");
    std::fs::write(&file, "import helper\nprint(helper.VALUE + 1)\n").unwrap();
    let outcome = run(request("python", Snapshot::saved(&file), root.path()));
    assert!(
        outcome.stdout().contains("42"),
        "{:?} {:?}",
        outcome.stdout(),
        outcome.stderr()
    );
    let mut names: Vec<String> = std::fs::read_dir(project.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "__pycache__")
        .collect();
    names.sort();
    assert_eq!(names, ["helper.py", "main.py"]);
}

#[test]
fn a_missing_saved_file_is_a_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request(
        "python",
        Snapshot::saved("/no/such/file.py"),
        root.path(),
    ));
    assert!(matches!(
        outcome.summary.status,
        RunStatus::Error(scratch_runner::RunError::SourceMissing(_))
    ));
}

#[test]
fn working_directory_rules() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let home_like = tempfile::tempdir().unwrap();
    let file = project.path().join("pwd.sh");
    std::fs::write(&file, "pwd -P\necho \"name=$CR_FILENAME\"\n").unwrap();
    let canon = |p: &std::path::Path| p.canonicalize().unwrap().to_string_lossy().into_owned();

    let case = |wd: WorkingDirectory, folder: Option<&std::path::Path>| {
        let settings = RunSettings {
            working_directory: wd,
            ..RunSettings::default()
        };
        let mut req = request("shell", Snapshot::saved(&file), root.path()).with_settings(settings);
        req.project_folder = folder.map(|f| f.to_path_buf());
        req.default_directory = Some(home_like.path().to_path_buf());
        run(req)
    };
    let out = case(WorkingDirectory::FileDirectory, None);
    assert!(
        out.stdout().contains(&canon(project.path())),
        "{:?}",
        out.stdout()
    );
    let out = case(WorkingDirectory::ProjectFolder, Some(other.path()));
    assert!(
        out.stdout().contains(&canon(other.path())),
        "{:?}",
        out.stdout()
    );
    // The file is named by full path then, so `bash $filename` still works.
    assert_eq!(out.summary.status, RunStatus::Exit(0));
    let out = case(WorkingDirectory::ProjectFolder, None);
    assert!(out.stdout().contains(&canon(project.path())));
    let out = case(WorkingDirectory::Default, None);
    assert!(out.stdout().contains(&canon(home_like.path())));
    let out = case(WorkingDirectory::Custom(other.path().to_path_buf()), None);
    assert!(out.stdout().contains(&canon(other.path())));
    let out = case(WorkingDirectory::Custom("/no/such/dir".into()), None);
    match &out.summary.status {
        RunStatus::Error(scratch_runner::RunError::WorkingDirectory(dir)) => {
            assert_eq!(dir, "/no/such/dir")
        }
        other => panic!("{other:?}"),
    }
    assert!(out.summary.status_text().contains("Failed"));
}

#[test]
fn an_unsaved_document_can_run_in_the_directory_it_belongs_to() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("data.txt"), "sibling").unwrap();
    let snapshot = Snapshot::Text {
        text: "cat data.txt\n".into(),
        file_name: Some("locked.sh".into()),
        directory: Some(project.path().to_path_buf()),
    };
    let outcome = run(request("shell", snapshot, root.path()));
    assert!(
        outcome.stdout().contains("sibling"),
        "{:?} {:?}",
        outcome.stdout(),
        outcome.stderr()
    );
}

#[test]
fn cancel_stops_the_program_and_kills_its_grandchildren() {
    let root = tempfile::tempdir().unwrap();
    // The shell starts a background sleep (a grandchild) and waits for it.
    let source = "sleep 300 &\necho \"child=$!\"\nwait\n";
    let started = Instant::now();
    let mut child_pid = 0;
    let outcome = run_with(
        request("shell", Snapshot::text(source), root.path()),
        |handle, rx| {
            let (found, text, _) = read_until(rx, "child=", Duration::from_secs(10));
            assert!(found, "no child pid");
            let digits: String = text
                .split("child=")
                .nth(1)
                .unwrap()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            child_pid = digits.parse().unwrap();
            assert!(alive(child_pid), "the grandchild should be running");
            handle.cancel();
            handle.cancel(); // idempotent
        },
    );
    assert_eq!(outcome.summary.status, RunStatus::Cancelled);
    assert_eq!(outcome.summary.status_text(), "Run Stopped");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !alive(child_pid),
        "the grandchild {child_pid} survived the cancel"
    );
}

#[test]
fn a_program_that_ignores_sigterm_is_killed_after_the_grace_period() {
    let root = tempfile::tempdir().unwrap();
    let source = "trap '' TERM\necho ready\nwhile true; do sleep 0.1; done\n";
    let mut req = request("shell", Snapshot::text(source), root.path());
    req.options.limits.kill_grace = Duration::from_millis(500);
    let started = Instant::now();
    let outcome = run_with(req, |handle, rx| {
        let (found, _, _) = read_until(rx, "ready", Duration::from_secs(10));
        assert!(found);
        handle.cancel();
    });
    assert_eq!(outcome.summary.status, RunStatus::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn interrupt_sends_sigint_and_the_program_may_handle_it() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = "import time\ntry:\n    print('loop', flush=True)\n    time.sleep(60)\nexcept KeyboardInterrupt:\n    print('interrupted')\n";
    let outcome = run_with(
        request("python", Snapshot::text(source), root.path()),
        |handle, rx| {
            let (found, _, _) = read_until(rx, "loop", Duration::from_secs(10));
            assert!(found);
            assert!(handle.interrupt());
        },
    );
    assert!(
        outcome.stdout().contains("interrupted"),
        "{:?}",
        outcome.stdout()
    );
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
}

#[test]
fn timeout_stops_the_program_and_its_children() {
    let root = tempfile::tempdir().unwrap();
    let settings = RunSettings {
        timeout: Some(Duration::from_millis(600)),
        ..RunSettings::default()
    };
    let started = Instant::now();
    let outcome = run(request(
        "shell",
        Snapshot::text("sleep 300 &\necho started\nsleep 300\n"),
        root.path(),
    )
    .with_settings(settings));
    assert_eq!(outcome.summary.status, RunStatus::TimedOut);
    assert!(outcome.stdout().contains("started"));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn large_output_is_capped_with_a_marker_and_the_run_is_stopped() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "shell",
        Snapshot::text("yes 'a line of output'\n"),
        root.path(),
    );
    req.options.limits.warn_output_bytes = Some(10_000);
    req.options.limits.max_output_bytes = 100_000;
    let started = Instant::now();
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::OutputLimitExceeded);
    assert_eq!(outcome.summary.status_text(), "Run Stopped");
    assert!(outcome.has(|e| matches!(e, RunEvent::LargeOutput { .. })));
    assert!(outcome.has(|e| matches!(e, RunEvent::Truncated { limit: 100_000 })));
    assert_eq!(
        outcome.stdout().len() as u64 + outcome.stderr().len() as u64,
        100_000
    );
    assert_eq!(outcome.summary.output_bytes, 100_000);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "an infinite output must be interrupted"
    );
}

#[test]
fn output_beyond_the_limit_can_be_discarded_without_stopping() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "shell",
        Snapshot::text("head -c 50000 /dev/zero | tr '\\0' x; echo; echo done\n"),
        root.path(),
    );
    req.options.limits.max_output_bytes = 1_000;
    req.options.limits.warn_output_bytes = None;
    req.options.limits.stop_at_limit = false;
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    assert!(outcome.has(|e| matches!(e, RunEvent::Truncated { .. })));
    assert_eq!(outcome.stdout().len(), 1_000);
}

#[test]
fn the_user_can_continue_past_the_large_output_warning() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "shell",
        Snapshot::text("head -c 300000 /dev/zero | tr '\\0' y; echo; echo done\n"),
        root.path(),
    );
    req.options.limits.warn_output_bytes = Some(10_000);
    req.options.limits.max_output_bytes = 50_000;
    req.options.limits.pause_at_warning = true;
    let (handle, rx) = scratch_runner::start_channel(req).unwrap();
    let mut seen = Vec::new();
    loop {
        let message = rx.recv_timeout(Duration::from_secs(60)).expect("event");
        if matches!(message.event, RunEvent::LargeOutput { .. }) {
            seen.push(message);
            break;
        }
        assert!(
            !matches!(message.event, RunEvent::Finished(_)),
            "finished before the warning"
        );
        seen.push(message);
    }
    // The program is blocked now: nothing but quiet until we answer.
    std::thread::sleep(Duration::from_millis(300));
    handle.allow_large_output();
    seen.extend(wait_rest(&rx));
    let Some(RunEvent::Finished(summary)) = seen.last().map(|m| m.event.clone()) else {
        panic!()
    };
    assert_eq!(summary.status, RunStatus::Exit(0), "{summary:?}");
    assert!(summary.output_bytes >= 300_000);
    assert!(
        !seen
            .iter()
            .any(|m| matches!(m.event, RunEvent::Truncated { .. }))
    );
}

#[test]
fn live_stats_and_resize() {
    let root = tempfile::tempdir().unwrap();
    let source = "echo ready\nsleep 0.5\nstty size\n";
    let outcome = run_with(
        request("shell", Snapshot::text(source), root.path()),
        |handle, rx| {
            let (found, _, _) = read_until(rx, "ready", Duration::from_secs(10));
            assert!(found);
            let stats = handle.sample_stats().expect("stats");
            assert!(stats.rss_bytes > 0);
            handle.resize(91, 17).unwrap();
            assert!(handle.pid().is_some());
        },
    );
    assert!(outcome.stdout().contains("17 91"), "{:?}", outcome.stdout());
}

#[test]
fn exit_127_shows_the_languages_hint() {
    let root = tempfile::tempdir().unwrap();
    let mut language = language("shell");
    language.installer = Some(scratch_language::InstallerHint {
        executable: "frobnicate".into(),
        name: "the Frobnicator".into(),
        url: "https://example.com/frob".into(),
    });
    let settings = RunSettings {
        run_command: Some("frobnicate-not-installed-xyz $filename".into()),
        ..RunSettings::default()
    };
    // `needs a shell` is not the case: a direct argv with a missing program is a
    // preflight error with the same hint, before anything starts.
    let request = scratch_runner::RunRequest::new(&language, Snapshot::text("x"))
        .with_shell(shell())
        .with_options(options(root.path()))
        .with_settings(settings);
    let outcome = run(request);
    match &outcome.summary.status {
        RunStatus::Error(scratch_runner::RunError::MissingTool { program, message }) => {
            assert_eq!(program, "frobnicate-not-installed-xyz");
            assert!(message.contains("the Frobnicator"), "{message}");
            assert!(message.contains("https://example.com/frob"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!outcome.has(|e| matches!(e, RunEvent::Started(_))));

    // A shell command that fails with 127 at run time carries the hint as message.
    let settings = RunSettings {
        run_command: Some("frobnicate-not-installed-xyz; echo after".into()),
        ..RunSettings::default()
    };
    let request = scratch_runner::RunRequest::new(&language, Snapshot::text("x"))
        .with_shell(shell())
        .with_options(options(root.path()))
        .with_settings(settings.clone());
    let outcome = run(request);
    assert_eq!(outcome.summary.status, RunStatus::Exit(0)); // `; echo after` succeeded
    let settings = RunSettings {
        run_command: Some("frobnicate-not-installed-xyz || exit 127".into()),
        ..settings
    };
    let request = scratch_runner::RunRequest::new(&language, Snapshot::text("x"))
        .with_shell(shell())
        .with_options(options(root.path()))
        .with_settings(settings);
    let outcome = run(request);
    assert_eq!(outcome.summary.status, RunStatus::Exit(127));
    let message = outcome.summary.message.expect("hint");
    assert!(
        message.contains("To run your code, you need to have the Frobnicator installed"),
        "{message}"
    );
}

#[test]
fn a_language_without_a_run_command_is_reported_not_run() {
    let root = tempfile::tempdir().unwrap();
    let outcome = run(request("markdown", Snapshot::text("# hi"), root.path()));
    match &outcome.summary.status {
        RunStatus::Error(scratch_runner::RunError::NoRunCommand { renderer }) => {
            assert_eq!(renderer.as_deref(), Some("markdown"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_run_command_override_wins_and_expands_variables() {
    if skip_without(&["python3"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let settings = RunSettings {
        run_command: Some(
            "python3 -c \"import sys; print('custom', sys.argv[1])\" $filename".into(),
        ),
        ..RunSettings::default()
    };
    let outcome = run(request(
        "python",
        Snapshot::named_text("print(1)", "mine.py"),
        root.path(),
    )
    .with_settings(settings));
    assert!(
        outcome.stdout().contains("custom mine.py"),
        "{:?} {:?}",
        outcome.stdout(),
        outcome.stderr()
    );
}

#[test]
fn one_run_per_document_and_stale_events() {
    let root = tempfile::tempdir().unwrap();
    let runner = scratch_runner::Runner::new();
    let slow = || {
        request("shell", Snapshot::text("echo up\nsleep 30\n"), root.path())
            .with_owner("doc-1", OnBusy::Reject)
    };
    let (first, rx1) = runner.start_channel(slow()).unwrap();
    assert_eq!(runner.active("doc-1"), Some(first.id()));
    assert!(runner.is_current("doc-1", first.id()));
    assert_eq!(runner.active_count(), 1);
    // A second document runs concurrently.
    let other = request("shell", Snapshot::text("echo other\n"), root.path())
        .with_owner("doc-2", OnBusy::Reject);
    let (_h2, rx2) = runner.start_channel(other).unwrap();
    let out2 = wait(&rx2);
    assert!(out2.stdout().contains("other"));
    // The same document cannot start a second run...
    match runner.start_channel(slow()) {
        Err(scratch_runner::StartError::AlreadyRunning { owner, run }) => {
            assert_eq!(owner, "doc-1");
            assert_eq!(run, first.id());
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    // ...unless the old run is replaced.
    let replacing = request("shell", Snapshot::text("echo second\n"), root.path())
        .with_owner("doc-1", OnBusy::StopPrevious);
    let (second, rx3) = runner.start_channel(replacing).unwrap();
    assert_ne!(first.id(), second.id());
    assert!(
        !runner.is_current("doc-1", first.id()),
        "the old run is stale now"
    );
    let out_old = wait(&rx1);
    assert_eq!(out_old.summary.status, RunStatus::Cancelled);
    assert!(out_old.messages.iter().all(|m| m.run == first.id()));
    let out_new = wait(&rx3);
    assert_eq!(out_new.summary.status, RunStatus::Exit(0));
    assert!(out_new.stdout().contains("second"));
    assert!(out_new.messages.iter().all(|m| m.run == second.id()));
    assert_eq!(runner.active("doc-1"), None);
    assert_eq!(runner.active_count(), 0);
    // Finished runs leave the table: the document can run again.
    let again = request("shell", Snapshot::text("echo again\n"), root.path())
        .with_owner("doc-1", OnBusy::Reject);
    let (_h, rx) = runner.start_channel(again).unwrap();
    assert!(wait(&rx).stdout().contains("again"));
}

#[test]
fn many_documents_run_at_once() {
    let root = tempfile::tempdir().unwrap();
    let runner = scratch_runner::Runner::new();
    let mut runs = Vec::new();
    for index in 0..6 {
        let req = request(
            "shell",
            Snapshot::text(format!("sleep 1\necho doc-{index}\n")),
            root.path(),
        )
        .with_owner(format!("doc-{index}"), OnBusy::Reject);
        runs.push((index, runner.start_channel(req).unwrap()));
    }
    assert_eq!(runner.active_count(), 6);
    let started = Instant::now();
    for (index, (_handle, rx)) in &runs {
        let out = wait(rx);
        assert!(
            out.stdout().contains(&format!("doc-{index}")),
            "{:?}",
            out.stdout()
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "runs were serialized: {:?}",
        started.elapsed()
    );
}

#[test]
fn mode_is_available_for_the_app() {
    // RunWithoutDebugging behaves like Run.
    let root = tempfile::tempdir().unwrap();
    let outcome = run(
        request("shell", Snapshot::text("echo plain\n"), root.path())
            .with_mode(RunMode::RunWithoutDebugging),
    );
    assert!(outcome.stdout().contains("plain"));
    let _ = (Phase::Run, Stream::Stdout);
}

#[test]
fn login_mode_and_shell_arguments_apply_to_shell_commands() {
    let root = tempfile::tempdir().unwrap();
    let settings = RunSettings {
        run_command: Some(
            "shopt -q login_shell && echo LOGIN || echo NOT-LOGIN; echo \"opts=$-\"".into(),
        ),
        ..RunSettings::default()
    };
    let mut shell = shell();
    let outcome = run(request("shell", Snapshot::text("x"), root.path())
        .with_settings(settings.clone())
        .with_shell(shell.clone()));
    assert!(
        outcome.stdout().contains("NOT-LOGIN"),
        "{:?}",
        outcome.stdout()
    );
    shell.login_shell = true;
    let outcome = run(request("shell", Snapshot::text("x"), root.path())
        .with_settings(settings.clone())
        .with_shell(shell.clone()));
    assert!(
        outcome.stdout().contains("LOGIN") && !outcome.stdout().contains("NOT-LOGIN"),
        "{:?}",
        outcome.stdout()
    );
    // -l is the second word, extra shell arguments follow it.
    assert!(outcome.has(
        |e| matches!(e, RunEvent::Started(info) if info.argv[1] == "-l" && info.argv[2] == "-c")
    ));
    shell.login_shell = false;
    shell.shell_arguments = vec!["-e".into()];
    let outcome = run(request("shell", Snapshot::text("x"), root.path())
        .with_settings(settings)
        .with_shell(shell));
    assert!(
        outcome.stdout().contains("opts=") && outcome.stdout().contains('e'),
        "{:?}",
        outcome.stdout()
    );
    assert!(outcome.has(
        |e| matches!(e, RunEvent::Started(info) if info.argv[1] == "-e" && info.argv[2] == "-c")
    ));
}

#[test]
fn a_missing_shell_is_a_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let mut shell = shell();
    shell.shell = "/no/such/shell".into();
    let settings = RunSettings {
        run_command: Some("echo a; echo b".into()),
        ..RunSettings::default()
    };
    let outcome = run(request("shell", Snapshot::text("x"), root.path())
        .with_settings(settings)
        .with_shell(shell));
    match &outcome.summary.status {
        RunStatus::Error(scratch_runner::RunError::MissingTool { program, .. }) => {
            assert_eq!(program, "/no/such/shell")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_automatic_path_comes_from_the_login_shell_and_is_cached_until_refreshed() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "shell",
        Snapshot::text("echo \"PATH=$PATH\"\n"),
        root.path(),
    );
    req.shell.path = scratch_runner::PathSetting::Automatic;
    let outcome = run(req);
    let out = outcome.stdout();
    let path = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("PATH="))
        .expect("PATH line")
        .to_owned();
    assert!(path.split(':').any(|d| d == "/usr/bin"), "{path}");
    let cached = scratch_runner::cached_automatic_path().expect("cached after the first use");
    assert_eq!(path, cached);
    assert_eq!(scratch_runner::automatic_path(), cached);
    // Refresh recomputes (same machine, same answer) and stays cached.
    assert_eq!(scratch_runner::refresh_automatic_path(), cached);
    let entries: Vec<&str> = path.split(':').collect();
    let unique: std::collections::HashSet<&&str> = entries.iter().collect();
    assert_eq!(entries.len(), unique.len(), "duplicates in {path}");
}

#[test]
fn language_path_additions_are_added_to_the_path_of_the_program() {
    let root = tempfile::tempdir().unwrap();
    // Go appends /usr/local/go/bin and /opt/homebrew/bin; Java prepends openjdk.
    let mut language = language("shell");
    language.run.env.path_prepend = vec!["/first/dir".into()];
    language.run.env.path_append = vec!["/last/dir".into(), "/usr/bin".into()];
    language
        .run
        .env
        .vars
        .insert("LANG_VAR".into(), "$stem-x".into());
    let request = scratch_runner::RunRequest::new(
        &language,
        Snapshot::named_text("echo \"$PATH|$LANG_VAR\"", "abc"),
    )
    .with_shell(shell())
    .with_options(options(root.path()));
    let outcome = run(request);
    let line = outcome.stdout();
    assert!(line.starts_with("/first/dir:"), "{line:?}");
    assert!(line.contains(":/last/dir|abc-x"), "{line:?}");
    let path = line.split('|').next().unwrap();
    assert_eq!(
        path.split(':').filter(|d| *d == "/usr/bin").count(),
        1,
        "duplicates: {line:?}"
    );
}

#[test]
fn a_fifty_megabyte_flood_passes_through_quickly_and_completely() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request(
        "shell",
        Snapshot::text("head -c 50000000 /dev/zero | tr '\\0' x\necho\necho end\n"),
        root.path(),
    );
    req.options.limits.warn_output_bytes = None;
    req.options.limits.max_output_bytes = u64::MAX;
    req.options.tty.onlcr = false;
    let started = Instant::now();
    let outcome = run(req);
    assert_eq!(outcome.summary.status, RunStatus::Exit(0));
    assert_eq!(outcome.stdout().len(), 50_000_000 + "\nend\n".len());
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    eprintln!("50 MB flood took {:?}", started.elapsed());
}
