//! A debounced file watcher: tells the app which open files changed on disk.
//!
//! [`WatchService`] wraps `notify` (FSEvents on macOS). Files are registered
//! with [`WatchService::watch`]; the parent directory is watched (so saves
//! that replace the file by renaming are seen too) and events for unrelated
//! files are dropped. A burst of events for one file is merged and delivered
//! once, after it has been quiet for the debounce interval, on a channel of
//! [`FileChanged`]. An event is a hint: ask [`crate::check_external`] what
//! really changed and [`crate::decide`] what to do.
//!
//! ```no_run
//! use std::path::Path;
//! use std::time::Duration;
//! use scratch_document::{WatchService, check_external, decide, FileStamp};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let service = WatchService::new(Duration::from_millis(200))?;
//! let path = Path::new("/tmp/example.txt");
//! service.watch(path)?;
//! let stamp = FileStamp::of_path(path)?;
//! while let Ok(event) = service.events().recv() {
//!     let action = decide(check_external(&event.path, &stamp)?, false);
//!     println!("{event:?} -> {action:?}");
//! }
//! # Ok(()) }
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// What happened to a watched file, as far as the last event of a burst says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    /// Content or metadata changed.
    Modified,
    /// The file appeared (also: replaced by a rename).
    Created,
    /// The file disappeared (deleted, or renamed away).
    Removed,
}

/// A watched file changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChanged {
    /// The path as it was passed to [`WatchService::watch`].
    pub path: PathBuf,
    /// A hint; see [`crate::check_external`] for the truth.
    pub kind: FileChangeKind,
}

/// Watcher failures.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// The platform watcher refused.
    #[error("file watcher: {0}")]
    Notify(#[from] notify::Error),
    /// The path has no parent directory or file name.
    #[error("{0} cannot be watched")]
    BadPath(PathBuf),
}

#[derive(Debug)]
struct Registered {
    original: PathBuf,
    count: usize,
}

#[derive(Debug, Default)]
struct Shared {
    /// canonical file path -> registration
    files: HashMap<PathBuf, Registered>,
    /// watched directory -> number of files in it
    dirs: HashMap<PathBuf, usize>,
    /// canonical file path -> ignore events until
    muted: HashMap<PathBuf, Instant>,
}

enum Message {
    Event(notify::Event),
    Stop,
}

/// See the module documentation of `watch`.
pub struct WatchService {
    watcher: Mutex<RecommendedWatcher>,
    shared: Arc<Mutex<Shared>>,
    events: Receiver<FileChanged>,
    control: Sender<Message>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for WatchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchService").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in another thread must not take the watcher down with it.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `<canonical parent>/<file name>`: stable whether or not the file exists.
fn canonical_key(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = path.file_name()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).ok()?;
    Some((parent.join(name), parent))
}

