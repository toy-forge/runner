//! Stack-trace style output: Python tracebacks, Node, Rust panics, Go
//! panics, Java exceptions, Ruby backtraces. Each failure becomes one
//! run-time error placed at the innermost *user* frame, with the callers as
//! children ("'f' called here").

use std::ops::Range;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

use super::{ParseOptions, scope_for};
use crate::model::{ColumnUnit, Diagnostic, Location, MatchedLocation, Severity};

/// One frame of a stack.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub unit: ColumnUnit,
    /// The function name of this frame.
    pub func: Option<String>,
    /// Where the frame was printed (byte range in the parsed text).
    pub range: Range<usize>,
    /// Precise location override (Python carets).
    pub location: Option<Location>,
}

impl Frame {
    fn location(&self) -> Location {
        self.location.clone().unwrap_or(Location::Lines {
            line: self.line,
            column: self.column,
            end_line: None,
            end_column: None,
            unit: self.unit,
        })
    }
}

pub(crate) fn lines_with_offsets(output: &str) -> Vec<(usize, &str)> {
    let mut start = 0;
    output
        .split_inclusive('\n')
        .map(|part| {
            let item = (start, part.trim_end_matches(['\n', '\r']));
            start += part.len();
            item
        })
        .collect()
}

pub(crate) fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn call_name(func: Option<&str>) -> String {
    match func.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) if f.contains("<anonymous>") || f.starts_with("<lambda") => {
            "anonymous function".to_owned()
        }
        Some(f) => f.to_owned(),
        None => "anonymous function".to_owned(),
    }
}

/// Builds the diagnostic of a failure: `frames` are innermost first and
/// already filtered to user code; the first frame is the location of the
/// diagnostic and the others become "called here" children. `range` is the
/// text of the error message in the output.
pub(crate) fn failure(
    message: String,
    message_id: Option<String>,
    frames: &[Frame],
    range: Range<usize>,
    opts: &ParseOptions,
) -> Diagnostic {
    let mut d = Diagnostic::new(Severity::Error, message);
    d.runtime = true;
    d.message_id = message_id;
    d.origin = opts.origin;
    if let Some(first) = frames.first() {
        d.file = Some(PathBuf::from(&first.file));
        d.location = Some(first.location());
    }
    d.matched.push(MatchedLocation {
        range,
        link: Some(String::new()),
        scopes: vec![scope_for(Severity::Error, true, opts.language.as_deref())],
    });
    for pair in frames.windows(2) {
        let (callee, caller) = (&pair[0], &pair[1]);
        let mut c = Diagnostic::new(
            Severity::Notice,
            format!("'{}' called here", call_name(callee.func.as_deref())),
        );
        c.runtime = true;
        c.origin = opts.origin;
        c.file = Some(PathBuf::from(&caller.file));
        c.location = Some(caller.location());
        c.matched.push(MatchedLocation {
            range: caller.range.clone(),
            link: Some(String::new()),
            scopes: vec![scope_for(Severity::Notice, true, opts.language.as_deref())],
        });
        d.children.push(c);
    }
    d
}

// ---------------------------------------------------------------- Python

fn is_python_library(file: &str) -> bool {
    file.starts_with('<')
        || file.contains("/site-packages/")
        || file.contains("/dist-packages/")
        || file.contains("/lib/python")
        || file.contains("/Lib/")
        || file.contains("/Python.framework/")
}

