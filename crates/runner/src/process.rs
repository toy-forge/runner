//! Process spawning with a pty for stdout, a separate pipe for stderr, a
//! non-blocking input channel and exact exit statistics.
//!
//! This is the low-level building block of the engine, and the one the
//! debugger crate uses for its own processes (the program's console pty, the
//! debugger client's hidden pty):
//!
//! ```no_run
//! use scratch_runner::process::{Process, ProcessConfig, ProcessEvent};
//! let (tx, rx) = std::sync::mpsc::channel();
//! let mut config = ProcessConfig::pty(["python3", "-c", "print(input())"]);
//! config.env.insert("PATH".into(), "/usr/bin:/bin".into());
//! let process = Process::spawn(config, move |event| { let _ = tx.send(event); })?;
//! process.write(b"hello\n")?;
//! while let Ok(event) = rx.recv() {
//!     match event {
//!         ProcessEvent::Stdout(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
//!         ProcessEvent::Stderr(bytes) => eprint!("{}", String::from_utf8_lossy(&bytes)),
//!         ProcessEvent::Exited(exit) => { println!("{exit:?}"); break; }
//!     }
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Behaviour
//!
//! * In pty mode the child gets the pty slave as stdout (and as stdin unless
//!   [`StdinSource`] says otherwise), is a session leader with the pty as its
//!   controlling terminal (so `isatty` is true, Ctrl-C is `\x03`, EOF is `\x04`
//!   at the start of a line), and its process group id is its pid
//!   ([`Process::signal`] signals the whole group).
//! * The line discipline echoes typed input and (with [`Tty::onlcr`]) turns
//!   `\n` into `\r\n` on output. Stderr is a plain pipe by default: exact
//!   stream tags, no echo, no CRLF.
//! * [`ProcessEvent::Exited`] is the last event. It is sent after the child was
//!   reaped with `wait4` and the output was drained (until both streams end, or
//!   until 300 ms pass without new output when a surviving grandchild keeps the
//!   pty open).
//! * The sink runs on reader threads: keep it cheap (send into a channel).
//! * [`Process::write`] never blocks the caller: bytes go through a queue to a
//!   writer thread.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long output may stay silent after the child exited before the readers
/// give up on streams that a surviving descendant still holds open.
const DRAIN_IDLE: Duration = Duration::from_millis(300);
/// How often a reader thread looks at its stop flag.
const POLL_MS: i32 = 40;
const READ_CHUNK: usize = 32 * 1024;

/// The terminal a program sees on stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tty {
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
    /// Echo typed input back (the line discipline's `ECHO`). Turn it off when the
    /// console shows typed text itself, and for machine-driven ptys (a debugger
    /// client).
    pub echo: bool,
    /// Turn `\n` into `\r\n` on output (`ONLCR`). A terminal emulator wants
    /// this; a line-oriented console does not.
    pub onlcr: bool,
}

impl Default for Tty {
    fn default() -> Self {
        Self {
            cols: 100,
            rows: 30,
            echo: true,
            onlcr: true,
        }
    }
}

/// Where the program's standard input comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StdinSource {
    /// The pty (interactive: [`Process::write`] is what the program reads).
    /// Without a pty this is the same as `Null`.
    #[default]
    Tty,
    /// A pipe of its own that [`Process::write`] feeds and
    /// [`Process::close_stdin`] closes (no pty needed).
    Pipe,
    /// A redirect from this file (`< file`: the Program Input of a run).
    File(PathBuf),
    /// `/dev/null`.
    Null,
}

/// How to start a process.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    /// The program and its arguments. `argv[0]` is executed as given (resolve
    /// it against `PATH` first, see [`crate::find_program`]).
    pub argv: Vec<String>,
    /// The working directory (`None` = inherit).
    pub cwd: Option<PathBuf>,
    /// The environment of the child.
    pub env: BTreeMap<String, String>,
    /// Keep the parent's environment and apply `env` on top. By default the
    /// child gets exactly `env`.
    pub inherit_env: bool,
    /// Standard input.
    pub stdin: StdinSource,
    /// Put stdout on a pty (`Some`) or on a pipe (`None`).
    pub tty: Option<Tty>,
    /// With a pty: put stderr on the pty too (merged with stdout) instead of
    /// its own pipe.
    pub merge_stderr: bool,
}

