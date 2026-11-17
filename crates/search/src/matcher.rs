//! The find/replace engine of the find bar: [`Matcher`].
//!
//! A [`Matcher`] is one compiled search (query + mode + ignore case). It
//! works on `&str` with **byte offsets**; convert with
//! [`LineIndex`](crate::LineIndex) for the editor's UTF-16 positions.
//!
//! Regular expressions use `fancy-regex` in its Oniguruma-compatible mode:
//! look-around, backreferences (`\1`, `\k<name>`), atomic groups, named
//! groups, `^`/`$` always at line boundaries, `(?m)` = dot matches newline
//! (Ruby), plus the [`onig`](crate::onig) translations (`\h`, `\Z`, `\R`).
//!
//! CRLF: the editor hands over `\n` text. Files read from disk may still
//! have `\r\n`; use [`Matcher::with_crlf`] for those (`$`, `.` and `\n` then
//! understand `\r\n`, and replacement newlines are written as `\r\n`).

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use fancy_regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::edit::TextEdit;
use crate::onig;
use crate::replacement::Replacement;
use crate::text::{ceil_char_boundary, floor_char_boundary};

/// Why a search or replacement could not be compiled or run.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatchError {
    /// The regular expression is invalid (message is the engine's).
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
    /// The pattern backtracked too much on this text (catastrophic pattern).
    #[error("the regular expression is too complex for this text (backtrack limit exceeded)")]
    BacktrackLimit,
    /// Any other failure while running the regex.
    #[error("search failed: {0}")]
    Runtime(String),
    /// The replacement refers to a named group the pattern does not define.
    #[error("the replacement refers to the unknown group \"{0}\"")]
    UnknownGroup(String),
}

fn map_error(e: fancy_regex::Error) -> MatchError {
    match e {
        fancy_regex::Error::RuntimeError(fancy_regex::RuntimeError::BacktrackLimitExceeded) => {
            MatchError::BacktrackLimit
        }
        fancy_regex::Error::RuntimeError(r) => MatchError::Runtime(r.to_string()),
        other => MatchError::InvalidRegex(other.to_string()),
    }
}

/// The find bar's matching popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MatchMode {
    /// The query as typed.
    #[default]
    Text,
    /// Whole words only.
    ExactWord,
    /// A word starting with the query.
    WordStartsWith,
    /// A word ending with the query.
    WordEndsWith,
    /// Find > Find Mode > "Ignoring Whitespace": whitespace in the text is
    /// insignificant (the query's characters may be separated by any
    /// whitespace, including none).
    IgnoringWhitespace,
    /// The query is an Oniguruma-style regular expression.
    Regex,
}

impl MatchMode {
    /// The five modes of the matching popup, in popup order.
    pub const POPUP: [MatchMode; 5] = [
        MatchMode::Text,
        MatchMode::ExactWord,
        MatchMode::WordStartsWith,
        MatchMode::WordEndsWith,
        MatchMode::Regex,
    ];

    /// The label CodeRunner shows.
    pub fn label(self) -> &'static str {
        match self {
            MatchMode::Text => "Text",
            MatchMode::ExactWord => "Exact Word",
            MatchMode::WordStartsWith => "Word Starts With",
            MatchMode::WordEndsWith => "Word Ends With",
            MatchMode::IgnoringWhitespace => "Ignoring Whitespace",
            MatchMode::Regex => "Regular Expression",
        }
    }

    /// Whether the query is a regular expression (and the replacement a
    /// template).
    pub fn is_regex(self) -> bool {
        self == MatchMode::Regex
    }
}

/// Options of a [`Matcher`] (the find bar's state apart from the texts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatcherOptions {
    /// Matching popup.
    pub mode: MatchMode,
    /// "Ignore Case" (CodeRunner default: on).
    pub ignore_case: bool,
    /// "Wrap Around" (CodeRunner default: on). Used by the `*_wrapping`
    /// helpers and read by UIs; [`Matcher::find_next`] takes it explicitly.
    pub wrap_around: bool,
    /// Backtracking budget for fancy regexes.
    pub backtrack_limit: usize,
}

impl Default for MatcherOptions {
    fn default() -> Self {
        Self {
            mode: MatchMode::Text,
            ignore_case: true,
            wrap_around: true,
            backtrack_limit: 1_000_000,
        }
    }
}

/// A match found by `find_next` / `find_prev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Byte range of the match.
    pub range: Range<usize>,
    /// The search wrapped around the end (or start) of the text.
    pub wrapped: bool,
}