/// Python tracebacks and syntax errors.
pub fn parse_python(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static FRAME: OnceLock<Regex> = OnceLock::new();
    static EXC: OnceLock<Regex> = OnceLock::new();
    static CARET: OnceLock<Regex> = OnceLock::new();
    let frame_re = re(&FRAME, r#"^\s+File "(?<file>.+)", line (?<line>\d+)(?:, in (?<func>.+))?$"#);
    let exc_re = re(&EXC, r"^(?<name>[A-Za-z_][\w.]*)(?::\s?(?<msg>.*))?$");
    let caret_re = re(&CARET, r"^(?<pre> +)(?<marks>[\^~]+)\s*$");
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (_, text) = lines[i];
        let in_traceback = text.starts_with("Traceback (most recent call last):");
        if !in_traceback && !(frame_re.is_match(text)) {
            i += 1;
            continue;
        }
        if in_traceback {
            i += 1;
        }
        let mut frames: Vec<Frame> = Vec::new();
        while i < lines.len() {
            let (start, text) = lines[i];
            if let Some(c) = frame_re.captures(text) {
                let mut frame = Frame {
                    file: c["file"].to_owned(),
                    line: c["line"].parse().unwrap_or(1),
                    column: None,
                    unit: ColumnUnit::Chars,
                    func: c.name("func").map(|m| m.as_str().to_owned()),
                    range: start..start + text.len(),
                    location: None,
                };
                i += 1;
                // Source line and optional caret line.
                let mut source: Option<&str> = None;
                while i < lines.len() {
                    let t = lines[i].1;
                    if frame_re.is_match(t) || t.trim().is_empty() || !t.starts_with(' ') {
                        break;
                    }
                    if let Some(cc) = caret_re.captures(t) {
                        if let Some(src) = source {
                            let pre = cc["pre"].len();
                            let marks = cc["marks"].chars().count();
                            let offset = pre.saturating_sub(4);
                            // Python indents the stripped source by 4 columns.
                            frame.location = Some(Location::InLine {
                                line: frame.line,
                                text: src.to_owned(),
                                offset,
                                length: marks,
                            });
                        }
                    } else if source.is_none() {
                        source = Some(t.trim());
                    }
                    i += 1;
                }
                frames.push(frame);
            } else if text.starts_with(' ') || text.trim().is_empty() && in_traceback && frames.is_empty() {
                i += 1;
            } else {
                break;
            }
        }
        // The exception line.
        let Some((start, text)) = lines.get(i).copied() else {
            break;
        };
        let Some(c) = exc_re.captures(text) else {
            continue;
        };
        let name = c["name"].to_owned();
        let known = ["Error", "Exception", "Exit", "Interrupt", "Warning", "Iteration"]
            .iter()
            .any(|s| name.ends_with(s));
        if !known {
            continue;
        }
        i += 1;
        let message = match c.name("msg").map(|m| m.as_str().trim()).filter(|m| !m.is_empty()) {
            Some(m) => format!("{name}: {m}"),
            None => name.clone(),
        };
        let is_syntax = name.ends_with("SyntaxError") || name.ends_with("IndentationError") || name.ends_with("TabError");
        // A syntax error is a compile-time problem even when it is raised
        // from `compile()` or an import inside a traceback.
        let runtime = in_traceback && !is_syntax;
        // Innermost user frame first.
        let mut user: Vec<Frame> = frames
            .iter()
            .filter(|f| !is_python_library(&f.file))
            .cloned()
            .collect();
        if user.is_empty() {
            user = frames.last().cloned().into_iter().collect();
        }
        user.reverse();
        // Only the innermost frame carries a caret location.
        for f in user.iter_mut().skip(1) {
            f.location = None;
        }
        if is_syntax && let Some(last) = frames.last() {
            // A syntax error is located at the frame it was reported for,
            // not at the importing user frame.
            if user.first().is_none_or(|u| u.file != last.file || u.line != last.line) {
                user.insert(0, last.clone());
            }
        }
        let mut d = failure(message, None, &user, start..start + text.len(), opts);
        d.runtime = runtime;
        for c in &mut d.children {
            c.runtime = runtime;
        }
        out.push(d);
    }
    out
}

// ------------------------------------------------------------------ Node

fn is_node_internal(file: &str) -> bool {
    file.starts_with("node:") || file.contains("node_modules") || file.contains("internal/")
}

/// `file`, `line` and the caret (1-based column, number of carets) of a Node
/// error header.
type NodeHeader = (String, u32, Option<(u32, usize)>);

