# ScratchRunner — Product & Technical Specification

**Version:** 1.0  
**Status:** Implemented (sections marked *Not implemented* describe code that does not exist yet)  
**Platform:** macOS 14+  
**Primary Language:** Rust  
**UI Framework:** Native Shell (NSH)  
**License:** Apache-2.0

---

# 1. Product Overview

## 1.1 Vision

ScratchRunner is a lightweight, native macOS scratchpad for writing, editing, and executing code with modern language intelligence.

It combines:

- The simplicity of CodeRunner
- The editing experience of modern code editors
- LSP-powered completion, diagnostics, hover, and navigation
- Fast startup and minimal resource usage
- A focused, distraction-free native interface

ScratchRunner is not intended to be a full IDE.

**Core principle: Open → Write → Run → See Results.**

No project setup should be required for common scripting workflows.

## 1.2 Problem Statement

Existing code editors and IDEs are optimized for project-based development.

For small tasks such as testing a Python snippet, experimenting with JavaScript, editing JSON, or executing a Shell script, they introduce unnecessary overhead.

Typical problems include:

- Slow application startup
- Excessive background services
- Complex project and workspace management
- Distracting UI panels
- Unnecessary project creation
- Heavy resource consumption

ScratchRunner removes this overhead while preserving essential editing intelligence.

## 1.3 Product Goals

### Primary Goals

1. Launch quickly into an editable scratchpad.
2. Execute code with a single keyboard shortcut.
3. Support multiple programming languages.
4. Provide syntax highlighting and code completion.
5. Integrate language servers without requiring a project.
6. Preserve unsaved work automatically.
7. Maintain low idle memory and CPU usage.
8. Provide a native macOS experience.

### Non-Goals

The following are explicitly outside the initial scope:

- Full project management
- Integrated Git interface
- Remote development
- SSH development environments
- AI coding agents
- Collaborative editing
- Debug Adapter Protocol (DAP); the debugger drives command-line debuggers through its own TOML adapter format instead
- Extension marketplace
- Full IDE refactoring workflows
- Cloud synchronization

---

# 2. Target Users and Use Cases

## 2.1 Target Users

- Software engineers
- SRE / DevOps engineers
- Data engineers
- Backend developers
- Script authors
- Developers experimenting with APIs
- Users who need a fast temporary text editor

## 2.2 Primary Use Cases

### UC-01: Execute a Python Snippet

1. Launch ScratchRunner.
2. An empty editor appears immediately.
3. Select Python or use the default language.
4. Type code.
5. Press `Cmd + R`.
6. Execution results appear in the Output panel.

The user is not required to save the file.

### UC-02: Edit Temporary Text

1. Launch ScratchRunner.
2. Type arbitrary text, JSON, Markdown, or configuration content.
3. Copy the content elsewhere or close the window.
4. ScratchRunner automatically preserves the draft.

### UC-03: Use LSP Completion

1. Open a Python scratchpad.
2. Start typing a library API.
3. ScratchRunner lazily starts the configured language server.
4. Completion suggestions appear.
5. The user can inspect hover information and diagnostics.

### UC-04: Run a Shell Script

1. Select Shell as the language.
2. Write a script.
3. Press `Cmd + R`.
4. View stdout, stderr, execution time, and exit status.

### UC-05: Open and Run an Existing File

1. Press `Cmd + O`.
2. Select an existing source file.
3. ScratchRunner detects the language from its extension.
4. Edit and execute it.
5. Save using `Cmd + S`.

---

# 3. Technology Stack

## 3.1 Primary Dependencies

| Component | Technology | Responsibility |
|---|---|---|
| Application | Rust | Core application |
| UI | NSH (`nsh-core`, `nsh-base`, `nsh-platform-macos`) | Native macOS interface |
| Editor | `nsh-base::Editor` | Text editing and UI |
| Syntax highlighting | Syntect + `.tmTheme` (`scratch-theme`) | Themes and incremental highlighting |
| Language definitions | TOML `.crLanguage`-style defs (`scratch-language`) | Detection, run/compile specs, diagnostics |
| LSP protocol | Hand-rolled JSON-RPC over stdio (`scratch-lsp`) | Language intelligence |
| Script execution | PTY + pipes (`scratch-runner`, `libc`; no async runtime) | Process management |
| Async runtime | Tokio (`scratch-lsp` only) | Language-server IO |
| Debugger | TOML adapter format (`scratch-debugger`) | lldb, pdb, node, jdb, bashdb, perl, phpdbg |
| Configuration | Serde + TOML (`scratch-config`) | User settings |
| Persistence | Filesystem (`scratch-document`) | Sessions, unsaved documents, recents |
| Logging | `tracing` + `env_logger` | Diagnostics |

Use NSH's existing abstractions rather than duplicating editing functionality.

## 3.2 Framework Selection

Repository:

https://github.com/longbridge/nsh

Required NSH components:

- `nsh-core`
- `nsh-base`
- `nsh-platform-macos`
- NSH Editor
- NSH layout and UI primitives

The application should use the native AppKit backend for macOS.

Avoid embedding a browser or WebView for the code editor.

Pin NSH to a tested release or commit. NSH is a young framework, so APIs must be checked against the pinned version before implementation.

## 3.3 NSH Editor Capabilities

The editor already includes:

