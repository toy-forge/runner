//! Parsers from the protocol's JSON to the typed values in [`crate::types`].
//!
//! Servers are lenient about optional fields and sometimes return older shapes (`Location` vs
//! `LocationLink`, `DocumentSymbol` vs `SymbolInformation`, a bare array vs a completion list);
//! these parsers accept every shape the protocol allows and skip entries that are unusable
//! rather than failing the whole answer.

use serde_json::Value;

use crate::error::{LspError, Result};
use crate::markup::from_markup_json;
use crate::text::utf16_len;
use crate::types::{
    CodeAction, Command, CompletionItem, CompletionKind, CompletionList, CompletionSource,
    Definition, Diagnostic, DiagnosticReport, DocumentSymbol, FileEdit, FoldKind, FoldingRange,
    Hover, InsertFormat, Location, ParameterInfo, PrepareRename, Range, RelatedInformation,
    Severity, SignatureHelp, SignatureInfo, SymbolKind, TextEdit, WorkspaceChange, WorkspaceEdit,
};

/// Parses the `diagnostics` array of `textDocument/publishDiagnostics` and pull reports.
pub fn parse_diagnostics(value: &Value) -> Vec<Diagnostic> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_diagnostic).collect())
        .unwrap_or_default()
}

/// Parses one diagnostic, keeping its original JSON for a code-action context.
pub fn parse_diagnostic(value: &Value) -> Option<Diagnostic> {
    let range = parse_range(value.get("range")?)?;
    let message = value.get("message").and_then(Value::as_str)?.to_owned();
    let mut diagnostic = Diagnostic::new(
        range,
        Severity::from_lsp(value.get("severity").and_then(Value::as_u64).unwrap_or(1) as u32),
        message,
    );
    diagnostic.source = value
        .get("source")
        .and_then(Value::as_str)
        .map(str::to_owned);
    diagnostic.code = match value.get("code") {
        Some(Value::String(code)) => Some(code.clone()),
        Some(Value::Number(code)) => Some(code.to_string()),
        _ => None,
    };
    diagnostic.code_href = value
        .pointer("/codeDescription/href")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(tags) = value.get("tags").and_then(Value::as_array) {
        for tag in tags.iter().filter_map(Value::as_u64) {
            match tag {
                1 => diagnostic.unnecessary = true,
                2 => diagnostic.deprecated = true,
                _ => {}
            }
        }
    }
    diagnostic.related = value
        .get("relatedInformation")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(RelatedInformation {
                        location: parse_location(item.get("location")?)?,
                        message: item.get("message")?.as_str()?.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    diagnostic.raw = Some(value.clone());
    Some(diagnostic)
}

/// Parses a completion answer: `null`, a bare item array, or a `CompletionList`.
pub fn parse_completion_list(value: &Value) -> CompletionList {
    if let Some(items) = value.as_array() {
        return CompletionList {
            items: items.iter().filter_map(parse_completion_item).collect(),
            is_incomplete: false,
        };
    }
    if value.is_object() {
        return CompletionList {
            items: value
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(parse_completion_item).collect())
                .unwrap_or_default(),
            is_incomplete: value
                .get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
    }
    CompletionList::default()
}

/// Parses one completion item.
pub fn parse_completion_item(value: &Value) -> Option<CompletionItem> {
    let label = value.get("label").and_then(Value::as_str)?.to_owned();
    let mut item = CompletionItem {
        label,
        ..CompletionItem::default()
    };
    item.source = CompletionSource::Server;
    item.kind = value
        .get("kind")
        .and_then(Value::as_u64)
        .and_then(|kind| CompletionKind::from_lsp(kind as u32));
    item.detail = value
        .get("detail")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.label_detail = value
        .pointer("/labelDetails/detail")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.label_description = value
        .pointer("/labelDetails/description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.documentation = value.get("documentation").and_then(from_markup_json);
    item.deprecated = value
        .get("deprecated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|tag| tag.as_u64() == Some(1)));
    item.preselect = value
        .get("preselect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    item.sort_text = value
        .get("sortText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.filter_text = value
        .get("filterText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.insert_text = value
        .get("insertText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.insert_format = match value.get("insertTextFormat").and_then(Value::as_u64) {
        Some(2) => InsertFormat::Snippet,
        _ => InsertFormat::PlainText,
    };
    if let Some(edit) = value.get("textEdit")
        && let Some(new_text) = edit.get("newText").and_then(Value::as_str)
    {
        if let Some(range) = edit.get("range").and_then(parse_range) {
            item.text_edit = Some(TextEdit {
                range,
                new_text: new_text.to_owned(),
            });
        } else if let Some(insert) = edit.get("insert").and_then(parse_range) {
            item.text_edit = Some(TextEdit {
                range: insert,
                new_text: new_text.to_owned(),
            });
            item.replace_range = edit.get("replace").and_then(parse_range);
        }
    }
    item.additional_edits = value
        .get("additionalTextEdits")
        .map(parse_text_edits)
        .unwrap_or_default();
    item.commit_characters = string_list(value.get("commitCharacters"));
    item.command = value.get("command").and_then(parse_command);
    item.raw = Some(value.clone());
    Some(item)
}

/// Parses a `TextEdit[]`.
pub fn parse_text_edits(value: &Value) -> Vec<TextEdit> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_text_edit).collect())
        .unwrap_or_default()
}

/// Parses one text edit. An annotated `AnnotatedTextEdit` is the same shape plus an id.
pub fn parse_text_edit(value: &Value) -> Option<TextEdit> {
    Some(TextEdit {
        range: parse_range(value.get("range")?)?,
        new_text: value.get("newText")?.as_str()?.to_owned(),
    })
}

/// Parses a hover answer.
pub fn parse_hover(value: &Value) -> Option<Hover> {
    if !value.is_object() {
        return None;
    }
    let contents = from_markup_json(value.get("contents")?)?;
    Some(Hover {
        contents,
        range: value.get("range").and_then(parse_range),
    })
}

/// Parses a signature-help answer.
pub fn parse_signature_help(value: &Value) -> Option<SignatureHelp> {
    let signatures = value.get("signatures")?.as_array()?;
    let signatures: Vec<SignatureInfo> = signatures.iter().filter_map(parse_signature).collect();
    if signatures.is_empty() {
        return None;
    }
    Some(SignatureHelp {
        signatures,
        active_signature: value
            .get("activeSignature")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        active_parameter: value
            .get("activeParameter")
            .and_then(Value::as_u64)
            .map(|index| index as usize),
    })
}

fn parse_signature(value: &Value) -> Option<SignatureInfo> {
    let label = value.get("label")?.as_str()?.to_owned();
    let mut signature = SignatureInfo {
        documentation: value.get("documentation").and_then(from_markup_json),
        active_parameter: value
            .get("activeParameter")
            .and_then(Value::as_u64)
            .map(|index| index as usize),
        label,
        parameters: Vec::new(),
    };
    if let Some(parameters) = value.get("parameters").and_then(Value::as_array) {
        for parameter in parameters {
            let Some(label) = parameter.get("label") else {
                continue;
            };
            let (text, label_range) = match label {
                Value::String(text) => (
                    text.clone(),
                    find_utf16(&signature.label, text)
                        .map(|start| (start, start + utf16_len(text) as u32)),
                ),
                Value::Array(offsets) => {
                    let start = offsets.first().and_then(Value::as_u64).unwrap_or(0) as u32;
                    let end = offsets.get(1).and_then(Value::as_u64).unwrap_or(0) as u32;
                    let text = utf16_slice(&signature.label, start, end);
                    (text, Some((start, end)))
                }
                _ => continue,
            };
            signature.parameters.push(ParameterInfo {
                label: text,
                label_range,
                documentation: parameter.get("documentation").and_then(from_markup_json),
            });
        }
    }
    Some(signature)
}

fn find_utf16(haystack: &str, needle: &str) -> Option<u32> {
    haystack
        .find(needle)
        .map(|byte| utf16_len(&haystack[..byte]) as u32)
}

/// The substring of `text` between two UTF-16 offsets, clamped to its length.
fn utf16_slice(text: &str, start: u32, end: u32) -> String {
    use crate::text::byte_offset_of_utf16;
    let start = byte_offset_of_utf16(text, start);
    let end = byte_offset_of_utf16(text, end).max(start);
    text[start..end].to_owned()
}

/// Parses a definition-like answer: `Location`, `LocationLink`, or an array of either.
pub fn parse_definition(value: &Value) -> Definition {
    let mut definition = Definition::default();
    match value {
        Value::Null => {}
        Value::Array(items) => {
            for item in items {
                merge_location(item, &mut definition);
            }
        }
        other => merge_location(other, &mut definition),
    }
    definition
}

fn merge_location(value: &Value, definition: &mut Definition) {
    if let Some(location) = parse_location(value) {
        definition.targets.push(location);
        return;
    }
    if let Some(target_uri) = value.get("targetUri").and_then(Value::as_str) {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"))
            .and_then(parse_range);
        if let Some(range) = range {
            definition.targets.push(Location {
                uri: target_uri.to_owned(),
                range,
            });
        }
        if definition.origin.is_none() {
            definition.origin = value.get("originSelectionRange").and_then(parse_range);
        }
    }
}

/// Parses a `Location[]` answer (references, workspace symbols).
pub fn parse_locations(value: &Value) -> Vec<Location> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_location).collect())
        .unwrap_or_default()
}

/// Parses a location, or a `LocationLink`'s target.
pub fn parse_location(value: &Value) -> Option<Location> {
    let uri = value.get("uri")?.as_str()?.to_owned();
    let range = parse_range(value.get("range")?)?;
    Some(Location { uri, range })
}

/// Parses a range.
pub fn parse_range(value: &Value) -> Option<Range> {
    serde_json::from_value(value.clone()).ok()
}

/// Parses a `DocumentSymbol[]` or `SymbolInformation[]` answer into one tree.
pub fn parse_document_symbols(value: &Value) -> Vec<DocumentSymbol> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_document_symbol).collect())
        .unwrap_or_default()
}