/// Node.js uncaught errors and `node --check` syntax errors.
pub fn parse_node(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    static CARET: OnceLock<Regex> = OnceLock::new();
    static ERR: OnceLock<Regex> = OnceLock::new();
    static FRAME: OnceLock<Regex> = OnceLock::new();
    let header_re = re(&HEADER, r"^(?<file>(?:file://)?/.*?|[A-Za-z]:[\\/].*?):(?<line>\d+)$");
    let caret_re = re(&CARET, r"^(?<pre>\s*)(?<marks>\^+)\s*$");
    let err_re = re(
        &ERR,
        r"^(?:Uncaught )?(?<name>(?:[A-Za-z_$][\w$]*)?(?:Error|Exception))(?: \[(?<code>[\w]+)\])?(?::\s?(?<msg>.*))?$",
    );
    let frame_re = re(
        &FRAME,
        r"^\s+at (?:(?<func>.+?) \()?(?<file>(?:file://)?[^()\s]+?):(?<line>\d+):(?<col>\d+)\)?$",
    );
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (_, text) = lines[i];
        let mut header: Option<NodeHeader> = None;
        let mut j = i;
        if let Some(c) = header_re.captures(text) {
            let file = c["file"].to_owned();
            let line: u32 = c["line"].parse().unwrap_or(1);
            // Source line, then caret line.
            let mut caret = None;
            if let Some((_, c2)) = lines.get(i + 2).copied()
                && let Some(cc) = caret_re.captures(c2)
            {
                caret = Some((
                    u32::try_from(cc["pre"].chars().count() + 1).unwrap_or(1),
                    cc["marks"].chars().count(),
                ));
            }
            header = Some((file, line, caret));
            j = i + 1;
            // Skip to the error line (within a few lines).
            let limit = (i + 8).min(lines.len());
            while j < limit && !err_re.is_match(lines[j].1) {
                j += 1;
            }
        }
        let Some((estart, etext)) = lines.get(j).copied() else {
            break;
        };
        let Some(ec) = err_re.captures(etext) else {
            i += 1;
            continue;
        };
        if header.is_none() && !lines.get(j + 1).is_some_and(|l| frame_re.is_match(l.1)) {
            i += 1;
            continue;
        }
        // Stack frames.
        let mut k = j + 1;
        let mut frames: Vec<Frame> = Vec::new();
        while k < lines.len() {
            let (start, t) = lines[k];
            if let Some(fc) = frame_re.captures(t) {
                let file = fc["file"].to_owned();
                if !is_node_internal(&file) {
                    frames.push(Frame {
                        file: file.strip_prefix("file://").unwrap_or(&file).to_owned(),
                        line: fc["line"].parse().unwrap_or(1),
                        column: fc["col"].parse().ok(),
                        unit: ColumnUnit::Chars,
                        func: fc.name("func").map(|m| m.as_str().to_owned()),
                        range: start..start + t.len(),
                        location: None,
                    });
                }
                k += 1;
            } else if t.trim_start().starts_with("at ") {
                k += 1;
            } else {
                break;
            }
        }
        let name = ec["name"].to_owned();
        let message = match ec.name("msg").map(|m| m.as_str().trim()).filter(|m| !m.is_empty()) {
            Some(m) => format!("{name}: {m}"),
            None => name.clone(),
        };
        let code = ec.name("code").map(|m| m.as_str().to_owned());
        if let Some((file, line, caret)) = header {
            let file = file.strip_prefix("file://").unwrap_or(&file).to_owned();
            // The header location is the precise one; stack frames after the
            // first (throw site) are the callers.
            let first = Frame {
                file: file.clone(),
                line,
                column: caret.map(|c| c.0),
                unit: ColumnUnit::Chars,
                func: frames.first().and_then(|f| f.func.clone()),
                range: lines[i].0..lines[i].0 + lines[i].1.len(),
                location: caret.map(|(col, len)| Location::Lines {
                    line,
                    column: Some(col),
                    end_line: None,
                    // A single caret is a pointer, not a range.
                    end_column: (len > 1).then(|| col + u32::try_from(len).unwrap_or(1)),
                    unit: ColumnUnit::Chars,
                }),
            };
            let mut chain = vec![first];
            let skip = usize::from(frames.first().is_some_and(|f| f.line == line && f.file == file));
            chain.extend(frames.into_iter().skip(skip));
            let mut d = failure(message, code, &chain, estart..estart + etext.len(), opts);
            d.runtime = name != "SyntaxError";
            for c in &mut d.children {
                c.runtime = d.runtime;
            }
            out.push(d);
        } else if !frames.is_empty() {
            let mut d = failure(message, code, &frames, estart..estart + etext.len(), opts);
            d.runtime = name != "SyntaxError";
            out.push(d);
        }
        i = k.max(j + 1);
    }
    out
}

