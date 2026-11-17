//! The environment contract: what variables the compile stages, the run
//! command and the program see.
//!
//! Layers, later ones win (except that `CR_*` always have the last word over
//! per-run environment settings):
//!
//! 1. the inherited environment ([`InheritEnvironment`]: `HOME`, `USER`,
//!    `LOGNAME`, `TMPDIR`, `SHELL`, `LANG`, `LC_*`, `SSH_AUTH_SOCK`, ...);
//! 2. `PATH` (Preferences > Shell: automatic or custom, plus the language's
//!    `path_prepend` / `path_append`), `HOME`, `DEVELOPER_DIR` (from
//!    `xcode-select -p`), `MACOSX_DEPLOYMENT_TARGET` when configured;
//! 3. the Shell Variables table (`LC_CTYPE=UTF-8`, `TERM=dumb`,
//!    `TERM_PROGRAM=ScratchRunner` by default);
//! 4. the language's `[run.env]` variables and the run's exported variables:
//!    `filename`, `compiler`, ... and the `CR_*` contract: `CR_FILENAME`,
//!    `CR_FILE`, `CR_TMPDIR`, `CR_SUGGESTED_OUTPUT_FILE`, `CR_DEBUGGING`,
//!    `CR_ENCODING`, `CR_ENCODING_NAME`, `CR_RUNID`, `CR_LANGUAGE_DIR`,
//!    `CR_SCRIPTS_DIR`, `CR_UNSAVED_DIR`;
//! 5. the per-run environment of the Run Settings;
//! 6. the run's own contract variables: `CR_RUN_COMMAND`, `CR_INPUT` (when the
//!    input is delivered through the environment), `CR_VERSION`, `CR_DIAGNOSTICS*`
//!    is the diagnostics crate's, and the `CR_*` of layer 4 again;
//! 7. `extra_env` of the request (`CR_DEBUGGER_DIR`, ...).

use std::collections::BTreeMap;
use std::path::Path;

use crate::{InheritEnvironment, ShellSettings};

/// The variables of the app's own environment that a program inherits in
/// [`InheritEnvironment::Minimal`] mode.
pub const INHERITED_VARIABLES: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_COLLATE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "SSH_AUTH_SOCK",
    "__CF_USER_TEXT_ENCODING",
];

/// The inherited layer.
pub fn inherited_environment(mode: InheritEnvironment) -> BTreeMap<String, String> {
    match mode {
        InheritEnvironment::All => std::env::vars().collect(),
        InheritEnvironment::Minimal => INHERITED_VARIABLES
            .iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| ((*name).to_owned(), value))
            })
            .collect(),
    }
}

/// Everything [`build_environment`] needs.
#[derive(Debug, Clone)]
pub struct EnvironmentInputs<'a> {
    /// Preferences > Shell.
    pub shell: &'a ShellSettings,
    /// How the environment is seeded.
    pub inherit: InheritEnvironment,
    /// The final `PATH` (custom or automatic, with the language's additions).
    pub path: &'a str,
    /// The developer directory (`DEVELOPER_DIR`), when tools are installed.
    pub developer_dir: Option<&'a Path>,
    /// The run's exported variables and the language's `env`
    /// ([`scratch_language::RunPlan::process_env`]).
    pub plan_env: &'a BTreeMap<String, String>,
    /// The per-run environment of the Run Settings.
    pub run_env: &'a BTreeMap<String, String>,
    /// `CR_RUN_COMMAND`: the run command text of this run.
    pub run_command: &'a str,
    /// `CR_INPUT`: the Program Input file, when delivered through the
    /// environment.
    pub input_file: Option<&'a Path>,
    /// `CR_VERSION`.
    pub app_version: Option<&'a str>,
    /// The app's last-word variables.
    pub extra_env: &'a BTreeMap<String, String>,
}

