//! Authored keyword and snippet tables for languages, used when no server can answer (and as a
//! complement when one can). The data lives in `data/keywords.toml`.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::{LspError, Result};
use crate::types::{CompletionItem, CompletionKind, CompletionSource, InsertFormat};

const KEYWORD_TABLES: &str = include_str!("../../data/keywords.toml");

/// One entry of a language's snippet table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SnippetTemplate {
    /// What the completion list shows (also matched against typed text).
    pub label: String,
    /// A one-line description.
    #[serde(default)]
    pub detail: String,
    /// The insert text, in LSP snippet syntax (`${1:name}`, `$0`).
    pub insert: String,
    /// The completion kind; `snippet` by default.
    #[serde(default)]
    pub kind: TableKind,
}

/// The kinds a static table entry can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    /// A snippet with placeholders.
    #[default]
    Snippet,
    /// A keyword.
    Keyword,
    /// A function.
    Function,
    /// A method.
    Method,
    /// A class.
    Class,
    /// A field or property.
    Field,
    /// A variable.
    Variable,
    /// A constant.
    Constant,
    /// A module.
    Module,
}

impl TableKind {
    /// The completion kind shown in the list.
    pub fn completion_kind(self) -> CompletionKind {
        match self {
            Self::Snippet => CompletionKind::Snippet,
            Self::Keyword => CompletionKind::Keyword,
            Self::Function => CompletionKind::Function,
            Self::Method => CompletionKind::Method,
            Self::Class => CompletionKind::Class,
            Self::Field => CompletionKind::Field,
            Self::Variable => CompletionKind::Variable,
            Self::Constant => CompletionKind::Constant,
            Self::Module => CompletionKind::Module,
        }
    }
}

/// The keywords, snippets and word characters of one language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LanguageTable {
    /// The ScratchRunner language id.
    pub id: String,
    /// File extensions (without the dot).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Characters that belong to words besides letters, digits and `_` (`$`, `-`, `@`).
    #[serde(default)]
    pub extra_word_chars: String,
    /// Keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Snippets; the TOML key is `[[language.snippet]]`.
    #[serde(default, rename = "snippet")]
    pub snippets: Vec<SnippetTemplate>,
}

/// The embedded tables, indexed by language id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageTables {
    languages: Vec<LanguageTable>,
}

#[derive(Deserialize)]
struct RawTables {
    #[serde(rename = "language", default)]
    languages: Vec<LanguageTable>,
}

