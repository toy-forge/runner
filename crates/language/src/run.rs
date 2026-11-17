//! The run specification: how a language is compiled and run.
//!
//! A [`RunSpec`] has three parts, executed in this order by the runner:
//!
//! 1. an optional **compile phase** ([`CompileSpec`]) that produces `$compiler`
//!    - [`CompileSpec::Steps`]: declarative, no shell. Ordered [`BuildStep`]s
//!      (`program`, `args` with variables, `env`, conditions, requirements,
//!      what the step produces). Small [`VarBinding`]s run before the steps to
//!      compute values from the source (sibling C sources, the Java main
//!      class, ...). All 29 bundled languages use this.
//!    - [`CompileSpec::Shell`]: CodeRunner's protocol for user-defined
//!      languages, an arbitrary shell script (see [`crate::protocol`]).
//! 2. the **run command** (`command`): a template expanded by
//!    [`crate::resolve_command`] (see [`crate::expand`]).
//! 3. the user's run arguments, appended to the run command's argv.
//!
//! [`RunSpec::plan`] turns the spec plus the variables of one run into a
//! [`RunPlan`]: concrete argv lists, working directories and environment for
//! the runner to execute. Planning reads the filesystem only to inspect the
//! source (includes, `package` lines, `go.mod`).
//!
//! # TOML shape
//!
//! ```toml
//! [run]
//! command = "$compiler"          # the run command template
//! flags = "-std=c11"             # default compile flags (per file override: RunContext)
//! [run.env]                      # applies to every step, the run command and diagnostics
//! path_prepend = ["/Library/TeX/texbin"]
//! [run.compile]
//! kind = "steps"                 # "none" | "steps" | "shell"
//! strip_flags = ["-sr-noautoinclude"]
//! [[run.compile.bindings]]
//! name = "sources"
//! source = { kind = "local_sources", family = "c", disable_flags = ["-sr-noautoinclude"] }
//! [[run.compile.steps]]
//! name = "Compile"
//! via = ["xcrun"]
//! program = "clang"
//! args = ["-o", "$output", "$sources", "$flags"]
//! debug_args = ["-g"]
//! produces = { value = "$output" }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ExpandError, PlanError};
use crate::expand::{ResolvedCommand, expand_arg, expand_string, glob_match, resolve_command};
use crate::probe::{self, SourceFamily};
use crate::protocol::parse_compiler_output;
use crate::vars::Vars;

fn is_false(value: &bool) -> bool {
    !*value
}

/// How a language is compiled and run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSpec {
    /// The run command template (CodeRunner's `RunCommand`). Variables:
    /// see [`Vars`]. Empty = nothing to run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Default compile flags (CodeRunner's `CompilerFlags`), shell-split into
    /// `$flags`. A file can override them ([`RunContext::compile_flags`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flags: String,
    /// Environment for compile steps, the run command and diagnostics.
    #[serde(default, skip_serializing_if = "EnvSpec::is_empty")]
    pub env: EnvSpec,
    /// The compile phase.
    #[serde(default, skip_serializing_if = "CompileSpec::is_none")]
    pub compile: CompileSpec,
}

