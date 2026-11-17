//! Replacement templates: `$1`, `${name}`, `$0`/`$&`, `\1`, escapes and the
//! case modifiers `\U \L \E \u \l` (the dialect of CodeRunner's Oniguruma
//! based find bar).
//!
//! | Syntax | Meaning |
//! |--|--|
//! | `$0`, `$&`, `\0` | the entire match |
//! | `$1`..`$99`, `${1}`, `\1`..`\9` | capture group (a missing group is empty) |
//! | `${name}`, `$<name>` | named group (unknown names are an error) |
//! | `\n \t \r \\ \$` | newline, tab, carriage return, backslash, dollar |
//! | `$$` | a dollar sign |
//! | `\U` / `\L` | upper / lower case everything up to `\E` or the end |
//! | `\u` / `\l` | upper / lower case the next character only |
//! | `\E` | end `\U` / `\L` |
//!
//! Any other `\x` stays as written (backslash included).

use crate::matcher::MatchError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    Literal(String),
    Group(usize),
    Case(CaseOp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseOp {
    UpperAll,
    LowerAll,
    End,
    UpperNext,
    LowerNext,
}

/// A parsed replacement template. Create with
/// [`Matcher::compile_replacement`](crate::Matcher::compile_replacement) or
/// [`Replacement::literal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) source: String,
}

impl Replacement {
    /// A replacement that inserts `text` as written (used by the non-regex
    /// matching modes).
    pub fn literal(text: &str) -> Self {
        Self {
            pieces: if text.is_empty() {
                Vec::new()
            } else {
                vec![Piece::Literal(text.to_string())]
            },
            source: text.to_string(),
        }
    }

    /// The template text this was compiled from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether the template never refers to the match (a pure literal).
    pub fn is_literal(&self) -> bool {
        self.pieces.iter().all(|p| matches!(p, Piece::Literal(_)))
    }

    /// Parses `template`. `group_count` counts group 0 (as `captures_len`),
    /// `names` maps group names to indexes.
    pub(crate) fn parse(
        template: &str,
        group_count: usize,
        names: &dyn Fn(&str) -> Option<usize>,
    ) -> Result<Self, MatchError> {
        let mut pieces: Vec<Piece> = Vec::new();
        let mut lit = String::new();
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        macro_rules! flush {
            () => {
                if !lit.is_empty() {
                    pieces.push(Piece::Literal(std::mem::take(&mut lit)));
                }
            };
        }
        while i < chars.len() {
            let c = chars[i];
            match c {
                '$' => {
                    let Some(&n) = chars.get(i + 1) else {
                        lit.push('$');
                        i += 1;
                        continue;
                    };
                    match n {
                        '$' => {
                            lit.push('$');
                            i += 2;
                        }
                        '&' => {
                            flush!();
                            pieces.push(Piece::Group(0));
                            i += 2;
                        }
                        '{' | '<' => {
                            let close = if n == '{' { '}' } else { '>' };
                            if let Some(rel) = chars[i + 2..].iter().position(|&c| c == close) {
                                let name: String = chars[i + 2..i + 2 + rel].iter().collect();
                                flush!();
                                if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
                                    pieces.push(Piece::Group(name.parse().unwrap_or(usize::MAX)));
                                } else if let Some(idx) = names(&name) {
                                    pieces.push(Piece::Group(idx));
                                } else {
                                    return Err(MatchError::UnknownGroup(name));
                                }
                                i += 2 + rel + 1;
                            } else {
                                lit.push('$');
                                i += 1;
                            }
                        }
                        d if d.is_ascii_digit() => {
                            // Greedy up to two digits, backing off to one when the
                            // two-digit group does not exist (`$10` with 3 groups = `$1` + "0").
                            let mut end = i + 2;
                            let mut num = d.to_digit(10).unwrap_or(0) as usize;
                            if let Some(d2) = chars.get(end).and_then(|c| c.to_digit(10)) {
                                let two = num * 10 + d2 as usize;
                                if two < group_count {
                                    num = two;
                                    end += 1;
                                }
                            }
                            flush!();
                            pieces.push(Piece::Group(num));
                            i = end;
                        }
                        _ => {
                            lit.push('$');
                            i += 1;
                        }
                    }
                }
                '\\' => {
                    let Some(&n) = chars.get(i + 1) else {
                        lit.push('\\');
                        i += 1;
                        continue;
                    };
                    i += 2;
                    match n {
                        'n' => lit.push('\n'),
                        't' => lit.push('\t'),
                        'r' => lit.push('\r'),
                        '\\' => lit.push('\\'),
                        '$' => lit.push('$'),
                        d if d.is_ascii_digit() => {
                            flush!();
                            pieces.push(Piece::Group(d.to_digit(10).unwrap_or(0) as usize));
                        }
                        'U' | 'L' | 'E' | 'u' | 'l' => {
                            flush!();
                            pieces.push(Piece::Case(match n {
                                'U' => CaseOp::UpperAll,
                                'L' => CaseOp::LowerAll,
                                'E' => CaseOp::End,
                                'u' => CaseOp::UpperNext,
                                _ => CaseOp::LowerNext,
                            }));
                        }
                        other => {
                            lit.push('\\');
                            lit.push(other);
                        }
                    }
                }
                other => {
                    lit.push(other);
                    i += 1;
                }
            }
        }
        flush!();
        Ok(Self {
            pieces,
            source: template.to_string(),
        })
    }

    /// Appends the expansion for one match to `out`. `group(i)` returns the
    /// text of group `i` (0 = whole match) or `None`.
    pub(crate) fn expand<'a>(&self, group: impl Fn(usize) -> Option<&'a str>, out: &mut String) {
        let mut case = CaseState::default();
        for p in &self.pieces {
            match p {
                Piece::Literal(s) => case.push(s, out),
                Piece::Group(i) => {
                    if let Some(s) = group(*i) {
                        case.push(s, out);
                    }
                }
                Piece::Case(op) => case.apply(*op),
            }
        }
    }
}

