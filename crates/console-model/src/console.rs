//! The bounded, stream-tagged line store ([`Console`]).

use std::borrow::Cow;
use std::collections::VecDeque;

use crate::ansi::{AnsiEvent, AnsiParser, EraseMode};
use crate::filter::{CompiledFilter, Filter, FilterError};
use crate::line::{Line, LineId, LineView, Run, Runs};
use crate::links::classify_level;
use crate::style::Style;
use crate::tag::{Level, Stream};

/// Limits and policies of a [`Console`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleConfig {
    /// Most lines kept; older ones are evicted (default 200 000).
    pub max_lines: usize,
    /// Most text bytes kept (default 32 MiB).
    pub max_bytes: usize,
    /// Longest line in characters; longer ones are soft-wrapped into
    /// continuation lines (default 8192).
    pub max_line_chars: usize,
    /// Tab stop width (default 8).
    pub tab_width: usize,
    /// Classify lines as error/warning/notice from their text at commit
    /// (default true).
    pub classify_levels: bool,
    /// Start a new line when the output switches between stdout, stderr,
    /// debugger and system text (default false: they share the line like on a
    /// terminal; stdout and typed input always share).
    pub break_on_stream_change: bool,
    /// Bytes per run after which [`GuardEvent::LargeOutput`] fires (default 8 MiB).
    pub warn_after_bytes: Option<u64>,
    /// Bytes per run after which [`GuardEvent::ExcessiveOutput`] fires (default 256 MiB).
    pub stop_after_bytes: Option<u64>,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        ConsoleConfig {
            max_lines: 200_000,
            max_bytes: 32 << 20,
            max_line_chars: 8192,
            tab_width: 8,
            classify_levels: true,
            break_on_stream_change: false,
            warn_after_bytes: Some(8 << 20),
            stop_after_bytes: Some(256 << 20),
        }
    }
}

/// The large-output guard fired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuardEvent {
    /// A lot of output: ask "Large output detected. Stop the run?". If the user
    /// continues, call [`Console::continue_after_warning`].
    LargeOutput {
        /// Bytes received this run.
        bytes: u64,
    },
    /// Way too much output: stop the run ("Run stopped due to excessively
    /// large output.").
    ExcessiveOutput {
        /// Bytes received this run.
        bytes: u64,
    },
}

/// What one [`Console::append`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Change {
    /// Lines committed (ended by a newline or a soft wrap).
    pub lines_added: usize,
    /// Lines evicted from the front by the limits.
    pub lines_evicted: usize,
    /// Whether anything visible in the store changed (a new revision).
    pub changed: bool,
    /// The guard, if it fired.
    pub guard: Option<GuardEvent>,
}

/// How much was dropped from the front by the limits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Truncated {
    /// Lines dropped since the last clear.
    pub lines: u64,
    /// Text bytes dropped since the last clear.
    pub bytes: u64,
}

impl Truncated {
    /// The marker text to show above the first line.
    pub fn message(&self) -> String {
        format!("... {} earlier lines not displayed ({} KB) ...", self.lines, self.bytes / 1024)
    }
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
    stream: Stream,
}

struct Open {
    cells: Vec<Cell>,
    cursor: usize,
    continued: bool,
    last_stream: Stream,
}

struct Guard {
    warn_next: Option<u64>,
    last_warn: u64,
    stop_sent: bool,
}

/// Everything but the parsers (so parser callbacks can borrow it mutably).
struct Store {
    cfg: ConsoleConfig,
    lines: VecDeque<Line>,
    bytes: usize,
    first_id: u64,
    open: Open,
    evicted_lines: u64,
    evicted_bytes: u64,
    revision: u64,
    filter_spec: Filter,
    filter: CompiledFilter,
    visible: VecDeque<u64>,
    open_visible: bool,
    echo: VecDeque<char>,
    styles: [Style; 5],
    // per-append counters
    added: usize,
    evicted: usize,
    dirty: bool,
}

/// The console model: feed it bytes with [`Console::append`], read lines back
/// by index (all lines, or only those passing the filter).
///
/// Indices are positions in the *current* list and shift when lines are
/// evicted from the front; [`LineId`]s are stable. The last line is "open"
/// while the program has not ended it (prompt, progress bar) and is only
/// counted when non-empty.
pub struct Console {
    parsers: [AnsiParser; 5],
    s: Store,
    run_bytes: u64,
    guard: Guard,
}

