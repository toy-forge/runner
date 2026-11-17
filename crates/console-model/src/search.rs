//! Find in console: matches over the visible lines.

use std::ops::Range;

use crate::console::Console;
use crate::line::LineId;
use crate::query::Matcher;
use crate::selection::Pos;

/// One match: a byte range in one line.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Match {
    /// The line.
    pub line: LineId,
    /// Byte range in the line text.
    pub range: Range<usize>,
}

impl Match {
    /// The start as a position.
    pub fn start(&self) -> Pos {
        Pos::new(self.line, self.range.start)
    }
}

impl Console {
    /// Matches in visible line `index` (for highlighting rows).
    pub fn matches_in_visible_line(&self, matcher: &Matcher, index: usize) -> Vec<Range<usize>> {
        self.visible_line(index).map(|v| matcher.find_all(v.text())).unwrap_or_default()
    }

    /// Up to `limit` matches in document order.
    pub fn find_all(&self, matcher: &Matcher, limit: usize) -> Vec<Match> {
        let mut out = Vec::new();
        for i in 0..self.visible_count() {
            let Some(v) = self.visible_line(i) else { break };
            for r in matcher.find_all(v.text()) {
                out.push(Match { line: v.id(), range: r });
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    /// Counts matches, stopping at `limit`.
    pub fn count_matches(&self, matcher: &Matcher, limit: usize) -> usize {
        self.find_all(matcher, limit).len()
    }

    /// The first match starting after `after` (`None`: from the top). With
    /// `wrap`, continues from the top when nothing follows.
    pub fn find_next(&self, matcher: &Matcher, after: Option<Pos>, wrap: bool) -> Option<Match> {
        let n = self.visible_count();
        let start_idx = after.map_or(0, |p| self.visible_lower_bound(p.line));
        for i in start_idx..n {
            let v = self.visible_line(i)?;
            for r in matcher.find_all(v.text()) {
                let pos = Pos::new(v.id(), r.start);
                if after.is_none_or(|a| pos > a) {
                    return Some(Match { line: v.id(), range: r });
                }
            }
        }
        if wrap {
            let limit = after.map_or(0, |p| self.visible_lower_bound(p.line) + 1).min(n);
            for i in 0..limit {
                let v = self.visible_line(i)?;
                if let Some(r) = matcher.find(v.text()) {
                    return Some(Match { line: v.id(), range: r });
                }
            }
        }
        None
    }

    /// The last match starting before `before` (`None`: from the bottom).
    pub fn find_prev(&self, matcher: &Matcher, before: Option<Pos>, wrap: bool) -> Option<Match> {
        let n = self.visible_count();
        if n == 0 {
            return None;
        }
        let from_idx = before.map_or(n - 1, |p| self.visible_lower_bound(p.line).min(n - 1));
        for i in (0..=from_idx).rev() {
            let v = self.visible_line(i)?;
            for r in matcher.find_all(v.text()).into_iter().rev() {
                let pos = Pos::new(v.id(), r.start);
                if before.is_none_or(|b| pos < b) {
                    return Some(Match { line: v.id(), range: r });
                }
            }
        }
        if wrap {
            let lo = before.map_or(n, |p| self.visible_lower_bound(p.line)).min(n - 1);
            for i in (lo..n).rev() {
                let v = self.visible_line(i)?;
                if let Some(r) = matcher.find_all(v.text()).into_iter().next_back() {
                    return Some(Match { line: v.id(), range: r });
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{Filter, OutputFilter};
    use crate::query::SearchQuery;
    use crate::tag::Stream;

    fn console() -> Console {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "foo bar foo\nnothing\nFOO\n");
        c
    }

    fn m(q: &str) -> Matcher {
        Matcher::new(&SearchQuery::plain(q)).unwrap()
    }

    #[test]
    fn find_all_and_count() {
        let c = console();
        let all = c.find_all(&m("foo"), 100);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].range, 0..3);
        assert_eq!(all[1].range, 8..11);
        assert_eq!(all[2].line, LineId(2));
        assert_eq!(c.count_matches(&m("foo"), 2), 2);
    }

    #[test]
    fn next_and_prev_walk_every_match_once() {
        let c = console();
        let q = m("foo");
        let mut seen = Vec::new();
        let mut cur = c.find_next(&q, None, false);
        while let Some(x) = cur {
            seen.push((x.line.0, x.range.start));
            cur = c.find_next(&q, Some(x.start()), false);
        }
        assert_eq!(seen, vec![(0, 0), (0, 8), (2, 0)]);
        let mut back = Vec::new();
        let mut cur = c.find_prev(&q, None, false);
        while let Some(x) = cur {
            back.push((x.line.0, x.range.start));
            cur = c.find_prev(&q, Some(x.start()), false);
        }
        back.reverse();
        assert_eq!(back, seen);
    }

    #[test]
    fn wrap_around() {
        let c = console();
        let q = m("foo");
        let last = Pos::new(LineId(2), 0);
        assert!(c.find_next(&q, Some(last), false).is_none());
        assert_eq!(c.find_next(&q, Some(last), true).unwrap().start(), Pos::new(LineId(0), 0));
        let first = Pos::new(LineId(0), 0);
        assert!(c.find_prev(&q, Some(first), false).is_none());
        assert_eq!(c.find_prev(&q, Some(first), true).unwrap().start(), Pos::new(LineId(2), 0));
    }

    #[test]
    fn searches_only_visible_lines() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "needle out\n");
        c.append_str(Stream::Stderr, "needle err\n");
        c.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
        let all = c.find_all(&m("needle"), 10);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].line, LineId(1));
        assert_eq!(c.matches_in_visible_line(&m("needle"), 0), vec![0..6]);
    }

    #[test]
    fn empty_console() {
        let c = Console::default();
        assert!(c.find_next(&m("x"), None, true).is_none());
        assert!(c.find_prev(&m("x"), None, true).is_none());
    }
}