#[derive(Default)]
struct CaseState {
    mode: Option<bool>, // Some(true) upper, Some(false) lower
    next: Option<bool>,
}

impl CaseState {
    fn apply(&mut self, op: CaseOp) {
        match op {
            CaseOp::UpperAll => self.mode = Some(true),
            CaseOp::LowerAll => self.mode = Some(false),
            CaseOp::End => {
                self.mode = None;
                self.next = None;
            }
            CaseOp::UpperNext => self.next = Some(true),
            CaseOp::LowerNext => self.next = Some(false),
        }
    }

    fn push(&mut self, s: &str, out: &mut String) {
        if self.mode.is_none() && self.next.is_none() {
            out.push_str(s);
            return;
        }
        for c in s.chars() {
            let mode = self.next.take().or(self.mode);
            match mode {
                Some(true) => out.extend(c.to_uppercase()),
                Some(false) => out.extend(c.to_lowercase()),
                None => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(t: &str) -> Replacement {
        Replacement::parse(t, 4, &|n| (n == "word").then_some(2)).unwrap()
    }

    fn run(t: &str, groups: &[&str]) -> String {
        let r = parse(t);
        let mut out = String::new();
        r.expand(|i| groups.get(i).copied(), &mut out);
        out
    }

    #[test]
    fn groups_and_escapes() {
        assert_eq!(
            run("[$1|${2}|$0|$&|\\3]", &["all", "a", "b", "c"]),
            "[a|b|all|all|c]"
        );
        assert_eq!(run("${word}!", &["x", "p", "q"]), "q!");
        assert_eq!(run("a\\tb\\nc\\\\ $$ \\$", &[]), "a\tb\nc\\ $ $");
        assert_eq!(run("$7", &["x"]), "");
        assert_eq!(run("cost $", &[]), "cost $");
        assert_eq!(run("\\q", &[]), "\\q");
    }

    #[test]
    fn two_digit_groups_back_off() {
        // 4 groups total: $10 is group 1 followed by "0".
        assert_eq!(run("$10", &["", "A"]), "A0");
        let r = Replacement::parse("$10", 12, &|_| None).unwrap();
        let mut out = String::new();
        r.expand(|i| (i == 10).then_some("ten"), &mut out);
        assert_eq!(out, "ten");
    }

    #[test]
    fn unknown_name_is_error() {
        assert_eq!(
            Replacement::parse("${nope}", 1, &|_| None),
            Err(MatchError::UnknownGroup("nope".into()))
        );
    }

    #[test]
    fn case_modifiers() {
        assert_eq!(run("\\U$1\\E-$2", &["", "abc", "Def"]), "ABC-Def");
        assert_eq!(run("\\L$1 $2", &["", "ABC", "DEF"]), "abc def");
        assert_eq!(run("\\u$1 \\l$2", &["", "abc", "DEF"]), "Abc dEF");
        assert_eq!(run("\\u\\L$1", &["", "hELLO"]), "Hello");
        assert_eq!(run("\\Ufoo\\Ebar", &[]), "FOObar");
        assert_eq!(run("\\Ustraße", &[]), "STRASSE");
        assert_eq!(run("\\u$1", &["", "élan"]), "Élan");
    }
}