- Multiple cursors
- Undo / redo
- Code folding
- Syntax highlighting
- Auto indentation
- Bracket matching
- Automatic bracket closing
- Diagnostics rendering
- Completion UI (wired to `scratch-lsp`, with keyword/path/word fallbacks)
- Hover interaction hooks (wired; the hover card shows LSP and diagnostic content)
- Large-file virtualization
- Native input method handling

Search and replace use the app's own Find bar (`src/workbench/find.rs` +
`scratch-search`). Go-to-definition, signature help and code-action UI are *not
implemented* — the LSP client supports the requests but no editor surface calls
them yet.

LSP process management and protocol synchronization are `scratch-lsp`
responsibilities.

---

# 4. Application Architecture

## 4.1 High-Level Architecture

```text
ScratchRunner (root crate, src/workbench/*)
│
├── Workbench
│   ├── Title band, toolbar, tab bar, status bar
│   ├── Navigator sidebar + project Search view
│   ├── Console (output, filter, input, debugger pane)
│   ├── Documentation panel, Find bar, Quick Open
│   ├── Preferences window, Run Settings sheet
│   └── Command/key-binding layer (scratch-commands)
│
├── Editor (nsh-base::Editor)
│   ├── Document model (scratch-document)
│   ├── Language detection (scratch-language)
│   └── Syntax highlighting (scratch-theme)
│
├── Language Intelligence (scratch-lsp)
│   ├── Server catalogue + discovery + lazy start
│   ├── Virtual workspaces for unsaved documents
│   ├── Document synchronization
│   └── Completion, hover, diagnostics (wired);
│       definition, signature help, code actions, rename (engine only)
│
├── Run engine (scratch-runner)
│   ├── Language-driven compile stages + run command
│   ├── PTY execution, CR_* environment contract
│   ├── stdin / working directory / timeout / output caps
│   └── Cancellation via process-group signals
│
├── Diagnostics engine (scratch-diagnostics)
│   └── Language-driven output parsers, annotations, fixes
│
├── Debugger (scratch-debugger)
│   └── TOML adapters, sessions, breakpoints, variables, REPL
│
├── Search (scratch-search)
│   └── Find/replace engine, project search, QuickNav providers
│
├── UI kit (scratch-ui-kit)
│   └── Tokens, typography, icons, motion, control primitives
│
├── Console model (scratch-console-model)
│   └── ANSI/VT parsing, bounded line store, filters, links
│
├── Persistence
│   ├── scratch-config (settings.toml, paths, schema)
│   └── scratch-document (Unsaved store, session, recents, watch)
│
└── Platform
    ├── macOS AppKit (nsh-platform-macos, objc2)
    ├── Filesystem, clipboard
    └── Process environment
```

## 4.2 Core Design Rules

1. UI must never block while a language server starts.
2. Language servers must be started lazily.
3. Script execution must happen outside the UI process.
4. Unsaved documents must be executable.
5. Every document must have a stable internal ID.
6. Code execution and LSP operations must use separate processes.
7. A language server failure must never crash the application.
8. All execution output must be captured asynchronously.
9. Closing the application must not lose pending drafts.
10. No network communication should occur unless required by an explicitly enabled feature or an invoked tool.

---

# 5. User Interface Specification

## 5.1 Reference Design

The visual source of truth is [DESIGN.md](DESIGN.md) and
[docs/design/screens.md](docs/design/screens.md) — the implemented CodeRunner-style
workbench, not the original four-region concept sketch.

The window, top to bottom:

```text
┌─────────────────────────────────────────────────────────────┐
│ ● ● ●   [Py] scratch.py – Edited                  (title band) │
│ [lang▾] [⌄][▶ Run][⌄] [◀ ▶]        [nav][console][docs] (toolbar) │
│ [Py] permute.py ●  [Sh] hello.sh ✕                      + ⌄  (tab bar) │
├──────────┬─────────────────────────────────────┬────────────┤
│          │  1  import requests                  │            │
│ File     │  2                                  │  Docs      │
│ navigator│  4  response = requests.get(         │  panel     │
│ or       │                                     │  (toggle)  │
│ Search   │────── console (bottom or right) ─────│            │
│          │  [debugger pane |] output, filter, input line     │
├──────────┴─────────────────────────────────────┴────────────┤
│ ln 12, col 5        Python · Pyright ✓   Ready   (status bar) │
└─────────────────────────────────────────────────────────────┘
```

Overlaid on top when open: the Find bar (top of the editor), Quick Open, the
Preferences window, the Run Settings sheet, the LSP status sheet and the
diagnostics overlays.

## 5.2 Layout

### Title band and toolbar

The app draws its own title band over a hidden system title bar: traffic
lights left, the active document's language tile and name (with an `– Edited`
suffix) centered. The band is a drag region and supports double-click zoom.

Below it, the toolbar holds the language popup, the Run group (Run/Stop with
a hold menu of Run, Run and Debug, Run Without Debugging, and Compile/Run
Diagnostics), the back/forward history buttons and the panel toggles
(navigator, console, documentation).

### Tab bar

One pill per document: language tile (spinner while running), title, dirty
dot, close button. Trailing `+` creates a document; `⌄` lists all tabs.
Visible by default; `General > Tabs` or View > Show/Hide Tab Bar turns it off.

### Sidebar (left)

The file navigator (`scratch-navigator`) or the project Search view, switched
in place by the header's search button. Hidden with ⇧⌘H.

### Editor and console

