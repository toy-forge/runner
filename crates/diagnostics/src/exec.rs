//! Process execution for diagnostics runs: cancelable, with a timeout, with
//! stdout and stderr merged in order (or kept apart), in its own process
//! group so a cancel kills the whole tree.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A shared cancel flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, not-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The environment a diagnostics process runs in. The caller decides (the
/// app builds it from the Shell/Advanced preferences: custom `PATH`,
/// environment variables, login shell `PATH`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerEnv {
    /// Environment variables (the process environment is *not* inherited
    /// beyond this).
    pub vars: BTreeMap<String, String>,
    /// The `PATH` value; empty = the `PATH` variable of `vars`.
    pub path: String,
}

impl RunnerEnv {
    /// This process's environment and `PATH`.
    pub fn from_process() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let path = vars.get("PATH").cloned().unwrap_or_default();
        Self { vars, path }
    }

    /// Sets the `PATH`.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Adds or replaces a variable.
    #[must_use]
    pub fn with_var(mut self, name: &str, value: &str) -> Self {
        self.vars.insert(name.to_owned(), value.to_owned());
        self
    }

    /// The effective `PATH` with directories prepended and appended.
    pub fn path_with(&self, prepend: &[String], append: &[String]) -> String {
        let base = if self.path.is_empty() {
            self.vars.get("PATH").cloned().unwrap_or_default()
        } else {
            self.path.clone()
        };
        let mut parts: Vec<&str> = prepend.iter().map(String::as_str).collect();
        if !base.is_empty() {
            parts.push(&base);
        }
        parts.extend(append.iter().map(String::as_str));
        parts.join(":")
    }
}

/// What to execute.
#[derive(Debug, Clone)]
pub struct ExecSpec {
    /// Program and arguments (the program is looked up on `path`).
    pub argv: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Environment variables (nothing else is inherited).
    pub env: BTreeMap<String, String>,
    /// `PATH` value, also used to find the program.
    pub path: String,
    /// Bytes written to stdin (then closed); `None` = closed stdin.
    pub stdin: Option<Vec<u8>>,
    /// Interleave stderr into stdout.
    pub merge_stderr: bool,
    /// Kill the process group after this long.
    pub timeout: Option<Duration>,
    /// Keep at most this many output bytes (the rest is drained and dropped).
    pub max_output: usize,
}

impl ExecSpec {
    /// A spec with merged output, no stdin, no timeout, 8 MiB output limit.
    pub fn new(argv: Vec<String>, cwd: &Path) -> Self {
        Self {
            argv,
            cwd: cwd.to_path_buf(),
            env: BTreeMap::new(),
            path: String::new(),
            stdin: None,
            merge_stderr: true,
            timeout: None,
            max_output: 8 * 1024 * 1024,
        }
    }
}

/// A finished process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    /// Stdout (and stderr when merged), lossily decoded.
    pub stdout: String,
    /// Stderr when not merged.
    pub stderr: String,
    /// Exit code; `None` when killed by a signal.
    pub code: Option<i32>,
    /// Terminating signal.
    pub signal: Option<i32>,
}

/// Why an execution did not produce output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    /// The program is not on `PATH`.
    #[error("{0} not found")]
    NotFound(String),
    /// Spawning or I/O failed.
    #[error("{0}")]
    Io(String),
    /// The timeout elapsed; the process group was killed.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// Cancelled; the process group was killed.
    #[error("cancelled")]
    Cancelled,
}

/// Finds `program` on the colon separated `path` (or checks it when it
/// contains a slash). Only regular files with an execute bit count.
pub fn find_program(program: &str, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let p = PathBuf::from(program);
        return is_exec(&p).then_some(p);
    }
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(program))
        .find(|c| is_exec(c))
}

struct Collector {
    buf: Mutex<Vec<u8>>,
    done: AtomicBool,
}

fn collect(mut reader: impl Read + Send + 'static, limit: usize) -> Arc<Collector> {
    let shared = Arc::new(Collector {
        buf: Mutex::new(Vec::new()),
        done: AtomicBool::new(false),
    });
    let sink = Arc::clone(&shared);
    thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = sink.buf.lock() {
                        let room = limit.saturating_sub(b.len());
                        b.extend_from_slice(&chunk[..n.min(room)]);
                    }
                }
            }
        }
        sink.done.store(true, Ordering::SeqCst);
    });
    shared
}

