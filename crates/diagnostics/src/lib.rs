//! ScratchRunner's diagnostics engine (NSH-free): lint and compile-error
//! handling for the editor and the console.
//!
//! # Pipeline
//!
//! ```text
//!  DiagnosticDef (scratch-language)         buffer text + path
//!        |                                        |
//!   Scheduler  --Plan-->  DiagnosticsRunner  (temp copy, process, merged output)
//!   (when)                     |
//!                         parse::interpret / script / checkstyle / auto
//!                              |  Vec<Diagnostic>     (raw, parser's coordinates)
//!                         JobOutput::annotations / resolve_all
//!                              |  Vec<Annotation>     (byte ranges in the buffer)
//!                         AnnotationSet  (dedupe, ignore, dismiss, counts,
//!                              |          navigation, fixes, edit mapping)
//!                            editor
//! ```
//!
//! # Main types
//!
//! | Area | Types and functions |
//! |--|--|
//! | Model | [`Diagnostic`], [`Severity`], [`Location`], [`Fix`], [`TextEdit`], [`Origin`], [`MatchedLocation`] |
//! | Placed in a buffer | [`Annotation`], [`AnnotationId`] (stable), [`ResolvedFix`], [`resolve`], [`resolve_all`], [`DocumentContext`] |
//! | The editor's set | [`AnnotationSet`]: `replace_source`, `visible`, `counts`, `next_from`/`previous_from` ([`NavFilter`]), `next_fix`/`all_fixes`/`fix_plan`/`auto_fixes` ([`FixPlan`]), `dismiss`, `map_edit` |
//! | Ignoring | [`IgnoreList`] (global, per document), [`AutoFixRules`], [`DocumentDiagnosticsData`] (serde), [`DiagnosticsPrefs`] (serde), [`DisableScope`] |
//! | Parsers | [`parse`]: `parse_unix`, `parse_regex`/[`parse::DiagRegex`], `parse_script_json` (contract documented there), `parse_checkstyle`, `parse_python`, `parse_node`, `parse_rustc`, `parse_rust_panic`, `parse_go_panic`, `parse_java`, `parse_ruby`, `parse_auto`, `interpret`, `infer` |
//! | Built-in checkers | [`builtin`]: `check_conflicts` (Choose First/Both/Last), `check_regex`, `check_syntax` (JSON, TOML, XML) |
//! | Running | [`DiagnosticJob`], [`run_job`], [`DiagnosticsRunner`] (threads, cancel, notifier), [`RunnerEnv`], [`perform_test_run`] -> [`TestRunReport`], [`RunError`], [`MissingTool`] |
//! | Scheduling | [`Scheduler`] (pure: debounce, run gate, cancel-on-new-issues), [`entries_for`], [`Plan`], [`JobRequest`], [`RunGate`] |
//! | Console | [`console::import_console`], [`console::import_into`] |
//!
//! # Example: parse, place, navigate, fix
//!
//! ```
//! use scratch_diagnostics::*;
//! use scratch_diagnostics::parse::parse_unix;
//!
//! let source = "int main(void) {\n  int x = 1\n  return 0;\n}\n";
//! let output = "main.c:2:12: error: expected ';' at end of declaration\n\
//!               fix-it:\"main.c\":{2:12-2:12}:\";\"\n";
//!
//! let diagnostics = parse_unix(output, &ParseOptions::default());
//! let (annotations, _other_files) = resolve_all(&diagnostics, &DocumentContext::new(source));
//!
//! let mut set = AnnotationSet::new();
//! set.replace_source("clang", Origin::Document, annotations);
//! assert_eq!(set.counts().errors, 1);
//!
//! let next = set.next_from(0, NavFilter::ErrorsOnly).unwrap();
//! assert_eq!(next.message, "expected ';' at end of declaration");
//! let plan = set.all_fixes();                       // "Apply All Fixes"
//! assert_eq!(plan.titles, ["Insert ';'"]);
//! assert!(plan.apply_to(source).contains("int x = 1;"));
//! ```
//!
//! # Example: scheduling and running
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Instant;
//! use scratch_diagnostics::*;
//! use scratch_language::LanguageRegistry;
//!
//! let registry = LanguageRegistry::builtin();
//! let language = Arc::new(registry.get("python").unwrap().clone());
//! let prefs = DiagnosticsPrefs::default();
//! let (defs, entries) = entries_for(&language, &prefs, &DocumentDiagnosticsData::default());
//!
//! let mut scheduler = Scheduler::new(SchedulerConfig::from(&prefs));
//! scheduler.set_entries(entries);
//! let runner = DiagnosticsRunner::new().with_notifier(|| { /* wake the UI */ });
//!
//! // On every text change (UI thread):
//! let plan = scheduler.text_changed(Instant::now());
//! # let _ = plan;
//! // From a timer set to `scheduler.next_deadline()`:
//! for request in scheduler.poll(Instant::now()).start {
//!     let def = defs.iter().find(|d| d.id == request.entry_id).unwrap().clone();
//!     let mut job = DiagnosticJob::new(language.clone(), def, "print('hi'\n");
//!     job.generation = request.generation;
//!     runner.spawn(request.token, job);
//! }
//! // When woken:
//! let mut set = AnnotationSet::new();
//! for result in runner.drain() {
//!     let new = match &result.outcome {
//!         Ok(output) => {
//!             let (annotations, _) = output.annotations("print('hi'\n", None);
//!             set.replace_source(&output.source, Origin::Document, annotations).new
//!         }
//!         Err(_) => Counts::default(),
//!     };
//!     let action = scheduler.job_finished(result.token, JobOutcome { new, ok: result.outcome.is_ok() });
//!     let _ = action;
//! }
//! ```
//!
//! # Column and offset conventions
//!
//! Parsers report 1-based lines and columns in the unit the tool uses
//! ([`ColumnUnit`]: bytes for clang/gcc/rustc/go, characters for Python,
//! script and CheckStyle output; UTF-16 units are treated as characters).
//! [`Annotation`] ranges are byte offsets into the buffer; convert to the
//! editor's positions with [`LineIndex::position`] (UTF-16 columns, 0-based).

