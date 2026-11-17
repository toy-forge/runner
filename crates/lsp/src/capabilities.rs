//! The server capabilities the manager needs, parsed from `initialize`'s answer.
//!
//! The protocol describes them with many optional fields; this is the subset that changes what
//! the editor does: how documents are synced, which requests may be sent, and which trigger
//! characters to honour. Anything absent means "the server does not offer it".

use serde_json::Value;

/// How a server wants document changes (`TextDocumentSyncKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSyncKind {
    /// The server does not want text documents.
    None,
    /// Every change sends the whole text.
    #[default]
    Full,
    /// Changes send the replaced range and the new text.
    Incremental,
}

impl TextSyncKind {
    fn from_lsp(number: u64) -> Self {
        match number {
            0 => Self::None,
            2 => Self::Incremental,
            _ => Self::Full,
        }
    }
}

/// How a server wants saves and opens announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSyncCapabilities {
    /// How changes are sent.
    pub kind: TextSyncKind,
    /// Whether `didOpen`/`didClose` are wanted.
    pub open_close: bool,
    /// Whether `didSave` is wanted.
    pub save: bool,
    /// Whether `didSave` should carry the whole text.
    pub save_include_text: bool,
}

impl Default for TextSyncCapabilities {
    fn default() -> Self {
        Self {
            kind: TextSyncKind::Full,
            open_close: true,
            save: false,
            save_include_text: false,
        }
    }
}

/// A completion provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionCapabilities {
    /// Characters that should ask for completions when typed.
    pub trigger_characters: Vec<String>,
    /// Whether `completionItem/resolve` is offered.
    pub resolve: bool,
    /// Characters that accept an item, when the server has defaults.
    pub commit_characters: Vec<String>,
}

/// A signature-help provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureHelpCapabilities {
    /// Characters that ask for signatures when typed.
    pub trigger_characters: Vec<String>,
    /// Characters that re-ask while a signature is shown.
    pub retrigger_characters: Vec<String>,
}

/// A code-action provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodeActionCapabilities {
    /// Whether `codeAction/resolve` is offered.
    pub resolve: bool,
    /// The kinds the server offers.
    pub kinds: Vec<String>,
}

/// A pull-diagnostics provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiagnosticCapabilities {
    /// Whether a report can depend on other files.
    pub inter_file_dependencies: bool,
    /// Whether workspace diagnostics are offered.
    pub workspace_diagnostics: bool,
}

/// What a workspace edit may contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkspaceEditCapabilities {
    /// Whether `documentChanges` (ordered edits and file operations) is understood.
    pub document_changes: bool,
    /// Whether create/rename/delete file operations are understood.
    pub resource_operations: bool,
}

/// The parts of `ServerCapabilities` the client uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCapabilities {
    /// The position encoding the server chose; always `utf-16` by the time the client accepts it.
    pub position_encoding: String,
    /// Text-document syncing.
    pub text_sync: TextSyncCapabilities,
    /// Completions, when offered.
    pub completion: Option<CompletionCapabilities>,
    /// Whether hovers are offered.
    pub hover: bool,
    /// Signature help, when offered.
    pub signature_help: Option<SignatureHelpCapabilities>,
    /// `textDocument/definition`.
    pub definition: bool,
    /// `textDocument/declaration`.
    pub declaration: bool,
    /// `textDocument/typeDefinition`.
    pub type_definition: bool,
    /// `textDocument/references`.
    pub references: bool,
    /// `textDocument/documentSymbol`.
    pub document_symbol: bool,
    /// Code actions, when offered.
    pub code_action: Option<CodeActionCapabilities>,
    /// `textDocument/formatting`.
    pub formatting: bool,
    /// `textDocument/rangeFormatting`.
    pub range_formatting: bool,
    /// `textDocument/foldingRange`.
    pub folding_range: bool,
    /// `textDocument/rename`.
    pub rename: bool,
    /// `textDocument/prepareRename`.
    pub prepare_rename: bool,
    /// Pull diagnostics, when offered.
    pub diagnostics: Option<DiagnosticCapabilities>,
    /// `workspace/executeCommand`.
    pub execute_command: bool,
    /// What the server expects in a workspace edit.
    pub workspace_edit: WorkspaceEditCapabilities,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            position_encoding: "utf-16".to_owned(),
            text_sync: TextSyncCapabilities::default(),
            completion: None,
            hover: false,
            signature_help: None,
            definition: false,
            declaration: false,
            type_definition: false,
            references: false,
            document_symbol: false,
            code_action: None,
            formatting: false,
            range_formatting: false,
            folding_range: false,
            rename: false,
            prepare_rename: false,
            diagnostics: None,
            execute_command: false,
            workspace_edit: WorkspaceEditCapabilities::default(),
        }
    }
}

