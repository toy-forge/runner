//! The protocol's values as the editor uses them. Positions are UTF-16 line and character pairs,
//! the protocol's default encoding, and the same shape NSH's editor uses, so the UI layer converts
//! field by field.
//!
//! Everything here is plain data; the parsers that build these from a server's JSON are internal,
//! and requests return these types directly.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::markup::RichText;

/// A position: a line and a UTF-16 character offset within it, both from zero.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Position {
    /// The line.
    pub line: u32,
    /// The character, in UTF-16 code units.
    pub character: u32,
}

impl Position {
    /// A position.
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A range between two positions (end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Range {
    /// Where it starts.
    pub start: Position,
    /// Where it ends.
    pub end: Position,
}

impl Range {
    /// A range from `(start_line, start_character)` to `(end_line, end_character)`.
    pub const fn new(
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    ) -> Self {
        Self {
            start: Position::new(start_line, start_character),
            end: Position::new(end_line, end_character),
        }
    }

    /// An empty range at `position`.
    pub const fn at(position: Position) -> Self {
        Self {
            start: position,
            end: position,
        }
    }

    /// Whether the range covers nothing.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Whether `position` is inside the range (start inclusive, end inclusive for the caret).
    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }
}

/// A place in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// The file's URI. For documents the manager has open this is the document's own URI; server
    /// URIs inside the virtual workspace are translated back by the manager.
    pub uri: String,
    /// The range in the file.
    pub range: Range,
}

/// One text replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    /// What is replaced.
    pub range: Range,
    /// What replaces it.
    pub new_text: String,
}

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something that is wrong.
    Error,
    /// Something that is likely wrong.
    Warning,
    /// Information.
    Information,
    /// A suggestion.
    Hint,
}

impl Severity {
    /// The severity for the protocol's number; [`Severity::Error`] for an unknown one.
    pub fn from_lsp(number: u32) -> Self {
        match number {
            2 => Self::Warning,
            3 => Self::Information,
            4 => Self::Hint,
            _ => Self::Error,
        }
    }

    /// The protocol's number (1 to 4).
    pub fn to_lsp(self) -> u32 {
        match self {
            Self::Error => 1,
            Self::Warning => 2,
            Self::Information => 3,
            Self::Hint => 4,
        }
    }
}

/// A related location of a diagnostic ("declared here").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedInformation {
    /// Where.
    pub location: Location,
    /// What it says.
    pub message: String,
}

/// A problem a server reports for a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Where it is.
    pub range: Range,
    /// How serious it is (`Error` when the server does not say).
    pub severity: Severity,
    /// The message.
    pub message: String,
    /// The tool that reported it.
    pub source: Option<String>,
    /// The tool's code for it.
    pub code: Option<String>,
    /// A link that explains the code.
    pub code_href: Option<String>,
    /// Whether the text is not used (`unnecessary`).
    pub unnecessary: bool,
    /// Whether the text is deprecated.
    pub deprecated: bool,
    /// Other places that belong to it.
    pub related: Vec<RelatedInformation>,
    /// The server's original JSON. Code-action requests send it back, as the protocol requires.
    pub raw: Option<Value>,
}

impl Diagnostic {
    /// A diagnostic with just a range, severity and message.
    pub fn new(range: Range, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            range,
            severity,
            message: message.into(),
            source: None,
            code: None,
            code_href: None,
            unnecessary: false,
            deprecated: false,
            related: Vec::new(),
            raw: None,
        }
    }

    /// The JSON the protocol wants in a `CodeActionContext`.
    pub fn to_json(&self) -> Value {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut value = serde_json::json!({
            "range": self.range,
            "severity": self.severity.to_lsp(),
            "message": self.message,
        });
        if let Some(source) = &self.source {
            value["source"] = Value::String(source.clone());
        }
        if let Some(code) = &self.code {
            value["code"] = Value::String(code.clone());
        }
        value
    }
}

