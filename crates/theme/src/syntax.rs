//! Syntax lookup: bat's syntaxes (two-face's set), exactly the set NSH's
//! editor highlights with, found by the same names, extensions and aliases.

use std::sync::OnceLock;

use syntect::parsing::{SyntaxReference, SyntaxSet};

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

/// bat's syntaxes with line breaks ("newlines" mode: each line passed to the
/// parser ends in `\n`). Loaded on first use (about 10-30 ms) and kept for
/// the process.
pub fn syntax_set() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// Whether `language` names plain text (no highlighting).
pub fn is_plain(language: &str) -> bool {
    let language = language.trim();
    ["", "text", "txt", "plain", "plaintext", "plain text"]
        .iter()
        .any(|plain| language.eq_ignore_ascii_case(plain))
}

/// The syntax for an editor language name (`"Rust"`, `"C#"`, `"Shell"`), an
/// extension (`"rs"`, `"tsx"`) or a file name (`"Dockerfile"`). `None` for
/// plain text and unknown languages. Same alias table as NSH's editor plus
/// the CodeRunner language names.
pub fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let language = language.trim();
    if is_plain(language) {
        return None;
    }
    let alias = match language.to_ascii_lowercase().as_str() {
        "shell" | "console" | "shellscript" | "shell script" | "bash" | "zsh" => "sh",
        "golang" => "go",
        // TSX reads JSX too (no JSX grammar of its own in the set).
        "jsx" => "tsx",
        "mjs" | "cjs" | "javascript (node.js)" | "javascript (browser)" | "javascript (jquery)" => "js",
        "typescript (node.js)" | "typescript (browser)" => "ts",
        "c++" => "cpp",
        "c#" | "csharp" | "c# (.net)" | "c# (mono)" | "c# (.net script)" => "cs",
        "f#" | "fsharp" => "fs",
        "jsonc" | "json5" => "json",
        "objective-c" | "objc" => "m",
        "objective-c++" | "objc++" | "objcpp" => "mm",
        "docker" => "Dockerfile",
        "make" => "Makefile",
        "vue" | "svelte" => "html",
        "latex" => "tex",
        _ => language,
    };
    syntax_set()
        .find_syntax_by_token(alias)
        .filter(|syntax| syntax.name != "Plain Text")
}

/// The display names of all syntaxes, sorted.
pub fn syntax_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = syntax_set()
        .syntaxes()
        .iter()
        .filter(|s| !s.hidden)
        .map(|s| s.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name_extension_and_alias() {
        for name in ["Rust", "rs", "Python", "py", "C#", "Shell", "TypeScript", "tsx", "Go", "JSON", "Markdown", "C", "Java", "Swift"] {
            assert!(find_syntax(name).is_some(), "missing syntax for {name}");
        }
        assert!(find_syntax("Plain Text").is_none());
        assert!(find_syntax("").is_none());
        assert!(find_syntax("definitely-not-a-language").is_none());
    }

    /// Coverage of CodeRunner's 29 languages in bat's set (research open
    /// question 7); printed, only the core ones are asserted.
    #[test]
    fn coverage_of_coderunner_languages() {
        let wanted = [
            "AppleScript", "C", "C++", "C# (.NET)", "CSS", "Go", "Haskell", "HTML", "Java", "JavaScript (Node.js)",
            "Kotlin", "LaTeX", "Lua", "Markdown", "Objective-C", "Objective-C++", "PHP", "Perl", "Python", "Ruby",
            "Rust", "Shell Script", "Swift", "TypeScript (Node.js)", "JSON", "XML", "YAML", "SQL", "Dart", "R", "Scala",
        ];
        let missing: Vec<&str> = wanted.iter().copied().filter(|n| find_syntax(n).is_none()).collect();
        eprintln!("syntaxes in the set: {}; missing for: {missing:?}", syntax_names().len());
        for core in ["C", "C++", "Go", "Java", "Python", "Ruby", "Rust", "Swift", "Lua", "Perl", "PHP", "HTML", "CSS"] {
            assert!(find_syntax(core).is_some(), "{core}");
        }
    }
}