impl WatchService {
    /// Starts the watcher and its debounce thread. Events of one file closer
    /// together than `debounce` are delivered as one.
    ///
    /// # Errors
    ///
    /// [`WatchError::Notify`] when the platform watcher cannot start.
    pub fn new(debounce: Duration) -> Result<Self, WatchError> {
        let (control, raw) = mpsc::channel::<Message>();
        let (out, events) = mpsc::channel::<FileChanged>();
        let callback_tx = control.clone();
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                let _ = callback_tx.send(Message::Event(event));
            }
        })?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("scratch-file-watch".into())
            .spawn(move || debounce_loop(&raw, &out, &worker_shared, debounce))
            .map_err(|e| WatchError::Notify(notify::Error::io(e)))?;
        Ok(Self {
            watcher: Mutex::new(watcher),
            shared,
            events,
            control,
            worker: Some(worker),
        })
    }

    /// The channel of changes. Receive on any thread.
    pub fn events(&self) -> &Receiver<FileChanged> {
        &self.events
    }

    /// Starts watching `path` (a file that exists or will). Watching the same
    /// path twice needs two [`WatchService::unwatch`] calls.
    ///
    /// # Errors
    ///
    /// [`WatchError`] when the directory cannot be watched.
    pub fn watch(&self, path: &Path) -> Result<(), WatchError> {
        let (key, dir) =
            canonical_key(path).ok_or_else(|| WatchError::BadPath(path.to_path_buf()))?;
        let mut shared = lock(&self.shared);
        if let Some(entry) = shared.files.get_mut(&key) {
            entry.count += 1;
            return Ok(());
        }
        let dir_count = shared.dirs.get(&dir).copied().unwrap_or(0);
        if dir_count == 0 {
            lock(&self.watcher).watch(&dir, RecursiveMode::NonRecursive)?;
        }
        shared.dirs.insert(dir, dir_count + 1);
        shared.files.insert(
            key,
            Registered {
                original: path.to_path_buf(),
                count: 1,
            },
        );
        Ok(())
    }

    /// Stops watching `path` (one registration).
    pub fn unwatch(&self, path: &Path) {
        let Some((key, dir)) = canonical_key(path) else {
            return;
        };
        let mut shared = lock(&self.shared);
        let Some(entry) = shared.files.get_mut(&key) else {
            return;
        };
        entry.count -= 1;
        if entry.count > 0 {
            return;
        }
        shared.files.remove(&key);
        shared.muted.remove(&key);
        let remaining = shared
            .dirs
            .get(&dir)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if remaining == 0 {
            shared.dirs.remove(&dir);
            // The directory may be gone already; nothing to undo then.
            let _ = lock(&self.watcher).unwatch(&dir);
        } else {
            shared.dirs.insert(dir, remaining);
        }
    }

    /// Whether `path` is currently registered.
    pub fn is_watching(&self, path: &Path) -> bool {
        canonical_key(path).is_some_and(|(key, _)| lock(&self.shared).files.contains_key(&key))
    }

    /// Drops events about `path` for the next `duration`; call right before
    /// writing the file yourself so your own save does not come back as an
    /// external change.
    pub fn mute(&self, path: &Path, duration: Duration) {
        if let Some((key, _)) = canonical_key(path) {
            lock(&self.shared)
                .muted
                .insert(key, Instant::now() + duration);
        }
    }
}

