//! A small scope-selector engine for console filters.
//!
//! Supports the subset of TextMate-style selectors the console filter menu and
//! its "Custom Output" field use:
//!
//! * `console.stdout` - prefix match on dotted names (`console.error` also
//!   matches `console.error.pdf`);
//! * `*` (one segment) and `**` (any number of segments): `**.debugger`;
//! * `,` or `|` - union; `&` or juxtaposition (`a b`) - both; `-x` as a prefix -
//!   not; `a - b` - `a` and not `b`; parentheses: `-(console.error, console.warning)`.
//!
//! Precedence, loosest first: union, intersection, negation.

use crate::tag::ScopeMask;

/// A selector failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid scope selector at {position}: {message}")]
pub struct SelectorError {
    /// Byte offset of the problem.
    pub position: usize,
    /// What is wrong.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Star,
    DoubleStar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Scope(Vec<Seg>),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// A parsed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSelector {
    source: String,
    expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    Union,
    Amp,
    Minus,
    Pat(String),
}

fn tokenize(src: &str) -> Result<Vec<(usize, Tok)>, SelectorError> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' => i += 1,
            b'(' => {
                out.push((i, Tok::LParen));
                i += 1;
            }
            b')' => {
                out.push((i, Tok::RParen));
                i += 1;
            }
            b',' | b'|' => {
                out.push((i, Tok::Union));
                i += 1;
            }
            b'&' => {
                out.push((i, Tok::Amp));
                i += 1;
            }
            b'-' => {
                out.push((i, Tok::Minus));
                i += 1;
            }
            _ if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'*' | b'+' | b'$') => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'.' | b'*' | b'+' | b'-' | b'$'))
                {
                    i += 1;
                }
                out.push((start, Tok::Pat(src[start..i].to_owned())));
            }
            _ => {
                return Err(SelectorError {
                    position: i,
                    message: format!("unexpected character {:?}", src[i..].chars().next().unwrap_or('?')),
                });
            }
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    at: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(_, t)| t)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.at).map_or(self.end, |(p, _)| *p)
    }
    fn err<T>(&self, message: &str) -> Result<T, SelectorError> {
        Err(SelectorError {
            position: self.pos(),
            message: message.to_owned(),
        })
    }

    fn union(&mut self) -> Result<Expr, SelectorError> {
        let mut items = vec![self.intersection()?];
        while self.peek() == Some(&Tok::Union) {
            self.at += 1;
            items.push(self.intersection()?);
        }
        Ok(if items.len() == 1 { items.remove(0) } else { Expr::Or(items) })
    }

    fn operand_start(&self) -> bool {
        matches!(self.peek(), Some(Tok::LParen | Tok::Pat(_)))
    }

    fn intersection(&mut self) -> Result<Expr, SelectorError> {
        let mut items = vec![self.unary()?];
        loop {
            match self.peek() {
                Some(Tok::Amp) => {
                    self.at += 1;
                    items.push(self.unary()?);
                }
                // `a - b`
                Some(Tok::Minus) => {
                    self.at += 1;
                    let rhs = self.unary()?;
                    items.push(Expr::Not(Box::new(rhs)));
                }
                // juxtaposition
                Some(Tok::LParen | Tok::Pat(_)) => items.push(self.unary()?),
                _ => break,
            }
        }
        Ok(if items.len() == 1 { items.remove(0) } else { Expr::And(items) })
    }

    fn unary(&mut self) -> Result<Expr, SelectorError> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.at += 1;
                let e = self.unary()?;
                Ok(Expr::Not(Box::new(e)))
            }
            Some(Tok::LParen) => {
                self.at += 1;
                let e = self.union()?;
                if self.peek() != Some(&Tok::RParen) {
                    return self.err("expected `)`");
                }
                self.at += 1;
                Ok(e)
            }
            Some(Tok::Pat(p)) => {
                let segs = p
                    .split('.')
                    .map(|s| match s {
                        "*" => Seg::Star,
                        "**" => Seg::DoubleStar,
                        other => Seg::Lit(other.to_owned()),
                    })
                    .collect();
                self.at += 1;
                Ok(Expr::Scope(segs))
            }
            _ => {
                let _ = self.operand_start();
                self.err("expected a scope name")
            }
        }
    }
}

fn seg_match(pat: &[Seg], scope: &[&str]) -> bool {
    match pat.first() {
        None => true,
        Some(Seg::Lit(l)) => !scope.is_empty() && scope[0] == l && seg_match(&pat[1..], &scope[1..]),
        Some(Seg::Star) => !scope.is_empty() && seg_match(&pat[1..], &scope[1..]),
        Some(Seg::DoubleStar) => (0..=scope.len()).any(|k| seg_match(&pat[1..], &scope[k..])),
    }
}

