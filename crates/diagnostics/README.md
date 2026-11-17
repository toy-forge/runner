# scratch-diagnostics

ScratchRunner's diagnostics engine: everything behind the squiggles, the Diagnostics menu and the
console error links. NSH-free (no UI dependency), fast `cargo test -p scratch-diagnostics`.

```text
DiagnosticDef (scratch-language) + buffer + path
      |  Scheduler (pure policy)  ->  Plan { cancel, start }
      v
DiagnosticsRunner / run_job   temp copy of the buffer, process (or built-in checker), merged output
      v
parse::{unix, regex, script JSON, CheckStyle, python, node, rustc, panics, ...}   Vec<Diagnostic>
      v   resolve / JobOutput::annotations
AnnotationSet   dedupe, ignore, dismiss, counts, next/previous, fixes, edit mapping
```

## Public API in one screen

| Need | Use |
|--|--|
| Raw result of a parser | `Diagnostic { file, location: Option<Location>, severity, runtime, message, message_id, code, fixes: Vec<Fix>, children, matched, source, origin }`; `Severity::{Notice, Warning, Error}`; `Fix { title, description, edits: Vec<TextEdit>, preferred }` |
| Place it in a buffer | `resolve_all(&diags, &DocumentContext::new(text).path(p))` -> `(Vec<Annotation>, foreign)`; `Annotation { id (stable hash), range (bytes), whole_line, fixes: Vec<ResolvedFix>, children, ... }`; `Annotation::positions(&LineIndex)` gives UTF-16 editor positions |
| The editor's state | `AnnotationSet`: `replace_source(source, origin, annotations) -> ChangeReport`, `visible()`, `counts()`, `at_offset`, `on_line`, `next_from`/`previous_from(caret, NavFilter::{Any, ErrorsOnly})`, `next_fix(caret)`, `all_fixes()`, `fix_plan(id, i)`, `auto_fixes(&AutoFixRules)` (all `FixPlan` with `apply_to(text)` or `edits_descending()`), `dismiss`, `dismiss_all`, `map_edit(range, new_len)`, `set_global_ignore`, `set_document_ignore` |
| Ignoring / settings | `IgnoreList`/`AutoFixRules` (= `RuleList`, text sheet <-> `from_text`/`to_text`, serde as a list of lines, `add_exact`, `add_like`, `add_for_message`), `DocumentDiagnosticsData` (per document, serde), `DiagnosticsPrefs` (global, serde, CodeRunner defaults), `DisableScope` |
| Parsing | `parse::interpret(output, mode, regex, &opts)`, `parse_unix`, `parse_regex`/`DiagRegex`, `parse_script_json`, `parse_checkstyle`, `parse_python`, `parse_node`, `parse_rustc`, `parse_rust_panic`, `parse_go_panic`, `parse_java`, `parse_ruby`, `parse_auto`, `infer` |
| Built-in checkers | `builtin::check_conflicts` (Choose First / Both / Last), `check_regex`, `check_syntax` (JSON, TOML, XML), `run_builtin(kind, language, text)` |
| Run one diagnostic | `DiagnosticJob::new(language, def, text).file(path).saved(bool).language_flags(..).env(RunnerEnv)`; `run_job(&job, &CancelToken)`; `DiagnosticsRunner::{spawn, spawn_with, try_recv, recv_timeout, drain, cancel, cancel_all}` with `with_notifier` / `with_env_provider`; `perform_test_run(&job, &cancel) -> TestRunReport` ("Perform Test Run...") |
| Decide when | `entries_for(language, &prefs, &doc_data) -> (defs, entries)`; `Scheduler::{text_changed, poll, next_deadline, check_now, run_all, run_entry, before_run, run_gate, abort_gate, after_run, job_finished, cancel_all}` |
| Console | `console::import_console(output, language) -> ConsoleImport { text, diagnostics, highlights }`, `console::import_into(&mut set, &ctx, &import, &prefs)` |
| Missing tools | `RunError::ToolMissing(MissingTool)` (`install_command()` = `brew install ...`), `InstallPrompts` |

See the crate docs (`cargo doc -p scratch-diagnostics --open`) for two complete examples.

## Update mode and options bits (research 10, G37)

`UpdateMode`: 1 on edit, 2 on run, 4 manual (3 = edit+run). `Options`: 1 custom compile flags, 2 add to
existing flags, 4 cancel the run on new errors, 8 on new warnings, 16 on new notices. The language crate
decodes them (`UpdateMode::from_bits`, `DiagnosticOptions::from_bits`); the scheduler enforces them.

