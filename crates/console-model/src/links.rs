//! Clickable spans in console text: source locations and URLs; plus a cheap
//! severity classifier for compiler/runtime messages.
//!
//! Recognised (all matched against one line of text):
//!
//! * `path:line[:col]` - gcc, clang, swiftc, rustc (`--> src/main.rs:3:5`), go,
//!   tsc, node (`(/tmp/x.js:3:9)`), java (`(Main.java:10)`), ruby, ...
//! * `path(line[,col])` - MSVC / C# style
//! * Python tracebacks: `File "x.py", line 12, in <module>`
//! * `path line N` / `path on line N` / `path: line N` - perl, php, bash
//! * `http://`, `https://`, `ftp://` URLs (trailing punctuation trimmed)
//!
//! Paths are not checked against the file system; the caller resolves
//! relative paths against the run's working directory and decides whether the
//! target exists. Paths containing spaces are not detected.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use crate::tag::Level;

/// Where a link leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A source location; `line`/`column` are 1-based as printed.
    File {
        /// The path as printed (may be relative, `~/...`).
        path: String,
        /// 1-based line.
        line: Option<u32>,
        /// 1-based column.
        column: Option<u32>,
    },
    /// A web URL.
    Url(String),
}

/// A clickable range of one line's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Byte range in the line text.
    pub range: Range<usize>,
    /// Where it leads.
    pub target: LinkTarget,
}

const PATH_CHARS: &str = r"A-Za-z0-9_@+~./\-";

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?|ftp)://[^\s<>"'`]+"#).expect("static URL regex")
});
static PYTHON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"File "(?P<path>[^"\n]+)", line (?P<line>\d+)"#).expect("static python regex")
});
static COLON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?P<path>[{PATH_CHARS}]+):(?P<line>\d+)(?::(?P<col>\d+))?"
    ))
    .expect("static colon regex")
});
static PAREN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?P<path>[{PATH_CHARS}]+)\((?P<line>\d+)(?:,\s*(?P<col>\d+))?\)"
    ))
    .expect("static paren regex")
});
static LINE_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?P<path>[{PATH_CHARS}]+):? (?:on )?line (?P<line>\d+)"
    ))
    .expect("static line regex")
});

const KNOWN_EXTENSIONS: &[&str] = &[
    "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "swift", "rs", "py", "pyw", "js", "jsx",
    "mjs", "cjs", "ts", "tsx", "java", "kt", "kts", "go", "rb", "php", "pl", "pm", "lua", "sh",
    "bash", "zsh", "cs", "fs", "vb", "scala", "clj", "ex", "exs", "erl", "hs", "ml", "r", "jl",
    "dart", "zig", "nim", "cr", "d", "f90", "f95", "pas", "tex", "html", "htm", "css", "scss",
    "json", "yaml", "yml", "toml", "xml", "txt", "md", "sql", "applescript", "scpt", "gradle",
    "cmake", "mk", "vue", "svelte", "m4", "s", "asm", "v", "sv", "tcl", "awk", "groovy",
];

fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 || !path.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if path.starts_with("//") || path.starts_with('<') {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    let ext = last.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.is_empty());
    match ext {
        Some(e) if e.len() <= 10 && e.chars().all(|c| c.is_ascii_alphanumeric()) && e.chars().any(|c| c.is_ascii_alphabetic()) => {
            path.contains('/') || KNOWN_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
        }
        _ => path.starts_with('/') || path.starts_with("./") || path.starts_with("../") || path.starts_with("~/"),
    }
}

fn num(m: Option<regex::Match<'_>>) -> Option<u32> {
    m.and_then(|m| m.as_str().parse().ok())
}

fn trim_url(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let Some(last) = s.chars().next_back() else { break };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '*' | '>' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            '}' => s.matches('{').count() < s.matches('}').count(),
            _ => false,
        };
        if strip {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    &url[..end]
}