impl LanguageTables {
    /// The tables shipped with the app.
    pub fn builtin() -> &'static LanguageTables {
        static BUILTIN: OnceLock<LanguageTables> = OnceLock::new();
        BUILTIN.get_or_init(|| {
            LanguageTables::from_toml(KEYWORD_TABLES)
                .unwrap_or_else(|error| panic!("the embedded keyword tables are invalid: {error}"))
        })
    }

    /// Loads tables from TOML.
    pub fn from_toml(text: &str) -> Result<Self> {
        let raw: RawTables =
            toml::from_str(text).map_err(|error| LspError::Config(format!("keywords: {error}")))?;
        let mut seen = std::collections::HashSet::new();
        for language in &raw.languages {
            if language.id.is_empty() {
                return Err(LspError::Config(
                    "a keyword table has no language id".to_owned(),
                ));
            }
            if !seen.insert(language.id.as_str()) {
                return Err(LspError::Config(format!(
                    "duplicate keyword table {}",
                    language.id
                )));
            }
            for snippet in &language.snippets {
                if snippet.label.is_empty() {
                    return Err(LspError::Config(format!(
                        "a snippet of {} has no label",
                        language.id
                    )));
                }
            }
        }
        Ok(Self {
            languages: raw.languages,
        })
    }

    /// The table of `language`, if it has one.
    pub fn get(&self, language: &str) -> Option<&LanguageTable> {
        self.languages.iter().find(|table| table.id == language)
    }

    /// The table whose extensions contain `extension` (the first match, in table order).
    pub fn for_extension(&self, extension: &str) -> Option<&LanguageTable> {
        self.languages
            .iter()
            .find(|table| table.extensions.iter().any(|item| item == extension))
    }

    /// All tables.
    pub fn languages(&self) -> &[LanguageTable] {
        &self.languages
    }

    /// The word characters of `language`, or `""`.
    pub fn extra_word_chars(&self, language: &str) -> &str {
        self.get(language)
            .map(|table| table.extra_word_chars.as_str())
            .unwrap_or("")
    }

    /// A language's first extension, for naming an unsaved document.
    pub fn extension(&self, language: &str) -> Option<&str> {
        self.get(language)
            .and_then(|table| table.extensions.first())
            .map(String::as_str)
    }

    /// The static completions of `language` whose label starts with `prefix` (case-insensitive),
    /// snippets first. `limit` caps the result.
    pub fn complete(&self, language: &str, prefix: &str, limit: usize) -> Vec<CompletionItem> {
        let Some(table) = self.get(language) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        let matches = |label: &str| {
            prefix.is_empty()
                || label
                    .to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
        };
        for snippet in &table.snippets {
            if !matches(&snippet.label) {
                continue;
            }
            let mut item = CompletionItem::simple(
                snippet.label.clone(),
                snippet.kind.completion_kind(),
                CompletionSource::Static,
            );
            item.detail = (!snippet.detail.is_empty()).then(|| snippet.detail.clone());
            item.insert_text = Some(snippet.insert.clone());
            item.insert_format = InsertFormat::Snippet;
            item.sort_text = Some(format!("0{}", snippet.label));
            items.push(item);
        }
        for keyword in &table.keywords {
            if !matches(keyword) {
                continue;
            }
            let mut item = CompletionItem::simple(
                keyword.clone(),
                CompletionKind::Keyword,
                CompletionSource::Static,
            );
            item.sort_text = Some(format!("1{keyword}"));
            items.push(item);
        }
        items.sort_by(|left, right| {
            left.sort_key()
                .cmp(right.sort_key())
                .then_with(|| left.label.cmp(&right.label))
        });
        items.truncate(limit);
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tables_parse_and_have_the_main_languages() {
        let tables = LanguageTables::builtin();
        for language in [
            "python",
            "rust",
            "go",
            "javascript-node",
            "typescript-node",
            "shell",
            "swift",
            "c",
            "cpp",
            "java",
            "ruby",
            "php",
            "lua",
            "kotlin",
            "haskell",
            "perl",
            "html",
            "css",
            "json",
            "yaml",
            "toml",
            "xml",
            "markdown",
            "latex",
            "objective-c",
            "csharp-dotnet",
            "applescript",
            "plain-text",
        ] {
            assert!(
                tables.get(language).is_some(),
                "missing table for {language}"
            );
        }
        let python = tables.get("python").unwrap();
        assert!(python.keywords.iter().any(|keyword| keyword == "def"));
        assert!(python.snippets.iter().any(|snippet| snippet.label == "def"));
        assert_eq!(tables.extra_word_chars("shell"), "$");
        assert_eq!(tables.extra_word_chars("python"), "");
        assert_eq!(tables.extension("python"), Some("py"));
        assert_eq!(tables.for_extension("rs").unwrap().id, "rust");
    }

    #[test]
    fn completes_with_prefix_and_snippets_first() {
        let tables = LanguageTables::builtin();
        let items = tables.complete("python", "de", 10);
        assert_eq!(items[0].label, "def");
        assert_eq!(items[0].kind, Some(CompletionKind::Snippet));
        assert_eq!(items[0].insert_format, InsertFormat::Snippet);
        assert!(items.iter().any(|item| item.label == "del"));
        assert!(
            tables
                .complete("python", "DEF", 10)
                .iter()
                .any(|item| item.label == "def")
        );
        assert!(tables.complete("python", "zzz", 10).is_empty());
        assert!(tables.complete("no-such-language", "", 10).is_empty());
        assert_eq!(tables.complete("python", "", 2).len(), 2);
    }

    #[test]
    fn rejects_bad_tables() {
        assert!(LanguageTables::from_toml("[[language]]\nid = \"\"\n").is_err());
        let duplicate = "[[language]]\nid = \"a\"\n[[language]]\nid = \"a\"\n";
        assert!(LanguageTables::from_toml(duplicate).is_err());
        assert!(
            LanguageTables::from_toml(
                "[[language]]\nid = \"a\"\n[[language.snippet]]\ninsert = \"x\"\n"
            )
            .is_err()
        );
    }
}
