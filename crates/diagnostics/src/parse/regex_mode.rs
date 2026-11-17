//! "Regular Expression" output mode: a user regex with named groups.
//!
//! Recognised groups: `file`, `line`, `column`, `message`, `messageID`,
//! `error`, `warning`, `note`, `zeroLine`, `zeroColumn`, `endLine`,
//! `endColumn`, `offset`, `endOffset`, `length`, `code`. The regex is
//! searched over the whole output in multi-line mode (`^`/`$` match at line
//! boundaries). The same group name may appear several times (Oniguruma
//! style); the first one that matched wins. Lookaround and backreferences
//! are not supported by the underlying `regex` engine and are reported as
//! [`ParseError::Regex`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use regex::{Captures, Regex, RegexBuilder};

use super::infer::infer;
use super::{ParseError, ParseOptions, scope_for};
use crate::model::{ColumnUnit, Diagnostic, Location, MatchedLocation, Severity};

/// A compiled user regex.
#[derive(Debug, Clone)]
pub struct DiagRegex {
    regex: Regex,
    /// Canonical group name -> the names (possibly renamed duplicates) to
    /// look at.
    groups: BTreeMap<&'static str, Vec<String>>,
}

const KNOWN: &[(&str, &[&str])] = &[
    ("file", &["file"]),
    ("line", &["line"]),
    ("column", &["column"]),
    ("message", &["message"]),
    ("message_id", &["messageid", "message_id"]),
    ("error", &["error"]),
    ("warning", &["warning"]),
    ("note", &["note", "notice"]),
    ("zero_line", &["zeroline"]),
    ("zero_column", &["zerocolumn"]),
    ("end_line", &["endline"]),
    ("end_column", &["endcolumn"]),
    ("offset", &["offset"]),
    ("end_offset", &["endoffset"]),
    ("length", &["length"]),
    ("code", &["code"]),
];