// ------------------------------------------------------------ Rust panic

/// Rust panics (`thread 'main' panicked at file:line:col:`).
pub fn parse_rust_panic(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static NEW: OnceLock<Regex> = OnceLock::new();
    static OLD: OnceLock<Regex> = OnceLock::new();
    let new_re = re(
        &NEW,
        r"^thread '(?<thread>[^']*)'(?: \(\d+\))? panicked at (?<file>.+?):(?<line>\d+):(?<col>\d+):$",
    );
    let old_re = re(
        &OLD,
        r"^thread '(?<thread>[^']*)'(?: \(\d+\))? panicked at '(?<msg>.*)', (?<file>.+?):(?<line>\d+):(?<col>\d+)$",
    );
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    for (i, (start, text)) in lines.iter().enumerate() {
        let (file, line, col, message) = if let Some(c) = old_re.captures(text) {
            (c["file"].to_owned(), c["line"].parse().unwrap_or(1), c["col"].parse().ok(), c["msg"].to_owned())
        } else if let Some(c) = new_re.captures(text) {
            let mut msg = Vec::new();
            for (_, t) in lines.iter().skip(i + 1) {
                if t.trim().is_empty() || t.starts_with("note:") || t.starts_with("stack backtrace") {
                    break;
                }
                msg.push(*t);
            }
            (c["file"].to_owned(), c["line"].parse().unwrap_or(1), c["col"].parse().ok(), msg.join("\n"))
        } else {
            continue;
        };
        let frame = Frame {
            file,
            line,
            column: col,
            unit: ColumnUnit::Chars,
            func: None,
            range: *start..start + text.len(),
            location: None,
        };
        let message = if message.is_empty() {
            "panicked".to_owned()
        } else {
            format!("panicked: {}", message.lines().next().unwrap_or_default())
        };
        out.push(failure(message, None, &[frame], *start..start + text.len(), opts));
    }
    out
}

// -------------------------------------------------------------------- Go

/// Go panics (`panic: ...` + `goroutine N [running]:` + frames).
pub fn parse_go_panic(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static PANIC: OnceLock<Regex> = OnceLock::new();
    static FILE: OnceLock<Regex> = OnceLock::new();
    let panic_re = re(&PANIC, r"^(?<kind>panic|fatal error): (?<msg>.*)$");
    let file_re = re(&FILE, r"^\t(?<file>.+?):(?<line>\d+)(?: \+0x[0-9a-fA-F]+)?$");
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, text) = lines[i];
        let Some(c) = panic_re.captures(text) else {
            i += 1;
            continue;
        };
        let message = format!("{}: {}", &c["kind"], &c["msg"]);
        let mut j = i + 1;
        while j < lines.len() && !lines[j].1.starts_with("goroutine ") {
            if panic_re.is_match(lines[j].1) {
                break;
            }
            j += 1;
        }
        if j >= lines.len() || !lines[j].1.starts_with("goroutine ") {
            i += 1;
            continue;
        }
        j += 1;
        let mut frames: Vec<Frame> = Vec::new();
        while j + 1 < lines.len() {
            let func_line = lines[j].1;
            let Some(fc) = file_re.captures(lines[j + 1].1) else {
                break;
            };
            let func = func_line
                .split('(')
                .next()
                .unwrap_or(func_line)
                .trim()
                .to_owned();
            let file = fc["file"].to_owned();
            let internal = func.starts_with("runtime.")
                || func.starts_with("panic")
                || func.starts_with("testing.")
                || file.contains("/src/runtime/")
                || file.contains("/libexec/src/")
                || file.contains("/pkg/mod/");
            if !internal {
                frames.push(Frame {
                    file,
                    line: fc["line"].parse().unwrap_or(1),
                    column: None,
                    unit: ColumnUnit::Bytes,
                    func: Some(func),
                    range: lines[j + 1].0..lines[j + 1].0 + lines[j + 1].1.len(),
                    location: None,
                });
            }
            j += 2;
        }
        if !frames.is_empty() {
            out.push(failure(message, None, &frames, start..start + text.len(), opts));
        }
        i = j.max(i + 1);
    }
    out
}

