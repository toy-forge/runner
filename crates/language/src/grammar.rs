//! Editor facts for TextMate grammar scopes.
//!
//! CodeRunner identifies a language's syntax by a TextMate scope name
//! (`source.python`). The importer ([`crate::from_cr_language_plist`]) uses this
//! table to pick the editor syntax and the comment markers for an imported
//! language. Everything here is our own data.

use crate::model::CommentMarkers;

/// What the editor needs to know about a grammar scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarInfo {
    /// The NSH editor language name.
    pub editor_syntax: &'static str,
    /// A syntect hint when NSH does not know the language by name.
    pub syntax_hint: Option<&'static str>,
    /// Line comment marker.
    pub line_comment: Option<&'static str>,
    /// Block comment markers.
    pub block_comment: Option<(&'static str, &'static str)>,
}

impl GrammarInfo {
    /// The comment markers as the model type.
    pub fn comments(&self) -> CommentMarkers {
        CommentMarkers {
            line: self.line_comment.map(str::to_owned),
            block: self
                .block_comment
                .map(|(open, close)| (open.to_owned(), close.to_owned())),
        }
    }
}

const SLASH: (Option<&str>, Option<(&str, &str)>) = (Some("//"), Some(("/*", "*/")));
const HASH: (Option<&str>, Option<(&str, &str)>) = (Some("#"), None);

fn info(
    editor_syntax: &'static str,
    syntax_hint: Option<&'static str>,
    comments: (Option<&'static str>, Option<(&'static str, &'static str)>),
) -> GrammarInfo {
    GrammarInfo {
        editor_syntax,
        syntax_hint,
        line_comment: comments.0,
        block_comment: comments.1,
    }
}

/// The editor facts for a grammar scope (`source.js.node` is looked up as
/// `source.js`), `None` for scopes this table does not know.
///
/// ```
/// use scratch_language::grammar_info;
/// assert_eq!(grammar_info("source.python").unwrap().editor_syntax, "Python");
/// assert_eq!(grammar_info("source.js.jquery").unwrap().editor_syntax, "JavaScript");
/// assert!(grammar_info("source.nonsense").is_none());
/// ```
pub fn grammar_info(scope: &str) -> Option<GrammarInfo> {
    let scope = scope.trim().to_ascii_lowercase();
    let mut candidate = scope.as_str();
    loop {
        if let Some(found) = exact(candidate) {
            return Some(found);
        }
        candidate = candidate.rsplit_once('.')?.0;
    }
}

fn exact(scope: &str) -> Option<GrammarInfo> {
    Some(match scope {
        "text.plain" => info("Plain Text", None, (None, None)),
        "source.c" => info("C", None, SLASH),
        "source.c++" => info("C++", None, SLASH),
        "source.objc" => info("Objective-C", None, SLASH),
        "source.objc++" => info("Objective-C++", None, SLASH),
        "source.cs" => info("C#", None, SLASH),
        "source.css" => info("CSS", None, (None, Some(("/*", "*/")))),
        "source.go" => info("Go", None, SLASH),
        "source.haskell" => info("Haskell", None, (Some("--"), Some(("{-", "-}")))),
        "text.html.basic" | "text.html" => info("HTML", None, (None, Some(("<!--", "-->")))),
        "source.java" => info("Java", None, SLASH),
        "source.js" => info("JavaScript", None, SLASH),
        "source.ts" | "source.tsx" => info("TypeScript", None, SLASH),
        "source.kotlin" => info("Kotlin", None, SLASH),
        "text.tex.latex" | "text.tex" => info("LaTeX", Some("tex"), (Some("%"), None)),
        "source.lua" => info("Lua", None, (Some("--"), Some(("--[[", "]]")))),
        "text.html.markdown" => info("Markdown", None, (None, Some(("<!--", "-->")))),
        "text.html.php" | "source.php" => info("PHP", None, SLASH),
        "source.perl" => info("Perl", None, HASH),
        "source.python" => info("Python", None, HASH),
        "source.ruby" => info("Ruby", None, HASH),
        "source.rust" => info("Rust", None, SLASH),
        "source.shell" => info("Shell", None, HASH),
        "source.swift" => info("Swift", None, SLASH),
        "source.applescript" => info(
            "AppleScript",
            Some("applescript"),
            (Some("--"), Some(("(*", "*)"))),
        ),
        "source.json" => info("JSON", None, (None, None)),
        "source.yaml" => info("YAML", None, HASH),
        "text.xml" => info("XML", None, (None, Some(("<!--", "-->")))),
        "source.toml" => info("TOML", None, HASH),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_lookup() {
        assert_eq!(
            grammar_info("source.js.node").unwrap().editor_syntax,
            "JavaScript"
        );
        assert_eq!(
            grammar_info("source.Kotlin").unwrap().editor_syntax,
            "Kotlin"
        );
        assert_eq!(
            grammar_info("text.html.php.blade").unwrap().editor_syntax,
            "PHP"
        );
        assert!(grammar_info("source.unknown").is_none());
        assert!(grammar_info("").is_none());
    }

    #[test]
    fn comments() {
        let c = grammar_info("source.c").unwrap().comments();
        assert_eq!(c.line.as_deref(), Some("//"));
        assert_eq!(c.block, Some(("/*".into(), "*/".into())));
    }
}
