//! The `PATH` of a run, and finding programs.
//!
//! A Mac app started from the Finder gets launchd's short `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`) without Homebrew or the user's toolchains.
//! CodeRunner reproduces what a terminal would have: it asks the user's login
//! shell for its `PATH`, adds the `path_helper` directories (`/etc/paths`,
//! `/etc/paths.d/*`), removes duplicates, and caches the result until the user
//! presses "Refresh PATH". This module does the same:
//!
//! * [`determine_path`] computes it (a few hundred milliseconds: it runs a
//!   shell), [`automatic_path`] caches it process-wide, [`refresh_automatic_path`]
//!   recomputes it. Call [`automatic_path`] on a background thread at launch so
//!   the first run does not wait for it.
//! * [`path_helper_paths`], [`dedupe_path`], [`merge_paths`] are the pure pieces.
//! * [`augmented_path`] is the fallback when no login shell answers.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const STANDARD_DIRS: [&str; 6] = [
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
];

const BEGIN: &str = "__SR_PATH_BEGIN__";
const END: &str = "__SR_PATH_END__";

/// How long the login shell has to answer.
pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(8);

/// `existing` with the usual tool directories appended where they are not
/// already listed. `home` adds the per-user toolchain directories. This is the
/// fallback when the login shell cannot be asked.
pub fn augmented_path(existing: &str, home: Option<&Path>) -> String {
    let mut entries: Vec<String> = existing
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect();
    let mut add = |entry: String| {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    };
    for dir in STANDARD_DIRS {
        add(dir.to_owned());
    }
    if let Some(home) = home {
        for dir in [".cargo/bin", ".bun/bin", "go/bin", ".local/bin"] {
            add(home.join(dir).display().to_string());
        }
    }
    entries.join(":")
}

/// The executable `name` in the directories of `path` (a `:`-separated list).
/// A name with a `/` is a path and is checked as it is.
pub fn find_program(name: &str, path: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then_some(candidate);
    }
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| is_executable(candidate))
}

/// Whether `path` is a file with an execute bit.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Removes empty entries and duplicates from a `:`-separated list, keeping the
/// first occurrence of each.
pub fn dedupe_path(path: &str) -> String {
    let mut seen = HashSet::new();
    path.split(':')
        .filter(|entry| !entry.is_empty() && seen.insert(*entry))
        .collect::<Vec<_>>()
        .join(":")
}

/// `primary` followed by the entries of `extra` that it does not have yet, with
/// duplicates removed.
pub fn merge_paths(primary: &str, extra: &[String]) -> String {
    let mut all = primary.to_owned();
    for entry in extra {
        all.push(':');
        all.push_str(entry);
    }
    dedupe_path(&all)
}

/// Puts `prepend` in front of `base` and `append` after it, without duplicates
/// (the first occurrence wins).
pub fn compose_path(prepend: &[String], base: &str, append: &[String]) -> String {
    let mut all = prepend.join(":");
    all.push(':');
    all.push_str(base);
    for entry in append {
        all.push(':');
        all.push_str(entry);
    }
    dedupe_path(&all)
}

/// The directories macOS's `path_helper` adds: the lines of `<etc>/paths`, then
/// the lines of every file in `<etc>/paths.d` in file name order. Missing files
/// give an empty list.
pub fn path_helper_paths(etc: &Path) -> Vec<String> {
    fn lines(file: &Path, out: &mut Vec<String>) {
        if let Ok(text) = std::fs::read_to_string(file) {
            out.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(str::to_owned),
            );
        }
    }
    let mut out = Vec::new();
    lines(&etc.join("paths"), &mut out);
    if let Ok(entries) = std::fs::read_dir(etc.join("paths.d")) {
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        for file in files {
            lines(&file, &mut out);
        }
    }
    out
}

/// The user's login shell: the passwd entry, else `$SHELL`, else `/bin/zsh`.
pub fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null; the shell
    // string is copied before any other call.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if !entry.is_null() && !(*entry).pw_shell.is_null() {
            let shell = std::ffi::CStr::from_ptr((*entry).pw_shell)
                .to_string_lossy()
                .into_owned();
            if !shell.is_empty() {
                return shell;
            }
        }
    }
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_owned())
}