impl RunSpec {
    /// Whether nothing is specified (so it is left out of TOML).
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Problems of the spec itself (empty programs, duplicate bindings, ...).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if let CompileSpec::Steps(steps) = &self.compile {
            let mut names = std::collections::BTreeSet::new();
            for binding in &steps.bindings {
                if binding.name.is_empty() || !names.insert(binding.name.as_str()) {
                    problems.push(format!(
                        "binding name `{}` is empty or duplicated",
                        binding.name
                    ));
                }
            }
            if steps.steps.is_empty() {
                problems.push("compile kind `steps` has no steps".to_owned());
            }
            for step in &steps.steps {
                if step.program.trim().is_empty() {
                    problems.push(format!("step `{}` has no program", step.name));
                }
            }
        }
        if let CompileSpec::Shell(shell) = &self.compile
            && shell.script.trim().is_empty()
        {
            problems.push("compile kind `shell` has an empty script".to_owned());
        }
        problems
    }

    /// Every variable name referenced by a template of this spec (command,
    /// arguments, working directories, environment, produced paths).
    pub fn referenced_variables(&self) -> BTreeSet<String> {
        let mut templates: Vec<&str> = vec![&self.command];
        templates.extend(self.env.path_prepend.iter().map(String::as_str));
        templates.extend(self.env.path_append.iter().map(String::as_str));
        templates.extend(self.env.vars.values().map(String::as_str));
        if let CompileSpec::Steps(spec) = &self.compile {
            for binding in &spec.bindings {
                match &binding.source {
                    VarSource::Template { value } | VarSource::Identifier { value } => {
                        templates.push(value)
                    }
                    _ => {}
                }
            }
            for step in &spec.steps {
                templates.push(&step.program);
                templates.extend(step.via.iter().map(String::as_str));
                templates.extend(step.args.iter().map(String::as_str));
                templates.extend(step.debug_args.iter().map(String::as_str));
                templates.extend(step.cwd.iter().map(String::as_str));
                templates.extend(step.env.values().map(String::as_str));
                templates.extend(step.mkdirs.iter().map(String::as_str));
                templates.extend(step.produces.iter().map(|p| p.value.as_str()));
            }
        }
        templates
            .into_iter()
            .flat_map(crate::expand::referenced_variables)
            .collect()
    }

    /// The variables this spec defines itself: binding names and the
    /// variables its steps produce.
    pub fn defined_variables(&self) -> BTreeSet<String> {
        let mut defined = BTreeSet::new();
        if let CompileSpec::Steps(spec) = &self.compile {
            defined.extend(spec.bindings.iter().map(|b| b.name.clone()));
            defined.extend(
                spec.steps
                    .iter()
                    .filter_map(|s| s.produces.as_ref())
                    .map(|p| p.var.clone()),
            );
        }
        defined
    }

    /// Plans a run: expands every template against `ctx.vars`, evaluates
    /// bindings and conditions, and returns what to execute.
    ///
    /// # Errors
    ///
    /// [`PlanError`] when a template cannot be expanded, a probe fails or a
    /// step requirement is not met (the message is for the user).
    pub fn plan(&self, ctx: &RunContext) -> Result<RunPlan, PlanError> {
        let flags = ctx.compile_flags.as_deref().unwrap_or(&self.flags);
        let mut vars = ctx
            .vars
            .clone()
            .with_flags(flags)
            .with_debugging(ctx.debugging);
        let raw_flags = flag_list(&vars);
        let dir = PathBuf::from(vars.get_str("dir").unwrap_or_default());

        let expand = |context: &str, r: Result<String, ExpandError>| {
            r.map_err(|source| PlanError::Expand {
                context: context.to_owned(),
                source,
            })
        };

        // Language-level environment.
        let mut env_vars = BTreeMap::new();
        for (name, template) in &self.env.vars {
            env_vars.insert(
                name.clone(),
                expand(&format!("env {name}"), expand_string(template, &vars))?,
            );
        }
        // An entry under `$home` is dropped when the home directory is unknown.
        let usable = |entry: &str| !entry.starts_with("$home") || vars.is_set("home");
        let mut path_prepend = Vec::new();
        for entry in self.env.path_prepend.iter().filter(|e| usable(e)) {
            path_prepend.push(expand("path_prepend", expand_string(entry, &vars))?);
        }
        let mut path_append = Vec::new();
        for entry in self.env.path_append.iter().filter(|e| usable(e)) {
            path_append.push(expand("path_append", expand_string(entry, &vars))?);
        }

        let compile = match &self.compile {
            CompileSpec::None => CompilePlan::None,
            CompileSpec::Shell(shell) => CompilePlan::Shell(ShellPlan {
                script: shell.script.clone(),
                args: flag_list(&vars),
                cwd: dir.clone(),
                env: vars.export_env(),
            }),
            CompileSpec::Steps(spec) => {
                for binding in &spec.bindings {
                    let value = binding
                        .source
                        .evaluate(&vars, &raw_flags, &dir)
                        .map_err(|e| match e {
                            PlanError::Expand { source, .. } => PlanError::Expand {
                                context: format!("binding `{}`", binding.name),
                                source,
                            },
                            other => other,
                        })?;
                    vars.set(&binding.name, value);
                }
                if !spec.strip_flags.is_empty() {
                    let kept: Vec<String> = raw_flags
                        .iter()
                        .filter(|f| !spec.strip_flags.contains(f))
                        .cloned()
                        .collect();
                    vars.set("flags", kept);
                }
                let mut planned = Vec::new();
                for step in &spec.steps {
                    if let Some(p) = step.plan(&mut vars, ctx.debugging, &dir, &env_vars)? {
                        planned.push(p);
                    }
                }
                CompilePlan::Steps(planned)
            }
        };

        Ok(RunPlan {
            compile,
            command: ctx
                .run_command
                .clone()
                .unwrap_or_else(|| self.command.clone()),
            arguments: ctx.arguments.clone(),
            vars,
            env: env_vars,
            path_prepend,
            path_append,
        })
    }
}

fn flag_list(vars: &Vars) -> Vec<String> {
    match vars.get("flags") {
        Some(crate::vars::Value::List(items)) => items.clone(),
        Some(crate::vars::Value::Str(s)) => s.split_whitespace().map(str::to_owned).collect(),
        None => Vec::new(),
    }
}

