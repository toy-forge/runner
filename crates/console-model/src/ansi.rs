//! A streaming ANSI/VT output parser.
//!
//! [`AnsiParser`] turns bytes into [`AnsiEvent`]s. It understands what a
//! console needs and nothing more:
//!
//! * SGR (`ESC [ ... m`): 16 / 256 / true colours (`38;5;n`, `38;2;r;g;b`, and
//!   the colon forms `38:2::r:g:b`), bold, faint, italic, underline, double
//!   underline, blink, inverse, hidden, strike and their resets.
//! * C0 controls that move the cursor: LF (also VT/FF), CR, BS, TAB.
//! * A few cursor/erase CSIs that progress bars use on the *current* line:
//!   `K` (erase in line), `G` (column), `C`/`D` (forward/back).
//! * Everything else is **stripped**: every other CSI (cursor keys, `?2004h`,
//!   `?1049h`, `?25l`, scrolling regions, ...), OSC (titles, hyperlinks), DCS /
//!   SOS / PM / APC strings, charset designations (`ESC ( B`), single-char ESC
//!   sequences, remaining C0 controls and C1 control characters.
//!
//! The parser is incremental: chunks may end anywhere, in the middle of an
//! escape sequence or of a multi-byte UTF-8 character, and the result is the
//! same as feeding the concatenation (this is property-tested). Invalid UTF-8
//! becomes U+FFFD, one per maximal invalid subpart. Runaway sequences are
//! bounded (CSI 128 bytes, strings 4096 bytes) so a stray ESC can never swallow
//! the rest of the output.

use crate::style::{Attrs, Color, Style};

/// How `ESC [ n K` erases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EraseMode {
    /// From the cursor to the end of the line (0, default).
    ToEnd,
    /// From the start of the line to the cursor (1).
    ToStart,
    /// The whole line (2).
    All,
}

