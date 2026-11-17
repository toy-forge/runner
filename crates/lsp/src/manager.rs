//! The language servers the editor uses: found on demand, started when a document needs one,
//! kept in sync with the open documents, restarted after a crash, and asked for language
//! intelligence.
//!
//! # Lifecycle
//!
//! [`Manager::sync`] is called whenever a document's text changes (or it is opened). The first
//! call for a language starts its server lazily; a crash is retried with exponential backoff; a
//! server whose program is missing is reported as [`ServerStatus::NotInstalled`] with an install
//! hint. Status and diagnostics arrive on the channel returned by [`Manager::new`].
//!
//! # Unsaved documents
//!
//! A document whose `file:` URI does not exist on disk (or a single Rust/Go/TypeScript file
//! outside a project) is materialised in a virtual workspace, so servers can read it. Diagnostics
//! and locations are translated back to the document's own URI before they reach the UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use crate::capabilities::{ServerCapabilities, TextSyncKind};
use crate::client::{
    Client, ClientEvent, MessageLevel, REQUEST_TIMEOUT, RequestTicket, StartOptions,
};
use crate::error::{LspError, Result};
use crate::fallback::LanguageTables;
use crate::parse::{
    parse_code_action, parse_code_actions, parse_completion_item, parse_completion_list,
    parse_definition, parse_diagnostic_report, parse_document_symbols, parse_folding_ranges,
    parse_hover, parse_locations, parse_prepare_rename, parse_signature_help, parse_text_edits,
    parse_workspace_edit,
};
use crate::servers::{ServerCatalog, ServerDefinition, ServerOverride, find_project_root, resolve};
use crate::text::{apply_edits, diff_change};
use crate::types::{
    CodeAction, Command, CompletionItem, CompletionList, CompletionTrigger, Definition, Diagnostic,
    DiagnosticReport, DocumentSymbol, FileEdit, FoldingRange, FormattingOptions, Hover, Location,
    Position, PrepareRename, Range, SignatureHelp, TextEdit, WorkspaceChange, WorkspaceEdit,
};
use crate::uri::{path_to_uri, uri_to_path};
use crate::workspace::{Scaffold, VirtualWorkspace, write_atomic};

/// How a crashed or failed server is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// The delay after the first failure; doubles with each further failure.
    pub initial: Duration,
    /// The longest delay.
    pub max: Duration,
    /// How long a server must have been ready before a crash resets the backoff.
    pub stable_after: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
            stable_after: Duration::from_secs(30),
        }
    }
}

impl RestartPolicy {
    /// The delay before the next start after `failures` consecutive failures (0 = the first).
    pub fn backoff(&self, failures: u32) -> Duration {
        let shift = failures.saturating_sub(1).min(10);
        self.initial.saturating_mul(1u32 << shift).min(self.max)
    }
}

/// Everything [`Manager::new`] needs.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// The application's project directory: servers prefer it as their workspace root when no
    /// document belongs to a project.
    pub workspace_root: PathBuf,
    /// Where virtual workspaces are written. Default:
    /// `~/Library/Caches/ScratchRunner/workspaces`.
    pub cache_dir: PathBuf,
    /// The `PATH` used to find servers (normally the app's `PATH`).
    pub path: String,
    /// The environment servers inherit (normally the app's environment).
    pub environment: Vec<(String, String)>,
    /// User overrides by server id.
    pub overrides: HashMap<String, ServerOverride>,
    /// The server definitions.
    pub catalog: ServerCatalog,
    /// Restart behaviour.
    pub restart: RestartPolicy,
}

impl ManagerConfig {
    /// A configuration with the built-in catalogue and the default cache.
    pub fn new(
        workspace_root: impl Into<PathBuf>,
        path: impl Into<String>,
        environment: Vec<(String, String)>,
    ) -> Self {
        let path = path.into();
        let environment = if environment.is_empty() {
            std::env::vars().collect()
        } else {
            environment
        };
        let path = if path.is_empty() {
            std::env::var("PATH").unwrap_or_default()
        } else {
            path
        };
        let workspace_root = workspace_root.into();
        let workspace_root = if workspace_root.is_absolute() {
            workspace_root
        } else {
            std::env::current_dir()
                .map(|current| current.join(&workspace_root))
                .unwrap_or(workspace_root)
        };
        Self {
            workspace_root,
            cache_dir: VirtualWorkspace::default_base(),
            path,
            environment,
            overrides: HashMap::new(),
            catalog: ServerCatalog::builtin().clone(),
            restart: RestartPolicy::default(),
        }
    }

    /// Uses a custom catalogue (tests, or an advanced user's file).
    pub fn with_catalog(mut self, catalog: ServerCatalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Uses a different virtual-workspace base.
    pub fn with_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = cache_dir.into();
        self
    }

    /// Sets the user overrides.
    pub fn with_overrides(mut self, overrides: HashMap<String, ServerOverride>) -> Self {
        self.overrides = overrides;
        self
    }

    /// Sets the restart policy.
    pub fn with_restart_policy(mut self, restart: RestartPolicy) -> Self {
        self.restart = restart;
        self
    }
}

/// Where a language's server stands, as the status bar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    /// Turned off in settings.
    Disabled,
    /// No program was found; the hint says what to install.
    NotInstalled {
        /// What the user can do.
        hint: String,
    },
    /// Starting and initializing.
    Starting,
    /// Ready for requests.
    Ready {
        /// The server's version, when it reports one.
        version: Option<String>,
    },
    /// Could not start, with the reason.
    Error {
        /// The reason.
        message: String,
    },
    /// Exited after having started; it starts again after the backoff.
    Stopped {
        /// Why it stopped.
        reason: String,
    },
}

impl ServerStatus {
    /// Whether the server is ready for requests.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Something the manager tells the app.
#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
    /// A server's diagnostics for a document.
    Diagnostics {
        /// The document's own URI (never a virtual-workspace URI).
        uri: String,
        /// The version they were computed for, when the server says.
        version: Option<i32>,
        /// The problems.
        diagnostics: Vec<Diagnostic>,
    },
    /// A server's status changed.
    Status {
        /// The server's catalogue id.
        key: String,
        /// Its display name.
        name: String,
        /// Its status now.
        status: ServerStatus,
    },
    /// A message a server asked the editor to show (for error banners).
    Message {
        /// The server's catalogue id.
        key: String,
        /// Its display name.
        name: String,
        /// How serious it is.
        level: MessageLevel,
        /// The text.
        message: String,
    },
}

/// The trigger characters a server wants for a document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerCharacters {
    /// Characters that ask for completions.
    pub completion: Vec<String>,
    /// Characters that ask for signature help.
    pub signature_help: Vec<String>,
}

/// One file changed by [`Manager::apply_workspace_edit`].
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedFileEdit {
    /// The document's own URI.
    pub uri: String,
    /// The document's version after the edit, for open documents.
    pub version: Option<i32>,
    /// The edits, relative to the text before they were applied.
    pub edits: Vec<TextEdit>,
    /// The whole text after the edits.
    pub new_text: String,
}

/// What [`Manager::apply_workspace_edit`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppliedEdit {
    /// Text changes, in order.
    pub edits: Vec<AppliedFileEdit>,
    /// Files and directories created.
    pub created: Vec<String>,
    /// Files moved: `(old_uri, new_uri)`.
    pub renamed: Vec<(String, String)>,
    /// Files and directories deleted.
    pub deleted: Vec<String>,
}

