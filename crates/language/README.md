# scratch-language

ScratchRunner's data-driven language system. NSH-free: no UI dependency, fast `cargo test`.

A language is a `LanguageDef` (serde + TOML). 34 definitions are embedded from `defs/*.toml`:
the 29 languages CodeRunner 4.6 bundles (own data, own compile steps), Plain Text, and four
editor-only languages (JSON, YAML, XML, TOML). Users override or add languages with TOML files in a
directory; `LanguageRegistry` merges both.

```rust
use std::path::Path;
use scratch_language::*;

let registry = LanguageRegistry::builtin();                     // + .load_user_dir(dir)
let lang = registry.detect(&DetectInput::new().path(Path::new("/work/main.c"))).language;

// Plan a run (nothing is executed by this crate).
let vars = Vars::for_source(Path::new("/work"), "main.c")
    .with_tmpdir(Path::new("/tmp/run-1"))
    .with_process_env();
let plan = lang.plan_run(&RunContext::new(vars))?;
// plan.compile: CompilePlan::{None, Steps(Vec<PlannedStep>), Shell(ShellPlan)}
// plan.run_argv(shell, login, shell_compile_stdout) -> Vec<String>
```

## Modules and main types

| Module | Contents |
|--|--|
| `model` | `LanguageDef`, `CommentMarkers`, `ConsoleSpec`/`ConsoleType`, `InstallerHint`, `DiagnosticDef` (+`DiagnosticKind`, `UpdateMode`, `DiagnosticOptions`, `OutputMode`), `DocSource`/`DocType`, `IndentStyle`, `EditorRegistration`, `Issue` |
| `run` | `RunSpec` (`command`, `flags`, `env`, `compile`), `CompileSpec::{None, Steps, Shell}`, `StepsSpec`, `VarBinding`/`VarSource`, `BuildStep`, `Condition`, `Requirement`, `Produces`; planning: `RunContext`, `RunPlan`, `CompilePlan`, `PlannedStep` |
| `vars` | `Vars` (the variables of one run), `Value`, `KNOWN_VARIABLES`, `shell_quote`, `split_words`, `encoding_name`, `file_url` |
| `expand` | `expand_command` (argv, no shell), `expand_shell_command` (CodeRunner quoting), `resolve_command`, `expand_arg`, `needs_shell`, `ResolvedCommand::to_argv` |
| `protocol` | CodeRunner's compile-script protocol: `parse_compiler_output`, `CompileExit` (127/69/56) |
| `probe` | pure source probes: `local_sources` (autoinclude), `framework_args`, `java_*`, `swift_sources` |
| `detect` | `DetectInput`/`Detection`, shebang and modeline parsing |
| `template` | `TemplateDef`, `render_template` (`<##>` caret, `___FILENAME___`...), `group_templates` |
| `registry` | `LanguageRegistry`, `MenuEntry`/`MenuOrder`, `LoadReport` |
| `usage` | `LanguageUsage` (use counts and last-used, kept outside the definitions) |
| `import` | `from_cr_language_plist`, `import_cr_language_bundle`, `import_cr_language_path` |

## Run model in one page

`[run]` of a definition:

```toml
[run]
command = "$compiler"            # run command template; per-file override: RunContext::run_command
flags = "-std=c11"               # default compile flags -> $flags; per-file override: RunContext::compile_flags
[run.env]                        # PATH additions and variables for steps, run and diagnostics
path_prepend = ["/Library/TeX/texbin"]
[run.compile]
kind = "steps"                   # "none" | "steps" (declarative, all built-ins) | "shell" (CodeRunner protocol)
[[run.compile.bindings]]         # values computed from the source before the steps
name = "sources"
source = { kind = "local_sources", family = "c", disable_flags = ["-cr-noautoinclude"] }
[[run.compile.steps]]
name = "Compile"
via = ["xcrun"]                  # wrapper; skipped when its program is missing
program = "clang"
args = ["-o", "$output", "$sources", "$flags"]   # one template per argument, never through a shell
debug_args = ["-g"]
when = [{ file_exists_up = "go.mod" }]            # also: unless, require = [{ condition, message }]
mkdirs = ["$build_dir"]
missing_tool = "..."             # shown on "not found"/exit 127
produces = { value = "$output", must_exist = true }   # sets $compiler
```