/// Finds the clickable ranges of `text`, left to right, never overlapping.
pub fn detect_links(text: &str) -> Vec<Link> {
    // Cheap rejection: every pattern needs one of these.
    if !(text.contains(':') || text.contains('(') || text.contains("line ")) {
        return Vec::new();
    }
    let mut found: Vec<Link> = Vec::new();

    for m in URL_RE.find_iter(text) {
        let url = trim_url(m.as_str());
        if url.len() > "https://".len() - 1 {
            found.push(Link {
                range: m.start()..m.start() + url.len(),
                target: LinkTarget::Url(url.to_owned()),
            });
        }
    }
    for c in PYTHON_RE.captures_iter(text) {
        let (Some(path), Some(line), Some(whole)) = (c.name("path"), c.name("line"), c.get(0)) else { continue };
        if path.as_str().starts_with('<') {
            continue;
        }
        // From the opening quote of the path to the end of the line number.
        found.push(Link {
            range: path.start() - 1..whole.end(),
            target: LinkTarget::File {
                path: path.as_str().to_owned(),
                line: line.as_str().parse().ok(),
                column: None,
            },
        });
    }
    for c in COLON_RE.captures_iter(text) {
        let (Some(path), Some(whole)) = (c.name("path"), c.get(0)) else { continue };
        if valid_path(path.as_str()) {
            found.push(Link {
                range: whole.range(),
                target: LinkTarget::File {
                    path: path.as_str().to_owned(),
                    line: num(c.name("line")),
                    column: num(c.name("col")),
                },
            });
        }
    }
    for c in PAREN_RE.captures_iter(text) {
        let (Some(path), Some(whole)) = (c.name("path"), c.get(0)) else { continue };
        if valid_path(path.as_str()) {
            found.push(Link {
                range: whole.range(),
                target: LinkTarget::File {
                    path: path.as_str().to_owned(),
                    line: num(c.name("line")),
                    column: num(c.name("col")),
                },
            });
        }
    }
    for c in LINE_WORD_RE.captures_iter(text) {
        let (Some(path), Some(whole)) = (c.name("path"), c.get(0)) else { continue };
        if valid_path(path.as_str()) {
            found.push(Link {
                range: whole.range(),
                target: LinkTarget::File {
                    path: path.as_str().to_owned(),
                    line: num(c.name("line")),
                    column: None,
                },
            });
        }
    }

    found.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.len().cmp(&a.range.len())));
    let mut out: Vec<Link> = Vec::with_capacity(found.len());
    for l in found {
        if out.last().is_none_or(|p| l.range.start >= p.range.end) {
            out.push(l);
        }
    }
    out
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Guesses a line's severity from its text: compiler/runtime messages
/// (`error:`, `Traceback`, `panicked at`, `warning:`, `note:` ...). Heuristic
/// by design; the runner may override per line with `Console::set_level`.
pub fn classify_level(text: &str) -> Level {
    if text.len() < 5 {
        return Level::Normal;
    }
    const ERRORS: &[&str] = &[
        "error:",
        "error[",
        ": error",
        "fatal error",
        "fatal:",
        "exception:",
        "traceback (most recent call last)",
        "panicked at",
        "exception in thread",
        "unhandled exception",
        "segmentation fault",
        "undefined reference",
        "cannot find symbol",
        "command not found",
    ];
    const WARNINGS: &[&str] = &["warning:", ": warning", "warning["];
    const NOTICES: &[&str] = &["note:", ": note"];
    if ERRORS.iter().any(|p| contains_ci(text, p)) {
        Level::Error
    } else if WARNINGS.iter().any(|p| contains_ci(text, p)) {
        Level::Warning
    } else if NOTICES.iter().any(|p| contains_ci(text, p)) {
        Level::Notice
    } else {
        Level::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, line: Option<u32>, column: Option<u32>) -> LinkTarget {
        LinkTarget::File {
            path: path.into(),
            line,
            column,
        }
    }

    fn one(text: &str) -> (String, LinkTarget) {
        let links = detect_links(text);
        assert_eq!(links.len(), 1, "{text:?} -> {links:?}");
        (text[links[0].range.clone()].to_owned(), links[0].target.clone())
    }

    #[test]
    fn gcc_clang_swift() {
        let (s, t) = one("main.c:12:5: error: expected ';'");
        assert_eq!(s, "main.c:12:5");
        assert_eq!(t, file("main.c", Some(12), Some(5)));
        let (s, t) = one("/Users/me/p/File.swift:10:5: warning: unused");
        assert_eq!(s, "/Users/me/p/File.swift:10:5");
        assert_eq!(t, file("/Users/me/p/File.swift", Some(10), Some(5)));
        let (_, t) = one("./hello.go:7:2: undefined: x");
        assert_eq!(t, file("./hello.go", Some(7), Some(2)));
    }

    #[test]
    fn rust_arrow_and_panic() {
        let (s, _) = one("  --> src/main.rs:3:9");
        assert_eq!(s, "src/main.rs:3:9");
        let (s, t) = one("thread 'main' panicked at src/main.rs:5:10:");
        assert_eq!(s, "src/main.rs:5:10");
        assert_eq!(t, file("src/main.rs", Some(5), Some(10)));
    }

    #[test]
    fn python_traceback() {
        let text = r#"  File "/tmp/x.py", line 12, in <module>"#;
        let (s, t) = one(text);
        assert_eq!(s, r#""/tmp/x.py", line 12"#);
        assert_eq!(t, file("/tmp/x.py", Some(12), None));
        assert!(detect_links(r#"  File "<stdin>", line 1, in <module>"#).is_empty());
    }

    #[test]
    fn node_and_java_stacks() {
        let (s, _) = one("    at Object.<anonymous> (/tmp/x.js:3:9)");
        assert_eq!(s, "/tmp/x.js:3:9");
        let (s, t) = one("\tat pkg.Main.main(Main.java:10)");
        assert_eq!(s, "Main.java:10");
        assert_eq!(t, file("Main.java", Some(10), None));
    }

    #[test]
    fn msvc_and_line_word() {
        let (_, t) = one("Program.cs(12,5): error CS1002");
        assert_eq!(t, file("Program.cs", Some(12), Some(5)));
        let (_, t) = one("syntax error at script.pl line 7.");
        assert_eq!(t, file("script.pl", Some(7), None));
        let (_, t) = one("PHP Parse error: in /var/www/x.php on line 3");
        assert_eq!(t, file("/var/www/x.php", Some(3), None));
        let (_, t) = one("./run.sh: line 4: foo: command not found");
        assert_eq!(t, file("./run.sh", Some(4), None));
    }

    #[test]
    fn urls() {
        let (s, t) = one("see https://example.com/a?b=1, for more");
        assert_eq!(s, "https://example.com/a?b=1");
        assert_eq!(t, LinkTarget::Url("https://example.com/a?b=1".into()));
        let (s, _) = one("(visit http://x.org/y_(z))");
        assert_eq!(s, "http://x.org/y_(z)");
        // A URL swallows host:port, no file link inside it.
        let links = detect_links("GET http://localhost:8080/x.py:3");
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn false_positives_rejected() {
        for t in [
            "12:30:45 started",
            "listening on localhost:8080",
            "version 1.2.3:4",
            "example.com:80",
            "ratio 3:2",
            "a: b",
            "Time: 5",
        ] {
            assert!(detect_links(t).is_empty(), "{t:?} -> {:?}", detect_links(t));
        }
    }

    #[test]
    fn several_per_line() {
        let links = detect_links("a.c:1: in a.h:2 and https://x.y/z");
        assert_eq!(links.len(), 3);
        assert!(links.windows(2).all(|w| w[0].range.end <= w[1].range.start));
    }

    #[test]
    fn levels() {
        assert_eq!(classify_level("main.c:1:1: error: boom"), Level::Error);
        assert_eq!(classify_level("ValueError: bad"), Level::Error);
        assert_eq!(classify_level("Traceback (most recent call last):"), Level::Error);
        assert_eq!(classify_level("warning: unused variable"), Level::Warning);
        assert_eq!(classify_level("x.c:1:2: note: declared here"), Level::Notice);
        assert_eq!(classify_level("hello world"), Level::Normal);
        assert_eq!(classify_level("ok"), Level::Normal);
    }
}