/// Environment additions that apply to compile steps, the run command and
/// diagnostic commands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
    /// Directories put in front of `PATH` (templates: `$home/.cargo/bin`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_prepend: Vec<String>,
    /// Directories added after `PATH`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_append: Vec<String>,
    /// Extra variables (values are templates).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
}

impl EnvSpec {
    /// Whether nothing is set.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The compile phase of a language.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompileSpec {
    /// Nothing to compile (interpreted languages).
    #[default]
    None,
    /// Declarative steps, no shell.
    Steps(StepsSpec),
    /// A user-defined compile script following CodeRunner's protocol
    /// ([`crate::protocol`]).
    Shell(ShellSpec),
}

impl CompileSpec {
    /// Whether there is no compile phase.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Declarative compile steps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepsSpec {
    /// Values computed from the source before the steps run; each becomes a
    /// variable usable in every template.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<VarBinding>,
    /// Compile flags that are consumed by the runner itself and removed from
    /// `$flags` (CodeRunner's `-cr-noautoinclude`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strip_flags: Vec<String>,
    /// The steps, in order. A step that fails (non-zero exit) ends the run.
    #[serde(default)]
    pub steps: Vec<BuildStep>,
}

/// A user-defined compile script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellSpec {
    /// The script text (run as `<shell> -c`/a temp file, cwd = the source
    /// directory, `$1..$n` = the compile flags, `CR_*` environment).
    #[serde(default)]
    pub script: String,
}

/// A named value computed before the steps run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarBinding {
    /// The variable name.
    pub name: String,
    /// How to compute it.
    pub source: VarSource,
}

/// How a [`VarBinding`] is computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VarSource {
    /// A template expanded to a string.
    Template {
        /// The template (`$stem.out`).
        value: String,
    },
    /// A template expanded, then turned into an identifier
    /// ([`probe::sanitize_identifier`]).
    Identifier {
        /// The template (`$stem`).
        value: String,
    },
    /// List: the main file plus the implementation files of the local headers
    /// it includes ([`probe::local_sources`]). If any of `disable_flags` is in
    /// the compile flags only the main file is used.
    LocalSources {
        /// The language dialect.
        family: SourceFamily,
        /// Flags that switch the scan off.
        #[serde(default)]
        disable_flags: Vec<String>,
    },
    /// List: `-framework X` arguments for the files in variable `of` (default
    /// `sources`) ([`probe::framework_args`]).
    Frameworks {
        /// The variable holding the file list.
        #[serde(default = "default_sources_var")]
        of: String,
    },
    /// List: the Swift files to compile ([`probe::swift_sources`]).
    SwiftSources,
    /// The Java package of the source (empty = default package).
    JavaPackage,
    /// The fully qualified main class of the Java source (falls back to the
    /// file's stem, qualified by the package, when there is no `main`).
    JavaMainClass,
    /// The directory `javac` must be run from ([`probe::java_source_root`]).
    JavaSourceRoot,
    /// The source path relative to the Java source root.
    JavaSourceRelative,
}

fn default_sources_var() -> String {
    "sources".to_owned()
}