impl Drop for WatchService {
    fn drop(&mut self) {
        let _ = self.control.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn classify(kind: &EventKind) -> Option<FileChangeKind> {
    match kind {
        EventKind::Create(_) => Some(FileChangeKind::Created),
        EventKind::Remove(_) => Some(FileChangeKind::Removed),
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => None, // decided by existence
        EventKind::Modify(_) | EventKind::Any => Some(FileChangeKind::Modified),
        EventKind::Access(_) | EventKind::Other => None,
    }
}

fn debounce_loop(
    raw: &Receiver<Message>,
    out: &Sender<FileChanged>,
    shared: &Mutex<Shared>,
    debounce: Duration,
) {
    // canonical path -> (kind, time of the last event)
    let mut pending: HashMap<PathBuf, (FileChangeKind, Instant)> = HashMap::new();
    loop {
        let message = if let Some(next) = pending.values().map(|(_, at)| *at + debounce).min() {
            raw.recv_timeout(next.saturating_duration_since(Instant::now()))
        } else {
            raw.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match message {
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Message::Event(event)) => {
                let is_rename = matches!(
                    event.kind,
                    EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                let direct = classify(&event.kind);
                if direct.is_none() && !is_rename {
                    continue;
                }
                let guard = lock(shared);
                for path in &event.paths {
                    let key = match canonical_key(path) {
                        Some((key, _)) => key,
                        None => continue,
                    };
                    if !guard.files.contains_key(&key) {
                        continue;
                    }
                    let kind = direct.unwrap_or_else(|| {
                        if key.exists() {
                            FileChangeKind::Created
                        } else {
                            FileChangeKind::Removed
                        }
                    });
                    pending.insert(key, (kind, Instant::now()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let due: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, (_, at))| now.duration_since(*at) >= debounce)
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let Some((kind, _)) = pending.remove(&key) else {
                continue;
            };
            let guard = lock(shared);
            if guard.muted.get(&key).is_some_and(|until| *until > now) {
                continue;
            }
            let Some(entry) = guard.files.get(&key) else {
                continue;
            };
            if out
                .send(FileChanged {
                    path: entry.original.clone(),
                    kind,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const WAIT: Duration = Duration::from_secs(10);

    fn service() -> Option<WatchService> {
        match WatchService::new(Duration::from_millis(100)) {
            Ok(service) => Some(service),
            Err(error) => {
                eprintln!("skipped: no file watcher available ({error})");
                None
            }
        }
    }

    #[test]
    fn reports_a_modification_once() {
        let Some(service) = service() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one").unwrap();
        service.watch(&path).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        {
            // FSEvents reports modifications when the writer closes the file.
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            for _ in 0..5 {
                file.write_all(b"x").unwrap();
                file.flush().unwrap();
            }
        }
        let event = service.events().recv_timeout(WAIT).expect("an event");
        assert_eq!(event.path, path);
        // The burst of five writes was merged. (FSEvents may deliver the file's
        // creation, which happened just before watching, as a second batch.)
        let mut more = 0;
        while service
            .events()
            .recv_timeout(Duration::from_millis(600))
            .is_ok()
        {
            more += 1;
        }
        assert!(more <= 1, "five writes produced {} events", more + 1);
    }

    #[test]
    fn sees_atomic_replacement_and_deletion() {
        let Some(service) = service() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.txt");
        fs::write(&path, "one").unwrap();
        service.watch(&path).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let temp = dir.path().join(".b.tmp");
        fs::write(&temp, "two").unwrap();
        fs::rename(&temp, &path).unwrap();
        let event = service
            .events()
            .recv_timeout(WAIT)
            .expect("replacement event");
        assert_eq!(event.path, path);
        std::thread::sleep(Duration::from_millis(300));
        while service.events().try_recv().is_ok() {}
        fs::remove_file(&path).unwrap();
        let event = service.events().recv_timeout(WAIT).expect("removal event");
        assert_eq!(event.path, path);
        assert_eq!(event.kind, FileChangeKind::Removed);
    }

    #[test]
    fn ignores_other_files_in_the_directory() {
        let Some(service) = service() else { return };
        let dir = tempfile::tempdir().unwrap();
        let watched = dir.path().join("watched.txt");
        let other = dir.path().join("other.txt");
        fs::write(&watched, "w").unwrap();
        fs::write(&other, "o").unwrap();
        service.watch(&watched).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        while service.events().try_recv().is_ok() {}
        fs::write(&other, "changed").unwrap();
        assert!(
            service
                .events()
                .recv_timeout(Duration::from_millis(800))
                .is_err()
        );
    }

    #[test]
    fn muted_writes_are_not_reported_and_unwatch_stops_events() {
        let Some(service) = service() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.txt");
        fs::write(&path, "x").unwrap();
        service.watch(&path).unwrap();
        assert!(service.is_watching(&path));
        std::thread::sleep(Duration::from_millis(300));
        while service.events().try_recv().is_ok() {}
        service.mute(&path, Duration::from_secs(3));
        fs::write(&path, "mine").unwrap();
        assert!(
            service
                .events()
                .recv_timeout(Duration::from_millis(800))
                .is_err()
        );
        service.unwatch(&path);
        assert!(!service.is_watching(&path));
        fs::write(&path, "theirs").unwrap();
        assert!(
            service
                .events()
                .recv_timeout(Duration::from_millis(800))
                .is_err()
        );
    }

    #[test]
    fn watching_twice_needs_two_unwatches() {
        let Some(service) = service() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.txt");
        fs::write(&path, "x").unwrap();
        service.watch(&path).unwrap();
        service.watch(&path).unwrap();
        service.unwatch(&path);
        assert!(service.is_watching(&path));
        service.unwatch(&path);
        assert!(!service.is_watching(&path));
    }

    #[test]
    fn paths_without_a_name_are_rejected() {
        let Some(service) = service() else { return };
        assert!(matches!(
            service.watch(Path::new("/")),
            Err(WatchError::BadPath(_))
        ));
    }
}
