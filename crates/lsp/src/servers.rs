//! The data-driven language-server catalogue, discovery, and overrides.
//!
//! [`ServerDefinition`]s are loaded from `data/servers.toml` (embedded in the binary) and can be
//! replaced wholesale with [`ServerCatalog::from_toml`] for tests or advanced users. Each server
//! lists the ScratchRunner language ids it serves, the command names to try, the project markers
//! that identify its root, the file it gets for unsaved documents, and the scaffolding a
//! single-file project needs. [`ServerOverride`]s (from preferences) can disable a server or
//! replace its command, arguments, settings and search paths.
//!
//! Programs are looked up on the supplied `PATH` first and then in the usual install locations
//! (`/opt/homebrew/bin`, `~/.cargo/bin`, `~/go/bin`, npm global bins, ...), so a server installed
//! after the app started is still found.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::ServerCommand;
use crate::error::{LspError, Result};
use crate::workspace::Scaffold;

/// The catalogue shipped with the app, embedded from `data/servers.toml`.
const BUILTIN_CATALOG: &str = include_str!("../data/servers.toml");

/// One language server the app knows how to start.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServerDefinition {
    /// Stable identifier, also the key of user overrides (`[lsp.python]`).
    pub id: String,
    /// Name for status messages.
    pub name: String,
    /// Executable names to try, in order.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Arguments, with `{root}` and `{cache}` substituted when the server starts.
    #[serde(default)]
    pub args: Vec<String>,
    /// ScratchRunner language id -> protocol `languageId`.
    #[serde(default)]
    pub languages: BTreeMap<String, String>,
    /// The `initializationOptions` object.
    #[serde(default)]
    pub initialization_options: Option<Value>,
    /// The settings answered to `workspace/configuration` and sent with
    /// `workspace/didChangeConfiguration`.
    #[serde(default)]
    pub settings: Option<Value>,
    /// What to tell the user when no command is found.
    #[serde(default)]
    pub install_hint: String,
    /// Files or directories that mark the project root, walking up from the document. A marker
    /// starting with `*.` matches a file suffix (`*.csproj`).
    #[serde(default)]
    pub root_markers: Vec<String>,
    /// Path of an unsaved document inside its virtual workspace, when it is the same for all
    /// languages.
    #[serde(default)]
    pub virtual_path: Option<String>,
    /// Per-language virtual paths, when languages need different extensions.
    #[serde(default)]
    pub virtual_paths: BTreeMap<String, String>,
    /// Project file to create in a virtual workspace.
    #[serde(default)]
    pub scaffold: Scaffold,
    /// Extra directories to search for the executable, besides the standard install locations.
    #[serde(default)]
    pub search_paths: Vec<String>,
    /// Whether the definition is enabled; user overrides can turn it off too.
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl ServerDefinition {
    /// The protocol `languageId` for a ScratchRunner language id, if this server serves it.
    pub fn language_id(&self, language: &str) -> Option<&str> {
        self.languages.get(language).map(String::as_str)
    }

    /// The path an unsaved document of `language` gets in its virtual workspace.
    pub fn virtual_path(&self, language: &str) -> Option<&str> {
        self.virtual_paths
            .get(language)
            .map(String::as_str)
            .or(self.virtual_path.as_deref())
    }
}

/// A set of server definitions, indexed for lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerCatalog {
    definitions: Vec<ServerDefinition>,
}

#[derive(Deserialize)]
struct RawCatalog {
    #[serde(rename = "server", default)]
    servers: Vec<ServerDefinition>,
}

impl ServerCatalog {
    /// The catalogue shipped with the app.
    pub fn builtin() -> &'static ServerCatalog {
        static BUILTIN: OnceLock<ServerCatalog> = OnceLock::new();
        BUILTIN.get_or_init(|| {
            ServerCatalog::from_toml(BUILTIN_CATALOG)
                .unwrap_or_else(|error| panic!("the embedded server catalogue is invalid: {error}"))
        })
    }

    /// Loads a catalogue from TOML.
    pub fn from_toml(text: &str) -> Result<Self> {
        let raw: RawCatalog =
            toml::from_str(text).map_err(|error| LspError::Config(format!("servers: {error}")))?;
        let catalog = Self {
            definitions: raw.servers,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Loads a catalogue from a file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| LspError::Config(format!("{}: {error}", path.display())))?;
        Self::from_toml(&text)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for definition in &self.definitions {
            if definition.id.is_empty() || definition.name.is_empty() {
                return Err(LspError::Config(
                    "a server definition has no id or name".to_owned(),
                ));
            }
            if definition.commands.is_empty() {
                return Err(LspError::Config(format!(
                    "server {} has no commands",
                    definition.id
                )));
            }
            if definition.languages.is_empty() {
                return Err(LspError::Config(format!(
                    "server {} serves no languages",
                    definition.id
                )));
            }
            if !seen.insert(definition.id.as_str()) {
                return Err(LspError::Config(format!(
                    "duplicate server id {}",
                    definition.id
                )));
            }
        }
        Ok(())
    }

    /// The definitions, in catalogue order.
    pub fn definitions(&self) -> &[ServerDefinition] {
        &self.definitions
    }

    /// The definition with `id`.
    pub fn get(&self, id: &str) -> Option<&ServerDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
    }

    /// The definition serving a ScratchRunner language id.
    pub fn server_for_language(&self, language: &str) -> Option<&ServerDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.languages.contains_key(language))
    }

    /// Every language id served by any definition.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.definitions
            .iter()
            .flat_map(|definition| definition.languages.keys().map(String::as_str))
    }
}

