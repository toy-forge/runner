//! A scripted language server for integration tests. It speaks the protocol over stdio and
//! deliberately covers the shapes the client must handle: incremental sync, push and pull
//! diagnostics, snippets, deferred code actions, slow requests with cancellation, and a crash
//! trigger. It is a test binary, not part of the shipped app.
//!
//! Run with `--full` to advertise full document sync instead of incremental.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

struct State {
    out: io::Stdout,
    text: String,
    uri: String,
    version: i64,
    opens: u64,
    closes: u64,
    saves: u64,
    apply_edit_sent: bool,
    apply_results: Vec<Value>,
    initialize: Value,
    settings: Value,
    requests: Vec<String>,
    notifications: Vec<String>,
    cancelled: Vec<u64>,
    full_sync: bool,
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.iter().any(|argument| argument == "--fail-init") {
        // Simulates a server that starts but dies before answering `initialize`.
        std::process::exit(3);
    }
    let full_sync = arguments.iter().any(|argument| argument == "--full");
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let state: Shared = Arc::new(Mutex::new(State {
        out: io::stdout(),
        text: String::new(),
        uri: String::new(),
        version: 0,
        opens: 0,
        closes: 0,
        saves: 0,
        apply_edit_sent: false,
        apply_results: Vec::new(),
        initialize: Value::Null,
        settings: Value::Null,
        requests: Vec::new(),
        notifications: Vec::new(),
        cancelled: Vec::new(),
        full_sync,
    }));
    while let Ok(Some(message)) = read_message(&mut reader) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.get("id").cloned();
        match (method, id) {
            (Some(method), Some(id)) => {
                lock(&state).requests.push(method.clone());
                handle_request(&state, &method, &id, &message);
            }
            (Some(method), None) => handle_notification(&state, &method, &message),
            (None, Some(_)) => {
                lock(&state)
                    .apply_results
                    .push(message.get("result").cloned().unwrap_or(Value::Null));
            }
            _ => {}
        }
    }
}