macro_rules! lsp_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $number:expr,)+ }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $(
                #[doc = concat!("The protocol's ", stringify!($variant), " (", stringify!($number), ").")]
                $variant,
            )+
        }

        impl $name {
            /// The kind for the protocol's number, if it is a known one.
            pub fn from_lsp(number: u32) -> Option<Self> {
                match number {
                    $($number => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The protocol's number.
            pub fn to_lsp(self) -> u32 {
                match self {
                    $(Self::$variant => $number,)+
                }
            }
        }
    };
}

lsp_enum! {
    /// The kind of a completion item (`CompletionItemKind`).
    CompletionKind {
        Text = 1, Method = 2, Function = 3, Constructor = 4, Field = 5, Variable = 6, Class = 7,
        Interface = 8, Module = 9, Property = 10, Unit = 11, Value = 12, Enum = 13, Keyword = 14,
        Snippet = 15, Color = 16, File = 17, Reference = 18, Folder = 19, EnumMember = 20,
        Constant = 21, Struct = 22, Event = 23, Operator = 24, TypeParameter = 25,
    }
}

lsp_enum! {
    /// The kind of a symbol (`SymbolKind`).
    SymbolKind {
        File = 1, Module = 2, Namespace = 3, Package = 4, Class = 5, Method = 6, Property = 7,
        Field = 8, Constructor = 9, Enum = 10, Interface = 11, Function = 12, Variable = 13,
        Constant = 14, String = 15, Number = 16, Boolean = 17, Array = 18, Object = 19, Key = 20,
        Null = 21, EnumMember = 22, Struct = 23, Event = 24, Operator = 25, TypeParameter = 26,
    }
}

/// Whether a completion's text is plain or an LSP snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsertFormat {
    /// Insert as it is.
    #[default]
    PlainText,
    /// Expand with [`crate::expand_snippet`] first.
    Snippet,
}

/// Where a completion item came from. Lets the UI merge lists and rank server answers above
/// guesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompletionSource {
    /// A language server.
    #[default]
    Server,
    /// A word already in the document.
    Word,
    /// A keyword or snippet from the language's static table.
    Static,
    /// A file path.
    Path,
    /// A shell command, variable or file (from `compgen`).
    Shell,
}

/// A command a server asks the client to run through `workspace/executeCommand`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// What to show.
    pub title: String,
    /// The command's identifier.
    pub command: String,
    /// Its arguments.
    #[serde(default)]
    pub arguments: Vec<Value>,
}

/// One completion.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompletionItem {
    /// What the list shows.
    pub label: String,
    /// A signature or qualifier shown right after the label (`labelDetails.detail`).
    pub label_detail: Option<String>,
    /// Where it comes from, such as a module path (`labelDetails.description`).
    pub label_description: Option<String>,
    /// The kind.
    pub kind: Option<CompletionKind>,
    /// More about it, such as a signature or a type.
    pub detail: Option<String>,
    /// Its documentation.
    pub documentation: Option<RichText>,
    /// Whether it is deprecated.
    pub deprecated: bool,
    /// Whether the server wants it selected first.
    pub preselect: bool,
    /// The order among equal matches, when not the label.
    pub sort_text: Option<String>,
    /// What typed text is matched against, when not the label.
    pub filter_text: Option<String>,
    /// What accepting it inserts, when not the label (and no `text_edit`).
    pub insert_text: Option<String>,
    /// Whether the inserted text is a snippet.
    pub insert_format: InsertFormat,
    /// The edit accepting it makes: the range to replace and the text. For servers that offer
    /// separate insert and replace ranges this holds the insert range.
    pub text_edit: Option<TextEdit>,
    /// The range to replace instead when the user chooses "replace", if the server gave both.
    pub replace_range: Option<Range>,
    /// Edits elsewhere (such as adding an import) that accepting it also makes.
    pub additional_edits: Vec<TextEdit>,
    /// Characters that accept the item when typed.
    pub commit_characters: Vec<String>,
    /// A command to run after accepting it.
    pub command: Option<Command>,
    /// Where the item came from.
    pub source: CompletionSource,
    /// The server's original JSON, sent back for `completionItem/resolve`.
    pub raw: Option<Value>,
}