impl Default for Console {
    fn default() -> Self {
        Console::new(ConsoleConfig::default())
    }
}

impl Console {
    /// An empty console.
    pub fn new(mut cfg: ConsoleConfig) -> Console {
        cfg.max_lines = cfg.max_lines.max(1);
        cfg.max_line_chars = cfg.max_line_chars.max(16);
        cfg.tab_width = cfg.tab_width.clamp(1, 32);
        let warn = cfg.warn_after_bytes;
        Console {
            parsers: Default::default(),
            run_bytes: 0,
            guard: Guard {
                warn_next: warn,
                last_warn: warn.unwrap_or(0),
                stop_sent: false,
            },
            s: Store {
                cfg,
                lines: VecDeque::new(),
                bytes: 0,
                first_id: 0,
                open: Open {
                    cells: Vec::new(),
                    cursor: 0,
                    continued: false,
                    last_stream: Stream::Stdout,
                },
                evicted_lines: 0,
                evicted_bytes: 0,
                revision: 0,
                filter_spec: Filter::all(),
                filter: CompiledFilter::all(),
                visible: VecDeque::new(),
                open_visible: false,
                echo: VecDeque::new(),
                styles: [Style::PLAIN; 5],
                added: 0,
                evicted: 0,
                dirty: false,
            },
        }
    }

    /// The configuration.
    pub fn config(&self) -> &ConsoleConfig {
        &self.s.cfg
    }

    // ----- writing -------------------------------------------------------

    /// Appends output bytes from `stream`. Chunks may split anywhere.
    pub fn append(&mut self, stream: Stream, bytes: &[u8]) -> Change {
        self.s.added = 0;
        self.s.evicted = 0;
        self.s.dirty = false;
        let Console { parsers, s, .. } = self;
        parsers[stream as usize].feed(bytes, &mut |ev| s.event(stream, ev));
        s.after_append();
        self.run_bytes += bytes.len() as u64;
        let guard = self.check_guard();
        let s = &mut self.s;
        if s.dirty {
            s.revision += 1;
        }
        Change {
            lines_added: s.added,
            lines_evicted: s.evicted,
            changed: s.dirty,
            guard,
        }
    }

    /// [`Console::append`] for text.
    pub fn append_str(&mut self, stream: Stream, text: &str) -> Change {
        self.append(stream, text.as_bytes())
    }

    /// Writes a whole message line: breaks the current open line first and
    /// ends the message with a newline (run status, exit status, notices).
    pub fn write_line(&mut self, stream: Stream, text: &str) -> Change {
        let mut change = self.line_break(stream);
        let c = self.append(stream, format!("{text}\n").as_bytes());
        change.lines_added += c.lines_added;
        change.lines_evicted += c.lines_evicted;
        change.changed |= c.changed;
        change.guard = change.guard.or(c.guard);
        change
    }

    /// Ends the open line if it has text.
    pub fn line_break(&mut self, stream: Stream) -> Change {
        self.s.added = 0;
        self.s.evicted = 0;
        self.s.dirty = false;
        if !self.s.open.cells.is_empty() {
            self.s.commit(stream);
        }
        self.s.after_append();
        if self.s.dirty {
            self.s.revision += 1;
        }
        Change {
            lines_added: self.s.added,
            lines_evicted: self.s.evicted,
            changed: self.s.dirty,
            guard: None,
        }
    }

    /// The end of a run: flushes partial UTF-8 characters and escape
    /// sequences of every stream and ends the open line.
    pub fn finish_streams(&mut self) -> Change {
        self.s.added = 0;
        self.s.evicted = 0;
        self.s.dirty = false;
        let Console { parsers, s, .. } = self;
        for stream in Stream::ALL {
            parsers[stream as usize].finish(&mut |ev| s.event(stream, ev));
        }
        if !s.open.cells.is_empty() {
            s.commit(Stream::Stdout);
        }
        s.echo.clear();
        s.after_append();
        if s.dirty {
            s.revision += 1;
        }
        Change {
            lines_added: s.added,
            lines_evicted: s.evicted,
            changed: s.dirty,
            guard: None,
        }
    }