/// What user preferences say about one server (`[lsp.<id>]`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerOverride {
    /// `false` turns the server off.
    pub enabled: Option<bool>,
    /// A program, instead of the catalogue's command names.
    pub command: Option<String>,
    /// Arguments, instead of the catalogue's.
    pub args: Option<Vec<String>>,
    /// `initializationOptions`, instead of the catalogue's.
    pub initialization_options: Option<Value>,
    /// Settings, instead of the catalogue's.
    pub settings: Option<Value>,
    /// Extra search directories.
    pub search_paths: Option<Vec<String>>,
}

/// A server ready to start.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSpec {
    /// Its catalogue id.
    pub key: String,
    /// Its name for status messages.
    pub name: String,
    /// The workspace root it starts in.
    pub root: PathBuf,
    /// How to start it.
    pub command: ServerCommand,
}

/// Finds an executable named `program`.
///
/// `path` is searched first (a `:`-separated list), then [`default_search_paths`], then `extra`
/// (a definition's or user's extra directories). A `program` containing `/` is used as a path
/// (with `~` expanded) and must be executable.
pub fn find_program(program: &str, path: &str, extra: &[String]) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') {
        let candidate = expand_tilde(program);
        return is_executable(&candidate).then_some(candidate);
    }
    let mut directories: Vec<PathBuf> = path
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect();
    directories.extend(default_search_paths());
    directories.extend(extra.iter().map(|entry| expand_tilde(entry)));
    let mut seen = std::collections::HashSet::new();
    directories
        .into_iter()
        .filter(|directory| seen.insert(directory.clone()))
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
}

/// The directories language servers are commonly installed in, after the user's `PATH`.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/local/bin"),
        PathBuf::from("/opt/homebrew/lib/node_modules/.bin"),
        PathBuf::from("/usr/local/lib/node_modules/.bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from(
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin",
        ),
    ];
    if let Some(home) = home_directory() {
        for relative in [
            ".cargo/bin",
            ".local/bin",
            ".npm-global/bin",
            ".npm/bin",
            ".volta/bin",
            ".asdf/shims",
            ".bun/bin",
            ".deno/bin",
            "go/bin",
            "Library/pnpm",
            "Library/Application Support/mise/shims",
        ] {
            directories.push(home.join(relative));
        }
        directories.push(home.join(".npm-global/lib/node_modules/.bin"));
    }
    directories
}

/// `~/...` expanded to an absolute path; other paths unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_directory().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home_directory()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Walks up from `start` (a file's directory or a file) and returns the first ancestor that
/// contains one of `markers`.
pub fn find_project_root(start: &Path, markers: &[String]) -> Option<PathBuf> {
    let mut directory = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if markers
            .iter()
            .any(|marker| marker_matches(&directory, marker))
        {
            return Some(directory);
        }
        if !directory.pop() {
            return None;
        }
    }
}

fn marker_matches(directory: &Path, marker: &str) -> bool {
    if let Some(suffix) = marker.strip_prefix('*') {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        return entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(suffix));
    }
    directory.join(marker).exists()
}

/// Whether a server is turned off, missing, or installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerAvailability {
    /// Turned off in preferences or in its definition.
    Disabled,
    /// No executable was found; the hint says what to install.
    NotInstalled {
        /// What the user can do.
        hint: String,
    },
    /// The executable that would be started.
    Installed {
        /// Its path.
        program: PathBuf,
    },
}