fn handle_request(state: &Shared, method: &str, id: &Value, message: &Value) {
    match method {
        "initialize" => {
            lock(state).initialize = message.get("params").cloned().unwrap_or(Value::Null);
            let sync = if lock(state).full_sync {
                json!(1)
            } else {
                json!({"openClose": true, "change": 2, "save": {"includeText": true}})
            };
            respond(
                state,
                id,
                json!({
                    "capabilities": {
                        "positionEncoding": "utf-16",
                        "textDocumentSync": sync,
                        "completionProvider": {
                            "triggerCharacters": [".", "("],
                            "resolveProvider": true,
                            "allCommitCharacters": ["."]
                        },
                        "hoverProvider": true,
                        "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                        "definitionProvider": true,
                        "declarationProvider": true,
                        "typeDefinitionProvider": true,
                        "referencesProvider": true,
                        "documentSymbolProvider": true,
                        "codeActionProvider": {
                            "resolveProvider": true,
                            "codeActionKinds": ["quickfix", "refactor"]
                        },
                        "documentFormattingProvider": true,
                        "documentRangeFormattingProvider": true,
                        "foldingRangeProvider": true,
                        "renameProvider": {"prepareProvider": true},
                        "diagnosticProvider": {
                            "interFileDependencies": false,
                            "workspaceDiagnostics": false
                        },
                        "executeCommandProvider": {"commands": ["scratch.fix"]},
                        "workspace": {
                            "workspaceEdit": {
                                "documentChanges": true,
                                "resourceOperations": ["create", "rename", "delete"]
                            }
                        }
                    },
                    "serverInfo": {"name": "ScratchRunner Mock", "version": "1.2.3"}
                }),
            );
        }
        "shutdown" => respond(state, id, Value::Null),
        "textDocument/completion" => {
            let (text, version, saves, uri) = {
                let state = lock(state);
                (
                    state.text.clone(),
                    state.version,
                    state.saves,
                    state.uri.clone(),
                )
            };
            let result = completion_result(&text, version, saves, &uri);
            if text.contains("SLOW") {
                let state = state.clone();
                let id = id.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(400));
                    let cancelled = lock(&state).cancelled.contains(&id.as_u64().unwrap_or(0));
                    if cancelled {
                        send_error(&state, &id, -32800, "cancelled");
                    } else {
                        respond(&state, &id, result);
                    }
                });
            } else {
                respond(state, id, result);
            }
        }
        "completionItem/resolve" => {
            let label = message
                .pointer("/params/label")
                .and_then(Value::as_str)
                .unwrap_or("item")
                .to_owned();
            respond(
                state,
                id,
                json!({
                    "label": label,
                    "kind": 3,
                    "detail": "resolved detail",
                    "documentation": {"kind": "markdown", "value": "Resolved **documentation**."},
                    "insertText": "resolved",
                    "data": {"resolved": true}
                }),
            );
        }
        "textDocument/hover" => respond(
            state,
            id,
            json!({
                "contents": {
                    "kind": "markdown",
                    "value": "```python\ndef print(value)\n```\n\nPrints [value](https://example.com/print)."
                },
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 3}
                }
            }),
        ),
        "textDocument/signatureHelp" => respond(
            state,
            id,
            json!({
                "signatures": [{
                    "label": "print(value, sep=' ', end='\\n')",
                    "documentation": {"kind": "markdown", "value": "**prints**"},
                    "parameters": [
                        {"label": [6, 11]},
                        {"label": "sep=' '"},
                        {"label": "end='\\n'"}
                    ]
                }],
                "activeSignature": 0,
                "activeParameter": 1
            }),
        ),
        "textDocument/definition" | "textDocument/declaration" | "textDocument/typeDefinition" => {
            let uri = current_uri(state);
            respond(
                state,
                id,
                json!([
                    {
                        "targetUri": uri,
                        "targetRange": {
                            "start": {"line": 1, "character": 0},
                            "end": {"line": 1, "character": 5}
                        },
                        "targetSelectionRange": {
                            "start": {"line": 1, "character": 1},
                            "end": {"line": 1, "character": 4}
                        },
                        "originSelectionRange": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 2}
                        }
                    },
                    {
                        "targetUri": format!("{uri}.other"),
                        "targetRange": {
                            "start": {"line": 3, "character": 0},
                            "end": {"line": 3, "character": 4}
                        },
                        "targetSelectionRange": {
                            "start": {"line": 3, "character": 0},
                            "end": {"line": 3, "character": 4}
                        }
                    }
                ]),
            );
        }
        "textDocument/references" => {
            let uri = current_uri(state);
            let range = json!({
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 2}
            });
            respond(
                state,
                id,
                json!([
                    {"uri": uri, "range": range},
                    {"uri": format!("{uri}.other"), "range": range}
                ]),
            );
        }
        "textDocument/documentSymbol" => respond(
            state,
            id,
            json!([{
                "name": "main",
                "detail": "() -> None",
                "kind": 12,
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 3, "character": 0}
                },
                "selectionRange": {
                    "start": {"line": 0, "character": 4},
                    "end": {"line": 0, "character": 8}
                },
                "children": [{
                    "name": "value",
                    "kind": 13,
                    "range": {
                        "start": {"line": 1, "character": 0},
                        "end": {"line": 1, "character": 3}
                    },
                    "selectionRange": {
                        "start": {"line": 1, "character": 0},
                        "end": {"line": 1, "character": 3}
                    }
                }]
            }]),
        ),
        "textDocument/codeAction" => respond(
            state,
            id,
            json!([
                {
                    "title": "Fix it",
                    "kind": "quickfix",
                    "isPreferred": true,
                    "diagnostics": message.pointer("/params/context/diagnostics").cloned().unwrap_or(json!([])),
                    "data": {"fix": true}
                },
                {
                    "title": "Organize imports",
                    "kind": "source.organizeImports",
                    "command": {"title": "Organize", "command": "scratch.fix"}
                }
            ]),
        ),
        "codeAction/resolve" => {
            let uri = current_uri(state);
            let version = lock(state).version;
            respond(
                state,
                id,
                json!({
                    "title": message.pointer("/params/title").and_then(Value::as_str).unwrap_or("Fix it"),
                    "kind": "quickfix",
                    "isPreferred": true,
                    "edit": {
                        "documentChanges": [{
                            "textDocument": {"uri": uri, "version": version},
                            "edits": [{
                                "range": {
                                    "start": {"line": 0, "character": 0},
                                    "end": {"line": 0, "character": 0}
                                },
                                "newText": "# fixed\n"
                            }]
                        }]
                    }
                }),
            );
        }
        "textDocument/formatting" | "textDocument/rangeFormatting" => respond(
            state,
            id,
            json!([{
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 0}
                },
                "newText": "# formatted\n"
            }]),
        ),
        "textDocument/foldingRange" => respond(
            state,
            id,
            json!([{
                "startLine": 0,
                "endLine": 2,
                "kind": "comment",
                "collapsedText": "…"
            }]),
        ),
        "textDocument/prepareRename" => respond(
            state,
            id,
            json!({
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 3}
                },
                "placeholder": "old_name"
            }),
        ),
        "textDocument/rename" => {
            let uri = current_uri(state);
            let new_name = message
                .pointer("/params/newName")
                .and_then(Value::as_str)
                .unwrap_or("new_name");
            let mut changes = serde_json::Map::new();
            changes.insert(
                uri,
                json!([{
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 3}
                    },
                    "newText": new_name
                }]),
            );
            respond(state, id, json!({"changes": changes}));
        }
        "textDocument/diagnostic" => {
            let previous = message
                .pointer("/params/previousResultId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let (text, version) = {
                let state = lock(state);
                (state.text.clone(), state.version)
            };
            let result_id = version.to_string();
            if previous.as_deref() == Some(result_id.as_str()) {
                respond(
                    state,
                    id,
                    json!({"kind": "unchanged", "resultId": result_id}),
                );
            } else {
                respond(
                    state,
                    id,
                    json!({
                        "kind": "full",
                        "resultId": result_id,
                        "items": diagnostics_for(&text)
                    }),
                );
            }
        }
        "workspace/executeCommand" => {
            let uri = current_uri(state);
            let mut changes = serde_json::Map::new();
            changes.insert(
                uri,
                json!([{
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0}
                    },
                    "newText": "# command\n"
                }]),
            );
            respond(state, id, json!({"changes": changes}));
        }
        "scratch/state" => {
            let result = {
                let state = lock(state);
                json!({
                    "text": state.text,
                    "uri": state.uri,
                    "version": state.version,
                    "opens": state.opens,
                    "closes": state.closes,
                    "saves": state.saves,
                    "requests": state.requests,
                    "notifications": state.notifications,
                    "cancelled": state.cancelled,
                    "apply_results": state.apply_results,
                    "initialize": state.initialize,
                    "settings": state.settings
                })
            };
            respond(state, id, result);
        }
        "scratch/slow" => {
            let state = state.clone();
            let id = id.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                let cancelled = lock(&state).cancelled.contains(&id.as_u64().unwrap_or(0));
                if cancelled {
                    send_error(&state, &id, -32800, "cancelled");
                } else {
                    respond(&state, &id, json!({"ok": true}));
                }
            });
        }
        other => send_error(state, id, -32601, &format!("unknown method {other}")),
    }
}

