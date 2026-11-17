//! Variables available to run commands, compile steps and diagnostics.
//!
//! [`Vars`] is a plain data bag. The runner fills it for one run (see
//! [`Vars::for_source`] and the `with_*` builders), then every template of a
//! language is expanded against it ([`crate::expand_command`]).
//!
//! # Variables
//!
//! | Name | Meaning |
//! |--|--|
//! | `filename` | file name only, relative to the working directory (`CR_FILENAME`) |
//! | `file` | absolute path of the source file (`CR_FILE`) |
//! | `dir` | directory of the source file (the working directory of a run) |
//! | `stem`, `ext` | file name without / with only its extension |
//! | `file_url` | percent-encoded `file://` URL of the source file |
//! | `tmpdir` | the run's temporary directory (`CR_TMPDIR`) |
//! | `output` | suggested compiler output path, no extension (`CR_SUGGESTED_OUTPUT_FILE`) |
//! | `build_dir` | a private directory for build products: `<output>.build` |
//! | `flags` | list: the compile flags, shell-split |
//! | `compiler` | what the compile phase produced (`$compiler` of CodeRunner) |
//! | `compiler_url` | `file://` URL of `compiler` when it is an absolute path |
//! | `debugging` | `"1"` on a debug run, empty otherwise (`CR_DEBUGGING`) |
//! | `encoding`, `encoding_name` | the I/O encoding code and its IANA name |
//! | `runid` | unique run number (`CR_RUNID`) |
//! | `home` | the user's home directory |
//! | `language_dir`, `scripts_dir`, `unsaved_dir` | support directories |
//!
//! Anything that is not set falls back to the process environment snapshot
//! given with [`Vars::with_env`] (so `$HOME`, `$PATH` work like in a shell), and
//! then to the empty string.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::ExpandError;

/// The variable names the runner provides ([`Vars`] documents each one).
pub const KNOWN_VARIABLES: &[&str] = &[
    "filename",
    "file",
    "dir",
    "stem",
    "ext",
    "file_url",
    "tmpdir",
    "output",
    "build_dir",
    "flags",
    "compiler",
    "compiler_url",
    "debugging",
    "encoding",
    "encoding_name",
    "runid",
    "home",
    "language_dir",
    "scripts_dir",
    "unsaved_dir",
];

/// The value of a variable: a string, or a list (such as the compile flags,
/// which splice into several arguments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A single string.
    Str(String),
    /// A list of strings. An unquoted `$name` alone in a word splices into one
    /// argument per item; everywhere else the items are joined with spaces.
    List(Vec<String>),
}

impl Value {
    /// The value as one string (lists are joined with a space).
    pub fn joined(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::List(items) => items.join(" "),
        }
    }

    /// Whether the value is an empty string or an empty list.
    pub fn is_empty(&self) -> bool {
        match self {
            Value::Str(s) => s.is_empty(),
            Value::List(items) => items.is_empty(),
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}

impl From<Vec<String>> for Value {
    fn from(value: Vec<String>) -> Self {
        Value::List(value)
    }
}

/// The variables of one run. Cheap to clone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    values: BTreeMap<String, Value>,
    env: BTreeMap<String, String>,
}

impl Vars {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The variables for running the file `file_name` that lives in `dir`:
    /// `filename`, `file`, `dir`, `stem`, `ext`, `file_url`, and an empty
    /// `flags` list. `output`/`build_dir` default to `<dir>/<stem>`; call
    /// [`Vars::with_tmpdir`] to move them into a temporary directory.
    pub fn for_source(dir: &Path, file_name: &str) -> Self {
        let mut vars = Self::new();
        let path = Path::new(file_name);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_owned();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let file = dir.join(file_name);
        vars.set("filename", file_name);
        vars.set("file", file.to_string_lossy().into_owned());
        vars.set("dir", dir.to_string_lossy().into_owned());
        vars.set("stem", stem.clone());
        vars.set("ext", ext);
        vars.set("file_url", file_url(&file));
        vars.set("flags", Value::List(Vec::new()));
        let output = dir.join(&stem).to_string_lossy().into_owned();
        vars.set("build_dir", format!("{output}.build"));
        vars.set("output", output);
        vars
    }

    /// Sets (or replaces) a variable.
    pub fn set(&mut self, name: &str, value: impl Into<Value>) {
        self.values.insert(name.to_owned(), value.into());
    }

    /// Removes a variable.
    pub fn unset(&mut self, name: &str) {
        self.values.remove(name);
    }

    /// The value of `name`: a set variable, else the environment snapshot,
    /// else `None`.
    pub fn lookup(&self, name: &str) -> Option<Value> {
        self.values
            .get(name)
            .cloned()
            .or_else(|| self.env.get(name).map(|v| Value::Str(v.clone())))
    }

    /// The value of a variable that was explicitly set (no environment
    /// fallback).
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    /// The string value of `name` (lists joined), from the set variables only.
    pub fn get_str(&self, name: &str) -> Option<String> {
        self.values.get(name).map(Value::joined)
    }

