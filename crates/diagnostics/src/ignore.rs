//! Ignore lists and auto-fix rules.
//!
//! Both are plain text, one rule per line, edited in the preferences sheets
//! "Globally Ignored Diagnostic Messages", "Ignored Diagnostic Messages For
//! Document" and "Automatically Applied Fixes". A rule is a regular
//! expression searched in the diagnostic's message and in its message id
//! (`-Wobjc-root-class`, `SC2086`, `E0425`). A line that is not a valid
//! regular expression matches as literal text.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// One line of a [`RuleList`].
#[derive(Debug, Clone)]
struct Rule {
    source: String,
    regex: Option<Regex>,
}

impl Rule {
    fn new(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            regex: Regex::new(source).ok(),
        }
    }

    fn matches(&self, text: &str) -> bool {
        match &self.regex {
            Some(re) => re.is_match(text),
            None => text.contains(&self.source),
        }
    }
}

/// A list of message rules; see the module docs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "Vec<String>", into = "Vec<String>")]
pub struct RuleList {
    rules: Vec<Rule>,
}

/// Messages that are not shown (global list or per-document list).
pub type IgnoreList = RuleList;
/// Messages whose fix is applied automatically before a run.
pub type AutoFixRules = RuleList;

impl From<Vec<String>> for RuleList {
    fn from(lines: Vec<String>) -> Self {
        let mut list = RuleList::default();
        for l in lines {
            list.add(&l);
        }
        list
    }
}

impl From<RuleList> for Vec<String> {
    fn from(list: RuleList) -> Self {
        list.lines()
    }
}

impl PartialEq for RuleList {
    fn eq(&self, other: &Self) -> bool {
        self.lines() == other.lines()
    }
}

impl Eq for RuleList {}

impl RuleList {
    /// Parses the sheet text: one rule per line; blank lines are dropped and
    /// lines are trimmed.
    pub fn from_text(text: &str) -> Self {
        let mut list = Self::default();
        for line in text.lines() {
            list.add(line);
        }
        list
    }

    /// The sheet text (rules joined by newlines).
    pub fn to_text(&self) -> String {
        self.lines().join("\n")
    }

    /// The rules.
    pub fn lines(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.source.clone()).collect()
    }

    /// Number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether there are no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Adds a rule (trimmed; blank and duplicate lines are ignored).
    pub fn add(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || self.rules.iter().any(|r| r.source == line) {
            return;
        }
        self.rules.push(Rule::new(line));
    }

    /// Adds a rule matching exactly this message (`Ignore "..." In Document`).
    pub fn add_exact(&mut self, message: &str) {
        self.add(&format!("^{}$", escape(message.trim())));
    }

    /// Adds a rule matching messages like this one (`Ignore "..."
    /// Globally`, "Messages Like"): quoted names and numbers are wildcards.
    pub fn add_like(&mut self, message: &str) {
        self.add(&like_pattern(message));
    }

    /// Adds the rule the context menus add for one diagnostic (`Ignore ...
    /// In Document`, `Automatically Apply Fix ...`): when the diagnostic has
    /// a message id the rule matches exactly that id (every message with
    /// that id, e.g. `-Wunused-variable`), otherwise messages like this one
    /// ([`RuleList::add_like`]).
    pub fn add_for_message(&mut self, message: &str, message_id: Option<&str>) {
        match message_id.filter(|id| !id.is_empty()) {
            Some(id) => self.add(&format!("^{}$", escape(id))),
            None => self.add_like(message),
        }
    }

    /// Removes the rule at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.rules.len() {
            self.rules.remove(index);
        }
    }

    /// Whether any rule matches the message or the message id.
    pub fn matches(&self, message: &str, message_id: Option<&str>) -> bool {
        self.rules
            .iter()
            .any(|r| r.matches(message) || message_id.is_some_and(|id| r.matches(id)))
    }
}

/// Escapes regex metacharacters (unlike `regex::escape` it leaves `-`, `#`,
/// `&` and `~` alone so the generated lines stay readable in the sheet).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Builds the "messages like this" regex: anchored, quoted text and digit
/// runs generalised.
pub fn like_pattern(message: &str) -> String {
    let message = message.trim();
    let mut out = String::from("^");
    let chars: Vec<char> = message.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '\'' => Some('\''),
            '"' => Some('"'),
            '`' => Some('\''),
            '\u{2018}' => Some('\u{2019}'),
            '\u{201c}' => Some('\u{201d}'),
            _ => None,
        };
        if let Some(close) = close
            && let Some(rel) = chars[i + 1..].iter().position(|x| *x == close)
        {
            out.push_str(&escape(&c.to_string()));
            out.push_str(".*?");
            out.push_str(&escape(&close.to_string()));
            i += rel + 2;
            continue;
        }
        if c.is_ascii_digit() {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            out.push_str(r"\d+");
            continue;
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip_and_matching() {
        let list = IgnoreList::from_text("  unused variable  \n\n-Wobjc-root-class\n(");
        assert_eq!(list.len(), 3);
        assert_eq!(list.to_text(), "unused variable\n-Wobjc-root-class\n(");
        assert!(list.matches("unused variable 'x'", None));
        assert!(list.matches("class 'A' defined without specifying a base class", Some("-Wobjc-root-class")));
        assert!(list.matches("a ( b", None), "invalid regex falls back to literal text");
        assert!(!list.matches("something else", Some("E0001")));
    }

    #[test]
    fn exact_and_like_rules() {
        let mut list = IgnoreList::default();
        list.add_exact("expected ';' after expression");
        assert!(list.matches("expected ';' after expression", None));
        assert!(!list.matches("expected ';' after expression!", None));
        let mut like = IgnoreList::default();
        like.add_like("unused variable 'count' at 12");
        assert!(like.matches("unused variable 'other_name' at 7", None));
        assert!(!like.matches("unused parameter 'count' at 12", None));
        like.add_like("unused variable 'count' at 12");
        assert_eq!(like.len(), 1, "duplicates are dropped");
    }

    #[test]
    fn rules_for_a_diagnostic_prefer_the_message_id() {
        let mut list = IgnoreList::default();
        list.add_for_message("unused variable 'x'", Some("-Wunused-variable"));
        assert_eq!(list.to_text(), "^-Wunused\\-variable$".replace("\\-", "-"));
        assert!(list.matches("unused variable 'q'", Some("-Wunused-variable")));
        assert!(!list.matches("unused variable 'x'", None), "the rule is about the id");
        let mut by_text = IgnoreList::default();
        by_text.add_for_message("expected 3 arguments", None);
        assert!(by_text.matches("expected 4 arguments", None));
    }

    #[test]
    fn serde_is_a_list_of_lines() {
        let list = IgnoreList::from_text("a\nb");
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(json, r#"["a","b"]"#);
        let back: IgnoreList = serde_json::from_str(&json).unwrap();
        assert_eq!(back, list);
    }
}