fn current_uri(state: &Shared) -> String {
    lock(state).uri.clone()
}

fn handle_notification(state: &Shared, method: &str, message: &Value) {
    match method {
        "initialized" => log(state, "mock server initialized"),
        "textDocument/didOpen" => {
            let (uri, version, text) = {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                (
                    params
                        .pointer("/textDocument/uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    params
                        .pointer("/textDocument/version")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    params
                        .pointer("/textDocument/text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )
            };
            {
                let mut state = lock(state);
                state.uri = uri.clone();
                state.text = text;
                state.version = version;
                state.opens += 1;
                state.notifications.push(method.to_owned());
            }
            publish_diagnostics(state, &uri);
            let wants_apply = lock(state).text.contains("APPLYEDIT");
            if wants_apply {
                request_apply_edit(state, &uri);
            }
        }
        "textDocument/didChange" => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let (uri, version) = {
                (
                    params
                        .pointer("/textDocument/uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    params
                        .pointer("/textDocument/version")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                )
            };
            let changes = params
                .get("contentChanges")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let crash = {
                let mut state = lock(state);
                for change in &changes {
                    if let Some(range) = change.get("range") {
                        let range: scratch_lsp::Range = serde_json::from_value(range.clone())
                            .unwrap_or(scratch_lsp::Range::new(0, 0, 0, 0));
                        let edit = scratch_lsp::TextEdit {
                            range,
                            new_text: change
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        };
                        state.text = scratch_lsp::text::apply_edits(&state.text, &[edit])
                            .unwrap_or_else(|_| state.text.clone());
                    } else if let Some(text) = change.get("text").and_then(Value::as_str) {
                        state.text = text.to_owned();
                    }
                }
                state.version = version;
                state.uri = uri.clone();
                state.notifications.push(method.to_owned());
                state.text.contains("CRASH")
            };
            if crash {
                std::process::exit(9);
            }
            publish_diagnostics(state, &uri);
            let wants_apply = lock(state).text.contains("APPLYEDIT");
            if wants_apply {
                request_apply_edit(state, &uri);
            }
        }
        "workspace/didChangeConfiguration" => {
            let mut state = lock(state);
            state.settings = message
                .pointer("/params/settings")
                .cloned()
                .unwrap_or(Value::Null);
            state.notifications.push(method.to_owned());
        }
        "textDocument/didSave" => {
            let mut state = lock(state);
            state.saves += 1;
            state.notifications.push(method.to_owned());
        }
        "textDocument/didClose" => {
            let mut state = lock(state);
            state.closes += 1;
            state.notifications.push(method.to_owned());
        }
        "$/cancelRequest" => {
            let id = message
                .pointer("/params/id")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            lock(state).cancelled.push(id);
        }
        "exit" => std::process::exit(0),
        other => lock(state).notifications.push(other.to_owned()),
    }
}

/// Asks the editor to apply an edit, exercising the `workspace/applyEdit` path. Only once per
/// server process, so the didChange that comes back does not loop.
fn request_apply_edit(state: &Shared, uri: &str) {
    {
        let mut state = lock(state);
        if state.apply_edit_sent {
            return;
        }
        state.apply_edit_sent = true;
    }
    let mut changes = serde_json::Map::new();
    changes.insert(
        uri.to_owned(),
        json!([{
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 0}
            },
            "newText": "# applied\n"
        }]),
    );
    send(
        state,
        json!({
            "jsonrpc": "2.0",
            "id": 1000,
            "method": "workspace/applyEdit",
            "params": {"edit": {"changes": changes}}
        }),
    );
}

fn completion_result(text: &str, version: i64, saves: u64, uri: &str) -> Value {
    json!({
        "isIncomplete": false,
        "items": [
            {
                "label": "print",
                "labelDetails": {"detail": "(value: Any)", "description": "builtins"},
                "kind": 3,
                "detail": format!("text={text:?};version={version};saves={saves};uri={uri}"),
                "documentation": {
                    "kind": "markdown",
                    "value": "Prints **value**.\n\n```python\nprint(value)\n```"
                },
                "insertText": "print(${1:value})",
                "insertTextFormat": 2,
                "sortText": "0001",
                "filterText": "print",
                "preselect": true,
                "commitCharacters": ["("],
                "textEdit": {
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0}
                    },
                    "newText": "print(${1:value})"
                },
                "additionalTextEdits": [{
                    "range": {
                        "start": {"line": 1, "character": 0},
                        "end": {"line": 1, "character": 0}
                    },
                    "newText": "# imported\n"
                }],
                "command": {"title": "After", "command": "scratch.after", "arguments": [1]},
                "data": {"resolved": false}
            },
            {
                "label": "os.path",
                "kind": 9,
                "detail": "module",
                "documentation": {"kind": "markdown", "value": "Paths."}
            }
        ]
    })
}