    /// Tells the console what was just sent to the program, so that the echo
    /// the pty sends back is tagged [`Stream::Input`] instead of stdout.
    /// Control characters other than newline are ignored.
    pub fn expect_echo(&mut self, sent: &str) {
        for c in sent.chars() {
            let c = match c {
                '\r' => continue,
                '\n' => '\n',
                c if c.is_control() => continue,
                c => c,
            };
            if self.s.echo.len() < 4096 {
                self.s.echo.push_back(c);
            }
        }
    }

    /// Empties the console. Line ids keep increasing; parser state is reset.
    pub fn clear(&mut self) {
        let next = self.s.first_id + self.s.lines.len() as u64 + u64::from(!self.s.open.cells.is_empty());
        self.s.lines.clear();
        self.s.bytes = 0;
        self.s.first_id = next;
        self.s.open = Open {
            cells: Vec::new(),
            cursor: 0,
            continued: false,
            last_stream: Stream::Stdout,
        };
        self.s.evicted_lines = 0;
        self.s.evicted_bytes = 0;
        self.s.visible.clear();
        self.s.open_visible = false;
        self.s.echo.clear();
        self.s.styles = [Style::PLAIN; 5];
        for p in &mut self.parsers {
            p.reset();
        }
        self.s.revision += 1;
    }

    // ----- reading -------------------------------------------------------

    /// Counter bumped by every change; bind UI updates to it.
    pub fn revision(&self) -> u64 {
        self.s.revision
    }

    /// The id the next committed line (and so the open line) has.
    pub fn open_id(&self) -> LineId {
        LineId(self.s.first_id + self.s.lines.len() as u64)
    }

    /// The id of the oldest retained line.
    pub fn first_id(&self) -> LineId {
        LineId(self.s.first_id)
    }

    /// Lines retained, including the open one when it has text.
    pub fn line_count(&self) -> usize {
        self.s.lines.len() + usize::from(!self.s.open.cells.is_empty())
    }

