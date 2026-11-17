//! ScratchRunner's data-driven language system (NSH-free).
//!
//! A *language* is a [`LanguageDef`]: plain data (TOML via serde) that says how
//! files are recognised, highlighted, edited, compiled, run, linted and
//! documented. The app embeds 29 definitions mirroring CodeRunner 4.6's
//! bundled languages plus plain text and a few editor-only ones (`defs/*.toml`),
//! and lets users override them or add their own as TOML files.
//!
//! # Main types
//!
//! | Type / function | Purpose |
//! |--|--|
//! | [`LanguageRegistry`] | built-ins + user overlay, lookup, detection, menus, templates, enable/disable/reset |
//! | [`LanguageDef`] | one language (see its fields) |
//! | [`RunSpec`], [`CompileSpec`], [`BuildStep`] | declarative compile/run description |
//! | [`RunSpec::plan`] -> [`RunPlan`] | concrete argv/cwd/env for the runner to execute |
//! | [`Vars`], [`RunContext`] | the variables of one run |
//! | [`expand_command`], [`expand_shell_command`], [`resolve_command`] | run-command templates |
//! | [`DetectInput`] -> [`Detection`] | language detection (explicit > extension > file name > shebang > modeline) |
//! | [`LanguageRegistry::guess_from_text`] | "New from Clipboard" detection |
//! | [`TemplateDef`], [`render_template`] | new-file templates with `<##>` caret |
//! | [`LanguageUsage`] | use counts / last-used times, kept outside the definitions |
//! | [`from_cr_language_plist`], [`import_cr_language_bundle`] | CodeRunner `.crLanguage` import |
//! | [`DiagnosticDef`], [`DocSource`] | linters and documentation sources |
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//! use scratch_language::{
//!     CompilePlan, DetectInput, LanguageRegistry, RunContext, Vars,
//! };
//!
//! let registry = LanguageRegistry::builtin();
//!
//! // Which language is this file?
//! let detected = registry.detect(&DetectInput::new().path(Path::new("/work/hello.py")));
//! let python = detected.language;
//! assert_eq!(python.id, "python");
//!
//! // Plan a run: nothing is executed here.
//! let vars = Vars::for_source(Path::new("/work"), "hello.py");
//! let plan = python.plan_run(&RunContext::new(vars)).unwrap();
//! assert!(matches!(plan.compile, CompilePlan::None));
//! let argv = plan.run_argv("/bin/bash", false, None).unwrap();
//! assert_eq!(argv, ["python3", "hello.py"]);
//! ```
//!
//! # How the runner uses a plan
//!
//! 1. `plan = def.plan_run(&ctx)?` (see [`RunContext`]).
//! 2. For [`CompilePlan::Steps`]: for each [`PlannedStep`] create `mkdirs`, spawn
//!    `step.argv(true)` (retry with `argv(false)` if the `via` wrapper is not
//!    installed) in `step.cwd` with `plan.process_env()` + `step.env`, PATH
//!    extended by `plan.path_prepend`/`path_append`; on exit 127 show
//!    `step.missing_tool` or `def.exit_status_message(127)`; on other non-zero
//!    exit stop and show the output; check `produces.must_exist`.
//!    For [`CompilePlan::Shell`] run the script by the protocol in [`protocol`]
//!    and keep its stdout.
//! 3. `argv = plan.run_argv(shell, login, stdout_of_shell_compile)?`; spawn it in
//!    the source directory.

pub mod builtin;
pub mod detect;
mod error;
pub mod expand;
mod grammar;
mod import;
mod model;
pub mod probe;
pub mod protocol;
mod registry;
mod run;
mod template;
mod usage;
mod vars;

pub use detect::{
    DetectInput, DetectReason, Detection, Guess, extension_candidates, interpreter_matches,
    modeline_hints, parse_content_pattern, shebang_interpreter,
};
pub use error::{ExpandError, LanguageError, PlanError};
pub use expand::{
    ResolvedCommand, expand_arg, expand_command, expand_shell_command, expand_string, expand_words,
    needs_shell, referenced_variables, resolve_command,
};
pub use grammar::{GrammarInfo, grammar_info};
pub use import::{
    Imported, from_cr_language_plist, import_cr_language_bundle, import_cr_language_path, slugify,
};
pub use model::{
    CommentMarkers, ConsolePosition, ConsoleSpec, ConsoleType, DiagnosticDef, DiagnosticKind,
    DiagnosticOptions, DocSource, DocType, EditorRegistration, IndentStyle, InstallerHint, Issue,
    LanguageDef, NSH_BUILTIN_EDITOR_LANGUAGES, OutputMode, Severity, UpdateMode,
    is_nsh_builtin_editor_language, split_group_variant,
};
pub use probe::{
    SourceFamily, framework_args, java_main_class, java_package, java_source_relative,
    java_source_root, local_sources, sanitize_identifier, swift_sources,
};
pub use protocol::{CompileExit, RunPhase, parse_compiler_output};
pub use registry::{LanguageRegistry, LoadReport, MenuEntry, MenuOrder, PLAIN_TEXT_ID};
pub use run::{
    BuildStep, CompilePlan, CompileSpec, Condition, EnvSpec, PlannedProduces, PlannedStep,
    Produces, Requirement, RunContext, RunPlan, RunSpec, ShellPlan, ShellSpec, StepsSpec,
    VarBinding, VarSource,
};
pub use template::{
    CARET_MARKER, FIRST_LAUNCH_LANGUAGE, FIRST_LAUNCH_TEXT, RenderedTemplate, TemplateContext,
    TemplateDef, TemplateGroup, find_placeholders, group_templates, render_template,
};
pub use usage::{LanguageUsage, UsageEntry};
pub use vars::{
    KNOWN_VARIABLES, Value, Vars, encoding_name, file_url, percent_encode, shell_quote, split_words,
};