The center is a resizable split: editors on one side, the console on the other
— below the editor by default, on its right via the View menu. The console
holds the run output (ANSI-colored, linkified, bounded), a filter field, an
output-stream popup, Stop/Clear/Hide controls and the program-input line.
While a debug session is live, the debugger pane (stack and variables) sits
beside it. Languages with a `switchable` console (Markdown) get a
Preview/Terminal toggle; PDF outputs rasterize pages.

Run states shown in the console: running, exit code/signal, duration, CPU and
peak memory, compile failures, cancellation, timeout and output-limit
truncation.

The console auto-opens on run and can auto-hide; it stays out of the way
until the first execution.

### Documentation panel (right)

Toggled with ⇧⌘I or the toolbar: a "Search Docs" field, an Automatic Lookups
toggle and one chip per documentation source of the active language, over a
web view. Hidden by default.

### Status bar

Cursor position, the document's language and language-server state, and the
run/notice status.

## 5.3 Theme

Editor colors come from TextMate `.tmTheme` files via `scratch-theme` — a
built-in set plus user themes under `~/Library/Application
Support/ScratchRunner/Themes/`. Chrome colors are `scratch-ui-kit` tokens.

`Preferences > Themes` selects the theme; light and dark appearances can use
separate themes (`use_separate_color_themes`), and the window chrome can match
the theme's light or dark nature.

## 5.4 Typography

The editor font family, size, ligatures, line spacing and smoothing are
configurable in `Preferences > Text Editing > Appearance` (SF Mono by
default).

## 5.5 Window Behavior

- Initial size: 960 × 640 points (no enforced minimum)
- Window frames, split positions and pane visibility persist
  (`window-state.json`)
- Native macOS fullscreen
- One window with multiple document tabs; multiple windows are *not
  implemented* (`File > New Window` opens a new tab)

---

# 6. Document Management

## 6.1 Document Model

Each document (`scratch_document::Document`) carries:

```rust
pub struct Document {
    pub id: DocumentId,            // stable internal identity
    pub path: Option<PathBuf>,     // file backing, when saved
    pub untitled: bool,
    pub language: String,          // language id
    pub language_locked: bool,     // explicit user selection
    pub grammar: Option<String>,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub line_ending: LineEnding,
    pub read_only: bool,
    pub locked: bool,
    pub dirty: bool,
    pub pristine: bool,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub revision: u64,
    // ...
}
```

NSH Editor remains the editing source of truth; the document model tracks
metadata, file state (external changes, encodings, line endings) and
annotations.

## 6.2 Document Types

### Scratch Document

- No user-visible file required
- Automatically assigned a stable internal ID
- Automatically persisted
- Can be executed immediately
- Can be converted into a normal file with Save As

### File-backed Document

- Associated with a filesystem path
- Supports opening and saving
- Supports automatic language detection
- Uses the file's containing directory as the default working directory

## 6.3 Autosave

Edits feed an `AutosaveScheduler` debounce: a document is due 1.5 s after its
last edit, or after 10 s of continuous typing, whichever comes first. Untitled
documents are real files in the `Unsaved/` store, so they can be executed
directly; file-backed documents keep their unsaved edits alongside.

Requirements met by `scratch-document`:

- Asynchronous disk writes
- Atomic file replacement
- No blocking of the editor thread
- `UnsavedStore::recover` restores documents on launch and
  `cleanup_orphans` removes stale files
- Flush pending edits during normal shutdown
- Recover from incomplete writes using the previous valid snapshot
- The store directory is `~/Library/Application Support/ScratchRunner/Unsaved/`
  by default and configurable via the hidden `advanced.autosave_documents_directory`
  preference

The storage layout (`scratch_config::Paths`):

```text
~/Library/Application Support/ScratchRunner/
├── settings.toml          preferences (sparse: only non-default values written)
├── Languages/             user language bundles
├── Themes/                user themes
├── KeyBindings/           user key-binding sets
├── Debuggers/             user debugger bundles
├── Docs/                  documentation source definitions
├── Unsaved/               untitled documents, as real files
├── session.json           open windows and tabs
├── window-state.json      window frames, split positions, pane visibility
├── recent.json            recent documents
└── logs/
~/Library/Caches/ScratchRunner/
└── lsp-workspaces/        one virtual workspace per unsaved document (LSP)
```

## 6.4 Session Restoration

On relaunch, `session.json` restores the open windows, their tabs, the active
document and each document's language and unsaved content; `window-state.json`
restores window frames, split positions and pane visibility. Per-file Run
Settings and the console's state are kept per document.

If there was no previous session, open one empty scratchpad.

---

# 7. Programming Language Support

## 7.1 Language Definitions

`scratch-language` embeds 34 TOML definitions (`crates/language/defs/*.toml`,
CodeRunner `.crLanguage`-compatible): 29 mirroring the CodeRunner 4.6 set plus
Plain Text and four editor-only additions.

Runnable (`[run]` section — 28):

| Group | Languages |
|---|---|
| Compiled | C, C++, Objective-C, Objective-C++, Rust, Swift, Go, Java, Kotlin, Haskell, C# (dotnet / mono / script), LaTeX |
| Interpreted | Python, Shell (bash), Ruby, Perl, PHP, Lua, AppleScript, JavaScript (Node), TypeScript (Node) |
| Output-only | HTML, CSS, JavaScript/TypeScript browser variants (emit markup) |

