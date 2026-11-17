//! Debugger adapter model.
//!
//! An adapter is a small TOML document that describes how to drive a
//! command-line debugger: how to start it, what its prompt looks like, which
//! command to send for each operation, and which regular expressions parse the
//! replies.  This is our own format; it is deliberately *not* CodeRunner's
//! proprietary `.crDebugger` bundle format, although it covers the same ideas.
//!
//! Built-in adapters live in `src/adapters/*.toml` and are embedded in the
//! binary.  Users can drop additional `*.toml` files into a directory loaded
//! with [`AdapterRegistry::load_user_dir`]; an adapter whose id matches a
//! built-in overrides it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fancy_regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::DebugError;

const BUILTIN_LLDB: &str = include_str!("adapters/lldb.toml");
const BUILTIN_PDB: &str = include_str!("adapters/pdb.toml");
const BUILTIN_NODE: &str = include_str!("adapters/node.toml");
const BUILTIN_JDB: &str = include_str!("adapters/jdb.toml");
const BUILTIN_BASHDB: &str = include_str!("adapters/bashdb.toml");
const BUILTIN_PERL: &str = include_str!("adapters/perl.toml");
const BUILTIN_PHPDBG: &str = include_str!("adapters/phpdbg.toml");

/// Ids of the adapters embedded in the crate.
pub const BUILTIN_ADAPTER_IDS: &[&str] =
    &["lldb", "pdb", "node", "jdb", "bashdb", "perl", "phpdbg"];

/// Which built-in parser family should be used for a debugger's text output.
///
/// The adapter's regular expressions are used for the generic family and as
/// supplements for the others; the specialised families know about the
/// line-oriented layout produced by that debugger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParserKind {
    /// Parse stacks and variables with the adapter's named-group regexes.
    #[default]
    Generic,
    /// LLDB's `bt all` / `frame variable` output.
    Lldb,
    /// Python `pdb`'s `where` output and JSON variables helper.
    Pdb,
    /// `node inspect` text mode.
    Node,
    /// `jdb` text mode.
    Jdb,
    /// `bashdb` text mode.
    Bashdb,
    /// `perl -d` text mode.
    Perl,
}

/// How the debugger echoes the command we typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EchoMode {
    /// No echo is produced (pipes with no line editor).
    #[default]
    None,
    /// The command appears as its own line (LLDB with `TERM=dumb`).
    Line,
    /// Every line beginning with `echo_prefix` is a readline redraw.
    Prefix,
}

/// Options that tune the session without changing its command set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterOptions {
    /// A prompt means the debuggee is paused (pdb, bashdb, node).  LLDB keeps
    /// its command interpreter responsive while the program runs, so it does
    /// not set this.
    pub prompt_means_paused: bool,
    /// The debugger does not read commands while the program is running, so
    /// commands must wait for a stop (pdb, node).
    pub requires_paused_commands: bool,
    /// Run the debuggee with its own terminal (PTY), giving the debugger the
    /// slave path as `{program_tty}` (LLDB).
    pub program_tty: bool,
    /// Default pointer chain depth for `frame variable -P`.
    pub ptr_depth: u8,
    /// Default child depth for variable queries.
    pub default_depth: u8,
    /// Maximum number of children reported per aggregate.
    pub max_children: usize,
    /// Maximum number of characters of a leaf `repr`.
    pub max_value_chars: usize,
    /// The adapter supports pausing a running program.
    pub supports_pause: bool,
    /// Send SIGINT to the debugger's process group for `pause`.  When false
    /// and a `pause` command is defined, the command is sent instead.
    pub pause_with_signal: bool,
    /// How command echoes look in the transcript.
    pub echo: EchoMode,
    /// Prefix used by [`EchoMode::Prefix`].
    pub echo_prefix: String,
    /// Program output is prefixed with this marker (node's `< ` lines).
    pub program_prefix: Option<String>,
    /// Hide frames whose file contains one of these strings (adapter-level
    /// defaults; `[parsers] hide_frames` is the usual place).
    pub hide_frames: Vec<String>,
    /// Commands to run once the first prompt is seen, before breakpoints and
    /// launch.
    pub setup_commands: Vec<String>,
    /// Adapter supplied installation hint, shown when the tool is missing.
    pub install_hint: Option<String>,
}