/// Renames duplicate group names (`name`, `name__2`, ...) so the `regex`
/// crate accepts the pattern; returns the pattern and every name used.
fn dedupe_names(pattern: &str) -> (String, Vec<(String, String)>) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut names: Vec<(String, String)> = Vec::new(); // (original, actual)
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut i = 0;
    let mut in_class = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            out.push(c);
            if let Some(n) = chars.get(i + 1) {
                out.push(*n);
            }
            i += 2;
            continue;
        }
        if in_class > 0 {
            if c == '[' {
                in_class += 1;
            } else if c == ']' {
                in_class -= 1;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if c == '[' {
            in_class = 1;
            out.push(c);
            i += 1;
            // A leading `]` (or `^]`) is a literal.
            if chars.get(i) == Some(&'^') {
                out.push('^');
                i += 1;
            }
            if chars.get(i) == Some(&']') {
                out.push(']');
                i += 1;
            }
            continue;
        }
        if c == '(' && chars.get(i + 1) == Some(&'?') {
            let (skip, is_named) = match (chars.get(i + 2), chars.get(i + 3)) {
                (Some('P'), Some('<')) => (4, true),
                (Some('<'), Some(n)) if *n != '=' && *n != '!' => (3, true),
                _ => (0, false),
            };
            if is_named
                && let Some(close) = chars[i + skip..].iter().position(|x| *x == '>')
            {
                let name: String = chars[i + skip..i + skip + close].iter().collect();
                let n = counts.entry(name.clone()).or_default();
                *n += 1;
                let actual = if *n == 1 {
                    name.clone()
                } else {
                    format!("{name}__{n}")
                };
                out.push_str("(?<");
                out.push_str(&actual);
                out.push('>');
                names.push((name, actual));
                i += skip + close + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    (out, names)
}

impl DiagRegex {
    /// Compiles `pattern` (multi-line mode).
    ///
    /// # Errors
    ///
    /// [`ParseError::Regex`] with the engine's message.
    pub fn new(pattern: &str) -> Result<Self, ParseError> {
        let (rewritten, names) = dedupe_names(pattern);
        let regex = RegexBuilder::new(&rewritten)
            .multi_line(true)
            .build()
            .map_err(|e| ParseError::Regex(e.to_string()))?;
        let mut groups: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        for (original, actual) in names {
            let lower = original.to_ascii_lowercase();
            for (canon, aliases) in KNOWN {
                if aliases.contains(&lower.as_str()) {
                    groups.entry(canon).or_default().push(actual.clone());
                }
            }
        }
        Ok(Self { regex, groups })
    }

    /// Whether the regex defines a group by canonical name (`line`,
    /// `message`, `offset`, ...).
    pub fn has_group(&self, canonical: &str) -> bool {
        self.groups.contains_key(canonical)
    }

    fn text<'t>(&self, caps: &Captures<'t>, canonical: &str) -> Option<&'t str> {
        self.groups
            .get(canonical)?
            .iter()
            .filter_map(|n| caps.name(n))
            .map(|m| m.as_str())
            .find(|s| !s.is_empty())
    }

    fn number(&self, caps: &Captures<'_>, canonical: &str) -> Option<u32> {
        self.text(caps, canonical)?.trim().parse().ok()
    }

    /// Builds a diagnostic from one match; `None` when the match carries no
    /// usable information.
    pub fn diagnostic(&self, caps: &Captures<'_>, opts: &ParseOptions) -> Option<Diagnostic> {
        let whole = caps.get(0)?;
        let file = self
            .text(caps, "file")
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(PathBuf::from);
        let code = self.text(caps, "code").map(str::to_owned);

        let line = self
            .number(caps, "line")
            .or_else(|| self.number(caps, "zero_line").map(|l| l + 1));
        let column = self
            .number(caps, "column")
            .or_else(|| self.number(caps, "zero_column").map(|c| c + 1));
        let location = if let Some(offset) = self.number(caps, "offset") {
            let offset = offset as usize;
            let length = self
                .number(caps, "length")
                .map(|l| l as usize)
                .or_else(|| {
                    self.number(caps, "end_offset")
                        .map(|e| (e as usize).saturating_sub(offset))
                })
                .unwrap_or(1)
                .max(1);
            Some(Location::Offsets {
                offset,
                length,
                bytes: false,
            })
        } else {
            line.map(|line| Location::Lines {
                line,
                column,
                end_line: self.number(caps, "end_line"),
                end_column: self.number(caps, "end_column"),
                unit: ColumnUnit::Bytes,
            })
        };

        let mut message = self
            .text(caps, "message")
            .map(|m| m.trim().to_owned())
            .unwrap_or_else(|| {
                whole
                    .as_str()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            });
        let mut message_id = self.text(caps, "message_id").map(str::to_owned);
        let explicit = if self.text(caps, "error").is_some() {
            Some(Severity::Error)
        } else if self.text(caps, "warning").is_some() {
            Some(Severity::Warning)
        } else if self.text(caps, "note").is_some() {
            Some(Severity::Notice)
        } else {
            None
        };
        let severity = match explicit {
            Some(s) => s,
            None => {
                let inferred = infer(&message);
                message = inferred.message;
                if message_id.is_none() {
                    message_id = inferred.message_id;
                }
                inferred.severity.unwrap_or(Severity::Error)
            }
        };
        let mut d = Diagnostic::new(severity, message);
        d.file = file;
        d.location = location;
        d.message_id = message_id;
        d.code = code;
        d.origin = opts.origin;
        d.matched = vec![MatchedLocation {
            range: whole.range(),
            link: Some(String::new()),
            scopes: vec![scope_for(severity, false, opts.language.as_deref())],
        }];
        Some(d)
    }

    /// All diagnostics in `output`. Matches that carry neither a location
    /// nor a message are skipped; so are empty matches.
    pub fn parse(&self, output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
        self.regex
            .captures_iter(output)
            .filter(|c| c.get(0).is_some_and(|m| !m.is_empty()))
            .filter_map(|c| self.diagnostic(&c, opts))
            .collect()
    }
}

/// Parses `output` with the user regex `pattern`.
///
/// # Errors
///
/// [`ParseError::Regex`] when the pattern does not compile.
pub fn parse_regex(output: &str, pattern: &str, opts: &ParseOptions) -> Result<Vec<Diagnostic>, ParseError> {
    Ok(DiagRegex::new(pattern)?.parse(output, opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> ParseOptions {
        ParseOptions::default()
    }

    #[test]
    fn named_groups_and_severity() {
        let re = r"^(?<file>[^\s].*?)\((?<line>\d+),(?<column>\d+)\): (((?<error>error)|(?<warning>warning)|(?<note>note|notice)) (?<messageID>\w+\d+): ?)?(?<message>.*)$";
        let out = "Program.cs(12,9): error CS1002: ; expected\nProgram.cs(3,1): warning CS0168: The variable 'x' is declared but never used\nBuild succeeded.\n";
        let d = parse_regex(out, re, &opts()).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].message, "; expected");
        assert_eq!(d[0].message_id.as_deref(), Some("CS1002"));
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("Program.cs"));
        assert_eq!(d[0].location, Some(Location::line_col(12, 9)));
        assert_eq!(d[1].severity, Severity::Warning);
    }

    #[test]
    fn duplicate_group_names_and_multiline_flag() {
        let re = r"^(?:(?<file>a\.c):(?<line>\d+): (?<message>.*)|(?<file>b\.c)\[(?<line>\d+)\] (?<message>.*))$";
        let d = parse_regex("a.c:1: warning: one\nb.c[2] error: two\n", re, &opts()).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("a.c"));
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].message, "one");
        assert_eq!(d[1].file.as_deref().and_then(|p| p.to_str()), Some("b.c"));
        assert_eq!(d[1].location, Some(Location::line(2)));
    }

    #[test]
    fn zero_based_and_offsets() {
        let re = r"^(?<zeroLine>\d+):(?<zeroColumn>\d+) (?<message>.*)$";
        let d = parse_regex("0:4 oops\n", re, &opts()).unwrap();
        assert_eq!(d[0].location, Some(Location::line_col(1, 5)));
        let re = r"^(?<file>[^\s].*?):(?<offset>\d+):((?<endOffset>\d+):)? (script error:)?(?<message>.*)$";
        let d = parse_regex("x.applescript:10:14: Expected end\n", re, &opts()).unwrap();
        assert_eq!(
            d[0].location,
            Some(Location::Offsets {
                offset: 10,
                length: 4,
                bytes: false
            })
        );
    }

    #[test]
    fn python_syntax_regex_spans_lines() {
        let re = r#"(?s)File "(?<file>[^"]+)", line (?<line>\d+)\n(?:[^\n]*\n)*?(?<message>(?<error>\w*Error): [^\n]*)"#;
        let text = "Traceback:\n  File \"x.py\", line 3\n    foo bar\n        ^^^\nSyntaxError: invalid syntax\n";
        let d = parse_regex(text, re, &opts()).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "SyntaxError: invalid syntax");
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn code_group_and_errors() {
        let re = r#"^(?<message>.*?) at (?<file>.*?) line (?<line>\d+).*( near "(?<code>.*)").*$"#;
        let d = parse_regex("syntax error at x.pl line 3, near \"foo bar\"\n", re, &opts()).unwrap();
        assert_eq!(d[0].code.as_deref(), Some("foo bar"));
        assert!(matches!(parse_regex("x", "(", &opts()), Err(ParseError::Regex(_))));
        assert!(matches!(
            parse_regex("x", r"(?<=a)b", &opts()),
            Err(ParseError::Regex(_))
        ));
    }

    #[test]
    fn character_class_with_group_syntax_is_not_renamed() {
        let (out, names) = dedupe_names(r"[(?<x>]+(?<a>\d)(?<a>\w)");
        assert_eq!(names.len(), 2);
        assert!(out.contains("(?<a__2>"));
        assert!(out.starts_with("[(?<x>]+"));
    }
}