impl CompletionItem {
    /// A plain item from a fallback provider.
    pub fn simple(
        label: impl Into<String>,
        kind: CompletionKind,
        source: CompletionSource,
    ) -> Self {
        Self {
            label: label.into(),
            kind: Some(kind),
            source,
            ..Self::default()
        }
    }

    /// The text matched against what the user typed.
    pub fn match_text(&self) -> &str {
        self.filter_text.as_deref().unwrap_or(&self.label)
    }

    /// The text the list sorts equal matches by.
    pub fn sort_key(&self) -> &str {
        self.sort_text.as_deref().unwrap_or(&self.label)
    }
}

/// The completions for one request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompletionList {
    /// The items.
    pub items: Vec<CompletionItem>,
    /// Whether the server would give more if the text were typed on (ask again then).
    pub is_incomplete: bool,
}

/// What asked for a completion, as the protocol names it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CompletionTrigger {
    /// The user invoked it (Esc, Ctrl-Space) or a word was typed.
    #[default]
    Invoked,
    /// A trigger character such as `.` was typed.
    Character(String),
    /// The previous list was incomplete and is being refreshed.
    Incomplete,
}

/// What a hover shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hover {
    /// The contents, as structured text.
    pub contents: RichText,
    /// The range the hover is about, when the server gives one.
    pub range: Option<Range>,
}

/// One parameter of a signature.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParameterInfo {
    /// The parameter's text, such as `url: str`.
    pub label: String,
    /// Where `label` sits inside the signature's label, in UTF-16 units `[start, end)`, so the UI
    /// can highlight it. Computed by searching the label when the server gave a string.
    pub label_range: Option<(u32, u32)>,
    /// Its documentation.
    pub documentation: Option<RichText>,
}

/// One signature of a function.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignatureInfo {
    /// The signature, such as `get(url, params=None)`.
    pub label: String,
    /// Its documentation.
    pub documentation: Option<RichText>,
    /// The parameters, in order.
    pub parameters: Vec<ParameterInfo>,
    /// The signature's own active parameter, which overrides the list's.
    pub active_parameter: Option<usize>,
}

/// The signatures at a call, and which one and parameter are current.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignatureHelp {
    /// The signatures.
    pub signatures: Vec<SignatureInfo>,
    /// The index of the current signature.
    pub active_signature: usize,
    /// The index of the current parameter.
    pub active_parameter: Option<usize>,
}

impl SignatureHelp {
    /// The index of the highlighted parameter of the active signature.
    pub fn current_parameter(&self) -> Option<usize> {
        let signature = self.signatures.get(self.active_signature)?;
        signature.active_parameter.or(self.active_parameter)
    }
}

/// The answer to a definition-like request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Definition {
    /// The range of the symbol the request was made on, when the server says (for the underline).
    pub origin: Option<Range>,
    /// Where it is defined. Several for overloads and ambiguous symbols.
    pub targets: Vec<Location>,
}

/// A symbol of a document, with its children.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSymbol {
    /// The name.
    pub name: String,
    /// More about it, such as a signature.
    pub detail: Option<String>,
    /// The kind.
    pub kind: SymbolKind,
    /// The whole symbol.
    pub range: Range,
    /// Its name.
    pub selection_range: Range,
    /// Whether it is deprecated.
    pub deprecated: bool,
    /// Nested symbols.
    pub children: Vec<DocumentSymbol>,
}

/// An action offered for a place or a problem.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodeAction {
    /// What to show.
    pub title: String,
    /// The kind, such as `quickfix` or `refactor.extract`.
    pub kind: Option<String>,
    /// The problems it fixes.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether it is the best fix for its problem.
    pub is_preferred: bool,
    /// Why it cannot be applied, when it is offered disabled.
    pub disabled_reason: Option<String>,
    /// The edit it makes, if the server resolved one already.
    pub edit: Option<WorkspaceEdit>,
    /// The command it runs afterwards.
    pub command: Option<Command>,
    /// The server's original JSON, sent back for `codeAction/resolve`.
    pub raw: Option<Value>,
}

