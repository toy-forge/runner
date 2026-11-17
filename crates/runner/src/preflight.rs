//! Checks before anything is started: is the toolchain there, would starting it
//! open a system dialog, which Python is meant.
//!
//! * **Developer tools.** On macOS `/usr/bin/clang`, `python3`, `git`, `swift`,
//!   `xcrun`... are shims from `libxcselect`. Without a developer directory
//!   (`xcode-select -p` fails) starting one opens the system's "install the
//!   command line developer tools" dialog. The engine refuses to start such a
//!   shim ([`crate::RunError::DeveloperToolsMissing`]) and shows the
//!   `xcode-select` guidance instead ([`developer_tools_message`]).
//! * **Python.** `python`/`python3` is resolved to a real interpreter when the
//!   first one on the `PATH` is only the shim ([`resolve_python`]).
//! * **Java.** `/usr/bin/java` is a stub that opens a dialog when no JDK is
//!   installed.
//! * **Missing tools** give [`crate::RunError::MissingTool`] with the language's
//!   installer hint.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use scratch_language::{LanguageDef, PlannedStep};

use crate::RunError;
use crate::path::{find_program, output_with_timeout};

/// Where Python is commonly installed besides the `PATH`.
pub const PYTHON_FALLBACK_DIRS: &[&str] = &[
    "/Library/Frameworks/Python.framework/Versions/Current/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
];

static DEVELOPER_DIR: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

/// Asks the system for the developer directory without caching: `DEVELOPER_DIR`
/// when it points at a directory, else `xcode-select -p`.
pub fn detect_developer_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DEVELOPER_DIR").map(PathBuf::from)
        && dir.is_dir()
    {
        return Some(dir);
    }
    let mut command = Command::new("/usr/bin/xcode-select");
    command.arg("-p");
    let (status, stdout) = output_with_timeout(command, Duration::from_secs(5)).ok()?;
    if !status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&stdout).trim());
    dir.is_dir().then_some(dir)
}

/// The developer directory, detected once and cached (like CodeRunner's
/// `Runner developerToolsPath`). `None` = no developer tools installed.
pub fn developer_dir() -> Option<PathBuf> {
    {
        let guard = DEVELOPER_DIR.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
    }
    let dir = detect_developer_dir();
    *DEVELOPER_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
    dir
}

/// Detects the developer directory again (the user installed the tools while the
/// app was running) and caches the answer.
pub fn refresh_developer_dir() -> Option<PathBuf> {
    let dir = detect_developer_dir();
    *DEVELOPER_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
    dir
}

/// Whether `path` is a `libxcselect` shim: a small executable in `/usr/bin`
/// that forwards to the developer directory (and opens an installer dialog when
/// there is none), recognised by the `xcode_select.tool-shim` marker in the
/// file; `/usr/bin/xcrun` counts too.
pub fn is_xcode_shim(path: &Path) -> bool {
    use std::io::Read;
    let Ok(resolved) = path.canonicalize() else {
        return false;
    };
    if !resolved.starts_with("/usr/bin") {
        return false;
    }
    // `xcrun` is not a forwarding shim but just as unusable without a developer
    // directory (it opens the same installer dialog).
    if resolved == Path::new("/usr/bin/xcrun") {
        return true;
    }
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return false;
    };
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return false;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    if std::fs::File::open(&resolved)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .is_err()
    {
        return false;
    }
    const MARK: &[u8] = b"xcode_select.tool-shim";
    bytes.windows(MARK.len()).any(|window| window == MARK)
}

/// The guidance shown when a shim must not be started.
pub fn developer_tools_message(program: &str) -> String {
    format!(
        "`{program}` requires Apple's command line developer tools, which are not installed. \
To install Command Line Tools, run the command in the Terminal: xcode-select --install\n\
If Xcode is installed, select it with: sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer"
    )
}

/// The text for "no usable Python".
pub fn python_missing_message() -> String {
    "Python is not installed on your system. It is recommended that you download and install \
it from python.org (https://www.python.org/downloads/).\n\
Alternatively, you can install Apple's command line developer tools: run \
xcode-select --install in the Terminal."
        .to_owned()
}

