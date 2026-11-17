//! The find bar's "Insert Pattern" menu: built-in regex snippets, replacement
//! insertions and the user's own patterns (`TextFinderUserPatterns`).

use serde::{Deserialize, Serialize};

use crate::matcher::{MatchError, MatchMode, Matcher, MatcherOptions, literal_source};

/// One menu entry: what to show and what to insert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertPattern {
    /// Menu title.
    pub title: String,
    /// Text inserted into the field.
    pub text: String,
}

fn p(title: &str, text: &str) -> InsertPattern {
    InsertPattern {
        title: title.into(),
        text: text.into(),
    }
}

/// Built-in snippets for the search field, in menu order.
pub fn builtin_find_patterns() -> Vec<InsertPattern> {
    vec![
        p("Newline", "\\n"),
        p("Any Characters", "(.+)"),
        p("Any Characters (Non-Greedy)", "(.+?)"),
        p("Any Word Characters", "(\\w+)"),
        p("Any Non-Word Characters", "(\\W+)"),
        p("Word Boundary", "\\b"),
        p("Whitespace", "[ \\t]+"),
        p("Whitespace And Newline", "\\s+"),
        p("Digits", "\\d+"),
        p("Leading Whitespace", "^[ \\t]+"),
        p("Trailing Whitespace", "(?<!^)[ \\t]+$"),
    ]
}

/// Built-in snippets for the replace field: the case modifiers and the
/// entire match. Group insertions come from [`capture_group_pattern`] and
/// [`named_group_pattern`] because they depend on the current search.
pub fn builtin_replace_patterns() -> Vec<InsertPattern> {
    vec![
        p("Entire Match", "$0"),
        p("Make Next Character Upper Case", "\\u"),
        p("Make Next Character Lower Case", "\\l"),
        p("Make Remaining Characters Upper Case", "\\U"),
        p("Make Remaining Characters Lower Case", "\\L"),
        p("End Case Change", "\\E"),
    ]
}

/// "Capture Group n" -> `$n`.
pub fn capture_group_pattern(n: usize) -> InsertPattern {
    InsertPattern {
        title: format!("Capture Group {n}"),
        text: format!("${n}"),
    }
}

/// "Named Capture Group "name"" -> `${name}`.
pub fn named_group_pattern(name: &str) -> InsertPattern {
    InsertPattern {
        title: format!("Named Capture Group \"{name}\""),
        text: format!("${{{name}}}"),
    }
}

/// The replace-field insertions that apply to `matcher`: entire match, one per
/// numbered group, one per named group, then the case modifiers.
pub fn replace_insertions_for(matcher: &Matcher) -> Vec<InsertPattern> {
    let mut out = vec![builtin_replace_patterns().remove(0)];
    out.extend((1..=matcher.group_count()).map(capture_group_pattern));
    out.extend(matcher.group_names().iter().map(|n| named_group_pattern(n)));
    out.extend(builtin_replace_patterns().into_iter().skip(1));
    out
}

/// "Escaped Current Replacement": `text` with `$` and `\` escaped so that it
/// is inserted literally by a regex-mode replacement.
pub fn escaped_replacement(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for c in text.chars() {
        match c {
            '$' => out.push_str("$$"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// "Current Search As Regular Expression": the regex that finds the same as
/// `query` does in `mode` (ignoring the ignore-case flag, which is a separate
/// toggle).
pub fn current_search_as_regex(query: &str, mode: MatchMode) -> String {
    if mode == MatchMode::Regex || query.is_empty() {
        query.to_string()
    } else {
        literal_source(query, mode, false)
    }
}

/// A user pattern: a name and a regular expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPattern {
    /// Name shown in the menu ("Enter A Name For Your Pattern").
    pub name: String,
    /// The regular expression ("Enter A Regular Expression").
    pub pattern: String,
}

/// The user's added patterns ("Add Pattern...", "Clear Added Patterns...").
/// Serialises as a plain list (`TextFinderUserPatterns`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserPatterns(Vec<UserPattern>);

impl UserPatterns {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a pattern after checking that it compiles. A pattern with the same
    /// name is replaced.
    pub fn add(&mut self, name: &str, pattern: &str) -> Result<(), MatchError> {
        Matcher::new(
            pattern,
            &MatcherOptions {
                mode: MatchMode::Regex,
                ..Default::default()
            },
        )?;
        let item = UserPattern {
            name: name.trim().to_string(),
            pattern: pattern.to_string(),
        };
        match self.0.iter_mut().find(|x| x.name == item.name) {
            Some(existing) => *existing = item,
            None => self.0.push(item),
        }
        Ok(())
    }

    /// Removes the pattern called `name`; whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|x| x.name != name);
        self.0.len() != before
    }

    /// Removes all patterns.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// The patterns in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &UserPattern> {
        self.0.iter()
    }

    /// Number of patterns.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Menu entries for the user patterns.
    pub fn as_insert_patterns(&self) -> Vec<InsertPattern> {
        self.0
            .iter()
            .map(|x| InsertPattern {
                title: x.name.clone(),
                text: x.pattern.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_patterns_compile_and_work() {
        for pat in builtin_find_patterns() {
            let m = Matcher::new(
                &pat.text,
                &MatcherOptions {
                    mode: MatchMode::Regex,
                    ..Default::default()
                },
            );
            assert!(m.is_ok(), "{} = {}", pat.title, pat.text);
        }
        let trailing = Matcher::simple("(?<!^)[ \\t]+$", MatchMode::Regex, true).unwrap();
        let text = "a  \n   \nb \t\nc";
        let found: Vec<_> = trailing.find_all(text).unwrap();
        // a blank line of spaces keeps its first char out of the match
        assert_eq!(found, vec![1..3, 5..7, 9..11]);
        let leading = Matcher::simple("^[ \\t]+", MatchMode::Regex, true).unwrap();
        assert_eq!(leading.find_all("  a\nb\n\tc").unwrap(), vec![0..2, 6..7]);
    }

    #[test]
    fn current_search_as_regex_roundtrips() {
        let re = current_search_as_regex("a.b", MatchMode::ExactWord);
        assert_eq!(re, "\\ba\\.b\\b");
        let m = Matcher::simple(&re, MatchMode::Regex, true).unwrap();
        assert_eq!(m.find_all("a.b axb").unwrap(), vec![0..3]);
        assert_eq!(current_search_as_regex("x+", MatchMode::Regex), "x+");
    }

    #[test]
    fn insertions_follow_groups() {
        let m = Matcher::simple("(?<a>x)(y)", MatchMode::Regex, true).unwrap();
        let titles: Vec<_> = replace_insertions_for(&m)
            .into_iter()
            .map(|p| p.title)
            .collect();
        assert_eq!(titles[0], "Entire Match");
        assert!(titles.contains(&"Capture Group 2".to_string()));
        assert!(titles.contains(&"Named Capture Group \"a\"".to_string()));
        assert_eq!(escaped_replacement("a$1\\n"), "a$$1\\\\n");
    }

    #[test]
    fn user_patterns() {
        let mut u = UserPatterns::new();
        u.add("Email", r"\w+@\w+\.\w+").unwrap();
        assert!(u.add("Bad", "(").is_err());
        u.add("Email", r"\S+@\S+").unwrap();
        assert_eq!(u.len(), 1);
        let json = serde_json::to_string(&u).unwrap();
        assert_eq!(json, r#"[{"name":"Email","pattern":"\\S+@\\S+"}]"#);
        assert!(u.remove("Email"));
        assert!(u.is_empty());
    }
}