/// Result of [`Matcher::replace_all`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplaceAll {
    /// Sorted, non-overlapping edits of the original text (feed them to
    /// `api.edit` after [`to_position_edits`](crate::to_position_edits), or to
    /// [`apply_edits`](crate::apply_edits)).
    pub edits: Vec<TextEdit>,
    /// Number of replaced matches (`edits.len()`; empty-to-empty edits dropped).
    pub count: usize,
}

/// Result of [`Matcher::replace_selection`] (the "Replace" button).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceStep {
    /// The edit, when the selection was a match.
    pub edit: Option<TextEdit>,
    /// Where to continue searching **forward** in the edited text.
    pub resume_forward: usize,
    /// Where to continue searching **backward** in the edited text.
    pub resume_backward: usize,
}

#[derive(Debug)]
struct Inner {
    pattern: String,
    options: MatcherOptions,
    plain: Option<Regex>,
    crlf_source: Option<String>,
    crlf: OnceLock<Result<Regex, MatchError>>,
}

/// One compiled search. Cheap to clone (shared).
#[derive(Debug, Clone)]
pub struct Matcher {
    inner: Arc<Inner>,
    crlf: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(crate) fn literal_source(pattern: &str, mode: MatchMode, crlf: bool) -> String {
    let pattern = pattern.replace("\r\n", "\n");
    let nl = if crlf { "\\r?\\n" } else { "\\n" };
    let esc = |c: char, out: &mut String| {
        if c == '\n' {
            out.push_str(nl);
        } else {
            let mut buf = [0u8; 4];
            out.push_str(&fancy_regex::escape(c.encode_utf8(&mut buf)));
        }
    };
    let mut body = String::with_capacity(pattern.len() + 8);
    if mode == MatchMode::IgnoringWhitespace {
        let mut first = true;
        for c in pattern.chars().filter(|c| !c.is_whitespace()) {
            if !first {
                body.push_str("\\s*");
            }
            esc(c, &mut body);
            first = false;
        }
        if first {
            body.push_str("\\s+");
        }
        return body;
    }
    for c in pattern.chars() {
        esc(c, &mut body);
    }
    let first_word = pattern.chars().next().is_some_and(is_word);
    let last_word = pattern.chars().next_back().is_some_and(is_word);
    match mode {
        MatchMode::ExactWord => {
            format!(
                "{}{body}{}",
                if first_word { "\\b" } else { "" },
                if last_word { "\\b" } else { "" }
            )
        }
        MatchMode::WordStartsWith => format!("{}{body}", if first_word { "\\b" } else { "" }),
        MatchMode::WordEndsWith => format!("{body}{}", if last_word { "\\b" } else { "" }),
        _ => body,
    }
}

fn build(source: &str, o: &MatcherOptions) -> Result<Regex, MatchError> {
    RegexBuilder::new(source)
        .case_insensitive(o.ignore_case)
        .multi_line(true)
        .oniguruma_mode(true)
        .backtrack_limit(o.backtrack_limit)
        .build()
        .map_err(map_error)
}

impl Matcher {
    /// Compiles `pattern` under `options`.
    ///
    /// An empty pattern is valid and matches nothing ([`Matcher::is_empty`]).
    /// An invalid regular expression is [`MatchError::InvalidRegex`].
    pub fn new(pattern: &str, options: &MatcherOptions) -> Result<Self, MatchError> {
        let (plain_src, crlf_src) = if pattern.is_empty() {
            (None, None)
        } else if options.mode.is_regex() {
            let p = onig::translate(pattern, false);
            let c = onig::translate(pattern, true);
            let c = (c != p).then_some(c);
            (Some(p), c)
        } else {
            let p = literal_source(pattern, options.mode, false);
            let c = literal_source(pattern, options.mode, true);
            let c = (c != p).then_some(c);
            (Some(p), c)
        };
        let plain = plain_src.map(|s| build(&s, options)).transpose()?;
        Ok(Self {
            inner: Arc::new(Inner {
                pattern: pattern.to_string(),
                options: *options,
                plain,
                crlf_source: crlf_src,
                crlf: OnceLock::new(),
            }),
            crlf: false,
        })
    }

    /// Convenience: `Matcher::new(pattern, &MatcherOptions { mode, ignore_case, ..default })`.
    pub fn simple(pattern: &str, mode: MatchMode, ignore_case: bool) -> Result<Self, MatchError> {
        Self::new(
            pattern,
            &MatcherOptions {
                mode,
                ignore_case,
                ..MatcherOptions::default()
            },
        )
    }