/// Finds a definition's executable, honouring the user override. This is the cheap check the
/// preferences pane uses; it does not start anything.
pub fn availability(
    definition: &ServerDefinition,
    overridden: &ServerOverride,
    path: &str,
) -> ServerAvailability {
    if !definition.enabled || overridden.enabled == Some(false) {
        return ServerAvailability::Disabled;
    }
    let commands: Vec<String> = match &overridden.command {
        Some(command) => vec![command.clone()],
        None => definition.commands.clone(),
    };
    let extra: Vec<String> = overridden
        .search_paths
        .clone()
        .unwrap_or_else(|| definition.search_paths.clone());
    match commands
        .iter()
        .find_map(|name| find_program(name, path, &extra))
    {
        Some(program) => ServerAvailability::Installed { program },
        None => ServerAvailability::NotInstalled {
            // The status bar shows "{name} is not installed. {hint}"; keeping
            // just the install hint avoids repeating the server name and the
            // (tried …) list, which made the notice unreadably long.
            hint: definition.install_hint.trim().to_owned(),
        },
    }
}

/// Resolves a definition (and the user's override) into something executable.
///
/// `status` is `Ok` with the spec when the program was found; otherwise it is the typed reason
/// the server cannot start (disabled or not installed), ready to be shown in the status bar.
pub fn resolve(
    definition: &ServerDefinition,
    overridden: &ServerOverride,
    path: &str,
    environment: &[(String, String)],
    root: &Path,
    cache: &Path,
) -> std::result::Result<ServerSpec, crate::manager::ServerStatus> {
    use crate::manager::ServerStatus;
    let program = match availability(definition, overridden, path) {
        ServerAvailability::Installed { program } => program,
        ServerAvailability::Disabled => return Err(ServerStatus::Disabled),
        ServerAvailability::NotInstalled { hint } => {
            return Err(ServerStatus::NotInstalled { hint });
        }
    };
    let extra: Vec<String> = overridden
        .search_paths
        .clone()
        .unwrap_or_else(|| definition.search_paths.clone());
    let args = overridden
        .args
        .clone()
        .unwrap_or_else(|| definition.args.clone())
        .into_iter()
        .map(|arg| {
            arg.replace("{root}", &root.to_string_lossy())
                .replace("{cache}", &cache.to_string_lossy())
        })
        .collect();
    let mut env = environment.to_vec();
    env.retain(|(name, _)| name != "PATH");
    let mut directories: Vec<PathBuf> = path
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect();
    directories.extend(default_search_paths());
    directories.extend(extra.iter().map(|entry| expand_tilde(entry)));
    let mut seen = std::collections::HashSet::new();
    let joined = directories
        .into_iter()
        .filter(|directory| seen.insert(directory.clone()))
        .map(|directory| directory.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    env.push(("PATH".to_owned(), joined));
    Ok(ServerSpec {
        key: definition.id.clone(),
        name: definition.name.clone(),
        root: root.to_path_buf(),
        command: ServerCommand {
            program,
            args,
            cwd: root.to_path_buf(),
            env,
        },
    })
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalogue_is_valid_and_serves_the_core_languages() {
        let catalog = ServerCatalog::builtin();
        assert!(catalog.definitions().len() >= 20);
        for (language, server) in [
            ("python", "python"),
            ("rust", "rust"),
            ("go", "go"),
            ("javascript-node", "typescript"),
            ("typescript-node", "typescript"),
            ("shell", "shell"),
            ("swift", "swift"),
            ("c", "clangd"),
            ("objective-cpp", "clangd"),
            ("yaml", "yaml"),
        ] {
            assert_eq!(
                catalog
                    .server_for_language(language)
                    .map(|def| def.id.as_str()),
                Some(server),
                "{language}"
            );
        }
        let python = catalog.get("python").unwrap();
        assert_eq!(python.language_id("python"), Some("python"));
        assert_eq!(python.virtual_path("python"), Some("main.py"));
        let mut ids = std::collections::HashSet::new();
        for definition in catalog.definitions() {
            assert!(ids.insert(definition.id.as_str()));
            for language in definition.languages.keys() {
                assert!(
                    definition.virtual_path(language).is_some(),
                    "{} has no virtual path for {language}",
                    definition.id
                );
            }
        }
    }

    #[test]
    fn rejects_bad_catalogues() {
        assert!(ServerCatalog::from_toml("not toml").is_err());
        assert!(
            ServerCatalog::from_toml(
                "[[server]]\nid = \"x\"\nname = \"X\"\ncommands = [\"x\"]\n[server.languages]\na = \"a\"\n\n[[server]]\nid = \"x\"\nname = \"X\"\ncommands = [\"y\"]\n[server.languages]\nb = \"b\"\n"
            )
            .is_err()
        );
        assert!(ServerCatalog::from_toml("[[server]]\nid = \"x\"\nname = \"X\"\n").is_err());
    }

    #[test]
    fn finds_executables_in_extra_locations() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("my-server");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = find_program(
            "my-server",
            "",
            &[dir.path().to_string_lossy().into_owned()],
        );
        assert_eq!(found.as_deref(), Some(program.as_path()));
        assert_eq!(
            find_program("my-server", dir.path().to_str().unwrap(), &[]),
            Some(program.clone())
        );
        assert_eq!(
            find_program(program.to_str().unwrap(), "", &[]),
            Some(program)
        );
        assert!(find_program("definitely-not-a-language-server", "", &[]).is_none());
        assert!(find_program("", "", &[]).is_none());
    }

    #[test]
    fn walks_up_to_project_markers() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            find_project_root(&nested.join("main.rs"), &["Cargo.toml".to_owned()]),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(find_project_root(&nested, &["go.mod".to_owned()]), None);
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(dir2.path().join("App.csproj"), "").unwrap();
        assert_eq!(
            find_project_root(dir2.path(), &["*.csproj".to_owned()]),
            Some(dir2.path().to_path_buf())
        );
    }

    #[test]
    fn resolves_overrides_environment_and_arg_templates() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-server");
        std::fs::write(&program, "").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let definition = ServerDefinition {
            id: "fake".into(),
            name: "Fake".into(),
            commands: vec!["not-installed".into()],
            args: vec!["--root={root}".into(), "{cache}/x".into()],
            languages: BTreeMap::from([("fake".to_owned(), "fake".to_owned())]),
            initialization_options: None,
            settings: None,
            install_hint: "install it".into(),
            root_markers: vec![],
            virtual_path: Some("main.fake".into()),
            virtual_paths: BTreeMap::new(),
            scaffold: Scaffold::None,
            search_paths: vec![],
            enabled: true,
        };
        let overridden = ServerOverride {
            enabled: None,
            command: Some(program.to_string_lossy().into_owned()),
            args: None,
            initialization_options: None,
            settings: None,
            search_paths: None,
        };
        let root = dir.path().join("project");
        let spec = resolve(
            &definition,
            &overridden,
            "/usr/bin:/bin",
            &[
                ("HOME".into(), "/tmp/home".into()),
                ("PATH".into(), "old".into()),
            ],
            &root,
            Path::new("/tmp/cache"),
        )
        .unwrap();
        assert_eq!(spec.key, "fake");
        assert_eq!(spec.command.program, program);
        assert_eq!(
            spec.command.args,
            [
                format!("--root={}", root.display()),
                "/tmp/cache/x".to_owned()
            ]
        );
        assert_eq!(spec.command.cwd, root);
        assert!(spec.command.env.iter().any(|(name, _)| name == "HOME"));
        let path = spec
            .command
            .env
            .iter()
            .find(|(name, _)| name == "PATH")
            .unwrap();
        assert!(path.1.contains("/opt/homebrew/bin"));
        assert!(!path.1.contains(":old"));

        let disabled = ServerOverride {
            enabled: Some(false),
            ..ServerOverride::default()
        };
        assert_eq!(
            availability(&definition, &disabled, ""),
            ServerAvailability::Disabled
        );
        assert_eq!(
            resolve(&definition, &disabled, "", &[], &root, Path::new("/tmp")),
            Err(crate::manager::ServerStatus::Disabled)
        );
        assert!(matches!(
            availability(&definition, &overridden, "/usr/bin:/bin"),
            ServerAvailability::Installed { .. }
        ));
        let missing = resolve(
            &definition,
            &ServerOverride::default(),
            "",
            &[],
            &root,
            Path::new("/tmp"),
        );
        assert!(
            matches!(missing, Err(crate::manager::ServerStatus::NotInstalled { hint }) if hint.contains("install it"))
        );
    }

    #[test]
    fn override_deserializes_from_preferences() {
        let overridden: ServerOverride = toml::from_str(
            "enabled = false\ncommand = \"/x/y\"\nargs = [\"--stdio\"]\nsearch_paths = [\"~/bin\"]",
        )
        .unwrap();
        assert_eq!(overridden.enabled, Some(false));
        assert_eq!(overridden.command.as_deref(), Some("/x/y"));
        assert_eq!(
            overridden.args.as_deref(),
            Some(&["--stdio".to_owned()][..])
        );
        assert_eq!(
            overridden.search_paths.as_deref(),
            Some(&["~/bin".to_owned()][..])
        );
    }
}