impl Default for AdapterOptions {
    fn default() -> Self {
        Self {
            prompt_means_paused: false,
            requires_paused_commands: false,
            program_tty: false,
            ptr_depth: 1,
            default_depth: 2,
            max_children: 100,
            max_value_chars: 1500,
            supports_pause: false,
            pause_with_signal: true,
            echo: EchoMode::None,
            echo_prefix: String::new(),
            program_prefix: None,
            hide_frames: Vec::new(),
            setup_commands: Vec::new(),
            install_hint: None,
        }
    }
}

/// How to launch the debugger (and, for attach-style debuggers, the program).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StartSpec {
    /// Executable name (looked up on `PATH`) or an absolute path.
    pub program: String,
    /// Arguments, rendered as templates.  An argument equal to
    /// `{program_args}` splices the debuggee's argument vector.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
    /// Run the debugger on a PTY instead of pipes.  Interactive line editors
    /// (LLDB, node) need this; pdb with pipes is cleaner.
    pub pty: bool,
    /// Name of a built-in startup file to materialise in the session dir and
    /// expose as `{init_file}` (currently `"lldb"`).
    pub init: Option<String>,
    /// Optional debuggee/server command for attach-style debuggers (jdb).
    /// Rendered like `args`; the reserved placeholder `{port}` receives a free
    /// local TCP port.
    pub server: Option<Vec<String>>,
}

/// Command templates for every debugger verb.
///
/// Values in braces are replaced by the session (`{file}`, `{line}`,
/// `{program}`, …).  A missing or empty template means "not supported" and
/// produces a [`DebugError::Unsupported`] event when used.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Commands {
    /// Start the program under the debugger.
    pub launch: String,
    /// Resume after a stop.
    #[serde(rename = "continue")]
    pub continue_cmd: String,
    /// Pause a running program.  When empty and
    /// `options.pause_with_signal` is set, SIGINT is used.
    pub pause: String,
    /// Step over the current statement.
    pub step_over: String,
    /// Step into the current call.
    pub step_in: String,
    /// Step out of the current function.
    pub step_out: String,
    /// Set a breakpoint.  Placeholders: `{file}`, `{file_base}`, `{line}`,
    /// `{file_stem}`, plus the optional `{condition_clause}` and
    /// `{temporary_clause}` fragments.
    pub breakpoint_set: String,
    /// Alternative whole command for a temporary/one-shot breakpoint.  When
    /// absent, `temporary_clause` is appended to `breakpoint_set`.
    pub breakpoint_set_temporary: Option<String>,
    /// Appended when the breakpoint has a condition.
    pub condition_clause: Option<String>,
    /// Appended for temporary breakpoints when
    /// `breakpoint_set_temporary` is absent.
    pub temporary_clause: Option<String>,
    /// Remove one breakpoint (`{file_base}`, `{line}`).
    pub breakpoint_clear: Option<String>,
    /// Remove all breakpoints.
    pub breakpoint_clear_all: Option<String>,
    /// List breakpoints (diagnostics only).
    pub breakpoint_list: Option<String>,
    /// Print the stack ("threads and frames").
    pub backtrace: String,
    /// Select a frame directly, possibly several lines separated by `\n`
    /// (LLDB: `thread select` + `frame select`).
    pub select_frame: Option<String>,
    /// Relative frame selection, up one frame.  `{count}` may be used.
    pub frame_up: Option<String>,
    /// Relative frame selection, down one frame.
    pub frame_down: Option<String>,
    /// List locals/arguments.  Placeholders: `{depth}`, `{ptr_depth}`,
    /// `{show_all}`, `{show_types}`, `{types_flag}`, `{globals_flag}`.
    pub variables: Option<String>,
    /// Fetch the subtree rooted at `{expr}` (lazy expansion).
    pub variables_at: Option<String>,
    /// Evaluate an expression and return its value.
    pub evaluate: String,
    /// Print a language-level description of an expression (`po`).
    pub print_description: Option<String>,
    /// Assign `{value}` to `{path}`.
    pub set_variable: Option<String>,
    /// Leave the debugger cleanly.
    pub quit: Option<String>,
}

