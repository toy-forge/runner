//! rustc / cargo human-readable diagnostics:
//!
//! ```text
//! error[E0425]: cannot find value `x` in this scope
//!  --> src/main.rs:2:20
//!   |
//! 2 |     println!("{}", x);
//!   |                    ^ not found in this scope
//!   = note: ...
//! ```

use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

use super::infer::infer;
use super::trace::{lines_with_offsets, re};
use super::{ParseOptions, scope_for};
use crate::model::{ColumnUnit, Diagnostic, Location, MatchedLocation, Severity};

/// Parses rustc's human format. Diagnostics without a `-->` location are
/// kept only when they are errors that are not the "aborting" summary.
pub fn parse_rustc(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    static ARROW: OnceLock<Regex> = OnceLock::new();
    static MARK: OnceLock<Regex> = OnceLock::new();
    static NOTE: OnceLock<Regex> = OnceLock::new();
    let head_re = re(
        &HEAD,
        r"^(?<kind>error|warning|note|help)(?:\[(?<id>[^\]]+)\])?: (?<msg>.+)$",
    );
    let arrow_re = re(&ARROW, r"^\s*--> (?<file>.+?):(?<line>\d+):(?<col>\d+)$");
    let mark_re = re(&MARK, r"^\s+\|\s.*?(?<marks>\^+)");
    let note_re = re(&NOTE, r"^\s*= (?<kind>note|help|warning): (?<msg>.+)$");
    let lines = lines_with_offsets(output);
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, text) = lines[i];
        let Some(h) = head_re.captures(text) else {
            i += 1;
            continue;
        };
        let msg = h["msg"].trim().to_owned();
        if msg.starts_with("aborting due to")
            || msg.starts_with("could not compile")
            || (msg.contains("warning") && msg.contains("emitted"))
            || msg.starts_with("For more information")
        {
            i += 1;
            continue;
        }
        let severity = match &h["kind"] {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => Severity::Notice,
        };
        let inferred = infer(&msg);
        let mut d = Diagnostic::new(severity, if inferred.severity.is_some() { inferred.message } else { msg });
        d.message_id = h.name("id").map(|m| m.as_str().to_owned());
        d.origin = opts.origin;
        d.matched.push(MatchedLocation {
            range: start..start + text.len(),
            link: Some(String::new()),
            scopes: vec![scope_for(severity, false, opts.language.as_deref())],
        });
        let mut j = i + 1;
        let mut has_location = false;
        // Location line directly after the header.
        if let Some(a) = lines.get(j).and_then(|l| arrow_re.captures(l.1)) {
            let line: u32 = a["line"].parse().unwrap_or(1);
            let col: u32 = a["col"].parse().unwrap_or(1);
            d.file = Some(PathBuf::from(&a["file"]));
            let mut end_column = None;
            j += 1;
            // Find the primary carets in the code frame.
            let mut k = j;
            while k < lines.len() && k < j + 12 {
                let t = lines[k].1;
                if head_re.is_match(t) || t.trim().is_empty() {
                    break;
                }
                if let Some(m) = mark_re.captures(t) {
                    end_column = Some(col + u32::try_from(m["marks"].len()).unwrap_or(1));
                    break;
                }
                k += 1;
            }
            d.location = Some(Location::Lines {
                line,
                column: Some(col),
                end_line: None,
                end_column,
                unit: ColumnUnit::Chars,
            });
            has_location = true;
        }
        // Skip the code frame and collect `= note:` / `= help:` lines.
        while j < lines.len() {
            let t = lines[j].1;
            if head_re.is_match(t) || (t.is_empty() && lines.get(j + 1).is_none_or(|n| head_re.is_match(n.1) || n.1.is_empty())) {
                break;
            }
            if let Some(n) = note_re.captures(t) {
                let mut c = Diagnostic::new(Severity::Notice, n["msg"].trim().to_owned());
                c.origin = opts.origin;
                d.children.push(c);
            }
            j += 1;
        }
        i = j.max(i + 1);
        if !has_location && severity != Severity::Error {
            continue;
        }
        // Notes with their own location belong to the previous diagnostic.
        if severity == Severity::Notice && has_location && let Some(prev) = out.last_mut() {
            prev.children.push(d);
            continue;
        }
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_format() {
        let out = r#"   Compiling x v0.1.0 (/work/x)
error[E0425]: cannot find value `x` in this scope
 --> src/main.rs:2:20
  |
2 |     println!("{}", x);
  |                    ^ not found in this scope

warning: unused variable: `y`
 --> src/main.rs:3:9
  |
3 |     let y = 1;
  |         ^ help: if this is intentional, prefix it with an underscore: `_y`
  |
  = note: `#[warn(unused_variables)]` on by default

error[E0308]: mismatched types
 --> src/main.rs:5:18
  |
5 |     let n: i32 = "a";
  |            ---   ^^^ expected `i32`, found `&str`
  |            |
  |            expected due to this
  |
note: required by a bound in `foo`
 --> src/lib.rs:1:1
  |
1 | fn foo() {}
  | ^^^

error: aborting due to 2 previous errors

For more information about this error, try `rustc --explain E0425`.
"#;
        let d = parse_rustc(out, &ParseOptions::default());
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!(d[0].message, "cannot find value `x` in this scope");
        assert_eq!(d[0].message_id.as_deref(), Some("E0425"));
        assert_eq!(
            d[0].location,
            Some(Location::Lines { line: 2, column: Some(20), end_line: None, end_column: Some(21), unit: ColumnUnit::Chars })
        );
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[1].children.len(), 1);
        assert!(d[1].children[0].message.contains("unused_variables"));
        assert_eq!(d[2].severity, Severity::Error);
        assert_eq!(d[2].children.len(), 1, "located note attaches to the error");
        assert_eq!(d[2].children[0].file.as_deref().and_then(|p| p.to_str()), Some("src/lib.rs"));
        assert_eq!(
            d[2].location,
            Some(Location::Lines { line: 5, column: Some(18), end_line: None, end_column: Some(21), unit: ColumnUnit::Chars })
        );
    }
}
