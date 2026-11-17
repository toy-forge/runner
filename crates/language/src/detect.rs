//! Language detection building blocks and the types of [`crate::LanguageRegistry::detect`].
//!
//! Precedence (first match wins): **explicit** choice (id, uuid or name) >
//! file **extension** > exact **file name** > `#!` **shebang** > editor
//! **modeline** (`# vim: ft=python`, `-*- mode: ruby -*-`) > the registry's
//! default language (plain text). When several enabled languages claim the
//! same extension (`h`, `js`, `ts`, `cs`) the one that lists it in
//! `default_for` wins and the others are reported as
//! [`Detection::candidates`].
//!
//! "New from Clipboard" uses [`crate::LanguageRegistry::guess_from_text`]:
//! shebang, then modeline, then the languages' `content_patterns` (each
//! matching pattern scores its weight; a pattern may start with `@N ` to set
//! the weight N, default 1; at least 2 points and a unique winner are needed).

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::model::LanguageDef;

/// What decided a detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectReason {
    /// The caller named the language.
    Explicit,
    /// The file extension.
    Extension,
    /// The exact file name.
    FileName,
    /// The `#!` line.
    Shebang,
    /// An editor modeline.
    Modeline,
    /// The text's content (clipboard heuristics).
    Content,
    /// Nothing matched: the default language.
    Fallback,
}

/// The inputs of a detection. Build with [`DetectInput::new`] and the
/// chained setters.
#[derive(Debug, Clone, Copy, Default)]
pub struct DetectInput<'a> {
    /// A language the document is explicitly set to (id, uuid or name).
    pub explicit: Option<&'a str>,
    /// The document's path (or just a file name).
    pub path: Option<&'a Path>,
    /// The document's text, or at least its first and last few lines.
    pub text: Option<&'a str>,
}

impl<'a> DetectInput<'a> {
    /// No input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the explicit language.
    pub fn explicit(mut self, language: &'a str) -> Self {
        self.explicit = Some(language);
        self
    }

    /// Sets the path.
    pub fn path(mut self, path: &'a Path) -> Self {
        self.path = Some(path);
        self
    }

    /// Sets the text.
    pub fn text(mut self, text: &'a str) -> Self {
        self.text = Some(text);
        self
    }
}

/// The result of [`crate::LanguageRegistry::detect`].
#[derive(Debug, Clone)]
pub struct Detection<'a> {
    /// The detected language.
    pub language: &'a LanguageDef,
    /// What decided.
    pub reason: DetectReason,
    /// Other enabled languages that claim the same extension (empty when the
    /// extension is unambiguous or another reason decided).
    pub candidates: Vec<&'a LanguageDef>,
}

/// The result of [`crate::LanguageRegistry::guess_from_text`].
#[derive(Debug, Clone)]
pub struct Guess<'a> {
    /// The guessed language.
    pub language: &'a LanguageDef,
    /// Shebang, modeline or content.
    pub reason: DetectReason,
    /// The content score (0 for shebang/modeline guesses).
    pub score: u32,
}

/// The interpreter named by a `#!` line: `python3` for `#!/usr/bin/env python3`
/// and `#!/usr/bin/python3 -u`; `env` options (`-S`, `-i`, `NAME=x`) are
/// skipped. `None` when the line is not a shebang.
///
/// ```
/// use scratch_language::shebang_interpreter;
/// assert_eq!(shebang_interpreter("#!/usr/bin/env -S node --no-warnings").as_deref(), Some("node"));
/// assert_eq!(shebang_interpreter("#!/bin/zsh -e").as_deref(), Some("zsh"));
/// assert_eq!(shebang_interpreter("hello"), None);
/// ```
pub fn shebang_interpreter(line: &str) -> Option<String> {
    let rest = line
        .trim_start_matches('\u{feff}')
        .strip_prefix("#!")?
        .trim();
    let mut words = rest.split_whitespace();
    let first = words.next()?;
    let program = if Path::new(first).file_name()? == "env" {
        words.find(|word| !word.starts_with('-') && !word.contains('='))?
    } else {
        first
    };
    Some(Path::new(program).file_name()?.to_str()?.to_owned())
}

/// Whether the interpreter `name` is the entry `entry` of a language's
/// `shebangs`: equal, or `entry` followed only by digits and dots
/// (`python` matches `python3.12`, not `pythonista`).
pub fn interpreter_matches(name: &str, entry: &str) -> bool {
    match name.strip_prefix(entry) {
        Some("") => true,
        Some(rest) => rest.chars().all(|c| c.is_ascii_digit() || c == '.'),
        None => false,
    }
}