/// Optional flag fragments used by the variables command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandFlags {
    /// Appended when types should be displayed (` -T`).
    pub types: String,
    /// Appended when globals should be included (` -g`).
    pub globals: String,
}

/// Output parsers.
///
/// Every regex is Oniguruma-flavoured (`fancy-regex`), the same dialect
/// CodeRunner used.  Named groups are documented by the field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ParserSpec {
    /// Parser family.
    pub kind: ParserKind,
    /// A stop reason line.  Named group `reason` (`stop reason = ...`).
    pub stop_reason: Option<String>,
    /// The statement at which the program stopped.  Named groups `file`,
    /// `line`, optional `function`, `thread`, `frame`.
    pub stop_location: Option<String>,
    /// One stack frame.  Named groups `index`, `name`/`function`, `file`,
    /// `line`, optional `thread` and `current`.
    pub frames: Option<String>,
    /// One thread header.  Named groups `thread`, optional `name`,
    /// `current`.
    pub threads: Option<String>,
    /// A breakpoint hit notice.  Named group `id`.
    pub breakpoint_hit: Option<String>,
    /// A breakpoint-set acknowledgement.  Named group `id`.
    pub breakpoint_set: Option<String>,
    /// One variable line for the generic parser.  Named groups `name`,
    /// optional `type`, `value`, `kind`, `depth`.
    pub variables: Option<String>,
    /// Variables are JSON on a line beginning with this marker (used by the
    /// Python helper).  Takes precedence over `variables`.
    pub variables_marker: Option<String>,
    /// The debugger prints innermost frame last (`pdb`, `perl`, `bashdb`).
    pub reverse_frames: bool,
    /// Hide stack frames whose file path contains one of these strings.
    pub hide_frames: Vec<String>,
}

/// Built-in helper files materialised in the session directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Helpers {
    /// Python modules to write into `<session>/python` and put on
    /// `PYTHONPATH`.  Known value: `"pdbvars"`.
    pub python: Vec<String>,
}

/// A complete debugger adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebuggerAdapter {
    /// Stable id, also the name used in `LanguageDef.debugger` overrides.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Language ids for which this adapter is the default.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Prompt regex, matched anywhere in the transcript; the session treats
    /// it as a command boundary.
    pub prompt: String,
    /// Program exit line; capture group 1 is the exit status.
    #[serde(default)]
    pub termination: Option<String>,
    /// Fatal signal/exception marker.
    #[serde(default)]
    pub crash: Option<String>,
    /// Informational debugger messages echoed into the console.
    #[serde(default)]
    pub message: Option<String>,
    /// Lines to drop from the transcript.
    #[serde(default)]
    pub ignore: Option<String>,
    /// When a stop transcript matches this, immediately continue (internal
    /// debugger frames).  Reserved for future use.
    #[serde(default)]
    pub continue_pattern: Option<String>,
    /// One-line installation hint.
    #[serde(default)]
    pub install_hint: Option<String>,
    /// Startup and process information.
    #[serde(default)]
    pub start: StartSpec,
    /// Command templates.
    #[serde(default)]
    pub commands: Commands,
    /// Output parsers.
    #[serde(default)]
    pub parsers: ParserSpec,
    /// Session options.
    #[serde(default)]
    pub options: AdapterOptions,
    /// Extra variable-command flag fragments.
    #[serde(default)]
    pub flags: CommandFlags,
    /// Built-in helper files.
    #[serde(default)]
    pub helpers: Helpers,
}

