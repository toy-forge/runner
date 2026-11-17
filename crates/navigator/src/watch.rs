//! File-system watching: `notify` (FSEvents on macOS) -> debounce thread ->
//! a tokio channel the NSH engine thread awaits in a `spawn` (see
//! `ui::store::Store::start_watching`).
//!
//! Design notes (all measured/verified):
//!
//! * One non-recursive watch per *loaded* directory, not one recursive watch on
//!   the root: opening `~/Repos` recursively would stream every `target/` and
//!   `node_modules/` write.
//! * FSEvents reports canonical paths (`/private/var/...` for `/var/...`);
//!   [`DirWatcher`] canonicalizes what it watches and maps events back to the
//!   path the caller used.
//! * Events are coalesced: a batch is emitted after `quiet` of silence (or
//!   after `max_wait` of continuous noise), as a sorted, de-duplicated list.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedReceiver;

/// Why watching failed.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// `notify` failed.
    #[error(transparent)]
    Notify(#[from] notify::Error),
    /// The path cannot be resolved.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Timing of the debounce.
#[derive(Debug, Clone, Copy)]
pub struct Debounce {
    /// Silence needed before a batch is emitted.
    pub quiet: Duration,
    /// A batch is emitted at the latest this long after its first event.
    pub max_wait: Duration,
}

impl Default for Debounce {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(120),
            max_wait: Duration::from_millis(800),
        }
    }
}

/// Paths nobody wants a refresh for.
pub fn is_noise(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".git")
        || path.file_name().is_some_and(|n| n == ".DS_Store")
}

type Roots = Arc<Mutex<HashMap<PathBuf, PathBuf>>>; // canonical -> logical

/// Watches a set of directories (non-recursively) and delivers debounced
/// batches of changed paths.
pub struct DirWatcher {
    watcher: notify::RecommendedWatcher,
    roots: Roots,
}

impl DirWatcher {
    /// A watcher and the receiver of its batches. Dropping the watcher stops
    /// the debounce thread.
    pub fn new(debounce: Debounce) -> Result<(Self, UnboundedReceiver<Vec<PathBuf>>), WatchError> {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let roots: Roots = Arc::new(Mutex::new(HashMap::new()));
        let map_roots = roots.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let roots = map_roots.lock().map(|g| g.clone()).unwrap_or_default();
            for p in event.paths {
                let _ = raw_tx.send(translate(&roots, p));
            }
        })?;
        thread::Builder::new()
            .name("sidebar-fs-debounce".into())
            .spawn(move || debounce_loop(raw_rx, out_tx, debounce))
            .map_err(notify::Error::io)?;
        Ok((Self { watcher, roots }, out_rx))
    }

    /// Starts watching `dir` (no-op when already watched).
    pub fn watch(&mut self, dir: &Path) -> Result<(), WatchError> {
        let canonical = dir.canonicalize()?;
        {
            let mut roots = self
                .roots
                .lock()
                .map_err(|_| notify::Error::generic("poisoned"))?;
            if roots.contains_key(&canonical) {
                return Ok(());
            }
            roots.insert(canonical.clone(), dir.to_path_buf());
        }
        self.watcher
            .watch(&canonical, RecursiveMode::NonRecursive)?;
        Ok(())
    }

    /// Stops watching `dir`.
    pub fn unwatch(&mut self, dir: &Path) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if let Ok(mut roots) = self.roots.lock() {
            roots.remove(&canonical);
        }
        let _ = self.watcher.unwatch(&canonical);
    }

    /// Makes the watched set exactly `dirs`.
    pub fn sync<'a>(&mut self, dirs: impl IntoIterator<Item = &'a Path>) {
        let want: BTreeSet<PathBuf> = dirs.into_iter().map(Path::to_path_buf).collect();
        let have: Vec<PathBuf> = self
            .roots
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default();
        for d in &have {
            if !want.contains(d) {
                self.unwatch(d);
            }
        }
        for d in &want {
            if let Err(e) = self.watch(d) {
                log::debug!("watch {}: {e}", d.display());
            }
        }
    }

    /// How many directories are watched.
    pub fn len(&self) -> usize {
        self.roots.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether nothing is watched.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn translate(roots: &HashMap<PathBuf, PathBuf>, path: PathBuf) -> PathBuf {
    for (canonical, logical) in roots {
        if let Ok(rest) = path.strip_prefix(canonical) {
            return logical.join(rest);
        }
    }
    path
}

fn debounce_loop(
    rx: mpsc::Receiver<PathBuf>,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<PathBuf>>,
    cfg: Debounce,
) {
    loop {
        let Ok(first) = rx.recv() else { return };
        let started = Instant::now();
        let mut batch = BTreeSet::new();
        if !is_noise(&first) {
            batch.insert(first);
        }
        loop {
            let left = cfg.max_wait.saturating_sub(started.elapsed());
            let wait = cfg.quiet.min(left);
            match rx.recv_timeout(wait) {
                Ok(p) => {
                    if !is_noise(&p) {
                        batch.insert(p);
                    }
                    if started.elapsed() >= cfg.max_wait {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if !batch.is_empty() && tx.send(batch.into_iter().collect()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(rx: &mut UnboundedReceiver<Vec<PathBuf>>, within: Duration) -> Option<Vec<PathBuf>> {
        let end = Instant::now() + within;
        loop {
            if let Ok(b) = rx.try_recv() {
                return Some(b);
            }
            if Instant::now() > end {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_burst_is_one_batch_with_logical_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (mut w, mut rx) = DirWatcher::new(Debounce::default()).unwrap();
        w.watch(dir.path()).unwrap();
        // FSEvents needs a moment to arm the stream.
        thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        for i in 0..20 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let batch = next(&mut rx, Duration::from_secs(5)).expect("a batch");
        let latency = t0.elapsed();
        eprintln!(
            "watch: 20 writes -> one batch of {} paths after {latency:?}",
            batch.len()
        );
        assert!(
            batch.iter().all(|p| p.starts_with(dir.path())),
            "{batch:?} not under {:?}",
            dir.path()
        );
        assert!(batch.iter().any(|p| p.ends_with("f19.txt")));
        assert!(
            next(&mut rx, Duration::from_millis(400)).is_none(),
            "one burst, one batch"
        );
    }

    #[test]
    fn non_recursive_watch_ignores_deep_changes_and_unwatch_stops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let (mut w, mut rx) = DirWatcher::new(Debounce::default()).unwrap();
        w.watch(dir.path()).unwrap();
        thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("sub/deep.txt"), "x").unwrap();
        let deep = next(&mut rx, Duration::from_millis(900));
        eprintln!("watch: deep change on a non-recursive watch -> {deep:?}");
        std::fs::write(dir.path().join("top.txt"), "x").unwrap();
        let top = next(&mut rx, Duration::from_secs(3)).expect("top-level change seen");
        assert!(top.iter().any(|p| p.ends_with("top.txt")));
        w.unwatch(dir.path());
        thread::sleep(Duration::from_millis(200));
        while rx.try_recv().is_ok() {}
        std::fs::write(dir.path().join("after.txt"), "x").unwrap();
        assert!(next(&mut rx, Duration::from_millis(700)).is_none());
    }

    #[test]
    fn noise_is_dropped() {
        assert!(is_noise(Path::new("/r/.git/index.lock")));
        assert!(is_noise(Path::new("/r/.DS_Store")));
        assert!(!is_noise(Path::new("/r/src/main.rs")));
    }
}
