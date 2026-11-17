//! The CustomScript JSON protocol and CheckStyle XML.
//!
//! # Custom script contract
//!
//! The interpreter script (any executable with a shebang) receives the raw
//! output on **stdin** and prints a **JSON array** on **stdout** (exit code
//! 0; stderr is shown as the script's own error). Each element:
//!
//! ```json
//! {
//!   "file": "/path/to/file",                 // optional, default: the document
//!   "location": {                            // optional
//!     "offset": 120, "length": 4,            // absolute character offsets, or
//!     "line": 7, "column": 3,                // 1-based line/column
//!     "endLine": 7, "endColumn": 9,          // optional, end column exclusive
//!     "byteOffsets": false                   // optional: columns/offsets are UTF-8 bytes
//!   },
//!   "message": "Something is wrong",
//!   "type": "error",                         // error | warning | notice | runtime_error | runtime_warning
//!   "messageID": "E123",                     // optional, used for ignoring
//!   "fixes": [                               // optional
//!     { "location": { ... }, "replacement": "text", "title": "Fix", "description": "..." },
//!     { "edits": [ { "location": { ... }, "replacement": "..." } ], "title": "Fix both" }
//!   ],
//!   "matchedLocations": [                    // optional, console text only
//!     { "location": { ... }, "link": "", "scopes": ["console.error.python"] }
//!   ],
//!   "children": [ { ...same shape... } ]     // optional, shown when expanded
//! }
//! ```
//!
//! `matchedLocations` locations are relative to the parsed text (the script's
//! stdin); `link: ""` asks for an automatically generated link to the
//! diagnostic. Unknown keys are ignored. A single object, or
//! `{"diagnostics": [...]}`, is accepted instead of an array; text before the
//! first `[`/`{` is skipped so scripts that print a banner still work.

use std::path::PathBuf;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::XmlVersion;
use serde_json::Value;

use super::{ParseError, ParseOptions, scope_for};
use crate::model::{
    ColumnUnit, Diagnostic, Fix, Location, MatchedLocation, Severity, TextEdit,
};
use crate::text::LineIndex;

/// A template for a new custom interpreter script (own text; documents the
/// contract above). Written to `<language>/Diagnostics/<id>/parser`.
pub const DEFAULT_INTERPRETER_SCRIPT: &str = r#"#!/usr/bin/env python3
# Diagnostics interpreter. Reads the raw output of the diagnostic on stdin and
# prints a JSON array of diagnostics on stdout. Change the first line to use
# another interpreter.
#
# Each diagnostic is an object with these keys (all but "message" optional):
#   "file":      path; default is the document being checked
#   "location":  {"line": 1, "column": 1, "endLine": 1, "endColumn": 5}  (1-based; end column exclusive)
#                or {"offset": 0, "length": 5}  (character offsets)
#   "message":   text shown to the user
#   "type":      "error" | "warning" | "notice" | "runtime_error" | "runtime_warning"
#   "messageID": identifier used by "Ignored Messages"
#   "fixes":     [{"location": {...}, "replacement": "text", "title": "Fix"}]
#   "children":  [diagnostics shown when this one is expanded]
#   "matchedLocations": [{"location": {...}, "link": "", "scopes": ["console.error"]}]
#                (console text only: where in the output the message came from)
import json
import re
import sys

text = sys.stdin.read()
diagnostics = []
for number, line in enumerate(text.splitlines(), 1):
    match = re.match(r"^(?P<file>[^\s].*?):(?P<line>\d+): (?P<message>.*)$", line)
    if match:
        diagnostics.append({
            "location": {"line": int(match["line"])},
            "message": match["message"],
        })
json.dump(diagnostics, sys.stdout)
"#;

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn uint(v: Option<&Value>) -> Option<usize> {
    v.and_then(int).and_then(|n| usize::try_from(n).ok())
}