impl DebuggerAdapter {
    /// Parse an adapter from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, DebugError> {
        let adapter: Self = toml::from_str(text).map_err(|e| DebugError::InvalidAdapter {
            adapter: "<toml>".into(),
            message: e.to_string(),
        })?;
        adapter.validate()?;
        Ok(adapter)
    }

    /// Check the adapter's required fields and compile every regex.
    pub fn validate(&self) -> Result<(), DebugError> {
        let invalid = |message: String| DebugError::InvalidAdapter {
            adapter: self.id.clone(),
            message,
        };
        if self.id.is_empty() {
            return Err(invalid("adapter id is empty".into()));
        }
        if self.start.program.is_empty() {
            return Err(invalid("start.program is empty".into()));
        }
        if self.commands.backtrace.is_empty() {
            return Err(invalid("commands.backtrace is empty".into()));
        }
        let mut regexes: Vec<(&str, &str)> = vec![("prompt", &self.prompt)];
        if let Some(re) = &self.termination {
            regexes.push(("termination", re));
        }
        if let Some(re) = &self.crash {
            regexes.push(("crash", re));
        }
        for (name, re) in [
            ("parsers.stop_reason", &self.parsers.stop_reason),
            ("parsers.stop_location", &self.parsers.stop_location),
            ("parsers.frames", &self.parsers.frames),
            ("parsers.threads", &self.parsers.threads),
            ("parsers.breakpoint_hit", &self.parsers.breakpoint_hit),
            ("parsers.breakpoint_set", &self.parsers.breakpoint_set),
            ("parsers.variables", &self.parsers.variables),
            ("parsers.variables_marker", &self.parsers.variables_marker),
        ] {
            if let Some(re) = re {
                regexes.push((name, re));
            }
        }
        for (name, re) in regexes {
            if re.is_empty() {
                continue;
            }
            Regex::new(re).map_err(|e| invalid(format!("{name}: {e}")))?;
        }
        for helper in &self.helpers.python {
            if crate::helpers::python_helper(helper).is_none() {
                return Err(invalid(format!("unknown python helper `{helper}`")));
            }
        }
        if let Some(init) = &self.start.init
            && crate::helpers::init_file(init).is_none() {
                return Err(invalid(format!("unknown init file `{init}`")));
            }
        Ok(())
    }

    /// Locate the adapter's executable on `PATH`.
    pub fn availability(&self) -> Availability {
        let tool = &self.start.program;
        match find_executable(tool) {
            Some(path) => Availability::Available { path },
            None => Availability::Missing {
                tool: tool.clone(),
                hint: self
                    .install_hint
                    .clone()
                    .or_else(|| self.options.install_hint.clone())
                    .unwrap_or_else(|| format!("Install `{tool}` and try again.")),
            },
        }
    }

    /// Render the debugger argv with the supplied placeholders.
    pub(crate) fn start_argv(&self, vars: &BTreeMap<String, String>) -> Vec<String> {
        let mut argv = Vec::new();
        argv.push(render(&self.start.program, vars));
        for arg in &self.start.args {
            if arg == "{program_args}" {
                if let Some(args) = vars.get("program_args_list") {
                    argv.extend(split_nul(args));
                }
            } else {
                argv.push(render(arg, vars));
            }
        }
        argv
    }

    /// Render the optional debuggee/server argv.
    pub(crate) fn server_argv(&self, vars: &BTreeMap<String, String>) -> Option<Vec<String>> {
        let server = self.start.server.as_ref()?;
        let mut argv = Vec::new();
        for arg in server {
            if arg == "{program_args}" {
                if let Some(args) = vars.get("program_args_list") {
                    argv.extend(split_nul(args));
                }
            } else {
                argv.push(render(arg, vars));
            }
        }
        Some(argv)
    }
}

/// Result of looking for a debugger tool.
#[derive(Debug, Clone)]
pub enum Availability {
    /// The executable was found.
    Available {
        /// Absolute path.
        path: PathBuf,
    },
    /// The executable was not found.
    Missing {
        /// Tool name that was looked up.
        tool: String,
        /// Installation hint.
        hint: String,
    },
}

impl Availability {
    /// True when the tool is installed.
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available { .. })
    }

    /// Short status for menus and tests.
    pub fn status_text(&self) -> String {
        match self {
            Availability::Available { .. } => "available".into(),
            Availability::Missing { tool, .. } => format!("not available ({tool} not found)"),
        }
    }
}

