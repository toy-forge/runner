//! Live pdb integration tests.

mod support;

use std::time::Duration;

use scratch_debugger::*;
use support::*;

const WAIT: Duration = Duration::from_secs(20);

fn pdb_adapter() -> DebuggerAdapter {
    AdapterRegistry::builtin()
        .get("pdb")
        .expect("pdb adapter")
        .clone()
}

fn session_for(
    dir: &std::path::Path,
    script: std::path::PathBuf,
    breakpoints: Vec<Breakpoint>,
) -> DebugSession {
    let config = SessionConfig::new(pdb_adapter(), script)
        .cwd(dir)
        .breakpoints(breakpoints)
        .start_timeout(WAIT);
    match DebugSession::start(config) {
        Ok(session) => session,
        Err(err) => panic!("could not start pdb: {err}"),
    }
}

#[test]
fn pdb_break_step_variables_edit_watch_and_input() {
    if skip_without("python3") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"def add(a, b):
    s = a + b
    return s

def main():
    n = 3
    p = {"x": 1, "y": 2}
    total = add(n, p["x"])
    print("total=", total)
    name = input("name? ")
    print("hi", name)
    return total

main()
"#;
    let script = write_script(dir.path(), "prog.py", source);
    let session = session_for(dir.path(), script, vec![Breakpoint::new("prog.py", 8)]);
    let mut log = EventLog::new();

    let (location, reason, variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.line, Some(8));
    assert_eq!(location.function.as_deref(), Some("main"));
    assert!(
        matches!(reason, StopReason::Breakpoint { .. }),
        "reason: {reason:?}"
    );
    assert_eq!(
        EventLog::variable(&variables, "n").value.as_deref(),
        Some("3")
    );

    // Step into add.
    session.step_into().expect("step in");
    let (location, reason, _variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.function.as_deref(), Some("add"));
    assert!(matches!(reason, StopReason::StepIn), "reason: {reason:?}");

    // pdb stops on the function's `def` line first; advance to the return
    // statement.  Newer Python versions may stop on the first body line
    // directly, so keep stepping until line 3.
    let mut line = 0;
    for _ in 0..3 {
        session.step_over().expect("step over");
        let (location, reason, _) = log.wait_stop_and_variables(&session, WAIT);
        assert_eq!(location.function.as_deref(), Some("add"));
        assert!(matches!(reason, StopReason::StepOver), "reason: {reason:?}");
        line = location.line.unwrap_or_default();
        if line >= 3 {
            break;
        }
    }
    assert_eq!(line, 3, "step over should reach `return s`");

    // Step out lands in the caller, one statement past the call.
    session.step_out().expect("step out");
    let (location, reason, variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.function.as_deref(), Some("main"));
    assert!(matches!(reason, StopReason::StepOut), "reason: {reason:?}");
    assert_eq!(location.line, Some(9));

    let p = EventLog::variable(&variables, "p");
    assert!(
        p.children.len() == 2 || p.expandable,
        "dict should have children: {p:?}"
    );
    if p.children.is_empty() {
        let p_id = p.id.clone();
        session
            .expand_variable(location.frame, p_id.clone(), 2)
            .expect("expand");
        let subtree = log.wait_subtree(&session, &p_id, WAIT);
        let names: Vec<&str> = subtree.roots.iter().map(|v| v.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains('x')), "{names:?}");
    }

    session.evaluate("n").expect("evaluate");
    let value = match log.wait(&session, WAIT, "evaluate", |event| {
        matches!(event, DebugEvent::ExpressionResult { kind: ExpressionKind::Evaluate, .. })
    }) {
        DebugEvent::ExpressionResult { text, .. } => text,
        other => panic!("expected ExpressionResult, got {other:?}"),
    };
    assert_eq!(value, "3");

    session.print_description("p").expect("print description");
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
    assert!(description.contains("'x': 1"), "{description:?}");

    session.add_watch("total").expect("watch");
    let watch = match log.wait(&session, WAIT, "watch", |event| {
        matches!(event, DebugEvent::WatchValues(_))
    }) {
        DebugEvent::WatchValues(values) => values,
        other => panic!("expected WatchValues, got {other:?}"),
    };
    let total = watch
        .iter()
        .find(|value| value.expression == "total")
        .expect("total watch");
    assert_eq!(total.value.as_deref(), Some("4"), "{watch:?}");

    session.repl("p n").expect("repl");
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
    assert_eq!(repl, "3");

    // Edit a variable.
    session.set_variable(location.frame, "n", "42").expect("set");
    let updated = log.wait_variables(&session, WAIT);
    assert_eq!(
        EventLog::variable(&updated, "n").value.as_deref(),
        Some("42")
    );

    // Run to the input prompt, feed stdin, and exit.
    session.continue_().expect("continue");
    session
        .send_program_input("bob\n")
        .expect("program input");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
    let output = log.program_output(&session, Duration::from_millis(300));
    assert!(
        output.iter().any(|line| line.contains("total= 4")),
        "program output: {output:?}"
    );
    assert!(
        output.iter().any(|line| line.contains("hi bob")),
        "program output: {output:?}"
    );
}

#[test]
fn pdb_conditional_and_temporary_breakpoints() {
    if skip_without("python3") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"def main():
    total = 0
    for i in range(5):
        total += i
    print(total)
    return total
main()
"#;
    let script = write_script(dir.path(), "loop.py", source);
    let session = session_for(
        dir.path(),
        script,
        vec![
            Breakpoint::new("loop.py", 4).with_condition("i == 3"),
            Breakpoint::new("loop.py", 5).temporary(),
        ],
    );
    let mut log = EventLog::new();
    let (location, _reason, variables) = log.wait_stop_and_variables(&session, WAIT);
    assert_eq!(location.line, Some(4));
    assert_eq!(
        EventLog::variable(&variables, "i").value.as_deref(),
        Some("3"),
        "conditional breakpoint must wait for i == 3"
    );

    session.continue_().expect("continue");
    let (location, _reason, annotation) = log.wait_stop(&session, WAIT);
    assert_eq!(location.line, Some(5));
    assert!(annotation.contains("Temporary"), "{annotation}");
    let changed = match log.wait(&session, WAIT, "breakpoints changed", |event| {
        matches!(event, DebugEvent::BreakpointsChanged(list) if !list.iter().any(|bp| bp.temporary))
    }) {
        DebugEvent::BreakpointsChanged(list) => list,
        other => panic!("expected BreakpointsChanged, got {other:?}"),
    };
    assert_eq!(changed.len(), 1, "temporary breakpoint was consumed: {changed:?}");

    session.continue_().expect("continue");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
}

#[test]
fn pdb_reports_uncaught_exceptions() {
    if skip_without("python3") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"def boom(n):
    return 10 // n

boom(0)
"#;
    let script = write_script(dir.path(), "crash.py", source);
    let session = session_for(dir.path(), script, Vec::new());
    let mut log = EventLog::new();
    let (location, reason, _annotation) = log.wait_stop(&session, WAIT);
    assert_eq!(location.function.as_deref(), Some("boom"));
    match &reason {
        StopReason::Exception { name, message } => {
            assert_eq!(name, "ZeroDivisionError");
            assert_eq!(message.as_deref(), Some("division by zero"));
        }
        other => panic!("expected an exception stop, got {other:?}"),
    }
    // The post-mortem frame can still be inspected.
    let variables = log.wait_variables(&session, WAIT);
    assert_eq!(
        EventLog::variable(&variables, "n").value.as_deref(),
        Some("0")
    );
}
