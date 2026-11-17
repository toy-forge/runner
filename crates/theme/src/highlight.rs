//! The incremental highlighting engine.
//!
//! A [`Highlighter`] owns a copy of the document text, the syntect parser
//! state at every 32nd line (`checkpoints`), and a theme. It answers "what are
//! the styled spans of lines `a..b`" by parsing from the nearest checkpoint,
//! and re-parses only what an edit invalidates:
//!
//! * [`Highlighter::update_text`] diffs the new text against the old (common
//!   prefix/suffix), keeps the checkpoints before the first changed line and
//!   shifts the ones after it aside as *stale*;
//! * parsing from the changed line stops being "new work" as soon as the
//!   state at a stale checkpoint equals the freshly computed one
//!   (convergence): everything after it is valid again without re-parsing;
//! * [`Highlighter::advance`] parses on in bounded slices (a time budget), so
//!   a 10 MB file is indexed in the background without ever blocking;
//! * [`Highlighter::highlight_lines`] returns spans for a visible range. If
//!   the checkpoints have not reached it within the budget (a jump to the end
//!   of a huge file) it *guesses* by parsing from a fresh state a few lines
//!   earlier and reports `exact: false`.
//!
//! The theme is not part of the parse state: [`Highlighter::set_theme`] only
//! clears the style cache, a theme switch re-parses nothing.
//!
//! Lines longer than [`MAX_LINE_BYTES`] are not parsed (they stay plain and
//! leave the parser state alone), which bounds the cost of minified files.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::resolve::{MatchedItem, ResolvedTheme, TextStyle};
use crate::syntax::{find_syntax, syntax_set};

/// A parser-state checkpoint is stored every this many lines.
pub const CHECKPOINT_INTERVAL: usize = 32;
/// Lines longer than this many bytes are left unhighlighted.
pub const MAX_LINE_BYTES: usize = 20_000;
/// How many lines before the first requested one a *guess* starts parsing
/// from a fresh state (enough for most multi-line constructs to re-open).
pub const GUESS_CONTEXT_LINES: usize = 40;

/// A run of equally styled text within one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyledSpan {
    /// First UTF-16 column (what the editor's `EditorPosition::column` uses).
    pub col_start: u32,
    /// UTF-16 column after the last unit.
    pub col_end: u32,
    /// First byte within the line.
    pub byte_start: u32,
    /// Byte after the last one.
    pub byte_end: u32,
    /// The resolved style.
    pub style: TextStyle,
}

/// The spans of one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpans {
    /// The line, from 0.
    pub line: u32,
    /// Spans in order, not overlapping; gaps are default-styled text when
    /// `skip_default` was requested, otherwise the line is covered.
    pub spans: Vec<StyledSpan>,
}

/// The answer to [`Highlighter::highlight_lines`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedRange {
    /// The requested lines that exist, in order (one entry per line, empty
    /// `spans` for plain lines).
    pub lines: Vec<LineSpans>,
    /// False when the parse had not reached the range and the spans were
    /// guessed from a fresh state; ask again after [`Highlighter::advance`].
    pub exact: bool,
}

/// How [`Highlighter::update_text`] changed the line structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextChange {
    /// The first line whose text changed.
    pub first_line: usize,
    /// How many old lines the change spanned.
    pub old_line_count: usize,
    /// How many lines it spans now.
    pub new_line_count: usize,
}

/// What [`Highlighter::scopes_at`] reports for the Color Scopes panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeInfo {
    /// Scope names, outermost first.
    pub stack: Vec<String>,
    /// The style the theme gives text there.
    pub style: TextStyle,
    /// Theme items that matched, best first.
    pub matched: Vec<MatchedItem>,
}

struct Checkpoint {
    line: usize,
    state: ParseState,
    scopes: ScopeStack,
}

/// One parse job for [`Highlighter::run`].
struct Job {
    start: usize,
    state: ParseState,
    stack: ScopeStack,
    end: usize,
    collect: Range<usize>,
    skip_default: bool,
    mode: Mode,
}

enum Mode {
    /// Store checkpoints and use convergence; stop at the deadline.
    Store(Option<Instant>),
    /// Parse without storing anything (guesses, scope queries).
    Peek,
}

