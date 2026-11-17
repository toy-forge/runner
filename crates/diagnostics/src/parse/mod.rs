//! Output parsers: turn the text a tool printed into [`Diagnostic`]s.
//!
//! | Function | Input |
//! |--|--|
//! | [`parse_unix`] | `file:line:col: message` (clang, gcc, swiftc, go, javac, shellcheck, ...) with notes, fix-its and caret lines |
//! | [`parse_regex`] / [`DiagRegex`] | user regex with named groups |
//! | [`parse_script_json`] | output of a custom interpreter script ([`json`] documents the contract) |
//! | [`parse_checkstyle`] | CheckStyle XML |
//! | [`parse_python`], [`parse_node`], [`parse_rust_panic`], [`parse_go_panic`], [`parse_java`], [`parse_ruby`] | run-time failures with stack traces |
//! | [`parse_rustc`] | rustc/cargo human format |
//! | [`parse_auto`] | tries all of the above (console import) |
//! | [`interpret`] | dispatch on a language definition's `output_mode` |
//!
//! [`infer`] holds the severity inference rules.

mod infer;
mod json;
mod regex_mode;
mod rustc;
mod trace;
mod unix;

use scratch_language::OutputMode;

pub use infer::{Inferred, infer};
pub use json::{DEFAULT_INTERPRETER_SCRIPT, parse_checkstyle, parse_script_json};
pub use regex_mode::{DiagRegex, parse_regex};
pub use rustc::parse_rustc;
pub use trace::{parse_go_panic, parse_java, parse_node, parse_python, parse_ruby, parse_rust_panic};
pub use unix::{UNIX_REGEX, parse_unix};

use crate::model::{Diagnostic, Origin, Severity};

/// Why parsing failed (as opposed to finding nothing).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The user's regular expression does not compile (message from the
    /// engine; lookaround and backreferences are not supported).
    #[error("invalid regular expression: {0}")]
    Regex(String),
    /// `OutputMode::Regex` without a regex.
    #[error("this diagnostic uses a regular expression but has none")]
    MissingRegex,
    /// The script output is not valid diagnostics JSON.
    #[error("invalid JSON from the diagnostics script: {0}")]
    Json(String),
    /// The CheckStyle output is not valid XML.
    #[error("invalid CheckStyle XML: {0}")]
    Xml(String),
    /// `OutputMode::CustomScript` needs a process; use the runner.
    #[error("custom script output needs a script run")]
    NeedsScript,
}

/// Options shared by the parsers.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// Language id used in console scopes (`console.error.python`).
    pub language: Option<String>,
    /// Origin stamped on the diagnostics.
    pub origin: Origin,
}

impl ParseOptions {
    /// Options for text parsed from the console.
    pub fn console(language: Option<&str>) -> Self {
        Self {
            language: language.map(str::to_owned),
            origin: Origin::Console,
        }
    }
}

/// The theme scope of a console span: `console.error`,
/// `console.error.runtime`, `console.warning.python`, ...
pub fn scope_for(severity: Severity, runtime: bool, language: Option<&str>) -> String {
    let base = match severity {
        Severity::Error => "console.error",
        Severity::Warning => "console.warning",
        Severity::Notice => "console.notice",
    };
    let mut s = base.to_owned();
    if runtime {
        s.push_str(".runtime");
    }
    if let Some(l) = language.filter(|l| !l.is_empty()) {
        s.push('.');
        s.push_str(l);
    }
    s
}

/// Interprets `output` according to a definition's output mode.
///
/// # Errors
///
/// [`ParseError`]; [`ParseError::NeedsScript`] for
/// [`OutputMode::CustomScript`], which the runner executes.
pub fn interpret(
    output: &str,
    mode: OutputMode,
    regex: Option<&str>,
    opts: &ParseOptions,
) -> Result<Vec<Diagnostic>, ParseError> {
    match mode {
        OutputMode::Unix => Ok(parse_unix(output, opts)),
        OutputMode::Regex => {
            let pattern = regex.filter(|r| !r.trim().is_empty()).ok_or(ParseError::MissingRegex)?;
            parse_regex(output, pattern, opts)
        }
        OutputMode::CheckStyle => parse_checkstyle(output, opts),
        OutputMode::CustomScript => Err(ParseError::NeedsScript),
    }
}

/// Position of a diagnostic in the parsed text (its first console span).
fn order_key(d: &Diagnostic) -> usize {
    d.matched.iter().map(|m| m.range.start).min().unwrap_or(usize::MAX)
}