impl AppliedEdit {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
            && self.created.is_empty()
            && self.renamed.is_empty()
            && self.deleted.is_empty()
    }

    fn merge(&mut self, other: AppliedEdit) {
        self.edits.extend(other.edits);
        self.created.extend(other.created);
        self.renamed.extend(other.renamed);
        self.deleted.extend(other.deleted);
    }
}

struct Document {
    app_uri: String,
    server_uri: String,
    server_key: Option<String>,
    language: String,
    language_id: String,
    version: i32,
    text: String,
    opened: bool,
    disk_path: Option<PathBuf>,
    workspace_dir: Option<PathBuf>,
    diagnostics_epoch: u64,
}

struct Server {
    status: ServerStatus,
    client: Option<Arc<Client>>,
    capabilities: Option<ServerCapabilities>,
    root: Option<PathBuf>,
    failures: u32,
    ready_at: Option<Instant>,
    last_failure: Option<Instant>,
    restart_scheduled: bool,
    status_reported: bool,
    generation: u64,
    diagnostic_ids: HashMap<String, String>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            status: ServerStatus::Disabled,
            client: None,
            capabilities: None,
            root: None,
            failures: 0,
            ready_at: None,
            last_failure: None,
            restart_scheduled: false,
            status_reported: false,
            generation: 0,
            diagnostic_ids: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RequestKind {
    Completion,
    CompletionResolve,
    Hover,
    SignatureHelp,
    Definition,
    Declaration,
    TypeDefinition,
    References,
    DocumentSymbol,
    CodeAction,
    CodeActionResolve,
    Command,
    Formatting,
    RangeFormatting,
    FoldingRange,
    Rename,
    PrepareRename,
    Diagnostics,
}

struct Inner {
    documents: HashMap<String, Document>,
    servers: HashMap<String, Server>,
    latest_requests: HashMap<(String, RequestKind), u64>,
}

impl Inner {
    /// The app's URI for a URI a server used, when it belongs to an open document.
    fn app_uri_for_server(&self, server_uri: &str) -> Option<String> {
        if self.documents.contains_key(server_uri) {
            return Some(server_uri.to_owned());
        }
        self.documents
            .values()
            .find(|document| document.server_uri == server_uri)
            .map(|document| document.app_uri.clone())
    }

    fn has_documents_for(&self, key: &str) -> bool {
        self.documents
            .values()
            .any(|document| document.server_key.as_deref() == Some(key))
    }

    fn translate_definition(&self, definition: &mut Definition) {
        for location in &mut definition.targets {
            if let Some(app_uri) = self.app_uri_for_server(&location.uri) {
                location.uri = app_uri;
            }
        }
    }

    fn translate_locations(&self, locations: &mut [Location]) {
        for location in locations {
            if let Some(app_uri) = self.app_uri_for_server(&location.uri) {
                location.uri = app_uri;
            }
        }
    }

    fn translate_diagnostics(&self, diagnostics: &mut [Diagnostic]) {
        for diagnostic in diagnostics {
            for related in &mut diagnostic.related {
                if let Some(app_uri) = self.app_uri_for_server(&related.location.uri) {
                    related.location.uri = app_uri;
                }
            }
        }
    }

    fn cancel_requests_for(&mut self, uri: &str, client: Option<&Arc<Client>>) {
        let ids: Vec<u64> = self
            .latest_requests
            .iter()
            .filter(|((request_uri, _), _)| request_uri == uri)
            .map(|(_, id)| *id)
            .collect();
        self.latest_requests
            .retain(|(request_uri, _), _| request_uri != uri);
        if let Some(client) = client {
            for id in ids {
                client.cancel(id);
            }
        }
    }
}

/// Starts, feeds and asks the language servers on the editor's behalf.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<Inner>>,
    events: mpsc::UnboundedSender<ManagerEvent>,
    config: Arc<ManagerConfig>,
    workspace: Arc<VirtualWorkspace>,
}

impl Manager {
    /// A manager and the channel of its events.
    pub fn new(mut config: ManagerConfig) -> (Self, mpsc::UnboundedReceiver<ManagerEvent>) {
        if !config.cache_dir.is_absolute()
            && let Ok(current) = std::env::current_dir()
        {
            config.cache_dir = current.join(&config.cache_dir);
        }
        let (events, received) = mpsc::unbounded_channel();
        // Servers may store data under the cache (jdtls does); make sure it exists.
        let _ = std::fs::create_dir_all(&config.cache_dir);
        let workspace = Arc::new(VirtualWorkspace::new(config.cache_dir.clone()));
        let manager = Self {
            inner: Arc::new(Mutex::new(Inner {
                documents: HashMap::new(),
                servers: HashMap::new(),
                latest_requests: HashMap::new(),
            })),
            events,
            config: Arc::new(config),
            workspace,
        };
        (manager, received)
    }

    /// A manager for a project with the built-in catalogue and the app's environment.
    pub fn for_workspace(
        workspace_root: impl Into<PathBuf>,
    ) -> (Self, mpsc::UnboundedReceiver<ManagerEvent>) {
        Self::new(ManagerConfig::new(
            workspace_root,
            std::env::var("PATH").unwrap_or_default(),
            std::env::vars().collect(),
        ))
    }

    /// The server catalogue in use.
    pub fn catalog(&self) -> &ServerCatalog {
        &self.config.catalog
    }

    /// The virtual workspace base.
    pub fn workspace(&self) -> &VirtualWorkspace {
        self.workspace.as_ref()
    }

