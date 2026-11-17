//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use scratch_language::{LanguageDef, LanguageRegistry};
use scratch_runner::{
    PathSetting, Phase, RunEvent, RunHandle, RunMessage, RunOptions, RunRequest, RunSummary,
    ShellSettings, Snapshot, Stream, augmented_path, home_dir,
};

/// Whether `tool` can be started (`--version` is tried; tools that do not know it
/// still count when they start).
pub fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Returns true (after printing why) when a test must be skipped.
pub fn skip_without(tools: &[&str]) -> bool {
    for tool in tools {
        if !have(tool) {
            eprintln!("SKIP: `{tool}` is not installed");
            return true;
        }
    }
    false
}

pub fn language(id: &str) -> LanguageDef {
    static REGISTRY: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    REGISTRY
        .get_or_init(LanguageRegistry::builtin)
        .get(id)
        .unwrap_or_else(|| panic!("no builtin language {id}"))
        .clone()
}

/// The test's shell settings: a fixed PATH (the process PATH plus the usual tool
/// directories) so that no login shell is started for every test.
pub fn shell() -> ShellSettings {
    let path = augmented_path(
        &std::env::var("PATH").unwrap_or_default(),
        home_dir().as_deref(),
    );
    ShellSettings {
        path: PathSetting::Custom(path),
        ..ShellSettings::default()
    }
}

/// Options with a private temp root, so tests can look at what is left behind.
pub fn options(root: &Path) -> RunOptions {
    RunOptions {
        temp_root: Some(root.to_path_buf()),
        ..RunOptions::default()
    }
}

pub fn request(id: &str, snapshot: Snapshot, root: &Path) -> RunRequest {
    RunRequest::new(&language(id), snapshot)
        .with_shell(shell())
        .with_options(options(root))
}

/// Everything a run reported.
pub struct Outcome {
    pub messages: Vec<RunMessage>,
    pub summary: RunSummary,
}

impl Outcome {
    pub fn events(&self) -> impl Iterator<Item = &RunEvent> {
        self.messages.iter().map(|m| &m.event)
    }

    fn collect(&self, phase: Phase, wanted: Stream) -> String {
        let mut bytes = Vec::new();
        for event in self.events() {
            if let RunEvent::Output {
                phase: p,
                stream,
                bytes: b,
            } = event
                && *p == phase
                && *stream == wanted
            {
                bytes.extend_from_slice(b);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn stdout(&self) -> String {
        self.collect(Phase::Run, Stream::Stdout)
    }

    pub fn stderr(&self) -> String {
        self.collect(Phase::Run, Stream::Stderr)
    }

    pub fn compile_stderr(&self) -> String {
        self.collect(Phase::Compile, Stream::Stderr)
    }

    pub fn started_pid(&self) -> Option<u32> {
        self.events().find_map(|e| match e {
            RunEvent::Started(info) => info.pid,
            _ => None,
        })
    }

    pub fn has(&self, f: impl Fn(&RunEvent) -> bool) -> bool {
        self.events().any(f)
    }
}

/// Waits for `Finished`, checking the message protocol on the way: sequence
/// numbers count up, one run id, `Preparing` first, `Finished` last.
pub fn wait(rx: &Receiver<RunMessage>) -> Outcome {
    let mut messages: Vec<RunMessage> = Vec::new();
    loop {
        let message = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the run did not finish in time");
        let finished = matches!(message.event, RunEvent::Finished(_));
        messages.push(message);
        if finished {
            break;
        }
    }
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(message.seq, index as u64, "sequence numbers");
        assert_eq!(message.run, messages[0].run, "one run id");
    }
    assert!(matches!(messages[0].event, RunEvent::Preparing));
    let Some(RunMessage {
        event: RunEvent::Finished(summary),
        ..
    }) = messages.last().cloned()
    else {
        unreachable!()
    };
    assert_eq!(summary.run, messages[0].run);
    // Nothing may follow Finished.
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    Outcome { messages, summary }
}

/// Runs `request` to its end.
pub fn run(request: RunRequest) -> Outcome {
    let (_handle, rx) = scratch_runner::start_channel(request).expect("start");
    wait(&rx)
}

/// Runs `request`, calling `act` with the handle and the receiver as soon as the
/// program has started; `act` may drive the run (write, cancel...).
pub fn run_with(
    request: RunRequest,
    act: impl FnOnce(&RunHandle, &Receiver<RunMessage>),
) -> Outcome {
    let (handle, rx) = scratch_runner::start_channel(request).expect("start");
    // Peek until Started, keeping the messages seen.
    let mut early: Vec<RunMessage> = Vec::new();
    loop {
        let message = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("no Started event");
        let started = matches!(message.event, RunEvent::Started(_));
        let finished = matches!(message.event, RunEvent::Finished(_));
        early.push(message);
        if started || finished {
            break;
        }
    }
    act(&handle, &rx);
    let mut rest = wait_rest(&rx);
    early.append(&mut rest);
    let summary = match early.last().map(|m| m.event.clone()) {
        Some(RunEvent::Finished(summary)) => summary,
        other => panic!("run did not finish: {other:?}"),
    };
    Outcome {
        messages: early,
        summary,
    }
}

/// Reads messages up to and including `Finished`.
pub fn wait_rest(rx: &Receiver<RunMessage>) -> Vec<RunMessage> {
    let mut messages = Vec::new();
    loop {
        let message = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the run did not finish in time");
        let finished = matches!(message.event, RunEvent::Finished(_));
        messages.push(message);
        if finished {
            return messages;
        }
    }
}

/// Reads output until `needle` appears in the program's stdout or the time is up.
pub fn read_until(
    rx: &Receiver<RunMessage>,
    needle: &str,
    limit: Duration,
) -> (bool, String, Vec<RunMessage>) {
    let deadline = std::time::Instant::now() + limit;
    let mut text = String::new();
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        if let Ok(message) = rx.recv_timeout(Duration::from_millis(100)) {
            {
                if let RunEvent::Output {
                    phase: Phase::Run,
                    stream: Stream::Stdout,
                    bytes,
                } = &message.event
                {
                    text.push_str(&String::from_utf8_lossy(bytes));
                }
                let finished = matches!(message.event, RunEvent::Finished(_));
                seen.push(message);
                if text.contains(needle) {
                    return (true, text, seen);
                }
                if finished {
                    return (false, text, seen);
                }
            }
        }
    }
    (false, text, seen)
}

/// Whether a process with this pid exists.
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc_kill(pid, 0) == 0 }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}
