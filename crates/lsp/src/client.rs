//! One language-server process and its JSON-RPC connection.
//!
//! The client owns the process: dropping it closes the server's stdin and kills it. It answers
//! the requests a server makes of an editor (configuration, workspace folders, and
//! `workspace/applyEdit` via [`ClientEvent::ApplyEdit`]), routes publish diagnostics as events,
//! and lets callers cancel requests with `$/cancelRequest` when the document changes.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::capabilities::ServerCapabilities;
use crate::error::{LspError, Result};
use crate::framing::{read_message, write_message};
use crate::parse::{parse_diagnostics, parse_workspace_edit};
use crate::types::{Diagnostic, WorkspaceEdit};

/// How long a normal request may take before it fails.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `initialize` may take: servers index the workspace first.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// How many stderr lines are kept for a crash message.
const STDERR_LINES: usize = 40;

/// The program to start, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    /// The executable, an absolute path.
    pub program: PathBuf,
    /// Its arguments, such as `--stdio`.
    pub args: Vec<String>,
    /// The directory it starts in.
    pub cwd: PathBuf,
    /// Its environment.
    pub env: Vec<(String, String)>,
}

/// What to pass to `initialize`.
#[derive(Debug, Clone, PartialEq)]
pub struct StartOptions {
    /// How to start the server.
    pub command: ServerCommand,
    /// The workspace root's `file://` URI.
    pub root_uri: String,
    /// The workspace folder's name.
    pub workspace_name: String,
    /// The `initializationOptions` object.
    pub initialization_options: Value,
    /// Settings answered to `workspace/configuration` and sent with
    /// `workspace/didChangeConfiguration`.
    pub settings: Value,
}

impl StartOptions {
    /// Options for `command` rooted at `root_uri`, with no settings yet.
    pub fn new(command: ServerCommand, root_uri: impl Into<String>) -> Self {
        Self {
            command,
            root_uri: root_uri.into(),
            workspace_name: "scratch".to_owned(),
            initialization_options: Value::Null,
            settings: Value::Null,
        }
    }
}

/// A server's name and version, as it reports them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerInfo {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: Option<String>,
}

/// The severity of a message a server asked the editor to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    /// `window/showMessage` type 1.
    Error,
    /// Type 2.
    Warning,
    /// Type 3.
    Info,
    /// `window/logMessage` or type 4.
    Log,
}

/// What a client reports besides answers to its requests.
#[derive(Debug)]
pub enum ClientEvent {
    /// A server's diagnostics for a document. An empty list clears them.
    Diagnostics {
        /// The document.
        uri: String,
        /// The version the diagnostics were computed for, when the server says.
        version: Option<i32>,
        /// The problems.
        diagnostics: Vec<Diagnostic>,
    },
    /// A message the server wants shown.
    Message {
        /// How serious it is.
        level: MessageLevel,
        /// The text.
        message: String,
    },
    /// The server asks the editor to apply an edit (`workspace/applyEdit`). The client replies
    /// once `respond` is answered with `Ok`, or with the failure reason.
    ApplyEdit {
        /// What to apply.
        edit: WorkspaceEdit,
        /// The answer to send back.
        respond: oneshot::Sender<std::result::Result<(), String>>,
    },
    /// The server exited or its connection broke.
    Stopped {
        /// Why, for the status bar.
        reason: String,
    },
}

struct PendingRequest {
    method: String,
    sender: oneshot::Sender<Result<Value>>,
}

type Pending = Arc<Mutex<HashMap<u64, PendingRequest>>>;
type StderrTail = Arc<Mutex<VecDeque<String>>>;

/// A request that was sent and can be cancelled.
pub struct RequestTicket {
    id: u64,
    receiver: oneshot::Receiver<Result<Value>>,
}

impl RequestTicket {
    /// The JSON-RPC id, for [`Client::cancel`].
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Waits for the answer.
    pub async fn receive(self) -> Result<Value> {
        self.receiver.await.map_err(|_| LspError::NotRunning)?
    }
}

/// A connection to one language server, initialized and ready for requests.
pub struct Client {
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Pending,
    next_id: AtomicU64,
    capabilities: ServerCapabilities,
    info: ServerInfo,
}

