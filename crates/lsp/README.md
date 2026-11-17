# scratch-lsp

ScratchRunner's language-intelligence layer. NSH-free, so `cargo test -p scratch-lsp` is fast and
headless. Everything is UTF-16 line/column positions, the protocol default and the shape NSH's
editor uses.

## What is here

| Module | Contents |
|--|--|
| `manager` | `Manager`, `ManagerConfig`, `ManagerEvent`, `ServerStatus`, `AppliedEdit`: lazy server start, document sync, crash restart with backoff, all requests |
| `client` | `Client`, `StartOptions`, `ClientEvent`, `RequestTicket`: one JSON-RPC server process, cancellation, server-request handling |
| `capabilities` | `ServerCapabilities` and friends, parsed from `initialize` |
| `servers` | `ServerCatalog`, `ServerDefinition`, `ServerOverride`, `find_program`, `find_project_root`: data-driven discovery |
| `workspace` | `VirtualWorkspace`, `Scaffold`: unsaved documents written under `~/Library/Caches/ScratchRunner/workspaces/<id>/`, with Cargo/go/TS scaffolding |
| `parse` (private) | protocol JSON to the typed values in `types` |
| `types` | `Position`, `Range`, `Diagnostic`, `CompletionItem`, `Hover`, `SignatureHelp`, `Definition`, `DocumentSymbol`, `CodeAction`, `WorkspaceEdit`, `FoldingRange`, ... |
| `snippet` | `expand_snippet`: LSP snippets as plain text plus UTF-16 tab stops |
| `markup` | `RichText`: markdown/plaintext to paragraphs, headings, lists, code blocks and links |
| `text` | UTF-16 conversions, line index, minimal diff, text edits, word ranges |
| `fallback` | word, keyword/snippet, path and shell-`compgen` completion that needs no server |

The server catalogue lives in [`data/servers.toml`](data/servers.toml); the fallback keyword and
snippet tables in [`data/keywords.toml`](data/keywords.toml). Both are embedded in the binary.

## Using it

```rust
use scratch_lsp::{CompletionTrigger, Manager, ManagerConfig, ManagerEvent, Position};

let (manager, mut events) = Manager::new(ManagerConfig::new(
    "/work/project",
    std::env::var("PATH").unwrap_or_default(),
    std::env::vars().collect(),
));

// A document changed (or was opened). The server starts lazily.
manager.sync("file:///work/project/main.py", "python", "import os\nos.".to_owned()).await?;

// Status and diagnostics arrive here.
while let Some(event) = events.recv().await {
    match event {
        ManagerEvent::Diagnostics { uri, diagnostics, .. } => show(uri, diagnostics),
        ManagerEvent::Status { key, status, .. } => status_bar(key, status),
        ManagerEvent::Message { message, .. } => banner(message),
    }
}

// Requests take the document's own URI; virtual-workspace URIs never leak out.
let completions = manager
    .completion(uri, Position::new(3, 7), CompletionTrigger::Invoked, None)
    .await?;
for item in completions.items {
    let text = match item.insert_format {
        scratch_lsp::InsertFormat::Snippet => scratch_lsp::expand_snippet(
            item.text_edit.as_ref().map_or(item.label.as_str(), |edit| edit.new_text.as_str()),
        ).text,
        scratch_lsp::InsertFormat::PlainText => item.insert_text.unwrap_or(item.label),
    };
    let _ = text;
}
```

Other requests: `resolve_completion`, `hover`, `signature_help`, `definition`, `declaration`,
`type_definition`, `references`, `document_symbols`, `code_actions` (+ `_with_kinds` and
`resolve_code_action`), `apply_code_action`, `format`, `format_range`, `folding_ranges`,
`prepare_rename`, `rename`, `execute_command`, `pull_diagnostics`/`diagnostics`,
`apply_workspace_edit`, `save`, `close`, `shutdown`. `trigger_characters(uri)` returns the
server's completion/signature triggers.

### Unsaved documents

`sync` writes the text to a virtual workspace when the document is not a file on disk, and also
for a single Rust/Go/TypeScript file outside a project (those servers require a package/module).
The directory is stable per document URI, contains the document and whatever scaffolding the
server needs (`Cargo.toml`, `go.mod`, `tsconfig.json`), and is removed by `close`/`shutdown`.
Diagnostics and definition/reference URIs are translated back to the app's document URIs.

One server runs per language and keeps the first project root it was started in (LSP multi-root
workspaces are not used); a document from another project still gets its file synced, but the
server's indexing root is the first one.

### Server catalogue and preferences

`ServerCatalog::builtin()` serves the 23 embedded definitions. `ServerCatalog::from_toml` loads
another one (tests, advanced users). `ManagerConfig::with_overrides` and `ServerOverride` implement
the app's preferences: `enabled`, `command`, `args`, `initialization_options`, `settings`,
`search_paths`. `availability()` checks whether a definition's executable exists without starting
anything, for the preferences pane.

Call `manager.shutdown()` before the app exits: it asks every server to quit and closes the
virtual workspaces. (Dropping the manager while its event pumps are alive would leave the server
processes to the runtime teardown.)

Discovery searches the app's `PATH` first, then `/opt/homebrew/bin`, `/usr/local/bin`,
`/opt/macports/bin`, `~/.cargo/bin`, `~/.local/bin`, `~/go/bin`, npm global bins, Volta/asdf/mise
shims, and per-server extra paths. A missing server yields
`ServerStatus::NotInstalled { hint }` with an install command; a crash yields
`ServerStatus::Stopped` and an automatic restart after 1 s, 2 s, 4 s, ... up to 30 s.

### Server-free completion

```rust
use scratch_lsp::fallback_completions;
let items = fallback_completions(text, caret, "python", document_dir);
```

`word_completions`, `LanguageTables` (34 languages), `path_completions`,
`shell_completions`/`compgen_command`/`parse_compgen_output`, and `rank_completions` are available
individually. `shell.rs` never spawns anything: it builds the `compgen` argv and parses its output,
so the app owns process execution.

## Tests

- Unit tests: framing, parsers, snippets, UTF-16/text edits, URI encoding, virtual workspace,
  catalogue, keyword tables, ranking, discovery.
- `tests/manager.rs`: drives the scripted mock server (`tests/support/mock_server.rs`) through
  lifecycle, incremental and full sync, every request shape, cancellation, crash restart, virtual
  workspaces, typed statuses, and validated workspace edits.
- `tests/pyright.rs` and `tests/gopls.rs`: real-server integration. They print `skipped: ...` and
  pass when Pyright/gopls (and Go) are not installed.

Run `cargo test -p scratch-lsp`.
