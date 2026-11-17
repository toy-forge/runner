//! Stored lines, and the borrowed/owned [`LineView`] the UI reads.

use std::borrow::Cow;
use std::ops::Range;

use crate::links::{Link, detect_links};
use crate::style::Style;
use crate::tag::{Level, ScopeMask, Stream};

/// A line's stable identity: assigned when the line is created, never reused,
/// not shifted by eviction or by `Console::clear`. Use it as the list key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LineId(pub u64);

/// A stretch of a line's text written in one style by one stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Run {
    /// Length in bytes.
    pub len: u32,
    /// SGR style.
    pub style: Style,
    /// Origin of the text.
    pub stream: Stream,
}

#[derive(Clone, Debug)]
pub(crate) enum Runs {
    One(Run),
    Many(Box<[Run]>),
}

impl Runs {
    pub(crate) fn as_slice(&self) -> &[Run] {
        match self {
            Runs::One(r) => std::slice::from_ref(r),
            Runs::Many(m) => m,
        }
    }
}

/// One stored line.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub(crate) text: Box<str>,
    pub(crate) runs: Runs,
    pub(crate) level: Level,
    pub(crate) continued: bool,
}

impl Line {
    pub(crate) fn mask(&self) -> ScopeMask {
        ScopeMask::of(self.runs.as_slice().iter().filter(|r| r.len > 0 || self.text.is_empty()).map(|r| r.stream), self.level)
    }
}

/// A run with its text and position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StyledRun<'a> {
    /// Byte range in the line.
    pub range_start: usize,
    /// The text.
    pub text: &'a str,
    /// SGR style.
    pub style: Style,
    /// Origin.
    pub stream: Stream,
}

impl StyledRun<'_> {
    /// Byte range in the line.
    pub fn range(&self) -> Range<usize> {
        self.range_start..self.range_start + self.text.len()
    }
}

/// Highlight marks a [`Span`] can carry (a bit set).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Marks(pub u8);

impl Marks {
    /// Inside a search match.
    pub const SEARCH: Marks = Marks(1);
    /// Inside the current search match.
    pub const CURRENT_SEARCH: Marks = Marks(2);
    /// Inside the selection.
    pub const SELECTED: Marks = Marks(4);
    /// Inside an imported console diagnostic (a matched error location).
    pub const DIAGNOSTIC: Marks = Marks(8);

    /// Whether all bits of `other` are set.
    pub fn contains(self, other: Marks) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Overlays for [`LineView::spans`]. Ranges are byte offsets in the line.
#[derive(Clone, Copy, Default, Debug)]
pub struct SpanOptions<'a> {
    /// Links from [`LineView::links`] (spans inside get `link = Some(index)`).
    pub links: &'a [Link],
    /// Search matches.
    pub search: &'a [Range<usize>],
    /// The current search match (also in `search`).
    pub current_search: Option<&'a Range<usize>>,
    /// The selected range of this line.
    pub selection: Option<&'a Range<usize>>,
    /// Imported console-diagnostic locations (`Marks::DIAGNOSTIC`).
    pub diagnostics: &'a [Range<usize>],
}

/// A piece of a line with uniform style, link and marks: what a renderer draws
/// as one text span.
#[derive(Clone, PartialEq, Debug)]
pub struct Span<'a> {
    /// Byte range in the line.
    pub range: Range<usize>,
    /// The text.
    pub text: &'a str,
    /// SGR style.
    pub style: Style,
    /// Origin.
    pub stream: Stream,
    /// Index into [`SpanOptions::links`] when the span is part of a link.
    pub link: Option<usize>,
    /// Search / selection marks.
    pub marks: Marks,
}

/// A read-only view of one line (borrowed for stored lines, owned for the
/// still-open last line).
#[derive(Clone, Debug)]
pub struct LineView<'a> {
    pub(crate) id: LineId,
    pub(crate) line: Cow<'a, Line>,
    pub(crate) is_open: bool,
}

impl<'a> LineView<'a> {
    /// The stable id.
    pub fn id(&self) -> LineId {
        self.id
    }

    /// The text (no trailing newline; tabs expanded; no escape sequences).
    pub fn text(&self) -> &str {
        &self.line.text
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.line.text.len()
    }

    /// Whether the line has no text.
    pub fn is_empty(&self) -> bool {
        self.line.text.is_empty()
    }

    /// Severity.
    pub fn level(&self) -> Level {
        self.line.level
    }

    /// True when this line is the soft-wrapped continuation of the previous
    /// one (a copy joins them without a newline).
    pub fn is_continued(&self) -> bool {
        self.line.continued
    }

    /// True for the last line while the program has not ended it with a
    /// newline yet (a prompt, a progress bar). Its content may still change.
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// The scope bits of this line.
    pub fn mask(&self) -> ScopeMask {
        self.line.mask()
    }

    /// The runs (same-style, same-stream stretches) in order.
    pub fn runs(&self) -> &[Run] {
        self.line.runs.as_slice()
    }