fn eval(e: &Expr, scopes: &[&str]) -> bool {
    match e {
        Expr::Scope(p) => scopes.iter().any(|s| {
            let parts: Vec<&str> = s.split('.').collect();
            seg_match(p, &parts)
        }),
        Expr::Not(e) => !eval(e, scopes),
        Expr::And(v) => v.iter().all(|e| eval(e, scopes)),
        Expr::Or(v) => v.iter().any(|e| eval(e, scopes)),
    }
}

impl ScopeSelector {
    /// Parses a selector.
    pub fn parse(src: &str) -> Result<ScopeSelector, SelectorError> {
        let toks = tokenize(src)?;
        if toks.is_empty() {
            return Err(SelectorError {
                position: 0,
                message: "empty selector".into(),
            });
        }
        let mut p = Parser {
            toks,
            at: 0,
            end: src.len(),
        };
        let expr = p.union()?;
        if p.at != p.toks.len() {
            return p.err("unexpected token");
        }
        Ok(ScopeSelector {
            source: src.trim().to_owned(),
            expr,
        })
    }

    /// The selector as written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether a text carrying exactly `scopes` is selected.
    pub fn matches(&self, scopes: &[&str]) -> bool {
        eval(&self.expr, scopes)
    }

    /// The answer for each of the 256 [`ScopeMask`]s, for O(1) line tests.
    pub fn truth_table(&self) -> [bool; 256] {
        let mut table = [false; 256];
        for (m, slot) in table.iter_mut().enumerate() {
            *slot = self.matches(&ScopeMask(m as u8).scopes());
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(sel: &str, scopes: &[&str]) -> bool {
        ScopeSelector::parse(sel).unwrap().matches(scopes)
    }

    #[test]
    fn prefix_and_union() {
        assert!(m("console.stdout", &["console", "console.stdout"]));
        assert!(!m("console.stdout", &["console", "console.stderr"]));
        assert!(m("console.stdout, console.stderr", &["console.stderr"]));
        assert!(m("console.error", &["console.error.pdf"]));
        assert!(!m("console.err", &["console.error"]));
    }

    #[test]
    fn wildcards() {
        assert!(m("**.debugger", &["console.debugger"]));
        assert!(m("console.*", &["console.stdout"]));
        assert!(!m("console.*", &["console"]));
        assert!(m("**", &["a"]));
    }

    #[test]
    fn negation() {
        let s = "-(console.error, console.warning, console.notice)";
        assert!(m(s, &["console", "console.stdout"]));
        assert!(!m(s, &["console", "console.stdout", "console.warning"]));
        assert!(m("console.stdout - console.error", &["console.stdout"]));
        assert!(!m("console.stdout - console.error", &["console.stdout", "console.error"]));
        assert!(m("-console.error", &["console.stdout"]));
    }

    #[test]
    fn intersection() {
        assert!(m("console.stdout & console.error", &["console.stdout", "console.error"]));
        assert!(!m("console.stdout & console.error", &["console.stdout"]));
        assert!(m("console.stdout console.error", &["console.stdout", "console.error"]));
    }

    #[test]
    fn hyphenated_names() {
        assert!(m("meta.coderunner.console-error", &["meta.coderunner.console-error"]));
    }

    #[test]
    fn errors() {
        assert!(ScopeSelector::parse("").is_err());
        assert!(ScopeSelector::parse("(a").is_err());
        assert!(ScopeSelector::parse("a b)").is_err());
        assert!(ScopeSelector::parse("a ,").is_err());
        assert!(ScopeSelector::parse("a #").is_err());
        let e = ScopeSelector::parse("a ,").unwrap_err();
        assert_eq!(e.position, 3);
    }

    #[test]
    fn truth_table_for_filter_menu() {
        use crate::tag::{Level, Stream};
        let t = ScopeSelector::parse("console.stdout, console.stderr").unwrap().truth_table();
        assert!(t[ScopeMask::of([Stream::Stdout], Level::Normal).0 as usize]);
        assert!(t[ScopeMask::of([Stream::Stderr], Level::Error).0 as usize]);
        assert!(!t[ScopeMask::of([Stream::Input], Level::Normal).0 as usize]);
        assert!(!t[ScopeMask::of([Stream::System], Level::Normal).0 as usize]);
    }
}