impl Client {
    /// Starts `options.command`, initializes it for `options.root_uri`, and returns the
    /// connection. Later events are reported on `events`.
    pub async fn start(
        options: &StartOptions,
        events: mpsc::UnboundedSender<ClientEvent>,
    ) -> Result<Self> {
        let mut child = Command::new(&options.command.program)
            .args(&options.command.args)
            .current_dir(&options.command.cwd)
            .env_clear()
            .envs(options.command.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                LspError::Start(format!(
                    "{} could not start: {error}",
                    options.command.program.display()
                ))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::Start("the server has no stdin".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::Start("the server has no stdout".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| LspError::Start("the server has no stderr".to_owned()))?;

        let (outgoing, messages) = mpsc::unbounded_channel::<Value>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let stderr_tail: StderrTail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_for_start = stderr_tail.clone();

        tokio::spawn(drain_stderr(stderr, stderr_tail.clone()));
        tokio::spawn(write_loop(messages, stdin, child));
        tokio::spawn(read_loop(
            BufReader::new(stdout),
            outgoing.clone(),
            pending.clone(),
            events,
            ReadContext {
                root_uri: options.root_uri.clone(),
                workspace_name: options.workspace_name.clone(),
                settings: options.settings.clone(),
            },
            stderr_tail,
        ));

        let client = Self {
            outgoing,
            pending,
            next_id: AtomicU64::new(1),
            capabilities: ServerCapabilities::default(),
            info: ServerInfo::default(),
        };
        let initialized = match client
            .request_with_timeout("initialize", initialize_params(options), INITIALIZE_TIMEOUT)
            .await
        {
            Ok(initialized) => initialized,
            Err(error) => {
                let tail = lock(&stderr_for_start)
                    .iter()
                    .rev()
                    .take(5)
                    .rev()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" | ");
                let tail: String = tail.chars().take(600).collect();
                return Err(LspError::Start(if tail.is_empty() {
                    format!(
                        "{} did not initialize: {error}",
                        options.command.program.display()
                    )
                } else {
                    format!(
                        "{} did not initialize: {error} ({tail})",
                        options.command.program.display()
                    )
                }));
            }
        };
        let capabilities = initialized
            .get("capabilities")
            .cloned()
            .unwrap_or(Value::Null);
        let encoding = capabilities
            .get("positionEncoding")
            .and_then(Value::as_str)
            .unwrap_or("utf-16");
        if encoding != "utf-16" {
            return Err(LspError::Start(format!(
                "the server chose {encoding} positions; ScratchRunner needs utf-16"
            )));
        }
        let info = initialized
            .get("serverInfo")
            .map(|info| ServerInfo {
                name: info
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                version: info
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
            .unwrap_or_default();
        client.notify("initialized", json!({}))?;
        if !options.settings.is_null() {
            client.notify(
                "workspace/didChangeConfiguration",
                json!({"settings": options.settings}),
            )?;
        }
        Ok(Self {
            capabilities: ServerCapabilities::from_json(&capabilities),
            info,
            ..client
        })
    }

    /// What the server said it supports.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// The server's name and version.
    pub fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    /// Sends a request and waits for its answer, with [`REQUEST_TIMEOUT`].
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value> {
        let ticket = self.request_async(method, params)?;
        let id = ticket.id();
        match tokio::time::timeout(limit, ticket.receive()).await {
            Ok(answer) => answer,
            Err(_) => {
                self.cancel(id);
                Err(LspError::Timeout {
                    method: method.to_owned(),
                })
            }
        }
    }

    /// Sends a request without waiting; correlate with [`RequestTicket`] and cancel with
    /// [`cancel`](Self::cancel).
    pub fn request_async(&self, method: &str, params: Value) -> Result<RequestTicket> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(
            id,
            PendingRequest {
                method: method.to_owned(),
                sender,
            },
        );
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.outgoing.send(message).is_err() {
            lock(&self.pending).remove(&id);
            return Err(LspError::NotRunning);
        }
        Ok(RequestTicket { id, receiver })
    }

    /// Cancels a request: the caller's wait fails with [`LspError::Cancelled`] immediately and
    /// the server is told with `$/cancelRequest`.
    pub fn cancel(&self, id: u64) {
        let removed = lock(&self.pending).remove(&id);
        if let Some(request) = removed {
            let _ = request.sender.send(Err(LspError::Cancelled));
            let _ = self.notify("$/cancelRequest", json!({"id": id}));
        }
    }

    /// Sends a notification. Nothing comes back.
    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.outgoing
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .map_err(|_| LspError::NotRunning)
    }

    /// Asks the server to shut down and exit.
    pub async fn shutdown(&self) {
        let _ = self
            .request_with_timeout("shutdown", Value::Null, Duration::from_secs(2))
            .await;
        let _ = self.notify("exit", Value::Null);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn initialize_params(options: &StartOptions) -> Value {
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "ScratchRunner", "version": env!("CARGO_PKG_VERSION")},
        "locale": "en",
        "rootUri": options.root_uri,
        "workspaceFolders": [{"uri": options.root_uri, "name": options.workspace_name}],
        "initializationOptions": options.initialization_options,
        "capabilities": {
            "general": {
                "positionEncodings": ["utf-16"],
                "staleRequestSupport": {"cancel": true, "retryOnContentModified": []}
            },
            "window": {
                "workDoneProgress": true,
                "showMessage": {"messageActionItem": {"additionalPropertiesSupport": false}},
                "showDocument": {"support": false}
            },
            "workspace": {
                "workspaceFolders": true,
                "configuration": true,
                "applyEdit": true,
                "workspaceEdit": {
                    "documentChanges": true,
                    "resourceOperations": ["create", "rename", "delete"],
                    "failureHandling": "abort"
                },
                "didChangeConfiguration": {"dynamicRegistration": false}
            },
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "willSave": false,
                    "willSaveWaitUntil": false,
                    "didSave": true
                },
                "completion": {
                    "dynamicRegistration": false,
                    "contextSupport": true,
                    "completionItem": {
                        "snippetSupport": true,
                        "commitCharactersSupport": true,
                        "documentationFormat": ["markdown", "plaintext"],
                        "labelDetailsSupport": true,
                        "insertReplaceSupport": true,
                        "resolveSupport": {"properties": ["documentation", "detail", "additionalTextEdits"]}
                    }
                },
                "hover": {"dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"]},
                "signatureHelp": {
                    "dynamicRegistration": false,
                    "contextSupport": true,
                    "signatureInformation": {
                        "documentationFormat": ["markdown", "plaintext"],
                        "parameterInformation": {"labelOffsetSupport": true},
                        "activeParameterSupport": true
                    }
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "versionSupport": true,
                    "codeDescriptionSupport": true,
                    "dataSupport": true,
                    "tagSupport": {"valueSet": [1, 2]}
                },
                "diagnostic": {"dynamicRegistration": false, "relatedDocumentSupport": false},
                "definition": {"dynamicRegistration": false, "linkSupport": true},
                "declaration": {"dynamicRegistration": false, "linkSupport": true},
                "typeDefinition": {"dynamicRegistration": false, "linkSupport": true},
                "references": {"dynamicRegistration": false},
                "documentSymbol": {
                    "dynamicRegistration": false,
                    "hierarchicalDocumentSymbolSupport": true,
                    "symbolKind": {"valueSet": (1..=26).collect::<Vec<u32>>()}
                },
                "codeAction": {
                    "dynamicRegistration": false,
                    "codeActionLiteralSupport": {
                        "codeActionKind": {
                            "valueSet": [
                                "quickfix", "refactor", "refactor.extract", "refactor.inline",
                                "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"
                            ]
                        }
                    },
                    "isPreferredSupport": true,
                    "disabledSupport": true,
                    "dataSupport": true,
                    "resolveSupport": {"properties": ["edit"]}
                },
                "formatting": {"dynamicRegistration": false},
                "rangeFormatting": {"dynamicRegistration": false},
                "rename": {"dynamicRegistration": false, "prepareSupport": true},
                "foldingRange": {"dynamicRegistration": false, "lineFoldingOnly": true}
            }
        }
    })
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, tail: StderrTail) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "scratch_lsp", "language server stderr: {line}");
        let mut tail = lock(&tail);
        if tail.len() == STDERR_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

