//! Built-in checkers that need no external tool.
//!
//! * [`check_conflicts`]: version control conflict markers with the fixes
//!   "Choose First", "Choose Both", "Choose Last" (`DiagnosticKind::Conflict`).
//! * [`check_regex`]: is the document a valid regular expression
//!   (`DiagnosticKind::RegexCheck`, "Regex Invalid").
//! * [`check_syntax`]: syntax errors of JSON, TOML and XML (and so property
//!   lists) documents (`DiagnosticKind::InvalidSyntax`).

use scratch_language::{DiagnosticKind, LanguageDef};

use crate::model::{ColumnUnit, Diagnostic, Fix, Location, Severity, TextEdit};
use crate::text::LineIndex;

/// Message of conflict diagnostics.
pub const CONFLICT_MESSAGE: &str = "Version Control Conflict";

fn is_marker(line: &str, marker: char, exact: bool) -> bool {
    let n = line.chars().take_while(|c| *c == marker).count();
    if n != 7 {
        return false;
    }
    let rest = &line[7..];
    if exact {
        rest.trim().is_empty()
    } else {
        rest.is_empty() || rest.starts_with(' ')
    }
}

/// Finds conflict regions (`<<<<<<<` ... `=======` ... `>>>>>>>`, with an
/// optional diff3 `|||||||` base section) and offers to resolve each.
pub fn check_conflicts(text: &str) -> Vec<Diagnostic> {
    // (start offset, end offset incl. newline, text of first, text of last)
    struct Open {
        start: usize,
        line: u32,
        first: Option<(usize, usize)>,
        second_start: Option<usize>,
        base: bool,
    }
    let mut out = Vec::new();
    let mut open: Option<Open> = None;
    let mut offset = 0;
    for (n, raw) in text.split_inclusive('\n').enumerate() {
        let line_no = u32::try_from(n + 1).unwrap_or(u32::MAX);
        let line = raw.trim_end_matches(['\n', '\r']);
        let start = offset;
        let end = offset + raw.len();
        offset = end;
        if is_marker(line, '<', false) {
            if let Some(prev) = open.take() {
                out.push(unterminated(prev.line));
            }
            open = Some(Open {
                start,
                line: line_no,
                first: None,
                second_start: None,
                base: false,
            });
        } else if let Some(o) = open.as_mut() {
            if is_marker(line, '|', false) && o.first.is_none() {
                // diff3 base section: ours ends here.
                o.first = Some((o.start, start));
                o.base = true;
            } else if is_marker(line, '=', true) {
                if o.first.is_none() {
                    o.first = Some((o.start, start));
                }
                o.second_start = Some(end);
            } else if is_marker(line, '>', false)
                && let (Some((_, first_end)), Some(second_start)) = (o.first, o.second_start)
            {
                // Content of "ours" starts after the `<<<<<<<` line.
                let ours_start = text[o.start..]
                    .find('\n')
                    .map_or(text.len(), |i| o.start + i + 1);
                let first = &text[ours_start..first_end.max(ours_start)];
                let last = &text[second_start..start.max(second_start)];
                let region = (o.start, end);
                out.push(conflict(o.line, region, first, last));
                open = None;
            }
        }
    }
    if let Some(prev) = open {
        out.push(unterminated(prev.line));
    }
    out
}

fn unterminated(line: u32) -> Diagnostic {
    let mut d = Diagnostic::new(Severity::Error, CONFLICT_MESSAGE).at(Location::line(line));
    d.message_id = Some("conflict".to_owned());
    d
}

fn conflict(line: u32, region: (usize, usize), first: &str, last: &str) -> Diagnostic {
    let location = Location::Offsets {
        offset: region.0,
        length: region.1 - region.0,
        bytes: true,
    };
    let choose = |title: &str, replacement: String| Fix {
        title: Some(title.to_owned()),
        description: None,
        edits: vec![TextEdit {
            location: location.clone(),
            replacement,
        }],
        preferred: false,
    };
    let mut d = Diagnostic::new(Severity::Error, CONFLICT_MESSAGE).at(Location::line(line));
    d.message_id = Some("conflict".to_owned());
    d.fixes = vec![
        choose("Choose First", first.to_owned()),
        choose("Choose Both", format!("{first}{last}")),
        choose("Choose Last", last.to_owned()),
    ];
    d
}

/// Whether the language is a regular expression language (its grammar is
/// the Oniguruma regex grammar).
pub fn is_regex_language(language: &LanguageDef) -> bool {
    language.id == "regex" || language.grammar == "source.regexp.oniguruma"
}