#![warn(missing_docs)]

pub mod annotation;
pub mod builtin;
pub mod console;
pub mod exec;
pub mod ignore;
pub mod install;
pub mod model;
pub mod parse;
pub mod runner;
pub mod scheduler;
pub mod settings;
pub mod text;

pub use annotation::{
    Annotation, AnnotationId, AnnotationSet, ChangeReport, Counts, DocumentContext, FixPlan,
    NavFilter, ResolvedEdit, ResolvedFix, resolve, resolve_all, same_file,
};
pub use exec::{CancelToken, RunnerEnv};
pub use ignore::{AutoFixRules, IgnoreList, RuleList, like_pattern};
pub use install::{InstallPrompts, InstallationInfo, MissingTool, homebrew_formula};
pub use model::{
    ColumnUnit, Diagnostic, Fix, Location, MatchedLocation, Origin, Severity, TextEdit,
};
pub use parse::{ParseError, ParseOptions};
pub use runner::{
    ClangSettings, DiagnosticJob, DiagnosticsRunner, EnvProvider, JobHandle, JobOutput, JobResult,
    JobToken, RunError, TestRunReport, perform_test_run, run_job,
};
pub use scheduler::{
    CancelReason, Entry, FinishAction, GateStatus, JobOutcome, JobRequest, Plan, RunGate, Scheduler,
    SchedulerConfig, Trigger, entries_for,
};
pub use settings::{
    DiagnosticsPrefs, DisableScope, DocumentDiagnosticsData, effective_diagnostics,
};
pub use text::{EditSpan, LineIndex, TextPos, apply_edits, strip_ansi};