async fn write_loop(
    mut messages: mpsc::UnboundedReceiver<Value>,
    mut stdin: tokio::process::ChildStdin,
    mut child: tokio::process::Child,
) {
    loop {
        tokio::select! {
            message = messages.recv() => match message {
                Some(message) => {
                    if write_message(&mut stdin, &message).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            status = child.wait() => {
                let _ = status;
                break;
            }
        }
    }
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;
}

struct ReadContext {
    root_uri: String,
    workspace_name: String,
    settings: Value,
}

async fn read_loop(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Pending,
    events: mpsc::UnboundedSender<ClientEvent>,
    context: ReadContext,
    stderr_tail: StderrTail,
) {
    let reason = loop {
        match read_message(&mut stdout).await {
            Ok(Some(message)) => dispatch(message, &outgoing, &pending, &events, &context),
            Ok(None) => break "the server closed its output".to_owned(),
            Err(error) => break format!("the connection broke: {error}"),
        }
    };
    let waiting: Vec<PendingRequest> = lock(&pending).drain().map(|(_, request)| request).collect();
    for request in waiting {
        tracing::debug!(method = %request.method, "language server stopped with a pending request");
        let _ = request.sender.send(Err(LspError::NotRunning));
    }
    let stderr: Vec<String> = lock(&stderr_tail).iter().cloned().collect();
    let reason = if stderr.is_empty() {
        reason
    } else {
        let tail = stderr
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
        if tail.is_empty() {
            reason
        } else {
            let tail: String = tail.chars().take(600).collect();
            format!("{reason} ({tail})")
        }
    };
    let _ = events.send(ClientEvent::Stopped { reason });
}

fn dispatch(
    message: Value,
    outgoing: &mpsc::UnboundedSender<Value>,
    pending: &Pending,
    events: &mpsc::UnboundedSender<ClientEvent>,
    context: &ReadContext,
) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            handle_server_request(&method, id, &message, outgoing, events, context);
        }
        (Some(method), None) => handle_notification(&method, &message, events),
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(request) = lock(pending).remove(&id) else {
                return;
            };
            let result = match message.get("error") {
                Some(error) => {
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the request failed")
                        .to_owned();
                    Err(LspError::from_rpc(code, message))
                }
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = request.sender.send(result);
        }
        (None, None) => {}
    }
}

