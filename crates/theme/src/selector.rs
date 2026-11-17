//! Scope selectors: the TextMate selector language with CodeRunner's
//! extensions (see CodeRunner's help page "Scope Selectors").
//!
//! Syntax accepted (loosest to tightest binding):
//!
//! | Form | Meaning |
//! |---|---|
//! | `a = b` | exclusive intersection: the innermost scope matched by `a` must also match `b` |
//! | `a, b` / `a \| b` | union |
//! | `a & b` | intersection (both match somewhere in the stack) |
//! | `a - b` | `a` and not `b`; a leading `-b` (e.g. `-(console.error)`) matches when `b` does not |
//! | `a b` | descendant (`b` somewhere inside `a`) |
//! | `a > b` (or `a ‣ b`) | direct child |
//! | `(...)` | grouping |
//! | `string` | prefix match: `string.quoted.double.c` is matched, `strings` is not |
//! | `*` / `**` | one segment / any number of segments, e.g. `console.*.red`, `**.debugger` |
//! | `string$` | matches only as the innermost scope of the stack |
//!
//! A leading `L:`, `R:` or `B:` (injection side markers) on a name is ignored.
//! `-` is an operator only at the start of a token, so scope names such as
//! `meta.toc-list` and `source.c++` are single names.
//!
//! [`ScopeSelector::score`] returns how well the selector matches a scope
//! stack; themes apply the matching items in ascending score order so the
//! most specific item wins (deeper scope first, then more matched segments,
//! then later position in the theme).

use std::fmt;

/// Why a selector failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid scope selector `{source_text}`: {reason}")]
pub struct SelectorError {
    /// The selector text.
    pub source_text: String,
    /// What was wrong.
    pub reason: String,
}

/// How strongly a selector matches a scope stack; higher is more specific.
/// Compare with `<`/`>`; the derived order is lexicographic: the stack depth
/// of the last matched scope, then earlier atoms' depths, then the number of
/// matched segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Score {
    depth: [u16; 4],
    specificity: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Star,
    DoubleStar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Name {
    segs: Vec<Seg>,
    /// `string$`: only as the innermost scope.
    end_anchor: bool,
    /// Literal segments (specificity).
    literals: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Atom {
    Name(Name),
    Group(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    /// `>` before it: it must be the direct child of the previous step.
    direct: bool,
    atom: Atom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    /// The empty left side of a leading `-`.
    Any,
    Path(Vec<Step>),
    Union(Vec<Expr>),
    Inter(Vec<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Excl(Box<Expr>, Box<Expr>),
}

/// A parsed scope selector.
#[derive(Clone, PartialEq, Eq)]
pub struct ScopeSelector {
    source: String,
    root: Expr,
}

impl fmt::Debug for ScopeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScopeSelector({:?})", self.source)
    }
}

impl fmt::Display for ScopeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

// ---- tokens -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Name(String),
    LParen,
    RParen,
    Comma,
    Pipe,
    Amp,
    Eq,
    Gt,
    Minus,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        let simple = match c {
            c if c.is_whitespace() => {
                chars.next();
                continue;
            }
            '(' => Some(Tok::LParen),
            ')' => Some(Tok::RParen),
            ',' => Some(Tok::Comma),
            '|' => Some(Tok::Pipe),
            '&' => Some(Tok::Amp),
            '=' => Some(Tok::Eq),
            '>' | '‣' => Some(Tok::Gt),
            '-' => Some(Tok::Minus),
            _ => None,
        };
        if let Some(tok) = simple {
            chars.next();
            toks.push(tok);
            continue;
        }
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || "()|,&=>‣".contains(c) {
                break;
            }
            name.push(c);
            chars.next();
        }
        toks.push(Tok::Name(name));
    }
    toks
}

