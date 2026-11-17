//! "Text (Unix/GCC style messages)" mode: `file:line:col: message` lines as
//! printed by clang, gcc, swiftc, javac, go, rustc (`--error-format=short`),
//! shellcheck (`-f gcc`), node, ruby and many more.
//!
//! Beyond the bare lines this parser understands:
//! * `note:` lines, which become children of the preceding diagnostic;
//! * `fix-it:"file":{l:c-l:c}:"text"` lines (clang
//!   `-fdiagnostics-parseable-fixits`), which become [`Fix`]es of the
//!   diagnostic or note they follow;
//! * the source line and `~~~^~~~` caret line some compilers print under a
//!   message, used for the column (javac) and the end of the range;
//! * javac's `symbol:` / `location:` detail lines (children);
//! * bash's `file: line N: message` form;
//! * `error:` lines without a location (`clang: error: linker command
//!   failed`), as location-less diagnostics.

use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

use super::infer::infer;
use super::{ParseOptions, scope_for};
use crate::model::{ColumnUnit, Diagnostic, Fix, Location, MatchedLocation, Severity, TextEdit};

/// CodeRunner's default output regex (documented in report 05).
pub const UNIX_REGEX: &str =
    r"^(?<file>[^\s].*?):(?<line>\d+):(?<column>\d+)?:? (?<message>.*)$";

fn line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(UNIX_REGEX).expect("static regex"))
}

fn bash_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?<file>[^\s].*?): line (?<line>\d+): (?<message>.*)$").expect("static regex")
    })
}

fn nolocation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:[\w+.\-]+: )?(?<message>(?:fatal )?error(?:\[[^\]]+\])?: .*)$")
            .expect("static regex")
    })
}

fn caret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?<pre> *)(?<marks>[~^][~^ ]*)$").expect("static regex"))
}

fn pipe_source_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\d+ \| ?(?<src>.*)$").expect("static regex"))
}

fn pipe_caret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\| ?(?<pre> *)(?<marks>[~^][~^ ]*)$").expect("static regex"))
}

fn fixit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^fix-it:"(?<file>(?:[^"\\]|\\.)*)":\{(?<l1>\d+):(?<c1>\d+)-(?<l2>\d+):(?<c2>\d+)\}:"(?<text>(?:[^"\\]|\\.)*)"$"#)
            .expect("static regex")
    })
}

fn detail_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s+(?:symbol|location|required|found|reason|actual|expected)\s*:\s*\S")
            .expect("static regex")
    })
}

/// Lines that are noise rather than diagnostics.
fn is_noise(message: &str) -> bool {
    let m = message.trim_start();
    m.starts_with("aborting due to")
        || m.starts_with("could not compile")
        || m.starts_with("build failed")
        || (m.chars().next().is_some_and(|c| c.is_ascii_digit())
            && (m.contains(" warning") || m.contains(" error"))
            && m.contains("generated"))
}

/// Cleans a file name captured by the line regex: `lua: x.lua` -> `x.lua`;
/// returns `None` for things that are not paths (`Time: 12`).
fn clean_file(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let candidate = match raw.rfind(": ") {
        Some(i) => &raw[i + 2..],
        None => raw,
    };
    let candidate = candidate.trim_matches(|c| c == '"' || c == '\'');
    if candidate.is_empty() || candidate.contains(char::is_whitespace) && !candidate.contains('/') {
        return None;
    }
    let pathish = candidate == "-"
        || candidate.contains('/')
        || candidate.contains('\\')
        || candidate
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty());
    pathish.then(|| candidate.to_owned())
}

struct Line<'a> {
    start: usize,
    text: &'a str,
}

fn split_lines(output: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for part in output.split_inclusive('\n') {
        let text = part.trim_end_matches(['\n', '\r']);
        lines.push(Line { start, text });
        start += part.len();
    }
    lines
}

