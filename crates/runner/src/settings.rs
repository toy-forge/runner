//! Settings that shape a run: the global Shell preferences, the per-file Run
//! Settings, and engine options.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use scratch_language::split_words;
use serde::{Deserialize, Serialize};

use crate::{RunError, Tty};

/// The default of the "Use custom PATH" field.
pub const DEFAULT_CUSTOM_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Where the `PATH` of a run comes from (Preferences > Shell).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSetting {
    /// "Automatically determine PATH environment variable": asked from the
    /// login shell once ([`crate::automatic_path`]), refreshed on demand.
    Automatic,
    /// "Use custom PATH": exactly this list (the language's own `path_prepend`
    /// and `path_append` still apply).
    Custom(String),
}

/// The global Shell preferences (Preferences > Shell) and the related
/// Advanced ones. `Default` is CodeRunner's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    /// `Shell`: runs shell run commands and compile scripts.
    pub shell: String,
    /// `RunAsLoginShell`: invoke the shell with `-l` when running code (not the
    /// same as how the automatic `PATH` is determined, which always uses a login
    /// shell).
    pub login_shell: bool,
    /// `AutomaticallyDeterminePATH` / `PATH`.
    pub path: PathSetting,
    /// `Environment`: the "Shell Variables" table. Defaults: `LC_CTYPE=UTF-8`,
    /// `TERM=dumb`, `TERM_PROGRAM=ScratchRunner`. `TERM` is also what the pty
    /// program sees.
    pub variables: BTreeMap<String, String>,
    /// `ShellArguments`: extra arguments for the shell that runs the command.
    pub shell_arguments: Vec<String>,
    /// `CompileShellArguments`: extra arguments for the shell that runs a
    /// compile script.
    pub compile_shell_arguments: Vec<String>,
    /// `advanced/WriteCompilerOutputFilesToTemporaryDirectory`: build products
    /// go to the temporary directory (true) or next to the source file.
    pub compiler_output_in_temp_dir: bool,
    /// `MACOSX_DEPLOYMENT_TARGET` to export. CodeRunner sets `10.13`; it is off
    /// here because modern SDKs reject some C++ library features at such a low
    /// target. Add `MACOSX_DEPLOYMENT_TARGET` to `variables` to force one.
    pub deployment_target: Option<String>,
}

impl ShellSettings {
    /// The default Shell Variables table.
    pub fn default_variables() -> BTreeMap<String, String> {
        [
            ("LC_CTYPE", "UTF-8"),
            ("TERM", "dumb"),
            ("TERM_PROGRAM", "ScratchRunner"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
    }

    /// The `PATH` this setting yields before the language's additions: the
    /// custom list, or the (cached) automatic one. The first automatic call may
    /// take a moment ([`crate::automatic_path`]).
    pub fn base_path(&self) -> String {
        match &self.path {
            PathSetting::Custom(path) => path.clone(),
            PathSetting::Automatic => crate::automatic_path(),
        }
    }
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            shell: "/bin/bash".to_owned(),
            login_shell: false,
            path: PathSetting::Automatic,
            variables: Self::default_variables(),
            shell_arguments: Vec::new(),
            compile_shell_arguments: Vec::new(),
            compiler_output_in_temp_dir: true,
            deployment_target: None,
        }
    }
}

/// The Program Input of a run (stdin of the program).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramInput {
    /// Text typed in the Run Settings sheet. Written to `cr_input_<n>.txt` in
    /// the run's temporary directory and redirected (`< file`).
    Text(String),
    /// An existing file, redirected as it is.
    File(PathBuf),
}

/// Which directory the program runs in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectory {
    /// The directory of the file (CodeRunner's behaviour). An unsaved document
    /// without a directory runs in the run's temporary directory.
    #[default]
    FileDirectory,
    /// The folder opened in the window ([`crate::RunRequest::project_folder`]),
    /// else the file's directory.
    ProjectFolder,
    /// The default directory ([`crate::RunRequest::default_directory`]), else the
    /// user's home.
    Default,
    /// This directory.
    Custom(PathBuf),
}