    /// Line `index` of all retained lines (ignores the filter).
    pub fn line(&self, index: usize) -> Option<LineView<'_>> {
        if index < self.s.lines.len() {
            Some(LineView {
                id: LineId(self.s.first_id + index as u64),
                line: Cow::Borrowed(&self.s.lines[index]),
                is_open: false,
            })
        } else if index == self.s.lines.len() && !self.s.open.cells.is_empty() {
            Some(self.open_view())
        } else {
            None
        }
    }

    /// The line with `id`, if retained.
    pub fn line_by_id(&self, id: LineId) -> Option<LineView<'_>> {
        let idx = id.0.checked_sub(self.s.first_id)?;
        self.line(usize::try_from(idx).ok()?)
    }

    fn open_view(&self) -> LineView<'_> {
        LineView {
            id: self.open_id(),
            line: Cow::Owned(self.s.build_line(self.s.open.last_stream)),
            is_open: true,
        }
    }

    /// Lines that pass the filter.
    pub fn visible_count(&self) -> usize {
        if self.s.filter.identity {
            self.line_count()
        } else {
            self.s.visible.len() + usize::from(self.s.open_visible)
        }
    }

    /// The id of visible line `index`.
    pub fn visible_id(&self, index: usize) -> Option<LineId> {
        if self.s.filter.identity {
            (index < self.line_count()).then(|| LineId(self.s.first_id + index as u64))
        } else if index < self.s.visible.len() {
            Some(LineId(self.s.visible[index]))
        } else if index == self.s.visible.len() && self.s.open_visible {
            Some(self.open_id())
        } else {
            None
        }
    }

    /// Visible line `index`.
    pub fn visible_line(&self, index: usize) -> Option<LineView<'_>> {
        let id = self.visible_id(index)?;
        self.line_by_id(id)
    }

    /// The visible index of line `id`, if it is retained and passes the filter.
    pub fn visible_index_of(&self, id: LineId) -> Option<usize> {
        if self.s.filter.identity {
            let i = id.0.checked_sub(self.s.first_id)? as usize;
            (i < self.line_count()).then_some(i)
        } else if let Ok(i) = self.s.visible.binary_search(&id.0) {
            Some(i)
        } else if self.s.open_visible && id == self.open_id() {
            Some(self.s.visible.len())
        } else {
            None
        }
    }

    /// The number of visible lines with an id below `id` (the index of the
    /// first visible line at or after `id`).
    pub fn visible_lower_bound(&self, id: LineId) -> usize {
        if self.s.filter.identity {
            (id.0.saturating_sub(self.s.first_id) as usize).min(self.line_count())
        } else {
            let lb = self.s.visible.partition_point(|&x| x < id.0);
            if lb == self.s.visible.len() && self.s.open_visible && id > self.open_id() {
                lb + 1
            } else {
                lb
            }
        }
    }

    /// What the limits dropped from the front, if anything.
    pub fn truncated(&self) -> Option<Truncated> {
        (self.s.evicted_lines > 0).then_some(Truncated {
            lines: self.s.evicted_lines,
            bytes: self.s.evicted_bytes,
        })
    }

    /// Retained text bytes (committed lines).
    pub fn byte_count(&self) -> usize {
        self.s.bytes
    }

    // ----- levels and filter --------------------------------------------

    /// Overrides the severity of a retained line (for example from parsed
    /// diagnostics). Returns whether the line exists.
    pub fn set_level(&mut self, id: LineId, level: Level) -> bool {
        let Some(idx) = id.0.checked_sub(self.s.first_id).map(|i| i as usize) else {
            return false;
        };
        let Some(line) = self.s.lines.get_mut(idx) else {
            return false;
        };
        line.level = level;
        self.s.revision += 1;
        if !self.s.filter.identity {
            self.s.rebuild_visible();
        }
        true
    }

    /// Installs a filter; on error the old filter stays.
    pub fn set_filter(&mut self, filter: Filter) -> Result<(), FilterError> {
        let compiled = filter.compile()?;
        self.s.filter = compiled;
        self.s.filter_spec = filter;
        self.s.rebuild_visible();
        self.s.revision += 1;
        Ok(())
    }

    /// The installed filter.
    pub fn filter(&self) -> &Filter {
        &self.s.filter_spec
    }

    // ----- guard ---------------------------------------------------------

    /// Starts counting output for a new run (re-arms the guard).
    pub fn reset_run_counters(&mut self) {
        self.run_bytes = 0;
        let warn = self.s.cfg.warn_after_bytes;
        self.guard = Guard {
            warn_next: warn,
            last_warn: warn.unwrap_or(0),
            stop_sent: false,
        };
    }

    /// The user chose "Continue Run" after [`GuardEvent::LargeOutput`]: the next
    /// warning comes at four times the output.
    pub fn continue_after_warning(&mut self) {
        self.guard.warn_next = Some(self.guard.last_warn.saturating_mul(4).max(1));
    }

    fn check_guard(&mut self) -> Option<GuardEvent> {
        let bytes = self.run_bytes;
        if let Some(stop) = self.s.cfg.stop_after_bytes && bytes > stop && !self.guard.stop_sent {
            self.guard.stop_sent = true;
            return Some(GuardEvent::ExcessiveOutput { bytes });
        }
        if let Some(w) = self.guard.warn_next && bytes > w {
            self.guard.last_warn = w;
            self.guard.warn_next = None;
            return Some(GuardEvent::LargeOutput { bytes });
        }
        None
    }
}

impl Store {
    fn event(&mut self, stream: Stream, ev: AnsiEvent<'_>) {
        match ev {
            AnsiEvent::Text(t) => {
                let style = self.styles[stream as usize];
                for ch in t.chars() {
                    self.put_char(stream, style, ch);
                }
            }
            AnsiEvent::LineFeed => {
                if self.echo.front() == Some(&'\n') {
                    self.echo.pop_front();
                } else {
                    self.echo.clear();
                }
                self.commit(stream);
            }
            AnsiEvent::CarriageReturn => {
                self.open.cursor = 0;
                self.dirty = true;
            }
            AnsiEvent::Backspace => {
                self.echo.clear();
                self.open.cursor = self.open.cursor.saturating_sub(1);
            }
            AnsiEvent::Tab => {
                self.echo.clear();
                let tab = self.cfg.tab_width;
                let n = tab - self.open.cursor % tab;
                let style = self.styles[stream as usize];
                for _ in 0..n {
                    self.put_char(stream, style, ' ');
                }
            }
            AnsiEvent::StyleChanged(s) => self.styles[stream as usize] = s,
            AnsiEvent::EraseInLine(mode) => {
                let o = &mut self.open;
                match mode {
                    EraseMode::ToEnd => o.cells.truncate(o.cursor),
                    EraseMode::ToStart => {
                        let end = (o.cursor + 1).min(o.cells.len());
                        for c in &mut o.cells[..end] {
                            c.ch = ' ';
                        }
                    }
                    EraseMode::All => o.cells.clear(),
                }
                self.dirty = true;
            }
            AnsiEvent::CursorForward(n) => {
                self.open.cursor = (self.open.cursor + n as usize).min(self.cfg.max_line_chars - 1);
            }
            AnsiEvent::CursorBack(n) => {
                self.open.cursor = self.open.cursor.saturating_sub(n as usize);
            }
            AnsiEvent::CursorColumn(c) => {
                self.open.cursor = (c as usize).min(self.cfg.max_line_chars - 1);
            }
        }
    }