/// Unescapes clang's fix-it text (`\\`, `\"`, `\ooo` octal bytes).
fn unescape_fixit(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n.is_ascii_digit()
                && i + 4 <= bytes.len()
                && bytes[i + 1..i + 4].iter().all(u8::is_ascii_digit)
            {
                let v = (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
                out.push(v);
                i += 4;
                continue;
            }
            out.push(n);
            i += 2;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_fixit(line: &str) -> Option<TextEdit> {
    let c = fixit_re().captures(line)?;
    let num = |n: &str| c[n].parse::<u32>().ok();
    Some(TextEdit {
        location: Location::Lines {
            line: num("l1")?,
            column: Some(num("c1")?),
            end_line: Some(num("l2")?),
            end_column: Some(num("c2")?),
            unit: ColumnUnit::Bytes,
        },
        replacement: unescape_fixit(&c["text"]),
    })
}

/// A diagnostic line matched by one of the line forms.
fn match_line(line: &str, opts: &ParseOptions) -> Option<Diagnostic> {
    let (file, line_no, column, message) = if let Some(c) = line_re().captures(line) {
        (
            clean_file(&c["file"])?,
            c["line"].parse::<u32>().ok()?,
            c.name("column").and_then(|m| m.as_str().parse::<u32>().ok()),
            c["message"].to_owned(),
        )
    } else {
        let c = bash_re().captures(line)?;
        (
            clean_file(&c["file"])?,
            c["line"].parse::<u32>().ok()?,
            None,
            c["message"].to_owned(),
        )
    };
    if line_no == 0 {
        return None;
    }
    let mut message = message.trim().to_owned();
    // rustc short format: "2:6 error: ..." carries the end position.
    let mut end: Option<(u32, u32)> = None;
    static END_RE: OnceLock<Regex> = OnceLock::new();
    let end_re = END_RE.get_or_init(|| Regex::new(r"^(\d+):(\d+) (.*)$").expect("static regex"));
    if let Some(c) = end_re.captures(&message) {
        let rest = c[3].to_owned();
        if infer(&rest).severity.is_some() {
            end = c[1].parse().ok().zip(c[2].parse().ok());
            message = rest;
        }
    }
    let inferred = infer(&message);
    let severity = inferred.severity.unwrap_or(Severity::Error);
    let mut d = Diagnostic::new(severity, inferred.message);
    d.message_id = inferred.message_id;
    d.file = Some(PathBuf::from(file));
    d.location = Some(Location::Lines {
        line: line_no,
        column,
        end_line: end.map(|e| e.0),
        end_column: end.map(|e| e.1),
        unit: ColumnUnit::Bytes,
    });
    d.origin = opts.origin;
    Some(d)
}

fn set_matched(d: &mut Diagnostic, start: usize, end: usize, opts: &ParseOptions) {
    d.matched.push(MatchedLocation {
        range: start..end,
        link: Some(String::new()),
        scopes: vec![scope_for(d.severity, d.runtime, opts.language.as_deref())],
    });
}

/// Uses a source line + caret line under a diagnostic to refine its
/// location. Returns the refined location.
fn refine_with_caret(loc: &Location, source: &str, caret: &regex::Captures<'_>) -> Location {
    let Location::Lines {
        line,
        column,
        end_line,
        end_column,
        unit,
    } = loc
    else {
        return loc.clone();
    };
    let pre = caret["pre"].chars().count();
    let marks = &caret["marks"];
    let last = pre + marks.trim_end().chars().count(); // exclusive end (0-based char idx)
    let first_caret = pre + marks.chars().position(|c| c == '^').unwrap_or(0);
    let plain = source.is_ascii();
    match column {
        None => Location::Lines {
            line: *line,
            column: Some(u32::try_from(first_caret + 1).unwrap_or(1)),
            end_line: *end_line,
            end_column: (last > first_caret + 1)
                .then(|| u32::try_from(last + 1).unwrap_or(1)),
            unit: ColumnUnit::Chars,
        },
        Some(col) if end_column.is_none() && plain && !source.contains('\t') => {
            let end = u32::try_from(last + 1).unwrap_or(1);
            // The range may start before the caret (`~~^~~`).
            let start = u32::try_from(pre + 1).unwrap_or(*col).min(*col);
            Location::Lines {
                line: *line,
                column: Some(start),
                end_line: *end_line,
                end_column: (end > start + 1).then_some(end),
                unit: *unit,
            }
        }
        _ => loc.clone(),
    }
}

/// Finds the source line and caret line printed under the diagnostic at
/// `lines[i]`: either the classic two lines (source, `~~^~~`) or the
/// `    3 | source` / `      |    ^` style of current clang.
fn caret_block<'a>(
    lines: &[Line<'a>],
    i: usize,
    opts: &ParseOptions,
) -> Option<(&'a str, regex::Captures<'a>)> {
    let (src, car) = (lines.get(i + 1)?, lines.get(i + 2)?);
    if let (Some(s), Some(c)) = (pipe_source_re().captures(src.text), pipe_caret_re().captures(car.text))
        && c["marks"].contains('^')
    {
        let text = s.name("src")?;
        return Some((&src.text[text.start()..text.end()], c));
    }
    if match_line(src.text, opts).is_none()
        && parse_fixit(src.text).is_none()
        && !src.text.trim().is_empty()
        && let Some(c) = caret_re().captures(car.text)
        && c["marks"].contains('^')
    {
        return Some((src.text, c));
    }
    None
}

/// Parses Unix style output; see the module docs.
pub fn parse_unix(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    let lines = split_lines(output);
    let mut out: Vec<Diagnostic> = Vec::new();
    // Index of the last non-note diagnostic, and the (main, child) position
    // of the last emitted node (target of fix-it lines).
    let mut last_main: Option<usize> = None;
    let mut target: Option<(usize, Option<usize>)> = None;
    let mut pending_fix: Option<(usize, Option<usize>, Fix)> = None;

    let flush_fix = |out: &mut Vec<Diagnostic>, pending: &mut Option<(usize, Option<usize>, Fix)>| {
        if let Some((m, c, fix)) = pending.take() {
            let node = match c {
                Some(c) => &mut out[m].children[c],
                None => &mut out[m],
            };
            node.fixes.push(fix);
        }
    };

    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if let Some(edit) = parse_fixit(line.text) {
            if let Some((m, c)) = target {
                match &mut pending_fix {
                    Some((pm, pc, fix)) if *pm == m && *pc == c => fix.edits.push(edit),
                    _ => {
                        flush_fix(&mut out, &mut pending_fix);
                        pending_fix = Some((
                            m,
                            c,
                            Fix {
                                title: None,
                                description: None,
                                edits: vec![edit],
                                preferred: false,
                            },
                        ));
                    }
                }
            }
            i += 1;
            continue;
        }
        let matched = match_line(line.text, opts);
        let (mut d, location_less) = match matched {
            Some(d) => (d, false),
            None => {
                if let Some(c) = nolocation_re().captures(line.text) {
                    let inferred = infer(&c["message"]);
                    if is_noise(&inferred.message) {
                        i += 1;
                        continue;
                    }
                    let mut d = Diagnostic::new(
                        inferred.severity.unwrap_or(Severity::Error),
                        inferred.message,
                    );
                    d.message_id = inferred.message_id;
                    d.origin = opts.origin;
                    (d, true)
                } else {
                    i += 1;
                    continue;
                }
            }
        };
        if is_noise(&d.message) {
            i += 1;
            continue;
        }
        flush_fix(&mut out, &mut pending_fix);
        set_matched(&mut d, line.start, line.start + line.text.len(), opts);
        let mut consumed = 1;

        // bash continuation: "x.sh: line 5: `fi'" belongs to the previous message.
        if d.message.starts_with('`')
            && let Some(m) = last_main
            && out[m].file == d.file
        {
            i += 1;
            continue;
        }

        // Source line + caret line (classic or `N | ` style).
        if !location_less
            && let Some((src, caps)) = caret_block(&lines, i, opts)
        {
            if let Some(loc) = &d.location {
                d.location = Some(refine_with_caret(loc, src, &caps));
            }
            consumed = 3;
        }
        // javac style detail lines.
        let mut details = Vec::new();
        while let Some(next) = lines.get(i + consumed) {
            if detail_re().is_match(next.text) {
                details.push(next.text.trim().to_owned());
                consumed += 1;
            } else {
                break;
            }
        }
        for text in details {
            let mut child = Diagnostic::new(Severity::Notice, text);
            child.origin = opts.origin;
            d.children.push(child);
        }
        i += consumed;

        let is_note = d.severity == Severity::Notice
            && !location_less
            && last_main.is_some()
            && d.message_id.is_none()
            && line.text.contains("note:");
        if is_note && let Some(m) = last_main {
            out[m].children.push(d);
            target = Some((m, Some(out[m].children.len() - 1)));
        } else {
            out.push(d);
            last_main = Some(out.len() - 1);
            target = Some((out.len() - 1, None));
        }
    }
    flush_fix(&mut out, &mut pending_fix);
    // Fix-its printed after a note (`place parentheses around ...`) fix the
    // diagnostic itself: move them up, keeping the note's text as the
    // description.
    for d in &mut out {
        let mut hoisted = Vec::new();
        for c in &mut d.children {
            for mut f in std::mem::take(&mut c.fixes) {
                f.description = Some(c.message.clone());
                hoisted.push(f);
            }
        }
        d.fixes.extend(hoisted);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Vec<Diagnostic> {
        parse_unix(text, &ParseOptions::default())
    }

    #[test]
    fn clang_with_notes_and_fixits() {
        let out = "\
In file included from main.c:1:
main.c:5:12: error: expected ';' after expression
main.c:5:12: note: insert ';' here
fix-it:\"main.c\":{5:12-5:12}:\";\"
main.c:7:9: warning: unused variable 'x' [-Wunused-variable]
2 diagnostics generated.
";
        let d = p(out);
        assert_eq!(d.len(), 2, "{d:#?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].message, "expected ';' after expression");
        assert_eq!(d[0].children.len(), 1);
        assert_eq!(d[0].children[0].severity, Severity::Notice);
        assert_eq!(d[0].children[0].message, "insert ';' here");
        assert!(d[0].children[0].fixes.is_empty(), "fixes of notes belong to the diagnostic");
        assert_eq!(d[0].fixes.len(), 1);
        assert_eq!(d[0].fixes[0].description.as_deref(), Some("insert ';' here"));
        assert_eq!(d[0].fixes[0].edits[0].replacement, ";");
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[1].message_id.as_deref(), Some("-Wunused-variable"));
        assert_eq!(d[1].location, Some(Location::line_col(7, 9)));
        assert_eq!(d[0].matched.len(), 1);
    }

    #[test]
    fn caret_gives_range_and_javac_column() {
        let clang = "a.c:3:7: error: use of undeclared identifier 'foo'\n  x = foo(1);\n      ^~~\n";
        let d = p(clang);
        assert_eq!(
            d[0].location,
            Some(Location::Lines {
                line: 3,
                column: Some(7),
                end_line: None,
                end_column: Some(10),
                unit: ColumnUnit::Bytes
            })
        );
        let javac = "Main.java:5: error: cannot find symbol\n        foo();\n        ^\n  symbol:   method foo()\n  location: class Main\n1 error\n";
        let d = p(javac);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "cannot find symbol");
        assert_eq!(
            d[0].location,
            Some(Location::Lines {
                line: 5,
                column: Some(9),
                end_line: None,
                end_column: None,
                unit: ColumnUnit::Chars
            })
        );
        assert_eq!(d[0].children.len(), 2);
        assert_eq!(d[0].children[0].message, "symbol:   method foo()");
    }

    #[test]
    fn go_and_shellcheck_and_lua() {
        let go = "# command-line-arguments\n./main.go:6:2: undefined: x\n./main.go:7:5: declared and not used: y\n";
        let d = p(go);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("./main.go"));
        assert_eq!(d[0].message, "undefined: x");
        let sc = "x.sh:3:8: warning: Quote this to prevent word splitting. [SC2086]\nx.sh:5:1: note: Use ./* so names with dashes won't become options. [SC2035]\nx.sh:9:1: error: Couldn't parse this if expression. [SC1073]\n";
        let d = p(sc);
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!(d[0].message_id.as_deref(), Some("SC2086"));
        assert_eq!(d[0].severity, Severity::Warning);
        // ShellCheck notes carry an id: they are diagnostics of their own.
        assert!(d[0].children.is_empty());
        assert_eq!(d[1].severity, Severity::Notice);
        assert_eq!(d[2].severity, Severity::Error);
        let lua = "lua: x.lua:5: attempt to call a nil value (global 'foo')\nstack traceback:\n\t[C]: in ?\n";
        let d = p(lua);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("x.lua"));
        assert_eq!(d[0].location, Some(Location::line(5)));
    }

    #[test]
    fn bash_syntax_check_and_noise() {
        let out = "x.sh: line 4: syntax error near unexpected token `fi'\nx.sh: line 4: `fi'\n";
        let d = p(out);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].location, Some(Location::line(4)));
        assert!(d[0].message.starts_with("syntax error"));
        let d = p("Time: 12:30: hello\nbuild took 12:30:15 ok\n3 warnings generated.\n");
        assert!(d.is_empty(), "{d:#?}");
    }

    #[test]
    fn linker_error_has_no_location() {
        let d = p("ld: symbol(s) not found for architecture arm64\nclang: error: linker command failed with exit code 1 (use -v to see invocation)\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].location.is_none());
        assert!(d[0].message.starts_with("linker command failed"));
    }

    #[test]
    fn rustc_short_format_with_end_position() {
        let d = p("src/main.rs:2:5: 2:6 error[E0425]: cannot find value `x` in this scope\nerror: aborting due to 1 previous error\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message_id.as_deref(), Some("E0425"));
        assert_eq!(
            d[0].location,
            Some(Location::Lines {
                line: 2,
                column: Some(5),
                end_line: Some(2),
                end_column: Some(6),
                unit: ColumnUnit::Bytes
            })
        );
    }

    #[test]
    fn fixit_unescape_and_grouping() {
        let out = "a.c:1:1: warning: x\nfix-it:\"a.c\":{1:1-1:1}:\"(\"\nfix-it:\"a.c\":{1:5-1:5}:\")\\012\"\n";
        let d = p(out);
        assert_eq!(d[0].fixes.len(), 1);
        assert_eq!(d[0].fixes[0].edits.len(), 2);
        assert_eq!(d[0].fixes[0].edits[1].replacement, ")\n");
    }
}