fn diagnostics_for(text: &str) -> Value {
    if let Some(offset) = text.find("bad") {
        let start = scratch_lsp::text::position_at(text, offset);
        let end = scratch_lsp::text::position_at(text, offset + 3);
        json!([{
            "range": {
                "start": {"line": start.line, "character": start.character},
                "end": {"line": end.line, "character": end.character}
            },
            "severity": 1,
            "message": "bad value",
            "source": "mock",
            "code": "M001",
            "codeDescription": {"href": "https://example.com/M001"},
            "tags": [1],
            "relatedInformation": []
        }])
    } else {
        json!([])
    }
}

fn publish_diagnostics(state: &Shared, uri: &str) {
    let (text, version) = {
        let state = lock(state);
        (state.text.clone(), state.version)
    };
    let diagnostics = diagnostics_for(&text);
    send(
        state,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": uri, "version": version, "diagnostics": diagnostics}
        }),
    );
}

fn log(state: &Shared, message: &str) {
    send(
        state,
        json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": {"type": 3, "message": message}
        }),
    );
}

fn respond(state: &Shared, id: &Value, result: Value) {
    send(state, json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn send_error(state: &Shared, id: &Value, code: i64, message: &str) {
    send(
        state,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message}
        }),
    );
}

fn send(state: &Shared, message: Value) {
    let mut state = lock(state);
    write_message(&mut state.out, &message);
}

fn write_message(out: &mut io::Stdout, message: &Value) {
    let body = match serde_json::to_vec(message) {
        Ok(body) => body,
        Err(_) => return,
    };
    if write!(out, "Content-Length: {}\r\n\r\n", body.len()).is_err() {
        return;
    }
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn read_message(reader: &mut BufReader<impl BufRead>) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length",
        ));
    };
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
