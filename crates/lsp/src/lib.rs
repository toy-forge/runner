//! ScratchRunner's language-intelligence layer: a language-server client and manager, the
//! virtual workspace that gives unsaved documents a real file, snippet and markup models, and
//! text-only completion fallbacks that need no server. NSH-free: the UI converts these values to
//! its editor types.
//!
//! All positions are UTF-16 line/column pairs, the protocol's default encoding and the one NSH's
//! editor uses, so ranges map one to one.
//!
//! # Main types
//!
//! | Type | Purpose |
//! |--|--|
//! | [`Manager`], [`ManagerConfig`] | starts, syncs, restarts and asks the servers |
//! | [`ManagerEvent`], [`ServerStatus`] | status and diagnostics stream |
//! | [`ServerCatalog`], [`ServerDefinition`], [`ServerOverride`], [`ServerAvailability`] | data-driven server table and discovery |
//! | [`Client`], [`StartOptions`], [`ClientEvent`] | one JSON-RPC server process |
//! | [`VirtualWorkspace`], [`Scaffold`] | unsaved documents on disk, with Cargo/go/TS scaffolding |
//! | [`CompletionItem`], [`Hover`], [`SignatureHelp`], [`Definition`], [`DocumentSymbol`], [`CodeAction`], [`WorkspaceEdit`], [`FoldingRange`] | protocol answers |
//! | [`expand_snippet`], [`ExpandedSnippet`], [`TabStop`] | snippets as plain text with tab stops |
//! | [`RichText`], [`Block`], [`Inline`] | markdown/plaintext markup for hover and documentation |
//! | [`LanguageTables`], [`word_completions`], [`path_completions`], [`shell_completions`], [`fallback_completions`] | server-free completion |
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//! use scratch_lsp::{Position, fallback_completions, expand_snippet};
//!
//! let items = fallback_completions("de", Position::new(0, 2), "python", Path::new("."));
//! assert!(items.iter().any(|item| item.label == "def"));
//!
//! let snippet = expand_snippet("print(${1:value})");
//! assert_eq!(snippet.text, "print(value)");
//! assert_eq!(snippet.stops[0].utf16_offset, 6);
//! ```
//!
//! A typical app loop:
//!
//! ```no_run
//! use scratch_lsp::{Manager, ManagerConfig, ManagerEvent, Position};
//!
//! # async fn run() -> Result<(), scratch_lsp::LspError> {
//! let path = std::env::var("PATH").unwrap_or_default();
//! let (manager, mut events) = Manager::new(ManagerConfig::new(
//!     "/work",
//!     path,
//!     std::env::vars().collect(),
//! ));
//! manager
//!     .sync("file:///work/main.py", "python", "import os\nos.".to_owned())
//!     .await?;
//! let completions = manager
//!     .completion("file:///work/main.py", Position::new(1, 3), Default::default(), Some(".".into()))
//!     .await?;
//! let _ = completions;
//! while let Some(event) = events.recv().await {
//!     match event {
//!         ManagerEvent::Diagnostics { uri, diagnostics, .. } => { let _ = (uri, diagnostics); }
//!         ManagerEvent::Status { key, status, .. } => { let _ = (key, status); }
//!         ManagerEvent::Message { message, .. } => { let _ = message; }
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod capabilities;
pub mod fallback;
pub mod markup;
pub mod snippet;
pub mod text;
pub mod types;
pub mod uri;
pub mod workspace;

mod client;
mod error;
mod framing;
mod manager;
mod parse;
mod servers;

pub use capabilities::{
    CodeActionCapabilities, CompletionCapabilities, DiagnosticCapabilities, ServerCapabilities,
    SignatureHelpCapabilities, TextSyncCapabilities, TextSyncKind, WorkspaceEditCapabilities,
};
pub use client::{
    Client, ClientEvent, MessageLevel, REQUEST_TIMEOUT, RequestTicket, ServerCommand, ServerInfo,
    StartOptions,
};
pub use error::{LspError, Result};
pub use fallback::keywords::{LanguageTable, LanguageTables, SnippetTemplate, TableKind};
pub use fallback::paths::{looks_like_path, path_completions, path_fragment};
pub use fallback::shell::{
    ShellCompletionKind, common_commands, compgen_argv, compgen_command, parse_compgen_output,
    shell_builtins, shell_completions,
};
pub use fallback::words::word_completions;
pub use fallback::{fallback_completions, fuzzy_score, rank_completions};
pub use framing::{read_message, write_message};
pub use manager::{
    AppliedEdit, AppliedFileEdit, Manager, ManagerConfig, ManagerEvent, RestartPolicy,
    ServerStatus, TriggerCharacters,
};
pub use markup::{Block, Inline, RichText};
pub use servers::{
    ServerAvailability, ServerCatalog, ServerDefinition, ServerOverride, ServerSpec, availability,
    default_search_paths, expand_tilde, find_program, find_project_root,
};
pub use snippet::{ExpandedSnippet, TabStop, expand_snippet, expand_snippet_with};
pub use types::{
    CodeAction, Command, CompletionItem, CompletionKind, CompletionList, CompletionSource,
    CompletionTrigger, Definition, Diagnostic, DiagnosticReport, DocumentSymbol, FileEdit,
    FoldKind, FoldingRange, FormattingOptions, Hover, InsertFormat, Location, ParameterInfo,
    Position, PrepareRename, Range, RelatedInformation, Severity, SignatureHelp, SignatureInfo,
    SymbolKind, TextEdit, WorkspaceChange, WorkspaceEdit,
};
pub use uri::{file_name, is_file_uri, path_to_uri, scheme, uri_to_path};
pub use workspace::{Scaffold, VirtualFile, VirtualWorkspace};