impl ProcessConfig {
    /// A program on a pty with the default [`Tty`], interactive stdin and a
    /// separate stderr pipe.
    pub fn pty<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            env: BTreeMap::new(),
            inherit_env: false,
            stdin: StdinSource::Tty,
            tty: Some(Tty::default()),
            merge_stderr: false,
        }
    }

    /// A program on pipes (stdout and stderr), with `/dev/null` as stdin. Used
    /// for compile stages.
    pub fn piped<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            env: BTreeMap::new(),
            inherit_env: false,
            stdin: StdinSource::Null,
            tty: None,
            merge_stderr: false,
        }
    }
}

/// How the process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    /// The exit status, when it exited normally.
    pub code: Option<i32>,
    /// The signal that killed it, if it was killed.
    pub signal: Option<i32>,
    /// Wall-clock time from the start.
    pub wall: Duration,
    /// User CPU time.
    pub cpu_user: Duration,
    /// System CPU time.
    pub cpu_sys: Duration,
    /// Peak resident memory in bytes.
    pub max_rss: u64,
}

/// What a running process produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    /// Bytes from the pty (or the stdout pipe).
    Stdout(Vec<u8>),
    /// Bytes from the stderr pipe.
    Stderr(Vec<u8>),
    /// The process ended (always the last event).
    Exited(ProcessExit),
}

type Sink = Arc<dyn Fn(ProcessEvent) + Send + Sync>;

/// A running process.
#[derive(Debug)]
pub struct Process {
    pid: i32,
    master: Option<OwnedFd>,
    writer: Mutex<Option<Sender<Vec<u8>>>>,
    stop_readers: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

fn cvt(result: i32) -> io::Result<i32> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

/// Opens a pty pair configured by `tty`.
fn open_pty(tty: &Tty) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut size = libc::winsize {
        ws_row: tty.rows,
        ws_col: tty.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: valid out-pointers; null termios and name are allowed; `size`
    // outlives the call.
    cvt(unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    })?;
    // SAFETY: openpty returned two fresh descriptors that we now own.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // SAFETY: tcgetattr fills a zeroed termios for a valid fd; tcsetattr applies it.
    unsafe {
        let mut attrs: libc::termios = std::mem::zeroed();
        cvt(libc::tcgetattr(master.as_raw_fd(), &mut attrs))?;
        if !tty.onlcr {
            attrs.c_oflag &= !libc::ONLCR;
        }
        if !tty.echo {
            attrs.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
        }
        cvt(libc::tcsetattr(master.as_raw_fd(), libc::TCSANOW, &attrs))?;
    }
    // The master must not leak into the child (or into other processes).
    // SAFETY: plain fcntl on an fd we own.
    cvt(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    Ok((master, slave))
}

/// Waits until `fd` has data or hung up. `Ok(false)` on timeout or EINTR.
fn wait_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let result = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
    if result < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(result > 0)
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    name: &str,
    mut source: File,
    stdout: bool,
    sink: Sink,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    activity: Arc<AtomicU64>,
    epoch: Instant,
    paused: Arc<AtomicBool>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut buffer = vec![0u8; READ_CHUNK];
            while !stop.load(Ordering::Relaxed) {
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                match wait_readable(source.as_raw_fd(), POLL_MS) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(_) => break,
                }
                match source.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        activity.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                        let bytes = buffer[..read].to_vec();
                        sink(if stdout {
                            ProcessEvent::Stdout(bytes)
                        } else {
                            ProcessEvent::Stderr(bytes)
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    // EIO once the slave side is closed.
                    Err(_) => break,
                }
            }
            done.fetch_add(1, Ordering::SeqCst);
        })?;
    Ok(())
}

impl Process {
    /// Starts the process; `sink` receives its output on helper threads and ends
    /// with [`ProcessEvent::Exited`].
    ///
    /// # Errors
    ///
    /// The `io::Error` of the failed pty, pipe, fork or exec (`NotFound`,
    /// `PermissionDenied`, ...). Nothing is left running in that case.
    pub fn spawn(
        config: ProcessConfig,
        sink: impl Fn(ProcessEvent) + Send + Sync + 'static,
    ) -> io::Result<Process> {
        let sink: Sink = Arc::new(sink);
        let (program, args) = config
            .argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty argv"))?;
        let mut command = Command::new(program);
        command.args(args);
        if !config.inherit_env {
            command.env_clear();
        }
        command.envs(&config.env);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }

        let mut master: Option<OwnedFd> = None;
        if let Some(tty) = &config.tty {
            let (m, slave) = open_pty(tty)?;
            command.stdout(Stdio::from(slave.try_clone()?));
            command.stdin(match &config.stdin {
                StdinSource::Tty => Stdio::from(slave.try_clone()?),
                StdinSource::File(path) => Stdio::from(File::open(path)?),
                StdinSource::Pipe => Stdio::piped(),
                StdinSource::Null => Stdio::null(),
            });
            if config.merge_stderr {
                command.stderr(Stdio::from(slave.try_clone()?));
            } else {
                command.stderr(Stdio::piped());
            }
            // SAFETY: only async-signal-safe calls (setsid, ioctl) between fork
            // and exec. Fd 1 is the pty slave in every pty configuration.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            drop(slave);
            master = Some(m);
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            command.stdin(match &config.stdin {
                StdinSource::Pipe => Stdio::piped(),
                StdinSource::File(path) => Stdio::from(File::open(path)?),
                StdinSource::Tty | StdinSource::Null => Stdio::null(),
            });
            command.process_group(0);
        }

        let started = Instant::now();
        let mut child = command.spawn()?;
        // The parent must not keep the slave: the master never sees EOF otherwise.
        drop(command);
        let pid = child.id() as i32;

        let done = Arc::new(AtomicUsize::new(0));
        let activity = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let mut readers = 0usize;

        let mut writer_file: Option<File> = None;
        if let Some(master) = &master {
            let reader = File::from(master.try_clone()?);
            spawn_reader(
                "run-stdout",
                reader,
                true,
                sink.clone(),
                stop.clone(),
                done.clone(),
                activity.clone(),
                started,
                paused.clone(),
            )?;
            readers += 1;
            if config.stdin == StdinSource::Tty {
                writer_file = Some(File::from(master.try_clone()?));
            }
        } else if let Some(stdout) = child.stdout.take() {
            spawn_reader(
                "run-stdout",
                File::from(OwnedFd::from(stdout)),
                true,
                sink.clone(),
                stop.clone(),
                done.clone(),
                activity.clone(),
                started,
                paused.clone(),
            )?;
            readers += 1;
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(
                "run-stderr",
                File::from(OwnedFd::from(stderr)),
                false,
                sink.clone(),
                stop.clone(),
                done.clone(),
                activity.clone(),
                started,
                paused.clone(),
            )?;
            readers += 1;
        }
        if let Some(stdin) = child.stdin.take() {
            writer_file = Some(File::from(OwnedFd::from(stdin)));
        }