fn parse_document_symbol(value: &Value) -> Option<DocumentSymbol> {
    let name = value.get("name")?.as_str()?.to_owned();
    let kind = value
        .get("kind")
        .and_then(Value::as_u64)
        .and_then(|kind| SymbolKind::from_lsp(kind as u32))
        .unwrap_or(SymbolKind::File);
    if let Some(location) = value.get("location") {
        let range = parse_range(location.get("range")?)?;
        return Some(DocumentSymbol {
            name,
            detail: value
                .pointer("/containerName")
                .and_then(Value::as_str)
                .map(str::to_owned),
            kind,
            range,
            selection_range: range,
            deprecated: value
                .get("deprecated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            children: Vec::new(),
        });
    }
    Some(DocumentSymbol {
        name,
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_owned),
        kind,
        range: parse_range(value.get("range")?)?,
        selection_range: parse_range(value.get("selectionRange")?)?,
        deprecated: value
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        children: value
            .get("children")
            .and_then(Value::as_array)
            .map(|children| children.iter().filter_map(parse_document_symbol).collect())
            .unwrap_or_default(),
    })
}

/// Parses a code-action answer.
pub fn parse_code_actions(value: &Value) -> Vec<CodeAction> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_code_action).collect())
        .unwrap_or_default()
}

