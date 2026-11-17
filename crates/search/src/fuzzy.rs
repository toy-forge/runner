//! Fuzzy subsequence matching with scoring, shared by QuickNav, popup
//! type-to-search and the command palette.
//!
//! * The query is split at whitespace into **terms**; every term must match
//!   (AND), as in fzf. Each term is a case-insensitive subsequence of the
//!   candidate.
//! * Scoring is a small Smith-Waterman: matches at the start of the text,
//!   after a separator (`/ \ _ - . space : ...`) and at camelCase / digit
//!   transitions score extra, consecutive matches score extra, gaps cost.
//! * Positions are **byte offsets** of the matched chars in the candidate
//!   (ready for highlighting); [`FuzzyMatch::ranges`] merges adjacent ones.
//! * Ordering is stable: higher score first, then shorter candidate, then the
//!   original position.
//!
//! ```
//! use scratch_search::fuzzy::{Fuzzy, rank};
//!
//! let m = Fuzzy::new("fb").score("FooBar").unwrap();
//! assert_eq!(m.positions, vec![0, 3]);
//!
//! let items = ["bar_foo.rs", "foo_bar.rs", "unrelated"];
//! let ranked = rank("fb", items.iter().map(|s| (*s, *s)), 10);
//! assert_eq!(ranked[0].item, "foo_bar.rs");
//! ```

use std::ops::Range;

use crate::cancel::CancelToken;

const SCORE_MATCH: i32 = 16;
const GAP_START: i32 = 3;
const GAP_EXT: i32 = 1;
const BONUS_CONSECUTIVE: i32 = 4;
const BONUS_BOUNDARY: i32 = 8;
const BONUS_PATH: i32 = 9;
const BONUS_START: i32 = 10;
const BONUS_CAMEL: i32 = 7;
const BONUS_EXACT_CASE: i32 = 1;
const NEG: i32 = i32::MIN / 4;
const MAX_CELLS: usize = 400_000;

/// How the case of query chars is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMatching {
    /// Ignore case; an exact-case match scores slightly higher.
    #[default]
    Ignore,
    /// Ignore case unless the term contains an upper-case letter.
    Smart,
    /// Always respect case.
    Respect,
}

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better. Only comparable between matches of one query.
    pub score: i32,
    /// Byte offsets (ascending, unique) of the matched chars.
    pub positions: Vec<usize>,
}

impl FuzzyMatch {
    /// The matched chars of `text` as merged byte ranges (for highlighting).
    /// `text` must be the candidate that produced the match.
    pub fn ranges(&self, text: &str) -> Vec<Range<usize>> {
        positions_to_ranges(text, &self.positions)
    }
}