    /// Whether `name` is set to a non-empty value.
    pub fn is_set(&self, name: &str) -> bool {
        self.values.get(name).is_some_and(|v| !v.is_empty())
    }

    /// The set variables.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Gives the process environment `$NAME` falls back to (typically a
    /// snapshot of the environment the run will have, including `PATH`/`HOME`).
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        if let Some(home) = env.get("HOME") {
            self.values
                .entry("home".to_owned())
                .or_insert_with(|| Value::Str(home.clone()));
        }
        self.env = env;
        self
    }

    /// [`Vars::with_env`] with this process's environment.
    pub fn with_process_env(self) -> Self {
        self.with_env(std::env::vars().collect())
    }

    /// Sets the temporary directory: `tmpdir`, `output` (`<tmp>/<stem>`) and
    /// `build_dir` (`<output>.build`).
    pub fn with_tmpdir(mut self, tmpdir: &Path) -> Self {
        let stem = self.get_str("stem").unwrap_or_default();
        let output = tmpdir.join(stem).to_string_lossy().into_owned();
        self.set("tmpdir", tmpdir.to_string_lossy().into_owned());
        self.set("build_dir", format!("{output}.build"));
        self.set("output", output);
        self
    }

    /// Sets `flags` from a flag string (shell-split; invalid quoting keeps the
    /// words as they are).
    pub fn with_flags(mut self, flags: &str) -> Self {
        let list = split_words(flags)
            .unwrap_or_else(|_| flags.split_whitespace().map(str::to_owned).collect());
        self.set("flags", Value::List(list));
        self
    }

    /// Marks a debug run (`debugging` becomes `"1"`).
    pub fn with_debugging(mut self, debugging: bool) -> Self {
        self.set("debugging", if debugging { "1" } else { "" });
        self
    }

    /// Sets `encoding` and `encoding_name` from an `NSStringEncoding` code
    /// (0 = default = UTF-8).
    pub fn with_encoding(mut self, code: i64) -> Self {
        self.set("encoding", code.to_string());
        self.set("encoding_name", encoding_name(code));
        self
    }

    /// Sets the unique run number.
    pub fn with_runid(mut self, id: u64) -> Self {
        self.set("runid", id.to_string());
        self
    }

    /// Sets `language_dir` and `scripts_dir`.
    pub fn with_language_dir(mut self, language_dir: &Path) -> Self {
        self.set("language_dir", language_dir.to_string_lossy().into_owned());
        self.set(
            "scripts_dir",
            language_dir.join("Scripts").to_string_lossy().into_owned(),
        );
        self
    }

    /// Records what the compile phase produced: sets `compiler` and, when the
    /// value is an absolute path, `compiler_url`.
    pub fn set_compiler(&mut self, compiler: &str) {
        self.set("compiler", compiler);
        if compiler.starts_with('/') {
            self.set("compiler_url", file_url(Path::new(compiler)));
        } else {
            self.unset("compiler_url");
        }
    }

    /// The environment to export to the process that runs a compile script,
    /// a shell run command or a diagnostic command: every set string variable
    /// under its own name, plus the `CR_*` names CodeRunner scripts expect
    /// (`CR_FILENAME`, `CR_FILE`, `CR_TMPDIR`, `CR_SUGGESTED_OUTPUT_FILE`,
    /// `CR_DEBUGGING`, `CR_ENCODING`, `CR_ENCODING_NAME`, `CR_RUNID`,
    /// `CR_LANGUAGE_DIR`, `CR_SCRIPTS_DIR`, `CR_UNSAVED_DIR`).
    pub fn export_env(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (name, value) in &self.values {
            if let Value::Str(s) = value
                && !s.is_empty()
            {
                out.insert(name.clone(), s.clone());
            }
        }
        for (cr, ours) in [
            ("CR_FILENAME", "filename"),
            ("CR_FILE", "file"),
            ("CR_TMPDIR", "tmpdir"),
            ("CR_SUGGESTED_OUTPUT_FILE", "output"),
            ("CR_DEBUGGING", "debugging"),
            ("CR_ENCODING", "encoding"),
            ("CR_ENCODING_NAME", "encoding_name"),
            ("CR_RUNID", "runid"),
            ("CR_LANGUAGE_DIR", "language_dir"),
            ("CR_SCRIPTS_DIR", "scripts_dir"),
            ("CR_UNSAVED_DIR", "unsaved_dir"),
        ] {
            if let Some(Value::Str(s)) = self.values.get(ours)
                && !s.is_empty()
            {
                out.insert(cr.to_owned(), s.clone());
            }
        }
        out
    }
}

/// The IANA/Java charset name of an `NSStringEncoding` code as used by
/// CodeRunner's `OutputEncoding` (0 = default = UTF-8; unknown codes are
/// UTF-8 too).
pub fn encoding_name(code: i64) -> &'static str {
    match code {
        1 => "US-ASCII",
        3 => "EUC-JP",
        5 => "ISO-8859-1",
        8 => "Shift_JIS",
        9 => "ISO-8859-2",
        10 => "UTF-16",
        12 => "windows-1252",
        30 => "macintosh",
        _ => "UTF-8",
    }
}