// ------------------------------------------------------------------ Java

/// Java exceptions (`Exception in thread "main" ...` + `\tat ...` frames).
pub fn parse_java(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    static FRAME: OnceLock<Regex> = OnceLock::new();
    let head_re = re(
        &HEAD,
        r#"^(?:Exception in thread "[^"]*" )?(?<name>[\w.$]+(?:Exception|Error))(?::\s?(?<msg>.*))?$"#,
    );
    let frame_re = re(
        &FRAME,
        r"^\s+at (?<method>[\w.$<>]+)\((?<file>[\w$.-]+\.(?:java|kt|scala|groovy)):(?<line>\d+)\)$",
    );
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, text) = lines[i];
        let Some(c) = head_re.captures(text) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        let mut frames = Vec::new();
        while j < lines.len() {
            let (fs, ft) = lines[j];
            if let Some(fc) = frame_re.captures(ft) {
                let method = fc["method"].to_owned();
                let lib = ["java.", "jdk.", "javax.", "sun.", "kotlin.", "scala."]
                    .iter()
                    .any(|p| method.starts_with(p));
                if !lib {
                    frames.push(Frame {
                        file: fc["file"].to_owned(),
                        line: fc["line"].parse().unwrap_or(1),
                        column: None,
                        unit: ColumnUnit::Chars,
                        func: Some(method),
                        range: fs..fs + ft.len(),
                        location: None,
                    });
                }
                j += 1;
            } else if ft.trim_start().starts_with("at ") || ft.trim_start().starts_with("...") {
                j += 1;
            } else {
                break;
            }
        }
        if !frames.is_empty() {
            let message = match c.name("msg").map(|m| m.as_str().trim()).filter(|m| !m.is_empty()) {
                Some(m) => format!("{}: {m}", &c["name"]),
                None => c["name"].to_owned(),
            };
            out.push(failure(message, None, &frames, start..start + text.len(), opts));
        }
        i = j.max(i + 1);
    }
    out
}

// ------------------------------------------------------------------ Ruby