fn handle_server_request(
    method: &str,
    id: Value,
    message: &Value,
    outgoing: &mpsc::UnboundedSender<Value>,
    events: &mpsc::UnboundedSender<ClientEvent>,
    context: &ReadContext,
) {
    let result = match method {
        "workspace/configuration" => {
            let items = message
                .pointer("/params/items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Value::Array(
                items
                    .iter()
                    .map(|item| {
                        configuration_section(
                            &context.settings,
                            item.get("section").and_then(Value::as_str),
                        )
                    })
                    .collect(),
            )
        }
        "workspace/workspaceFolders" => json!([{
            "uri": context.root_uri,
            "name": context.workspace_name
        }]),
        "workspace/applyEdit" => {
            let edit = message
                .pointer("/params/edit")
                .ok_or_else(|| LspError::Protocol("workspace/applyEdit has no edit".to_owned()))
                .and_then(parse_workspace_edit);
            match edit {
                Ok(edit) => {
                    let outgoing = outgoing.clone();
                    let id = id.clone();
                    let (respond, answer) = oneshot::channel();
                    if events
                        .send(ClientEvent::ApplyEdit { edit, respond })
                        .is_err()
                    {
                        let _ = outgoing.send(json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"applied": false, "failureReason": "the editor is shutting down"}
                        }));
                    } else {
                        tokio::spawn(async move {
                            let result = match tokio::time::timeout(Duration::from_secs(15), answer)
                                .await
                            {
                                Ok(Ok(Ok(()))) => json!({"applied": true}),
                                Ok(Ok(Err(reason))) => {
                                    json!({"applied": false, "failureReason": reason})
                                }
                                _ => {
                                    json!({"applied": false, "failureReason": "the edit was not applied"})
                                }
                            };
                            let _ = outgoing
                                .send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                        });
                    }
                    return;
                }
                Err(error) => {
                    let _ = outgoing.send(json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"applied": false, "failureReason": error.to_string()}
                    }));
                    return;
                }
            }
        }
        "window/showMessageRequest" => {
            let level = match message
                .pointer("/params/type")
                .and_then(Value::as_u64)
                .unwrap_or(4)
            {
                1 => MessageLevel::Error,
                2 => MessageLevel::Warning,
                3 => MessageLevel::Info,
                _ => MessageLevel::Log,
            };
            if let Some(text) = message.pointer("/params/message").and_then(Value::as_str) {
                let _ = events.send(ClientEvent::Message {
                    level,
                    message: text.to_owned(),
                });
            }
            Value::Null
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "workspace/diagnostic/refresh"
        | "window/showDocument" => Value::Null,
        other => {
            tracing::debug!(
                method = other,
                "server asked for something this editor does not do"
            );
            let _ = outgoing.send(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "not supported"}
            }));
            return;
        }
    };
    let _ = outgoing.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn handle_notification(method: &str, message: &Value, events: &mpsc::UnboundedSender<ClientEvent>) {
    match method {
        "textDocument/publishDiagnostics" => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            if let Some(uri) = params.get("uri").and_then(Value::as_str) {
                let _ = events.send(ClientEvent::Diagnostics {
                    uri: uri.to_owned(),
                    version: params
                        .get("version")
                        .and_then(Value::as_i64)
                        .map(|version| version as i32),
                    diagnostics: params
                        .get("diagnostics")
                        .map(parse_diagnostics)
                        .unwrap_or_default(),
                });
            }
        }
        "window/logMessage" | "window/showMessage" => {
            let level = match message
                .pointer("/params/type")
                .and_then(Value::as_u64)
                .unwrap_or(4)
            {
                1 => MessageLevel::Error,
                2 => MessageLevel::Warning,
                3 => MessageLevel::Info,
                _ => MessageLevel::Log,
            };
            if let Some(text) = message.pointer("/params/message").and_then(Value::as_str) {
                let _ = events.send(ClientEvent::Message {
                    level,
                    message: text.to_owned(),
                });
            }
        }
        "telemetry/event" | "$/progress" | "window/workDoneProgress/cancel" => {}
        other => tracing::trace!(method = other, "ignored server notification"),
    }
}

