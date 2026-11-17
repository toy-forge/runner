//! The raw diagnostic model produced by the output parsers.
//!
//! A [`Diagnostic`] is what a parser reads out of text: it has a
//! [`Location`] in the parser's own terms (line/column, character offsets,
//! ...). It becomes an [`crate::Annotation`] (byte ranges in one buffer) with
//! [`crate::resolve`].

use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How serious a diagnostic is. Ordered: `Notice < Warning < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational (`note:`, `info`, `remark:`).
    Notice,
    /// Something suspicious (`warning:`).
    Warning,
    /// A real problem (`error:`).
    Error,
}

impl Severity {
    /// Parses CodeRunner's type strings (`error`, `warning`, `notice`,
    /// `note`, `info`, `runtime_error`, `runtime_warning`, ...) into a
    /// severity and the "runtime" flag.
    pub fn parse_type(text: &str) -> Option<(Severity, bool)> {
        let t = text.trim().to_ascii_lowercase();
        let (runtime, base) = match t.strip_prefix("runtime_").or_else(|| t.strip_prefix("runtime ")) {
            Some(rest) => (true, rest.to_owned()),
            None => (false, t),
        };
        let severity = match base.as_str() {
            "error" | "err" | "fatal" | "fatal error" | "critical" | "high" => Severity::Error,
            "warning" | "warn" | "medium" => Severity::Warning,
            "notice" | "note" | "info" | "information" | "hint" | "remark" | "low" => {
                Severity::Notice
            }
            _ => return None,
        };
        Some((severity, runtime))
    }

    /// CodeRunner's type string for this severity (`runtime` selects
    /// `runtime_error` / `runtime_warning`).
    pub fn type_name(self, runtime: bool) -> &'static str {
        match (self, runtime) {
            (Severity::Error, false) => "error",
            (Severity::Error, true) => "runtime_error",
            (Severity::Warning, false) => "warning",
            (Severity::Warning, true) => "runtime_warning",
            (Severity::Notice, _) => "notice",
        }
    }

    /// Display word (`Error`, `Warning`, `Notice`).
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
            Severity::Notice => "Notice",
        }
    }
}

/// Where a diagnostic came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A linter/compiler run on the document.
    #[default]
    Document,
    /// Parsed from the console output of a run ("Include errors from the
    /// console in the editor").
    Console,
}

/// Unit of the columns of a [`Location::Lines`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnUnit {
    /// UTF-8 bytes (clang, gcc, rustc, go, swiftc).
    #[default]
    Bytes,
    /// Unicode scalar values (python, JSON scripts, CheckStyle).
    Chars,
}

/// A place in a text, in the terms the producing tool used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// Line and column. Lines and columns are **1-based**; `end_column` is
    /// exclusive. Without a column the whole line is meant; with a column
    /// but no end the identifier-like run at the column is meant.
    Lines {
        /// 1-based line.
        line: u32,
        /// 1-based column.
        column: Option<u32>,
        /// 1-based last line (default: `line`).
        end_line: Option<u32>,
        /// 1-based exclusive end column.
        end_column: Option<u32>,
        /// Unit of the columns.
        unit: ColumnUnit,
    },
    /// An absolute offset into the document (characters, or bytes when
    /// `bytes`), 0-based, and a length in the same unit.
    Offsets {
        /// 0-based offset.
        offset: usize,
        /// Length (at least 1 when resolved).
        length: usize,
        /// Offset and length are bytes, not characters.
        bytes: bool,
    },
    /// A span inside a line identified by the line's content, for tools
    /// that print the source line but not its indentation (Python): the
    /// span starts `offset` characters into `text` (the trimmed source
    /// line) and has `length` characters.
    InLine {
        /// 1-based line.
        line: u32,
        /// The trimmed source line as printed by the tool.
        text: String,
        /// Character offset into `text`.
        offset: usize,
        /// Length in characters (0 = to the end of the text).
        length: usize,
    },
}

impl Location {
    /// A whole line (1-based).
    pub fn line(line: u32) -> Self {
        Location::Lines {
            line,
            column: None,
            end_line: None,
            end_column: None,
            unit: ColumnUnit::Bytes,
        }
    }

    /// A line and column (both 1-based, bytes).
    pub fn line_col(line: u32, column: u32) -> Self {
        Location::Lines {
            line,
            column: Some(column),
            end_line: None,
            end_column: None,
            unit: ColumnUnit::Bytes,
        }
    }

    /// The 1-based start line when the location is line based.
    pub fn start_line(&self) -> Option<u32> {
        match self {
            Location::Lines { line, .. } | Location::InLine { line, .. } => Some(*line),
            Location::Offsets { .. } => None,
        }
    }
}

/// One replacement of a [`Fix`]: the text at `location` becomes
/// `replacement` (an empty location length inserts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// What to replace. A [`Location::Lines`] with a column and an end
    /// column is exact; with only a column it is an insertion point.
    pub location: Location,
    /// The new text.
    pub replacement: String,
}

/// A way to fix a diagnostic: one or more edits applied together ("Fix",
/// "Insert ';'", "Choose First").
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Fix {
    /// Button title; `None` = "Fix" (see [`Fix::display_title`]).
    pub title: Option<String>,
    /// Longer description (tooltip).
    pub description: Option<String>,
    /// The edits.
    pub edits: Vec<TextEdit>,
    /// Safe to apply without asking ("Apply Next/All Fixes", auto-fix).
    /// A diagnostic with exactly one fix treats it as preferred whatever
    /// this says; with alternatives (conflict markers) only a fix marked
    /// preferred is applied in bulk.
    pub preferred: bool,
}

impl Fix {
    /// A fix with one edit.
    pub fn single(title: impl Into<String>, location: Location, replacement: impl Into<String>) -> Self {
        Fix {
            title: Some(title.into()),
            description: None,
            edits: vec![TextEdit {
                location,
                replacement: replacement.into(),
            }],
            preferred: false,
        }
    }

    /// The button text: the title or `Fix`.
    pub fn display_title(&self) -> &str {
        self.title.as_deref().filter(|t| !t.is_empty()).unwrap_or("Fix")
    }
}

/// A span of the *parsed text* (console output) that produced a diagnostic:
/// the console colours it with `scopes` and links it back to the diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedLocation {
    /// Byte range in the parsed text.
    pub range: Range<usize>,
    /// `None` = no link, `Some("")` = link to the diagnostic itself
    /// (`coderunner://`-style link generated by the app), otherwise a URL.
    pub link: Option<String>,
    /// Theme scopes, e.g. `console.error.runtime.python`.
    pub scopes: Vec<String>,
}