impl VarSource {
    fn evaluate(
        &self,
        vars: &Vars,
        raw_flags: &[String],
        dir: &Path,
    ) -> Result<crate::vars::Value, PlanError> {
        use crate::vars::Value;
        let file_name = vars.get_str("filename").unwrap_or_default();
        let expand = |template: &str| {
            expand_string(template, vars).map_err(|source| PlanError::Expand {
                context: String::new(),
                source,
            })
        };
        let read_source = || {
            let path = dir.join(&file_name);
            std::fs::read_to_string(&path).map_err(|e| PlanError::Probe {
                path,
                message: e.to_string(),
            })
        };
        Ok(match self {
            VarSource::Template { value } => Value::Str(expand(value)?),
            VarSource::Identifier { value } => {
                Value::Str(probe::sanitize_identifier(&expand(value)?))
            }
            VarSource::LocalSources {
                family,
                disable_flags,
            } => {
                if disable_flags.iter().any(|f| raw_flags.contains(f)) {
                    Value::List(vec![file_name])
                } else {
                    Value::List(probe::local_sources(dir, &file_name, *family)?)
                }
            }
            VarSource::Frameworks { of } => {
                let files = match vars.get(of) {
                    Some(Value::List(items)) => items.clone(),
                    Some(Value::Str(s)) => vec![s.clone()],
                    None => vec![file_name],
                };
                Value::List(probe::framework_args(dir, &files))
            }
            VarSource::SwiftSources => Value::List(probe::swift_sources(dir, &file_name)?),
            VarSource::JavaPackage => Value::Str(probe::java_package(&read_source()?)),
            VarSource::JavaMainClass => {
                let source = read_source()?;
                Value::Str(probe::java_main_class(&source).unwrap_or_else(|| {
                    let stem = Path::new(&file_name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Main")
                        .to_owned();
                    match probe::java_package(&source) {
                        p if p.is_empty() => stem,
                        p => format!("{p}.{stem}"),
                    }
                }))
            }
            VarSource::JavaSourceRoot => {
                let package = probe::java_package(&read_source()?);
                Value::Str(
                    probe::java_source_root(dir, &package)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            VarSource::JavaSourceRelative => {
                let package = probe::java_package(&read_source()?);
                Value::Str(probe::java_source_relative(dir, &file_name, &package))
            }
        })
    }
}

/// A condition on a step, evaluated at planning time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// It is a debug run.
    Debugging,
    /// The variable is set to a non-empty value.
    VarSet(String),
    /// A flag is among the compile flags.
    FlagPresent(String),
    /// The source file has exactly this name.
    FileNameIs(String),
    /// A file matching the name (one `*` wildcard allowed) exists in the
    /// working directory.
    FileExists(String),
    /// A file matching the name exists in the working directory or one of its
    /// ancestors (`go.mod`, `*.csproj`).
    FileExistsUp(String),
}

impl Condition {
    fn holds(&self, vars: &Vars, debugging: bool, dir: &Path) -> bool {
        match self {
            Condition::Debugging => debugging,
            Condition::VarSet(name) => vars.is_set(name),
            Condition::FlagPresent(flag) => flag_list(vars).contains(flag),
            Condition::FileNameIs(name) => vars.get_str("filename").as_deref() == Some(name),
            Condition::FileExists(pattern) => dir_has(dir, pattern),
            Condition::FileExistsUp(pattern) => dir.ancestors().any(|d| dir_has(d, pattern)),
        }
    }
}

fn dir_has(dir: &Path, pattern: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return dir.join(pattern).exists();
    }
    let pat: Vec<char> = pattern.chars().collect();
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name: Vec<char> = entry.file_name().to_string_lossy().chars().collect();
            glob_match(&pat, &name)
        })
    })
}

/// A requirement of a step: when `condition` does not hold the run stops with
/// `message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    /// What must hold.
    pub condition: Condition,
    /// What to tell the user otherwise.
    pub message: String,
}

/// What a step produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Produces {
    /// The variable that is set (`compiler` by default, which is what the run
    /// command's `$compiler` refers to).
    #[serde(default = "default_produces_var")]
    pub var: String,
    /// Template of the value (an output path).
    pub value: String,
    /// After the step succeeded the value (a path) must exist; otherwise the
    /// run stops with `missing_message`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub must_exist: bool,
    /// Message when `must_exist` fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_message: Option<String>,
}

fn default_produces_var() -> String {
    "compiler".to_owned()
}

/// One process of the compile phase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildStep {
    /// Label for progress and error output (`Compile`).
    #[serde(default)]
    pub name: String,
    /// The program (looked up on `PATH`; a template).
    pub program: String,
    /// A wrapper argv put in front (`["xcrun", "-sdk", "macosx"]`). If the
    /// wrapper's program is not installed the step runs without it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
    /// Arguments. Each entry is one argument template (see
    /// [`crate::expand_arg`]); `$flags` and other list variables splice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Arguments added after `args` on a debug run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub debug_args: Vec<String>,
    /// Working directory template; default the source directory (`$dir`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment variables of this step (values are templates).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The step is only planned when all of these hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
    /// The step is skipped when any of these holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unless: Vec<Condition>,
    /// Requirements checked when the step is planned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require: Vec<Requirement>,
    /// Directories the runner creates before the step (templates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mkdirs: Vec<String>,
    /// Message when the program is not found or exits 127 (overrides the
    /// language's installer hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_tool: Option<String>,
    /// What the step produces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produces: Option<Produces>,
}