/// What [`AnsiParser::feed`] reports.
#[derive(Clone, PartialEq, Debug)]
pub enum AnsiEvent<'a> {
    /// Printable text (no control characters). Written in the parser's
    /// current [`AnsiParser::style`].
    Text(&'a str),
    /// LF, VT or FF.
    LineFeed,
    /// CR.
    CarriageReturn,
    /// BS: cursor one column left.
    Backspace,
    /// TAB.
    Tab,
    /// An SGR sequence changed the style; carries the new style.
    StyleChanged(Style),
    /// `ESC [ n K`.
    EraseInLine(EraseMode),
    /// `ESC [ n C`.
    CursorForward(u32),
    /// `ESC [ n D`.
    CursorBack(u32),
    /// `ESC [ n G`, zero-based column.
    CursorColumn(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    Esc,
    EscInter,
    Csi,
    CsiIgnore,
    Osc,
    Str,
}

const MAX_CSI: usize = 128;
const MAX_STRING: usize = 4096;

/// The incremental parser. One per byte stream (stdout and stderr each have
/// their own: SGR state does not leak between them).
#[derive(Clone, Debug)]
pub struct AnsiParser {
    state: State,
    style: Style,
    csi: Vec<u8>,
    csi_inter: bool,
    str_len: usize,
    str_esc: bool,
    pending: [u8; 3],
    pending_len: u8,
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn is_ctl(b: u8) -> bool {
    b < 0x20 || b == 0x7f
}

impl AnsiParser {
    /// A parser in the ground state with the default style.
    pub fn new() -> Self {
        AnsiParser {
            state: State::Ground,
            style: Style::PLAIN,
            csi: Vec::new(),
            csi_inter: false,
            str_len: 0,
            str_esc: false,
            pending: [0; 3],
            pending_len: 0,
        }
    }

    /// The style text written now would have.
    pub fn style(&self) -> Style {
        self.style
    }

    /// Back to the ground state and the default style, dropping any partial
    /// sequence or partial character.
    pub fn reset(&mut self) {
        *self = AnsiParser::new();
    }

    /// Flushes what the end of the stream leaves over: an incomplete UTF-8
    /// character becomes U+FFFD and a partial escape sequence is dropped.
    /// The SGR style is kept.
    pub fn finish(&mut self, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        if self.pending_len > 0 {
            self.pending_len = 0;
            sink(AnsiEvent::Text("\u{FFFD}"));
        }
        self.state = State::Ground;
        self.csi.clear();
    }

    /// Feeds a chunk. Events borrow from the chunk or from scratch space and
    /// are valid only during the call to `sink`.
    pub fn feed(&mut self, bytes: &[u8], sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        let n = bytes.len();
        let mut i = 0;
        while i < n {
            if self.state == State::Ground {
                let start = i;
                while i < n && !is_ctl(bytes[i]) {
                    i += 1;
                }
                let ended_by_control = i < n;
                if i > start || self.pending_len > 0 {
                    self.text(&bytes[start..i], !ended_by_control, sink);
                }
                if i < n {
                    let b = bytes[i];
                    i += 1;
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        Self::exec_c0(b, sink);
                    }
                }
            } else {
                let b = bytes[i];
                i += 1;
                if self.sequence_byte(b, sink) {
                    i -= 1;
                }
            }
        }
    }

    fn exec_c0(b: u8, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        match b {
            b'\n' | 0x0b | 0x0c => sink(AnsiEvent::LineFeed),
            b'\r' => sink(AnsiEvent::CarriageReturn),
            0x08 => sink(AnsiEvent::Backspace),
            b'\t' => sink(AnsiEvent::Tab),
            _ => {}
        }
    }

    /// Text bytes of the ground state. `at_chunk_end`: the run ends because the
    /// chunk does (an incomplete character may be completed later).
    fn text(&mut self, run: &[u8], at_chunk_end: bool, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        if self.pending_len > 0 {
            let mut joined = Vec::with_capacity(self.pending_len as usize + run.len());
            joined.extend_from_slice(&self.pending[..self.pending_len as usize]);
            joined.extend_from_slice(run);
            self.pending_len = 0;
            self.decode(&joined, at_chunk_end, sink);
        } else {
            self.decode(run, at_chunk_end, sink);
        }
    }

    fn decode(&mut self, mut buf: &[u8], at_chunk_end: bool, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        loop {
            match std::str::from_utf8(buf) {
                Ok(s) => {
                    emit_str(s, sink);
                    return;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 && let Ok(s) = std::str::from_utf8(&buf[..valid]) {
                        emit_str(s, sink);
                    }
                    match e.error_len() {
                        Some(len) => {
                            sink(AnsiEvent::Text("\u{FFFD}"));
                            buf = &buf[valid + len..];
                        }
                        None => {
                            let rest = &buf[valid..];
                            if at_chunk_end && rest.len() <= 3 {
                                self.pending[..rest.len()].copy_from_slice(rest);
                                self.pending_len = rest.len() as u8;
                            } else {
                                sink(AnsiEvent::Text("\u{FFFD}"));
                            }
                            return;
                        }
                    }
                }
            }
        }
    }

    /// One byte in a non-ground state. Returns true when the byte must be
    /// processed again in the ground state.
    fn sequence_byte(&mut self, b: u8, sink: &mut dyn FnMut(AnsiEvent<'_>)) -> bool {
        match self.state {
            State::Ground => true,
            State::Esc => {
                match b {
                    0x18 | 0x1a => self.state = State::Ground,
                    0x1b => {}
                    0x00..=0x1f => Self::exec_c0(b, sink),
                    b'[' => {
                        self.csi.clear();
                        self.csi_inter = false;
                        self.state = State::Csi;
                    }
                    b']' => self.start_string(State::Osc),
                    b'P' | b'X' | b'^' | b'_' => self.start_string(State::Str),
                    0x20..=0x2f => self.state = State::EscInter,
                    0x30..=0x7e => self.state = State::Ground,
                    0x7f => {}
                    _ => {
                        self.state = State::Ground;
                        return true;
                    }
                }
                false
            }
            State::EscInter => {
                match b {
                    0x18 | 0x1a => self.state = State::Ground,
                    0x1b => self.state = State::Esc,
                    0x00..=0x1f => Self::exec_c0(b, sink),
                    0x20..=0x2f | 0x7f => {}
                    0x30..=0x7e => self.state = State::Ground,
                    _ => {
                        self.state = State::Ground;
                        return true;
                    }
                }
                false
            }
            State::Csi => {
                match b {
                    0x18 | 0x1a => self.state = State::Ground,
                    0x1b => self.state = State::Esc,
                    0x00..=0x1f => Self::exec_c0(b, sink),
                    0x20..=0x2f => {
                        self.csi_inter = true;
                        self.csi_push(b);
                    }
                    0x30..=0x3f => {
                        if self.csi_inter {
                            self.state = State::CsiIgnore;
                        } else {
                            self.csi_push(b);
                        }
                    }
                    0x40..=0x7e => {
                        self.dispatch_csi(b, sink);
                        self.state = State::Ground;
                    }
                    0x7f => {}
                    _ => {
                        self.state = State::Ground;
                        return true;
                    }
                }
                false
            }
            State::CsiIgnore => {
                match b {
                    0x18 | 0x1a => self.state = State::Ground,
                    0x1b => self.state = State::Esc,
                    0x00..=0x1f => Self::exec_c0(b, sink),
                    0x40..=0x7e => self.state = State::Ground,
                    0x20..=0x3f | 0x7f => {}
                    _ => {
                        self.state = State::Ground;
                        return true;
                    }
                }
                false
            }
            State::Osc | State::Str => self.string_byte(b),
        }
    }

    fn start_string(&mut self, state: State) {
        self.state = state;
        self.str_len = 0;
        self.str_esc = false;
    }

    fn csi_push(&mut self, b: u8) {
        if self.csi.len() >= MAX_CSI {
            self.state = State::CsiIgnore;
            self.csi.clear();
        } else {
            self.csi.push(b);
        }
    }

    /// Returns true to reprocess `b` in the ground state (never; strings
    /// restart through the ESC state instead, which is signalled by setting
    /// the state and returning true here only for that case).
    fn string_byte(&mut self, b: u8) -> bool {
        if self.str_esc {
            self.str_esc = false;
            if b == b'\\' {
                self.state = State::Ground;
                return false;
            }
            // ESC followed by something else: the string is abandoned and the
            // ESC starts a new sequence; `b` is its second byte.
            self.state = State::Esc;
            return true;
        }
        match b {
            0x1b => self.str_esc = true,
            0x18 | 0x1a => self.state = State::Ground,
            0x07 if self.state == State::Osc => self.state = State::Ground,
            _ => {
                self.str_len += 1;
                if self.str_len > MAX_STRING {
                    self.state = State::Ground;
                }
            }
        }
        false
    }

    fn dispatch_csi(&mut self, final_byte: u8, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
        if self.csi_inter || matches!(self.csi.first(), Some(b'<' | b'=' | b'>' | b'?')) {
            return;
        }
        match final_byte {
            b'm' => {
                let before = self.style;
                apply_sgr(&mut self.style, &self.csi);
                if self.style != before {
                    sink(AnsiEvent::StyleChanged(self.style));
                }
            }
            b'K' => {
                let mode = match first_param(&self.csi) {
                    Some(1) => EraseMode::ToStart,
                    Some(2) | Some(3) => EraseMode::All,
                    _ => EraseMode::ToEnd,
                };
                sink(AnsiEvent::EraseInLine(mode));
            }
            b'G' => {
                let col = first_param(&self.csi).unwrap_or(1).max(1) - 1;
                sink(AnsiEvent::CursorColumn(col));
            }
            b'C' => sink(AnsiEvent::CursorForward(first_param(&self.csi).unwrap_or(1).max(1))),
            b'D' => sink(AnsiEvent::CursorBack(first_param(&self.csi).unwrap_or(1).max(1))),
            _ => {}
        }
    }
}

fn emit_str(s: &str, sink: &mut dyn FnMut(AnsiEvent<'_>)) {
    if s.is_empty() {
        return;
    }
    if s.is_ascii() || !s.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)) {
        sink(AnsiEvent::Text(s));
    } else {
        // C1 control characters are dropped.
        let cleaned: String = s.chars().filter(|c| !('\u{80}'..='\u{9f}').contains(c)).collect();
        if !cleaned.is_empty() {
            sink(AnsiEvent::Text(&cleaned));
        }
    }
}

fn first_param(buf: &[u8]) -> Option<u32> {
    let mut value: Option<u32> = None;
    for &b in buf {
        match b {
            b'0'..=b'9' => value = Some((value.unwrap_or(0) * 10 + (b - b'0') as u32).min(65535)),
            _ => break,
        }
    }
    value
}

/// One SGR parameter: its value and whether a `:` (sub-parameter) follows.
type Param = (Option<u32>, bool);

fn apply_sgr(style: &mut Style, buf: &[u8]) {
    let mut params: [Param; 32] = [(None, false); 32];
    let mut count = 0;
    let mut cur: Option<u32> = None;
    for &b in buf {
        match b {
            b'0'..=b'9' => cur = Some((cur.unwrap_or(0) * 10 + (b - b'0') as u32).min(65535)),
            b';' | b':' => {
                if count < params.len() {
                    params[count] = (cur, b == b':');
                    count += 1;
                }
                cur = None;
            }
            _ => {}
        }
    }
    if count < params.len() {
        params[count] = (cur, false);
        count += 1;
    }
    let params = &params[..count];
    let mut i = 0;
    while i < params.len() {
        let code = params[i].0.unwrap_or(0);
        i += 1;
        match code {
            0 => *style = Style::PLAIN,
            1 => style.attrs.insert(Attrs::BOLD),
            2 => style.attrs.insert(Attrs::FAINT),
            3 => style.attrs.insert(Attrs::ITALIC),
            4 => {
                // 4:n selects the underline style.
                let mut sub = None;
                if params[i - 1].1 && i < params.len() {
                    sub = params[i].0;
                    i += 1;
                }
                match sub {
                    Some(0) => {
                        style.attrs.remove(Attrs::UNDERLINE);
                        style.attrs.remove(Attrs::DOUBLE_UNDERLINE);
                    }
                    Some(2) => {
                        style.attrs.remove(Attrs::UNDERLINE);
                        style.attrs.insert(Attrs::DOUBLE_UNDERLINE);
                    }
                    _ => {
                        style.attrs.remove(Attrs::DOUBLE_UNDERLINE);
                        style.attrs.insert(Attrs::UNDERLINE);
                    }
                }
            }
            5 | 6 => style.attrs.insert(Attrs::BLINK),
            7 => style.attrs.insert(Attrs::INVERSE),
            8 => style.attrs.insert(Attrs::HIDDEN),
            9 => style.attrs.insert(Attrs::STRIKE),
            21 => {
                style.attrs.remove(Attrs::UNDERLINE);
                style.attrs.insert(Attrs::DOUBLE_UNDERLINE);
            }
            22 => {
                style.attrs.remove(Attrs::BOLD);
                style.attrs.remove(Attrs::FAINT);
            }
            23 => style.attrs.remove(Attrs::ITALIC),
            24 => {
                style.attrs.remove(Attrs::UNDERLINE);
                style.attrs.remove(Attrs::DOUBLE_UNDERLINE);
            }
            25 => style.attrs.remove(Attrs::BLINK),
            27 => style.attrs.remove(Attrs::INVERSE),
            28 => style.attrs.remove(Attrs::HIDDEN),
            29 => style.attrs.remove(Attrs::STRIKE),
            30..=37 => style.fg = Color::Ansi((code - 30) as u8),
            39 => style.fg = Color::Default,
            40..=47 => style.bg = Color::Ansi((code - 40) as u8),
            49 => style.bg = Color::Default,
            90..=97 => style.fg = Color::Ansi((code - 90 + 8) as u8),
            100..=107 => style.bg = Color::Ansi((code - 100 + 8) as u8),
            38 | 48 | 58 => {
                let (color, next) = extended_color(params, i - 1);
                i = next;
                if let Some(c) = color {
                    match code {
                        38 => style.fg = c,
                        48 => style.bg = c,
                        _ => {} // 58: underline colour, ignored
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parses the arguments of `38`/`48`/`58` at `params[at]`; returns the colour
/// and the index of the next unconsumed parameter.
fn extended_color(params: &[Param], at: usize) -> (Option<Color>, usize) {
    let mut args = [0u32; 6];
    let mut n = 0;
    let mut j = at;
    if params[at].1 {
        // Colon form: all following sub-parameters belong to this code.
        while params[j].1 && j + 1 < params.len() {
            j += 1;
            if n < args.len() {
                args[n] = params[j].0.unwrap_or(0);
                n += 1;
            }
        }
        j += 1;
    } else {
        // Semicolon form: `5;n` or `2;r;g;b`.
        j = at + 1;
        if j < params.len() {
            let kind = params[j].0.unwrap_or(0);
            let want = match kind {
                5 => 2,
                2 => 4,
                _ => 1,
            };
            for _ in 0..want {
                if j < params.len() {
                    if n < args.len() {
                        args[n] = params[j].0.unwrap_or(0);
                        n += 1;
                    }
                    j += 1;
                }
            }
        }
    }
    let clamp = |v: u32| v.min(255) as u8;
    let color = match (n, args[0]) {
        (2.., 5) => Some(Color::from_index(clamp(args[1]))),
        (4.., 2) => {
            let (r, g, b) = (args[n - 3], args[n - 2], args[n - 1]);
            Some(Color::Rgb(clamp(r), clamp(g), clamp(b)))
        }
        _ => None,
    };
    (color, j)
}

/// The text of `input` with every escape sequence removed (LF kept, CR and
/// other controls dropped, no overwrite semantics). Handy for tests and for
/// copying raw output.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut parser = AnsiParser::new();
    let mut sink = |e: AnsiEvent<'_>| match e {
        AnsiEvent::Text(t) => out.push_str(t),
        AnsiEvent::LineFeed => out.push('\n'),
        AnsiEvent::Tab => out.push('\t'),
        _ => {}
    };
    parser.feed(input.as_bytes(), &mut sink);
    parser.finish(&mut sink);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds chunks and returns a normalised event list (texts coalesced).
    fn run(chunks: &[&[u8]]) -> Vec<String> {
        let mut p = AnsiParser::new();
        let mut out: Vec<String> = Vec::new();
        let mut sink = |e: AnsiEvent<'_>| match e {
            AnsiEvent::Text(t) => match out.last_mut() {
                Some(last) if last.starts_with("T:") => last.push_str(t),
                _ => out.push(format!("T:{t}")),
            },
            other => out.push(format!("{other:?}")),
        };
        for c in chunks {
            p.feed(c, &mut sink);
        }
        p.finish(&mut sink);
        out
    }

    #[test]
    fn plain_text_and_controls() {
        assert_eq!(run(&[b"ab\r\ncd\te\x08"]), vec!["T:ab", "CarriageReturn", "LineFeed", "T:cd", "Tab", "T:e", "Backspace"]);
    }

    #[test]
    fn sgr_16_256_truecolor() {
        let ev = run(&[b"\x1b[31;1mA\x1b[0m\x1b[38;5;200mB\x1b[48;2;1;2;3mC\x1b[92mD"]);
        assert!(ev.contains(&"T:A".to_string()));
        let mut p = AnsiParser::new();
        let mut last = Style::PLAIN;
        p.feed(b"\x1b[31;1m", &mut |e| {
            if let AnsiEvent::StyleChanged(s) = e {
                last = s
            }
        });
        assert_eq!(last.fg, Color::Ansi(1));
        assert!(last.bold());
        p.feed(b"\x1b[38;5;200;48;2;1;2;3m", &mut |e| {
            if let AnsiEvent::StyleChanged(s) = e {
                last = s
            }
        });
        assert_eq!(last.fg, Color::Indexed(200));
        assert_eq!(last.bg, Color::Rgb(1, 2, 3));
        p.feed(b"\x1b[92;4m", &mut |e| {
            if let AnsiEvent::StyleChanged(s) = e {
                last = s
            }
        });
        assert_eq!(last.fg, Color::Ansi(10));
        assert!(last.underline());
        p.feed(b"\x1b[m", &mut |e| {
            if let AnsiEvent::StyleChanged(s) = e {
                last = s
            }
        });
        assert!(last.is_plain());
    }

    #[test]
    fn colon_forms() {
        let mut p = AnsiParser::new();
        p.feed(b"\x1b[38:2::10:20:30m", &mut |_| {});
        assert_eq!(p.style().fg, Color::Rgb(10, 20, 30));
        p.feed(b"\x1b[48:2:4:5:6m", &mut |_| {});
        assert_eq!(p.style().bg, Color::Rgb(4, 5, 6));
        p.feed(b"\x1b[38:5:9m", &mut |_| {});
        assert_eq!(p.style().fg, Color::Ansi(9));
        p.feed(b"\x1b[4:2m", &mut |_| {});
        assert!(p.style().attrs.contains(Attrs::DOUBLE_UNDERLINE));
        p.feed(b"\x1b[4:0m", &mut |_| {});
        assert!(!p.style().underline());
        // 58 consumes its arguments without touching colours.
        p.feed(b"\x1b[58;5;1;1m", &mut |_| {});
        assert!(p.style().bold());
    }

    #[test]
    fn malformed_sgr_is_harmless() {
        let mut p = AnsiParser::new();
        p.feed(b"\x1b[38;5m\x1b[38;2;1m\x1b[38m\x1b[;;;m\x1b[999999999m", &mut |_| {});
        assert!(p.style().is_plain());
    }

    #[test]
    fn strips_other_sequences() {
        let ev = run(&[b"a\x1b[?2004hb\x1b[?1049l\x1b[?25lc\x1b]0;title\x07d\x1b]8;;http://x\x1b\\e\x1b(Bf\x1b=g\x1bPdcs\x1b\\h\x1b[2;3Hi\x1b[1Aj"]);
        assert_eq!(ev, vec!["T:abcdefghij"]);
    }

    #[test]
    fn private_m_is_not_sgr() {
        let mut p = AnsiParser::new();
        p.feed(b"\x1b[>4;2m", &mut |_| {});
        assert!(p.style().is_plain());
    }

    #[test]
    fn erase_and_cursor_events() {
        let ev = run(&[b"\r\x1b[K\x1b[2K\x1b[1K\x1b[5G\x1b[3C\x1b[D"]);
        assert_eq!(
            ev,
            vec![
                "CarriageReturn",
                "EraseInLine(ToEnd)",
                "EraseInLine(All)",
                "EraseInLine(ToStart)",
                "CursorColumn(4)",
                "CursorForward(3)",
                "CursorBack(1)"
            ]
        );
    }

    #[test]
    fn split_everywhere_equals_unsplit() {
        let input = "x\x1b[1;31mred\x1b[0m \u{4e2d}\u{6587}\x1b]0;t\x07\x1b[?25l end\u{1f600}\n";
        let whole = run(&[input.as_bytes()]);
        let bytes = input.as_bytes();
        for cut in 0..=bytes.len() {
            assert_eq!(run(&[&bytes[..cut], &bytes[cut..]]), whole, "cut at {cut}");
        }
        // Byte by byte.
        let singles: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(run(&singles), whole);
    }

    #[test]
    fn invalid_utf8() {
        assert_eq!(run(&[b"a\xffb"]), vec!["T:a\u{FFFD}b"]);
        // Truncated character at the very end.
        assert_eq!(run(&[b"a\xe2\x82"]), vec!["T:a\u{FFFD}"]);
        // Truncated character followed by a control byte.
        assert_eq!(run(&[b"a\xe2\x82\nb"]), vec!["T:a\u{FFFD}", "LineFeed", "T:b"]);
        // Split inside a character: no replacement.
        assert_eq!(run(&[b"\xe2", b"\x82", b"\xac"]), vec!["T:\u{20ac}"]);
        // Lone continuation bytes.
        assert_eq!(run(&[b"\x80\x80"]), vec!["T:\u{FFFD}\u{FFFD}"]);
    }

    #[test]
    fn unterminated_osc_is_bounded() {
        let mut data = b"\x1b]0;".to_vec();
        data.extend(std::iter::repeat_n(b'x', 5000));
        data.extend_from_slice(b"visible");
        let ev = run(&[&data]);
        // The OSC is abandoned after the cap; what follows shows (some of the
        // payload leaks as text, but the output does not vanish).
        assert!(ev.last().is_some_and(|l| l.ends_with("visible")));
    }

    #[test]
    fn esc_inside_csi_restarts() {
        let ev = run(&[b"\x1b[3\x1b[31mX"]);
        assert_eq!(ev.last().map(String::as_str), Some("T:X"));
    }

    #[test]
    fn can_aborts_sequence() {
        assert_eq!(run(&[b"\x1b[3\x18ok"]), vec!["T:ok"]);
    }

    #[test]
    fn c1_controls_dropped() {
        assert_eq!(run(&["a\u{9b}b".as_bytes()]), vec!["T:ab"]);
    }

    #[test]
    fn strip_helper() {
        assert_eq!(strip_ansi("\x1b[1mhi\x1b[0m\r\nthere"), "hi\nthere");
    }
}