/// A diagnostic as read from a tool's output, before it is placed in a
/// buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The file it is about; `None` = the document being checked.
    pub file: Option<PathBuf>,
    /// Where; `None` = the file as a whole (or "same place as the parent"
    /// for children).
    pub location: Option<Location>,
    /// Severity.
    pub severity: Severity,
    /// A run-time failure (traceback, panic) rather than a compile-time
    /// problem (CodeRunner's `runtime_error`/`runtime_warning`).
    pub runtime: bool,
    /// The message, without the severity prefix.
    pub message: String,
    /// Identifier of the message (`E0425`, `SC2086`, `-Wunused-variable`,
    /// `CS1002`), used for ignoring.
    pub message_id: Option<String>,
    /// The offending source text, when the tool reports it: used to find
    /// the span in a line when no column is known.
    pub code: Option<String>,
    /// Alternative fixes.
    pub fixes: Vec<Fix>,
    /// Sub-messages (notes, stack frames), shown when the diagnostic is
    /// expanded.
    pub children: Vec<Diagnostic>,
    /// Console spans (only for diagnostics parsed from console text).
    pub matched: Vec<MatchedLocation>,
    /// Id of the [`scratch_language::DiagnosticDef`] that produced it.
    pub source: Option<String>,
    /// Where it came from.
    pub origin: Origin,
}

impl Diagnostic {
    /// A diagnostic with a message and severity and nothing else.
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Diagnostic {
            file: None,
            location: None,
            severity,
            runtime: false,
            message: message.into(),
            message_id: None,
            code: None,
            fixes: Vec::new(),
            children: Vec::new(),
            matched: Vec::new(),
            source: None,
            origin: Origin::Document,
        }
    }

    /// Builder: sets the location.
    #[must_use]
    pub fn at(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }

    /// Builder: sets the file.
    #[must_use]
    pub fn in_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Builder: sets the message id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.message_id = Some(id.into());
        self
    }

    /// Builder: adds a fix.
    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }

    /// The text ignore rules are matched against: the message id when there
    /// is one, otherwise the message (see [`crate::IgnoreList`], which also
    /// tries the message when an id exists).
    pub fn ignore_key(&self) -> &str {
        self.message_id.as_deref().unwrap_or(&self.message)
    }
}

/// FNV-1a 64, used for stable ids (std's hashers are not stable across
/// releases).
pub(crate) fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_types() {
        assert_eq!(Severity::parse_type("error"), Some((Severity::Error, false)));
        assert_eq!(
            Severity::parse_type("runtime_warning"),
            Some((Severity::Warning, true))
        );
        assert_eq!(Severity::parse_type("Note"), Some((Severity::Notice, false)));
        assert_eq!(Severity::parse_type("bogus"), None);
        assert!(Severity::Error > Severity::Warning && Severity::Warning > Severity::Notice);
        assert_eq!(Severity::Warning.type_name(true), "runtime_warning");
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv64(&[b"a", b"b"]), fnv64(&[b"a", b"b"]));
        assert_ne!(fnv64(&[b"ab"]), fnv64(&[b"a", b"b"]));
    }
}