fn take(c: &Collector, wait: Duration) -> String {
    let deadline = Instant::now() + wait;
    while !c.done.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    let bytes = c.buf.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn kill_group(pid: u32) {
    // SAFETY: plain syscall on a pid we spawned into its own process group.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Runs a process to completion.
///
/// # Errors
///
/// [`ExecError`]: program missing, I/O failure, timeout, cancellation.
pub fn execute(spec: &ExecSpec, cancel: &CancelToken) -> Result<ExecOutput, ExecError> {
    let program = spec
        .argv
        .first()
        .ok_or_else(|| ExecError::Io("empty command".to_owned()))?;
    if cancel.is_cancelled() {
        return Err(ExecError::Cancelled);
    }
    let resolved = find_program(program, &spec.path)
        .ok_or_else(|| ExecError::NotFound(program.clone()))?;
    let mut cmd = Command::new(resolved);
    cmd.args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .env("PATH", &spec.path)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .process_group(0);

    let io = |e: std::io::Error| ExecError::Io(e.to_string());
    let (out, err) = if spec.merge_stderr {
        let (reader, writer) = std::io::pipe().map_err(io)?;
        cmd.stdout(writer.try_clone().map_err(io)?).stderr(writer);
        (collect(reader, spec.max_output), None)
    } else {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        // Pipes are collected after spawn below.
        let placeholder = Arc::new(Collector {
            buf: Mutex::new(Vec::new()),
            done: AtomicBool::new(true),
        });
        (placeholder, None::<Arc<Collector>>)
    };
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ExecError::NotFound(program.clone())
        } else {
            ExecError::Io(e.to_string())
        }
    })?;
    drop(cmd); // closes the parent's copy of the write end

    let (out, err) = if spec.merge_stderr {
        (out, err)
    } else {
        let o = child.stdout.take().map(|s| collect(s, spec.max_output));
        let e = child.stderr.take().map(|s| collect(s, spec.max_output));
        (
            o.unwrap_or_else(|| {
                Arc::new(Collector {
                    buf: Mutex::new(Vec::new()),
                    done: AtomicBool::new(true),
                })
            }),
            e,
        )
    };
    if let (Some(data), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }

    let started = Instant::now();
    let pid = child.id();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_group(pid);
                return Err(ExecError::Io(e.to_string()));
            }
        }
        if cancel.is_cancelled() {
            kill_group(pid);
            let _ = child.wait();
            return Err(ExecError::Cancelled);
        }
        if let Some(t) = spec.timeout
            && started.elapsed() >= t
        {
            kill_group(pid);
            let _ = child.wait();
            return Err(ExecError::Timeout(t));
        }
        thread::sleep(Duration::from_millis(4));
    };
    // Background grandchildren may keep the pipe open: do not wait forever.
    let stdout = take(&out, Duration::from_millis(400));
    let stderr = err.map(|e| take(&e, Duration::from_millis(400))).unwrap_or_default();
    Ok(ExecOutput {
        stdout,
        stderr,
        code: status.code(),
        signal: status.signal(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(script: &str) -> ExecSpec {
        let mut s = ExecSpec::new(
            vec!["/bin/sh".into(), "-c".into(), script.into()],
            Path::new("/"),
        );
        s.path = "/usr/bin:/bin".into();
        s
    }

    #[test]
    fn merged_output_keeps_order_and_exit_code() {
        let out = execute(&spec("echo one; echo two >&2; echo three; exit 3"), &CancelToken::new()).unwrap();
        assert_eq!(out.stdout, "one\ntwo\nthree\n");
        assert_eq!(out.code, Some(3));
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn separate_streams_and_stdin() {
        let mut s = spec("cat; echo err >&2");
        s.merge_stderr = false;
        s.stdin = Some(b"hello".to_vec());
        let out = execute(&s, &CancelToken::new()).unwrap();
        assert_eq!(out.stdout, "hello");
        assert_eq!(out.stderr, "err\n");
    }

    #[test]
    fn env_is_not_inherited() {
        let mut s = spec("echo [$HOME][$FOO]");
        s.env.insert("FOO".into(), "bar".into());
        let out = execute(&s, &CancelToken::new()).unwrap();
        assert_eq!(out.stdout, "[][bar]\n");
    }

    #[test]
    fn timeout_kills_the_group() {
        let mut s = spec("sleep 30 & sleep 30");
        s.timeout = Some(Duration::from_millis(150));
        let started = Instant::now();
        assert_eq!(execute(&s, &CancelToken::new()), Err(ExecError::Timeout(Duration::from_millis(150))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancel_from_another_thread() {
        let token = CancelToken::new();
        let t2 = token.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            t2.cancel();
        });
        let started = Instant::now();
        assert_eq!(execute(&spec("sleep 30"), &token), Err(ExecError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        handle.join().unwrap();
        assert_eq!(execute(&spec("true"), &token), Err(ExecError::Cancelled), "already cancelled");
    }

    #[test]
    fn missing_program() {
        let mut s = spec("true");
        s.argv = vec!["definitely-not-a-program-xyz".into()];
        assert_eq!(execute(&s, &CancelToken::new()), Err(ExecError::NotFound("definitely-not-a-program-xyz".into())));
    }

    #[test]
    fn background_child_does_not_hang_collection() {
        let started = Instant::now();
        let out = execute(&spec("(sleep 3 &) ; echo done"), &CancelToken::new()).unwrap();
        assert_eq!(out.stdout, "done\n");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn path_composition() {
        let env = RunnerEnv::default().with_path("/usr/bin");
        assert_eq!(env.path_with(&["/a".into()], &["/z".into()]), "/a:/usr/bin:/z");
        assert!(find_program("sh", "/usr/bin:/bin").is_some());
        assert!(find_program("/bin/sh", "").is_some());
        assert!(find_program("nope-xyz", "/bin").is_none());
    }
}