* on edit: debounce (`DiagnosticsDelay`, 1.5 s) after the last edit, only with `DiagnosticsWhileTyping`;
  every edit cancels the still-running edit jobs; results of stale edit jobs are discarded.
* on run: `before_run` starts the entries whose results are not current and arms the gate; poll
  `run_gate` (`Waiting` / `Proceed` / `Cancel(reason)`); *new* issues (by message, not position, from
  `ChangeReport::new`) of an entry with a cancel bit cancel the run and the other pre-run jobs.
* manual: `run_entry` ("Run X Now"), `run_all` (Run Diagnostics, Cmd-B), `check_now` (Check Document Now,
  Cmd-;, edit and run entries). One job per entry; starting it again cancels the old one.
* nothing runs when a document opens; console entries are fed after a run (`after_run`).

## Output modes

* **Unix** (`parse_unix`): `file:line:col: message` with `error:`/`warning:`/`note:` inference (`parse::infer`),
  notes as children, `fix-it:` lines as fixes (notes' fixes move up to the diagnostic), `[-Wfoo]`/`[SC2086]`
  tags as message ids, caret lines for ranges (classic and clang's `N | ` style), javac `symbol:`/`location:`
  details, bash `file: line N:`, location-less `error:` lines.
* **Regex** (`DiagRegex`): named groups `file line column message messageID error warning note zeroLine
  zeroColumn endLine endColumn offset endOffset length code`; multi-line mode, duplicate group names allowed
  (Oniguruma habit); lookaround/backreferences are rejected with `ParseError::Regex`. `error|warning|note`
  groups give the severity, otherwise it is inferred from the message, otherwise error.
* **Custom script**: the script gets the raw output on stdin and prints a JSON array on stdout. Exact
  contract (also in the `parse::json` module docs and `DEFAULT_INTERPRETER_SCRIPT`):
  `[{file?, location?: {offset,length | line,column,endLine?,endColumn?, byteOffsets?}, message,
  type?: error|warning|notice|runtime_error|runtime_warning, messageID?, fixes?: [{location, replacement,
  title?, description?} | {edits: [{location, replacement}], title?}], matchedLocations?: [{location, link?:
  ""|url, scopes?: []}], children?: [...]}]`. Lines/columns are 1-based (end column exclusive), offsets are
  0-based characters. Env: `CR_DIAGNOSTICS=1`, `CR_DIAGNOSTICS_DIR`, plus the run variables.
* **CheckStyle** (`parse_checkstyle`, quick-xml): `<file name><error line column severity message source>`.
* Run-time failures: Python tracebacks and syntax errors (`InLine` locations from the caret), Node, Rust
  panics, Go panics, Java exceptions, Ruby backtraces: a runtime error at the innermost user frame with
  `'f' called here` children. `parse_auto` tries everything (used by console import and as the fallback
  when a configured mode finds nothing but the tool failed).

## Running

The buffer is written to a unique temp directory under its real file name, siblings of the real directory
are symlinked next to it (relative includes/imports work), and the temp copy is removed afterwards. With
`saved(true)` the real file is used (keeps `go.mod` etc.). Process env/`PATH` come from `RunnerEnv`
(provide them from the Shell/Advanced preferences; nothing else is inherited), the language's `run.env` is
added. stdout and stderr are merged in order; the process group is killed on cancel or timeout
(default 30 s). Kinds: `Command` (template like a run command), `CompileScript` (the language's compile
steps with the diagnostic's effective flags, output of all steps), `Clang` (`xcrun clang -fsyntax-only
-fdiagnostics-parseable-fixits ...`; carets stay on because clang prints no fix-its without them),
console kinds (interpret `console_output`), built-ins (conflict, regex, invalid syntax).

## Tests

`cargo test -p scratch-diagnostics`: unit tests per module; `tests/formats.rs` parses real captured output
(`tests/fixtures/*.txt` from clang, swiftc, rustc, go, python, node, javac, java, bash, ruby, perl, tsc,
osacompile); `tests/runner.rs` runs real processes (every case needing clang, python3, node, go, rustc,
swiftc or javac skips visibly when the tool is missing).

## Not done / limits

No libclang (the process-based clang front end is used); no Oniguruma lookaround; no YAML syntax check;
UTF-16 columns reported by tools are treated as characters; the `InvalidSyntaxLimitToOne` preference has no
effect because the built-in checkers report one error; the app must persist `DiagnosticsPrefs`,
`DocumentDiagnosticsData` and `InstallPrompts` itself.