/// The home directory: `$HOME`, else the passwd entry.
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    // SAFETY: as in `login_shell`.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if !entry.is_null() && !(*entry).pw_dir.is_null() {
            return Some(PathBuf::from(
                std::ffi::CStr::from_ptr((*entry).pw_dir)
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
    }
    None
}

/// Runs `command` with `timeout`, returning its status and stdout. The process
/// is killed (with its group) when it does not finish in time. Stdin and stderr
/// are closed.
pub(crate) fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> std::io::Result<(ExitStatus, Vec<u8>)> {
    use std::os::unix::process::CommandExt;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            let _ = stdout.read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            // SAFETY: the child leads its own group (`process_group(0)`).
            unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
            let _ = child.kill();
            child.wait()?;
            let _ = reader.join();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the command did not finish in time",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader.join().unwrap_or_default();
    Ok((status, bytes))
}

/// Why the login shell could not tell its `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathProbeError {
    /// The shell could not be started or did not answer in time.
    #[error("could not run the login shell `{shell}`: {message}")]
    Shell {
        /// The shell.
        shell: String,
        /// What went wrong.
        message: String,
    },
    /// The shell ran but printed no `PATH`.
    #[error("the login shell `{0}` printed no PATH")]
    Empty(String),
}

/// Asks `shell` (in login mode, non-interactive) for its `PATH`. Startup noise
/// printed by the user's profile is ignored.
///
/// # Errors
///
/// [`PathProbeError`] when the shell cannot be run, times out or prints nothing.
pub fn probe_login_shell_path(shell: &str, timeout: Duration) -> Result<String, PathProbeError> {
    let mut command = Command::new(shell);
    command
        .arg("-l")
        .arg("-c")
        .arg(format!("printf '%s%s%s' '{BEGIN}' \"$PATH\" '{END}'"));
    command.env_clear();
    for name in ["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let (_, bytes) =
        output_with_timeout(command, timeout).map_err(|error| PathProbeError::Shell {
            shell: shell.to_owned(),
            message: error.to_string(),
        })?;
    let text = String::from_utf8_lossy(&bytes);
    let path = text
        .split_once(BEGIN)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(path, _)| path.trim().to_owned())
        .unwrap_or_default();
    if path.is_empty() {
        Err(PathProbeError::Empty(shell.to_owned()))
    } else {
        Ok(path)
    }
}

/// Computes the automatic `PATH` from scratch: the login shell's `PATH` (or,
/// when it cannot be asked, this process's `PATH` plus [`augmented_path`]'s
/// standard directories), then the `path_helper` directories of `etc` that are
/// missing, without duplicates.
pub fn determine_path_with(shell: &str, etc: &Path, timeout: Duration) -> String {
    let base = match probe_login_shell_path(shell, timeout) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(%error, "falling back to the process PATH");
            augmented_path(
                &std::env::var("PATH").unwrap_or_default(),
                home_dir().as_deref(),
            )
        }
    };
    merge_paths(&base, &path_helper_paths(etc))
}

/// [`determine_path_with`] for the user's login shell and `/etc`.
pub fn determine_path() -> String {
    determine_path_with(&login_shell(), Path::new("/etc"), LOGIN_SHELL_TIMEOUT)
}

static AUTOMATIC: Mutex<Option<String>> = Mutex::new(None);

/// The automatically determined `PATH`, computed on first use and then cached
/// for the life of the process (CodeRunner's `automaticallyDeterminedPathVariable`).
/// The first call takes as long as the login shell needs: call it on a
/// background thread at launch.
pub fn automatic_path() -> String {
    {
        let guard = AUTOMATIC.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(path) = guard.as_ref() {
            return path.clone();
        }
    }
    let path = determine_path();
    let mut guard = AUTOMATIC.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert(path).clone()
}

