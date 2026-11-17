//! The caller's side of a running run.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::process::Process;
use crate::stats::{ProcessStats, sample_tree};
use crate::{InputError, RunId};

/// What the engine thread receives besides process output.
pub(crate) enum Inbound {
    /// An event of the process of generation `gen` (events of an earlier stage's
    /// process are ignored).
    Proc {
        generation: u64,
        event: crate::process::ProcessEvent,
    },
    /// Re-check the control flags.
    Wake,
}

#[derive(Default)]
struct State {
    process: Option<Arc<Process>>,
    pending_input: Vec<u8>,
    size: Option<(u16, u16)>,
    pid: Option<u32>,
    /// The program has exited: input is refused from now on.
    ended: bool,
}

/// Shared between the [`RunHandle`]s and the engine thread.
pub(crate) struct Control {
    id: RunId,
    wake: Sender<Inbound>,
    cancelled: AtomicBool,
    large_ok: AtomicBool,
    finished: AtomicBool,
    state: Mutex<State>,
}

impl Control {
    pub(crate) fn new(id: RunId, wake: Sender<Inbound>, size: (u16, u16)) -> Self {
        Self {
            id,
            wake,
            cancelled: AtomicBool::new(false),
            large_ok: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            state: Mutex::new(State {
                size: Some(size),
                ..State::default()
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn sender(&self) -> Sender<Inbound> {
        self.wake.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn large_output_allowed(&self) -> bool {
        self.large_ok.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_finished(&self) {
        self.finished.store(true, Ordering::SeqCst);
        let mut state = self.state();
        state.process = None;
        state.pending_input.clear();
    }

    pub(crate) fn size(&self) -> Option<(u16, u16)> {
        self.state().size
    }

    /// Makes the program reachable by the handle: applies the remembered window
    /// size and flushes input typed before it started.
    pub(crate) fn attach(&self, process: Arc<Process>) {
        let mut state = self.state();
        if let Some((cols, rows)) = state.size {
            let _ = process.resize(cols, rows);
        }
        if !state.pending_input.is_empty() {
            let _ = process.write(&std::mem::take(&mut state.pending_input));
        }
        state.pid = Some(process.pid());
        state.process = Some(process);
    }

    /// The program ended or was never started: input is no longer accepted.
    pub(crate) fn detach(&self) {
        let mut state = self.state();
        state.process = None;
        state.ended = true;
    }
}

/// A running (or finished) run. Cheap to clone; every clone controls the same
/// run. Nothing here blocks.
#[derive(Clone)]
pub struct RunHandle {
    pub(crate) control: Arc<Control>,
}

impl std::fmt::Debug for RunHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunHandle")
            .field("id", &self.control.id)
            .field("finished", &self.is_finished())
            .finish()
    }
}

impl RunHandle {
    /// The run's id (also `CR_RUNID`).
    pub fn id(&self) -> RunId {
        self.control.id
    }

    /// The program's pid once it has started.
    pub fn pid(&self) -> Option<u32> {
        self.control.state().pid
    }

    /// Whether the run has ended ([`crate::RunEvent::Finished`] was sent or is
    /// being sent).
    pub fn is_finished(&self) -> bool {
        self.control.finished.load(Ordering::SeqCst)
    }

    /// Stops the run (Stop, ⌘.): SIGTERM to the program's process group, SIGKILL
    /// after [`crate::RunLimits::kill_grace`]; during compilation the stage is
    /// stopped the same way. The run ends with [`crate::RunStatus::Cancelled`].
    /// Idempotent.
    pub fn cancel(&self) {
        self.control.cancelled.store(true, Ordering::SeqCst);
        let _ = self.control.wake.send(Inbound::Wake);
    }

    /// Sends what the user typed to the program's stdin (through the pty: the
    /// line discipline echoes it and ends lines at `\n`). Input sent before the
    /// program has started is held back and delivered when it starts. Never
    /// blocks.
    ///
    /// # Errors
    ///
    /// [`InputError::NotRunning`] after the run ended or the program exited.
    pub fn write_stdin(&self, bytes: &[u8]) -> Result<(), InputError> {
        if self.is_finished() {
            return Err(InputError::NotRunning);
        }
        let mut state = self.control.state();
        if state.ended {
            return Err(InputError::NotRunning);
        }
        match &state.process {
            Some(process) => process.write(bytes).map_err(InputError::Io),
            None => {
                state.pending_input.extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    /// Sends end-of-file (Ctrl-D through the pty; it works at the start of a
    /// line).
    ///
    /// # Errors
    ///
    /// As [`RunHandle::write_stdin`].
    pub fn send_eof(&self) -> Result<(), InputError> {
        self.write_stdin(&[0x04])
    }

    /// Tells the program the console's size (SIGWINCH). Remembered for a program
    /// that has not started yet.
    ///
    /// # Errors
    ///
    /// [`InputError::Io`] when the `ioctl` fails.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), InputError> {
        let mut state = self.control.state();
        state.size = Some((cols, rows));
        match &state.process {
            Some(process) => process.resize(cols, rows).map_err(InputError::Io),
            None => Ok(()),
        }
    }

    /// Sends a signal to the program's process group (Ctrl-C is `SIGINT`).
    /// Returns whether a program was running to receive it.
    pub fn signal(&self, signal: i32) -> bool {
        match &self.control.state().process {
            Some(process) => process.signal(signal).is_ok(),
            None => false,
        }
    }

    /// `SIGINT` to the program's process group: interrupts an infinite loop
    /// without ending the run by force (the program may handle it).
    pub fn interrupt(&self) -> bool {
        self.signal(libc::SIGINT)
    }

    /// The user chose to continue after [`crate::RunEvent::LargeOutput`]: lifts
    /// the output limit and resumes a paused program
    /// ([`crate::RunLimits::pause_at_warning`]).
    pub fn allow_large_output(&self) {
        self.control.large_ok.store(true, Ordering::SeqCst);
        let _ = self.control.wake.send(Inbound::Wake);
    }

    /// CPU and memory of the program and its descendants right now (the live
    /// `CPU n%` / `Memory n K` of the status bar). Spawns `ps`; call it a few
    /// times per second at most.
    pub fn sample_stats(&self) -> Option<ProcessStats> {
        let pid = self.pid()?;
        if self.is_finished() {
            return None;
        }
        sample_tree(pid)
    }
}

impl From<InputError> for io::Error {
    fn from(error: InputError) -> Self {
        match error {
            InputError::Io(error) => error,
            InputError::NotRunning => io::Error::from(io::ErrorKind::BrokenPipe),
        }
    }
}