/// Builds the complete environment of a process of the run (see the module
/// documentation for the layers).
pub fn build_environment(inputs: &EnvironmentInputs<'_>) -> BTreeMap<String, String> {
    let mut env = inherited_environment(inputs.inherit);
    if !env.contains_key("HOME")
        && let Some(home) = crate::path::home_dir()
    {
        env.insert("HOME".to_owned(), home.to_string_lossy().into_owned());
    }
    env.insert("PATH".to_owned(), inputs.path.to_owned());
    if let Some(dir) = inputs.developer_dir {
        env.insert(
            "DEVELOPER_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
    }
    if let Some(target) = &inputs.shell.deployment_target {
        env.insert("MACOSX_DEPLOYMENT_TARGET".to_owned(), target.clone());
    }
    for (name, value) in &inputs.shell.variables {
        if name != "PATH" && !name.is_empty() {
            env.insert(name.clone(), value.clone());
        }
    }
    for (name, value) in inputs.plan_env {
        if name != "PATH" {
            env.insert(name.clone(), value.clone());
        }
    }
    for (name, value) in inputs.run_env {
        if !name.starts_with("CR_") && !name.is_empty() {
            env.insert(name.clone(), value.clone());
        }
    }
    for (name, value) in inputs.plan_env {
        if name.starts_with("CR_") {
            env.insert(name.clone(), value.clone());
        }
    }
    env.insert("CR_RUN_COMMAND".to_owned(), inputs.run_command.to_owned());
    if let Some(input) = inputs.input_file {
        env.insert("CR_INPUT".to_owned(), input.to_string_lossy().into_owned());
    }
    if let Some(version) = inputs.app_version {
        env.insert("CR_VERSION".to_owned(), version.to_owned());
    }
    for (name, value) in inputs.extra_env {
        env.insert(name.clone(), value.clone());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn layers_apply_in_order_and_cr_variables_cannot_be_overridden_per_run() {
        let shell = ShellSettings::default();
        let plan_env = map(&[
            ("CR_FILENAME", "a.py"),
            ("filename", "a.py"),
            ("FROM_LANGUAGE", "1"),
        ]);
        let run_env = map(&[
            ("CR_FILENAME", "evil"),
            ("MINE", "yes"),
            ("TERM", "xterm-256color"),
            ("FROM_LANGUAGE", "2"),
        ]);
        let extra = map(&[("CR_DEBUGGER_DIR", "/dbg")]);
        let env = build_environment(&EnvironmentInputs {
            shell: &shell,
            inherit: InheritEnvironment::Minimal,
            path: "/x:/y",
            developer_dir: Some(Path::new("/Dev")),
            plan_env: &plan_env,
            run_env: &run_env,
            run_command: "python3 $filename",
            input_file: Some(Path::new("/tmp/cr_input_1.txt")),
            app_version: Some("0.1"),
            extra_env: &extra,
        });
        assert_eq!(env["PATH"], "/x:/y");
        assert_eq!(env["DEVELOPER_DIR"], "/Dev");
        assert_eq!(env["LC_CTYPE"], "UTF-8");
        assert_eq!(env["TERM_PROGRAM"], "ScratchRunner");
        assert_eq!(env["TERM"], "xterm-256color"); // per-run beats Shell Variables
        assert_eq!(env["FROM_LANGUAGE"], "2");
        assert_eq!(env["MINE"], "yes");
        assert_eq!(env["CR_FILENAME"], "a.py");
        assert_eq!(env["CR_RUN_COMMAND"], "python3 $filename");
        assert_eq!(env["CR_INPUT"], "/tmp/cr_input_1.txt");
        assert_eq!(env["CR_VERSION"], "0.1");
        assert_eq!(env["CR_DEBUGGER_DIR"], "/dbg");
        assert!(env.contains_key("HOME"));
        assert!(!env.contains_key("MACOSX_DEPLOYMENT_TARGET"));
    }

    #[test]
    fn the_minimal_environment_is_a_whitelist() {
        let minimal = inherited_environment(InheritEnvironment::Minimal);
        assert!(
            minimal
                .keys()
                .all(|k| INHERITED_VARIABLES.contains(&k.as_str()))
        );
        assert!(!minimal.contains_key("PATH"));
        let all = inherited_environment(InheritEnvironment::All);
        assert!(all.len() >= minimal.len());
    }

    #[test]
    fn deployment_target_and_path_variable_rules() {
        let mut shell = ShellSettings {
            deployment_target: Some("10.13".into()),
            ..ShellSettings::default()
        };
        shell.variables.insert("PATH".into(), "/ignored".into());
        let empty = BTreeMap::new();
        let env = build_environment(&EnvironmentInputs {
            shell: &shell,
            inherit: InheritEnvironment::Minimal,
            path: "/p",
            developer_dir: None,
            plan_env: &empty,
            run_env: &empty,
            run_command: "",
            input_file: None,
            app_version: None,
            extra_env: &empty,
        });
        assert_eq!(env["MACOSX_DEPLOYMENT_TARGET"], "10.13");
        assert_eq!(env["PATH"], "/p");
        assert!(!env.contains_key("DEVELOPER_DIR"));
        assert!(!env.contains_key("CR_INPUT"));
    }
}