/// Whether `name` is the name of a Python interpreter (`python`, `python3`,
/// `python3.12`).
pub fn is_python_name(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|rest| {
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// The interpreter to use for `name` (`python3`): the first match in the
/// directories of `path`, then [`PYTHON_FALLBACK_DIRS`]. Shims are skipped when
/// `developer_tools` is false (they would open the installer). For `python`
/// `python3` is tried when there is no `python`. `None` = no usable Python.
pub fn resolve_python(name: &str, path: &str, developer_tools: bool) -> Option<PathBuf> {
    let mut dirs: Vec<&str> = path.split(':').filter(|d| !d.is_empty()).collect();
    dirs.extend(PYTHON_FALLBACK_DIRS);
    let mut names = vec![name];
    if name == "python" {
        names.push("python3");
    }
    for name in names {
        for dir in &dirs {
            let candidate = Path::new(dir).join(name);
            if crate::path::is_executable(&candidate)
                && (developer_tools || !is_xcode_shim(&candidate))
            {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `path` is the system's Java stub and no JDK is installed (it would
/// open a dialog).
fn is_java_stub_without_runtime(path: &Path) -> bool {
    let is_stub = matches!(
        path.to_str(),
        Some("/usr/bin/java" | "/usr/bin/javac" | "/usr/bin/jar")
    );
    if !is_stub || !Path::new("/usr/libexec/java_home").exists() {
        return false;
    }
    let command = Command::new("/usr/libexec/java_home");
    match output_with_timeout(command, Duration::from_secs(5)) {
        Ok((status, _)) => !status.success(),
        Err(_) => false,
    }
}

/// The message for a missing `program`: the language's `errors["127"]` /
/// installer hint, else a plain sentence.
pub fn missing_tool_message(language: &LanguageDef, program: &str) -> String {
    language.exit_status_message(127).unwrap_or_else(|| {
        format!("Could not find `{program}`. Check that it is installed and on your PATH.")
    })
}

/// Decides which argv to start for one compile stage: with the `via` wrapper
/// when it is installed, else without; programs are replaced by their absolute
/// path; shims are refused without developer tools.
///
/// # Errors
///
/// [`RunError::MissingTool`] (the step's own `missing_tool` text or the
/// language's hint) or [`RunError::DeveloperToolsMissing`].
pub fn resolve_step_argv(
    step: &PlannedStep,
    language: &LanguageDef,
    path: &str,
    developer_tools: bool,
) -> Result<Vec<String>, RunError> {
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if step
        .via
        .first()
        .is_some_and(|wrapper| find_program(wrapper, path).is_some())
    {
        candidates.push(step.argv(true));
    }
    candidates.push(step.argv(false));
    let mut blocked: Option<String> = None;
    for mut argv in candidates {
        let Some(found) = find_program(&argv[0], path) else {
            continue;
        };
        if !developer_tools && is_xcode_shim(&found) {
            blocked.get_or_insert_with(|| argv[0].clone());
            continue;
        }
        if is_java_stub_without_runtime(&found) {
            return Err(RunError::MissingTool {
                program: argv[0].clone(),
                message: step
                    .missing_tool
                    .clone()
                    .unwrap_or_else(|| missing_tool_message(language, &argv[0])),
            });
        }
        argv[0] = found.to_string_lossy().into_owned();
        return Ok(argv);
    }
    if let Some(program) = blocked {
        return Err(RunError::DeveloperToolsMissing {
            message: developer_tools_message(&program),
            program,
        });
    }
    Err(RunError::MissingTool {
        program: step.program.clone(),
        message: step
            .missing_tool
            .clone()
            .unwrap_or_else(|| missing_tool_message(language, &step.program)),
    })
}

/// Resolves the program of the final run argv (not for shell commands): the
/// absolute path, Python resolution, shim and stub refusal.
///
/// # Errors
///
/// [`RunError::MissingTool`], [`RunError::DeveloperToolsMissing`] or
/// [`RunError::PythonMissing`].
pub fn resolve_run_program(
    program: &str,
    language: &LanguageDef,
    path: &str,
    developer_tools: bool,
) -> Result<PathBuf, RunError> {
    let base = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    let found = find_program(program, path);
    if is_python_name(base) && !program.contains('/') {
        let usable = found
            .as_deref()
            .filter(|found| developer_tools || !is_xcode_shim(found))
            .map(Path::to_path_buf);
        return usable
            .or_else(|| resolve_python(base, path, developer_tools))
            .ok_or_else(|| RunError::PythonMissing {
                message: python_missing_message(),
            });
    }
    let Some(found) = found else {
        return Err(RunError::MissingTool {
            program: program.to_owned(),
            message: missing_tool_message(language, program),
        });
    };
    if !developer_tools && is_xcode_shim(&found) {
        return Err(RunError::DeveloperToolsMissing {
            program: program.to_owned(),
            message: developer_tools_message(program),
        });
    }
    if is_java_stub_without_runtime(&found) {
        return Err(RunError::MissingTool {
            program: program.to_owned(),
            message: missing_tool_message(language, program),
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_names() {
        for name in ["python", "python3", "python3.12", "python2"] {
            assert!(is_python_name(name), "{name}");
        }
        for name in ["pythonista", "py", "python-config", "ipython", "python3x"] {
            assert!(!is_python_name(name), "{name}");
        }
    }

    #[test]
    fn shims_are_recognised_by_content() {
        if !Path::new("/usr/bin/clang").exists() {
            eprintln!("SKIP: no /usr/bin/clang");
            return;
        }
        assert!(is_xcode_shim(Path::new("/usr/bin/clang")));
        assert!(!is_xcode_shim(Path::new("/usr/bin/perl")));
        assert!(!is_xcode_shim(Path::new("/bin/sh")));
        assert!(!is_xcode_shim(Path::new("/no/such/file")));
    }

    #[test]
    fn a_shim_without_developer_tools_is_refused_with_guidance() {
        if !Path::new("/usr/bin/clang").exists() {
            eprintln!("SKIP: no /usr/bin/clang");
            return;
        }
        let language = LanguageDef::new("c", "C");
        let error =
            resolve_run_program("/usr/bin/clang", &language, "/usr/bin", false).unwrap_err();
        match error {
            RunError::DeveloperToolsMissing { program, message } => {
                assert_eq!(program, "/usr/bin/clang");
                assert!(message.contains("xcode-select --install"), "{message}");
                assert!(message.contains("xcode-select --switch"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        // With tools it is fine.
        assert_eq!(
            resolve_run_program("clang", &language, "/usr/bin", true).unwrap(),
            Path::new("/usr/bin/clang")
        );
    }

    #[test]
    fn python_resolution_skips_the_shim_without_tools() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python3");
        std::fs::write(&python, "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("/usr/bin:{}", dir.path().display());
        // Without tools: /usr/bin/python3 (a shim, when present) is skipped.
        let resolved = resolve_python("python3", &path, false).unwrap();
        if Path::new("/usr/bin/python3").exists() && is_xcode_shim(Path::new("/usr/bin/python3")) {
            assert_eq!(resolved, python);
        }
        let language = LanguageDef::new("python", "Python");
        assert_eq!(
            resolve_run_program("python3", &language, &path, false).unwrap(),
            resolved
        );
        // Nothing usable anywhere: PythonMissing (fallback dirs may exist on a dev
        // machine, so only check the error shape with an impossible name).
        assert!(resolve_python("python9.99", "/nonexistent", false).is_none());
        assert!(matches!(
            resolve_run_program("python9.99", &language, "/nonexistent", false),
            Err(RunError::PythonMissing { .. })
        ));
    }

    #[test]
    fn a_missing_tool_carries_the_installer_hint() {
        let mut language = LanguageDef::new("go", "Go");
        language.installer = Some(scratch_language::InstallerHint {
            executable: "go".into(),
            name: "the Go compiler".into(),
            url: "https://go.dev".into(),
        });
        let error = resolve_run_program("no-such-tool-xyz", &language, "/bin", true).unwrap_err();
        match error {
            RunError::MissingTool { program, message } => {
                assert_eq!(program, "no-such-tool-xyz");
                assert!(message.contains("the Go compiler"), "{message}");
                assert!(message.contains("https://go.dev"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn detect_developer_dir_agrees_with_xcode_select() {
        let detected = detect_developer_dir();
        let status = Command::new("xcode-select").arg("-p").output();
        match status {
            Ok(output) if output.status.success() => {
                assert!(detected.is_some());
            }
            Ok(_) => assert!(detected.is_none() || std::env::var_os("DEVELOPER_DIR").is_some()),
            Err(_) => eprintln!("SKIP: no xcode-select"),
        }
        assert_eq!(developer_dir(), developer_dir());
    }
}