/// Editor mode names found in the modelines of a text (its first and last
/// five lines), lowercase, in order: `-*- mode: Python -*-`, `-*- ruby -*-`,
/// `# vim: set ft=python :`, `// vi: syntax=cpp`.
///
/// ```
/// use scratch_language::modeline_hints;
/// assert_eq!(modeline_hints("# -*- mode: Python; coding: utf-8 -*-\nx"), ["python"]);
/// assert_eq!(modeline_hints("x\n// vim: set ts=4 ft=cpp :"), ["cpp"]);
/// ```
pub fn modeline_hints(text: &str) -> Vec<String> {
    static EMACS: OnceLock<Regex> = OnceLock::new();
    static VIM: OnceLock<Regex> = OnceLock::new();
    // Constant patterns: they are valid, `Regex::new` cannot fail on them.
    let emacs = EMACS.get_or_init(|| Regex::new(r"-\*-\s*(.*?)\s*-\*-").expect("constant regex"));
    let vim = VIM.get_or_init(|| {
        Regex::new(r"(?:\bvim?|\bex):\s*(?:set?\s+)?[^\n]*?\b(?:ft|filetype|syntax)=([\w.+-]+)")
            .expect("constant regex")
    });
    let lines: Vec<&str> = text.lines().collect();
    let mut picked: Vec<&str> = lines.iter().take(5).copied().collect();
    if lines.len() > 5 {
        let tail_start = lines.len().saturating_sub(5).max(5);
        picked.extend(lines[tail_start..].iter().copied());
    }
    let mut hints = Vec::new();
    for line in picked {
        if let Some(caps) = emacs.captures(line) {
            let body = &caps[1];
            if body.contains(':') {
                for part in body.split(';') {
                    if let Some((key, value)) = part.split_once(':')
                        && key.trim().eq_ignore_ascii_case("mode")
                    {
                        hints.push(value.trim().to_ascii_lowercase());
                    }
                }
            } else if !body.is_empty() {
                hints.push(body.to_ascii_lowercase());
            }
        }
        if let Some(caps) = vim.captures(line) {
            hints.push(caps[1].to_ascii_lowercase());
        }
    }
    hints
}

/// Splits a `content_patterns` entry into its weight and regex source:
/// `"@2 <\\?php"` -> `(2, "<\\?php")`, `"fn main"` -> `(1, "fn main")`.
pub fn parse_content_pattern(entry: &str) -> (u32, &str) {
    if let Some(rest) = entry.strip_prefix('@')
        && let Some((weight, source)) = rest.split_once(' ')
        && let Ok(weight) = weight.parse::<u32>()
    {
        return (weight, source);
    }
    (1, entry)
}

/// The extensions a file name can have, longest first: `a.d.ts` -> `d.ts`,
/// `ts`. Lowercase. A leading dot (`.zshrc`) is not an extension separator.
pub fn extension_candidates(file_name: &str) -> Vec<String> {
    let trimmed = file_name.strip_prefix('.').unwrap_or(file_name);
    trimmed
        .match_indices('.')
        .map(|(at, _)| trimmed[at + 1..].to_ascii_lowercase())
        .filter(|ext| !ext.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shebangs() {
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env python3").as_deref(),
            Some("python3")
        );
        assert_eq!(shebang_interpreter("#!/bin/zsh -e").as_deref(), Some("zsh"));
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env -S node --no-warnings").as_deref(),
            Some("node")
        );
        assert_eq!(
            shebang_interpreter("#! /usr/bin/env FOO=1 ruby").as_deref(),
            Some("ruby")
        );
        assert_eq!(
            shebang_interpreter("\u{feff}#!/bin/sh").as_deref(),
            Some("sh")
        );
        assert_eq!(shebang_interpreter("# not a shebang"), None);
        assert_eq!(shebang_interpreter("#!"), None);
        assert_eq!(shebang_interpreter("#!/usr/bin/env"), None);
    }

    #[test]
    fn interpreter_matching() {
        assert!(interpreter_matches("python", "python"));
        assert!(interpreter_matches("python3", "python"));
        assert!(interpreter_matches("python3.12", "python"));
        assert!(!interpreter_matches("pythonista", "python"));
        assert!(!interpreter_matches("bash", "sh"));
        assert!(interpreter_matches("sh", "sh"));
        assert!(!interpreter_matches("ruby", "python"));
    }

    #[test]
    fn modelines() {
        assert_eq!(
            modeline_hints("# -*- mode: Python; coding: utf-8 -*-"),
            ["python"]
        );
        assert_eq!(modeline_hints("# -*- ruby -*-"), ["ruby"]);
        assert_eq!(
            modeline_hints("# -*- coding: utf-8 -*-"),
            Vec::<String>::new()
        );
        assert_eq!(
            modeline_hints("x\ny\n/* vim: set ts=4 sw=4 ft=c : */"),
            ["c"]
        );
        assert_eq!(modeline_hints("// vi: syntax=cpp"), ["cpp"]);
        assert_eq!(modeline_hints("nothing here"), Vec::<String>::new());
    }

    #[test]
    fn modelines_only_in_head_and_tail() {
        let mut text = String::new();
        for _ in 0..20 {
            text.push_str("line\n");
        }
        text.insert_str(text.len() / 2, "# vim: ft=lua\n");
        assert!(modeline_hints(&text).is_empty());
        text.push_str("# vim: ft=perl\n");
        assert_eq!(modeline_hints(&text), ["perl"]);
    }

    #[test]
    fn content_pattern_weights() {
        assert_eq!(parse_content_pattern("@3 <\\?php"), (3, "<\\?php"));
        assert_eq!(parse_content_pattern("fn main"), (1, "fn main"));
        assert_eq!(parse_content_pattern("@x y"), (1, "@x y"));
        assert_eq!(parse_content_pattern("@2"), (1, "@2"));
    }

    #[test]
    fn extension_candidates_longest_first() {
        assert_eq!(extension_candidates("a.d.ts"), ["d.ts", "ts"]);
        assert_eq!(extension_candidates("Main.JAVA"), ["java"]);
        assert_eq!(extension_candidates(".zshrc"), Vec::<String>::new());
        assert_eq!(extension_candidates("Makefile"), Vec::<String>::new());
        assert_eq!(extension_candidates("a."), Vec::<String>::new());
    }
}