    /// The runs with their text.
    pub fn styled_runs(&self) -> impl Iterator<Item = StyledRun<'_>> {
        let text: &str = &self.line.text;
        let mut at = 0usize;
        self.line.runs.as_slice().iter().filter_map(move |r| {
            let start = at;
            at += r.len as usize;
            (r.len > 0).then(|| StyledRun {
                range_start: start,
                text: &text[start..at],
                style: r.style,
                stream: r.stream,
            })
        })
    }

    /// The stream of the character at byte `offset` (the last run's stream
    /// when past the end).
    pub fn stream_at(&self, offset: usize) -> Stream {
        let mut at = 0usize;
        let runs = self.line.runs.as_slice();
        for r in runs {
            at += r.len as usize;
            if offset < at {
                return r.stream;
            }
        }
        runs.last().map_or(Stream::Stdout, |r| r.stream)
    }

    /// Clickable ranges in this line.
    pub fn links(&self) -> Vec<Link> {
        detect_links(self.text())
    }

    /// The line cut at every run, link, search and selection boundary, each
    /// piece carrying what applies to it. Empty for an empty line.
    pub fn spans<'s>(&'s self, opts: &SpanOptions<'_>) -> Vec<Span<'s>> {
        let text: &str = &self.line.text;
        let len = text.len();
        if len == 0 {
            return Vec::new();
        }
        let clamp = |o: usize| crate::textutil::floor_char_boundary(text, o);
        let mut cuts: Vec<usize> = Vec::with_capacity(8 + opts.search.len() * 2);
        cuts.push(0);
        cuts.push(len);
        let mut at = 0usize;
        for r in self.line.runs.as_slice() {
            at += r.len as usize;
            cuts.push(at.min(len));
        }
        for l in opts.links {
            cuts.push(clamp(l.range.start));
            cuts.push(clamp(l.range.end));
        }
        for r in opts.search.iter().chain(opts.current_search).chain(opts.selection).chain(opts.diagnostics) {
            cuts.push(clamp(r.start));
            cuts.push(clamp(r.end));
        }
        cuts.sort_unstable();
        cuts.dedup();

        let covers = |r: &Range<usize>, a: usize, b: usize| clamp(r.start) <= a && b <= clamp(r.end);
        let runs = self.line.runs.as_slice();
        let (mut run_idx, mut run_end) = (0usize, runs.first().map_or(len, |r| r.len as usize));
        let mut out = Vec::with_capacity(cuts.len());
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a == b {
                continue;
            }
            while a >= run_end && run_idx + 1 < runs.len() {
                run_idx += 1;
                run_end += runs[run_idx].len as usize;
            }
            let run = runs.get(run_idx);
            let mut marks = 0u8;
            if opts.search.iter().any(|r| covers(r, a, b)) {
                marks |= Marks::SEARCH.0;
            }
            if opts.current_search.is_some_and(|r| covers(r, a, b)) {
                marks |= Marks::CURRENT_SEARCH.0;
            }
            if opts.selection.is_some_and(|r| covers(r, a, b)) {
                marks |= Marks::SELECTED.0;
            }
            if opts.diagnostics.iter().any(|r| covers(r, a, b)) {
                marks |= Marks::DIAGNOSTIC.0;
            }
            out.push(Span {
                range: a..b,
                text: &text[a..b],
                style: run.map_or(Style::PLAIN, |r| r.style),
                stream: run.map_or(Stream::Stdout, |r| r.stream),
                link: opts.links.iter().position(|l| covers(&l.range, a, b)),
                marks: Marks(marks),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Color;

    fn view(text: &str, runs: Vec<Run>) -> LineView<'static> {
        LineView {
            id: LineId(1),
            line: Cow::Owned(Line {
                text: text.into(),
                runs: Runs::Many(runs.into()),
                level: Level::Normal,
                continued: false,
            }),
            is_open: false,
        }
    }

    fn run(len: u32, fg: Color, stream: Stream) -> Run {
        Run {
            len,
            style: Style {
                fg,
                ..Style::PLAIN
            },
            stream,
        }
    }

    #[test]
    fn spans_split_on_every_boundary() {
        let v = view(
            "ab cd ef",
            vec![run(3, Color::Default, Stream::Stdout), run(5, Color::Ansi(1), Stream::Stderr)],
        );
        let links = [Link {
            range: 3..5,
            target: crate::links::LinkTarget::Url("u".into()),
        }];
        let search = 4..7;
        let current = 4..7;
        let sel = 1..4;
        let spans = v.spans(&SpanOptions {
            links: &links,
            search: std::slice::from_ref(&search),
            current_search: Some(&current),
            selection: Some(&sel),
            diagnostics: &[],
        });
        let got: Vec<(&str, Option<usize>, u8, Stream)> =
            spans.iter().map(|s| (s.text, s.link, s.marks.0, s.stream)).collect();
        assert_eq!(
            got,
            vec![
                ("a", None, 0, Stream::Stdout),
                ("b ", None, 4, Stream::Stdout),
                ("c", Some(0), 4, Stream::Stderr),
                ("d", Some(0), 3, Stream::Stderr),
                (" e", None, 3, Stream::Stderr),
                ("f", None, 0, Stream::Stderr),
            ]
        );
        // Spans tile the line.
        assert_eq!(spans.iter().map(|s| s.text).collect::<String>(), "ab cd ef");
    }

    #[test]
    fn styled_runs_and_stream_at() {
        let v = view(
            "ab cd",
            vec![run(2, Color::Default, Stream::Stdout), run(3, Color::Ansi(1), Stream::Stderr)],
        );
        let r: Vec<_> = v.styled_runs().map(|r| (r.text, r.range())).collect();
        assert_eq!(r, vec![("ab", 0..2), (" cd", 2..5)]);
        assert_eq!(v.stream_at(1), Stream::Stdout);
        assert_eq!(v.stream_at(2), Stream::Stderr);
        assert_eq!(v.stream_at(99), Stream::Stderr);
    }

    #[test]
    fn empty_line_has_no_spans() {
        let v = view("", vec![]);
        assert!(v.spans(&SpanOptions::default()).is_empty());
    }
}
