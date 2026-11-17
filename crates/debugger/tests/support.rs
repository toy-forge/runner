//! Shared helpers for the live debugger tests.
//!
//! Every helper that needs an external tool returns early with a visible
//! `skipping ...` message when the tool is absent, so the suite passes on
//! machines without LLDB/Python/Node while still exercising the real tools
//! when they are installed.

#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use scratch_debugger::*;

/// Return true (and print a skip notice) when `tool` is missing.
pub fn skip_without(tool: &str) -> bool {
    if find_executable(tool).is_none() {
        eprintln!("skipping test: `{tool}` is not installed");
        true
    } else {
        false
    }
}

/// Compile a C source with clang.  Returns false when clang is unavailable or
/// the build fails.
pub fn compile_c(dir: &Path, source: &str, name: &str) -> Option<std::path::PathBuf> {
    if skip_without("clang") {
        return None;
    }
    let src = dir.join(format!("{name}.c"));
    std::fs::write(&src, source).expect("write source");
    let bin = dir.join(name);
    let status = Command::new("clang")
        .args(["-g", "-O0", "-o"])
        .arg(&bin)
        .arg(&src)
        .status()
        .expect("run clang");
    assert!(status.success(), "clang failed for {name}");
    Some(bin)
}

/// Write a Python script.
pub fn write_script(dir: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write script");
    path
}

/// Collects every event and lets tests wait for a specific one.
pub struct EventLog {
    pub events: Vec<DebugEvent>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLog {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Wait until an event matches, keeping all non-matching events.
    pub fn wait<F>(
        &mut self,
        session: &DebugSession,
        timeout: Duration,
        label: &str,
        mut predicate: F,
    ) -> DebugEvent
    where
        F: FnMut(&DebugEvent) -> bool,
    {
        let start = Instant::now();
        loop {
            if let Some(index) = self.events.iter().position(&mut predicate) {
                return self.events.remove(index);
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                let seen: Vec<String> = self.events.iter().map(summarise).collect();
                panic!("timed out waiting for {label}; seen: {seen:#?}");
            }
            match session.recv_timeout(remaining.min(Duration::from_millis(250))) {
                Ok(event) => self.events.push(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let seen: Vec<String> = self.events.iter().map(summarise).collect();
                    panic!("session ended while waiting for {label}; seen: {seen:#?}");
                }
            }
        }
    }

    /// Wait for the next `StoppedAt` event.
    pub fn wait_stop(&mut self, session: &DebugSession, timeout: Duration) -> (Location, StopReason, String) {
        match self.wait(session, timeout, "stopped", |event| {
            matches!(event, DebugEvent::StoppedAt { .. })
        }) {
            DebugEvent::StoppedAt {
                location,
                reason,
                annotation,
            } => (location, reason, annotation),
            other => panic!("expected StoppedAt, got {other:?}"),
        }
    }

    /// Wait for `StoppedAt` and then for the variables refresh that follows.
    pub fn wait_stop_and_variables(
        &mut self,
        session: &DebugSession,
        timeout: Duration,
    ) -> (Location, StopReason, VariableTree) {
        let (location, reason, _) = self.wait_stop(session, timeout);
        let variables = self.wait_variables(session, timeout);
        (location, reason, variables)
    }

    /// Wait for the next `VariablesChanged` with no parent (a full refresh).
    pub fn wait_variables(&mut self, session: &DebugSession, timeout: Duration) -> VariableTree {
        match self.wait(session, timeout, "variables", |event| {
            matches!(event, DebugEvent::VariablesChanged { parent: None, .. })
        }) {
            DebugEvent::VariablesChanged { variables, .. } => variables,
            other => panic!("expected VariablesChanged, got {other:?}"),
        }
    }

    /// Wait for a lazy variable subtree.
    pub fn wait_subtree(
        &mut self,
        session: &DebugSession,
        parent: &VariableId,
        timeout: Duration,
    ) -> VariableTree {
        let parent = parent.clone();
        match self.wait(session, timeout, "variable subtree", |event| {
            matches!(
                event,
                DebugEvent::VariablesChanged {
                    parent: Some(id),
                    ..
                } if *id == parent
            )
        }) {
            DebugEvent::VariablesChanged { variables, .. } => variables,
            other => panic!("expected VariablesChanged, got {other:?}"),
        }
    }

    /// Wait for the program to exit and return the code.
    pub fn wait_exit(&mut self, session: &DebugSession, timeout: Duration) -> i32 {
        match self.wait(session, timeout, "exit", |event| {
            matches!(event, DebugEvent::Exited { .. })
        }) {
            DebugEvent::Exited { code } => code,
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    /// Collect all program output seen so far plus any that arrives within the
    /// grace period.
    pub fn program_output(&mut self, session: &DebugSession, grace: Duration) -> Vec<String> {
        let start = Instant::now();
        while start.elapsed() < grace {
            match session.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => self.events.push(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.events
            .iter()
            .filter_map(|event| match event {
                DebugEvent::Output { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Find a variable in a tree by display name.
    pub fn variable<'a>(tree: &'a VariableTree, name: &str) -> &'a Variable {
        tree.roots
            .iter()
            .find(|variable| variable.name == name || variable.display_name == name)
            .unwrap_or_else(|| {
                let names: Vec<&str> = tree.roots.iter().map(|v| v.name.as_str()).collect();
                panic!("no variable `{name}`; have {names:?}")
            })
    }
}

fn summarise(event: &DebugEvent) -> String {
    match event {
        DebugEvent::State(state) => format!("State({state:?})"),
        DebugEvent::Started => "Started".into(),
        DebugEvent::Output { text, .. } => format!("Output({text})"),
        DebugEvent::DebuggerOutput { text } => format!("DebuggerOutput({text})"),
        DebugEvent::LogLine { text, .. } => format!("Log({text})"),
        DebugEvent::StoppedAt {
            location,
            annotation,
            ..
        } => format!("StoppedAt({annotation}, {})", location.describe()),
        DebugEvent::StackChanged(stack) => format!("StackChanged({} frames)", stack.frames.len()),
        DebugEvent::VariablesChanged { variables, .. } => format!(
            "VariablesChanged({:?})",
            variables.roots.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        ),
        DebugEvent::BreakpointsChanged(list) => format!("BreakpointsChanged({})", list.len()),
        DebugEvent::WatchValues(values) => format!("WatchValues({values:?})"),
        DebugEvent::ExpressionResult { text, .. } => format!("ExpressionResult({text})"),
        DebugEvent::Exited { code } => format!("Exited({code})"),
        DebugEvent::Error { message } => format!("Error({message})"),
    }
}