/// Ruby backtraces (`file:line:in 'meth': message (Class)` + `\tfrom ...`).
pub fn parse_ruby(output: &str, opts: &ParseOptions) -> Vec<Diagnostic> {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    static FROM: OnceLock<Regex> = OnceLock::new();
    let head_re = re(
        &HEAD,
        r"^(?<file>[^\s].*?):(?<line>\d+):in [`'](?<func>[^']*)': (?<msg>.*?)(?: \((?<class>[A-Z][\w:]*)\))?$",
    );
    let from_re = re(&FROM, r"^\s+from (?<file>.+?):(?<line>\d+):in [`'](?<func>[^']*)'$");
    let lines = lines_with_offsets(output);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, text) = lines[i];
        let Some(c) = head_re.captures(text) else {
            i += 1;
            continue;
        };
        let mut frames = Vec::new();
        let user = |f: &str| !f.contains("/gems/") && !f.contains("/lib/ruby/") && !f.starts_with("<internal:");
        let first_file = c["file"].to_owned();
        if user(&first_file) {
            frames.push(Frame {
                file: first_file,
                line: c["line"].parse().unwrap_or(1),
                column: None,
                unit: ColumnUnit::Bytes,
                func: Some(c["func"].to_owned()),
                range: start..start + text.len(),
                location: None,
            });
        }
        let mut j = i + 1;
        while j < lines.len() {
            let (fs, ft) = lines[j];
            let Some(fc) = from_re.captures(ft) else { break };
            let file = fc["file"].to_owned();
            if user(&file) {
                frames.push(Frame {
                    file,
                    line: fc["line"].parse().unwrap_or(1),
                    column: None,
                    unit: ColumnUnit::Bytes,
                    func: Some(fc["func"].to_owned()),
                    range: fs..fs + ft.len(),
                    location: None,
                });
            }
            j += 1;
        }
        if !frames.is_empty() {
            let msg = c["msg"].trim();
            let message = match c.name("class") {
                Some(class) => format!("{}: {msg}", class.as_str()),
                None => msg.to_owned(),
            };
            out.push(failure(message, None, &frames, start..start + text.len(), opts));
        }
        i = j.max(i + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> ParseOptions {
        ParseOptions::default()
    }

    #[test]
    fn python_runtime_traceback_chain() {
        let out = r#"Traceback (most recent call last):
  File "/work/a.py", line 9, in <module>
    main()
  File "/work/a.py", line 6, in main
    helper(0)
  File "/work/a.py", line 2, in helper
    return 1 / x
           ~~^~~
ZeroDivisionError: division by zero
"#;
        let d = parse_python(out, &o());
        assert_eq!(d.len(), 1);
        let d = &d[0];
        assert!(d.runtime);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.message, "ZeroDivisionError: division by zero");
        assert_eq!(d.file.as_deref().and_then(|p| p.to_str()), Some("/work/a.py"));
        match d.location.as_ref().unwrap() {
            Location::InLine { line, text, offset, length } => {
                assert_eq!((*line, text.as_str(), *offset, *length), (2, "return 1 / x", 7, 5));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(d.children.len(), 2);
        assert_eq!(d.children[0].message, "'helper' called here");
        assert_eq!(d.children[0].location, Some(Location::Lines { line: 6, column: None, end_line: None, end_column: None, unit: ColumnUnit::Chars }));
        assert_eq!(d.children[1].message, "'main' called here");
        assert_eq!(d.matched.len(), 1);
        assert!(d.matched[0].scopes[0].starts_with("console.error.runtime"));
    }

    #[test]
    fn python_syntax_error_is_compile_time() {
        let out = "  File \"/work/x.py\", line 3\n    foo bar\n        ^^^\nSyntaxError: invalid syntax\n";
        let d = parse_python(out, &o());
        assert_eq!(d.len(), 1);
        assert!(!d[0].runtime);
        assert_eq!(d[0].message, "SyntaxError: invalid syntax");
        assert_eq!(
            d[0].location,
            Some(Location::InLine { line: 3, text: "foo bar".into(), offset: 4, length: 3 })
        );
        let ind = "  File \"x.py\", line 2\n    y = 1\nIndentationError: unexpected indent\n";
        let d = parse_python(ind, &o());
        assert_eq!(d[0].message, "IndentationError: unexpected indent");
        assert!(matches!(d[0].location, Some(Location::Lines { line: 2, .. })));
    }

    #[test]
    fn python_skips_library_frames() {
        let out = r#"Traceback (most recent call last):
  File "a.py", line 3, in <module>
    json.loads("{")
  File "/usr/lib/python3.12/json/__init__.py", line 346, in loads
    return _default_decoder.decode(s)
json.decoder.JSONDecodeError: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)
"#;
        let d = parse_python(out, &o());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("a.py"));
        assert!(d[0].children.is_empty());
        assert!(d[0].message.starts_with("json.decoder.JSONDecodeError: Expecting"));
    }

    #[test]
    fn node_runtime_and_syntax() {
        let rt = r#"/work/a.js:3
throw new Error('boom');
^

Error: boom
    at inner (/work/a.js:3:9)
    at outer (/work/a.js:7:3)
    at Object.<anonymous> (/work/a.js:10:1)
    at Module._compile (node:internal/modules/cjs/loader:1256:14)

Node.js v20.1.0
"#;
        let d = parse_node(rt, &o());
        assert_eq!(d.len(), 1, "{d:#?}");
        assert!(d[0].runtime);
        assert_eq!(d[0].message, "Error: boom");
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("/work/a.js"));
        assert_eq!(
            d[0].location,
            Some(Location::Lines { line: 3, column: Some(1), end_line: None, end_column: None, unit: ColumnUnit::Chars })
        );
        assert_eq!(d[0].children.len(), 2);
        assert_eq!(d[0].children[0].message, "'inner' called here");
        assert_eq!(d[0].children[1].message, "'outer' called here");

        let syn = "/work/b.js:2\nfoo bar\n    ^^^\n\nSyntaxError: Unexpected identifier 'bar'\n    at wrapSafe (node:internal/modules/cjs/loader:1378:20)\n\nNode.js v20.1.0\n";
        let d = parse_node(syn, &o());
        assert_eq!(d.len(), 1);
        assert!(!d[0].runtime);
        assert_eq!(d[0].message, "SyntaxError: Unexpected identifier 'bar'");
        assert_eq!(
            d[0].location,
            Some(Location::Lines { line: 2, column: Some(5), end_line: None, end_column: Some(8), unit: ColumnUnit::Chars })
        );
    }

    #[test]
    fn node_error_without_header() {
        let out = "Error: Cannot find module './nope'\n    at Module._resolveFilename (node:internal/modules/cjs/loader:1:1)\n    at Object.<anonymous> (/work/c.js:1:1)\n";
        let d = parse_node(out, &o());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("/work/c.js"));
    }

    #[test]
    fn rust_panics() {
        let new = "thread 'main' panicked at src/main.rs:4:13:\nindex out of bounds: the len is 3 but the index is 5\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
        let d = parse_rust_panic(new, &o());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "panicked: index out of bounds: the len is 3 but the index is 5");
        assert_eq!(d[0].location, Some(Location::Lines { line: 4, column: Some(13), end_line: None, end_column: None, unit: ColumnUnit::Chars }));
        let old = "thread 'main' panicked at 'called `Option::unwrap()` on a `None` value', src/main.rs:2:5\n";
        let d = parse_rust_panic(old, &o());
        assert_eq!(d[0].message, "panicked: called `Option::unwrap()` on a `None` value");
        assert!(d[0].runtime);
    }

    #[test]
    fn go_panic() {
        let out = "panic: runtime error: index out of range [5] with length 3\n\ngoroutine 1 [running]:\nmain.get(...)\n\t/work/main.go:8\nmain.main()\n\t/work/main.go:12 +0x1d\nexit status 2\n";
        let d = parse_go_panic(out, &o());
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].message, "panic: runtime error: index out of range [5] with length 3");
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("/work/main.go"));
        assert_eq!(d[0].location.as_ref().and_then(Location::start_line), Some(8));
        assert_eq!(d[0].children.len(), 1);
        assert_eq!(d[0].children[0].message, "'main.get' called here");
    }

    #[test]
    fn java_exception() {
        let out = "Exception in thread \"main\" java.lang.IllegalStateException: boom\n\tat Main.fail(Main.java:7)\n\tat Main.main(Main.java:3)\n\tat java.base/java.lang.Thread.run(Thread.java:833)\n";
        let d = parse_java(out, &o());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "java.lang.IllegalStateException: boom");
        assert_eq!(d[0].file.as_deref().and_then(|p| p.to_str()), Some("Main.java"));
        assert_eq!(d[0].location.as_ref().and_then(Location::start_line), Some(7));
        assert_eq!(d[0].children[0].message, "'Main.fail' called here");
    }

    #[test]
    fn ruby_backtrace() {
        let out = "x.rb:3:in 'Integer#/': divided by 0 (ZeroDivisionError)\n\tfrom x.rb:3:in 'foo'\n\tfrom x.rb:6:in '<main>'\n";
        let d = parse_ruby(out, &o());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "ZeroDivisionError: divided by 0");
        assert_eq!(d[0].children.len(), 2);
        assert_eq!(d[0].children[1].message, "'foo' called here");
    }
}