impl BuildStep {
    fn plan(
        &self,
        vars: &mut Vars,
        debugging: bool,
        dir: &Path,
        language_env: &BTreeMap<String, String>,
    ) -> Result<Option<PlannedStep>, PlanError> {
        let label = if self.name.is_empty() {
            self.program.as_str()
        } else {
            self.name.as_str()
        };
        let ctx_err = |what: &str, source: ExpandError| PlanError::Expand {
            context: format!("step `{label}` {what}"),
            source,
        };
        if !self.when.iter().all(|c| c.holds(vars, debugging, dir)) {
            return Ok(None);
        }
        if self.unless.iter().any(|c| c.holds(vars, debugging, dir)) {
            return Ok(None);
        }
        for requirement in &self.require {
            if !requirement.condition.holds(vars, debugging, dir) {
                return Err(PlanError::Requirement(requirement.message.clone()));
            }
        }
        if self.program.trim().is_empty() {
            return Err(PlanError::Invalid(format!("step `{label}` has no program")));
        }
        let program = expand_string(&self.program, vars).map_err(|e| ctx_err("program", e))?;
        let mut via = Vec::new();
        for entry in &self.via {
            via.extend(expand_arg(entry, vars).map_err(|e| ctx_err("via", e))?);
        }
        let mut args = Vec::new();
        let debug_args = if debugging {
            self.debug_args.as_slice()
        } else {
            &[]
        };
        for arg in self.args.iter().chain(debug_args) {
            args.extend(expand_arg(arg, vars).map_err(|e| ctx_err("args", e))?);
        }
        let cwd = match &self.cwd {
            Some(template) => {
                PathBuf::from(expand_string(template, vars).map_err(|e| ctx_err("cwd", e))?)
            }
            None => dir.to_path_buf(),
        };
        let mut env = language_env.clone();
        for (name, template) in &self.env {
            env.insert(
                name.clone(),
                expand_string(template, vars).map_err(|e| ctx_err("env", e))?,
            );
        }
        let mut mkdirs = Vec::new();
        for template in &self.mkdirs {
            mkdirs.push(PathBuf::from(
                expand_string(template, vars).map_err(|e| ctx_err("mkdirs", e))?,
            ));
        }
        let produces = match &self.produces {
            Some(p) => {
                let value = expand_string(&p.value, vars).map_err(|e| ctx_err("produces", e))?;
                if p.var == "compiler" {
                    vars.set_compiler(&value);
                } else {
                    vars.set(&p.var, value.clone());
                }
                Some(PlannedProduces {
                    var: p.var.clone(),
                    value,
                    must_exist: p.must_exist,
                    missing_message: p.missing_message.clone(),
                })
            }
            None => None,
        };
        Ok(Some(PlannedStep {
            name: self.name.clone(),
            via,
            program,
            args,
            cwd,
            env,
            mkdirs,
            missing_tool: self.missing_tool.clone(),
            produces,
        }))
    }
}

/// The inputs of planning one run.
#[derive(Debug, Clone)]
pub struct RunContext {
    /// The run's variables ([`Vars::for_source`] + `with_tmpdir`, `with_encoding`,
    /// `with_env`, ...). `flags` and `debugging` are set by planning.
    pub vars: Vars,
    /// Per-file compile flags; `None` = the spec's default flags.
    pub compile_flags: Option<String>,
    /// Per-file run command; `None` = the spec's command.
    pub run_command: Option<String>,
    /// Program arguments (already split), appended to the run command.
    pub arguments: Vec<String>,
    /// A debug run (`-g` steps, `CR_DEBUGGING`).
    pub debugging: bool,
}

impl RunContext {
    /// A context with the given variables and nothing overridden.
    pub fn new(vars: Vars) -> Self {
        Self {
            vars,
            compile_flags: None,
            run_command: None,
            arguments: Vec::new(),
            debugging: false,
        }
    }
}

/// The result of planning: everything the runner needs to execute one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    /// The compile phase.
    pub compile: CompilePlan,
    /// The effective run command template.
    pub command: String,
    /// The user's program arguments.
    pub arguments: Vec<String>,
    /// The variables after bindings and `produces` (`compiler` is set when a
    /// step produced it).
    pub vars: Vars,
    /// Environment variables of the language (expanded).
    pub env: BTreeMap<String, String>,
    /// Directories to put in front of `PATH`.
    pub path_prepend: Vec<String>,
    /// Directories to add after `PATH`.
    pub path_append: Vec<String>,
}

impl RunPlan {
    /// The run command, expanded. `compiler_stdout` is the stdout of a
    /// [`CompilePlan::Shell`] compile script; its last non-empty line becomes
    /// `$compiler`. For declarative steps pass `None`.
    ///
    /// # Errors
    ///
    /// [`PlanError::Expand`] when the command cannot be expanded.
    pub fn resolve_command(
        &self,
        compiler_stdout: Option<&str>,
    ) -> Result<ResolvedCommand, PlanError> {
        let mut vars = self.vars.clone();
        if let Some(stdout) = compiler_stdout
            && let Some(compiler) = parse_compiler_output(stdout)
        {
            vars.set_compiler(&compiler);
        }
        resolve_command(&self.command, &vars).map_err(|source| PlanError::Expand {
            context: "run command".to_owned(),
            source,
        })
    }

    /// The final argv to spawn: [`RunPlan::resolve_command`] with the user's
    /// arguments appended ([`ResolvedCommand::to_argv`]).
    pub fn run_argv(
        &self,
        shell: &str,
        login_shell: bool,
        compiler_stdout: Option<&str>,
    ) -> Result<Vec<String>, PlanError> {
        Ok(self
            .resolve_command(compiler_stdout)?
            .to_argv(shell, login_shell, &self.arguments))
    }