    /// Makes `uri` hold `text` as `language` (a ScratchRunner language id), starting the
    /// language's server lazily. The manager keeps its own version counter.
    pub async fn sync(&self, uri: &str, language: &str, text: String) -> Result<()> {
        let definition = self.catalog().server_for_language(language).cloned();
        let key = definition.as_ref().map(|definition| definition.id.clone());
        let language_id = definition
            .as_ref()
            .and_then(|definition| definition.language_id(language))
            .unwrap_or(language)
            .to_owned();
        let enabled = definition.as_ref().is_some_and(|definition| {
            definition.enabled
                && self
                    .config
                    .overrides
                    .get(&definition.id)
                    .and_then(|overridden| overridden.enabled)
                    .unwrap_or(true)
        });
        let disk_path = uri_to_path(uri).ok().filter(|path| path.is_file());
        let materialized = if let Some(definition) = definition.as_ref().filter(|_| enabled) {
            let project_root = disk_path
                .as_deref()
                .and_then(|path| find_project_root(path, &definition.root_markers));
            let needs_virtual = disk_path.is_none()
                || (project_root.is_none() && definition.scaffold != Scaffold::None);
            if needs_virtual {
                let extension = LanguageTables::builtin()
                    .extension(language)
                    .map(str::to_owned)
                    .or_else(|| {
                        disk_path
                            .as_deref()
                            .and_then(|path| path.extension())
                            .map(|extension| extension.to_string_lossy().into_owned())
                    });
                Some(self.workspace.prepare(
                    uri,
                    extension.as_deref(),
                    definition.virtual_path(language),
                    definition.scaffold,
                    &text,
                )?)
            } else {
                None
            }
        } else {
            None
        };
        let server_uri = materialized
            .as_ref()
            .map(|file| file.uri.clone())
            .unwrap_or_else(|| uri.to_owned());

        let mut start = None;
        let mut schedule_pull = false;
        let mut remove_workspace = false;
        {
            let mut inner = self.inner.lock().await;
            let existing = inner.documents.remove(uri);
            let old_client = existing
                .as_ref()
                .and_then(|document| document.server_key.as_ref())
                .and_then(|key| inner.servers.get(key))
                .and_then(|server| server.client.clone());
            inner.cancel_requests_for(uri, old_client.as_ref());
            let old_text = existing
                .as_ref()
                .map(|document| document.text.clone())
                .unwrap_or_default();
            let old_version = existing
                .as_ref()
                .map(|document| document.version)
                .unwrap_or(0);
            let old_epoch = existing
                .as_ref()
                .map(|document| document.diagnostics_epoch)
                .unwrap_or(0);
            let old_workspace = existing
                .as_ref()
                .and_then(|document| document.workspace_dir.clone());
            let same_identity = existing.as_ref().is_some_and(|document| {
                document.language == language
                    && document.language_id == language_id
                    && document.server_key.as_deref() == key.as_deref()
                    && document.server_uri == server_uri
            });
            let old_opened = existing.as_ref().is_some_and(|document| document.opened);
            if old_opened
                && !same_identity
                && let (Some(client), Some(old)) = (
                    old_client.as_ref(),
                    existing
                        .as_ref()
                        .map(|document| document.server_uri.clone()),
                )
            {
                let _ = client.notify(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": old}}),
                );
            }
            let running = key
                .as_ref()
                .and_then(|key| inner.servers.get(key))
                .is_some_and(|server| server.client.is_some());
            let had_existing = existing.is_some();
            if let Some(existing) = existing {
                if same_identity && existing.text == text && existing.opened && running {
                    inner.documents.insert(uri.to_owned(), existing);
                    return Ok(());
                }
                // The document changed identity or text; its old copy is replaced below. Keep the
                // old workspace only when it is the one being written now.
                remove_workspace = old_workspace.is_some() && materialized.is_none();
            }
            let changed = !same_identity || old_text != text;
            let version = match (had_existing, changed) {
                (false, _) => 1,
                (true, true) => old_version.saturating_add(1),
                (true, false) => old_version,
            };
            let epoch = old_epoch + u64::from(changed || !had_existing);
            let mut opened = old_opened && same_identity;
            let client = key
                .as_ref()
                .and_then(|key| inner.servers.get(key))
                .and_then(|server| server.client.clone());
            let sync_kind = key
                .as_ref()
                .and_then(|key| inner.servers.get(key))
                .and_then(|server| server.capabilities.as_ref())
                .map(|capabilities| capabilities.text_sync.kind)
                .unwrap_or(TextSyncKind::Full);
            match &client {
                Some(client) => {
                    if opened {
                        if old_text != text {
                            send_change(client, sync_kind, &server_uri, version, &old_text, &text);
                        }
                    } else {
                        let _ = client.notify(
                            "textDocument/didOpen",
                            open_params(&server_uri, &language_id, version, &text),
                        );
                        opened = true;
                    }
                }
                None => {
                    opened = false;
                    if enabled && key.is_some() {
                        start = key.clone();
                    }
                }
            }
            if enabled && key.is_some() {
                schedule_pull = opened
                    && key
                        .as_ref()
                        .and_then(|key| inner.servers.get(key))
                        .and_then(|server| server.capabilities.as_ref())
                        .is_some_and(|capabilities| capabilities.diagnostics.is_some());
            }
            inner.documents.insert(
                uri.to_owned(),
                Document {
                    app_uri: uri.to_owned(),
                    server_uri: server_uri.clone(),
                    server_key: key.clone(),
                    language: language.to_owned(),
                    language_id,
                    version,
                    text,
                    opened,
                    disk_path,
                    workspace_dir: materialized.as_ref().map(|file| file.root.clone()),
                    diagnostics_epoch: epoch,
                },
            );
            if !enabled && let Some(key) = &key {
                self.set_status(&mut inner, key, ServerStatus::Disabled);
            }
        }
        if remove_workspace {
            self.workspace.remove(uri);
        }
        if let Some(key) = start {
            self.ensure_started(&key).await?;
        } else if schedule_pull {
            self.schedule_pull(uri).await;
        }
        Ok(())
    }

    /// Forgets `uri` and tells its server.
    pub async fn close(&self, uri: &str) {
        let document = {
            let mut inner = self.inner.lock().await;
            let document = inner.documents.remove(uri);
            if let Some(document) = &document {
                let client = document
                    .server_key
                    .as_ref()
                    .and_then(|key| inner.servers.get(key))
                    .and_then(|server| server.client.clone());
                if let Some(client) = &client
                    && document.opened
                {
                    let _ = client.notify(
                        "textDocument/didClose",
                        json!({"textDocument": {"uri": document.server_uri}}),
                    );
                }
                inner.cancel_requests_for(uri, client.as_ref());
            }
            document
        };
        if document
            .as_ref()
            .is_some_and(|document| document.workspace_dir.is_some())
        {
            self.workspace.remove(uri);
        }
    }

    /// Tells the server the document was saved. Servers that do not want saves are not told.
    pub async fn save(&self, uri: &str) -> Result<()> {
        let inner = self.inner.lock().await;
        let document = inner
            .documents
            .get(uri)
            .ok_or_else(|| LspError::UnknownDocument(uri.to_owned()))?;
        let Some(key) = &document.server_key else {
            return Ok(());
        };
        let Some(server) = inner.servers.get(key) else {
            return Ok(());
        };
        if !document.opened {
            return Ok(());
        }
        let Some(capabilities) = &server.capabilities else {
            return Ok(());
        };
        if !capabilities.text_sync.save {
            return Ok(());
        }
        if let Some(client) = &server.client {
            let mut params = json!({"textDocument": {"uri": document.server_uri}});
            if capabilities.text_sync.save_include_text {
                params["text"] = Value::String(document.text.clone());
            }
            let _ = client.notify("textDocument/didSave", params);
        }
        Ok(())
    }

    /// The current status of a server, if it was ever needed.
    pub async fn status(&self, key: &str) -> Option<ServerStatus> {
        self.inner
            .lock()
            .await
            .servers
            .get(key)
            .map(|server| server.status.clone())
    }

    /// The current status of the server serving `language`, if it has one.
    pub async fn status_for_language(&self, language: &str) -> Option<ServerStatus> {
        let definition = self.catalog().server_for_language(language)?;
        if let Some(status) = self.status(&definition.id).await {
            return Some(status);
        }
        let disabled = !definition.enabled
            || self
                .config
                .overrides
                .get(&definition.id)
                .and_then(|overridden| overridden.enabled)
                == Some(false);
        disabled.then_some(ServerStatus::Disabled)
    }

    /// The completion and signature-help trigger characters of the document's server.
    pub async fn trigger_characters(&self, uri: &str) -> TriggerCharacters {
        let inner = self.inner.lock().await;
        let Some(document) = inner.documents.get(uri) else {
            return TriggerCharacters::default();
        };
        let Some(key) = &document.server_key else {
            return TriggerCharacters::default();
        };
        let Some(capabilities) = inner
            .servers
            .get(key)
            .and_then(|server| server.capabilities.as_ref())
        else {
            return TriggerCharacters::default();
        };
        TriggerCharacters {
            completion: capabilities
                .completion
                .as_ref()
                .map(|completion| completion.trigger_characters.clone())
                .unwrap_or_default(),
            signature_help: capabilities
                .signature_help
                .as_ref()
                .map(|signature| signature.trigger_characters.clone())
                .unwrap_or_default(),
        }
    }