Variables: `$filename $file $dir $stem $ext $file_url $tmpdir $output $build_dir $flags $compiler
$compiler_url $debugging $encoding $encoding_name $runid $home $language_dir $scripts_dir`, plus
binding names. A list variable alone in an argument (`$flags`, `$sources`) splices.

The run command is a template: `expand_command` tokenises it like a shell (quotes, escapes), then
substitutes, so values can never inject syntax. `${compiler%:*}` style parameter expansion works.
Templates that need real shell syntax (`;`, `|`, redirects, `$(...)`) are detected
(`needs_shell`) and expanded with CodeRunner's rule (`$filename`/`$compiler` outside quotes become
`'quoted'`; the exported environment, `Vars::export_env`, serves the rest) for `<shell> -c`.
`resolve_command` picks the mode. Run arguments are appended without expansion.

User-defined languages use `kind = "shell"` with a compile script following CodeRunner's protocol
(cwd = source dir, `$1..$n` = flags, `CR_*` env, exit 0, last stdout line = `$compiler`, 127 -> installer
hint, 69 Xcode licence, 56 file access) - see `protocol.rs`.

## Runner contract (what the executor of a `RunPlan` does)

1. `plan = def.plan_run(&ctx)?` - `PlanError::Requirement(msg)` is shown to the user as is.
2. `CompilePlan::Steps`: per step create `mkdirs`; spawn `step.argv(true)` (retry `argv(false)` if
   `via[0]` is not installed) in `step.cwd` with `plan.process_env()` + `step.env` and `PATH` extended by
   `plan.path_prepend/path_append`; exit 127 or "not found" -> `step.missing_tool` or
   `def.exit_status_message(127)`; other non-zero -> show output, stop; check `produces.must_exist`.
   `CompilePlan::Shell`: run `script` with `args` as `$1..`, keep stdout.
3. `plan.run_argv(shell, login_shell, shell_compile_stdout)?` and spawn it in the source directory.
   `tests/exec.rs` contains a 60-line reference executor that builds and runs C, C++, Objective-C,
   Swift, Rust, Go, Java and TypeScript with these plans.

## Detection

`LanguageRegistry::detect`: explicit (id/uuid/name) > extension > exact file name > `#!` shebang >
modeline > default language (plain text). Ambiguous extensions (`h`, `js`, `ts`, `cs`) use the
definition that lists the extension in `default_for`; the others are in `Detection::candidates`.
`guess_from_text` (New from Clipboard): shebang, modeline, then weighted `content_patterns`.

## Editor integration

`editor_syntax` is the name for NSH's `Editor::language()`; `LanguageDef::editor_syntax_is_builtin()`
tells whether NSH knows it. AppleScript and LaTeX are not NSH built-ins: register them with the data in
`def.editor_registration()` (`EditorLanguage::new(name).extensions(..).line_comment(..).syntax(hint)`).
`tests/builtins.rs` checks the name list against the pinned NSH source when it is checked out.

## Tests

`cargo test -p scratch-language`: unit tests in every module; `tests/builtins.rs` (integrity of the 34
definitions, plans, diagnostics regexes on real output), `tests/cr_import.rs` (imports the 29 installed
CodeRunner bundles read-only, skipped visibly when absent), `tests/exec.rs` (real builds and runs,
each case skips visibly when its toolchain is missing).

## Deliberate deviations from CodeRunner

* Rust compiles with `--edition 2021` by default; the debugger is `lldb` (research 10, C19).
* Go builds the package when a `go.mod` exists in or above the directory, else the single file.
* Java compiles into a private build directory and runs from there (classpath = build dir).
* TypeScript and LaTeX write into a private build directory, not next to the source.
* Markdown has no run command: `console.renderer = "markdown"` asks the app for an in-process renderer.
* jQuery is loaded from a CDN (no bundled copy); Python's custom-script console parser is a regex.
* Extension lists include a few extras (`mjs`, `cjs`, `mts`, `tsx`, `pyw`, `bash`, `zsh`, ...).