    /// The environment variables for processes of this run: the exported
    /// variables ([`Vars::export_env`]) plus the language's `env`. `PATH`
    /// additions are in `path_prepend`/`path_append`.
    pub fn process_env(&self) -> BTreeMap<String, String> {
        let mut env = self.vars.export_env();
        env.extend(self.env.clone());
        env
    }
}

/// The planned compile phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilePlan {
    /// Nothing to compile.
    None,
    /// Run these processes in order; stop at the first failure.
    Steps(Vec<PlannedStep>),
    /// Run a compile script (see [`crate::protocol`]).
    Shell(ShellPlan),
}

/// A compile script to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPlan {
    /// The script text.
    pub script: String,
    /// `$1..$n`: the compile flags.
    pub args: Vec<String>,
    /// The working directory (the source directory).
    pub cwd: PathBuf,
    /// The exported environment (`CR_*` and the run's variables).
    pub env: BTreeMap<String, String>,
}

/// One concrete process of the compile phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    /// The step's label.
    pub name: String,
    /// A wrapper argv in front of the program; skipped when the wrapper's
    /// program is not installed.
    pub via: Vec<String>,
    /// The program name (looked up on the run's `PATH`).
    pub program: String,
    /// The arguments (never passed through a shell).
    pub args: Vec<String>,
    /// The working directory.
    pub cwd: PathBuf,
    /// Extra environment variables (language `env` and the step's).
    pub env: BTreeMap<String, String>,
    /// Directories to create first.
    pub mkdirs: Vec<PathBuf>,
    /// Message for "program not found" / exit 127.
    pub missing_tool: Option<String>,
    /// What the step is expected to produce.
    pub produces: Option<PlannedProduces>,
}

impl PlannedStep {
    /// The argv to spawn: `via` + program + args (pass `use_via = false` to
    /// run without the wrapper).
    pub fn argv(&self, use_via: bool) -> Vec<String> {
        let mut argv = if use_via {
            self.via.clone()
        } else {
            Vec::new()
        };
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }

    /// The command line for display.
    pub fn display(&self) -> String {
        self.argv(true)
            .iter()
            .map(|a| {
                if a.is_empty() || a.contains(char::is_whitespace) {
                    crate::vars::shell_quote(a)
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What a planned step produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProduces {
    /// The variable (`compiler`).
    pub var: String,
    /// The value (an output path).
    pub value: String,
    /// The path must exist after the step succeeded.
    pub must_exist: bool,
    /// Message when it does not.
    pub missing_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn step(program: &str, args: &[&str]) -> BuildStep {
        BuildStep {
            name: "Compile".into(),
            program: program.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            ..BuildStep::default()
        }
    }

    fn ctx(dir: &Path, file: &str) -> RunContext {
        RunContext::new(Vars::for_source(dir, file).with_tmpdir(Path::new("/tmp/run1")))
    }

    #[test]
    fn simple_step_with_flags_and_produces() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.rs", "fn main(){}");
        let mut s = step("rustc", &["-o", "$output", "$filename", "$flags"]);
        s.debug_args = vec!["-g".into()];
        s.produces = Some(Produces {
            var: "compiler".into(),
            value: "$output".into(),
            must_exist: true,
            missing_message: None,
        });
        let spec = RunSpec {
            command: "$compiler".into(),
            flags: "--edition 2021".into(),
            compile: CompileSpec::Steps(StepsSpec {
                steps: vec![s],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(tmp.path(), "a.rs")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!("expected steps")
        };
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].argv(true),
            ["rustc", "-o", "/tmp/run1/a", "a.rs", "--edition", "2021"]
        );
        assert_eq!(steps[0].cwd, tmp.path());
        assert_eq!(
            plan.vars.get_str("compiler").as_deref(),
            Some("/tmp/run1/a")
        );
        assert_eq!(
            plan.resolve_command(None).unwrap(),
            ResolvedCommand::Argv(vec!["/tmp/run1/a".into()])
        );

        let mut debug = ctx(tmp.path(), "a.rs");
        debug.debugging = true;
        debug.compile_flags = Some(String::new());
        let plan = spec.plan(&debug).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps[0].args.last().map(String::as_str), Some("-g"));
        assert!(
            !steps[0].args.contains(&"--edition".to_owned()),
            "per-file flags replace defaults"
        );
    }

    #[test]
    fn conditions_and_requirements() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "main.go", "package main");
        let mut module = step("go", &["build"]);
        module.when = vec![Condition::FileExistsUp("go.mod".into())];
        let mut single = step("go", &["build", "$filename"]);
        single.unless = vec![Condition::FileExistsUp("go.mod".into())];
        let spec = RunSpec {
            compile: CompileSpec::Steps(StepsSpec {
                steps: vec![module, single],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(tmp.path(), "main.go")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].args, ["build", "main.go"]);

        write(tmp.path(), "go.mod", "module x");
        let plan = spec.plan(&ctx(tmp.path(), "main.go")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].args, ["build"]);

        // a parent directory counts for FileExistsUp
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        write(&sub, "main.go", "package main");
        let plan = spec.plan(&ctx(&sub, "main.go")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps[0].args, ["build"]);
    }

    #[test]
    fn requirement_failure_is_a_user_message() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "p.cs", "");
        let mut s = step("dotnet", &["build"]);
        s.require = vec![Requirement {
            condition: Condition::FileExistsUp("*.csproj".into()),
            message: "No project found.".into(),
        }];
        let spec = RunSpec {
            compile: CompileSpec::Steps(StepsSpec {
                steps: vec![s],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        assert_eq!(
            spec.plan(&ctx(tmp.path(), "p.cs")),
            Err(PlanError::Requirement("No project found.".into()))
        );
        write(tmp.path(), "x.csproj", "");
        assert!(spec.plan(&ctx(tmp.path(), "p.cs")).is_ok());
    }

    #[test]
    fn bindings_feed_steps_and_strip_flags() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "main.c", "#include \"u.h\"\nint main(){}");
        write(tmp.path(), "u.h", "");
        write(tmp.path(), "u.c", "");
        let spec = RunSpec {
            flags: "-std=c11".into(),
            compile: CompileSpec::Steps(StepsSpec {
                bindings: vec![VarBinding {
                    name: "sources".into(),
                    source: VarSource::LocalSources {
                        family: SourceFamily::C,
                        disable_flags: vec!["-sr-noautoinclude".into()],
                    },
                }],
                strip_flags: vec!["-sr-noautoinclude".into()],
                steps: vec![step("clang", &["-o", "$output", "$sources", "$flags"])],
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(tmp.path(), "main.c")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(
            steps[0].args,
            ["-o", "/tmp/run1/main", "main.c", "u.c", "-std=c11"]
        );

        let mut c = ctx(tmp.path(), "main.c");
        c.compile_flags = Some("-sr-noautoinclude -O2".into());
        let plan = spec.plan(&c).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps[0].args, ["-o", "/tmp/run1/main", "main.c", "-O2"]);
    }

    #[test]
    fn java_bindings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("src/com/acme");
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir,
            "App.java",
            "package com.acme;\npublic class App { public static void main(String[] a) {} }",
        );
        let mut javac = step("javac", &["-d", "$build_dir", "$java_rel"]);
        javac.cwd = Some("$java_root".into());
        javac.produces = Some(Produces {
            var: "compiler".into(),
            value: "$build_dir".into(),
            must_exist: false,
            missing_message: None,
        });
        let b = |name: &str, source| VarBinding {
            name: name.into(),
            source,
        };
        let spec = RunSpec {
            command: "java -cp $compiler $main_class".into(),
            compile: CompileSpec::Steps(StepsSpec {
                bindings: vec![
                    b("main_class", VarSource::JavaMainClass),
                    b("java_root", VarSource::JavaSourceRoot),
                    b("java_rel", VarSource::JavaSourceRelative),
                ],
                steps: vec![javac],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(&dir, "App.java")).unwrap();
        let CompilePlan::Steps(steps) = &plan.compile else {
            panic!()
        };
        assert_eq!(steps[0].cwd, tmp.path().join("src"));
        assert_eq!(
            steps[0].args,
            ["-d", "/tmp/run1/App.build", "com/acme/App.java"]
        );
        assert_eq!(
            plan.resolve_command(None).unwrap(),
            ResolvedCommand::Argv(vec![
                "java".into(),
                "-cp".into(),
                "/tmp/run1/App.build".into(),
                "com.acme.App".into()
            ])
        );
    }

    #[test]
    fn java_without_main_falls_back_to_the_file_stem() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "Lib.java", "class Lib {}");
        let spec = RunSpec {
            compile: CompileSpec::Steps(StepsSpec {
                bindings: vec![VarBinding {
                    name: "main_class".into(),
                    source: VarSource::JavaMainClass,
                }],
                steps: vec![step("javac", &["$filename"])],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(tmp.path(), "Lib.java")).unwrap();
        assert_eq!(plan.vars.get_str("main_class").as_deref(), Some("Lib"));
    }

    #[test]
    fn shell_compile_protocol_sets_compiler_from_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = RunSpec {
            command: "$compiler --run".into(),
            flags: "-a 'b c'".into(),
            compile: CompileSpec::Shell(ShellSpec {
                script: "echo built".into(),
            }),
            ..RunSpec::default()
        };
        let plan = spec.plan(&ctx(tmp.path(), "x.foo")).unwrap();
        let CompilePlan::Shell(shell) = &plan.compile else {
            panic!()
        };
        assert_eq!(shell.args, ["-a", "b c"]);
        assert_eq!(shell.cwd, tmp.path());
        assert_eq!(
            shell.env.get("CR_FILENAME").map(String::as_str),
            Some("x.foo")
        );
        assert_eq!(
            plan.resolve_command(Some("warming up\n/tmp/out bin\n\n"))
                .unwrap(),
            ResolvedCommand::Argv(vec!["/tmp/out bin".into(), "--run".into()])
        );
    }

    #[test]
    fn run_command_override_and_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = RunSpec {
            command: "python3 $filename".into(),
            ..RunSpec::default()
        };
        let mut c = ctx(tmp.path(), "x.py");
        c.run_command = Some("python3 -u $filename".into());
        c.arguments = vec!["a b".into()];
        let plan = spec.plan(&c).unwrap();
        assert_eq!(
            plan.run_argv("/bin/bash", false, None).unwrap(),
            ["python3", "-u", "x.py", "a b"]
        );
    }