/// The value a server's `workspace/configuration` item asks for: `section` is a dotted path into
/// the settings object. A missing section is `null`.
fn configuration_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return settings.clone();
    };
    let mut value = settings;
    for part in section.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => return Value::Null,
        }
    }
    value.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_sections_are_dotted_paths() {
        let settings = json!({
            "python": {"analysis": {"typeCheckingMode": "basic"}},
            "rust-analyzer": {"checkOnSave": false}
        });
        assert_eq!(
            configuration_section(&settings, Some("python.analysis.typeCheckingMode")),
            json!("basic")
        );
        assert_eq!(
            configuration_section(&settings, Some("python.analysis")),
            json!({"typeCheckingMode": "basic"})
        );
        assert_eq!(
            configuration_section(&settings, Some("missing")),
            Value::Null
        );
        assert_eq!(configuration_section(&settings, None), settings);
    }

    #[test]
    fn initialize_params_advertise_utf16_and_snippets() {
        let command = ServerCommand {
            program: PathBuf::from("/bin/true"),
            args: vec![],
            cwd: PathBuf::from("/tmp"),
            env: vec![],
        };
        let mut options = StartOptions::new(command, "file:///tmp/ws");
        options.initialization_options = json!({"x": true});
        let params = initialize_params(&options);
        assert_eq!(params["rootUri"], "file:///tmp/ws");
        assert_eq!(params["initializationOptions"]["x"], true);
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"][0],
            "utf-16"
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["completion"]["completionItem"]["snippetSupport"],
            true
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["definition"]["linkSupport"],
            true
        );
        assert_eq!(
            params["capabilities"]["workspace"]["workspaceEdit"]["resourceOperations"][0],
            "create"
        );
    }
}