    fn put_char(&mut self, stream: Stream, style: Style, ch: char) {
        let mut stream = stream;
        if stream == Stream::Stdout && !self.echo.is_empty() {
            if self.echo.front() == Some(&ch) {
                self.echo.pop_front();
                stream = Stream::Input;
            } else {
                self.echo.clear();
            }
        }
        if self.cfg.break_on_stream_change
            && !self.open.cells.is_empty()
            && self.open.last_stream.class() != stream.class()
        {
            self.commit(self.open.last_stream);
        }
        if self.open.cursor >= self.cfg.max_line_chars {
            self.commit_wrapped(stream);
        }
        let o = &mut self.open;
        while o.cells.len() < o.cursor {
            o.cells.push(Cell {
                ch: ' ',
                style: Style::PLAIN,
                stream,
            });
        }
        let cell = Cell { ch, style, stream };
        if o.cursor == o.cells.len() {
            o.cells.push(cell);
        } else {
            o.cells[o.cursor] = cell;
        }
        o.cursor += 1;
        o.last_stream = stream;
        self.dirty = true;
    }

    /// Builds a line from the open cells (not consuming them).
    fn build_line(&self, empty_stream: Stream) -> Line {
        let cells = &self.open.cells;
        let mut text = String::with_capacity(cells.len());
        let mut runs: Vec<Run> = Vec::new();
        for c in cells {
            text.push(c.ch);
            let l = c.ch.len_utf8() as u32;
            match runs.last_mut() {
                Some(r) if r.style == c.style && r.stream == c.stream => r.len += l,
                _ => runs.push(Run {
                    len: l,
                    style: c.style,
                    stream: c.stream,
                }),
            }
        }
        let level = if self.cfg.classify_levels && !text.is_empty() {
            classify_level(&text)
        } else {
            Level::Normal
        };
        let runs = match runs.len() {
            0 => Runs::One(Run {
                len: 0,
                style: Style::PLAIN,
                stream: empty_stream,
            }),
            1 => Runs::One(runs[0]),
            _ => Runs::Many(runs.into_boxed_slice()),
        };
        Line {
            text: text.into_boxed_str(),
            runs,
            level,
            continued: self.open.continued,
        }
    }

    fn commit_wrapped(&mut self, stream: Stream) {
        self.commit_inner(stream, true);
    }

    fn commit(&mut self, stream: Stream) {
        self.commit_inner(stream, false);
    }

    fn commit_inner(&mut self, stream: Stream, next_continued: bool) {
        let line = self.build_line(stream);
        let id = self.first_id + self.lines.len() as u64;
        self.bytes += line.text.len();
        if !self.filter.identity && self.filter.passes(line.mask(), &line.text) {
            self.visible.push_back(id);
        }
        self.lines.push_back(line);
        self.added += 1;
        self.open.cells.clear();
        self.open.cursor = 0;
        self.open.continued = next_continued;
        self.open_visible = false;
        self.dirty = true;
        self.evict();
    }

    fn evict(&mut self) {
        let mut any = false;
        while self.lines.len() > self.cfg.max_lines || (self.bytes > self.cfg.max_bytes && !self.lines.is_empty()) {
            let Some(l) = self.lines.pop_front() else { break };
            self.bytes -= l.text.len();
            self.evicted_lines += 1;
            self.evicted_bytes += l.text.len() as u64 + 1;
            self.first_id += 1;
            self.evicted += 1;
            any = true;
        }
        if any && !self.filter.identity {
            while self.visible.front().is_some_and(|&id| id < self.first_id) {
                self.visible.pop_front();
            }
        }
    }