    #[test]
    fn env_is_expanded_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("HOME".to_owned(), "/Users/me".to_owned());
        let mut spec = RunSpec {
            command: "dotnet-script $filename".into(),
            ..RunSpec::default()
        };
        spec.env.path_append = vec!["$home/.dotnet/tools".into()];
        spec.env.vars.insert("DOTNET_NOLOGO".into(), "1".into());
        let c = RunContext::new(Vars::for_source(tmp.path(), "a.csx").with_env(env));
        let plan = spec.plan(&c).unwrap();
        assert_eq!(plan.path_append, ["/Users/me/.dotnet/tools"]);
        assert_eq!(
            plan.process_env().get("DOTNET_NOLOGO").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            plan.process_env().get("CR_FILENAME").map(String::as_str),
            Some("a.csx")
        );
    }

    #[test]
    fn unknown_binding_errors_carry_context() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = RunSpec {
            compile: CompileSpec::Steps(StepsSpec {
                bindings: vec![VarBinding {
                    name: "x".into(),
                    source: VarSource::Template {
                        value: "${oops".into(),
                    },
                }],
                steps: vec![step("true", &[])],
                ..StepsSpec::default()
            }),
            ..RunSpec::default()
        };
        let err = spec.plan(&ctx(tmp.path(), "a")).unwrap_err();
        assert!(err.to_string().contains("binding `x`"), "{err}");
    }

    #[test]
    fn validate_reports_spec_problems() {
        let spec = RunSpec {
            compile: CompileSpec::Steps(StepsSpec::default()),
            ..RunSpec::default()
        };
        assert!(!spec.validate().is_empty());
        let spec = RunSpec {
            compile: CompileSpec::Shell(ShellSpec::default()),
            ..RunSpec::default()
        };
        assert!(!spec.validate().is_empty());
        assert!(RunSpec::default().validate().is_empty());
    }

    #[test]
    fn display_quotes_arguments_with_spaces() {
        let s = PlannedStep {
            name: String::new(),
            via: vec!["xcrun".into()],
            program: "clang".into(),
            args: vec!["-o".into(), "/tmp/a b".into()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            mkdirs: vec![],
            missing_tool: None,
            produces: None,
        };
        assert_eq!(s.display(), "xcrun clang -o '/tmp/a b'");
        assert_eq!(s.argv(false), ["clang", "-o", "/tmp/a b"]);
    }

    #[test]
    fn toml_shape_round_trips() {
        let text = r#"
            command = "$compiler"
            flags = "-std=c11"
            [env]
            path_prepend = ["/x"]
            [compile]
            kind = "steps"
            strip_flags = ["-n"]
            [[compile.bindings]]
            name = "sources"
            source = { kind = "local_sources", family = "c", disable_flags = ["-n"] }
            [[compile.steps]]
            name = "Compile"
            via = ["xcrun"]
            program = "clang"
            args = ["-o", "$output", "$sources"]
            when = ["debugging", { file_exists_up = "go.mod" }]
            produces = { value = "$output" }
        "#;
        let spec: RunSpec = toml::from_str(text).unwrap();
        let CompileSpec::Steps(steps) = &spec.compile else {
            panic!()
        };
        assert_eq!(steps.steps[0].when.len(), 2);
        assert_eq!(steps.steps[0].produces.as_ref().unwrap().var, "compiler");
        let again: RunSpec = toml::from_str(&toml::to_string(&spec).unwrap()).unwrap();
        assert_eq!(spec, again);
    }

    #[test]
    fn home_entries_are_dropped_without_a_home() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = RunSpec::default();
        spec.env.path_append = vec!["$home/.cargo/bin".into(), "/opt/homebrew/bin".into()];
        let plan = spec.plan(&ctx(tmp.path(), "a.rs")).unwrap();
        assert_eq!(plan.path_append, ["/opt/homebrew/bin"]);
    }
}