impl ServerCapabilities {
    /// Parses the `capabilities` object of an `initialize` result. Missing or malformed fields
    /// mean "not offered"; the result is always usable.
    pub fn from_json(value: &Value) -> Self {
        let mut caps = Self::default();
        let Some(map) = value.as_object() else {
            return caps;
        };
        caps.position_encoding = map
            .get("positionEncoding")
            .and_then(Value::as_str)
            .unwrap_or("utf-16")
            .to_owned();
        caps.text_sync = parse_text_sync(map.get("textDocumentSync"));
        caps.completion = map
            .get("completionProvider")
            .map(|provider| CompletionCapabilities {
                trigger_characters: strings(provider.get("triggerCharacters")),
                resolve: boolean(provider.get("resolveProvider")),
                commit_characters: strings(provider.get("allCommitCharacters")),
            });
        caps.hover = map.get("hoverProvider").is_some_and(is_offered);
        caps.signature_help =
            map.get("signatureHelpProvider")
                .map(|provider| SignatureHelpCapabilities {
                    trigger_characters: strings(provider.get("triggerCharacters")),
                    retrigger_characters: strings(provider.get("retriggerCharacters")),
                });
        caps.definition = map.get("definitionProvider").is_some_and(is_offered);
        caps.declaration = map.get("declarationProvider").is_some_and(is_offered);
        caps.type_definition = map.get("typeDefinitionProvider").is_some_and(is_offered);
        caps.references = map.get("referencesProvider").is_some_and(is_offered);
        caps.document_symbol = map.get("documentSymbolProvider").is_some_and(is_offered);
        caps.code_action = map.get("codeActionProvider").and_then(|provider| {
            if provider.is_boolean() && !is_offered(provider) {
                return None;
            }
            Some(CodeActionCapabilities {
                resolve: boolean(provider.get("resolveProvider")),
                kinds: strings(provider.get("codeActionKinds")),
            })
        });
        caps.formatting = map
            .get("documentFormattingProvider")
            .is_some_and(is_offered);
        caps.range_formatting = map
            .get("documentRangeFormattingProvider")
            .is_some_and(is_offered);
        caps.folding_range = map.get("foldingRangeProvider").is_some_and(is_offered);
        caps.rename = map.get("renameProvider").is_some_and(is_offered);
        caps.prepare_rename = match map.get("renameProvider") {
            Some(Value::Object(provider)) => boolean(provider.get("prepareProvider")),
            _ => false,
        };
        caps.diagnostics = map
            .get("diagnosticProvider")
            .map(|provider| DiagnosticCapabilities {
                inter_file_dependencies: boolean(provider.get("interFileDependencies")),
                workspace_diagnostics: boolean(provider.get("workspaceDiagnostics")),
            });
        caps.execute_command = map.get("executeCommandProvider").is_some();
        caps.workspace_edit = map
            .get("workspace")
            .and_then(|workspace| workspace.get("workspaceEdit"))
            .map(|edit| WorkspaceEditCapabilities {
                document_changes: boolean(edit.get("documentChanges")),
                resource_operations: edit
                    .get("resourceOperations")
                    .and_then(Value::as_array)
                    .is_some_and(|operations| !operations.is_empty()),
            })
            .unwrap_or_default();
        caps
    }
}