/// The per-file Run Settings (Run > Edit Run Settings...), plus the overrides
/// ScratchRunner adds (environment, working directory, timeout). Everything
/// `None`/empty means "use the language's configuration".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunSettings {
    /// Run command override (`CodeRunner:RunCommand`).
    pub run_command: Option<String>,
    /// Compile flags override (`CodeRunner:CompilerFlags`).
    pub compile_flags: Option<String>,
    /// Arguments as typed (`CodeRunner:Arguments`): split like a shell does and
    /// appended to the run command, never expanded. See
    /// [`RunSettings::argument_list`].
    pub arguments: String,
    /// Program Input (`CodeRunner:Input`).
    pub input: Option<ProgramInput>,
    /// Deliver the input as `CR_INPUT` (a file path) instead of redirecting it
    /// to stdin.
    pub input_via_environment: bool,
    /// Extra environment variables for this run (above the Shell Variables,
    /// below `CR_*`).
    pub environment: BTreeMap<String, String>,
    /// The working directory rule.
    pub working_directory: WorkingDirectory,
    /// Stop the program after this long (`None` = no limit). Only the program is
    /// timed, not the compile stages.
    #[serde(with = "opt_millis")]
    pub timeout: Option<Duration>,
}

mod opt_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(d) => s.serialize_some(&(d.as_millis() as u64)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_millis))
    }
}

impl RunSettings {
    /// Whether any per-file override is in effect (the toolbar's "Custom run
    /// settings in effect" state).
    pub fn is_customized(&self) -> bool {
        self.run_command.is_some()
            || self.compile_flags.is_some()
            || !self.arguments.trim().is_empty()
            || self.input.is_some()
            || !self.environment.is_empty()
            || self.working_directory != WorkingDirectory::FileDirectory
            || self.timeout.is_some()
    }

    /// The arguments as a list, split like a shell does (quotes and escapes
    /// honoured, no expansion): the "Argument list preview".
    ///
    /// # Errors
    ///
    /// [`RunError::InvalidArguments`] for an unterminated quote.
    pub fn argument_list(&self) -> Result<Vec<String>, RunError> {
        split_words(&self.arguments).map_err(|error| RunError::InvalidArguments(error.to_string()))
    }
}

/// The large-output guard and the stop sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLimits {
    /// Emit [`crate::RunEvent::LargeOutput`] once the program printed this many
    /// bytes (`None` = never).
    pub warn_output_bytes: Option<u64>,
    /// Drop output past this many bytes ([`crate::RunEvent::Truncated`]).
    pub max_output_bytes: u64,
    /// Stop reading the program's output when the warning threshold is reached,
    /// so the program blocks until [`crate::RunHandle::allow_large_output`] or
    /// [`crate::RunHandle::cancel`] (CodeRunner's "Large output detected. Would you
    /// like to stop the run?" alert). Off by default: an app that ignores
    /// [`crate::RunEvent::LargeOutput`] would otherwise hang the run.
    pub pause_at_warning: bool,
    /// Stop the run when the limit is hit (CodeRunner: "Run stopped due to
    /// excessively large output."); otherwise keep running and discard output.
    pub stop_at_limit: bool,
    /// How long a stopped process has to exit after SIGTERM before it gets
    /// SIGKILL.
    pub kill_grace: Duration,
    /// Stop a compile stage after this long (`None` = no limit).
    pub compile_timeout: Option<Duration>,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            warn_output_bytes: Some(16 * 1024 * 1024),
            max_output_bytes: 64 * 1024 * 1024,
            pause_at_warning: false,
            stop_at_limit: true,
            kill_grace: Duration::from_secs(2),
            compile_timeout: None,
        }
    }
}

/// Whether to look for an installed developer directory or assume one (for
/// tests and for the app's own cache).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeveloperDir {
    /// Ask `xcode-select -p` (honouring `DEVELOPER_DIR`), cached
    /// ([`crate::developer_dir`]).
    #[default]
    Detect,
    /// Use this answer: `Some(dir)` = installed, `None` = no developer tools.
    Assume(Option<PathBuf>),
}

/// How the environment of the program is seeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InheritEnvironment {
    /// A small whitelist of the app's environment: `HOME`, `USER`, `LOGNAME`,
    /// `TMPDIR`, `SHELL`, `LANG`, `LC_*`, `SSH_AUTH_SOCK`,
    /// `__CF_USER_TEXT_ENCODING`. Deterministic, whatever launched the app.
    #[default]
    Minimal,
    /// Everything the app has.
    All,
}