/// Parses one code action (or the answer to `codeAction/resolve`).
pub fn parse_code_action(value: &Value) -> Option<CodeAction> {
    let title = value.get("title")?.as_str()?.to_owned();
    let mut action = CodeAction {
        title,
        kind: value.get("kind").and_then(Value::as_str).map(str::to_owned),
        diagnostics: value
            .get("diagnostics")
            .map(parse_diagnostics)
            .unwrap_or_default(),
        is_preferred: value
            .get("isPreferred")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        disabled_reason: value
            .pointer("/disabled/reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        edit: value
            .get("edit")
            .and_then(|edit| parse_workspace_edit(edit).ok()),
        command: value.get("command").and_then(parse_command),
        raw: Some(value.clone()),
    };
    if action.edit.as_ref().is_some_and(WorkspaceEdit::is_empty) {
        action.edit = None;
    }
    Some(action)
}

/// Parses a command.
pub fn parse_command(value: &Value) -> Option<Command> {
    Some(Command {
        title: value.get("title")?.as_str()?.to_owned(),
        command: value.get("command")?.as_str()?.to_owned(),
        arguments: value
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Parses a `WorkspaceEdit`.
pub fn parse_workspace_edit(value: &Value) -> Result<WorkspaceEdit> {
    let mut edit = WorkspaceEdit::default();
    if let Some(document_changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if let Some(file) = parse_text_document_edit(change) {
                edit.changes.push(WorkspaceChange::Edit(file));
                continue;
            }
            let Some(kind) = change.get("kind").and_then(Value::as_str) else {
                return Err(LspError::Protocol(
                    "a workspace edit change has no kind".to_owned(),
                ));
            };
            match kind {
                "create" => {
                    edit.changes.push(WorkspaceChange::Create {
                        uri: change
                            .get("uri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| LspError::Protocol("create without uri".to_owned()))?
                            .to_owned(),
                        overwrite: change
                            .pointer("/options/overwrite")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        ignore_if_exists: change
                            .pointer("/options/ignoreIfExists")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
                "rename" => {
                    edit.changes.push(WorkspaceChange::Rename {
                        old_uri: change
                            .get("oldUri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| LspError::Protocol("rename without oldUri".to_owned()))?
                            .to_owned(),
                        new_uri: change
                            .get("newUri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| LspError::Protocol("rename without newUri".to_owned()))?
                            .to_owned(),
                        overwrite: change
                            .pointer("/options/overwrite")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
                "delete" => {
                    edit.changes.push(WorkspaceChange::Delete {
                        uri: change
                            .get("uri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| LspError::Protocol("delete without uri".to_owned()))?
                            .to_owned(),
                        recursive: change
                            .pointer("/options/recursive")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
                other => {
                    return Err(LspError::Protocol(format!(
                        "unknown workspace edit change {other}"
                    )));
                }
            }
        }
    }
    // `documentChanges` wins when a server sends both shapes (as the protocol says).
    if edit.changes.is_empty()
        && let Some(changes) = value.get("changes").and_then(Value::as_object)
    {
        for (uri, edits) in changes {
            edit.changes.push(WorkspaceChange::Edit(FileEdit {
                uri: uri.clone(),
                version: None,
                edits: parse_text_edits(edits),
            }));
        }
    }
    Ok(edit)
}

fn parse_text_document_edit(value: &Value) -> Option<FileEdit> {
    let uri = value.pointer("/textDocument/uri")?.as_str()?.to_owned();
    Some(FileEdit {
        uri,
        version: value
            .pointer("/textDocument/version")
            .and_then(Value::as_i64)
            .map(|version| version as i32),
        edits: value.get("edits").map(parse_text_edits).unwrap_or_default(),
    })
}

/// Parses a folding-range answer.
pub fn parse_folding_ranges(value: &Value) -> Vec<FoldingRange> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(FoldingRange {
                        start_line: item.get("startLine")?.as_u64()? as u32,
                        end_line: item.get("endLine")?.as_u64()? as u32,
                        start_character: item
                            .get("startCharacter")
                            .and_then(Value::as_u64)
                            .map(|value| value as u32),
                        end_character: item
                            .get("endCharacter")
                            .and_then(Value::as_u64)
                            .map(|value| value as u32),
                        kind: item
                            .get("kind")
                            .and_then(Value::as_str)
                            .and_then(FoldKind::from_lsp),
                        collapsed_text: item
                            .get("collapsedText")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses a `prepareRename` answer: `null`, a bare range, or `{range, placeholder}`.
pub fn parse_prepare_rename(value: &Value) -> Option<PrepareRename> {
    if value.is_null() {
        return None;
    }
    if let Some(range) = parse_range(value) {
        return Some(PrepareRename {
            range,
            placeholder: None,
        });
    }
    Some(PrepareRename {
        range: parse_range(value.get("range")?)?,
        placeholder: value
            .get("placeholder")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Parses a pull-diagnostics report.
pub fn parse_diagnostic_report(value: &Value) -> Result<DiagnosticReport> {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("full");
    let result_id = value
        .get("resultId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match kind {
        "unchanged" => Ok(DiagnosticReport {
            result_id,
            diagnostics: Vec::new(),
            unchanged: true,
        }),
        "full" | "partial" => Ok(DiagnosticReport {
            result_id,
            diagnostics: value
                .get("items")
                .map(parse_diagnostics)
                .unwrap_or_default(),
            unchanged: false,
        }),
        other => Err(LspError::Protocol(format!(
            "unknown diagnostic report kind {other}"
        ))),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::Block;
    use crate::types::Position;
    use serde_json::json;

    #[test]
    fn diagnostics_with_related_information_and_tags() {
        let diagnostics = parse_diagnostics(&json!([{
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
            "severity": 2,
            "message": "unused",
            "source": "pyright",
            "code": 123,
            "codeDescription": {"href": "https://example.com/rule"},
            "tags": [1, 2],
            "relatedInformation": [{
                "location": {"uri": "file:///tmp/x.py", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}},
                "message": "declared here"
            }]
        }]));
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.code.as_deref(), Some("123"));
        assert!(diagnostic.unnecessary && diagnostic.deprecated);
        assert_eq!(diagnostic.related[0].message, "declared here");
        assert!(diagnostic.to_json()["range"].is_object());
        assert!(parse_diagnostics(&json!(null)).is_empty());
    }

    #[test]
    fn completion_list_shapes_and_snippet_edits() {
        let bare = parse_completion_list(&json!([{
            "label": "print",
            "kind": 3,
            "labelDetails": {"detail": "(x)", "description": "builtins"},
            "documentation": {"kind": "markdown", "value": "Prints **x**."},
            "insertText": "print(${1:x})",
            "insertTextFormat": 2,
            "textEdit": {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "print()"},
            "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "import "}],
            "commitCharacters": ["("],
            "command": {"title": "t", "command": "c", "arguments": [1]},
            "deprecated": true
        }]));
        assert_eq!(bare.items.len(), 1);
        let item = &bare.items[0];
        assert_eq!(item.kind, Some(CompletionKind::Function));
        assert_eq!(item.label_detail.as_deref(), Some("(x)"));
        assert_eq!(item.insert_format, InsertFormat::Snippet);
        assert_eq!(item.text_edit.as_ref().unwrap().new_text, "print()");
        assert_eq!(item.additional_edits.len(), 1);
        assert_eq!(item.command.as_ref().unwrap().arguments, [json!(1)]);
        assert!(item.deprecated);

        let list = parse_completion_list(&json!({"isIncomplete": true, "items": [{"label": "x"}]}));
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 1);
        assert!(parse_completion_list(&Value::Null).items.is_empty());
    }

    #[test]
    fn insert_replace_edit() {
        let item = parse_completion_item(&json!({
            "label": "x",
            "textEdit": {
                "newText": "x",
                "insert": {"start": {"line": 2, "character": 3}, "end": {"line": 2, "character": 4}},
                "replace": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 4}}
            }
        }))
        .unwrap();
        assert_eq!(item.text_edit.unwrap().range.start, Position::new(2, 3));
        assert_eq!(item.replace_range.unwrap().start, Position::new(2, 0));
    }

    #[test]
    fn hover_and_signatures() {
        let hover = parse_hover(&json!({
            "contents": {"kind": "markdown", "value": "```python\nprint(x)\n```\n\nPrints [x](https://x.y)."},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}
        }))
        .unwrap();
        assert!(matches!(hover.contents.blocks[0], Block::CodeBlock { .. }));
        assert_eq!(hover.contents.links()[0].0, "x");
        assert!(hover.range.is_some());
        assert!(parse_hover(&json!(null)).is_none());

        let help = parse_signature_help(&json!({
            "signatures": [{
                "label": "print(value, sep=' ')",
                "documentation": "prints",
                "parameters": [
                    {"label": [6, 11]},
                    {"label": "sep=' '"}
                ]
            }],
            "activeSignature": 0,
            "activeParameter": 1
        }))
        .unwrap();
        assert_eq!(help.signatures[0].label, "print(value, sep=' ')");
        assert_eq!(help.signatures[0].parameters[0].label_range, Some((6, 11)));
        assert_eq!(help.signatures[0].parameters[0].label, "value");
        assert_eq!(help.signatures[0].parameters[1].label, "sep=' '");
        assert_eq!(help.current_parameter(), Some(1));
        assert!(parse_signature_help(&json!({"signatures": []})).is_none());
    }

    #[test]
    fn definitions_from_locations_and_links() {
        let definition = parse_definition(&json!([
            {"uri": "file:///a.py", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}},
            {
                "targetUri": "file:///b.py",
                "targetRange": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 9}},
                "targetSelectionRange": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 7}},
                "originSelectionRange": {"start": {"line": 0, "character": 5}, "end": {"line": 0, "character": 6}}
            }
        ]));
        assert_eq!(definition.targets.len(), 2);
        assert_eq!(definition.targets[1].uri, "file:///b.py");
        assert_eq!(definition.targets[1].range.start, Position::new(3, 4));
        assert_eq!(definition.origin.unwrap().start, Position::new(0, 5));
        assert!(parse_definition(&Value::Null).targets.is_empty());
    }

    #[test]
    fn document_symbols_both_shapes() {
        let symbols = parse_document_symbols(&json!([
            {
                "name": "main",
                "detail": "() -> None",
                "kind": 12,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 4, "character": 0}},
                "selectionRange": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 8}},
                "children": [{
                    "name": "x",
                    "kind": 13,
                    "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}},
                    "selectionRange": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}
                }]
            },
            {
                "name": "Old",
                "kind": 5,
                "location": {"uri": "file:///x.py", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}}}
            }
        ]));
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].children[0].name, "x");
        assert_eq!(symbols[0].selection_range.start, Position::new(0, 4));
        assert_eq!(symbols[1].range.start, Position::new(0, 0));
        assert_eq!(symbols[1].kind, SymbolKind::Class);
    }

    #[test]
    fn code_actions_and_workspace_edits() {
        let actions = parse_code_actions(&json!([{
            "title": "Fix it",
            "kind": "quickfix",
            "isPreferred": true,
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "message": "bad"
            }],
            "edit": {
                "documentChanges": [
                    {
                        "textDocument": {"uri": "file:///a.py", "version": 3},
                        "edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "good"}]
                    },
                    {"kind": "create", "uri": "file:///new.py", "options": {"overwrite": true}},
                    {"kind": "rename", "oldUri": "file:///a.py", "newUri": "file:///b.py"},
                    {"kind": "delete", "uri": "file:///gone.py", "options": {"recursive": true}}
                ]
            },
            "data": {"id": 1}
        }, {
            "title": "By command",
            "command": {"title": "Run", "command": "x.run"}
        }]));
        assert_eq!(actions.len(), 2);
        let action = &actions[0];
        assert!(action.is_preferred);
        assert_eq!(action.diagnostics.len(), 1);
        let edit = action.edit.as_ref().unwrap();
        assert_eq!(edit.changes.len(), 4);
        match &edit.changes[0] {
            WorkspaceChange::Edit(file) => {
                assert_eq!(file.version, Some(3));
                assert_eq!(file.edits[0].new_text, "good");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            edit.changes[1],
            WorkspaceChange::Create {
                overwrite: true,
                ..
            }
        ));
        assert!(matches!(
            edit.changes[3],
            WorkspaceChange::Delete {
                recursive: true,
                ..
            }
        ));
        assert_eq!(actions[1].command.as_ref().unwrap().command, "x.run");

        let legacy = parse_workspace_edit(&json!({"changes": {"file:///a": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}
        ]}}))
        .unwrap();
        assert_eq!(legacy.changes.len(), 1);
        assert!(parse_workspace_edit(&json!({"documentChanges": [{"kind": "explode"}]})).is_err());
    }

    #[test]
    fn folding_and_rename() {
        let ranges = parse_folding_ranges(&json!([
            {"startLine": 2, "endLine": 8, "kind": "comment"},
            {"startLine": 10, "endLine": 12, "startCharacter": 4, "collapsedText": "..."},
            {"startLine": 20, "endLine": 21, "kind": "custom"}
        ]));
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].kind, Some(FoldKind::Comment));
        assert_eq!(ranges[1].start_character, Some(4));
        assert_eq!(ranges[2].kind, None);
        assert_eq!(FoldKind::Imports.to_lsp(), "imports");

        let range =
            json!({"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}});
        assert_eq!(
            parse_prepare_rename(&range).unwrap().range.start,
            Position::new(1, 2)
        );
        let prepared =
            parse_prepare_rename(&json!({"range": range, "placeholder": "name"})).unwrap();
        assert_eq!(prepared.placeholder.as_deref(), Some("name"));
        assert!(parse_prepare_rename(&Value::Null).is_none());
    }

    #[test]
    fn diagnostic_reports() {
        let full = parse_diagnostic_report(&json!({
            "kind": "full",
            "resultId": "7",
            "items": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "x"}]
        }))
        .unwrap();
        assert_eq!(full.result_id.as_deref(), Some("7"));
        assert_eq!(full.diagnostics.len(), 1);
        assert!(!full.unchanged);
        let unchanged =
            parse_diagnostic_report(&json!({"kind": "unchanged", "resultId": "8"})).unwrap();
        assert!(unchanged.unchanged && unchanged.diagnostics.is_empty());
        assert!(parse_diagnostic_report(&json!({"kind": "weird"})).is_err());
    }
}