fn location(v: &Value) -> Option<Location> {
    let obj = v.as_object()?;
    let bytes = obj
        .get("byteOffsets")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(offset) = uint(obj.get("offset")) {
        return Some(Location::Offsets {
            offset,
            length: uint(obj.get("length")).unwrap_or(1),
            bytes,
        });
    }
    let line = u32::try_from(uint(obj.get("line"))?).ok()?;
    let col = |k: &str| uint(obj.get(k)).and_then(|c| u32::try_from(c).ok());
    Some(Location::Lines {
        line,
        column: col("column"),
        end_line: col("endLine"),
        end_column: col("endColumn"),
        unit: if bytes { ColumnUnit::Bytes } else { ColumnUnit::Chars },
    })
}

/// Resolves a location of the *parsed text* to a byte range in it.
fn text_range(loc: &Location, index: &LineIndex<'_>) -> Option<std::ops::Range<usize>> {
    let text = index.text();
    match loc {
        Location::Offsets { offset, length, bytes } => {
            let (s, e) = if *bytes {
                ((*offset).min(text.len()), (offset + length).min(text.len()))
            } else {
                (
                    crate::text::char_to_byte(text, *offset),
                    crate::text::char_to_byte(text, offset + length),
                )
            };
            Some(s..e)
        }
        Location::Lines { line, column, end_line, end_column, unit } => {
            let l = *line as usize;
            let bytes = *unit == ColumnUnit::Bytes;
            match column {
                None => index.line_range(l.checked_sub(1)?),
                Some(c) => {
                    let s = index.offset_of(l, *c as usize, bytes)?;
                    let e = end_column
                        .and_then(|ec| {
                            index.offset_of(end_line.map_or(l, |x| x as usize), ec as usize, bytes)
                        })
                        .unwrap_or_else(|| index.line_range(l - 1).map_or(s, |r| r.end));
                    Some(s..e.max(s))
                }
            }
        }
        Location::InLine { .. } => None,
    }
}

