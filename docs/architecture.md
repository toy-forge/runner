# Architecture

ScratchRunner is a Rust application whose window is drawn by NSH (`nsh-platform-macos`,
AppKit). Its code is split into crates by responsibility. The app itself is the root package (`src/`), the only part that depends on NSH.

```text
crates/
├── config/        settings.toml (typed, defaulted), storage paths
├── document/      languages and detection, document ids, drafts, session
├── runner/        run plans per language, process execution, output decoding
├── lsp/           language-server client: framing, JSON-RPC, sync, requests
├── commands/      command table, key bindings, menus, compound (vim) commands
├── console-model/ console output model: ANSI, filtering, search, selection
├── debugger/      debugger adapters and sessions
├── diagnostics/   linters and run-output diagnostics
├── language/ navigator/ search/ theme/ ui-kit/   language data, file tree, project search, themes, controls
```

## Data flow

- **Edits** stay in NSH's `Editor`, which is the source of truth for the text. The app reads
  it on demand (`EditorApi::value`) rather than copying it on every keystroke.
- **Drafts**: 500 ms after an edit the text is written to `drafts/<id>.json` by a background
  task, atomically, and the previous draft is kept as `recovery/<id>.json`. On quit every
  open document is written, and the session (tabs, carets, active tab, output panel) is
  saved to `session.json`.
- **Runs**: `runner::execute` runs on the Tokio runtime and streams `ExecutionEvent`s over a
  channel. An engine-thread task applies them to the output editor in order. Each run has an
  id; a run cancels with a `watch` signal that signals the step's process group.
- **Language servers**: `scratch-lsp::Manager` starts a server the first time a document of
  its language is synced, sends `didOpen`/`didChange` with per-document versions, and
  restarts a crashed server after a cooldown. Completions, hovers and diagnostics come back
  as events and are handed to the editor (`present_completions`, `diagnostics`).

## Threads

NSH runs the UI and every `spawn`ed task on the engine thread. Work that blocks (file
I/O, process output, language-server requests) runs on the Tokio runtime through
`background` or `blocking`, and its result returns to the engine thread before the UI
changes. `state::with` is the only access to the engine-thread workspace; it must not call
back into the editor.

## State

- `state::Ui` is plain data in a signal. Views read it, and a change redraws only what reads
  the changed field.
- `state::Workspace` holds the document handles (editor refs, drafts, timers), the run in
  progress and the language-server manager.

## Notes & Known Scopes

- **Title bar**: the main window uses `TitleBarStyle::Hidden` with a custom 32 pt title band (`chrome.rs`) sharing space with traffic lights, window placement persistence, and inactive focus dimming.
- **Multi-window**: `File > New Window` (`--new-window` / `--open`), `Move Tab to New Window` (`--handoff` recovery snapshot), Preferences, `About ScratchRunner`, and `ScratchRunner Help` run as native desktop windows.
- **Language-server files**: unsaved documents use a stable virtual workspace URI so LSP requests (`hover`, `definition`, `signatureHelp`, `codeAction`, `rename`, `formatting`) operate without forcing a save first.
- **Preferences & Themes**: all Settings panes (`General`, `Text Editing`, `Shell`, `Languages`, `Keyboard`, `Themes`, `Advanced`) persist live changes to `settings.toml` / language TOMLs / `.crBindings` / `.tmTheme`, and `theme::install()` synchronizes NSH's design tokens and editor palette across light and dark appearances.

## Workbench modules

The window is built in `src/workbench/`, one file per concern:

| File | Owns |
|---|---|
| `mod.rs` | The root layout, menu and key bindings, CLI handoff/window args, startup and autosave, editors |
| `chrome.rs` | The toolbar (language, 3-button run capsule, panel toggles), tab strip, status bar, CPU/RSS pill |
| `console.rs` | Console panel: ANSI/diagnostics output (`scratch-console-model`), `Jump to Bottom`, Up/Down input history, Web/Markdown `WKWebView` preview + JS Inspector |
| `debugger.rs` | Debugger sessions (`lldb`/`gdb`/`xdebug`/`pdb`/`node`/`jdb`/`bashdb`/`perl`/`phpdbg`), split `Stack \| Variables` panes, sort/type/hover/auto-hide options, breakpoint sheets |
| `diagnostics.rs` | Manual/on-run/on-edit linter jobs, output parsers, ignored-message sheets |
| `docs.rs` | Embedded `WKWebView` documentation browser, global/per-language sources, TOC footer, reader CSS, bookmarks, offline fallback |
| `execution.rs` | Runs: `CompileOnly`/`Run`/`RunSelection` planning, streaming into console, live stats, stop, >128 KiB output alert |
| `run_settings.rs` | The Run Settings sheet (`Arguments`, `Input`, `Compile Flags`, `Make Default`, Help); persists on `Document.run` |
| `navigator.rs` | The left navigator wrapper: folder tree, `Go to Folder…`, new file from template, context menu, Search switch |
| `search_sidebar.rs` | Project Search & Replace sidebar: regex/whole-word/case modes, custom path sets, `ReplacePlan` confirmation, `ReplaceJournal` undo |
| `quick_open.rs` | Open Quickly (`⇧⌘O`), Command/Language selection palettes |
| `find.rs` | Find & Replace bar (`Text`, `Exact Word`, `Word Prefix`, `Word Suffix`, `Ignoring Whitespace`, `Regex`, regex helpers, history, `All In Selection`), Go to Line |
| `edit.rs` | Text commands, Emacs kill-ring (`⌃K`/`⌃Y`), Custom surround sheet, Color Scopes inspector, editor display options |
| `preferences.rs` | Preferences window: General/Text Editing/Shell/Languages/Keyboard/Themes/Advanced panes, interactive `KeyRecorder`, Shell Auto-PATH preview & refresh, per-pane sizing |
| `help.rs` | Native `About ScratchRunner` dialog and searchable multi-topic `ScratchRunner Help` (`⌘?`) window |
| `compound.rs`, `bindings.rs` | `.crBindings` resolution, command dispatch, `Machine<EditorHost>` multi-stroke compound/Vim execution with read-only modal locks |
| `lsp.rs` | Language servers: sync, completion, hover, go-to-definition, signature help, code actions, rename, format document, status sheet |
| `theme.rs`, `native.rs`, `ui.rs` | TextMate syntax decorations, token sync, syntax-colored HTML printing, AppKit `KeyDown` recorder monitor, WebKit/Pasteboard/Speech/Font bridges |
| `files.rs`, `state.rs`, `persistence.rs` | Document files, first-launch welcome script & numbered untitled tabs, tab-insertion rules, clean reload, window placement, recovery journal |

The controls come from `crates/ui-kit` (`scratch-ui-kit`), the app's component
layer. The theme is `ui/theme.json`, named by `nsh.json`. `cargo run` reads the
icons through `dev-assets/@scratch-ui-kit` (a symbolic link to
`crates/ui-kit/assets`); a packaged app gets them from the registry.