    /// The same search for text whose line ends are `\r\n` (or not). Cheap.
    pub fn with_crlf(&self, crlf: bool) -> Matcher {
        Matcher {
            inner: self.inner.clone(),
            crlf,
        }
    }

    /// Whether this matcher is in CRLF mode.
    pub fn is_crlf(&self) -> bool {
        self.crlf
    }

    /// The pattern as typed.
    pub fn pattern(&self) -> &str {
        &self.inner.pattern
    }

    /// The options it was built with.
    pub fn options(&self) -> &MatcherOptions {
        &self.inner.options
    }

    /// An empty pattern matches nothing.
    pub fn is_empty(&self) -> bool {
        self.inner.plain.is_none()
    }

    /// Number of capture groups (not counting group 0).
    pub fn group_count(&self) -> usize {
        self.inner
            .plain
            .as_ref()
            .map_or(0, |r| r.captures_len().saturating_sub(1))
    }

    /// The names of the named groups, in order.
    pub fn group_names(&self) -> Vec<String> {
        self.inner
            .plain
            .as_ref()
            .map(|r| r.capture_names().flatten().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn regex(&self) -> Result<Option<&Regex>, MatchError> {
        let Some(plain) = self.inner.plain.as_ref() else {
            return Ok(None);
        };
        if !self.crlf {
            return Ok(Some(plain));
        }
        let Some(src) = self.inner.crlf_source.as_deref() else {
            return Ok(Some(plain));
        };
        self.inner
            .crlf
            .get_or_init(|| build(src, &self.inner.options))
            .as_ref()
            .map(Some)
            .map_err(Clone::clone)
    }

    /// First match starting at or after byte `from` (no wrapping).
    pub fn find_at(&self, text: &str, from: usize) -> Result<Option<Range<usize>>, MatchError> {
        let Some(re) = self.regex()? else {
            return Ok(None);
        };
        let from = ceil_char_boundary(text, from);
        Ok(re
            .find_from_pos(text, from)
            .map_err(map_error)?
            .map(|m| m.range()))
    }

    /// Iterates all non-overlapping matches from the start of `text`.
    pub fn iter<'a>(&'a self, text: &'a str) -> Matches<'a> {
        self.iter_range(text, 0..text.len())
    }

    /// Iterates the matches lying entirely inside `range` (context before and
    /// after the range is still visible to `^`, `\b` and look-around).
    pub fn iter_range<'a>(&'a self, text: &'a str, range: Range<usize>) -> Matches<'a> {
        let start = ceil_char_boundary(text, range.start);
        let end = floor_char_boundary(text, range.end).max(start);
        Matches {
            matcher: self,
            text,
            pos: start,
            limit: end,
            last_end: None,
            done: false,
        }
    }

    /// All matches (byte ranges, sorted).
    pub fn find_all(&self, text: &str) -> Result<Vec<Range<usize>>, MatchError> {
        self.iter(text).collect()
    }

    /// All matches inside `range` ("in selection").
    pub fn find_all_in(
        &self,
        text: &str,
        range: Range<usize>,
    ) -> Result<Vec<Range<usize>>, MatchError> {
        self.iter_range(text, range).collect()
    }

    /// At most `limit` matches; the flag says more exist.
    pub fn find_limited(
        &self,
        text: &str,
        limit: usize,
    ) -> Result<(Vec<Range<usize>>, bool), MatchError> {
        let mut out = Vec::new();
        for m in self.iter(text) {
            if out.len() == limit {
                return Ok((out, true));
            }
            out.push(m?);
        }
        Ok((out, false))
    }

    /// Number of matches, stopping at `limit`.
    pub fn count(&self, text: &str, limit: usize) -> Result<usize, MatchError> {
        let mut n = 0;
        for m in self.iter(text) {
            m?;
            n += 1;
            if n >= limit {
                break;
            }
        }
        Ok(n)
    }

    /// The next match at or after `from` (use the selection's **end** after a
    /// found match). With `wrap` it continues at the start of the text.
    pub fn find_next(
        &self,
        text: &str,
        from: usize,
        wrap: bool,
    ) -> Result<Option<Found>, MatchError> {
        if let Some(range) = self.find_at(text, from)? {
            return Ok(Some(Found {
                range,
                wrapped: false,
            }));
        }
        if wrap
            && from > 0
            && let Some(range) = self.find_at(text, 0)?
        {
            return Ok(Some(Found {
                range,
                wrapped: true,
            }));
        }
        Ok(None)
    }

    /// The previous match: the last one that **starts before** `before` (use
    /// the selection's **start** after a found match). With `wrap` it
    /// continues from the end of the text.
    ///
    /// Scans backwards in growing windows, so stepping back in a huge file
    /// costs time proportional to the distance to the previous match.
    pub fn find_prev(
        &self,
        text: &str,
        before: usize,
        wrap: bool,
    ) -> Result<Option<Found>, MatchError> {
        let before = floor_char_boundary(text, before);
        if let Some(range) = self.last_start_in(text, 0, before)? {
            return Ok(Some(Found {
                range,
                wrapped: false,
            }));
        }
        if wrap
            && before < text.len() + 1
            && let Some(range) = self.last_start_in(text, before, text.len() + 1)?
        {
            return Ok(Some(Found {
                range,
                wrapped: true,
            }));
        }
        Ok(None)
    }

    /// [`find_next`](Self::find_next) with the "Wrap Around" preference of
    /// this matcher's [`MatcherOptions`].
    pub fn find_next_wrapping(&self, text: &str, from: usize) -> Result<Option<Found>, MatchError> {
        self.find_next(text, from, self.options().wrap_around)
    }

    /// [`find_prev`](Self::find_prev) with the "Wrap Around" preference of
    /// this matcher's [`MatcherOptions`].
    pub fn find_prev_wrapping(
        &self,
        text: &str,
        before: usize,
    ) -> Result<Option<Found>, MatchError> {
        self.find_prev(text, before, self.options().wrap_around)
    }

    /// The last match whose start lies in `lo..hi`.
    fn last_start_in(
        &self,
        text: &str,
        lo: usize,
        hi: usize,
    ) -> Result<Option<Range<usize>>, MatchError> {
        if self.is_empty() || lo >= hi {
            return Ok(None);
        }
        let mut win_hi = hi;
        let mut size = 4096usize;
        loop {
            let win_lo = if win_hi <= lo + size {
                lo
            } else {
                let p = floor_char_boundary(text, win_hi - size);
                let snapped = memchr::memrchr(b'\n', &text.as_bytes()[..p]).map_or(0, |i| i + 1);
                snapped.max(lo)
            };
            let mut last = None;
            for m in self.iter_from(text, win_lo) {
                let m = m?;
                if m.start >= win_hi {
                    break;
                }
                last = Some(m);
            }
            if last.is_some() {
                return Ok(last);
            }
            if win_lo <= lo {
                return Ok(None);
            }
            win_hi = win_lo;
            size = size.saturating_mul(4);
        }
    }

    /// Iterates matches starting at `from` through the end of the text.
    pub fn iter_from<'a>(&'a self, text: &'a str, from: usize) -> Matches<'a> {
        self.iter_range(text, from..text.len())
    }

    /// Whether `range` is exactly one match (what "the selection is a match" means).
    pub fn is_match_at(&self, text: &str, range: &Range<usize>) -> Result<bool, MatchError> {
        Ok(self
            .find_at(text, range.start)?
            .is_some_and(|m| m == *range))
    }

    // ----------------------------------------------------------- replace

    /// Parses a replacement template for this matcher. In the non-regex modes
    /// the replacement is literal; in regex mode see [`crate::replacement`].
    pub fn compile_replacement(&self, template: &str) -> Result<Replacement, MatchError> {
        if !self.inner.options.mode.is_regex() {
            return Ok(Replacement::literal(template));
        }
        let (count, names): (usize, Vec<Option<String>>) = match &self.inner.plain {
            Some(re) => (
                re.captures_len(),
                re.capture_names().map(|n| n.map(str::to_string)).collect(),
            ),
            None => (1, vec![None]),
        };
        Replacement::parse(template, count, &|n| {
            names.iter().position(|x| x.as_deref() == Some(n))
        })
    }

    fn finish(&self, s: String) -> String {
        if self.crlf { to_crlf(&s) } else { s }
    }

    /// The edit that replaces the match found exactly at `range`
    /// (`None` when `range` is not a match of this search in `text`).
    pub fn replacement_for(
        &self,
        text: &str,
        range: &Range<usize>,
        rep: &Replacement,
    ) -> Result<Option<TextEdit>, MatchError> {
        let Some(re) = self.regex()? else {
            return Ok(None);
        };
        if range.start > text.len() || !text.is_char_boundary(range.start) {
            return Ok(None);
        }
        let Some(caps) = re.captures_from_pos(text, range.start).map_err(map_error)? else {
            return Ok(None);
        };
        let Some(m) = caps.get(0) else {
            return Ok(None);
        };
        if m.range() != *range {
            return Ok(None);
        }
        let mut out = String::new();
        rep.expand(|i| caps.get(i).map(|m| m.as_str()), &mut out);
        Ok(Some(TextEdit {
            range: range.clone(),
            text: self.finish(out),
        }))
    }

    /// "Replace": if `selection` is a match, the edit replacing it, plus where
    /// to continue searching in the edited text.
    ///
    /// ```
    /// # use scratch_search::*;
    /// let m = Matcher::simple("cat", MatchMode::Text, true).unwrap();
    /// let rep = m.compile_replacement("dog").unwrap();
    /// let step = m.replace_selection("a cat, a cat", &(2..5), &rep).unwrap();
    /// let edit = step.edit.unwrap();
    /// let new_text = apply_edits("a cat, a cat", &[edit]).unwrap();
    /// assert_eq!(new_text, "a dog, a cat");
    /// let next = m.find_next(&new_text, step.resume_forward, true).unwrap().unwrap();
    /// assert_eq!(next.range, 9..12);
    /// ```
    pub fn replace_selection(
        &self,
        text: &str,
        selection: &Range<usize>,
        rep: &Replacement,
    ) -> Result<ReplaceStep, MatchError> {
        match self.replacement_for(text, selection, rep)? {
            Some(edit) => {
                let end = edit.range.start + edit.text.len();
                Ok(ReplaceStep {
                    resume_forward: end,
                    resume_backward: edit.range.start,
                    edit: Some(edit),
                })
            }
            None => Ok(ReplaceStep {
                edit: None,
                resume_forward: selection.start,
                resume_backward: selection.end,
            }),
        }
    }

    /// Replaces every match (optionally only those inside `within`, "All In
    /// Selection"), at most `limit` of them. Edits whose replacement equals
    /// the matched text are dropped.
    pub fn replace_all(
        &self,
        text: &str,
        within: Option<Range<usize>>,
        rep: &Replacement,
        limit: Option<usize>,
    ) -> Result<ReplaceAll, MatchError> {
        let mut out = ReplaceAll::default();
        let Some(re) = self.regex()? else {
            return Ok(out);
        };
        let range = within.unwrap_or(0..text.len());
        let start = ceil_char_boundary(text, range.start);
        let end = floor_char_boundary(text, range.end).max(start);
        let mut pos = start;
        let mut last_end: Option<usize> = None;
        let literal = rep.is_literal();
        let limit = limit.unwrap_or(usize::MAX);
        let mut buf = String::new();
        while pos <= end && out.count < limit {
            let Some(caps) = re.captures_from_pos(text, pos).map_err(map_error)? else {
                break;
            };
            let Some(m) = caps.get(0) else { break };
            let r = m.range();
            if r.end > end {
                break;
            }
            if r.is_empty() {
                pos = r.end + text[r.end..].chars().next().map_or(1, char::len_utf8);
                if last_end == Some(r.end) {
                    continue;
                }
            } else {
                pos = r.end;
            }
            last_end = Some(r.end);
            buf.clear();
            if literal {
                rep.expand(|_| None, &mut buf);
            } else {
                rep.expand(|i| caps.get(i).map(|m| m.as_str()), &mut buf);
            }
            let new_text = self.finish(std::mem::take(&mut buf));
            if new_text != text[r.clone()] {
                out.edits.push(TextEdit {
                    range: r,
                    text: new_text,
                });
            }
            out.count += 1;
        }
        Ok(out)
    }
}

/// Converts bare `\n` to `\r\n` (existing `\r\n` stay).
fn to_crlf(s: &str) -> String {
    if !s.contains('\n') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut prev_cr = false;
    for c in s.chars() {
        if c == '\n' && !prev_cr {
            out.push('\r');
        }
        prev_cr = c == '\r';
        out.push(c);
    }
    out
}

/// Iterator over matches, see [`Matcher::iter`].
#[derive(Debug)]
pub struct Matches<'a> {
    matcher: &'a Matcher,
    text: &'a str,
    pos: usize,
    limit: usize,
    last_end: Option<usize>,
    done: bool,
}

impl Iterator for Matches<'_> {
    type Item = Result<Range<usize>, MatchError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done || self.pos > self.limit {
                return None;
            }
            let found = match self.matcher.find_at(self.text, self.pos) {
                Ok(f) => f,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            let Some(r) = found else {
                self.done = true;
                return None;
            };
            if r.end > self.limit {
                self.done = true;
                return None;
            }
            if r.is_empty() {
                self.pos = r.end + self.text[r.end..].chars().next().map_or(1, char::len_utf8);
                if self.last_end == Some(r.end) {
                    continue;
                }
            } else {
                self.pos = r.end;
            }
            self.last_end = Some(r.end);
            return Some(Ok(r));
        }
    }
}