    /// Starts a server now instead of waiting for the next edit. A missing program is reported
    /// through [`ServerStatus::NotInstalled`], not as an error here.
    pub async fn ensure_server(&self, key: &str) -> Result<()> {
        self.ensure_started(key).await
    }

    fn ensure_started<'a>(
        &'a self,
        key: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.ensure_started_inner(key))
    }

    async fn ensure_started_inner(&self, key: &str) -> Result<()> {
        let Some(definition) = self.catalog().get(key).cloned() else {
            return Ok(());
        };
        let overridden = self.config.overrides.get(key).cloned().unwrap_or_default();
        let (root, generation, spec) = {
            let mut inner = self.inner.lock().await;
            let root = self.choose_root(&inner, &definition);
            {
                let server = inner.servers.entry(key.to_owned()).or_default();
                if server.client.is_some() || matches!(&server.status, ServerStatus::Starting) {
                    return Ok(());
                }
                if let Some(last_failure) = server.last_failure
                    && last_failure.elapsed() < self.config.restart.backoff(server.failures)
                {
                    return Ok(());
                }
            }
            let cache = self.config.cache_dir.clone();
            match resolve(
                &definition,
                &overridden,
                &self.config.path,
                &self.config.environment,
                &root,
                &cache,
            ) {
                Ok(spec) => {
                    let server = inner.servers.entry(key.to_owned()).or_default();
                    server.root = Some(spec.root.clone());
                    server.generation = server.generation.wrapping_add(1);
                    let generation = server.generation;
                    self.set_status(&mut inner, key, ServerStatus::Starting);
                    (spec.root.clone(), generation, spec)
                }
                Err(status) => {
                    self.set_status(&mut inner, key, status);
                    return Ok(());
                }
            }
        };
        let _ = std::fs::create_dir_all(&root);
        let workspace_name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scratch".to_owned());
        let mut options = StartOptions::new(spec.command.clone(), path_to_uri(&root));
        options.workspace_name = workspace_name;
        options.initialization_options = overridden
            .initialization_options
            .clone()
            .or_else(|| definition.initialization_options.clone())
            .unwrap_or(Value::Null);
        options.settings = overridden
            .settings
            .clone()
            .or_else(|| definition.settings.clone())
            .unwrap_or(Value::Null);
        let (client_events, received) = mpsc::unbounded_channel();
        let started = Client::start(&options, client_events).await;
        match started {
            Ok(client) => {
                let client = Arc::new(client);
                let version = client.server_info().version.clone();
                let mut opened = Vec::new();
                {
                    let mut inner = self.inner.lock().await;
                    let stale = inner
                        .servers
                        .get(key)
                        .map(|server| server.generation != generation)
                        .unwrap_or(true);
                    if stale {
                        drop(inner);
                        client.shutdown().await;
                        return Ok(());
                    }
                    for document in inner
                        .documents
                        .values_mut()
                        .filter(|document| document.server_key.as_deref() == Some(key))
                    {
                        if !document.opened {
                            let _ = client.notify(
                                "textDocument/didOpen",
                                open_params(
                                    &document.server_uri,
                                    &document.language_id,
                                    document.version,
                                    &document.text,
                                ),
                            );
                            document.opened = true;
                            opened.push(document.app_uri.clone());
                        }
                    }
                    if let Some(server) = inner.servers.get_mut(key) {
                        server.client = Some(client.clone());
                        server.capabilities = Some(client.capabilities().clone());
                        server.root = Some(root.clone());
                        server.ready_at = Some(Instant::now());
                        server.last_failure = None;
                        server.restart_scheduled = false;
                    }
                    self.set_status(&mut inner, key, ServerStatus::Ready { version });
                }
                let manager = self.clone();
                let key_owned = key.to_owned();
                tokio::spawn(async move {
                    pump_events(manager, key_owned, generation, received).await;
                });
                for uri in opened {
                    self.schedule_pull(&uri).await;
                }
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                {
                    let mut inner = self.inner.lock().await;
                    let stale = inner
                        .servers
                        .get(key)
                        .map(|server| server.generation != generation)
                        .unwrap_or(true);
                    if !stale {
                        if let Some(server) = inner.servers.get_mut(key) {
                            server.client = None;
                            server.capabilities = None;
                            server.failures = server.failures.saturating_add(1);
                            server.last_failure = Some(Instant::now());
                        }
                        self.set_status(&mut inner, key, ServerStatus::Error { message });
                    }
                }
                self.schedule_restart(key).await;
                Ok(())
            }
        }
    }

    fn choose_root(&self, inner: &Inner, definition: &ServerDefinition) -> PathBuf {
        let mut virtual_root = None;
        let mut file_directory = None;
        for document in inner
            .documents
            .values()
            .filter(|document| document.server_key.as_deref() == Some(definition.id.as_str()))
        {
            if let Some(disk_path) = &document.disk_path {
                if !definition.root_markers.is_empty()
                    && let Some(root) = find_project_root(disk_path, &definition.root_markers)
                {
                    return root;
                }
                if file_directory.is_none() {
                    file_directory = disk_path.parent().map(Path::to_path_buf);
                }
            }
            if virtual_root.is_none() {
                virtual_root = document.workspace_dir.clone();
            }
        }
        virtual_root
            .or(file_directory)
            .unwrap_or_else(|| self.config.workspace_root.clone())
    }

    fn set_status(&self, inner: &mut Inner, key: &str, status: ServerStatus) {
        let name = self
            .catalog()
            .get(key)
            .map(|definition| definition.name.clone())
            .unwrap_or_else(|| key.to_owned());
        let server = inner.servers.entry(key.to_owned()).or_default();
        if server.status_reported && server.status == status {
            return;
        }
        server.status = status.clone();
        server.status_reported = true;
        let _ = self.events.send(ManagerEvent::Status {
            key: key.to_owned(),
            name,
            status,
        });
    }

    async fn schedule_restart(&self, key: &str) {
        let delay = {
            let mut inner = self.inner.lock().await;
            if !inner.has_documents_for(key) {
                return;
            }
            let Some(server) = inner.servers.get_mut(key) else {
                return;
            };
            if server.restart_scheduled
                || server.client.is_some()
                || matches!(&server.status, ServerStatus::Disabled)
            {
                return;
            }
            server.restart_scheduled = true;
            self.config.restart.backoff(server.failures)
        };
        let manager = self.clone();
        let key = key.to_owned();
        tokio::spawn(async move {
            // A little slack: `Instant::elapsed` at the other end must not be a hair short.
            tokio::time::sleep(delay + Duration::from_millis(50)).await;
            {
                let mut inner = manager.inner.lock().await;
                if !inner.has_documents_for(&key) {
                    return;
                }
                let Some(server) = inner.servers.get_mut(&key) else {
                    return;
                };
                server.restart_scheduled = false;
                if server.client.is_some() || matches!(&server.status, ServerStatus::Disabled) {
                    return;
                }
            }
            let _ = manager.ensure_started(&key).await;
        });
    }

    async fn schedule_pull(&self, uri: &str) {
        let Some((epoch, version)) = ({
            let inner = self.inner.lock().await;
            let Some(document) = inner.documents.get(uri) else {
                return;
            };
            let Some(key) = &document.server_key else {
                return;
            };
            let pull = inner
                .servers
                .get(key)
                .and_then(|server| server.capabilities.as_ref())
                .is_some_and(|capabilities| capabilities.diagnostics.is_some());
            pull.then_some((document.diagnostics_epoch, document.version))
        }) else {
            return;
        };
        let manager = self.clone();
        let uri = uri.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            {
                let inner = manager.inner.lock().await;
                let Some(document) = inner.documents.get(&uri) else {
                    return;
                };
                if document.diagnostics_epoch != epoch || !document.opened {
                    return;
                }
                let Some(key) = &document.server_key else {
                    return;
                };
                let ready = inner
                    .servers
                    .get(key)
                    .is_some_and(|server| server.client.is_some());
                if !ready {
                    return;
                }
            }
            let Ok(report) = manager.pull_diagnostics(&uri).await else {
                return;
            };
            if report.unchanged {
                return;
            }
            let version = {
                let inner = manager.inner.lock().await;
                inner
                    .documents
                    .get(&uri)
                    .map(|document| document.version)
                    .unwrap_or(version)
            };
            let _ = manager.events.send(ManagerEvent::Diagnostics {
                uri,
                version: Some(version),
                diagnostics: report.diagnostics,
            });
        });
    }

    async fn target(&self, uri: &str) -> Result<(Arc<Client>, String, ServerCapabilities)> {
        let inner = self.inner.lock().await;
        let document = inner
            .documents
            .get(uri)
            .ok_or_else(|| LspError::UnknownDocument(uri.to_owned()))?;
        let Some(key) = &document.server_key else {
            return Err(LspError::Unsupported(
                "this language has no language server".to_owned(),
            ));
        };
        if !document.opened {
            return Err(LspError::NotRunning);
        }
        let server = inner.servers.get(key).ok_or(LspError::NotRunning)?;
        let client = server.client.clone().ok_or(LspError::NotRunning)?;
        let capabilities = server.capabilities.clone().ok_or(LspError::NotRunning)?;
        Ok((client, document.server_uri.clone(), capabilities))
    }

    async fn send_request(
        &self,
        uri: &str,
        kind: RequestKind,
        client: &Arc<Client>,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let ticket: RequestTicket = client.request_async(method, params)?;
        let id = ticket.id();
        {
            let mut inner = self.inner.lock().await;
            let previous = inner.latest_requests.insert((uri.to_owned(), kind), id);
            if let Some(previous) = previous.filter(|previous| *previous != id) {
                client.cancel(previous);
            }
        }
        let answer = match tokio::time::timeout(REQUEST_TIMEOUT, ticket.receive()).await {
            Ok(answer) => answer,
            Err(_) => {
                client.cancel(id);
                Err(LspError::Timeout {
                    method: method.to_owned(),
                })
            }
        };
        {
            let mut inner = self.inner.lock().await;
            if inner.latest_requests.get(&(uri.to_owned(), kind)) == Some(&id) {
                inner.latest_requests.remove(&(uri.to_owned(), kind));
            }
        }
        answer
    }

    /// Completions at `position`. The returned items keep the protocol's shapes; snippet
    /// insertion texts can be expanded with [`crate::expand_snippet`].
    pub async fn completion(
        &self,
        uri: &str,
        position: Position,
        trigger: CompletionTrigger,
        character: Option<String>,
    ) -> Result<CompletionList> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if capabilities.completion.is_none() {
            return Err(LspError::Unsupported("textDocument/completion".to_owned()));
        }
        let mut context = match trigger {
            CompletionTrigger::Invoked => json!({"triggerKind": 1}),
            CompletionTrigger::Character(_) => json!({"triggerKind": 2}),
            CompletionTrigger::Incomplete => json!({"triggerKind": 3}),
        };
        if let Some(character) = character {
            context["triggerCharacter"] = Value::String(character);
        }
        let value = self
            .send_request(
                uri,
                RequestKind::Completion,
                &client,
                "textDocument/completion",
                json!({
                    "textDocument": {"uri": server_uri},
                    "position": position_json(position),
                    "context": context
                }),
            )
            .await?;
        Ok(parse_completion_list(&value))
    }

    /// Resolves a completion item (`completionItem/resolve`). Returns the item unchanged when the
    /// server does not support resolving.
    pub async fn resolve_completion(
        &self,
        uri: &str,
        item: &CompletionItem,
    ) -> Result<CompletionItem> {
        let (client, _server_uri, capabilities) = self.target(uri).await?;
        let Some(completion) = &capabilities.completion else {
            return Err(LspError::Unsupported("textDocument/completion".to_owned()));
        };
        if !completion.resolve {
            return Ok(item.clone());
        }
        let params = item
            .raw
            .clone()
            .unwrap_or_else(|| json!({"label": item.label}));
        let value = self
            .send_request(
                uri,
                RequestKind::CompletionResolve,
                &client,
                "completionItem/resolve",
                params,
            )
            .await?;
        parse_completion_item(&value).ok_or_else(|| {
            LspError::Protocol("completionItem/resolve returned no completion item".to_owned())
        })
    }

    /// The hover at `position`.
    pub async fn hover(&self, uri: &str, position: Position) -> Result<Option<Hover>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.hover {
            return Err(LspError::Unsupported("textDocument/hover".to_owned()));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::Hover,
                &client,
                "textDocument/hover",
                json!({"textDocument": {"uri": server_uri}, "position": position_json(position)}),
            )
            .await?;
        Ok(parse_hover(&value))
    }

    /// Signature help at `position`.
    pub async fn signature_help(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<SignatureHelp>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if capabilities.signature_help.is_none() {
            return Err(LspError::Unsupported(
                "textDocument/signatureHelp".to_owned(),
            ));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::SignatureHelp,
                &client,
                "textDocument/signatureHelp",
                json!({"textDocument": {"uri": server_uri}, "position": position_json(position)}),
            )
            .await?;
        Ok(parse_signature_help(&value))
    }

    /// `textDocument/definition`, with targets in app URIs.
    pub async fn definition(&self, uri: &str, position: Position) -> Result<Definition> {
        self.definition_like(
            uri,
            position,
            RequestKind::Definition,
            "textDocument/definition",
        )
        .await
    }

    /// `textDocument/declaration`.
    pub async fn declaration(&self, uri: &str, position: Position) -> Result<Definition> {
        self.definition_like(
            uri,
            position,
            RequestKind::Declaration,
            "textDocument/declaration",
        )
        .await
    }

    /// `textDocument/typeDefinition`.
    pub async fn type_definition(&self, uri: &str, position: Position) -> Result<Definition> {
        self.definition_like(
            uri,
            position,
            RequestKind::TypeDefinition,
            "textDocument/typeDefinition",
        )
        .await
    }

    async fn definition_like(
        &self,
        uri: &str,
        position: Position,
        kind: RequestKind,
        method: &str,
    ) -> Result<Definition> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        let supported = match kind {
            RequestKind::Definition => capabilities.definition,
            RequestKind::Declaration => capabilities.declaration,
            RequestKind::TypeDefinition => capabilities.type_definition,
            _ => false,
        };
        if !supported {
            return Err(LspError::Unsupported(method.to_owned()));
        }
        let value = self
            .send_request(
                uri,
                kind,
                &client,
                method,
                json!({"textDocument": {"uri": server_uri}, "position": position_json(position)}),
            )
            .await?;
        let mut definition = parse_definition(&value);
        self.inner
            .lock()
            .await
            .translate_definition(&mut definition);
        Ok(definition)
    }

    /// `textDocument/references`, with locations in app URIs.
    pub async fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.references {
            return Err(LspError::Unsupported("textDocument/references".to_owned()));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::References,
                &client,
                "textDocument/references",
                json!({
                    "textDocument": {"uri": server_uri},
                    "position": position_json(position),
                    "context": {"includeDeclaration": include_declaration}
                }),
            )
            .await?;
        let mut locations = parse_locations(&value);
        self.inner.lock().await.translate_locations(&mut locations);
        Ok(locations)
    }

    /// `textDocument/documentSymbol`.
    pub async fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.document_symbol {
            return Err(LspError::Unsupported(
                "textDocument/documentSymbol".to_owned(),
            ));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::DocumentSymbol,
                &client,
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": server_uri}}),
            )
            .await?;
        Ok(parse_document_symbols(&value))
    }

    /// `textDocument/codeAction`.
    pub async fn code_actions(
        &self,
        uri: &str,
        range: Range,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<CodeAction>> {
        self.code_actions_with_kinds(uri, range, diagnostics, None)
            .await
    }

    /// Code actions restricted to the given kinds (`["quickfix"]`, `["source.organizeImports"]`).
    pub async fn code_actions_with_kinds(
        &self,
        uri: &str,
        range: Range,
        diagnostics: &[Diagnostic],
        only: Option<&[String]>,
    ) -> Result<Vec<CodeAction>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if capabilities.code_action.is_none() {
            return Err(LspError::Unsupported("textDocument/codeAction".to_owned()));
        }
        let mut context = json!({
            "diagnostics": diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
            "triggerKind": 1
        });
        if let Some(only) = only {
            context["only"] = json!(only);
        }
        let value = self
            .send_request(
                uri,
                RequestKind::CodeAction,
                &client,
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": server_uri},
                    "range": range,
                    "context": context
                }),
            )
            .await?;
        Ok(parse_code_actions(&value))
    }

    /// Resolves a code action (`codeAction/resolve`), returning it unchanged when unsupported.
    pub async fn resolve_code_action(&self, uri: &str, action: &CodeAction) -> Result<CodeAction> {
        let (client, _server_uri, capabilities) = self.target(uri).await?;
        let Some(code_action) = &capabilities.code_action else {
            return Err(LspError::Unsupported("textDocument/codeAction".to_owned()));
        };
        if !code_action.resolve {
            return Ok(action.clone());
        }
        let Some(raw) = &action.raw else {
            return Ok(action.clone());
        };
        let value = self
            .send_request(
                uri,
                RequestKind::CodeActionResolve,
                &client,
                "codeAction/resolve",
                raw.clone(),
            )
            .await?;
        Ok(parse_code_action(&value).unwrap_or_else(|| action.clone()))
    }

    /// `workspace/executeCommand`.
    pub async fn execute_command(&self, uri: &str, command: &Command) -> Result<Value> {
        let (client, _server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.execute_command {
            return Err(LspError::Unsupported("workspace/executeCommand".to_owned()));
        }
        self.send_request(
            uri,
            RequestKind::Command,
            &client,
            "workspace/executeCommand",
            json!({"command": command.command, "arguments": command.arguments}),
        )
        .await
    }

    /// Resolves (if needed), applies the edit of and runs the command of a code action.
    pub async fn apply_code_action(&self, uri: &str, action: &CodeAction) -> Result<AppliedEdit> {
        if let Some(reason) = &action.disabled_reason {
            return Err(LspError::InvalidEdit(format!(
                "the code action is disabled: {reason}"
            )));
        }
        let mut action = action.clone();
        if action.edit.is_none() {
            let command = action.command.clone();
            let mut resolved = self.resolve_code_action(uri, &action).await?;
            if resolved.command.is_none() {
                resolved.command = command;
            }
            action = resolved;
        }
        let mut applied = AppliedEdit::default();
        if let Some(edit) = &action.edit {
            applied.merge(self.apply_workspace_edit(edit).await?);
        }
        if let Some(command) = &action.command {
            let result = self.execute_command(uri, command).await?;
            let edit = parse_workspace_edit(&result)?;
            if !edit.is_empty() {
                applied.merge(self.apply_workspace_edit(&edit).await?);
            }
        }
        Ok(applied)
    }

    /// `textDocument/formatting`.
    pub async fn format(&self, uri: &str, options: FormattingOptions) -> Result<Vec<TextEdit>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.formatting {
            return Err(LspError::Unsupported("textDocument/formatting".to_owned()));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::Formatting,
                &client,
                "textDocument/formatting",
                json!({"textDocument": {"uri": server_uri}, "options": options.to_json()}),
            )
            .await?;
        Ok(parse_text_edits(&value))
    }

    /// `textDocument/rangeFormatting`.
    pub async fn format_range(
        &self,
        uri: &str,
        range: Range,
        options: FormattingOptions,
    ) -> Result<Vec<TextEdit>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.range_formatting {
            return Err(LspError::Unsupported(
                "textDocument/rangeFormatting".to_owned(),
            ));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::RangeFormatting,
                &client,
                "textDocument/rangeFormatting",
                json!({
                    "textDocument": {"uri": server_uri},
                    "range": range,
                    "options": options.to_json()
                }),
            )
            .await?;
        Ok(parse_text_edits(&value))
    }

    /// `textDocument/foldingRange`.
    pub async fn folding_ranges(&self, uri: &str) -> Result<Vec<FoldingRange>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.folding_range {
            return Err(LspError::Unsupported(
                "textDocument/foldingRange".to_owned(),
            ));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::FoldingRange,
                &client,
                "textDocument/foldingRange",
                json!({"textDocument": {"uri": server_uri}}),
            )
            .await?;
        Ok(parse_folding_ranges(&value))
    }

    /// `textDocument/prepareRename`.
    pub async fn prepare_rename(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<PrepareRename>> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.prepare_rename {
            return Err(LspError::Unsupported(
                "textDocument/prepareRename".to_owned(),
            ));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::PrepareRename,
                &client,
                "textDocument/prepareRename",
                json!({"textDocument": {"uri": server_uri}, "position": position_json(position)}),
            )
            .await?;
        Ok(parse_prepare_rename(&value))
    }

    /// `textDocument/rename`, returning the edit (not applying it).
    pub async fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit> {
        let (client, server_uri, capabilities) = self.target(uri).await?;
        if !capabilities.rename {
            return Err(LspError::Unsupported("textDocument/rename".to_owned()));
        }
        let value = self
            .send_request(
                uri,
                RequestKind::Rename,
                &client,
                "textDocument/rename",
                json!({
                    "textDocument": {"uri": server_uri},
                    "position": position_json(position),
                    "newName": new_name
                }),
            )
            .await?;
        parse_workspace_edit(&value)
    }

    /// Pull diagnostics (`textDocument/diagnostic`).
    pub async fn pull_diagnostics(&self, uri: &str) -> Result<DiagnosticReport> {
        let (key, server_uri) = {
            let inner = self.inner.lock().await;
            let document = inner
                .documents
                .get(uri)
                .ok_or_else(|| LspError::UnknownDocument(uri.to_owned()))?;
            let Some(key) = document.server_key.clone() else {
                return Err(LspError::Unsupported(
                    "this language has no language server".to_owned(),
                ));
            };
            if !document.opened {
                return Err(LspError::NotRunning);
            }
            (key, document.server_uri.clone())
        };
        let (client, capabilities, previous) = {
            let inner = self.inner.lock().await;
            let server = inner.servers.get(&key).ok_or(LspError::NotRunning)?;
            (
                server.client.clone().ok_or(LspError::NotRunning)?,
                server.capabilities.clone().ok_or(LspError::NotRunning)?,
                server.diagnostic_ids.get(&server_uri).cloned(),
            )
        };
        if capabilities.diagnostics.is_none() {
            return Err(LspError::Unsupported("textDocument/diagnostic".to_owned()));
        }
        let mut params = json!({"textDocument": {"uri": server_uri}});
        if let Some(previous) = &previous {
            params["previousResultId"] = Value::String(previous.clone());
        }
        let value = self
            .send_request(
                uri,
                RequestKind::Diagnostics,
                &client,
                "textDocument/diagnostic",
                params,
            )
            .await?;
        let mut report = parse_diagnostic_report(&value)?;
        {
            let mut inner = self.inner.lock().await;
            let key = inner
                .documents
                .get(uri)
                .and_then(|document| document.server_key.clone());
            if let Some(key) = key
                && let Some(server) = inner.servers.get_mut(&key)
            {
                match &report.result_id {
                    Some(id) => {
                        server.diagnostic_ids.insert(server_uri.clone(), id.clone());
                    }
                    None => {
                        server.diagnostic_ids.remove(&server_uri);
                    }
                }
            }
            inner.translate_diagnostics(&mut report.diagnostics);
        }
        Ok(report)
    }

    /// Pull diagnostics, as a plain list.
    pub async fn diagnostics(&self, uri: &str) -> Result<Vec<Diagnostic>> {
        Ok(self.pull_diagnostics(uri).await?.diagnostics)
    }

    /// Applies a `WorkspaceEdit` to open documents and to files on disk, validating versions and
    /// ranges and refusing paths outside the workspace. Open documents get the new text and the
    /// per-file edits; the UI applies those edits to its editor buffers.
    pub async fn apply_workspace_edit(&self, edit: &WorkspaceEdit) -> Result<AppliedEdit> {
        let mut applied = AppliedEdit::default();
        let mut changed_uris = Vec::new();
        for change in &edit.changes {
            match change {
                WorkspaceChange::Edit(file) => {
                    self.apply_file_edit(file, &mut applied, &mut changed_uris)
                        .await?;
                }
                WorkspaceChange::Create {
                    uri,
                    overwrite,
                    ignore_if_exists,
                } => {
                    self.apply_create(uri, *overwrite, *ignore_if_exists, &mut applied)
                        .await?
                }
                WorkspaceChange::Rename {
                    old_uri,
                    new_uri,
                    overwrite,
                } => {
                    self.apply_rename(old_uri, new_uri, *overwrite, &mut applied)
                        .await?;
                }
                WorkspaceChange::Delete { uri, recursive } => {
                    self.apply_delete(uri, *recursive, &mut applied).await?;
                }
            }
        }
        for uri in changed_uris {
            self.schedule_pull(&uri).await;
        }
        Ok(applied)
    }

    async fn apply_file_edit(
        &self,
        file: &FileEdit,
        applied: &mut AppliedEdit,
        changed_uris: &mut Vec<String>,
    ) -> Result<()> {
        if file.edits.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let app_uri = inner
            .app_uri_for_server(&file.uri)
            .unwrap_or_else(|| file.uri.clone());
        if let Some(document) = inner.documents.get_mut(&app_uri) {
            let (opened, server_key, server_uri, version, new_text, old_text, virtual_path) = {
                if let Some(expected) = file.version
                    && expected != document.version
                {
                    return Err(LspError::InvalidEdit(format!(
                        "{app_uri} is version {}, but the edit is for version {expected}",
                        document.version
                    )));
                }
                let new_text = apply_edits(&document.text, &file.edits)?;
                let server_uri = document.server_uri.clone();
                let virtual_path = (server_uri != document.app_uri)
                    .then(|| uri_to_path(&server_uri).ok())
                    .flatten();
                // Write the virtual copy before mutating the document, so a cache write failure
                // leaves the document and the server in their previous state.
                if let Some(path) = &virtual_path {
                    write_atomic(path, &new_text)?;
                }
                let old_text = std::mem::replace(&mut document.text, new_text.clone());
                document.version = document.version.saturating_add(1);
                document.diagnostics_epoch = document.diagnostics_epoch.saturating_add(1);
                (
                    document.opened,
                    document.server_key.clone(),
                    server_uri,
                    document.version,
                    new_text,
                    old_text,
                    virtual_path,
                )
            };
            let change = if opened {
                server_key
                    .as_ref()
                    .and_then(|key| inner.servers.get(key))
                    .and_then(|server| {
                        server.client.clone().map(|client| {
                            let kind = server
                                .capabilities
                                .as_ref()
                                .map(|capabilities| capabilities.text_sync.kind)
                                .unwrap_or(TextSyncKind::Full);
                            (client, kind, server_uri.clone(), old_text)
                        })
                    })
            } else {
                None
            };
            applied.edits.push(AppliedFileEdit {
                uri: app_uri.clone(),
                version: Some(version),
                edits: file.edits.clone(),
                new_text: new_text.clone(),
            });
            drop(inner);
            if let Some((client, kind, server_uri, old_text)) = change {
                send_change(&client, kind, &server_uri, version, &old_text, &new_text);
            }
            let _ = virtual_path;
            changed_uris.push(app_uri);
            return Ok(());
        }
        let path = uri_to_path(&file.uri)?;
        self.check_path_allowed(&inner, &path)?;
        drop(inner);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| LspError::Io(format!("{}: {error}", path.display())))?;
        let new_text = apply_edits(&text, &file.edits)?;
        write_atomic(&path, &new_text)?;
        applied.edits.push(AppliedFileEdit {
            uri: file.uri.clone(),
            version: None,
            edits: file.edits.clone(),
            new_text,
        });
        Ok(())
    }

    async fn apply_create(
        &self,
        uri: &str,
        overwrite: bool,
        ignore_if_exists: bool,
        applied: &mut AppliedEdit,
    ) -> Result<()> {
        let path = uri_to_path(uri)?;
        let inner = self.inner.lock().await;
        self.check_path_allowed(&inner, &path)?;
        drop(inner);
        let is_directory = uri.ends_with('/');
        if path.exists() {
            if ignore_if_exists {
                return Ok(());
            }
            if !overwrite {
                return Err(LspError::InvalidEdit(format!(
                    "{} already exists",
                    path.display()
                )));
            }
        }
        if is_directory {
            std::fs::create_dir_all(&path)?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_atomic(&path, "")?;
        }
        applied.created.push(uri.to_owned());
        Ok(())
    }

    async fn apply_rename(
        &self,
        old_uri: &str,
        new_uri: &str,
        overwrite: bool,
        applied: &mut AppliedEdit,
    ) -> Result<()> {
        let old_path = uri_to_path(old_uri)?;
        let new_path = uri_to_path(new_uri)?;
        let mut inner = self.inner.lock().await;
        self.check_path_allowed(&inner, &old_path)?;
        self.check_path_allowed(&inner, &new_path)?;
        if new_path.exists() && !overwrite {
            return Err(LspError::InvalidEdit(format!(
                "{} already exists",
                new_path.display()
            )));
        }
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_path, &new_path)
            .map_err(|error| LspError::Io(format!("rename: {error}")))?;
        // Keep an open document's server URI current.
        let mut app_old = old_uri.to_owned();
        let mut app_new = new_uri.to_owned();
        for document in inner.documents.values_mut() {
            if document.server_uri == old_uri {
                document.server_uri = new_uri.to_owned();
                app_old = document.app_uri.clone();
            }
        }
        if inner.documents.contains_key(new_uri) {
            app_new = new_uri.to_owned();
        }
        applied.renamed.push((app_old, app_new));
        Ok(())
    }

    async fn apply_delete(
        &self,
        uri: &str,
        recursive: bool,
        applied: &mut AppliedEdit,
    ) -> Result<()> {
        let path = uri_to_path(uri)?;
        let inner = self.inner.lock().await;
        self.check_path_allowed(&inner, &path)?;
        if path == self.config.workspace_root || path == self.config.cache_dir {
            return Err(LspError::InvalidEdit(
                "refusing to delete the workspace root".to_owned(),
            ));
        }
        drop(inner);
        if path.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_dir(&path)?;
            }
        } else {
            std::fs::remove_file(&path)?;
        }
        let app_uri = uri.to_owned();
        applied.deleted.push(app_uri);
        Ok(())
    }

    fn check_path_allowed(&self, inner: &Inner, path: &Path) -> Result<()> {
        if !path.is_absolute() {
            return Err(LspError::InvalidEdit(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        let mut roots: Vec<PathBuf> = vec![
            self.config.workspace_root.clone(),
            self.config.cache_dir.clone(),
        ];
        for document in inner.documents.values() {
            if let Some(disk_path) = &document.disk_path
                && let Some(parent) = disk_path.parent()
            {
                roots.push(parent.to_path_buf());
            }
        }
        // Check both the lexical path and the path with symlinks and `..` resolved, so a symlink
        // or a `..` inside the workspace cannot reach outside it.
        let resolved = resolved_path(path);
        let allowed = roots.iter().any(|root| path.starts_with(root))
            && roots
                .iter()
                .any(|root| resolved.starts_with(resolved_path(root)));
        if allowed {
            Ok(())
        } else {
            Err(LspError::InvalidEdit(format!(
                "{} is outside the workspace",
                path.display()
            )))
        }
    }

    /// Shuts every server down and clears the documents. Call before the app quits.
    pub async fn shutdown(&self) {
        let (clients, ids) = {
            let mut inner = self.inner.lock().await;
            inner.latest_requests.clear();
            let ids: Vec<String> = inner.documents.keys().cloned().collect();
            inner.documents.clear();
            let mut clients = Vec::new();
            for server in inner.servers.values_mut() {
                server.generation = server.generation.wrapping_add(1);
                if let Some(client) = server.client.take() {
                    clients.push(client);
                }
            }
            (clients, ids)
        };
        for client in clients {
            let _ = tokio::time::timeout(Duration::from_secs(2), client.shutdown()).await;
        }
        for id in ids {
            self.workspace.remove(&id);
        }
    }
}

async fn pump_events(
    manager: Manager,
    key: String,
    generation: u64,
    mut events: mpsc::UnboundedReceiver<ClientEvent>,
) {
    while let Some(event) = events.recv().await {
        match event {
            ClientEvent::Diagnostics {
                uri,
                version,
                mut diagnostics,
            } => {
                let app_uri = {
                    let inner = manager.inner.lock().await;
                    let app_uri = inner
                        .app_uri_for_server(&uri)
                        .unwrap_or_else(|| uri.clone());
                    inner.translate_diagnostics(&mut diagnostics);
                    app_uri
                };
                let _ = manager.events.send(ManagerEvent::Diagnostics {
                    uri: app_uri,
                    version,
                    diagnostics,
                });
            }
            ClientEvent::Message { level, message } => {
                let name = manager
                    .catalog()
                    .get(&key)
                    .map(|definition| definition.name.clone())
                    .unwrap_or_else(|| key.clone());
                let _ = manager.events.send(ManagerEvent::Message {
                    key: key.clone(),
                    name,
                    level,
                    message,
                });
            }
            ClientEvent::ApplyEdit { edit, respond } => {
                let result = manager.apply_workspace_edit(&edit).await;
                let answer = result.map(|_| ()).map_err(|error| error.to_string());
                let _ = respond.send(answer);
            }
            ClientEvent::Stopped { reason } => {
                let mut inner = manager.inner.lock().await;
                let current = inner
                    .servers
                    .get(&key)
                    .map(|server| server.generation == generation)
                    .unwrap_or(false);
                if current {
                    if let Some(server) = inner.servers.get_mut(&key) {
                        server.client = None;
                        server.capabilities = None;
                        if server
                            .ready_at
                            .is_some_and(|at| at.elapsed() >= manager.config.restart.stable_after)
                        {
                            server.failures = 0;
                        }
                        server.failures = server.failures.saturating_add(1);
                        server.last_failure = Some(Instant::now());
                    }
                    for document in inner
                        .documents
                        .values_mut()
                        .filter(|document| document.server_key.as_deref() == Some(key.as_str()))
                    {
                        document.opened = false;
                    }
                    inner.latest_requests.clear();
                    manager.set_status(
                        &mut inner,
                        &key,
                        ServerStatus::Stopped {
                            reason: reason.clone(),
                        },
                    );
                }
                drop(inner);
                if current {
                    manager.schedule_restart(&key).await;
                }
                break;
            }
        }
    }
}

/// The path with symlinks and `..` resolved as far as it exists; the non-existing tail is kept.
/// Used for the workspace-boundary check, which must not be fooled by a symlink.
fn resolved_path(path: &Path) -> PathBuf {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for part in suffix.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn open_params(uri: &str, language_id: &str, version: i32, text: &str) -> Value {
    json!({
        "textDocument": {
            "uri": uri,
            "languageId": language_id,
            "version": version,
            "text": text
        }
    })
}

fn position_json(position: Position) -> Value {
    json!({"line": position.line, "character": position.character})
}

fn send_change(
    client: &Client,
    kind: TextSyncKind,
    server_uri: &str,
    version: i32,
    old_text: &str,
    new_text: &str,
) {
    let changes = match kind {
        TextSyncKind::None => return,
        TextSyncKind::Full => json!([{"text": new_text}]),
        TextSyncKind::Incremental => match diff_change(old_text, new_text) {
            Some(change) => json!([{
                "range": change.range,
                "rangeLength": change.range_length,
                "text": change.text
            }]),
            None => json!([]),
        },
    };
    let _ = client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": server_uri, "version": version},
            "contentChanges": changes
        }),
    );
}