Editor-only (no run command — 6): Plain Text, JSON, Markdown, TOML, XML, YAML.

Each definition carries its own capabilities — extensions, filenames, shebangs,
modelines and content patterns for detection; comments; a run command plus
optional compile steps; diagnostics specs (compiler/linters and console
highlight patterns); templates; documentation sources; a `debugger` adapter
name where supported.

The LSP catalogue (`crates/lsp/data/servers.toml`) lists 23 servers keyed to
language ids: Pyright, TypeScript Language Server, bash-language-server,
rust-analyzer, gopls, clangd, sourcekit-lsp, JDT LS, Intelephense, Solargraph,
Lua LS, OmniSharp, Kotlin LS, vscode's HTML/CSS/JSON servers, Taplo, LemMinX,
Marksman, TexLab, Haskell LS and Perl Navigator. None are bundled; the first
executable found on `PATH` or in the usual install locations is used, with an
install hint when none is found.

## 7.2 Language Detection

Determine the language using, in order:

1. Explicit user selection (`language_locked`)
2. File extension
3. Filename conventions (`filenames`, e.g. `SConstruct`, `Makefile`)
4. Shebang (`shebangs`)
5. Editor modelines (`modelines`)
6. Content patterns (`content_patterns`, weighted regexes)
7. Default language

Never unexpectedly change the language of an explicitly configured scratchpad.

---

# 8. Script Execution System

## 8.1 Execution Requirements

Pressing `Cmd + R` must execute the current document.

The application must support:

- Unsaved documents
- Saved files
- stdout
- stderr
- Exit status
- Process duration
- Cancellation
- Runtime selection
- Working directory selection
- Optional stdin
- Environment variables

## 8.2 Run Engine

`scratch-runner` (NSH-free, no async runtime) turns a document into processes
plus a stream of events:

```rust
// crates/runner/src/request.rs — everything one run needs
pub struct RunRequest { /* language + Snapshot + RunSettings +
                           ShellSettings + RunOptions + RunMode */ }

let request = RunRequest::new(language, Snapshot::text("print(1)"))
    .with_shell(shell_settings)          // global Preferences > Shell
    .with_settings(run_settings)         // per-file Run Settings sheet
    .with_owner("document-1", OnBusy::Reject);
```

- `Runner::start` / `start_channel` start a run and stream `RunMessage`s:
  `Preparing`, `StageStarted/Finished`, `Output { phase, stream, bytes }`,
  `Started`, `LargeOutput`, `Truncated`, `Compiled`, `Finished`.
- `RunHandle` gives `cancel()`, `write_stdin()`, `send_eof()`, `resize()`,
  `interrupt()`, `signal()`, `allow_large_output()` and `sample_stats()`.
- `RunSummary`/`RunStatus`: `Exit(code)`, `Signal`, `Cancelled`, `TimedOut`,
  `OutputLimitExceeded`, `CompileFailed { stage }`, `Compiled`, `Error`.
- The language definition plans the run: compile stages (e.g. `rustc -o`) then
  the run command, expanded from `$filename`, `$compiler`, `$output`, `$flags`.
- `build_environment` layers the `CR_*` environment contract; `warm_up`
  computes the automatic `PATH` and developer directory off the UI thread at
  launch; `preflight` turns missing tools into actionable errors.
- The program runs on a PTY with stderr on its own pipe; `process::PtyConsole`
  provides a pty without a child for debuggers.

## 8.3 Execution Lifecycle

```text
User presses Cmd + R
        │
        ▼
Capture document snapshot
        │
        ▼
Resolve language run configuration
        │
        ▼
Prepare temporary source if required
        │
        ▼
Spawn child process
        │
        ▼
Stream stdout / stderr
        │
        ▼
Update Output panel
        │
        ▼
Process finishes or is cancelled
        │
        ▼
Report exit status and duration
        │
        ▼
Clean temporary resources
```

## 8.4 Unsaved Document Execution

Unsaved documents are executed using temporary source files (`Snapshot::Text`).

Example:

```text
$TMPDIR/ScratchRunner/
└── run-XXXXXX/          (mkdtemp, mode 0700 — TempRun)
    ├── main.py          unsaved document
    ├── scratch          build products, compile script
    └── Program Input    optional stdin file
```

The run directory is exposed to the program as `CR_TMPDIR` and removed when
the run ends (unless kept for diagnostics). The filename keeps the document's
name when it matches the language; Java is named after its main class and
Swift is `main.swift`.

Requirements:

- Use unique directories per execution.
- Use appropriate file extensions.
- Never execute user text by concatenating it into a shell command.
- Pass arguments directly to the process API.
- Clean temporary files after execution.
- Keep each execution's source snapshot immutable.
- Prevent stale executions from overwriting newer output.

## 8.5 Working Directory

The `WorkingDirectory` setting (per-file Run Settings) resolves to:

1. `FileDirectory` (default): the file's directory. An unsaved document
   without a directory runs in its run's temporary directory; a snapshot may
   carry a `directory` (locked document, project member) used here so
   relative paths keep working.
2. `ProjectFolder`: the folder opened in the window, else the file's
   directory.
3. `Default`: the configured default directory, else the user's home.
4. `Custom(path)`: that directory.

## 8.6 Python Execution

Default command (from `python.toml`):

```text
python3 $filename
```