/// Checks that the text compiles as a regular expression (the `regex`
/// crate's dialect). The error is placed at the offending part of the
/// pattern when the engine reports one.
pub fn check_regex(text: &str) -> Vec<Diagnostic> {
    let pattern = text.trim_end_matches(['\n', '\r']);
    if pattern.trim().is_empty() {
        return Vec::new();
    }
    let err = match regex::Regex::new(pattern) {
        Ok(_) => return Vec::new(),
        Err(e) => e,
    };
    let formatted = err.to_string();
    let message = formatted
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("error: "))
        .map_or_else(|| formatted.lines().last().unwrap_or("invalid regex"), str::trim)
        .to_owned();
    let mut d = Diagnostic::new(Severity::Error, message);
    d.message_id = Some("regex".to_owned());
    // The engine prints the offending line and a caret line below it,
    // indented by four spaces.
    let lines: Vec<&str> = formatted.lines().collect();
    let located = lines.iter().enumerate().rev().find_map(|(i, l)| {
        let pre = l.len() - l.trim_start().len();
        let carets = l.trim();
        (i > 0 && pre >= 4 && !carets.is_empty() && carets.chars().all(|c| c == '^'))
            .then(|| (lines[i - 1], pre - 4, carets.len()))
    });
    let index = LineIndex::new(text);
    d.location = Some(match located {
        Some((pat_line, offset, len)) => {
            let shown = pat_line.get(4..).unwrap_or(pat_line);
            let line = (0..index.line_count())
                .find(|n| index.line_text(*n) == Some(shown))
                .unwrap_or(0);
            Location::Lines {
                line: u32::try_from(line + 1).unwrap_or(1),
                column: Some(u32::try_from(offset + 1).unwrap_or(1)),
                end_line: None,
                end_column: Some(u32::try_from(offset + 1 + len).unwrap_or(1)),
                unit: ColumnUnit::Chars,
            }
        }
        None => Location::line(1),
    });
    vec![d]
}

/// Whether [`check_syntax`] knows the language (JSON, TOML, XML).
pub fn has_syntax_checker(language: &LanguageDef) -> bool {
    matches!(language.id.as_str(), "json" | "toml" | "xml")
}

/// Syntax errors of a JSON, TOML or XML document (at most one: parsers stop
/// at the first error). Other languages produce nothing.
pub fn check_syntax(language: &LanguageDef, text: &str) -> Vec<Diagnostic> {
    match language.id.as_str() {
        "json" => check_json(text),
        "toml" => check_toml(text),
        "xml" => check_xml(text),
        _ => Vec::new(),
    }
}

fn syntax_error(message: String, location: Location) -> Vec<Diagnostic> {
    let mut d = Diagnostic::new(Severity::Error, message).at(location);
    d.message_id = Some("syntax".to_owned());
    vec![d]
}

fn check_json(text: &str) -> Vec<Diagnostic> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<serde::de::IgnoredAny>(text) {
        Ok(_) => Vec::new(),
        Err(e) => {
            let full = e.to_string();
            let message = full
                .split(" at line ")
                .next()
                .unwrap_or(&full)
                .to_owned();
            let line = u32::try_from(e.line().max(1)).unwrap_or(1);
            let column = u32::try_from(e.column()).ok().filter(|c| *c > 0);
            syntax_error(
                message,
                Location::Lines {
                    line,
                    column,
                    end_line: None,
                    end_column: None,
                    unit: ColumnUnit::Bytes,
                },
            )
        }
    }
}

fn check_toml(text: &str) -> Vec<Diagnostic> {
    match text.parse::<toml::Table>() {
        Ok(_) => Vec::new(),
        Err(e) => {
            let message = e.message().lines().next().unwrap_or("invalid TOML").to_owned();
            let location = match e.span() {
                Some(span) => Location::Offsets {
                    offset: span.start,
                    length: span.end.saturating_sub(span.start).max(1),
                    bytes: true,
                },
                None => Location::line(1),
            };
            syntax_error(message, location)
        }
    }
}

fn check_xml(text: &str) -> Vec<Diagnostic> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<(String, usize)> = Vec::new();
    loop {
        let before = usize::try_from(reader.buffer_position()).unwrap_or(0);
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                stack.push((String::from_utf8_lossy(e.name().as_ref().as_bytes()).into_owned(), before));
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) => {
                return match stack.pop() {
                    Some((name, at)) => syntax_error(
                        format!("Unclosed tag <{name}>"),
                        Location::Offsets {
                            offset: at,
                            length: name.len() + 1,
                            bytes: true,
                        },
                    ),
                    None => Vec::new(),
                };
            }
            Ok(_) => {}
            Err(e) => {
                return syntax_error(
                    e.to_string(),
                    Location::Offsets {
                        offset: before.min(text.len()),
                        length: 1,
                        bytes: true,
                    },
                );
            }
        }
    }
}