/// `file://` URL of an absolute path, percent-encoded.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for &byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                url.push(byte as char)
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// Percent-encodes a string for use inside a URL query (everything but the
/// RFC 3986 unreserved characters).
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Quotes a string for a POSIX shell: wraps it in single quotes, escaping
/// embedded single quotes as `'\''`.
pub fn shell_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Splits a string into words like a shell does for arguments: whitespace
/// separates, single quotes are literal, double quotes group (with `\"`,
/// `\\`, `\$` and `` \` `` escapes), a backslash escapes the next character.
/// No variable or glob expansion happens. This is how compile flags and run
/// arguments are split.
pub fn split_words(text: &str) -> Result<Vec<String>, ExpandError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err(ExpandError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.peek().copied() {
                            Some(next @ ('"' | '\\' | '$' | '`')) => {
                                current.push(next);
                                chars.next();
                            }
                            _ => current.push('\\'),
                        },
                        Some(c) => current.push(c),
                        None => return Err(ExpandError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(next) => current.push(next),
                    None => current.push('\\'),
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_source_sets_the_basic_variables() {
        let vars = Vars::for_source(Path::new("/work/my proj"), "main file.c");
        assert_eq!(vars.get_str("filename").as_deref(), Some("main file.c"));
        assert_eq!(
            vars.get_str("file").as_deref(),
            Some("/work/my proj/main file.c")
        );
        assert_eq!(vars.get_str("stem").as_deref(), Some("main file"));
        assert_eq!(vars.get_str("ext").as_deref(), Some("c"));
        assert_eq!(
            vars.get_str("file_url").as_deref(),
            Some("file:///work/my%20proj/main%20file.c")
        );
        assert_eq!(vars.get("flags"), Some(&Value::List(vec![])));
    }

    #[test]
    fn tmpdir_moves_output() {
        let vars = Vars::for_source(Path::new("/w"), "a.c").with_tmpdir(Path::new("/tmp/x"));
        assert_eq!(vars.get_str("output").as_deref(), Some("/tmp/x/a"));
        assert_eq!(vars.get_str("build_dir").as_deref(), Some("/tmp/x/a.build"));
        assert_eq!(vars.get_str("tmpdir").as_deref(), Some("/tmp/x"));
    }

    #[test]
    fn export_env_has_cr_names() {
        let env = Vars::for_source(Path::new("/w"), "a.c")
            .with_debugging(true)
            .with_encoding(4)
            .export_env();
        assert_eq!(env.get("CR_FILENAME").map(String::as_str), Some("a.c"));
        assert_eq!(env.get("CR_FILE").map(String::as_str), Some("/w/a.c"));
        assert_eq!(env.get("CR_DEBUGGING").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("CR_ENCODING_NAME").map(String::as_str),
            Some("UTF-8")
        );
        assert!(!env.contains_key("flags"), "lists are not exported");
    }

    #[test]
    fn debugging_off_is_not_exported() {
        let env = Vars::for_source(Path::new("/w"), "a.c")
            .with_debugging(false)
            .export_env();
        assert!(!env.contains_key("CR_DEBUGGING"));
    }

    #[test]
    fn compiler_url_follows_compiler() {
        let mut vars = Vars::new();
        vars.set_compiler("/tmp/a b.js");
        assert_eq!(
            vars.get_str("compiler_url").as_deref(),
            Some("file:///tmp/a%20b.js")
        );
        vars.set_compiler("java");
        assert!(vars.get("compiler_url").is_none());
    }

    #[test]
    fn env_fallback_and_home() {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_owned(), "/Users/x".to_owned());
        env.insert("FOO".to_owned(), "bar".to_owned());
        let vars = Vars::new().with_env(env);
        assert_eq!(vars.lookup("FOO"), Some(Value::Str("bar".into())));
        assert_eq!(vars.get_str("home").as_deref(), Some("/Users/x"));
        assert_eq!(vars.lookup("NOPE"), None);
    }

    #[test]
    fn split_words_follows_shell_rules() {
        assert_eq!(split_words("-O2  -Wall").unwrap(), ["-O2", "-Wall"]);
        assert_eq!(
            split_words(r#"-DNAME="a b" 'c d' e\ f"#).unwrap(),
            ["-DNAME=a b", "c d", "e f"]
        );
        assert_eq!(split_words("''").unwrap(), [""]);
        assert!(split_words("").unwrap().is_empty());
        assert_eq!(split_words("'oops"), Err(ExpandError::UnterminatedQuote));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
    }

    #[test]
    fn encoding_names() {
        assert_eq!(encoding_name(0), "UTF-8");
        assert_eq!(encoding_name(30), "macintosh");
        assert_eq!(encoding_name(12), "windows-1252");
    }
}