/// Engine options of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// Output guard and stop sequence.
    pub limits: RunLimits,
    /// The terminal the program sees. Its `rows`/`cols` are the initial size
    /// ([`crate::RunHandle::resize`] changes it).
    pub tty: Tty,
    /// Put the program's stderr on the pty too, merged with stdout (as CodeRunner
    /// does): ordering between the streams is then exact, but every byte is tagged
    /// [`crate::Stream::Stdout`]. Off by default so that stream tags are exact.
    /// Note that Python's `input(prompt)` writes the prompt to stderr when stdin
    /// is a terminal, so with separate streams it arrives tagged as stderr.
    pub merge_stderr: bool,
    /// Kill what the program left in its process group after a *normal* exit
    /// (cancel, timeout and the output limit always kill the whole group).
    pub kill_leftover_processes: bool,
    /// Where run directories are made (`None` = the system temp directory).
    pub temp_root: Option<PathBuf>,
    /// Keep the run directory afterwards (debugging aid).
    pub keep_temp: bool,
    /// Developer directory detection.
    pub developer_dir: DeveloperDir,
    /// How the environment is seeded.
    pub inherit_environment: InheritEnvironment,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            limits: RunLimits::default(),
            tty: Tty::default(),
            merge_stderr: false,
            kill_leftover_processes: false,
            temp_root: None,
            keep_temp: false,
            developer_dir: DeveloperDir::Detect,
            inherit_environment: InheritEnvironment::Minimal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_coderunners() {
        let shell = ShellSettings::default();
        assert_eq!(shell.shell, "/bin/bash");
        assert!(!shell.login_shell);
        assert_eq!(shell.path, PathSetting::Automatic);
        assert_eq!(shell.variables["TERM"], "dumb");
        assert_eq!(shell.variables["LC_CTYPE"], "UTF-8");
        assert!(shell.variables.contains_key("TERM_PROGRAM"));
        assert!(shell.compiler_output_in_temp_dir);
    }

    #[test]
    fn a_custom_path_is_used_as_it_is() {
        let shell = ShellSettings {
            path: PathSetting::Custom("/x:/y".into()),
            ..ShellSettings::default()
        };
        assert_eq!(shell.base_path(), "/x:/y");
    }

    #[test]
    fn arguments_are_split_like_a_shell() {
        let settings = RunSettings {
            arguments: r#"one "two words" 'three four' five\ six"#.into(),
            ..RunSettings::default()
        };
        assert_eq!(
            settings.argument_list().unwrap(),
            ["one", "two words", "three four", "five six"]
        );
        let broken = RunSettings {
            arguments: "\"open".into(),
            ..RunSettings::default()
        };
        assert!(matches!(
            broken.argument_list(),
            Err(RunError::InvalidArguments(_))
        ));
    }

    #[test]
    fn customization_is_detected() {
        let mut settings = RunSettings::default();
        assert!(!settings.is_customized());
        settings.arguments = "  ".into();
        assert!(!settings.is_customized());
        settings.arguments = "-v".into();
        assert!(settings.is_customized());
        let settings = RunSettings {
            timeout: Some(Duration::from_secs(1)),
            ..RunSettings::default()
        };
        assert!(settings.is_customized());
    }

    #[test]
    fn settings_round_trip_through_serde() {
        let settings = RunSettings {
            run_command: Some("python3 -u $filename".into()),
            input: Some(ProgramInput::Text("1 2".into())),
            working_directory: WorkingDirectory::Custom("/tmp".into()),
            timeout: Some(Duration::from_millis(1500)),
            ..RunSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("1500"), "{json}");
        assert_eq!(
            serde_json::from_str::<RunSettings>(&json).unwrap(),
            settings
        );
        // Missing fields take their defaults.
        assert_eq!(
            serde_json::from_str::<RunSettings>("{}").unwrap(),
            RunSettings::default()
        );
        let shell = ShellSettings::default();
        let json = serde_json::to_string(&shell).unwrap();
        assert_eq!(serde_json::from_str::<ShellSettings>(&json).unwrap(), shell);
        let custom = ShellSettings {
            path: PathSetting::Custom("/a".into()),
            ..ShellSettings::default()
        };
        let json = serde_json::to_string(&custom).unwrap();
        assert_eq!(
            serde_json::from_str::<ShellSettings>(&json).unwrap(),
            custom
        );
    }
}