/// Merges char start offsets into byte ranges (adjacent chars are joined).
pub fn positions_to_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for &p in positions {
        let Some(c) = text.get(p..).and_then(|t| t.chars().next()) else {
            continue;
        };
        let end = p + c.len_utf8();
        match out.last_mut() {
            Some(last) if last.end == p => last.end = end,
            _ => out.push(p..end),
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sep,
    PathSep,
    Lower,
    Upper,
    Digit,
    Other, // letters without case (CJK...) and symbols
}

fn kind(c: char) -> Kind {
    if c == '/' || c == '\\' {
        Kind::PathSep
    } else if c.is_whitespace()
        || matches!(
            c,
            '_' | '-'
                | '.'
                | ':'
                | ','
                | ';'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | '"'
                | '\''
                | '@'
                | '#'
        )
    {
        Kind::Sep
    } else if c.is_lowercase() {
        Kind::Lower
    } else if c.is_uppercase() {
        Kind::Upper
    } else if c.is_numeric() {
        Kind::Digit
    } else {
        Kind::Other
    }
}

fn bonus(prev: Kind, cur: Kind) -> i32 {
    let word = |k: Kind| !matches!(k, Kind::Sep | Kind::PathSep);
    if !word(cur) {
        return 0;
    }
    match prev {
        Kind::PathSep => BONUS_PATH,
        Kind::Sep => BONUS_BOUNDARY,
        Kind::Lower if cur == Kind::Upper => BONUS_CAMEL,
        Kind::Lower | Kind::Upper if cur == Kind::Digit => BONUS_CAMEL - 2,
        _ => 0,
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[derive(Debug, Clone)]
struct Term {
    raw: Vec<char>,
    lower: Vec<char>,
    respect_case: bool,
}

/// A compiled query. Reuse it across candidates (it keeps scratch buffers).
#[derive(Debug, Clone)]
pub struct Fuzzy {
    terms: Vec<Term>,
    // scratch
    chars: Vec<(usize, char, char)>,
    bonus: Vec<i32>,
    d: Vec<i32>,
    m: Vec<i32>,
    m_from: Vec<u32>,
    consec: Vec<bool>,
    chunk: Vec<i32>,
}

impl Fuzzy {
    /// Compiles `query` with [`CaseMatching::Ignore`].
    pub fn new(query: &str) -> Self {
        Self::with_case(query, CaseMatching::Ignore)
    }

    /// Compiles `query` with the given case handling.
    pub fn with_case(query: &str, case: CaseMatching) -> Self {
        let terms = query
            .split_whitespace()
            .map(|t| {
                let raw: Vec<char> = t.chars().collect();
                let respect_case = match case {
                    CaseMatching::Respect => true,
                    CaseMatching::Smart => raw.iter().any(|c| c.is_uppercase()),
                    CaseMatching::Ignore => false,
                };
                Term {
                    lower: raw.iter().map(|&c| lower(c)).collect(),
                    raw,
                    respect_case,
                }
            })
            .collect();
        Self {
            terms,
            chars: Vec::new(),
            bonus: Vec::new(),
            d: Vec::new(),
            m: Vec::new(),
            m_from: Vec::new(),
            consec: Vec::new(),
            chunk: Vec::new(),
        }
    }

    /// True for a blank query (matches everything with score 0).
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Scores `candidate`; `None` when some term is not a subsequence of it.
    pub fn score(&mut self, candidate: &str) -> Option<FuzzyMatch> {
        if self.terms.is_empty() {
            return Some(FuzzyMatch {
                score: 0,
                positions: Vec::new(),
            });
        }
        self.chars.clear();
        self.bonus.clear();
        let mut prev = Kind::Sep;
        for (i, c) in candidate.char_indices() {
            let k = kind(c);
            self.chars.push((i, c, lower(c)));
            self.bonus.push(
                if self.chars.len() == 1 && k != Kind::Sep && k != Kind::PathSep {
                    BONUS_START
                } else {
                    bonus(prev, k)
                },
            );
            prev = k;
        }
        let mut total = 0;
        let mut positions: Vec<usize> = Vec::new();
        for ti in 0..self.terms.len() {
            let (score, mut pos) = self.match_term(ti)?;
            total += score;
            positions.append(&mut pos);
        }
        positions.sort_unstable();
        positions.dedup();
        Some(FuzzyMatch {
            score: total,
            positions,
        })
    }

    /// Cheap check without scoring.
    pub fn is_match(&self, candidate: &str) -> bool {
        self.terms.iter().all(|t| {
            let mut it = candidate.chars().map(|c| (c, lower(c)));
            t.raw
                .iter()
                .zip(&t.lower)
                .all(|(&qr, &ql)| it.any(|(c, cl)| if t.respect_case { c == qr } else { cl == ql }))
        })
    }

    fn eq(term: &Term, qi: usize, c: (usize, char, char)) -> bool {
        if term.respect_case {
            c.1 == term.raw[qi]
        } else {
            c.2 == term.lower[qi]
        }
    }

    /// Returns (score, byte positions) of one term.
    fn match_term(&mut self, ti: usize) -> Option<(i32, Vec<usize>)> {
        let term = self.terms[ti].clone();
        let m = term.raw.len();
        let n = self.chars.len();
        if m == 0 || m > n {
            return if m == 0 { Some((0, Vec::new())) } else { None };
        }
        // Window: first occurrence of q[0] .. last occurrence of q[m-1], with a
        // forward greedy subsequence check.
        let lo = (0..n).find(|&j| Self::eq(&term, 0, self.chars[j]))?;
        let mut qi = 0;
        for j in lo..n {
            if qi < m && Self::eq(&term, qi, self.chars[j]) {
                qi += 1;
            }
        }
        if qi < m {
            return None;
        }
        let hi = (lo..n)
            .rev()
            .find(|&j| Self::eq(&term, m - 1, self.chars[j]))?;
        let w = hi - lo + 1;
        if m > w {
            return None;
        }
        if m * w > MAX_CELLS {
            // Greedy fallback for pathological sizes.
            let mut qi = 0;
            let mut pos = Vec::with_capacity(m);
            for j in lo..=hi {
                if qi < m && Self::eq(&term, qi, self.chars[j]) {
                    pos.push(self.chars[j].0);
                    qi += 1;
                }
            }
            return (qi == m).then_some((m as i32 * SCORE_MATCH, pos));
        }
        let cells = m * w;
        self.d.clear();
        self.d.resize(cells, NEG);
        self.m.clear();
        self.m.resize(cells, NEG);
        self.m_from.clear();
        self.m_from.resize(cells, 0);
        self.consec.clear();
        self.consec.resize(cells, false);
        self.chunk.clear();
        self.chunk.resize(cells, 0);
        for i in 0..m {
            for jj in 0..w {
                let j = lo + jj;
                let idx = i * w + jj;
                let mut dv = NEG;
                if Self::eq(&term, i, self.chars[j]) {
                    let b = self.bonus[j];
                    let exact = if self.chars[j].1 == term.raw[i] {
                        BONUS_EXACT_CASE
                    } else {
                        0
                    };
                    if i == 0 {
                        dv = SCORE_MATCH + 2 * b + exact - (j as i32).min(6);
                        self.chunk[idx] = b;
                    } else {
                        let mut best = NEG;
                        let mut cons = false;
                        let mut cb = b;
                        if jj > 0 {
                            let p = self.d[(i - 1) * w + jj - 1];
                            if p > NEG {
                                // A chunk of consecutive matches keeps the bonus of its first char.
                                let cbp = self.chunk[(i - 1) * w + jj - 1];
                                let c = if b >= BONUS_BOUNDARY && b > cbp {
                                    b
                                } else {
                                    cbp.max(BONUS_CONSECUTIVE)
                                };
                                best = p + SCORE_MATCH + exact + c.max(b);
                                cb = c;
                                cons = true;
                            }
                        }
                        if jj >= 2 {
                            let p = self.m[(i - 1) * w + jj - 2];
                            let v = p - GAP_START + SCORE_MATCH + exact + b;
                            if p > NEG && v > best {
                                best = v;
                                cb = b;
                                cons = false;
                            }
                        }
                        dv = best;
                        self.consec[idx] = cons;
                        self.chunk[idx] = cb;
                    }
                }
                self.d[idx] = dv;
                let (mut mv, mut from) = (dv, jj as u32);
                if jj > 0 {
                    let left = self.m[idx - 1];
                    if left > NEG && left - GAP_EXT > mv {
                        mv = left - GAP_EXT;
                        from = self.m_from[idx - 1];
                    }
                }
                self.m[idx] = mv;
                self.m_from[idx] = from;
            }
        }
        // Best end.
        let last = (m - 1) * w;
        let (mut best, mut best_jj) = (NEG, usize::MAX);
        for jj in 0..w {
            let v = self.d[last + jj];
            if v > NEG && v >= best {
                best = v;
                best_jj = jj;
            }
        }
        if best_jj == usize::MAX {
            return None;
        }
        // Backtrack.
        let mut pos = vec![0usize; m];
        let mut jj = best_jj;
        for i in (0..m).rev() {
            pos[i] = self.chars[lo + jj].0;
            if i == 0 {
                break;
            }
            let idx = i * w + jj;
            jj = if self.consec[idx] {
                jj - 1
            } else {
                self.m_from[(i - 1) * w + jj - 2] as usize
            };
        }
        Some((best, pos))
    }
}

/// One-shot convenience for [`Fuzzy::new`] + [`Fuzzy::score`].
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    Fuzzy::new(query).score(candidate)
}

/// A ranked item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked<T> {
    /// The caller's item.
    pub item: T,
    /// The match (score and positions in the scored text).
    pub matched: FuzzyMatch,
    /// Index of the item in the input (the tie breaker).
    pub index: usize,
}

/// Ranks `(item, text)` pairs against `query`, best first, at most `limit`.
/// A blank query keeps the input order.
pub fn rank<'a, T>(
    query: &str,
    items: impl IntoIterator<Item = (T, &'a str)>,
    limit: usize,
) -> Vec<Ranked<T>> {
    rank_cancellable(query, items, limit, &CancelToken::new()).unwrap_or_default()
}

/// Like [`rank`], checking `cancel` every 256 items; `None` when cancelled.
pub fn rank_cancellable<'a, T>(
    query: &str,
    items: impl IntoIterator<Item = (T, &'a str)>,
    limit: usize,
    cancel: &CancelToken,
) -> Option<Vec<Ranked<T>>> {
    let mut fuzzy = Fuzzy::new(query);
    let mut out: Vec<(Ranked<T>, usize)> = Vec::new();
    for (index, (item, text)) in items.into_iter().enumerate() {
        if index % 256 == 0 && cancel.is_cancelled() {
            return None;
        }
        if let Some(matched) = fuzzy.score(text) {
            let len = if fuzzy.is_empty() {
                0
            } else {
                text.chars().count()
            };
            out.push((
                Ranked {
                    item,
                    matched,
                    index,
                },
                len,
            ));
        }
    }
    let cmp = |a: &(Ranked<T>, usize), b: &(Ranked<T>, usize)| {
        b.0.matched
            .score
            .cmp(&a.0.matched.score)
            .then(a.1.cmp(&b.1))
            .then(a.0.index.cmp(&b.0.index))
    };
    if limit < out.len() {
        out.select_nth_unstable_by(limit, cmp);
        out.truncate(limit);
    }
    out.sort_by(cmp);
    Some(out.into_iter().map(|(r, _)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(q: &str, c: &str) -> Option<Vec<usize>> {
        fuzzy_match(q, c).map(|m| m.positions)
    }

    #[test]
    fn subsequence_and_positions() {
        assert_eq!(pos("fb", "FooBar"), Some(vec![0, 3]));
        assert_eq!(pos("abc", "a_b_c"), Some(vec![0, 2, 4]));
        assert_eq!(pos("xyz", "abc"), None);
        assert_eq!(pos("ab", "ba"), None);
        assert_eq!(pos("", "anything"), Some(vec![]));
        assert_eq!(pos("longer", "long"), None);
    }

    #[test]
    fn boundaries_beat_scattered() {
        let a = fuzzy_match("fb", "foo_bar").unwrap();
        let b = fuzzy_match("fb", "fabulous").unwrap();
        assert!(a.score > b.score, "{a:?} {b:?}");
        let camel = fuzzy_match("gc", "getCount").unwrap();
        let flat = fuzzy_match("gc", "agcb").unwrap();
        assert!(camel.score > flat.score);
    }

    #[test]
    fn prefers_consecutive_and_path_boundaries() {
        let a = fuzzy_match("main", "src/main.rs").unwrap();
        let b = fuzzy_match("main", "src/m_a_i_n.rs").unwrap();
        assert!(a.score > b.score);
        assert_eq!(a.positions, vec![4, 5, 6, 7]);
        // Prefers the basename match over a directory scatter.
        let m = fuzzy_match("lib", "lib/x/lib.rs").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
    }

    #[test]
    fn multiple_terms_are_anded() {
        let m = fuzzy_match("set py", "Set Language: Python").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2, 14, 15]);
        assert!(fuzzy_match("set rb", "Set Language: Python").is_none());
    }

    #[test]
    fn case_modes() {
        assert!(Fuzzy::new("abc").score("ABC").is_some());
        assert!(
            Fuzzy::with_case("Abc", CaseMatching::Smart)
                .score("abc")
                .is_none()
        );
        assert!(
            Fuzzy::with_case("abc", CaseMatching::Smart)
                .score("ABC")
                .is_some()
        );
        assert!(
            Fuzzy::with_case("abc", CaseMatching::Respect)
                .score("ABC")
                .is_none()
        );
        let exact = fuzzy_match("abc", "abc").unwrap();
        let other = fuzzy_match("abc", "ABC").unwrap();
        assert!(exact.score > other.score);
    }

    #[test]
    fn unicode_positions_are_byte_offsets() {
        let text = "日本語-テキスト😀x";
        let m = fuzzy_match("テx", text).unwrap();
        assert_eq!(m.positions, vec![10, text.len() - 1]);
        assert_eq!(m.ranges(text), vec![10..13, text.len() - 1..text.len()]);
        let m = fuzzy_match("é", "cafe\u{301}").unwrap_or(FuzzyMatch {
            score: 0,
            positions: vec![],
        });
        assert!(m.positions.is_empty()); // precomposed query does not match decomposed text
        assert_eq!(
            fuzzy_match("ÉCL", "école").unwrap().positions,
            vec![0, 2, 4]
        );
    }

    #[test]
    fn rank_is_stable_and_limited() {
        let items = ["ab", "xab", "ab", "a-b", "zzz"];
        let r = rank("ab", items.iter().enumerate().map(|(i, s)| (i, *s)), 10);
        assert_eq!(
            r.iter().map(|x| x.item).collect::<Vec<_>>(),
            vec![0, 2, 3, 1]
        );
        let r = rank("ab", items.iter().enumerate().map(|(i, s)| (i, *s)), 2);
        assert_eq!(r.iter().map(|x| x.item).collect::<Vec<_>>(), vec![0, 2]);
        // blank query: input order
        let r = rank("", items.iter().enumerate().map(|(i, s)| (i, *s)), 3);
        assert_eq!(r.iter().map(|x| x.item).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn cancellation() {
        let t = CancelToken::new();
        t.cancel();
        let items = (0..1000).map(|i| (i, "abc"));
        assert!(rank_cancellable("a", items, 10, &t).is_none());
    }

    #[test]
    fn long_candidates_do_not_blow_up() {
        let long = "a".repeat(5000) + "b";
        let m = fuzzy_match("ab", &long).unwrap();
        assert_eq!(m.positions.len(), 2);
        let start = std::time::Instant::now();
        let many: Vec<String> = (0..50_000)
            .map(|i| format!("src/module{i}/file_{i}.rs"))
            .collect();
        let r = rank("mod5 f9", many.iter().map(|s| (s.as_str(), s.as_str())), 20);
        assert!(!r.is_empty());
        assert!(start.elapsed().as_secs() < 5);
    }
}