/// An incremental, theme-resolving syntax highlighter for one document.
///
/// `Send` (move it into a worker thread or an `Arc<Mutex<_>>`), not `Sync`.
pub struct Highlighter {
    set: &'static SyntaxSet,
    syntax: Option<&'static SyntaxReference>,
    theme: Arc<ResolvedTheme>,
    text: Arc<str>,
    /// Byte offset of the start of every line; `[0]` for an empty text.
    line_starts: Vec<usize>,
    /// Valid checkpoints, ascending by line; `head[0]` is line 0.
    head: Vec<Checkpoint>,
    /// Checkpoints from before an edit, shifted to the new line numbers:
    /// only good for convergence checks.
    tail: Vec<Checkpoint>,
    styles: HashMap<Box<[Scope]>, TextStyle>,
}

// SAFETY: with Oniguruma's regexes a `ParseState` keeps the captures of the
// match that pushed a context as an `onig::Region`, which owns raw pointers to
// memory it allocated and frees itself. Nothing in it is shared with another
// owner or tied to a thread, so moving the whole highlighter to another
// thread is sound (NSH's own highlighter makes the same claim for its parse
// state). It is deliberately not `Sync`.
unsafe impl Send for Highlighter {}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("syntax", &self.syntax.map(|s| s.name.as_str()))
            .field("lines", &self.line_starts.len())
            .field("checkpoints", &(self.head.len(), self.tail.len()))
            .finish()
    }
}

