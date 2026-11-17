//! Live LLDB integration tests.
//!
//! These compile real C programs with `clang -g` and drive the installed
//! `/usr/bin/lldb`.  They skip visibly when either tool is missing, and also
//! skip when LLDB refuses to attach because Developer Tools access has not
//! been granted to the test runner.

mod support;

use std::path::PathBuf;
use std::time::Duration;

use scratch_debugger::*;
use support::*;

const WAIT: Duration = Duration::from_secs(15);

fn lldb_adapter() -> DebuggerAdapter {
    AdapterRegistry::builtin()
        .get("lldb")
        .expect("lldb adapter")
        .clone()
}

fn session_for(
    dir: &std::path::Path,
    binary: PathBuf,
    breakpoints: Vec<Breakpoint>,
) -> DebugSession {
    let config = SessionConfig::new(lldb_adapter(), binary)
        .cwd(dir)
        .breakpoints(breakpoints)
        .start_timeout(WAIT);
    match DebugSession::start(config) {
        Ok(session) => session,
        Err(err) => panic!("could not start lldb: {err}"),
    }
}

#[test]
fn lldb_break_step_variables_edit_and_temporary() {
    if skip_without("clang") || skip_without("lldb") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"#include <stdio.h>

typedef struct { int x; int y; } Point;

int add(int a, int b) {
    int s = a + b;
    return s;
}

int main(void) {
    int n = 3;
    Point p = {1, 2};
    int total = add(n, p.x);
    printf("total=%d\n", total);
    total = add(total, 10);
    return 0;
}
"#;
    let Some(binary) = compile_c(dir.path(), source, "stepper") else {
        return;
    };
    let file = dir.path().join("stepper.c");
    let session = session_for(
        dir.path(),
        binary,
        vec![
            Breakpoint::new(&file, 13),
            Breakpoint::new(&file, 15).temporary(),
        ],
    );
    let mut log = EventLog::new();

    let (location, reason, variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.line, Some(13), "first breakpoint line");
    assert_eq!(location.function.as_deref(), Some("main"));
    assert!(
        matches!(
            reason,
            StopReason::Breakpoint {
                temporary: false,
                ..
            }
        ),
        "unexpected reason: {reason:?}"
    );
    assert_eq!(
        EventLog::variable(&variables, "n").value.as_deref(),
        Some("3")
    );
    let p = EventLog::variable(&variables, "p");
    assert_eq!(p.children.len(), 2, "struct should have two children");

    // Lazy expansion fetches just the struct.
    let p_id = p.id.clone();
    session
        .expand_variable(location.frame, p_id.clone(), 2)
        .expect("expand");
    let subtree = log.wait_subtree(&session, &p_id, WAIT);
    let names: Vec<&str> = subtree.roots.iter().map(|v| v.name.as_str()).collect();
    assert!(names.contains(&"x"), "expanded subtree: {names:?}");
    assert!(names.contains(&"y"), "expanded subtree: {names:?}");

    // Expression evaluation and print description.
    session.evaluate("n").expect("evaluate");
    let value = match log.wait(&session, WAIT, "evaluate", |event| {
        matches!(event, DebugEvent::ExpressionResult { .. })
    }) {
        DebugEvent::ExpressionResult { text, .. } => text,
        other => panic!("expected ExpressionResult, got {other:?}"),
    };
    assert_eq!(value, "3");

    session.print_description("n").expect("print description");
    let description = match log.wait(&session, WAIT, "print description", |event| {
        matches!(
            event,
            DebugEvent::ExpressionResult {
                kind: ExpressionKind::PrintDescription,
                ..
            }
        )
    }) {
        DebugEvent::ExpressionResult { text, .. } => text,
        other => panic!("expected ExpressionResult, got {other:?}"),
    };
    assert!(
        description.contains('3'),
        "po output should contain 3: {description:?}"
    );

    // Edit a value and check the refreshed tree.
    session.set_variable(location.frame, "n", "42").expect("set");
    let updated = log.wait_variables(&session, WAIT);
    assert_eq!(
        EventLog::variable(&updated, "n").value.as_deref(),
        Some("42")
    );

    // Step in, over and out.
    session.step_into().expect("step in");
    let (location, reason, _) = log.wait_stop(&session, WAIT);
    assert_eq!(location.function.as_deref(), Some("add"));
    assert!(matches!(reason, StopReason::StepIn), "{reason:?}");

    session.step_over().expect("step over");
    let (_, reason, _) = log.wait_stop(&session, WAIT);
    assert!(matches!(reason, StopReason::StepOver), "{reason:?}");

    session.step_out().expect("step out");
    let (location, reason, _) = log.wait_stop(&session, WAIT);
    assert_eq!(location.function.as_deref(), Some("main"));
    assert!(matches!(reason, StopReason::StepOut), "{reason:?}");

    // Continue into the temporary breakpoint.
    session.continue_().expect("continue");
    let (location, reason, annotation) = log.wait_stop(&session, WAIT);
    assert_eq!(location.line, Some(15));
    assert!(
        matches!(
            reason,
            StopReason::Breakpoint {
                temporary: true,
                ..
            }
        ),
        "expected temporary breakpoint, got {reason:?}"
    );
    assert!(annotation.contains("Temporary"), "{annotation}");

    // The adapter's list and clear-all verbs round-trip with the debugger.
    session
        .set_breakpoints(vec![Breakpoint::new(&file, 13)])
        .expect("reset breakpoints");
    log.wait(&session, WAIT, "breakpoints replaced", |event| {
        matches!(event, DebugEvent::BreakpointsChanged(list) if list.len() == 1)
    });
    session.list_breakpoints().expect("list breakpoints");
    log.wait(&session, WAIT, "breakpoint listing", |event| {
        matches!(event, DebugEvent::DebuggerOutput { text } if text.contains("Breakpoint 1"))
    });
    session.clear_all_breakpoints().expect("clear all");
    log.wait(&session, WAIT, "breakpoints cleared", |event| {
        matches!(event, DebugEvent::BreakpointsChanged(list) if list.is_empty())
    });

    // And then to exit.
    session.continue_().expect("continue");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
}

