//! Starting runs, and the one-run-per-document rule.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use crate::engine::{self, Sink};
use crate::{OnBusy, RunEvent, RunHandle, RunId, RunMessage, RunRequest, StartError};

/// Starts a run that nobody tracks: `sink` receives every [`RunMessage`] on the
/// run's thread (keep it cheap: push into a channel or a signal), ending with
/// [`RunEvent::Finished`]. See [`Runner`] for the per-document rule.
///
/// # Errors
///
/// [`StartError::Thread`] when the thread cannot be created.
pub fn start(
    request: RunRequest,
    sink: impl Fn(RunMessage) + Send + Sync + 'static,
) -> Result<RunHandle, StartError> {
    engine::spawn(request, Arc::new(sink))
}

/// [`start`] with a `std::sync::mpsc` channel as the sink: no async runtime
/// needed to consume a run.
///
/// # Errors
///
/// As [`start`].
pub fn start_channel(request: RunRequest) -> Result<(RunHandle, Receiver<RunMessage>), StartError> {
    let (tx, rx) = mpsc::channel();
    let handle = start(request, move |message| {
        let _ = tx.send(message);
    })?;
    Ok((handle, rx))
}

/// Tracks the active run of every owner (document) and enforces *one run per
/// document*: a document with a run in progress cannot start another
/// ([`OnBusy::Reject`]) unless the request says the old one is replaced
/// ([`OnBusy::StopPrevious`]). Different documents run concurrently, each on
/// its own thread with its own process group, temporary directory and
/// [`RunId`].
///
/// # Stale events
///
/// Every message carries its run's id. After `StopPrevious` the old run still
/// delivers its last events (its `Cancelled` summary) while the new one starts;
/// a consumer keeps the id of each document's current run
/// ([`RunHandle::id`], [`Runner::active`]) and drops messages for any other id
/// ([`Runner::is_current`]).
///
/// `Runner` is cheap to clone; clones share the same table.
#[derive(Clone, Default)]
pub struct Runner {
    active: Arc<Mutex<HashMap<String, RunHandle>>>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("active", &self.active_count())
            .finish()
    }
}

impl Runner {
    /// An empty runner.
    pub fn new() -> Self {
        Self::default()
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<String, RunHandle>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a run. With `request.owner` set the run is tracked until it
    /// finishes. Messages go to `sink` as for [`start`]; the run is removed from
    /// the table *before* its `Finished` message is delivered, so a handler can
    /// already start the next run of the document.
    ///
    /// # Errors
    ///
    /// [`StartError::AlreadyRunning`] when the owner is busy and `on_busy` is
    /// `Reject`; [`StartError::Thread`].
    pub fn start(
        &self,
        request: RunRequest,
        sink: impl Fn(RunMessage) + Send + Sync + 'static,
    ) -> Result<RunHandle, StartError> {
        let Some(owner) = request.owner.clone() else {
            return start(request, sink);
        };
        let mut table = self.table();
        if let Some(existing) = table.get(&owner).filter(|h| !h.is_finished()) {
            match request.on_busy {
                OnBusy::Reject => {
                    return Err(StartError::AlreadyRunning {
                        owner,
                        run: existing.id(),
                    });
                }
                OnBusy::StopPrevious => existing.cancel(),
            }
        }
        let tracker = self.clone();
        let tracked_owner = owner.clone();
        let wrapped: Sink = Arc::new(move |message: RunMessage| {
            if matches!(message.event, RunEvent::Finished(_)) {
                let mut table = tracker.table();
                if table
                    .get(&tracked_owner)
                    .is_some_and(|handle| handle.id() == message.run)
                {
                    table.remove(&tracked_owner);
                }
            }
            sink(message);
        });
        // The lock is held while the thread starts: a run that finishes at once
        // cannot remove its entry before it was inserted.
        let handle = engine::spawn(request, wrapped)?;
        table.insert(owner, handle.clone());
        Ok(handle)
    }

    /// [`Runner::start`] with a channel as the sink.
    ///
    /// # Errors
    ///
    /// As [`Runner::start`].
    pub fn start_channel(
        &self,
        request: RunRequest,
    ) -> Result<(RunHandle, Receiver<RunMessage>), StartError> {
        let (tx, rx) = mpsc::channel();
        let handle = self.start(request, move |message| {
            let _ = tx.send(message);
        })?;
        Ok((handle, rx))
    }

    /// The id of the run in progress for `owner`.
    pub fn active(&self, owner: &str) -> Option<RunId> {
        self.table()
            .get(owner)
            .filter(|handle| !handle.is_finished())
            .map(RunHandle::id)
    }

    /// The handle of the run in progress for `owner`.
    pub fn handle(&self, owner: &str) -> Option<RunHandle> {
        self.table()
            .get(owner)
            .filter(|handle| !handle.is_finished())
            .cloned()
    }

    /// Whether `run` is the current run of `owner`. A message whose run is not
    /// current is stale and must be ignored. (After a run finished, `owner` has
    /// no current run: the final `Finished` message of a run is delivered when it
    /// has already left the table, so check ids against the id you stored when
    /// you started the run, and use this to detect replaced runs.)
    pub fn is_current(&self, owner: &str, run: RunId) -> bool {
        self.table()
            .get(owner)
            .is_some_and(|handle| handle.id() == run)
    }

    /// Stops the run of `owner`, if any. Returns whether there was one.
    pub fn cancel(&self, owner: &str) -> bool {
        match self.handle(owner) {
            Some(handle) => {
                handle.cancel();
                true
            }
            None => false,
        }
    }

    /// Stops every run (the app is quitting).
    pub fn cancel_all(&self) {
        for handle in self.table().values() {
            handle.cancel();
        }
    }

    /// How many runs are in progress.
    pub fn active_count(&self) -> usize {
        self.table().values().filter(|h| !h.is_finished()).count()
    }
}