fn compute_line_starts(text: &str, from_line_start: usize, into: &mut Vec<usize>) {
    for (i, _) in text[from_line_start..].match_indices('\n') {
        into.push(from_line_start + i + 1);
    }
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    const BLOCK: usize = 4096;
    while i + BLOCK <= n && a[i..i + BLOCK] == b[i..i + BLOCK] {
        i += BLOCK;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn common_suffix(a: &[u8], b: &[u8], max: usize) -> usize {
    let n = a.len().min(b.len()).min(max);
    let mut i = 0;
    const BLOCK: usize = 4096;
    while i + BLOCK <= n && a[a.len() - i - BLOCK..a.len() - i] == b[b.len() - i - BLOCK..b.len() - i] {
        i += BLOCK;
    }
    while i < n && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

/// Walks a line forward converting byte offsets to UTF-16 columns.
struct Utf16Cursor<'a> {
    line: &'a str,
    ascii: bool,
    byte: usize,
    col: u32,
}

impl<'a> Utf16Cursor<'a> {
    fn new(line: &'a str) -> Self {
        Utf16Cursor {
            line,
            ascii: line.is_ascii(),
            byte: 0,
            col: 0,
        }
    }

    /// Offsets must be asked in ascending order.
    fn col_at(&mut self, byte: usize) -> u32 {
        if self.ascii {
            return byte as u32;
        }
        for c in self.line[self.byte..byte].chars() {
            self.col += c.len_utf16() as u32;
        }
        self.byte = byte;
        self.col
    }
}

/// The byte offset of UTF-16 column `col` in `line` (clamped to the end).
fn byte_of_col(line: &str, col: u32) -> usize {
    if line.is_ascii() {
        return (col as usize).min(line.len());
    }
    let mut units = 0u32;
    for (i, c) in line.char_indices() {
        if units >= col {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    line.len()
}

impl Highlighter {
    /// A highlighter for `language` (an editor language name, extension or
    /// file name; see [`find_syntax`]) with `theme`, holding no text yet.
    /// Unknown languages and plain text highlight nothing.
    pub fn new(language: &str, theme: Arc<ResolvedTheme>) -> Highlighter {
        let mut h = Highlighter {
            set: syntax_set(),
            syntax: find_syntax(language),
            theme,
            text: Arc::from(""),
            line_starts: vec![0],
            head: Vec::new(),
            tail: Vec::new(),
            styles: HashMap::new(),
        };
        h.reset_checkpoints();
        h
    }

    fn reset_checkpoints(&mut self) {
        self.head.clear();
        self.tail.clear();
        if let Some(syntax) = self.syntax {
            self.head.push(Checkpoint {
                line: 0,
                state: ParseState::new(syntax),
                scopes: ScopeStack::new(),
            });
        }
    }

    /// Whether anything is highlighted (a known, non-plain syntax).
    pub fn is_active(&self) -> bool {
        self.syntax.is_some()
    }

    /// The syntax name, if any.
    pub fn syntax_name(&self) -> Option<&str> {
        self.syntax.map(|s| s.name.as_str())
    }

    /// Switches language; drops all parse state.
    pub fn set_language(&mut self, language: &str) {
        self.syntax = find_syntax(language);
        self.reset_checkpoints();
    }

    /// Switches theme. Parse state is theme independent: nothing is
    /// re-parsed, only the style cache is cleared.
    pub fn set_theme(&mut self, theme: Arc<ResolvedTheme>) {
        self.theme = theme;
        self.styles.clear();
    }

    /// The current theme.
    pub fn theme(&self) -> &Arc<ResolvedTheme> {
        &self.theme
    }

    /// The text the highlighter holds.
    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    /// Number of lines (a trailing `\n` makes a last empty line, as in the
    /// editor).
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    fn line_end(&self, line: usize) -> usize {
        self.line_starts.get(line + 1).copied().unwrap_or(self.text.len())
    }

    /// The text of `line` without its line break.
    pub fn line_text(&self, line: usize) -> &str {
        let Some(&start) = self.line_starts.get(line) else {
            return "";
        };
        self.text[start..self.line_end(line)].trim_end_matches('\n')
    }

    fn line_of(starts: &[usize], offset: usize) -> usize {
        starts.partition_point(|&s| s <= offset).saturating_sub(1)
    }

    /// Replaces the document text, re-using everything the old and new text
    /// share. Returns what changed (`None`: identical). Costs O(prefix and
    /// suffix comparison + changed lines) plus invalidation of the parse
    /// state after the first changed line; nothing is parsed here.
    pub fn update_text(&mut self, new: impl Into<Arc<str>>) -> Option<TextChange> {
        let new: Arc<str> = new.into();
        if *new == *self.text {
            return None;
        }
        let old = std::mem::replace(&mut self.text, new);
        let new = &self.text;
        let mut prefix = common_prefix(old.as_bytes(), new.as_bytes());
        while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = common_suffix(old.as_bytes(), new.as_bytes(), old.len().min(new.len()) - prefix);
        while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
            suffix -= 1;
        }
        let old_end = old.len() - suffix;
        let new_end = new.len() - suffix;
        let first = Self::line_of(&self.line_starts, prefix);
        let old_last = Self::line_of(&self.line_starts, old_end);
        // New line starts: keep the unchanged prefix, rescan from the first
        // changed line.
        let first_start = self.line_starts[first];
        self.line_starts.truncate(first + 1);
        compute_line_starts(new, first_start, &mut self.line_starts);
        let new_last = Self::line_of(&self.line_starts, new_end);
        let change = TextChange {
            first_line: first,
            old_line_count: old_last - first + 1,
            new_line_count: new_last - first + 1,
        };
        self.invalidate(change);
        Some(change)
    }

    /// Moves checkpoints after an edit: those at or before the first
    /// changed line stay; later ones become stale (shifted, or dropped when
    /// inside the changed lines).
    fn invalidate(&mut self, change: TextChange) {
        let first = change.first_line;
        let old_end = first + change.old_line_count; // exclusive
        let delta = change.new_line_count as isize - change.old_line_count as isize;
        // Stale checkpoints of an earlier, unresolved edit are relative to
        // text that has changed since: only checkpoints at or before this
        // edit's first line are still meaningful for them, and they are not
        // valid either. Drop them all, the next parse simply runs on.
        let mut stale_old: Vec<Checkpoint> = Vec::new();
        let keep = self.head.partition_point(|c| c.line <= first);
        stale_old.extend(self.head.drain(keep..));
        let had_tail = !self.tail.is_empty();
        self.tail.clear();
        if had_tail {
            // Cannot trust shifted tail entries across two pending edits.
            return;
        }
        self.tail = stale_old
            .into_iter()
            .filter(|c| c.line >= old_end)
            .map(|mut c| {
                c.line = (c.line as isize + delta).max(0) as usize;
                c
            })
            .collect();
    }

    /// Parses `line` (with its `\n`), updating `state`/`stack`; `sink` gets
    /// `(byte_start, byte_end, scope stack)` for every run. Returns false if
    /// the parser failed (state untouched on a skipped line).
    fn parse_one(
        set: &SyntaxSet,
        line: &str,
        state: &mut ParseState,
        stack: &mut ScopeStack,
        sink: &mut dyn FnMut(usize, usize, &[Scope]),
    ) -> bool {
        if line.len() > MAX_LINE_BYTES {
            sink(0, line.len(), &[]);
            return true;
        }
        let Ok(ops) = state.parse_line(line, set) else {
            sink(0, line.len(), stack.as_slice());
            return false;
        };
        let mut prev = 0usize;
        for (i, op) in ops {
            if i > prev {
                sink(prev, i, stack.as_slice());
                prev = i;
            }
            if stack.apply(&op).is_err() {
                return false;
            }
        }
        if prev < line.len() {
            sink(prev, line.len(), stack.as_slice());
        }
        true
    }

    /// The line's text including `\n`, owned only when it lacks one.
    fn parse_text<'a>(text: &'a str, start: usize, end: usize, scratch: &'a mut String) -> &'a str {
        let line = &text[start..end];
        if line.ends_with('\n') {
            line
        } else {
            scratch.clear();
            scratch.push_str(line);
            scratch.push('\n');
            scratch.as_str()
        }
    }

    /// Core loop: parses from `(line, state, stack)` up to `end` (exclusive),
    /// collecting spans for lines in `collect`. In `Mode::Store` it records
    /// checkpoints past the last valid one and stops extending on
    /// convergence. Returns the collected lines and the line parsing stopped
    /// at (equal to `end` unless the deadline passed).
    fn run(&mut self, job: Job) -> (Vec<LineSpans>, usize) {
        let Job {
            start,
            mut state,
            mut stack,
            end,
            collect,
            skip_default,
            mode,
        } = job;
        let text = self.text.clone();
        let set = self.set;
        let end = end.min(self.line_starts.len());
        let (store, deadline) = match mode {
            Mode::Store(d) => (true, d),
            Mode::Peek => (false, None),
        };
        let default_style = self.theme.default_style();
        let mut ext_line = self.head.last().map_or(0, |c| c.line);
        let mut extending = store;
        let mut tail_index = 0usize;
        let mut collected = Vec::new();
        let mut scratch = String::new();
        let mut k = start;
        while k < end {
            if extending && k > ext_line {
                while tail_index < self.tail.len() && self.tail[tail_index].line < k {
                    tail_index += 1;
                }
                if tail_index < self.tail.len()
                    && self.tail[tail_index].line == k
                    && self.tail[tail_index].state == state
                    && self.tail[tail_index].scopes == stack
                {
                    // Converged: everything from here on is what it was.
                    let rest: Vec<Checkpoint> = self.tail.drain(tail_index..).collect();
                    self.tail.clear();
                    self.head.extend(rest);
                    ext_line = self.head.last().map_or(k, |c| c.line);
                    extending = false;
                } else if k - ext_line >= CHECKPOINT_INTERVAL {
                    self.head.push(Checkpoint {
                        line: k,
                        state: state.clone(),
                        scopes: stack.clone(),
                    });
                    ext_line = k;
                }
            }
            let (ls, le) = (self.line_starts[k], self.line_end(k));
            let line_with_nl = Self::parse_text(&text, ls, le, &mut scratch);
            let content_len = line_with_nl.trim_end_matches('\n').len();
            if collect.contains(&k) {
                let mut runs: Vec<(usize, usize, TextStyle)> = Vec::new();
                let styles = &mut self.styles;
                let theme = &self.theme;
                let mut sink = |a: usize, b: usize, scopes: &[Scope]| {
                    let b = b.min(content_len);
                    if a >= b {
                        return;
                    }
                    let style = match styles.get(scopes) {
                        Some(&s) => s,
                        None => {
                            let names: Vec<String> = scopes.iter().map(|s| s.build_string()).collect();
                            let s = theme.style_for(&names);
                            if styles.len() > 50_000 {
                                styles.clear();
                            }
                            styles.insert(scopes.into(), s);
                            s
                        }
                    };
                    match runs.last_mut() {
                        Some(last) if last.1 == a && last.2 == style => last.1 = b,
                        _ => runs.push((a, b, style)),
                    }
                };
                Self::parse_one(set, line_with_nl, &mut state, &mut stack, &mut sink);
                let content = &line_with_nl[..content_len];
                let mut cursor = Utf16Cursor::new(content);
                let spans = runs
                    .into_iter()
                    .filter(|(_, _, s)| !(skip_default && *s == default_style))
                    .map(|(a, b, style)| StyledSpan {
                        col_start: cursor.col_at(a),
                        col_end: cursor.col_at(b),
                        byte_start: a as u32,
                        byte_end: b as u32,
                        style,
                    })
                    .collect();
                collected.push(LineSpans {
                    line: k as u32,
                    spans,
                });
            } else {
                Self::parse_one(set, line_with_nl, &mut state, &mut stack, &mut |_, _, _| {});
            }
            k += 1;
            if k.is_multiple_of(4)
                && let Some(deadline) = deadline
                && Instant::now() >= deadline
            {
                break;
            }
        }
        (collected, k)
    }

    /// Whether valid checkpoints reach line `line` (a parse from the last one
    /// is at most [`CHECKPOINT_INTERVAL`] lines).
    fn reaches(&self, line: usize) -> bool {
        match self.head.last() {
            Some(last) => last.line + CHECKPOINT_INTERVAL > line || last.line + 1 >= self.line_starts.len(),
            None => true,
        }
    }

    /// Parses on from the last valid checkpoint until `line` is reachable,
    /// for at most `budget`. Returns whether it is.
    fn extend_to(&mut self, line: usize, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while !self.reaches(line) {
            let Some(last) = self.head.last() else {
                return true;
            };
            let (start, state, scopes) = (last.line, last.state.clone(), last.scopes.clone());
            // Run in slices so a deadline can interrupt a long file.
            let target = (start + 4096).min(line + 1);
            let (_, stopped) = self.run(Job {
                start,
                state,
                stack: scopes,
                end: target,
                collect: 0..0,
                skip_default: false,
                mode: Mode::Store(Some(deadline)),
            });
            if stopped < target || Instant::now() >= deadline {
                return self.reaches(line);
            }
        }
        true
    }

    /// Parses on in the background-friendly way: for at most `budget`,
    /// extend the checkpoints towards the end of the text. Returns true when
    /// the whole text has valid checkpoints (nothing left to do).
    pub fn advance(&mut self, budget: Duration) -> bool {
        if self.syntax.is_none() {
            return true;
        }
        let last_line = self.line_starts.len().saturating_sub(1);
        self.extend_to(last_line, budget)
    }

    /// Whether [`Highlighter::advance`] has nothing left to do.
    pub fn is_fully_parsed(&self) -> bool {
        self.syntax.is_none() || self.reaches(self.line_starts.len().saturating_sub(1))
    }

    /// The first line without a valid checkpoint nearby (progress of the
    /// background parse).
    pub fn parsed_through(&self) -> usize {
        self.head.last().map_or(0, |c| c.line)
    }

    /// Styled spans for lines `first..last_exclusive`.
    ///
    /// Tries to reach the range with the exact parse state for at most
    /// `budget`; otherwise guesses (see [`HighlightedRange::exact`]). With
    /// `skip_default`, text in the theme's default style gets no span (this
    /// is what to turn into editor decorations).
    pub fn highlight_lines(
        &mut self,
        first: usize,
        last_exclusive: usize,
        skip_default: bool,
        budget: Duration,
    ) -> HighlightedRange {
        let count = self.line_starts.len();
        let first = first.min(count);
        let last_exclusive = last_exclusive.min(count).max(first);
        if self.syntax.is_none() {
            return HighlightedRange {
                lines: (first..last_exclusive)
                    .map(|l| LineSpans {
                        line: l as u32,
                        spans: Vec::new(),
                    })
                    .collect(),
                exact: true,
            };
        }
        if self.extend_to(first.min(count.saturating_sub(1)), budget) {
            let idx = self.head.partition_point(|c| c.line <= first).saturating_sub(1);
            let cp = &self.head[idx];
            let (start, state, scopes) = (cp.line, cp.state.clone(), cp.scopes.clone());
            let (lines, _) = self.run(Job {
                start,
                state,
                stack: scopes,
                end: last_exclusive,
                collect: first..last_exclusive,
                skip_default,
                mode: Mode::Store(None),
            });
            return HighlightedRange { lines, exact: true };
        }
        let start = first.saturating_sub(GUESS_CONTEXT_LINES);
        let (state, scopes) = match self.syntax {
            Some(syntax) => (ParseState::new(syntax), ScopeStack::new()),
            None => return HighlightedRange { lines: Vec::new(), exact: true },
        };
        let (lines, _) = self.run(Job {
            start,
            state,
            stack: scopes,
            end: last_exclusive,
            collect: first..last_exclusive,
            skip_default,
            mode: Mode::Peek,
        });
        HighlightedRange { lines, exact: false }
    }

    /// The scope stack, theme style and matching theme items at `line` and
    /// UTF-16 column `column` (the character after the caret), for the Color
    /// Scopes panel. `None` when the syntax is plain or the position is out
    /// of range, or the parse could not reach the line within `budget`.
    pub fn scopes_at(&mut self, line: usize, column: u32, budget: Duration) -> Option<ScopeInfo> {
        self.syntax?;
        if line >= self.line_starts.len() || !self.extend_to(line, budget) {
            return None;
        }
        let idx = self.head.partition_point(|c| c.line <= line).saturating_sub(1);
        let cp = &self.head[idx];
        let (start, mut state, mut stack) = (cp.line, cp.state.clone(), cp.scopes.clone());
        let text = self.text.clone();
        let set = self.set;
        let mut scratch = String::new();
        for k in start..line {
            let (ls, le) = (self.line_starts[k], self.line_end(k));
            let l = Self::parse_text(&text, ls, le, &mut scratch);
            Self::parse_one(set, l, &mut state, &mut stack, &mut |_, _, _| {});
        }
        let (ls, le) = (self.line_starts[line], self.line_end(line));
        let l = Self::parse_text(&text, ls, le, &mut scratch);
        let content = l.trim_end_matches('\n');
        let at = byte_of_col(content, column);
        if l.len() > MAX_LINE_BYTES {
            return Some(self.scope_info(&[]));
        }
        let ops = state.parse_line(l, set).ok()?;
        for (i, op) in ops {
            if i > at {
                break;
            }
            stack.apply(&op).ok()?;
        }
        Some(self.scope_info(stack.as_slice()))
    }

    fn scope_info(&self, stack: &[Scope]) -> ScopeInfo {
        let names: Vec<String> = stack.iter().map(|s| s.build_string()).collect();
        let (style, matched) = self.theme.explain(&names);
        ScopeInfo {
            stack: names,
            style,
            matched,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::builtin_themes;

    fn theme() -> Arc<ResolvedTheme> {
        let dark = builtin_themes().into_iter().find(|t| t.name.contains("Dark")).expect("dark");
        Arc::new(dark.compile())
    }

    fn rust_doc(lines: usize) -> String {
        let mut s = String::new();
        for i in 0..lines {
            s.push_str(&format!("fn f{i}(x: u32) -> u32 {{ x + {i} /* c */ }} // trailing\n"));
        }
        s
    }

    fn all(h: &mut Highlighter) -> Vec<LineSpans> {
        let n = h.line_count();
        h.highlight_lines(0, n, false, Duration::from_secs(30)).lines
    }

    /// Reference: highlight a text from scratch.
    fn fresh(text: &str) -> Vec<LineSpans> {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text(text);
        all(&mut h)
    }

    #[test]
    fn highlights_rust_with_theme_colours() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("fn main() {\n    let s = \"hi\"; // note\n}\n");
        let lines = all(&mut h);
        assert_eq!(lines.len(), 4);
        let theme = theme();
        let kw = theme.style_for(&["source.rust", "storage.type.function.rust"]);
        let first = &lines[0].spans[0];
        assert_eq!((first.byte_start, first.byte_end), (0, 2));
        assert_eq!(first.style.foreground, kw.foreground);
        let string = theme.style_for(&["source.rust", "string.quoted.double.rust"]);
        assert!(lines[1].spans.iter().any(|s| s.style.foreground == string.foreground));
        let comment = lines[1].spans.last().expect("span");
        assert!(comment.style.italic);
        // Spans cover the line exactly.
        let covered: u32 = lines[1].spans.iter().map(|s| s.byte_end - s.byte_start).sum();
        assert_eq!(covered as usize, h.line_text(1).len());
    }

    #[test]
    fn skip_default_drops_plain_text() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("fn main() {}\n");
        let all_spans = h.highlight_lines(0, 1, false, Duration::from_secs(1)).lines;
        let skipped = h.highlight_lines(0, 1, true, Duration::from_secs(1)).lines;
        assert!(skipped[0].spans.len() < all_spans[0].spans.len());
        let d = h.theme().default_style();
        assert!(skipped[0].spans.iter().all(|s| s.style != d));
    }

    #[test]
    fn utf16_columns_differ_from_bytes_for_non_ascii() {
        let mut h = Highlighter::new("Rust", theme());
        // "é" is 2 bytes / 1 unit, "😀" 4 bytes / 2 units.
        h.update_text("let s = \"é😀\"; // x\n");
        let spans = h.highlight_lines(0, 1, false, Duration::from_secs(1)).lines.remove(0).spans;
        let comment = spans.last().expect("comment");
        assert_eq!(comment.byte_start - comment.col_start, 1 + 2);
        assert!(spans.iter().all(|s| s.col_start <= s.col_end));
    }

    #[test]
    fn edits_match_a_fresh_parse() {
        let base = rust_doc(300);
        let mut h = Highlighter::new("Rust", theme());
        h.update_text(base.as_str());
        let _ = all(&mut h);
        let mut text = base.clone();
        // A series of edits: typing, deleting, newlines, opening a block comment.
        let edits: [(usize, usize, &str); 6] = [
            (10, 10, "x"),
            (500, 520, ""),
            (3000, 3000, "\n\n"),
            (3500, 3500, "/*"), // opens a comment that swallows the rest
            (3600, 3600, "*/"), // and closes it again
            (0, 0, "// head\n"),
        ];
        for (a, b, ins) in edits {
            let a = a.min(text.len());
            let b = b.min(text.len());
            let mut a2 = a;
            while !text.is_char_boundary(a2) {
                a2 -= 1;
            }
            let mut b2 = b.max(a2);
            while !text.is_char_boundary(b2) {
                b2 += 1;
            }
            text.replace_range(a2..b2, ins);
            h.update_text(text.as_str());
            let got = all(&mut h);
            assert_eq!(got, fresh(&text), "after edit {a}..{b} -> {ins:?}");
        }
    }

    #[test]
    fn convergence_avoids_reparsing_the_whole_file() {
        let base = rust_doc(2000);
        let mut h = Highlighter::new("Rust", theme());
        h.update_text(base.as_str());
        assert!(h.advance(Duration::from_secs(30)));
        let through = h.parsed_through();
        assert!(through > 1900);
        // Edit near the top: head shrinks to the edit, tail keeps the rest.
        let mut text = base.clone();
        text.insert(40, 'z');
        let change = h.update_text(text.as_str()).expect("changed");
        assert_eq!(change.first_line, 0);
        assert!(h.parsed_through() <= CHECKPOINT_INTERVAL);
        // Highlighting line 0 reaches only a little way; the far checkpoints are stale.
        let _ = h.highlight_lines(0, 50, true, Duration::from_secs(5));
        // Converged: the checkpoint chain is whole again without parsing 2000 lines.
        assert!(h.parsed_through() > 1900, "parsed through {}", h.parsed_through());
        assert!(h.is_fully_parsed());
    }

    #[test]
    fn unchanged_text_is_a_no_op() {
        let mut h = Highlighter::new("Rust", theme());
        assert!(h.update_text("a\n").is_some());
        assert!(h.update_text("a\n").is_none());
    }

    #[test]
    fn line_structure_follows_edits() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("a\nb\nc");
        assert_eq!(h.line_count(), 3);
        let change = h.update_text("a\nb1\nb2\nc").expect("change");
        assert_eq!(change.first_line, 1);
        assert_eq!(h.line_count(), 4);
        assert_eq!(h.line_text(2), "b2");
        h.update_text("a\nb1\nb2\nc\n");
        assert_eq!(h.line_count(), 5);
        assert_eq!(h.line_text(4), "");
        h.update_text("");
        assert_eq!(h.line_count(), 1);
    }

    #[test]
    fn far_ranges_are_guessed_within_a_tiny_budget_then_exact() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text(rust_doc(20_000).as_str());
        let guess = h.highlight_lines(19_000, 19_050, true, Duration::ZERO);
        assert!(!guess.exact);
        assert_eq!(guess.lines.len(), 50);
        assert!(guess.lines.iter().any(|l| !l.spans.is_empty()));
        assert!(h.advance(Duration::from_secs(60)));
        let exact = h.highlight_lines(19_000, 19_050, true, Duration::ZERO);
        assert!(exact.exact);
        // In plain code the guess agrees with the exact parse.
        assert_eq!(guess.lines, exact.lines);
    }

    #[test]
    fn theme_switch_does_not_reparse_and_recolours() {
        let themes = builtin_themes();
        let a = Arc::new(themes[0].compile());
        let b = Arc::new(themes[1].compile());
        let mut h = Highlighter::new("Rust", a);
        h.update_text(rust_doc(100).as_str());
        let before = h.highlight_lines(0, 5, true, Duration::from_secs(1)).lines;
        let through = h.parsed_through();
        h.set_theme(b);
        let after = h.highlight_lines(0, 5, true, Duration::from_secs(1)).lines;
        assert_eq!(h.parsed_through(), through);
        assert_ne!(before, after);
        assert_eq!(before.len(), after.len());
        // Same span geometry, different colours.
        let geo = |l: &Vec<LineSpans>| l.iter().map(|l| l.spans.iter().map(|s| (s.byte_start, s.byte_end)).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(geo(&before), geo(&after));
    }

    #[test]
    fn multi_line_constructs_carry_state() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("/* open\nstill comment\n*/ let x = 1;\n");
        let lines = all(&mut h);
        let comment = h.theme().style_for(&["source.rust", "comment.block.rust"]);
        assert_eq!(lines[1].spans[0].style.foreground, comment.foreground);
        assert_ne!(lines[2].spans.last().expect("span").style.foreground, comment.foreground);
    }

    #[test]
    fn scopes_at_reports_the_stack_and_matches() {
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("fn main() {\n    let s = \"hi\";\n}\n");
        let info = h.scopes_at(1, 13, Duration::from_secs(1)).expect("info");
        assert!(info.stack.iter().any(|s| s.starts_with("string.quoted.double")), "{:?}", info.stack);
        assert_eq!(info.stack[0], "source.rust");
        assert!(info.matched.iter().any(|m| m.name.as_deref() == Some("Strings")));
        assert!(h.scopes_at(99, 0, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn plain_text_and_unknown_languages_are_empty() {
        for language in ["Plain Text", "nope"] {
            let mut h = Highlighter::new(language, theme());
            h.update_text("a b\nc\n");
            assert!(!h.is_active());
            let r = h.highlight_lines(0, 3, true, Duration::ZERO);
            assert!(r.exact && r.lines.iter().all(|l| l.spans.is_empty()));
            assert!(h.advance(Duration::ZERO));
        }
    }

    #[test]
    fn very_long_lines_stay_plain_and_do_not_break_state() {
        let mut h = Highlighter::new("Rust", theme());
        let long = "x".repeat(MAX_LINE_BYTES + 10);
        h.update_text(format!("{long}\nfn a() {{}}\n").as_str());
        let lines = all(&mut h);
        assert!(lines[0].spans.iter().all(|s| s.style == h.theme().default_style()));
        assert!(lines[1].spans.iter().any(|s| s.style != h.theme().default_style()));
    }

    #[test]
    fn highlighter_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Highlighter>();
        let mut h = Highlighter::new("Rust", theme());
        h.update_text("fn a() {}\n");
        let joined = std::thread::spawn(move || {
            let n = h.line_count();
            h.highlight_lines(0, n, true, Duration::from_secs(1)).lines.len()
        })
        .join();
        assert_eq!(joined.ok(), Some(2));
    }
}