fn parse_text_sync(value: Option<&Value>) -> TextSyncCapabilities {
    let Some(value) = value else {
        return TextSyncCapabilities {
            kind: TextSyncKind::None,
            open_close: false,
            save: false,
            save_include_text: false,
        };
    };
    if let Some(number) = value.as_u64() {
        return TextSyncCapabilities {
            kind: TextSyncKind::from_lsp(number),
            open_close: true,
            save: false,
            save_include_text: false,
        };
    }
    let mut caps = TextSyncCapabilities {
        kind: value
            .get("change")
            .and_then(Value::as_u64)
            .map(TextSyncKind::from_lsp)
            .unwrap_or(TextSyncKind::None),
        open_close: boolean(value.get("openClose")),
        save: false,
        save_include_text: false,
    };
    match value.get("save") {
        Some(Value::Bool(enabled)) => caps.save = *enabled,
        Some(Value::Object(save)) => {
            caps.save = true;
            caps.save_include_text = boolean(save.get("includeText"));
        }
        _ => {}
    }
    caps
}

fn boolean(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A provider field may be `true` or an options object; both mean offered.
fn is_offered(value: &Value) -> bool {
    match value {
        Value::Bool(offered) => *offered,
        Value::Object(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_rich_server() {
        let caps = ServerCapabilities::from_json(&json!({
            "positionEncoding": "utf-16",
            "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}},
            "completionProvider": {"triggerCharacters": [".", "("], "resolveProvider": true},
            "hoverProvider": true,
            "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
            "definitionProvider": true,
            "declarationProvider": {"workDoneProgress": true},
            "referencesProvider": true,
            "documentSymbolProvider": true,
            "codeActionProvider": {"resolveProvider": true, "codeActionKinds": ["quickfix"]},
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": true,
            "foldingRangeProvider": true,
            "renameProvider": {"prepareProvider": true},
            "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false},
            "executeCommandProvider": {"commands": ["x"]},
            "workspace": {"workspaceEdit": {"documentChanges": true, "resourceOperations": ["create"]}}
        }));
        assert_eq!(caps.text_sync.kind, TextSyncKind::Incremental);
        assert!(caps.text_sync.save && caps.text_sync.save_include_text);
        let completion = caps.completion.as_ref().unwrap();
        assert_eq!(completion.trigger_characters, [".", "("]);
        assert!(completion.resolve);
        assert!(caps.hover && caps.definition && caps.declaration && caps.references);
        assert!(caps.prepare_rename && caps.rename && caps.folding_range);
        assert!(caps.diagnostics.unwrap().inter_file_dependencies);
        assert!(caps.workspace_edit.document_changes);
        assert!(caps.workspace_edit.resource_operations);
        assert_eq!(caps.position_encoding, "utf-16");
    }

    #[test]
    fn absent_fields_mean_not_offered() {
        let caps = ServerCapabilities::from_json(&json!({}));
        assert_eq!(caps.text_sync.kind, TextSyncKind::None);
        assert!(!caps.text_sync.open_close);
        assert!(caps.completion.is_none());
        assert!(!caps.hover);
        assert!(!caps.execute_command);
        assert!(caps.diagnostics.is_none());
        assert_eq!(
            ServerCapabilities::from_json(&json!(null)),
            ServerCapabilities::default()
        );
    }

    #[test]
    fn numbers_and_booleans_for_sync() {
        let caps = ServerCapabilities::from_json(&json!({"textDocumentSync": 1}));
        assert_eq!(caps.text_sync.kind, TextSyncKind::Full);
        assert!(caps.text_sync.open_close);
        let caps = ServerCapabilities::from_json(
            &json!({"textDocumentSync": {"openClose": true, "change": 0}}),
        );
        assert_eq!(caps.text_sync.kind, TextSyncKind::None);
        let caps = ServerCapabilities::from_json(&json!({"textDocumentSync": {"save": true}}));
        assert!(caps.text_sync.save && !caps.text_sync.save_include_text);
        let caps = ServerCapabilities::from_json(
            &json!({"codeActionProvider": false, "hoverProvider": false}),
        );
        assert!(caps.code_action.is_none());
        assert!(!caps.hover);
    }
}