    /// Recomputes whether the open line is visible (filtered mode only).
    fn after_append(&mut self) {
        if self.filter.identity {
            return;
        }
        if self.open.cells.is_empty() {
            self.open_visible = false;
        } else {
            let line = self.build_line(self.open.last_stream);
            self.open_visible = self.filter.passes(line.mask(), &line.text);
        }
    }

    fn rebuild_visible(&mut self) {
        self.visible.clear();
        if self.filter.identity {
            self.open_visible = false;
            return;
        }
        for (i, l) in self.lines.iter().enumerate() {
            if self.filter.passes(l.mask(), &l.text) {
                self.visible.push_back(self.first_id + i as u64);
            }
        }
        self.after_append();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::OutputFilter;
    use crate::query::SearchQuery;

    fn texts(c: &Console) -> Vec<String> {
        (0..c.line_count()).map(|i| c.line(i).unwrap().text().to_owned()).collect()
    }

    fn vtexts(c: &Console) -> Vec<String> {
        (0..c.visible_count()).map(|i| c.visible_line(i).unwrap().text().to_owned()).collect()
    }

    #[test]
    fn lines_and_open_line() {
        let mut c = Console::default();
        let ch = c.append_str(Stream::Stdout, "a\nb\nprompt> ");
        assert_eq!(ch.lines_added, 2);
        assert_eq!(texts(&c), ["a", "b", "prompt> "]);
        assert!(c.line(2).unwrap().is_open());
        assert!(!c.line(1).unwrap().is_open());
        c.append_str(Stream::Stdout, "x\n");
        assert_eq!(texts(&c), ["a", "b", "prompt> x"]);
        assert!(!c.line(2).unwrap().is_open());
        // Trailing newline leaves no empty open line.
        assert_eq!(c.line_count(), 3);
        c.append_str(Stream::Stdout, "\n");
        assert_eq!(texts(&c), ["a", "b", "prompt> x", ""]);
    }

    #[test]
    fn ids_are_stable_across_commit_and_eviction() {
        let mut c = Console::new(ConsoleConfig {
            max_lines: 3,
            ..Default::default()
        });
        c.append_str(Stream::Stdout, "partial");
        let id = c.line(0).unwrap().id();
        c.append_str(Stream::Stdout, " done\nl2\nl3\nl4\n");
        // "partial done" got evicted, l2 l3 l4 remain.
        assert_eq!(texts(&c), ["l2", "l3", "l4"]);
        assert!(c.line_by_id(id).is_none());
        assert_eq!(c.first_id(), LineId(1));
        assert_eq!(c.truncated().unwrap().lines, 1);
        assert_eq!(c.line(0).unwrap().id(), LineId(1));
    }

    #[test]
    fn carriage_return_overwrites() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "10%\r50%\r100%\n");
        assert_eq!(texts(&c), ["100%"]);
        c.append_str(Stream::Stdout, "abcdef\rXY\n");
        assert_eq!(texts(&c)[1], "XYcdef");
        // CRLF keeps the text.
        c.append_str(Stream::Stdout, "win\r\nnext\r\n");
        assert_eq!(texts(&c)[2..], ["win", "next"]);
    }

    #[test]
    fn erase_in_line_with_cr() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "long text here\r\x1b[Kshort\n");
        assert_eq!(texts(&c), ["short"]);
        c.append_str(Stream::Stdout, "abc\x1b[2Kz\n");
        assert_eq!(texts(&c)[1], "   z");
    }

    #[test]
    fn backspace_overstrike_and_tabs() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "ab\x08c\n");
        assert_eq!(texts(&c), ["ac"]);
        c.append_str(Stream::Stdout, "a\tb\n");
        assert_eq!(texts(&c)[1], "a       b");
    }

    #[test]
    fn styles_become_runs() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "ab\x1b[31mcd\x1b[0mef\n");
        let l = c.line(0).unwrap();
        let runs: Vec<_> = l.styled_runs().map(|r| (r.text.to_owned(), r.style.fg)).collect();
        assert_eq!(
            runs,
            vec![
                ("ab".to_owned(), crate::style::Color::Default),
                ("cd".to_owned(), crate::style::Color::Ansi(1)),
                ("ef".to_owned(), crate::style::Color::Default)
            ]
        );
    }

    #[test]
    fn style_state_is_per_stream() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "\x1b[31m");
        c.append_str(Stream::Stderr, "e\n");
        c.append_str(Stream::Stdout, "o\n");
        assert!(c.line(0).unwrap().runs()[0].style.is_plain());
        assert_eq!(c.line(1).unwrap().runs()[0].style.fg, crate::style::Color::Ansi(1));
    }

    #[test]
    fn stream_tags_and_mixed_lines() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "out ");
        c.append_str(Stream::Stderr, "err\n");
        let l = c.line(0).unwrap();
        assert_eq!(l.text(), "out err");
        assert_eq!(l.stream_at(0), Stream::Stdout);
        assert_eq!(l.stream_at(4), Stream::Stderr);
        let mut c2 = Console::new(ConsoleConfig {
            break_on_stream_change: true,
            ..Default::default()
        });
        c2.append_str(Stream::Stdout, "out ");
        c2.append_str(Stream::Stderr, "err\n");
        assert_eq!(texts(&c2), ["out ", "err"]);
    }

    #[test]
    fn empty_lines_keep_their_stream() {
        let mut c = Console::default();
        c.append_str(Stream::Stderr, "\n");
        assert_eq!(c.line(0).unwrap().runs()[0].stream, Stream::Stderr);
        assert!(c.line(0).unwrap().mask().has_stream(Stream::Stderr));
    }

    #[test]
    fn echo_is_tagged_input() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "Name? ");
        c.expect_echo("bob\n");
        c.append_str(Stream::Stdout, "bo");
        c.append_str(Stream::Stdout, "b\r\nHello bob\r\n");
        let l = c.line(0).unwrap();
        assert_eq!(l.text(), "Name? bob");
        assert_eq!(l.stream_at(0), Stream::Stdout);
        assert_eq!(l.stream_at(6), Stream::Input);
        assert_eq!(c.line(1).unwrap().stream_at(0), Stream::Stdout);
        // A mismatch cancels the expectation.
        c.expect_echo("secret\n");
        c.append_str(Stream::Stdout, "\r\nok\n");
        assert_eq!(c.line(3).unwrap().stream_at(0), Stream::Stdout);
    }

    #[test]
    fn long_lines_soft_wrap() {
        let mut c = Console::new(ConsoleConfig {
            max_line_chars: 16,
            ..Default::default()
        });
        c.append_str(Stream::Stdout, &"x".repeat(40));
        c.append_str(Stream::Stdout, "\n");
        assert_eq!(texts(&c).iter().map(String::len).collect::<Vec<_>>(), [16, 16, 8]);
        assert!(!c.line(0).unwrap().is_continued());
        assert!(c.line(1).unwrap().is_continued());
        assert!(c.line(2).unwrap().is_continued());
        c.append_str(Stream::Stdout, "y\n");
        assert!(!c.line(3).unwrap().is_continued());
    }

    #[test]
    fn byte_limit_evicts() {
        let mut c = Console::new(ConsoleConfig {
            max_bytes: 20,
            ..Default::default()
        });
        for i in 0..10 {
            c.append_str(Stream::Stdout, &format!("line{i}\n"));
        }
        assert!(c.byte_count() <= 20);
        assert_eq!(texts(&c).last().unwrap(), "line9");
        assert!(c.truncated().unwrap().lines >= 6);
    }

    #[test]
    fn level_classification() {
        let mut c = Console::default();
        c.append_str(Stream::Stderr, "a.c:1:2: error: boom\nwarning: x\nfine\n");
        assert_eq!(c.line(0).unwrap().level(), Level::Error);
        assert_eq!(c.line(1).unwrap().level(), Level::Warning);
        assert_eq!(c.line(2).unwrap().level(), Level::Normal);
        assert!(c.set_level(LineId(2), Level::Notice));
        assert_eq!(c.line(2).unwrap().level(), Level::Notice);
        assert!(!c.set_level(LineId(99), Level::Notice));
    }

    #[test]
    fn filter_by_stream_with_stable_ids() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "o1\n");
        c.append_str(Stream::Stderr, "e1\n");
        c.append_str(Stream::Stdout, "o2\n");
        c.append_str(Stream::Stderr, "e2\n");
        c.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
        assert_eq!(vtexts(&c), ["e1", "e2"]);
        assert_eq!(c.visible_id(1), Some(LineId(3)));
        assert_eq!(c.visible_index_of(LineId(3)), Some(1));
        assert_eq!(c.visible_index_of(LineId(2)), None);
        assert_eq!(c.visible_lower_bound(LineId(2)), 1);
        // Incremental maintenance.
        c.append_str(Stream::Stdout, "o3\n");
        c.append_str(Stream::Stderr, "e3");
        assert_eq!(vtexts(&c), ["e1", "e2", "e3"]);
        assert!(c.visible_line(2).unwrap().is_open());
        c.append_str(Stream::Stderr, "\n");
        assert_eq!(vtexts(&c), ["e1", "e2", "e3"]);
        c.set_filter(Filter::all()).unwrap();
        assert_eq!(c.visible_count(), c.line_count());
    }

    #[test]
    fn filter_text_and_eviction() {
        let mut c = Console::new(ConsoleConfig {
            max_lines: 4,
            ..Default::default()
        });
        c.set_filter(Filter::all().with_text(SearchQuery::plain("x"))).unwrap();
        for i in 0..10 {
            c.append_str(Stream::Stdout, &format!("{}{i}\n", if i % 2 == 0 { "x" } else { "y" }));
        }
        assert_eq!(vtexts(&c), ["x6", "x8"]);
        // The open line is visible only once it matches.
        c.append_str(Stream::Stdout, "ab");
        assert_eq!(c.visible_count(), 2);
        c.append_str(Stream::Stdout, "x");
        assert_eq!(c.visible_count(), 3);
    }

    #[test]
    fn bad_filter_keeps_old() {
        let mut c = Console::default();
        c.set_filter(Filter::output(OutputFilter::Errors)).unwrap();
        assert!(c.set_filter(Filter::output(OutputFilter::Custom("(".into()))).is_err());
        assert_eq!(c.filter(), &Filter::output(OutputFilter::Errors));
    }

    #[test]
    fn clear_keeps_ids_growing() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "a\nb\nc");
        c.clear();
        assert_eq!(c.line_count(), 0);
        c.append_str(Stream::Stdout, "z\n");
        assert_eq!(c.line(0).unwrap().id(), LineId(3));
        assert!(c.truncated().is_none());
    }

    #[test]
    fn guard_fires_once_then_after_continue() {
        let mut c = Console::new(ConsoleConfig {
            warn_after_bytes: Some(100),
            stop_after_bytes: Some(1000),
            ..Default::default()
        });
        let chunk = vec![b'a'; 60];
        assert_eq!(c.append(Stream::Stdout, &chunk).guard, None);
        assert_eq!(
            c.append(Stream::Stdout, &chunk).guard,
            Some(GuardEvent::LargeOutput { bytes: 120 })
        );
        assert_eq!(c.append(Stream::Stdout, &chunk).guard, None);
        c.continue_after_warning();
        assert_eq!(c.append(Stream::Stdout, &chunk).guard, None);
        let big = vec![b'a'; 300];
        assert_eq!(
            c.append(Stream::Stdout, &big).guard,
            Some(GuardEvent::LargeOutput { bytes: 540 })
        );
        c.continue_after_warning();
        assert_eq!(
            c.append(Stream::Stdout, &vec![b'a'; 600]).guard,
            Some(GuardEvent::ExcessiveOutput { bytes: 1140 })
        );
        c.reset_run_counters();
        assert_eq!(c.append(Stream::Stdout, &chunk).guard, None);
    }

    #[test]
    fn finish_flushes() {
        let mut c = Console::default();
        c.append(Stream::Stdout, b"abc\xe2\x82");
        assert_eq!(texts(&c), ["abc"]);
        c.finish_streams();
        assert_eq!(texts(&c), ["abc\u{FFFD}"]);
        assert!(!c.line(0).unwrap().is_open());
    }

    #[test]
    fn write_line_breaks_open_line() {
        let mut c = Console::default();
        c.append_str(Stream::Stdout, "prompt");
        c.write_line(Stream::System, "Run Succeeded");
        assert_eq!(texts(&c), ["prompt", "Run Succeeded"]);
        assert_eq!(c.line(1).unwrap().stream_at(0), Stream::System);
    }

    #[test]
    fn revision_tracks_changes() {
        let mut c = Console::default();
        let r0 = c.revision();
        assert!(c.append_str(Stream::Stdout, "a").changed);
        assert!(c.revision() > r0);
        let r1 = c.revision();
        // An escape-only chunk changes nothing visible.
        assert!(!c.append_str(Stream::Stdout, "\x1b[?25l").changed);
        assert_eq!(c.revision(), r1);
    }
}
