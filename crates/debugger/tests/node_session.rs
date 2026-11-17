//! Live `node inspect` integration test (best-effort adapter).
//!
//! Node's text-mode inspector reprints its prompt on every readline redraw, so
//! this test mainly proves that the prompt/echo handling, stop detection,
//! evaluation and program output forwarding work against the real tool.

mod support;

use std::time::Duration;

use scratch_debugger::*;
use support::*;

const WAIT: Duration = Duration::from_secs(25);

#[test]
fn node_break_evaluate_step_and_exit() {
    if skip_without("node") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = r#"function add(a, b) {
  const s = a + b;
  return s;
}
let n = 3;
const total = add(n, 4);
console.log("total=", total);
process.exit(0);
"#;
    std::fs::write(dir.path().join("prog.js"), source).expect("write script");
    let adapter = AdapterRegistry::builtin()
        .get("node")
        .expect("node adapter")
        .clone();
    let session = DebugSession::start(
        SessionConfig::new(adapter, dir.path().join("prog.js"))
            .cwd(dir.path())
            .breakpoints(vec![Breakpoint::new("prog.js", 6)])
            .start_timeout(WAIT),
    )
    .expect("start node inspect");
    let mut log = EventLog::new();

    let (location, reason, _) = log.wait_stop(&session, WAIT);
    assert_eq!(location.line, Some(6), "first breakpoint: {location:?}");
    assert!(
        matches!(reason, StopReason::Breakpoint { .. }),
        "reason: {reason:?}"
    );

    session.evaluate("n").expect("evaluate");
    let value = match log.wait(&session, WAIT, "evaluate", |event| {
        matches!(
            event,
            DebugEvent::ExpressionResult {
                kind: ExpressionKind::Evaluate,
                ..
            }
        )
    }) {
        DebugEvent::ExpressionResult { text, .. } => text,
        other => panic!("expected ExpressionResult, got {other:?}"),
    };
    assert_eq!(value, "3");

    session.step_over().expect("step over");
    let (location, reason, _) = log.wait_stop(&session, WAIT);
    assert_eq!(location.line, Some(7));
    assert!(matches!(reason, StopReason::StepOver), "reason: {reason:?}");

    session.continue_().expect("continue");
    assert_eq!(log.wait_exit(&session, WAIT), 0);
    let output = log.program_output(&session, Duration::from_millis(300));
    assert!(
        output.iter().any(|line| line.contains("total= 7")),
        "program output: {output:?}"
    );
}