/// MSBuild/tsc style `file(line,col): error CODE: message` lines.
const PAREN_REGEX: &str = r"^(?<file>[^\s].*?)\((?<line>\d+),(?<column>\d+)\): (((?<error>error)|(?<warning>warning)|(?<note>note|notice)) ((?<messageID>\w+\d+): )?)?(?<message>.*)$";
/// PHP `... in file on line N`.
const PHP_REGEX: &str = r"^(?:PHP )?(?<message>(?:Parse error|Fatal error|Warning|Notice|Deprecated)\b.*?) in (?<file>/.*?) on line (?<line>\d+)$";
/// Perl `... at file line N.`
const PERL_REGEX: &str = r"^(?<message>.*?) at (?<file>[^\s]+) line (?<line>\d+)(?:, near .*|\.)$";

/// Tries every known output format on console/tool output and returns the
/// union, ordered by position in the text, without duplicates (a
/// location-less diagnostic is dropped when a located one has the same
/// message).
pub fn parse_auto(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    let mut all: Vec<Diagnostic> = Vec::new();
    all.extend(parse_python(output, opts));
    all.extend(parse_node(output, opts));
    all.extend(parse_rust_panic(output, opts));
    all.extend(parse_go_panic(output, opts));
    all.extend(parse_java(output, opts));
    all.extend(parse_ruby(output, opts));
    all.extend(parse_rustc(output, opts));
    all.extend(parse_unix(output, opts));
    for pattern in [PAREN_REGEX, PHP_REGEX, PERL_REGEX] {
        if let Ok(found) = parse_regex(output, pattern, opts) {
            all.extend(found);
        }
    }
    dedupe(all)
}

/// Orders by position in the parsed text and removes duplicates.
pub fn dedupe(mut all: Vec<Diagnostic>) -> Vec<Diagnostic> {
    all.sort_by_key(order_key);
    let located: Vec<(String, Option<String>)> = all
        .iter()
        .filter(|d| d.location.is_some())
        .map(|d| (d.message.clone(), d.message_id.clone()))
        .collect();
    let mut seen: Vec<(Option<std::path::PathBuf>, Option<crate::model::Location>, String)> = Vec::new();
    let mut out = Vec::new();
    for d in all {
        if d.location.is_none() && located.iter().any(|(m, _)| *m == d.message) {
            continue;
        }
        let key = (d.file.clone(), d.location.clone(), d.message.clone());
        // Same place and same message from two parsers: keep the richer one
        // (the one with children or fixes comes first by sorting stability
        // only when equal, so prefer replacing).
        if let Some(pos) = seen.iter().position(|k| *k == key) {
            let existing: &mut Diagnostic = &mut out[pos];
            if d.children.len() + d.fixes.len() > existing.children.len() + existing.fixes.len() {
                *existing = d;
            }
            continue;
        }
        seen.push(key);
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        assert_eq!(scope_for(Severity::Error, false, None), "console.error");
        assert_eq!(scope_for(Severity::Warning, true, Some("python")), "console.warning.runtime.python");
    }

    #[test]
    fn interpret_dispatch() {
        let opts = ParseOptions::default();
        let d = interpret("a.c:1:2: error: x\n", OutputMode::Unix, None, &opts).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(interpret("x", OutputMode::Regex, None, &opts), Err(ParseError::MissingRegex));
        assert_eq!(interpret("x", OutputMode::CustomScript, None, &opts), Err(ParseError::NeedsScript));
        let d = interpret("L1: hello\n", OutputMode::Regex, Some(r"^L(?<line>\d+): (?<message>.*)$"), &opts).unwrap();
        assert_eq!(d[0].message, "hello");
    }

    #[test]
    fn auto_handles_mixed_output_without_duplicates() {
        let out = "\
error[E0425]: cannot find value `x` in this scope
 --> src/main.rs:2:20
  |
2 |     println!(\"{}\", x);
  |                    ^ not found in this scope

error: aborting due to 1 previous error

Traceback (most recent call last):
  File \"/work/a.py\", line 1, in <module>
    1/0
ZeroDivisionError: division by zero
Program.cs(12,9): error CS1002: ; expected
Parse error: syntax error, unexpected end of file in /work/a.php on line 4
";
        let d = parse_auto(out, &ParseOptions::console(None));
        let msgs: Vec<&str> = d.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "cannot find value `x` in this scope",
                "ZeroDivisionError: division by zero",
                "; expected",
                "syntax error, unexpected end of file"
            ],
            "{d:#?}"
        );
        assert!(d.iter().all(|d| d.origin == Origin::Console));
    }
}