        let writer = match writer_file {
            Some(mut file) => {
                let (tx, rx) = mpsc::channel::<Vec<u8>>();
                std::thread::Builder::new()
                    .name("run-stdin".into())
                    .spawn(move || {
                        for bytes in rx {
                            if file.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                    })?;
                Some(tx)
            }
            None => None,
        };

        let wait_sink = sink;
        let wait_stop = stop.clone();
        let wait_paused = paused.clone();
        std::thread::Builder::new()
            .name("run-wait".into())
            .spawn(move || {
                let mut status = 0;
                // SAFETY: a zeroed rusage is a valid out-value.
                let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
                loop {
                    // SAFETY: valid pointers; `pid` is our child.
                    let result = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
                    if result >= 0
                        || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
                    {
                        break;
                    }
                }
                let wall = started.elapsed();
                drop(child); // already reaped; dropping does not wait
                wait_paused.store(false, Ordering::SeqCst);
                // Drain what is buffered. A surviving descendant may hold the pty
                // open, so stop after a quiet period instead of waiting for EOF.
                let exited_at = wall.as_millis() as u64;
                loop {
                    if done.load(Ordering::SeqCst) >= readers {
                        break;
                    }
                    let last = activity.load(Ordering::Relaxed).max(exited_at);
                    let now = started.elapsed().as_millis() as u64;
                    if now.saturating_sub(last) >= DRAIN_IDLE.as_millis() as u64 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                wait_stop.store(true, Ordering::SeqCst);
                let timeval = |t: libc::timeval| {
                    Duration::new(t.tv_sec.max(0) as u64, (t.tv_usec.max(0) as u32) * 1000)
                };
                wait_sink(ProcessEvent::Exited(ProcessExit {
                    code: libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)),
                    signal: libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)),
                    wall,
                    cpu_user: timeval(usage.ru_utime),
                    cpu_sys: timeval(usage.ru_stime),
                    // macOS reports bytes, other systems kilobytes.
                    max_rss: if cfg!(target_os = "macos") {
                        usage.ru_maxrss.max(0) as u64
                    } else {
                        usage.ru_maxrss.max(0) as u64 * 1024
                    },
                }));
            })?;

        Ok(Process {
            pid,
            master,
            writer: Mutex::new(writer),
            stop_readers: stop,
            paused,
        })
    }

    /// The child's pid (also its process group id).
    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    /// Whether stdout is a pty.
    pub fn has_tty(&self) -> bool {
        self.master.is_some()
    }

    /// Queues bytes for the program's stdin. Through a pty they pass the line
    /// discipline: `\n` ends a line, `\x04` at the start of a line is EOF, `\x03`
    /// is Ctrl-C. Returns at once; a program that does not read cannot block the
    /// caller.
    ///
    /// # Errors
    ///
    /// `BrokenPipe` when the process has no input channel (stdin is a file or
    /// `/dev/null`) or its input was closed.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let guard = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("input lock poisoned"))?;
        match guard.as_ref() {
            Some(sender) => sender
                .send(bytes.to_vec())
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe)),
            None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        }
    }

    /// Sends end-of-file: Ctrl-D through a pty (it works at the start of a line),
    /// or closes the pipe for [`StdinSource::Pipe`].
    pub fn send_eof(&self) -> io::Result<()> {
        if self.master.is_some() {
            self.write(&[0x04])
        } else {
            self.close_stdin();
            Ok(())
        }
    }

    /// Closes the input channel (pipe stdin: the program sees EOF). Queued bytes
    /// are still delivered first.
    pub fn close_stdin(&self) {
        if let Ok(mut guard) = self.writer.lock() {
            guard.take();
        }
    }

    /// Tells the program the terminal size changed (the pty's window size, and
    /// SIGWINCH). No effect without a pty.
    ///
    /// # Errors
    ///
    /// The `ioctl` error.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let Some(master) = &self.master else {
            return Ok(());
        };
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the master fd is open as long as `self` lives; valid winsize pointer.
        cvt(unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) }).map(|_| ())
    }

    /// Stops (or resumes) reading the program's output. While paused the pty and
    /// pipe buffers fill up and the program blocks in its next `write`: natural
    /// back-pressure, used to hold a flood while the user decides what to do.
    pub fn pause_output(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// Sends `signal` to the child's whole process group.
    ///
    /// # Errors
    ///
    /// `ESRCH` when the group no longer exists.
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: killpg with the group this process created (setsid / process_group).
        cvt(unsafe { libc::killpg(self.pid, signal) }).map(|_| ())
    }

    /// SIGTERM now, SIGKILL to the whole group after `grace` (also reaching
    /// descendants that survived the leader).
    pub fn stop(&self, grace: Duration) {
        let _ = self.signal(libc::SIGTERM);
        let pid = self.pid;
        let _ = std::thread::Builder::new()
            .name("run-kill".into())
            .spawn(move || {
                std::thread::sleep(grace);
                // SAFETY: signalling a (possibly gone) group; ESRCH is ignored.
                unsafe { libc::killpg(pid, libc::SIGKILL) };
            });
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Readers stop at their next poll; the master closes with `self.master`.
        self.stop_readers.store(true, Ordering::SeqCst);
    }
}

/// A pty without a child process: the console of a debug run.
///
/// A debugger launches the program itself (`lldb`: `process launch -i <tty> -o
/// <tty> -e <tty>`), so the app must own the pty and hand its slave path to the
/// debugger. Output written to the slave arrives at the sink; bytes from
/// [`PtyConsole::write`] are what the program reads (through the line
/// discipline, like [`Process::write`]).
///
/// ```no_run
/// use scratch_runner::process::{PtyConsole, Tty};
/// let console = PtyConsole::open(&Tty::default(), |bytes| print!("{}", String::from_utf8_lossy(&bytes)))?;
/// println!("launch the program with -i {0} -o {0}", console.slave_path().display());
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct PtyConsole {
    master: OwnedFd,
    _slave: OwnedFd,
    path: PathBuf,
    writer: Mutex<Option<Sender<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
}