plus a `compile` syntax-check command used by Compile Diagnostics.

Runtime resolution: `preflight` checks the language's compiler command
against `PATH` and developer-tools locations and surfaces an actionable error
when missing. Virtual-environment detection and a user-selected interpreter
per language are *not implemented*.

## 8.7 JavaScript and TypeScript Execution

JavaScript:

```text
node $filename
```

TypeScript:

```text
node $compiler        # $compiler compiles the .ts first, then node runs it
```

Bun and tsx fallbacks are *not implemented* — the language definitions run
with Node only.

## 8.8 Rust Execution

Standalone Rust snippets:

```text
rustc $compiler-flags $filename -o $output
$output
```

via the `compile` stage in `rust.toml`. `cargo run` is *not implemented*;
external crates need a Cargo project the user runs themselves.

## 8.9 Process Management

Every active execution has a unique execution ID.

The process manager must support:

- Running state
- Cancellation
- Timeout
- Exit status
- Streaming output
- Cleanup
- Prevention of duplicate-run races

On macOS, cancellation should target the relevant process group when the command spawns child processes, with an escalation strategy if graceful termination fails.

Do not terminate unrelated processes.

## 8.10 Runtime Availability

When a runtime or compiler is not installed, `preflight` surfaces an
actionable error (e.g. "command not found: python3" plus the install hint from
the language definition's `error` table).

The application must not crash.

Do not silently install runtimes or language servers.

---

# 9. LSP Architecture

## 9.1 LSP Design Philosophy

LSP must be optional, asynchronous, and isolated.

ScratchRunner should remain usable when:

- No language server is installed
- The language server is starting
- The language server crashes
- The user is offline
- LSP is disabled

## 9.2 LSP Manager

`scratch-lsp`'s `LspManager` (crates/lsp/src/manager.rs) is driven over a
`mpsc` channel (`LspCommand`/`LspEvent`) by the workbench's LSP glue
(`src/workbench/lsp.rs`).

Responsibilities:

- Resolve servers from the embedded catalogue plus user overrides
- Start servers lazily per workspace root
- Manage server lifecycle and capabilities
- Synchronize documents (`didOpen`/`didChange`/`didClose`)
- Route requests and forward notifications
- Restart crashed servers
- Report status (`LspStatus`) to the status bar and the LSP status sheet

## 9.3 Lazy Initialization

When the user opens a document:

1. The editor appears immediately.
2. Syntax highlighting becomes available independently.
3. LSP configuration is resolved asynchronously.
4. The server starts only when required.
5. The status bar shows its state.

Possible states:

```text
Disabled
Not Installed
Starting
Ready
Error
Stopped
```

## 9.4 Virtual Workspace

Every scratch document must have an internal document identity that can be mapped to a valid LSP document URI.

For servers that require filesystem-backed content, ScratchRunner may maintain a stable virtual workspace on disk.

Implemented as:

```text
~/Library/Caches/ScratchRunner/lsp-workspaces/
└── <workspace>/
    └── <document>.py
```

Important distinction:

- LSP document identity is persistent for the lifetime of the document.
- Execution snapshots are temporary and may change for every run.

Execution snapshots are not used as the LSP document identity.

## 9.5 LSP Document Synchronization

Implement:

- `initialize`
- `initialized`
- `textDocument/didOpen`
- `textDocument/didChange`
- `textDocument/didSave` where appropriate
- `textDocument/didClose`
- `shutdown`
- `exit`

Support server-negotiated full or incremental synchronization.

Every edit must maintain a monotonically increasing document version.

LSP positions must correctly convert between UTF-8 text offsets and negotiated LSP position encodings.

NSH Editor already exposes UTF-16-based editor positions, which is useful for many language servers, but the implementation must honor the encoding selected during server initialization.

## 9.6 Completion

Use NSH Editor's completion request and presentation APIs.

Required behavior:

- Request completions at the cursor.
- Display completion candidates.
- Support keyboard navigation.
- Accept selected completion.
- Apply text edits safely.
- Respect document versions.
- Ignore stale responses.

Initial completion features:

- Label
- Kind
- Detail
- Documentation
- Insert text
- Text edit
- Additional text edits when supported

Snippet completion can be deferred if the existing editor API does not fully support LSP snippet placeholders.

## 9.7 Hover

When the user hovers over a symbol:

1. Delay the request briefly.
2. Send `textDocument/hover`.
3. Convert server markup into safe UI content.
4. Display the response near the target symbol.
5. Dismiss on cursor movement or Escape.

Do not allow hover requests to block typing.

## 9.8 Signature Help

**Not implemented.** The LSP client supports `textDocument/signatureHelp`,
but no editor surface requests or displays it yet.

## 9.9 Diagnostics

Listen for `textDocument/publishDiagnostics` and supported pull-diagnostics workflows.

Map diagnostics to NSH Editor's diagnostic structures.

Severity levels:

- Error
- Warning
- Information
- Hint

Requirements:

- Underline affected text.
- Show diagnostic details on hover.
- Avoid displaying stale diagnostics.
- Clear diagnostics when the document closes.
- Respect related locations and code descriptions.

## 9.10 Go to Definition

**Not implemented.** The LSP client supports `textDocument/definition`, but
no editor surface calls it yet — the navigator's symbol menu is powered by
`scratch-search`, not LSP.

## 9.11 Code Actions

**Not implemented** in the editor. The LSP client supports
`textDocument/codeAction` and `textDocument/rename`; the diagnostics engine's
"Apply Fix/Apply All Fixes" comes from language-definition fix patterns
(`scratch-diagnostics`), not LSP code actions.

## 9.12 Language Server Configuration

Per-server overrides live in `settings.toml` under `[lsp.<id>]` (preserved as
an extra table by `scratch-config`, parsed by `scratch-lsp`):

```toml
[lsp.pyright]
enabled = false                              # turn a server off
command = "basedpyright-langserver"          # program instead of the catalogue's
args = ["--stdio"]                           # arguments
initialization_options = { }                 # initialize payload
settings = { }                               # workspace/didChangeConfiguration
search_paths = ["~/bin"]                     # extra executable directories
```

The catalogue ships 23 definitions (`crates/lsp/data/servers.toml`); no
language server is bundled — the first matching executable wins, with an
install hint in the LSP status sheet when none is found.

---

# 10. Keyboard Shortcuts

Shortcuts live in the command table (`crates/commands/src/table.rs`) with
user-rebindable sets under `KeyBindings/` (`default`, `global`, `vim`).

File and tabs:

| Shortcut | Action |
|---|---|
| `Cmd + N` | New window |
| `Cmd + T` | New tab |
| `Cmd + O` | Open file |
| `Cmd + Shift + O` | Open Quickly (tabs, files, typed paths) |
| `Cmd + S` / `Cmd + Shift + S` | Save / Save As |
| `Cmd + W` | Close tab or window |
| `Cmd + {` / `Cmd + }` | Previous / next tab |
| `Cmd + 1…9` | Switch tab |
| `Opt + Cmd + {` / `}` | Move tab left / right |
| `Ctrl + Cmd + V` | New tab from clipboard |

Run and debug:

| Shortcut | Action |
|---|---|
| `Cmd + R` | Run current document |
| `Cmd + .` | Stop execution |
| `Opt + Cmd + R` | Run and Debug |
| `Ctrl + Cmd + R` | Run Without Debugging |
| `Cmd + B` | Compile / Run Diagnostics |
| `Cmd + K` | Clear console |
| `Cmd + Shift + R` | Edit Run Settings |
| `Opt + Cmd + L` | Select Language |
| `Cmd + \` | Toggle breakpoint |
| `Cmd + Y` | Clear breakpoints |
| `Ctrl + 1…4` | Pause / step over / step in / step out |

Find and edit:

| Shortcut | Action |
|---|---|
| `Cmd + F` / `Opt + Cmd + F` | Find / Find and Replace |
| `Opt + Shift + Cmd + F` | Search in File System |
| `Cmd + G` / `Cmd + Shift + G` | Find next / previous |
| `Cmd + E` | Use selection for Find |
| `Cmd + L` | Go to Line |
| `Cmd + /` | Un/Comment lines |
| `Cmd + D` | Duplicate line |
| `Cmd + Shift + Delete` | Delete line |
| `Cmd + [` / `Cmd + ]` | Shift left / right |
| `Opt + Cmd + ↑ / ↓` | Swap lines up / down |

View and app:

| Shortcut | Action |
|---|---|
| `Cmd + Shift + C` | Show / hide console |
| `Cmd + Shift + H` | Show / hide file navigator |
| `Cmd + Shift + I` | Show / hide documentation view |
| `Opt + Shift + Cmd + T` | Hide tab bar |
| `Opt + Cmd + ;` / `Opt + Shift + Cmd + ;` | Apply next fix / all fixes |
| `Opt + Cmd + ← / →` | Previous / next diagnostic |
| `Cmd + +` / `Cmd + -` / `Cmd + 0` | Bigger / smaller / actual font size |
| `Cmd + ,` | Preferences |
| `Cmd + ?` | ScratchRunner Help |
| `Esc` | Show completions / dismiss overlays |

Editor-owned shortcuts (cursor movement, selection, word ops, `Ctrl + Space`
completion, `Cmd + I` look-up) come from the `default`/`vim` binding sets and
NSH Editor. `Cmd + .` is handled context-sensitively between process
cancellation and editor cancel operations.

---

# 11. Configuration

## 11.1 Configuration File

```text
~/Library/Application Support/ScratchRunner/settings.toml
```

## 11.2 Example

`settings.toml` is written sparsely — only values that differ from defaults
are persisted, comments and unknown tables are preserved across saves, and a
`version` header drives migrations. Top-level sections: `general` (documents,
tabs, console defaults, file navigator, notifications, updates),
`text_editing.*`, `languages`, `themes`, `key_bindings`, `shell`, `debugger`,
`printing`, `advanced`, `ui` (window/pane UI state), `misc`:

```toml
version = 1

[general.tabs]
prefer_tabs = true

[text_editing.appearance]
syntax_highlighting = true
wrap_lines = false

[text_editing.appearance.font]
family = "SF Mono"
size = 14
ligatures = true

[text_editing.appearance.line_numbers]
enabled = true

[text_editing.indentation]
soft_tabs = true
tab_width = 4

[themes]
light = "ScratchRunner Light"
dark = "ScratchRunner Dark"

[lsp.pyright]
command = "basedpyright-langserver"
args = ["--stdio"]
```

Top-level tables that do not belong to `scratch-config` (`[lsp.*]`,
`[execution]`, `[runtimes]`, `[templates]`, `[misc]`) are preserved verbatim
for the crates that own them.

All settings have sensible defaults. A file that fails to parse is copied to
`settings.toml.bak` and defaults are used — the application never crashes on
invalid configuration.

---

# 12. Performance Requirements

Performance is a primary product requirement.

The initial targets below are engineering goals, not verified NSH performance measurements.

## 12.1 Startup

Target environment:

- Apple Silicon Mac
- Release build
- Local filesystem
- No restored network activity

Targets:

| Metric | Target |
|---|---|
| Warm start to interactive editor | Under 300 ms |
| Cold start to interactive editor | Under 800 ms |
| Empty idle CPU usage | Near 0% |
| Idle memory without LSP | Under 100 MB |
| Typing latency | P95 under 16 ms |
| Run command dispatch | Under 50 ms before runtime startup |

If profiling shows these targets are impractical with the chosen NSH version, record measured baselines and optimize based on evidence.

## 12.2 Performance Strategy

- Initialize only essential UI components at startup.
- Delay LSP startup.
- Avoid background project indexing.
- Avoid automatic package scanning.
- Avoid initializing unused programming languages.
- Use incremental syntax highlighting.
- Perform filesystem and process operations asynchronously.
- Avoid full document copies on every keystroke where possible.
- Limit retained output.
- Reuse language-server processes when appropriate.

## 12.3 Benchmark Scenarios

Measure:

1. Cold startup
2. Warm startup
3. Opening an empty scratchpad
4. Opening a 1 MB source file
5. Typing into a 10,000-line document
6. First LSP completion
7. Repeated LSP completion
8. Running a short Python script
9. Repeated executions
10. Idle memory after closing documents

Benchmarks must distinguish application overhead from runtime and language-server overhead.

---

# 13. Error Handling

All subsystem failures must be recoverable wherever feasible.

Examples:

### Runtime Missing

Display an actionable message and allow runtime configuration.

### LSP Crash

Keep the editor usable and update language-server status.

### Execution Timeout

Terminate the relevant process tree and display `Timed Out`.

### Permission Denied

Display the affected path and operation without exposing secrets.

### Invalid Source Encoding

Provide an explicit encoding error or supported conversion path.

### Autosave Failure

Retain the in-memory document, show a warning, and retry safely.

### Output Overflow

Preserve a bounded portion of output and indicate truncation.

---

# 14. Security Requirements

ScratchRunner executes arbitrary local scripts by design.

Execution is a privileged user action.

## 14.1 Process Safety

- Never interpolate source text into a shell command.
- Prefer direct argument arrays.
- Use unique temporary directories.
- Clean temporary resources.
- Handle child processes and process groups.
- Limit captured output size.
- Support timeouts.
- Do not automatically run code when opening a document.

## 14.2 Trust Model

Normal execution runs with the current user's permissions.

It is **not sandboxed**.

The application must not claim untrusted code is safe to execute.

Untrusted code execution must require explicit user action.

## 14.3 LSP Security

Language servers are external executables.

- Do not silently download or install them.
- Clearly display configured executables.
- Avoid automatically executing repository-provided commands.
- Apply a clear trust policy before enabling workspace-specific executable configuration.

## 14.4 Privacy

- No telemetry by default.
- No cloud account required.
- No automatic upload of source code.
- Local-only draft persistence.
- Sensitive values in logs should be redacted.
- Diagnostic logging must be configurable.

---

# 15. Repository Structure

Actual layout — the app itself is the root crate; shared subsystems are
workspace crates:

```text
runner/
├── Cargo.toml              workspace + scratchrunner (root crate)
├── Cargo.lock
├── README.md               user-facing overview
├── SPEC.md                 this file
├── DESIGN.md               design guide
├── docs/
│   ├── development.md      build/run/test/package instructions
│   └── design/screens.md   screen-by-screen design reference
│
├── src/                    the application (scratchrunner)
│   ├── main.rs, lib.rs
│   └── workbench/          mod, chrome, state, files, execution,
│                           run_settings, console, debugger, diagnostics,
│                           lsp, find, navigator, search_sidebar,
│                           quick_open, preferences, docs, edit,
│                           compound, bindings, persistence, theme, ui,
│                           native
│
└── crates/
    ├── commands/           command table, key bindings, vim/compound commands
    ├── config/             settings.toml model, paths, schema, migrations
    ├── console-model/      ANSI/VT parsing, bounded line store, filters, links
    ├── debugger/           TOML adapters, sessions, breakpoints, variables, REPL
    ├── diagnostics/        language-driven output parsers, annotations, fixes
    ├── document/           document model, autosave, Unsaved store, session, watch
    ├── language/           34 TOML language definitions, detection
    ├── lsp/                JSON-RPC LSP client, server catalogue, workspaces
    ├── navigator/          file navigator sidebar
    ├── runner/             PTY run engine, compile stages, CR_* env, diagnostics glue
    ├── search/             find/replace engine, project search, QuickNav
    ├── theme/              .tmTheme parsing, incremental highlighting
    └── ui-kit/             design tokens, typography, icons, motion, controls
```

Packaging is `nsh package` (see docs/development.md); there is no `packaging/`
directory.

---

# 16. Implementation Status

Phases 0–4 are complete. What exists today, beyond the original MVP:

- 34 language definitions, compile stages, diagnostics engine, per-file Run
  Settings, CR_* environment contract
- LSP: 23-server catalogue, lazy per-workspace servers, virtual workspaces,
  completion, hover, diagnostics; an LSP status sheet
- Debugger: lldb / pdb / node / jdb / bashdb / perl / phpdbg via TOML adapters,
  breakpoints, stack and variables pane, console REPL
- Navigator sidebar, project search, Quick Open, Find bar, symbol menu,
  documentation panel
- Preferences window, themes, rebindable key sets (default, global, vim),
  session/window-state persistence, crash recovery
- Native packaging via `nsh package`

## Not yet implemented

- Signature help, go to definition, code actions, rename — LSP client supports
  the requests; no editor UI calls them
- Multiple windows (`File > New Window` opens a tab)
- Bun / tsx runtimes, Python virtualenv detection, `cargo run`
- Per-language interpreter/runtime selection beyond language definitions
- Installable-bundle distribution and performance budgets (unverified — see
  §18)

---

# 17. Testing Strategy

## 17.1 Unit Tests

Test:

- Language detection
- Filename generation
- Execution command construction
- Runtime detection
- Process cancellation state
- Document ID stability
- Autosave serialization
- Configuration parsing
- LSP position conversions
- LSP request routing

## 17.2 Integration Tests

Test:

- Python execution
- Shell execution
- stdout and stderr streaming
- Process timeouts
- Interpreter not found
- Unsaved document execution
- Pyright initialization
- LSP document synchronization
- Completion request cancellation
- Diagnostics mapping

## 17.3 UI Tests

Test:

- New scratchpad
- Editing
- Keyboard shortcuts
- Run / Stop controls
- Output panel resize
- Language switching
- Tab switching
- Hover popup
- Completion popup
- Session restoration
- Native IME composition

## 17.4 Performance Tests

Maintain repeatable cold-start, memory, typing, and execution benchmarks.

Collect results from Release builds, not Debug builds.

---

# 18. Acceptance Criteria

Status against the original first-version criteria:

- [x] The application launches directly into an editable scratchpad.
- [x] NSH Editor provides usable syntax highlighting.
- [x] Unsaved Python code can execute with `Cmd + R`.
- [x] Shell scripts execute correctly.
- [x] Output appears without freezing the UI.
- [x] Running processes can be cancelled.
- [x] Multiple files and tabs work.
- [x] Drafts restore after restart.
- [x] Python LSP completion is available (when Pyright is installed).
- [x] LSP diagnostics and hover work.
- [x] A missing LSP server does not prevent editing or running.
- [x] Keyboard shortcuts work consistently.
- [x] Native macOS input methods work correctly (IME composition).
- [x] The application does not automatically execute untrusted code.
- [ ] Startup and memory use are measured and reported — no benchmark suite
      exists yet.
- [ ] An installable macOS application bundle can be produced — `nsh package`
      exists; distribution signing/notarization is unverified.

---

# 19. Engineering Constraints

The implementation must follow these rules:

1. Use Rust for core application logic.
2. Use NSH for the user interface.
3. Use NSH Editor for code editing.
4. Do not reimplement a text editor.
5. Do not depend on Zed's `editor` or `project` crates.
6. Do not embed Monaco or CodeMirror.
7. Do not use Electron.
8. Do not require a local web server for the native UI.
9. Keep script execution independent of UI rendering.
10. Keep the LSP system independent of the script runner.
11. Avoid premature abstractions.
12. Pin and document dependency versions.
13. Prefer testable, narrowly scoped modules.
14. Preserve the CodeRunner-style minimal visual language (DESIGN.md).
15. Do not introduce Git or AI panels without an explicit future requirement.

---

# 20. Future Enhancements

These features remain intentionally deferred:

### P1 — Quality of Life

- Run selected code
- Execution history
- Favorite scratchpads
- Command palette (Quick Open covers files/symbols, not commands)
- Custom execution profiles
- Code formatting (LSP `textDocument/formatting` is engine-ready)
- Multiple real windows

### P2 — Advanced Editing

- Tree-sitter-based highlighting
- LSP signature help, go to definition, code actions, rename (client ready,
  UI missing)
- Workspace symbols
- Semantic tokens
- Runtime environment selector / virtualenv detection
- Bun / tsx runtimes, `cargo run`

### P3 — Additional Product Features

- Global quick-launch window
- Menu-bar mode
- Export execution results
- Share code snippets
- Cross-platform builds
- Optional plugin system

---

# 21. Development Instructions for Coding Agents

When implementing this specification:

1. Read the pinned NSH source and Editor documentation first.
2. Verify the actual NSH API before writing integration code.
3. Implement incrementally, beginning with Phase 0.
4. Never invent framework APIs.
5. Compile and run tests after completing each milestone.
6. Reuse existing NSH Editor interfaces.
7. Keep the UI responsive during asynchronous operations.
8. Implement process management with cancellation and cleanup from the beginning.
9. Separate required functionality from future enhancements.
10. Record architectural deviations and performance measurements in project documentation.

**The highest-priority success criterion is a fast, reliable, native scratchpad where a user can type and execute code without configuring a project or saving a file.**

---

# 22. Final Product Definition

ScratchRunner is:

**A native, lightweight, LSP-powered code scratchpad built with Rust and NSH for macOS.**

Its purpose is to make small coding tasks effortless.

The ideal interaction is:

**Launch → Type → Cmd + R → Result.**

Everything else is secondary.