/// One step of a workspace edit.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceChange {
    /// Text edits to one file.
    Edit(FileEdit),
    /// Create a file.
    Create {
        /// The file.
        uri: String,
        /// Replace it if it exists.
        overwrite: bool,
        /// Do nothing if it exists.
        ignore_if_exists: bool,
    },
    /// Move a file.
    Rename {
        /// From.
        old_uri: String,
        /// To.
        new_uri: String,
        /// Replace the target if it exists.
        overwrite: bool,
    },
    /// Delete a file or folder.
    Delete {
        /// What to delete.
        uri: String,
        /// Delete folders with their contents.
        recursive: bool,
    },
}

/// The edits to one file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    /// The file.
    pub uri: String,
    /// The version of the document the edits were computed for, if the server says.
    pub version: Option<i32>,
    /// The edits, all relative to the same text.
    pub edits: Vec<TextEdit>,
}

/// A set of changes to several files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceEdit {
    /// The steps, in order.
    pub changes: Vec<WorkspaceChange>,
}

impl WorkspaceEdit {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// What a folding range is about (`FoldingRangeKind`). Unknown kinds are `None` in the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldKind {
    /// A comment.
    Comment,
    /// An import block.
    Imports,
    /// A `#region`-style marker.
    Region,
}

impl FoldKind {
    /// The kind for one of the protocol's strings, if it is a known one.
    pub fn from_lsp(name: &str) -> Option<Self> {
        match name {
            "comment" => Some(Self::Comment),
            "imports" => Some(Self::Imports),
            "region" => Some(Self::Region),
            _ => None,
        }
    }

    /// The protocol's string.
    pub fn to_lsp(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Imports => "imports",
            Self::Region => "region",
        }
    }
}

/// A range the editor can fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldingRange {
    /// The first line (the one that stays visible).
    pub start_line: u32,
    /// The last line.
    pub end_line: u32,
    /// The character the fold starts at on the first line, when the server says.
    pub start_character: Option<u32>,
    /// The character the fold ends at on the last line, when the server says.
    pub end_character: Option<u32>,
    /// What it is.
    pub kind: Option<FoldKind>,
    /// The text to show for the folded part, when the server says.
    pub collapsed_text: Option<String>,
}

/// The answer to `prepareRename`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareRename {
    /// The range of the name.
    pub range: Range,
    /// The text to offer in the rename field, when the server says; otherwise the text in `range`.
    pub placeholder: Option<String>,
}

/// Options for formatting requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormattingOptions {
    /// The width of a tab, or the number of spaces for one indent level.
    pub tab_size: u32,
    /// Whether indentation uses spaces.
    pub insert_spaces: bool,
    /// Whether to trim trailing whitespace.
    pub trim_trailing_whitespace: bool,
    /// Whether the file ends with a newline.
    pub insert_final_newline: bool,
}

impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
        }
    }
}

impl FormattingOptions {
    pub(crate) fn to_json(self) -> Value {
        serde_json::json!({
            "tabSize": self.tab_size,
            "insertSpaces": self.insert_spaces,
            "trimTrailingWhitespace": self.trim_trailing_whitespace,
            "insertFinalNewline": self.insert_final_newline,
        })
    }
}

/// The answer to a pull-diagnostics request (`textDocument/diagnostic`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiagnosticReport {
    /// The server's identifier for this report, sent back on the next request so an unchanged
    /// report can be answered cheaply.
    pub result_id: Option<String>,
    /// The problems.
    pub diagnostics: Vec<Diagnostic>,
    /// The server answered "unchanged" instead of sending items.
    pub unchanged: bool,
}