impl PtyConsole {
    /// Opens a pty configured by `tty`; `sink` receives what is written to it, on
    /// a reader thread, until the console is dropped.
    ///
    /// # Errors
    ///
    /// The `io::Error` of `openpty`, `ttyname_r` or thread creation.
    pub fn open(tty: &Tty, sink: impl Fn(Vec<u8>) + Send + Sync + 'static) -> io::Result<Self> {
        let (master, slave) = open_pty(tty)?;
        let mut name = [0 as libc::c_char; 256];
        // SAFETY: `name` is a valid writable buffer of the stated length.
        let result = unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        // SAFETY: ttyname_r wrote a NUL-terminated string.
        let path = PathBuf::from(
            unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let sink: Sink = Arc::new(move |event| {
            if let ProcessEvent::Stdout(bytes) = event {
                sink(bytes);
            }
        });
        spawn_reader(
            "pty-console",
            File::from(master.try_clone()?),
            true,
            sink,
            stop.clone(),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicU64::new(0)),
            Instant::now(),
            Arc::new(AtomicBool::new(false)),
        )?;
        let mut file = File::from(master.try_clone()?);
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("pty-console-stdin".into())
            .spawn(move || {
                for bytes in rx {
                    if file.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            master,
            _slave: slave,
            path,
            writer: Mutex::new(Some(tx)),
            stop,
        })
    }

    /// The path of the slave side (`/dev/ttys004`) to give to the debugger.
    pub fn slave_path(&self) -> &std::path::Path {
        &self.path
    }

    /// Sends what the user typed to whatever reads the slave. Never blocks.
    ///
    /// # Errors
    ///
    /// `BrokenPipe` after the console was closed.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let guard = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("input lock poisoned"))?;
        match guard.as_ref() {
            Some(sender) => sender
                .send(bytes.to_vec())
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe)),
            None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        }
    }

    /// Changes the window size (SIGWINCH for the foreground program).
    ///
    /// # Errors
    ///
    /// The `ioctl` error.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the master fd is open while `self` lives; valid winsize pointer.
        cvt(unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) })
            .map(|_| ())
    }
}

impl Drop for PtyConsole {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn have(tool: &str) -> bool {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    struct Done {
        out: Vec<u8>,
        err: Vec<u8>,
        exit: ProcessExit,
    }

    fn base_env() -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert(
            "PATH".to_owned(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        );
        env.insert("TERM".to_owned(), "dumb".to_owned());
        env
    }