#[test]
fn lldb_conditional_breakpoint_watch_and_repl() {
    if skip_without("clang") || skip_without("lldb") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"#include <stdio.h>

int main(void) {
    int sum = 0;
    for (int i = 0; i < 5; i++) {
        sum += i;
    }
    printf("%d\n", sum);
    return 0;
}
"#;
    let Some(binary) = compile_c(dir.path(), source, "cond") else {
        return;
    };
    let file = dir.path().join("cond.c");
    let session = session_for(
        dir.path(),
        binary,
        vec![Breakpoint::new(&file, 6).with_condition("i == 3")],
    );
    let mut log = EventLog::new();
    let (location, _reason, variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.line, Some(6));
    assert_eq!(
        EventLog::variable(&variables, "i").value.as_deref(),
        Some("3"),
        "conditional breakpoint should stop when i == 3"
    );

    session.add_watch("sum").expect("watch");
    let watch = match log.wait(&session, WAIT, "watch values", |event| {
        matches!(event, DebugEvent::WatchValues(_))
    }) {
        DebugEvent::WatchValues(values) => values,
        other => panic!("expected WatchValues, got {other:?}"),
    };
    let sum = watch
        .iter()
        .find(|value| value.expression == "sum")
        .expect("sum watch");
    assert_eq!(sum.value.as_deref(), Some("3"), "{watch:?}");

    session.repl("frame variable i").expect("repl");
    let repl = match log.wait(&session, WAIT, "repl", |event| {
        matches!(
            event,
            DebugEvent::ExpressionResult {
                kind: ExpressionKind::Repl,
                ..
            }
        )
    }) {
        DebugEvent::ExpressionResult { text, .. } => text,
        other => panic!("expected ExpressionResult, got {other:?}"),
    };
    assert!(repl.contains("i = 3"), "repl output: {repl:?}");

    session.continue_().expect("continue");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
}

#[test]
fn lldb_forwards_program_stdin() {
    if skip_without("clang") || skip_without("lldb") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"#include <stdio.h>

int main(void) {
    char buf[32];
    if (fgets(buf, sizeof buf, stdin)) printf("got:%s", buf);
    return 0;
}
"#;
    let Some(binary) = compile_c(dir.path(), source, "stdin") else {
        return;
    };
    let file = dir.path().join("stdin.c");
    let session = session_for(dir.path(), binary, vec![Breakpoint::new(&file, 5)]);
    let mut log = EventLog::new();
    let (_location, _reason, _variables) = log.wait_stop_and_variables(&session, WAIT);

    session.continue_().expect("continue");
    session
        .send_program_input("hello\n")
        .expect("program input");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
    let output = log.program_output(&session, Duration::from_millis(300));
    assert!(
        output.iter().any(|line| line.contains("got:hello")),
        "program output: {output:?}"
    );
}

#[test]
fn lldb_reports_a_crash_and_can_be_inspected() {
    if skip_without("clang") || skip_without("lldb") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"#include <stdio.h>

int main(void) {
    printf("before\n");
    fflush(stdout);
    int *p = 0;
    *p = 1;
    return 0;
}
"#;
    let Some(binary) = compile_c(dir.path(), source, "crash") else {
        return;
    };
    let session = session_for(dir.path(), binary, Vec::new());
    let mut log = EventLog::new();
    let (location, reason, _annotation) = log.wait_stop(&session, WAIT);
    assert!(location.line.is_some());
    assert!(
        matches!(reason, StopReason::Signal { .. }),
        "expected a signal stop, got {reason:?}"
    );
    // The session should still answer variables for the crashed frame.
    let _variables = log.wait_variables(&session, WAIT);
}

#[test]
fn lldb_pause_interrupts_a_running_program() {
    if skip_without("clang") || skip_without("lldb") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"#include <stdio.h>

int main(void) {
    volatile long x = 0;
    printf("loop\n");
    fflush(stdout);
    for (;;) { x = x + 1; }
    return 0;
}
"#;
    let Some(binary) = compile_c(dir.path(), source, "loop") else {
        return;
    };
    let session = session_for(dir.path(), binary, Vec::new());
    let mut log = EventLog::new();
    log.wait(&session, WAIT, "running", |event| {
        matches!(event, DebugEvent::State(DebuggerState::Running))
    });
    log.wait(&session, WAIT, "program output", |event| {
        matches!(event, DebugEvent::Output { text, .. } if text.contains("loop"))
    });
    session.pause().expect("pause");
    let (location, reason, _annotation) = log.wait_stop(&session, WAIT);
    assert!(location.function.as_deref() == Some("main"));
    assert!(matches!(reason, StopReason::Pause), "reason: {reason:?}");
    session.quit().expect("quit");
}
