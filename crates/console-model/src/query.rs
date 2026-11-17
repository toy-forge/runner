//! Text queries shared by the console filter field and find-in-console.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// What to look for in line text.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchQuery {
    /// The text, or a regular expression when [`SearchQuery::regex`] is set.
    pub text: String,
    /// Case-sensitive match (default off).
    pub case_sensitive: bool,
    /// Match whole words only.
    pub whole_word: bool,
    /// Interpret `text` as a regular expression.
    pub regex: bool,
}

impl SearchQuery {
    /// A plain, case-insensitive query.
    pub fn plain(text: impl Into<String>) -> Self {
        SearchQuery {
            text: text.into(),
            ..Default::default()
        }
    }

    /// A regular-expression query.
    pub fn regex(text: impl Into<String>) -> Self {
        SearchQuery {
            text: text.into(),
            regex: true,
            ..Default::default()
        }
    }
}

/// A query could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    /// The query text is empty (it matches nothing; clear the query instead).
    #[error("the query is empty")]
    Empty,
    /// The regular expression is invalid.
    #[error("invalid regular expression: {0}")]
    Regex(String),
}

/// A compiled [`SearchQuery`].
#[derive(Clone, Debug)]
pub struct Matcher {
    re: Regex,
}

impl Matcher {
    /// Compiles `query`.
    pub fn new(query: &SearchQuery) -> Result<Matcher, QueryError> {
        if query.text.is_empty() {
            return Err(QueryError::Empty);
        }
        let mut pattern = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        if query.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!query.case_sensitive)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| QueryError::Regex(e.to_string()))?;
        Ok(Matcher { re })
    }

    /// Whether `text` contains a match.
    pub fn is_match(&self, text: &str) -> bool {
        self.re.is_match(text)
    }

    /// The first match.
    pub fn find(&self, text: &str) -> Option<Range<usize>> {
        self.re.find(text).map(|m| m.range())
    }

    /// Every non-empty, non-overlapping match, left to right.
    pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
        self.re
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
            .collect()
    }

    /// The last match starting before `before` (byte offset; `text.len()` for
    /// the last of the line).
    pub fn rfind_before(&self, text: &str, before: usize) -> Option<Range<usize>> {
        self.find_all(text).into_iter().rev().find(|r| r.start < before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_escaped_and_case_insensitive() {
        let m = Matcher::new(&SearchQuery::plain("a.b")).unwrap();
        assert!(m.is_match("xA.Bx"));
        assert!(!m.is_match("axb"));
    }

    #[test]
    fn whole_word_and_case() {
        let q = SearchQuery {
            text: "err".into(),
            whole_word: true,
            case_sensitive: true,
            ..Default::default()
        };
        let m = Matcher::new(&q).unwrap();
        assert!(m.is_match("an err here"));
        assert!(!m.is_match("error"));
        assert!(!m.is_match("an ERR here"));
    }

    #[test]
    fn regex_and_errors() {
        let m = Matcher::new(&SearchQuery::regex(r"\d+")).unwrap();
        assert_eq!(m.find_all("a1 b22 c"), vec![1..2, 4..6]);
        assert!(matches!(Matcher::new(&SearchQuery::regex("(")), Err(QueryError::Regex(_))));
        assert_eq!(Matcher::new(&SearchQuery::plain("")).unwrap_err(), QueryError::Empty);
    }

    #[test]
    fn rfind() {
        let m = Matcher::new(&SearchQuery::plain("ab")).unwrap();
        assert_eq!(m.rfind_before("ab ab ab", 8), Some(6..8));
        assert_eq!(m.rfind_before("ab ab ab", 6), Some(3..5));
        assert_eq!(m.rfind_before("ab", 0), None);
    }
}