    fn collect(rx: &mpsc::Receiver<ProcessEvent>) -> Done {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).expect("event") {
                ProcessEvent::Stdout(bytes) => out.extend(bytes),
                ProcessEvent::Stderr(bytes) => err.extend(bytes),
                ProcessEvent::Exited(exit) => return Done { out, err, exit },
            }
        }
    }

    fn drive(mut config: ProcessConfig, input: &[(u64, &[u8])]) -> Done {
        config.env = base_env();
        let (tx, rx) = channel();
        let process = Process::spawn(config, move |event| {
            let _ = tx.send(event);
        })
        .expect("spawn");
        for (delay, bytes) in input {
            std::thread::sleep(Duration::from_millis(*delay));
            process.write(bytes).expect("write");
        }
        collect(&rx)
    }

    #[test]
    fn python_echo_reply_and_separate_stderr() {
        if !have("python3") {
            eprintln!("SKIP: python3 not installed");
            return;
        }
        let config = ProcessConfig::pty([
            "python3",
            "-c",
            "print(input()); import sys; print('err', file=sys.stderr)",
        ]);
        let done = drive(config, &[(400, b"hello\n")]);
        let out = String::from_utf8_lossy(&done.out);
        assert!(out.contains("hello\r\nhello\r\n"), "stdout was {out:?}");
        assert_eq!(String::from_utf8_lossy(&done.err).trim(), "err");
        assert_eq!(done.exit.code, Some(0));
        assert!(done.exit.max_rss > 0);
    }

    #[test]
    fn echo_can_be_turned_off_and_onlcr_too() {
        if !have("python3") {
            eprintln!("SKIP: python3 not installed");
            return;
        }
        let mut config = ProcessConfig::pty(["python3", "-c", "print(input())"]);
        config.tty = Some(Tty {
            echo: false,
            onlcr: false,
            ..Tty::default()
        });
        let done = drive(config, &[(400, b"hello\n")]);
        assert_eq!(String::from_utf8_lossy(&done.out), "hello\n");
    }

    #[test]
    fn isatty_and_eof_through_ctrl_d() {
        if !have("python3") {
            eprintln!("SKIP: python3 not installed");
            return;
        }
        let config = ProcessConfig::pty([
            "python3",
            "-c",
            "import sys; print(sys.stdin.isatty(), sys.stdout.isatty(), sys.stderr.isatty()); print(repr(sys.stdin.read()))",
        ]);
        let done = drive(config, &[(400, b"ab\n"), (100, b"\x04")]);
        let out = String::from_utf8_lossy(&done.out);
        assert!(out.contains("True True False"), "{out:?}");
        assert!(out.contains("'ab\\n'"), "{out:?}");
    }

    #[test]
    fn stdin_from_a_file_is_not_a_tty_but_stdout_is() {
        if !have("python3") {
            eprintln!("SKIP: python3 not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.txt");
        std::fs::write(&input, "line one\nline two\n").unwrap();
        let mut config = ProcessConfig::pty([
            "python3",
            "-c",
            "import sys; print(sys.stdin.isatty(), sys.stdout.isatty()); print(sys.stdin.read().upper(), end='')",
        ]);
        config.stdin = StdinSource::File(input);
        config.tty.as_mut().unwrap().onlcr = false;
        let done = drive(config, &[]);
        assert_eq!(
            String::from_utf8_lossy(&done.out),
            "False True\nLINE ONE\nLINE TWO\n"
        );
    }

    #[test]
    fn pipe_mode_tags_streams_and_pipe_stdin_closes() {
        let config = ProcessConfig {
            stdin: StdinSource::Pipe,
            ..ProcessConfig::piped(["sh", "-c", "cat; echo err 1>&2"])
        };
        let mut config = config;
        config.env = base_env();
        let (tx, rx) = channel();
        let process = Process::spawn(config, move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        assert!(!process.has_tty());
        process.write(b"piped\n").unwrap();
        process.close_stdin();
        let done = collect(&rx);
        assert_eq!(done.out, b"piped\n");
        assert_eq!(done.err, b"err\n");
        assert!(process.write(b"late").is_err());
    }

    #[test]
    fn exit_codes_and_signals_are_reported() {
        assert_eq!(
            drive(ProcessConfig::pty(["sh", "-c", "exit 3"]), &[])
                .exit
                .code,
            Some(3)
        );
        let mut config = ProcessConfig::pty(["sleep", "30"]);
        config.env = base_env();
        let (tx, rx) = channel();
        let process = Process::spawn(config, move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        process.stop(Duration::from_millis(500));
        let done = collect(&rx);
        assert_eq!(done.exit.signal, Some(libc::SIGTERM));
        assert_eq!(done.exit.code, None);
    }

    #[test]
    fn resize_reaches_the_program() {
        let config = ProcessConfig::pty(["sh", "-c", "sleep 0.4; stty size"]);
        let mut config = config;
        config.env = base_env();
        let (tx, rx) = channel();
        let process = Process::spawn(config, move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        process.resize(77, 33).unwrap();
        let done = collect(&rx);
        assert!(String::from_utf8_lossy(&done.out).contains("33 77"));
    }

    #[test]
    fn a_pty_console_without_a_child_connects_a_third_party_program() {
        let (tx, rx) = channel::<Vec<u8>>();
        let console = PtyConsole::open(
            &Tty {
                onlcr: false,
                ..Tty::default()
            },
            move |bytes| {
                let _ = tx.send(bytes);
            },
        )
        .unwrap();
        let path = console.slave_path().to_string_lossy().into_owned();
        assert!(path.starts_with("/dev/tty"), "{path}");
        // A program that was told to use the tty: reads one line from it and
        // answers on it, plus the size it sees.
        let script =
            format!("read line < {path}; echo \"got:$line\" > {path}; stty size < {path} > {path}");
        let mut child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        console.resize(70, 20).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        console.write(b"hello\n").unwrap();
        child.wait().unwrap();
        let mut text = String::new();
        while let Ok(bytes) = rx.recv_timeout(Duration::from_millis(500)) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(text.contains("got:hello"), "{text:?}");
        assert!(text.contains("20 70"), "{text:?}");
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_panic() {
        let error =
            Process::spawn(ProcessConfig::pty(["/no/such/program-xyz"]), |_| {}).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let error = Process::spawn(ProcessConfig::pty(Vec::<String>::new()), |_| {}).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_grandchild_holding_the_pty_does_not_stall_the_exit_event() {
        let started = Instant::now();
        let done = drive(
            ProcessConfig::pty(["sh", "-c", "sleep 5 & echo started"]),
            &[],
        );
        assert!(String::from_utf8_lossy(&done.out).contains("started"));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        // Clean up the stray sleep: it is in the child's (finished) group.
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn flood_loses_nothing() {
        let mut config = ProcessConfig::pty(["sh", "-c", "yes | head -n 1000000"]);
        config.tty.as_mut().unwrap().onlcr = false;
        let done = drive(config, &[]);
        assert_eq!(done.out.len(), 2_000_000);
        assert!(done.out.chunks(2).all(|pair| pair == b"y\n"));
    }
}