/// A failed user adapter file.
#[derive(Debug, Clone)]
pub struct AdapterLoadError {
    /// File that failed.
    pub path: PathBuf,
    /// Why it failed.
    pub message: String,
}

/// Outcome of loading a directory of user adapters.
#[derive(Debug, Clone, Default)]
pub struct AdapterLoadReport {
    /// Number of adapters loaded (including overrides).
    pub loaded: usize,
    /// Per-file failures; loading continues with the other files.
    pub errors: Vec<AdapterLoadError>,
}

/// Registry of debugger adapters.
#[derive(Debug, Clone, Default)]
pub struct AdapterRegistry {
    adapters: BTreeMap<String, DebuggerAdapter>,
}

impl AdapterRegistry {
    /// Build the registry with the embedded adapters.
    pub fn builtin() -> Self {
        let mut registry = Self::default();
        for text in [
            BUILTIN_LLDB,
            BUILTIN_PDB,
            BUILTIN_NODE,
            BUILTIN_JDB,
            BUILTIN_BASHDB,
            BUILTIN_PERL,
            BUILTIN_PHPDBG,
        ] {
            match DebuggerAdapter::from_toml(text) {
                Ok(adapter) => {
                    registry.adapters.insert(adapter.id.clone(), adapter);
                }
                Err(err) => {
                    // Embedded data is covered by unit tests; keep going so a
                    // single bad adapter cannot make the app unusable.
                    eprintln!("scratch-debugger: embedded adapter is invalid: {err}");
                }
            }
        }
        registry
    }

    /// Insert or replace an adapter, validating it first.
    pub fn insert(&mut self, adapter: DebuggerAdapter) -> Result<(), DebugError> {
        adapter.validate()?;
        self.adapters.insert(adapter.id.clone(), adapter);
        Ok(())
    }

    /// Load every `*.toml` file in `dir` (non-recursive).  Missing directories
    /// are reported as an error, but do not make this function fail.
    pub fn load_user_dir(&mut self, dir: &Path) -> AdapterLoadReport {
        let mut report = AdapterLoadReport::default();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                report.errors.push(AdapterLoadError {
                    path: dir.to_path_buf(),
                    message: err.to_string(),
                });
                return report;
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let result = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    DebuggerAdapter::from_toml(&text).map_err(|e| e.to_string())
                });
            match result {
                Ok(adapter) => {
                    self.adapters.insert(adapter.id.clone(), adapter);
                    report.loaded += 1;
                }
                Err(message) => report.errors.push(AdapterLoadError { path, message }),
            }
        }
        report
    }

    /// Look one adapter up by id.
    pub fn get(&self, id: &str) -> Option<&DebuggerAdapter> {
        self.adapters.get(id)
    }

    /// The adapter configured for a language id, if any.
    pub fn for_language(&self, language: &str) -> Option<&DebuggerAdapter> {
        self.adapters
            .values()
            .find(|a| a.languages.iter().any(|l| l == language))
    }

    /// The default adapter id for a ScratchRunner language id.
    ///
    /// Mirrors CodeRunner 4.6's bundled debugger assignments, with Rust on
    /// LLDB (research report 10, C19) instead of the unavailable gdb.
    pub fn default_id_for_language(language: &str) -> Option<&'static str> {
        match language {
            "c" | "cpp" | "objective-c" | "objective-cpp" | "swift" | "go" | "rust" => Some("lldb"),
            "python" => Some("pdb"),
            "javascript-node" => Some("node"),
            "java" => Some("jdb"),
            "shell" => Some("bashdb"),
            "perl" => Some("perl"),
            "php" => Some("phpdbg"),
            _ => None,
        }
    }

    /// All adapters in id order.
    pub fn iter(&self) -> impl Iterator<Item = &DebuggerAdapter> {
        self.adapters.values()
    }

    /// Number of adapters.
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// True when no adapters are registered.
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