fn parse_name(raw: &str) -> Name {
    let mut raw = raw;
    if raw.len() > 2 && matches!(&raw[..2], "L:" | "R:" | "B:") {
        raw = &raw[2..];
    }
    let end_anchor = raw.len() > 1 && raw.ends_with('$');
    let raw = raw.strip_suffix('$').filter(|_| end_anchor).unwrap_or(raw);
    let segs: Vec<Seg> = raw
        .split('.')
        .map(|s| match s {
            "*" => Seg::Star,
            "**" => Seg::DoubleStar,
            s => Seg::Lit(s.to_owned()),
        })
        .collect();
    let literals = segs.iter().filter(|s| matches!(s, Seg::Lit(_))).count() as u16;
    Name {
        segs,
        end_anchor,
        literals,
    }
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let left = self.union()?;
        if self.eat(&Tok::Eq) {
            let right = self.union()?;
            return Ok(Expr::Excl(Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn union(&mut self) -> Result<Expr, String> {
        let mut parts = vec![self.inter()?];
        while matches!(self.peek(), Some(Tok::Comma | Tok::Pipe)) {
            self.pos += 1;
            // A trailing separator is tolerated (`a, b,`).
            if matches!(self.peek(), None | Some(Tok::RParen)) {
                break;
            }
            parts.push(self.inter()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::Union(parts)
        })
    }

    fn inter(&mut self) -> Result<Expr, String> {
        let mut parts = vec![self.sub()?];
        while self.eat(&Tok::Amp) {
            parts.push(self.sub()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::Inter(parts)
        })
    }

    fn sub(&mut self) -> Result<Expr, String> {
        let mut left = if self.peek() == Some(&Tok::Minus) {
            Expr::Any
        } else {
            self.path()?
        };
        while self.eat(&Tok::Minus) {
            let right = self.path()?;
            left = Expr::Sub(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn path(&mut self) -> Result<Expr, String> {
        let mut steps = Vec::new();
        loop {
            let direct = self.eat(&Tok::Gt);
            match self.peek() {
                Some(Tok::Name(_)) => {
                    if let Some(Tok::Name(n)) = self.bump() {
                        steps.push(Step {
                            direct,
                            atom: Atom::Name(parse_name(&n)),
                        });
                    }
                }
                Some(Tok::LParen) => {
                    self.pos += 1;
                    let inner = self.expr()?;
                    if !self.eat(&Tok::RParen) {
                        return Err("missing `)`".to_owned());
                    }
                    steps.push(Step {
                        direct,
                        atom: Atom::Group(Box::new(inner)),
                    });
                }
                _ => break,
            }
        }
        if steps.is_empty() {
            return Err(match self.peek() {
                Some(t) => format!("unexpected {t:?}"),
                None => "unexpected end".to_owned(),
            });
        }
        Ok(Expr::Path(steps))
    }
}

// ---- matching -----------------------------------------------------------

/// A scope stack with every scope split into segments once, so many
/// selectors can be scored against it cheaply ([`ScopeSelector::score_in`]).
pub struct StackView<'a> {
    scopes: Vec<Vec<&'a str>>,
}

impl<'a> StackView<'a> {
    /// Splits `stack` (outermost scope first) into segments.
    pub fn new<S: AsRef<str>>(stack: &'a [S]) -> StackView<'a> {
        StackView {
            scopes: stack.iter().map(|s| s.as_ref().split('.').collect()).collect(),
        }
    }
}

fn match_segs(pattern: &[Seg], scope: &[&str]) -> bool {
    match pattern.first() {
        None => true,
        Some(Seg::Lit(s)) => scope.first() == Some(&s.as_str()) && match_segs(&pattern[1..], &scope[1..]),
        Some(Seg::Star) => !scope.is_empty() && match_segs(&pattern[1..], &scope[1..]),
        Some(Seg::DoubleStar) => (0..=scope.len()).any(|n| match_segs(&pattern[1..], &scope[n..])),
    }
}

fn max_score(a: Option<Score>, b: Option<Score>) -> Option<Score> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl StackView<'_> {
    fn len(&self) -> usize {
        self.scopes.len()
    }

    fn name_at(&self, name: &Name, i: usize) -> Option<u16> {
        if name.end_anchor && i + 1 != self.len() {
            return None;
        }
        match_segs(&name.segs, &self.scopes[i]).then_some(name.literals)
    }

    fn atom_at(&self, atom: &Atom, i: usize) -> Option<(u16, Score)> {
        match atom {
            Atom::Name(n) => self.name_at(n, i).map(|spec| (spec, Score::default())),
            Atom::Group(e) => self.eval_at(e, i).map(|s| (s.specificity, s)),
        }
    }

    /// Best score of `expr` matching with its last scope exactly at `i`.
    fn eval_at(&self, expr: &Expr, i: usize) -> Option<Score> {
        match expr {
            Expr::Any => Some(Score::default()),
            Expr::Path(steps) => {
                let last = steps.len() - 1;
                let mut score = Score::default();
                let mut pos = i;
                for k in (0..=last).rev() {
                    let step = &steps[k];
                    let found = if k == last {
                        self.atom_at(&step.atom, i).map(|m| (i, m))
                    } else if steps[k + 1].direct {
                        pos.checked_sub(1)
                            .and_then(|j| self.atom_at(&step.atom, j).map(|m| (j, m)))
                    } else {
                        (0..pos).rev().find_map(|j| self.atom_at(&step.atom, j).map(|m| (j, m)))
                    };
                    let (j, (spec, _)) = found?;
                    let slot = last - k;
                    if slot < score.depth.len() {
                        score.depth[slot] = j as u16 + 1;
                    }
                    score.specificity = score.specificity.saturating_add(spec);
                    pos = j;
                }
                Some(score)
            }
            Expr::Union(parts) => parts.iter().fold(None, |acc, p| max_score(acc, self.eval_at(p, i))),
            Expr::Inter(parts) => {
                let mut best: Option<Score> = None;
                let mut at_i = false;
                for p in parts {
                    let any = self.eval_any(p)?;
                    let here = self.eval_at(p, i);
                    at_i |= here.is_some();
                    best = max_score(best, Some(here.unwrap_or(any)));
                }
                at_i.then_some(best).flatten()
            }
            Expr::Sub(left, right) => {
                let l = self.eval_at(left, i)?;
                self.eval_any(right).is_none().then_some(l)
            }
            Expr::Excl(left, right) => {
                let deepest = (0..self.len()).rev().find(|&j| self.eval_at(left, j).is_some())?;
                if deepest != i {
                    return None;
                }
                let l = self.eval_at(left, i)?;
                let r = self.eval_at(right, i)?;
                Some(l.max(r))
            }
        }
    }

    /// Best score of `expr` matching anywhere in the stack.
    fn eval_any(&self, expr: &Expr) -> Option<Score> {
        if self.len() == 0 {
            return self.eval_empty(expr);
        }
        (0..self.len()).fold(None, |acc, i| max_score(acc, self.eval_at(expr, i)))
    }

    /// Purely negative selectors match an empty stack.
    fn eval_empty(&self, expr: &Expr) -> Option<Score> {
        match expr {
            Expr::Any => Some(Score::default()),
            Expr::Sub(l, r) => {
                let ls = self.eval_empty(l)?;
                self.eval_empty(r).is_none().then_some(ls)
            }
            Expr::Union(parts) => parts.iter().fold(None, |a, p| max_score(a, self.eval_empty(p))),
            Expr::Inter(parts) => {
                let mut best = None;
                for p in parts {
                    best = max_score(best, Some(self.eval_empty(p)?));
                }
                best
            }
            _ => None,
        }
    }
}

impl ScopeSelector {
    /// Parses `source`.
    ///
    /// # Errors
    /// [`SelectorError`] for unbalanced parentheses, a dangling operator or
    /// an empty selector.
    pub fn parse(source: &str) -> Result<ScopeSelector, SelectorError> {
        let fail = |reason: String| SelectorError {
            source_text: source.to_owned(),
            reason,
        };
        let toks = tokenize(source);
        if toks.is_empty() {
            return Err(fail("empty selector".to_owned()));
        }
        let mut parser = Parser { toks, pos: 0 };
        let root = parser.expr().map_err(fail)?;
        if parser.pos < parser.toks.len() {
            return Err(fail(format!("unexpected {:?}", parser.toks[parser.pos])));
        }
        Ok(ScopeSelector {
            source: source.to_owned(),
            root,
        })
    }

    /// The text the selector was parsed from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The match score against `stack` (outermost scope first, as
    /// `source.rust meta.function.rust ...`), or `None` when it does not
    /// match.
    pub fn score<S: AsRef<str>>(&self, stack: &[S]) -> Option<Score> {
        self.score_in(&StackView::new(stack))
    }

    /// [`ScopeSelector::score`] against a pre-split stack (use when scoring
    /// many selectors against one stack).
    pub fn score_in(&self, stack: &StackView<'_>) -> Option<Score> {
        stack.eval_any(&self.root)
    }

    /// Whether the selector matches `stack` at all.
    pub fn matches<S: AsRef<str>>(&self, stack: &[S]) -> bool {
        self.score(stack).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(selector: &str, stack: &str) -> bool {
        let stack: Vec<&str> = stack.split_whitespace().collect();
        ScopeSelector::parse(selector)
            .unwrap_or_else(|e| panic!("{e}"))
            .matches(&stack)
    }

    #[test]
    fn prefix_match_and_descendant() {
        assert!(m("string", "source.c string.quoted.double.c"));
        assert!(!m("strings", "source.c string.quoted.double.c"));
        assert!(m("string.quoted", "source.c string.quoted.double.c"));
        assert!(!m("string.double", "source.c string.quoted.double.c"));
        assert!(m("source string", "source.c string.quoted.double.c"));
        assert!(!m("string source", "source.c string.quoted.double.c"));
    }

    #[test]
    fn subtraction_and_negation() {
        let kw = "keyword - keyword.operator - constant.numeric";
        assert!(m(kw, "source.c keyword.control.c"));
        assert!(!m(kw, "source.c keyword.operator.c"));
        assert!(m("-(console.error, console.warning)", "console.stdout"));
        assert!(!m("-(console.error, console.warning)", "console.error"));
        assert!(m("-comment", ""));
    }

    #[test]
    fn union_group_and_intersection() {
        assert!(m("(string | punctuation.definition.string) - comment", "a string.quoted"));
        assert!(!m("(string | punctuation.definition.string) - comment", "comment.line string.quoted"));
        assert!(m("comment, string", "x string.quoted"));
        assert!(m("string & source.js", "source.js string.quoted"));
        assert!(!m("string & source.js", "source.c string.quoted"));
    }

    #[test]
    fn child_wildcards_and_anchor() {
        assert!(m("meta.tag > punctuation", "text.html meta.tag punctuation.definition.tag"));
        assert!(!m("meta.tag > punctuation", "text.html meta.tag meta.x punctuation.definition.tag"));
        assert!(m("meta.tag ‣ punctuation", "meta.tag punctuation.definition.tag"));
        assert!(m("console.*.red", "console.ansi.red"));
        assert!(m("**.debugger", "console.debugger"));
        assert!(m("**.debugger", "debugger"));
        assert!(m("string$", "source.c string.quoted"));
        assert!(!m("string$", "source.c string.quoted meta.embedded"));
    }

    #[test]
    fn exclusive_intersection() {
        let sel = "source|text|string|comment = source";
        assert!(m(sel, "source.c meta.function"));
        assert!(!m(sel, "source.c string.quoted"));
        assert!(!m(sel, "source.c comment.block"));
    }

    #[test]
    fn names_with_dash_and_plus() {
        assert!(m("meta.toc-list", "meta.toc-list.pragma-mark.c"));
        assert!(m("source.c++", "source.c++ meta.x"));
        assert!(m("L:source.regexp - constant", "source.regexp"));
    }

    #[test]
    fn specificity_orders_scores() {
        let stack: Vec<&str> = "source.rust meta.fn string.quoted.double".split(' ').collect();
        let deep = ScopeSelector::parse("string.quoted").unwrap().score(&stack).unwrap();
        let shallow = ScopeSelector::parse("string").unwrap().score(&stack).unwrap();
        let outer = ScopeSelector::parse("source").unwrap().score(&stack).unwrap();
        assert!(deep > shallow);
        assert!(shallow > outer);
    }

    #[test]
    fn rejects_garbage() {
        assert!(ScopeSelector::parse("").is_err());
        assert!(ScopeSelector::parse("(a b").is_err());
        assert!(ScopeSelector::parse("a b)").is_err());
        assert!(ScopeSelector::parse("a ,").is_ok());
        assert!(ScopeSelector::parse("a = ").is_err());
    }
}
