//! Severity inference: what the text of a message says about its severity.

use std::sync::OnceLock;

use regex::Regex;

use crate::model::Severity;

/// The result of [`infer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inferred {
    /// The severity the text states; `None` when it says nothing.
    pub severity: Option<Severity>,
    /// The message without the severity prefix and without a trailing
    /// option tag.
    pub message: String,
    /// An identifier found in the text (`E0425` from `error[E0425]:`,
    /// `-Wunused-variable` or `SC2086` from a trailing `[...]`).
    pub message_id: Option<String>,
}

fn prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?is)^(?:php\s+)?(?<kind>fatal error|error|warning|warn|note|notice|info|remark|hint|help|deprecated|parse error|fatal|critical|failure)(?:\[(?<id>[^\]\s]+)\])?:\s*(?<rest>.*)$",
        )
        .expect("static regex")
    })
}

fn named_error_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\w*(?:Error|Exception)(?:\b|:)").expect("static regex"))
}

fn named_warning_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\w*Warning:").expect("static regex"))
}

fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\s+\[(?<tag>-W[^\]\s]+|SC\d+)\]\s*$").expect("static regex")
    })
}

/// Reads the severity out of a message and cleans it.
///
/// Rules, in order:
/// 1. A leading `error:`, `fatal error:`, `warning:`, `note:`, `info:`,
///    `remark:`, `hint:`, `deprecated:`, `parse error:`, ... (case
///    insensitive, optional `PHP ` and `[id]`) gives the severity and is
///    removed from the message.
/// 2. A leading `FooError:` / `FooException` is an error and `FooWarning:`
///    a warning; the message is kept as is.
/// 3. A trailing clang/ShellCheck tag (`[-Wunused-variable]`, `[SC2086]`)
///    becomes the message id and is removed.
/// 4. Otherwise nothing is inferred (callers default to error).
pub fn infer(message: &str) -> Inferred {
    let message = message.trim();
    let mut severity = None;
    let mut id = None;
    let mut body = message.to_owned();
    if let Some(c) = prefix_re().captures(message) {
        severity = Some(match c["kind"].to_ascii_lowercase().as_str() {
            "warning" | "warn" | "deprecated" => Severity::Warning,
            "note" | "notice" | "info" | "remark" | "hint" | "help" => Severity::Notice,
            _ => Severity::Error,
        });
        id = c.name("id").map(|m| m.as_str().to_owned());
        body = c["rest"].trim().to_owned();
    } else if named_warning_re().is_match(message) {
        severity = Some(Severity::Warning);
    } else if named_error_re().is_match(message) {
        severity = Some(Severity::Error);
    }
    if let Some(c) = tag_re().captures(&body) {
        let tag = c["tag"].to_owned();
        let whole = c.get(0).map_or(body.len(), |m| m.start());
        body.truncate(whole);
        if id.is_none() {
            id = Some(tag);
        }
    }
    Inferred {
        severity,
        message: body,
        message_id: id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inf(s: &str) -> (Option<Severity>, String, Option<String>) {
        let i = infer(s);
        (i.severity, i.message, i.message_id)
    }

    #[test]
    fn prefixes() {
        assert_eq!(
            inf("error: use of undeclared identifier 'x'"),
            (Some(Severity::Error), "use of undeclared identifier 'x'".into(), None)
        );
        assert_eq!(
            inf("fatal error: 'foo.h' file not found"),
            (Some(Severity::Error), "'foo.h' file not found".into(), None)
        );
        assert_eq!(
            inf("warning: unused variable 'y' [-Wunused-variable]"),
            (
                Some(Severity::Warning),
                "unused variable 'y'".into(),
                Some("-Wunused-variable".into())
            )
        );
        assert_eq!(
            inf("note: previous declaration is here"),
            (Some(Severity::Notice), "previous declaration is here".into(), None)
        );
        assert_eq!(
            inf("error[E0425]: cannot find value `x` in this scope"),
            (
                Some(Severity::Error),
                "cannot find value `x` in this scope".into(),
                Some("E0425".into())
            )
        );
        assert_eq!(
            inf("Parse error: syntax error, unexpected end of file"),
            (Some(Severity::Error), "syntax error, unexpected end of file".into(), None)
        );
        assert_eq!(
            inf("PHP Warning:  Undefined variable $x"),
            (Some(Severity::Warning), "Undefined variable $x".into(), None)
        );
    }

    #[test]
    fn named_exceptions_keep_their_text() {
        assert_eq!(
            inf("SyntaxError: invalid syntax"),
            (Some(Severity::Error), "SyntaxError: invalid syntax".into(), None)
        );
        assert_eq!(
            inf("DeprecationWarning: x is deprecated"),
            (Some(Severity::Warning), "DeprecationWarning: x is deprecated".into(), None)
        );
        assert_eq!(inf("attempt to call a nil value"), (None, "attempt to call a nil value".into(), None));
    }

    #[test]
    fn shellcheck_tag() {
        assert_eq!(
            inf("warning: Quote this to prevent word splitting. [SC2086]"),
            (
                Some(Severity::Warning),
                "Quote this to prevent word splitting.".into(),
                Some("SC2086".into())
            )
        );
    }
}