/// The cached automatic `PATH` if it was determined already (never blocks).
pub fn cached_automatic_path() -> Option<String> {
    AUTOMATIC.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Recomputes the automatic `PATH` ("Refresh PATH") and returns it.
pub fn refresh_automatic_path() -> String {
    let path = determine_path();
    *AUTOMATIC.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_standard_directories_once() {
        let path = augmented_path("/usr/bin:/opt/homebrew/bin", Some(Path::new("/Users/u")));
        let entries: Vec<&str> = path.split(':').collect();
        assert_eq!(entries[0], "/usr/bin");
        assert_eq!(
            entries
                .iter()
                .filter(|e| **e == "/opt/homebrew/bin")
                .count(),
            1
        );
        assert!(entries.contains(&"/Users/u/.cargo/bin"));
        assert!(entries.contains(&"/usr/local/bin"));
    }

    #[test]
    fn finds_a_program_on_the_path() {
        assert_eq!(
            find_program("sh", "/no/such/dir:/bin"),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(find_program("no-such-program-xyz", "/bin:/usr/bin"), None);
    }

    #[test]
    fn checks_an_absolute_path_directly() {
        assert_eq!(find_program("/bin/sh", ""), Some(PathBuf::from("/bin/sh")));
        assert_eq!(find_program("/no/such/python", "/bin"), None);
    }

    #[test]
    fn dedupe_keeps_the_first_occurrence_and_drops_empties() {
        assert_eq!(dedupe_path("/a:/b::/a:/c:/b"), "/a:/b:/c");
        assert_eq!(dedupe_path(""), "");
    }

    #[test]
    fn merge_and_compose() {
        assert_eq!(
            merge_paths("/a:/b", &["/b".to_owned(), "/c".to_owned()]),
            "/a:/b:/c"
        );
        assert_eq!(
            compose_path(
                &["/x".to_owned()],
                "/a:/x",
                &["/z".to_owned(), "/a".to_owned()]
            ),
            "/x:/a:/z"
        );
    }

    #[test]
    fn path_helper_reads_paths_then_paths_d_in_order() {
        let etc = tempfile::tempdir().unwrap();
        std::fs::write(
            etc.path().join("paths"),
            "/usr/local/bin\n/usr/bin\n\n# comment\n",
        )
        .unwrap();
        std::fs::create_dir(etc.path().join("paths.d")).unwrap();
        std::fs::write(etc.path().join("paths.d/20-b"), "/opt/b\n").unwrap();
        std::fs::write(etc.path().join("paths.d/10-a"), "/opt/a\n/usr/bin\n").unwrap();
        assert_eq!(
            path_helper_paths(etc.path()),
            ["/usr/local/bin", "/usr/bin", "/opt/a", "/usr/bin", "/opt/b"]
        );
        assert!(path_helper_paths(Path::new("/no/such/etc")).is_empty());
    }

    #[test]
    fn determine_path_merges_shell_path_and_helper_paths_without_duplicates() {
        // A fake login shell that ignores its arguments and prints a PATH with
        // profile noise around it.
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("fakesh");
        std::fs::write(
            &shell,
            format!("#!/bin/sh\necho 'welcome noise'\nprintf '%s%s%s' {BEGIN} '/opt/x:/usr/bin:/opt/x' {END}\necho trailing\n"),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let etc = tempfile::tempdir().unwrap();
        std::fs::write(etc.path().join("paths"), "/usr/bin\n/sbin\n").unwrap();
        let path = determine_path_with(shell.to_str().unwrap(), etc.path(), Duration::from_secs(5));
        assert_eq!(path, "/opt/x:/usr/bin:/sbin");
    }

    #[test]
    fn a_broken_login_shell_falls_back_to_the_process_path() {
        let etc = tempfile::tempdir().unwrap();
        let path = determine_path_with("/no/such/shell", etc.path(), Duration::from_secs(2));
        assert!(path.contains("/usr/bin"), "{path}");
        assert!(path.contains("/opt/homebrew/bin"), "{path}");
    }

    #[test]
    fn the_real_login_shell_gives_a_path_with_system_directories() {
        let shell = login_shell();
        if !Path::new(&shell).exists() {
            eprintln!("SKIP: login shell {shell} missing");
            return;
        }
        let path = determine_path();
        assert!(path.split(':').any(|d| d == "/usr/bin"), "{path}");
        assert!(!path.contains("::"), "{path}");
        let entries: Vec<&str> = path.split(':').collect();
        let unique: HashSet<&&str> = entries.iter().collect();
        assert_eq!(entries.len(), unique.len(), "duplicates in {path}");
    }

    #[test]
    fn a_hanging_shell_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("hang");
        std::fs::write(&shell, "#!/bin/sh\nsleep 30\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let started = Instant::now();
        let error = probe_login_shell_path(shell.to_str().unwrap(), Duration::from_millis(300))
            .unwrap_err();
        assert!(matches!(error, PathProbeError::Shell { .. }), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
