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

## Deviations from the specification

- **Title bar**: the window keeps the system title bar; the 52 pt toolbar with the name and
  Run/Stop is drawn below it. NSH's `TitleBarStyle::Hidden` is set on windows the app makes,
  not on the root window that `run_app` creates.
- **One run at a time** across the app, not one per document.
- **Language-server files**: unsaved documents are identified by a stable URI in
  `~/Library/Caches/ScratchRunner/workspaces/<id>/`, but nothing is written there.
- **Cargo projects** run the saved file on disk, not the editor's text.
- **Closed scratchpads** keep their draft on disk; there is no list to reopen them from yet.
- **Settings UI** shows the values in effect and reloads `settings.toml`; it does not edit them.
- **Theme**: colors follow the system appearance. The `theme` setting is read but not applied.
- **Signature help** is implemented in `scratch-lsp` and not yet shown in the UI.

## Workbench modules

The window is built in `src/workbench/`, one file per concern:

| File | Owns |
|---|---|
| `mod.rs` | The root layout, the menu and key bindings, startup and autosave, the editors |
| `chrome.rs` | The toolbar (language, run group, panel toggles), the tab strip, the status bar |
| `console.rs` | The console panel: output from `scratch-console-model`, status, Clear, Stop, program input |
| `execution.rs` | Runs: request construction, streaming into the console, stop, input |
| `run_settings.rs` | The Run Settings sheet; run settings persist on `Document.run` |
| `navigator.rs` | The left navigator: folder tree, filter, new file and folder, context menu |
| `quick_open.rs` | Open Quickly (Shift-Cmd-O): tabs, files, typed paths |
| `find.rs` | The find bar (on the NSH editor's search) and Go to Line |
| `edit.rs` | Text commands and editor display options |
| `preferences.rs` | The Preferences window; `settings()` is the live source, `save()` writes `settings.toml` |
| `lsp.rs` | Language servers: sync, completion, hover, diagnostics |
| `files.rs`, `state.rs`, `persistence.rs` | Document files, workspace state, recovery journal |

The controls come from `crates/ui-kit` (`scratch-ui-kit`), the app's component
layer. The theme is `ui/theme.json`, named by `nsh.json`. `cargo run` reads the
icons through `dev-assets/@scratch-ui-kit` (a symbolic link to
`crates/ui-kit/assets`); a packaged app gets them from the registry.