fn fix(v: &Value) -> Option<Fix> {
    let obj = v.as_object()?;
    let mut edits = Vec::new();
    if let Some(list) = obj.get("edits").and_then(Value::as_array) {
        for e in list {
            edits.push(TextEdit {
                location: location(e.get("location")?)?,
                replacement: e.get("replacement")?.as_str()?.to_owned(),
            });
        }
    } else {
        edits.push(TextEdit {
            location: location(obj.get("location")?)?,
            replacement: obj
                .get("replacement")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    Some(Fix {
        title: obj.get("title").and_then(Value::as_str).map(str::to_owned),
        description: obj
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        edits,
        preferred: obj
            .get("preferred")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn diagnostic(v: &Value, index: &LineIndex<'_>, opts: &ParseOptions) -> Option<Diagnostic> {
    let obj = v.as_object()?;
    let (severity, runtime) = obj
        .get("type")
        .and_then(Value::as_str)
        .and_then(Severity::parse_type)
        .unwrap_or((Severity::Error, false));
    let message = obj
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut d = Diagnostic::new(severity, message);
    d.runtime = runtime;
    d.origin = opts.origin;
    d.file = obj
        .get("file")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty())
        .map(PathBuf::from);
    d.location = obj.get("location").and_then(location);
    d.message_id = obj
        .get("messageID")
        .or_else(|| obj.get("messageId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    d.fixes = obj
        .get("fixes")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(fix).collect())
        .unwrap_or_default();
    d.children = obj
        .get("children")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|c| diagnostic(c, index, opts)).collect())
        .unwrap_or_default();
    if let Some(list) = obj.get("matchedLocations").and_then(Value::as_array) {
        for m in list {
            let Some(loc) = m.get("location").and_then(location) else {
                continue;
            };
            let Some(range) = text_range(&loc, index) else {
                continue;
            };
            let mut scopes: Vec<String> = m
                .get("scopes")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            if scopes.is_empty() {
                scopes.push(scope_for(severity, runtime, opts.language.as_deref()));
            }
            d.matched.push(MatchedLocation {
                range,
                link: m.get("link").and_then(Value::as_str).map(str::to_owned),
                scopes,
            });
        }
    }
    Some(d)
}

/// Parses the JSON printed by a custom interpreter script. `parsed_text` is
/// the text that was fed to the script (needed to place `matchedLocations`).
///
/// # Errors
///
/// [`ParseError::Json`] when the output is not valid JSON of the expected
/// shape.
pub fn parse_script_json(
    json: &str,
    parsed_text: &str,
    opts: &ParseOptions,
) -> Result<Vec<Diagnostic>, ParseError> {
    let start = json
        .find(['[', '{'])
        .ok_or_else(|| ParseError::Json("no JSON found in the script output".to_owned()))?;
    let mut stream = serde_json::Deserializer::from_str(&json[start..]).into_iter::<Value>();
    let value = match stream.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(ParseError::Json(e.to_string())),
        None => return Err(ParseError::Json("empty script output".to_owned())),
    };
    let index = LineIndex::new(parsed_text);
    let items: Vec<&Value> = match &value {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => match o.get("diagnostics").and_then(Value::as_array) {
            Some(a) => a.iter().collect(),
            None => vec![&value],
        },
        _ => return Err(ParseError::Json("expected an array of diagnostics".to_owned())),
    };
    Ok(items
        .into_iter()
        .filter_map(|v| diagnostic(v, &index, opts))
        .collect())
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// Parses CheckStyle XML (`<checkstyle><file name=".."><error line=".."
/// column=".." severity="error|warning|info|ignore" message=".."
/// source=".."/></file></checkstyle>`), as printed by ESLint, ShellCheck
/// (`-f checkstyle`), PMD, SwiftLint and others. `severity="ignore"` entries
/// are skipped; unknown severities are errors; `source` becomes the message
/// id; a missing/zero line means the whole file.
///
/// # Errors
///
/// [`ParseError::Xml`] on malformed XML.
pub fn parse_checkstyle(xml: &str, opts: &ParseOptions) -> Result<Vec<Diagnostic>, ParseError> {
    let start = xml.find('<').unwrap_or(0);
    let mut reader = Reader::from_str(&xml[start..]);
    let mut file: Option<PathBuf> = None;
    let mut out = Vec::new();
    let mut saw_root = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match e.name().as_ref() {
                    "checkstyle" => saw_root = true,
                    "file" => file = attr(&e, "name").filter(|n| !n.is_empty()).map(PathBuf::from),
                    "error" => {
                        let severity = match attr(&e, "severity").as_deref().map(str::to_ascii_lowercase).as_deref() {
                            Some("ignore") => continue,
                            Some("warning" | "warn") => Severity::Warning,
                            Some("info" | "note" | "notice" | "hint") => Severity::Notice,
                            _ => Severity::Error,
                        };
                        let mut d = Diagnostic::new(severity, attr(&e, "message").unwrap_or_default());
                        d.file.clone_from(&file);
                        d.origin = opts.origin;
                        d.message_id = attr(&e, "source").filter(|s| !s.is_empty());
                        let line = attr(&e, "line").and_then(|l| l.trim().parse::<u32>().ok()).filter(|l| *l > 0);
                        let column = attr(&e, "column").and_then(|c| c.trim().parse::<u32>().ok()).filter(|c| *c > 0);
                        d.location = line.map(|line| Location::Lines {
                            line,
                            column,
                            end_line: None,
                            end_column: None,
                            unit: ColumnUnit::Chars,
                        });
                        out.push(d);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == "file" => file = None,
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(ParseError::Xml(e.to_string())),
        }
    }
    if !saw_root && out.is_empty() {
        return Err(ParseError::Xml("not a CheckStyle document".to_owned()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_json_full_shape() {
        let json = r#"banner
[
 {"file": "/a/b.py", "location": {"line": 3, "column": 5, "endLine": 3, "endColumn": 9},
  "message": "bad", "type": "runtime_warning", "messageID": "W1",
  "fixes": [{"location": {"line": 3, "column": 5, "endColumn": 9}, "replacement": "ok", "title": "Use ok"},
            {"edits": [{"location": {"offset": 0, "length": 0}, "replacement": "x"}]}],
  "matchedLocations": [{"location": {"offset": 2, "length": 3}, "link": "", "scopes": ["console.warning.runtime"]},
                       {"location": {"line": 2}}],
  "children": [{"message": "called here", "location": {"line": 9}, "type": "notice"}]},
 {"message": "no location"}
]"#;
        let d = parse_script_json(json, "ab\ncdefg\n", &ParseOptions::default()).unwrap();
        assert_eq!(d.len(), 2);
        let a = &d[0];
        assert_eq!(a.file.as_deref().and_then(|p| p.to_str()), Some("/a/b.py"));
        assert_eq!((a.severity, a.runtime), (Severity::Warning, true));
        assert_eq!(a.message_id.as_deref(), Some("W1"));
        assert_eq!(
            a.location,
            Some(Location::Lines { line: 3, column: Some(5), end_line: Some(3), end_column: Some(9), unit: ColumnUnit::Chars })
        );
        assert_eq!(a.fixes.len(), 2);
        assert_eq!(a.fixes[0].display_title(), "Use ok");
        assert_eq!(a.fixes[1].display_title(), "Fix");
        assert_eq!(a.matched.len(), 2);
        assert_eq!(a.matched[0].range, 2..5);
        assert_eq!(a.matched[0].link.as_deref(), Some(""));
        assert_eq!(a.matched[1].range, 3..8, "line 2 of the parsed text");
        assert_eq!(a.matched[1].scopes, vec![scope_for(Severity::Warning, true, None)]);
        assert_eq!(a.children[0].severity, Severity::Notice);
        assert_eq!(d[1].severity, Severity::Error);
        assert!(d[1].location.is_none());
    }

    #[test]
    fn script_json_variants_and_errors() {
        let single = parse_script_json(r#"{"message":"x","location":{"line":"4"}}"#, "", &ParseOptions::default()).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].location, Some(Location::Lines { line: 4, column: None, end_line: None, end_column: None, unit: ColumnUnit::Chars }));
        let wrapped = parse_script_json(r#"{"diagnostics":[{"message":"a"},{"message":"b"}]}"#, "", &ParseOptions::default()).unwrap();
        assert_eq!(wrapped.len(), 2);
        let bytes = parse_script_json(r#"[{"message":"a","location":{"line":1,"column":2,"byteOffsets":true}}]"#, "", &ParseOptions::default()).unwrap();
        assert!(matches!(bytes[0].location, Some(Location::Lines { unit: ColumnUnit::Bytes, .. })));
        assert!(matches!(parse_script_json("no json", "", &ParseOptions::default()), Err(ParseError::Json(_))));
        assert!(matches!(parse_script_json("[{", "", &ParseOptions::default()), Err(ParseError::Json(_))));
        assert!(parse_script_json("[]", "", &ParseOptions::default()).unwrap().is_empty());
    }

    #[test]
    fn checkstyle_eslint_like() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<checkstyle version="4.3">
<file name="/work/a.js">
<error line="1" column="5" severity="error" message="&apos;x&apos; is not defined." source="eslint.rules.no-undef" />
<error line="2" column="1" severity="warning" message="Missing semicolon." source="eslint.rules.semi" />
<error line="3" severity="info" message="fyi" />
<error line="4" severity="ignore" message="skipped" />
</file>
<file name="/work/b.js"><error line="0" severity="weird" message="whole file"/></file>
</checkstyle>"#;
        let d = parse_checkstyle(xml, &ParseOptions::default()).unwrap();
        assert_eq!(d.len(), 4);
        assert_eq!(d[0].message, "'x' is not defined.");
        assert_eq!(d[0].message_id.as_deref(), Some("eslint.rules.no-undef"));
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("/work/a.js"));
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[2].severity, Severity::Notice);
        assert_eq!(d[2].location, Some(Location::Lines { line: 3, column: None, end_line: None, end_column: None, unit: ColumnUnit::Chars }));
        assert_eq!(d[3].file.as_deref().and_then(|p| p.to_str()), Some("/work/b.js"));
        assert!(d[3].location.is_none());
        assert!(matches!(parse_checkstyle("<a><b></a>", &ParseOptions::default()), Err(ParseError::Xml(_))));
        assert!(matches!(parse_checkstyle("plain text", &ParseOptions::default()), Err(ParseError::Xml(_))));
    }

    #[test]
    fn template_script_mentions_contract() {
        assert!(DEFAULT_INTERPRETER_SCRIPT.starts_with("#!"));
        assert!(DEFAULT_INTERPRETER_SCRIPT.contains("messageID"));
    }
}