/// Find an executable on `PATH` (or accept an absolute/relative path with a
/// slash, checking that it is executable).
pub fn find_executable(tool: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if tool.contains('/') {
        let path = PathBuf::from(tool);
        return executable(&path).then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(tool);
        if executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Substitute `{name}` placeholders.
///
/// Unknown placeholders are left untouched so an adapter can embed literal
/// braces for the debugger's own syntax.
pub(crate) fn render(template: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let key = &after[..close];
            if let Some(value) = vars.get(key) {
                out.push_str(value);
            } else {
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
            rest = &after[close + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Quote a value for a debugger command line that accepts double quotes.
pub(crate) fn quote_double(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Quote a value as a Python string expression (for the pdb helper).
pub(crate) fn quote_python(value: &str) -> String {
    quote_double(value)
}

/// Join debuggee arguments for a debugger command that accepts quoted argv.
pub(crate) fn quote_argv(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_double(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_nul(value: &str) -> Vec<String> {
    value
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Build the NUL-separated argument list placeholder.
pub(crate) fn join_nul(args: &[String]) -> String {
    args.join("\0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_adapters_are_valid() {
        let registry = AdapterRegistry::builtin();
        assert_eq!(registry.len(), BUILTIN_ADAPTER_IDS.len());
        for id in BUILTIN_ADAPTER_IDS {
            let adapter = registry.get(id).unwrap_or_else(|| panic!("missing {id}"));
            adapter.validate().unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    #[test]
    fn language_defaults_cover_code_runner_debuggers() {
        assert_eq!(AdapterRegistry::default_id_for_language("c"), Some("lldb"));
        assert_eq!(AdapterRegistry::default_id_for_language("rust"), Some("lldb"));
        assert_eq!(AdapterRegistry::default_id_for_language("python"), Some("pdb"));
        assert_eq!(AdapterRegistry::default_id_for_language("javascript-node"), Some("node"));
        assert_eq!(AdapterRegistry::default_id_for_language("java"), Some("jdb"));
        assert_eq!(AdapterRegistry::default_id_for_language("shell"), Some("bashdb"));
        assert_eq!(AdapterRegistry::default_id_for_language("perl"), Some("perl"));
        assert_eq!(AdapterRegistry::default_id_for_language("php"), Some("phpdbg"));
        assert_eq!(AdapterRegistry::default_id_for_language("ruby"), None);
    }

    #[test]
    fn missing_builtin_tools_report_a_clear_status_and_hint() {
        for adapter in AdapterRegistry::builtin().iter() {
            let availability = adapter.availability();
            match &availability {
                Availability::Available { .. } => {
                    assert_eq!(availability.status_text(), "available");
                }
                Availability::Missing { tool, hint } => {
                    assert!(!tool.is_empty());
                    assert!(!hint.is_empty(), "{}: missing install hint", adapter.id);
                    assert!(
                        availability.status_text().starts_with("not available"),
                        "{}: {}",
                        adapter.id,
                        availability.status_text()
                    );
                }
            }
        }
    }

    #[test]
    fn render_replaces_known_placeholders_and_keeps_unknown() {
        let mut vars = BTreeMap::new();
        vars.insert("line".to_string(), "12".to_string());
        assert_eq!(render("break {file}:{line}", &vars), "break {file}:12");
        assert_eq!(render("break {line}{", &vars), "break 12{");
    }

    #[test]
    fn quoting_escapes_quotes_and_backslashes() {
        assert_eq!(quote_double(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quote_argv(&["a b".into(), "c".into()]), r#""a b" "c""#);
    }

    #[test]
    fn registry_loads_user_adapter_and_reports_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("good.toml"),
            r#"
id = "good"
name = "Good"
prompt = "(?m)^ok> $"
[start]
program = "sh"
[commands]
backtrace = "bt"
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("bad.toml"), "id = ").unwrap();
        let mut registry = AdapterRegistry::builtin();
        let report = registry.load_user_dir(dir.path());
        assert_eq!(report.loaded, 1);
        assert_eq!(report.errors.len(), 1);
        assert!(registry.get("good").is_some());
    }
}