/// Runs the built-in checker of `kind`; other kinds produce nothing.
pub fn run_builtin(kind: DiagnosticKind, language: &LanguageDef, text: &str) -> Vec<Diagnostic> {
    match kind {
        DiagnosticKind::Conflict => check_conflicts(text),
        DiagnosticKind::RegexCheck => check_regex(text),
        DiagnosticKind::InvalidSyntax => check_syntax(language, text),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{AnnotationSet, DocumentContext, resolve};
    use crate::model::Origin;
    use scratch_language::LanguageRegistry;

    const CONFLICT: &str = "keep\n<<<<<<< HEAD\nmine 1\nmine 2\n=======\ntheirs\n>>>>>>> feature\ntail\n";

    #[test]
    fn conflict_with_three_fixes() {
        let d = check_conflicts(CONFLICT);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, CONFLICT_MESSAGE);
        assert_eq!(d[0].location, Some(Location::line(2)));
        let titles: Vec<&str> = d[0].fixes.iter().map(Fix::display_title).collect();
        assert_eq!(titles, ["Choose First", "Choose Both", "Choose Last"]);
        let ctx = DocumentContext::new(CONFLICT);
        let a = resolve(&d[0], &ctx);
        let mut set = AnnotationSet::new();
        set.replace_source("builtin-conflict", Origin::Document, vec![a.clone()]);
        let id = a.id;
        assert_eq!(
            set.fix_plan(id, 0).unwrap().apply_to(CONFLICT),
            "keep\nmine 1\nmine 2\ntail\n"
        );
        assert_eq!(
            set.fix_plan(id, 1).unwrap().apply_to(CONFLICT),
            "keep\nmine 1\nmine 2\ntheirs\ntail\n"
        );
        assert_eq!(set.fix_plan(id, 2).unwrap().apply_to(CONFLICT), "keep\ntheirs\ntail\n");
        assert!(set.all_fixes().is_empty(), "choices are never applied in bulk");
    }

    #[test]
    fn conflict_edge_cases() {
        assert!(check_conflicts("a\n=======\nb\n").is_empty(), "setext underline is not a conflict");
        assert!(check_conflicts("<<<< not enough\n").is_empty());
        let open = check_conflicts("<<<<<<< HEAD\nx\n=======\ny\n");
        assert_eq!(open.len(), 1);
        assert!(open[0].fixes.is_empty());
        let two = format!("{CONFLICT}{CONFLICT}");
        assert_eq!(check_conflicts(&two).len(), 2);
        let diff3 = "<<<<<<< a\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> b\n";
        let d = check_conflicts(diff3);
        assert_eq!(d.len(), 1);
        let ctx = DocumentContext::new(diff3);
        let a = resolve(&d[0], &ctx);
        let mut set = AnnotationSet::new();
        set.replace_source("c", Origin::Document, vec![a.clone()]);
        assert_eq!(set.fix_plan(a.id, 0).unwrap().apply_to(diff3), "ours\n");
        let crlf = CONFLICT.replace('\n', "\r\n");
        let d = check_conflicts(&crlf);
        assert_eq!(d.len(), 1);
        let a = resolve(&d[0], &DocumentContext::new(&crlf));
        let mut set = AnnotationSet::new();
        set.replace_source("c", Origin::Document, vec![a.clone()]);
        assert_eq!(set.fix_plan(a.id, 2).unwrap().apply_to(&crlf), "keep\r\ntheirs\r\ntail\r\n");
    }

    #[test]
    fn regex_check() {
        assert!(check_regex("^(?<file>.*):(\\d+)$\n").is_empty());
        assert!(check_regex("").is_empty());
        let d = check_regex("abc(def\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "unclosed group");
        assert!(matches!(d[0].location, Some(Location::Lines { line: 1, column: Some(4), .. })), "{:?}", d[0].location);
        let d = check_regex("*a");
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn json_toml_xml_syntax() {
        let reg = LanguageRegistry::builtin();
        let json = reg.get("json").unwrap();
        assert!(check_syntax(json, "{\"a\": [1, 2]}").is_empty());
        let d = check_syntax(json, "{\n  \"a\": [1, 2,]\n}");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].location.as_ref().and_then(Location::start_line), Some(2));
        let toml = reg.get("toml").unwrap();
        assert!(check_syntax(toml, "a = 1\n[b]\nc = \"x\"\n").is_empty());
        let d = check_syntax(toml, "a = 1\nb = \n");
        assert_eq!(d.len(), 1);
        let xml = reg.get("xml").unwrap();
        assert!(check_syntax(xml, "<?xml version=\"1.0\"?><a><b/></a>").is_empty());
        let d = check_syntax(xml, "<a><b></a>");
        assert_eq!(d.len(), 1, "{d:?}");
        let d = check_syntax(xml, "<a><b>");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("Unclosed"));
        assert!(check_syntax(reg.get("c").unwrap(), "int main(").is_empty());
        assert!(has_syntax_checker(json) && !has_syntax_checker(reg.get("c").unwrap()));
    }
}
