//! LSP snippets (`insertTextFormat` 2) expanded to plain text plus tab stops.
//!
//! The NSH editor has no snippet engine, so completions whose `insertText` is a snippet are
//! inserted as plain text: `$1`/`${1:placeholder}` become the placeholder text, `${1|a,b|}` the
//! first choice, and variables are substituted. The tab stops the user would have cycled through
//! are returned as UTF-16 offsets into the inserted text, so the UI can place the caret after the
//! insertion without knowing the snippet grammar.
//!
//! Supported syntax (the subset language servers actually send): `$1`, `${1}`, `${1:text}`,
//! `${1|one,two|}`, `$0`, `\$`, `\\`, `\}`, `${VAR}`, `${VAR:default}`, `$VAR`, nested
//! placeholders, and mirrors (a repeated index keeps the first placeholder's text).

use crate::text::utf16_len;

/// One tab stop of an expanded snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabStop {
    /// The snippet index: 1, 2, ...; 0 is only used by [`ExpandedSnippet::final_stop`].
    pub index: u32,
    /// The offset from the start of the inserted text, in UTF-16 code units.
    pub utf16_offset: u32,
    /// The length of the placeholder's text, in UTF-16 code units.
    pub utf16_length: u32,
    /// The text the stop covers, when it had a placeholder.
    pub placeholder: Option<String>,
}

/// A snippet expanded to plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedSnippet {
    /// The text to insert.
    pub text: String,
    /// The tab stops with index 1 and above, in tab order.
    pub stops: Vec<TabStop>,
    /// Where the caret goes last: `$0`, or the end of the text when the snippet has no `$0`.
    pub final_stop: TabStop,
}

impl ExpandedSnippet {
    /// The UTF-16 offset of the final stop.
    pub fn final_offset(&self) -> u32 {
        self.final_stop.utf16_offset
    }
}

/// Expands a snippet, substituting no variables (an unknown variable with no default becomes
/// empty, as the snippet specification says).
pub fn expand_snippet(source: &str) -> ExpandedSnippet {
    expand_snippet_with(source, |_| None)
}

/// Expands a snippet, asking `variable` for the values of `${VAR}`/`$VAR`. Return `None` for an
/// unknown variable; its default text or an empty string is inserted.
pub fn expand_snippet_with(
    source: &str,
    variable: impl Fn(&str) -> Option<String>,
) -> ExpandedSnippet {
    let mut parser = Parser {
        source,
        position: 0,
        output: String::with_capacity(source.len()),
        stops: Vec::new(),
        variable: &variable,
    };
    parser.parse(false);
    finish(parser.output, parser.stops)
}

struct RawStop {
    index: u32,
    offset: usize,
    length: usize,
    placeholder: Option<String>,
}

struct Parser<'a, F: Fn(&str) -> Option<String>> {
    source: &'a str,
    position: usize,
    output: String,
    stops: Vec<RawStop>,
    variable: &'a F,
}

impl<F: Fn(&str) -> Option<String>> Parser<'_, F> {
    fn rest(&self) -> &str {
        &self.source[self.position..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.position += ch.len_utf8();
        Some(ch)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.position += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn push(&mut self, ch: char) {
        self.output.push(ch);
    }

    /// Parses until `}` at nesting level `nested`, or to the end. The terminator is not consumed.
    fn parse(&mut self, nested: bool) {
        loop {
            let Some(ch) = self.peek() else { return };
            if nested && ch == '}' {
                return;
            }
            match ch {
                '\\' => {
                    let after = self.rest()[1..].chars().next();
                    match after {
                        Some(escaped @ ('$' | '}' | '\\' | '|' | ',')) => {
                            self.position += 1 + escaped.len_utf8();
                            self.push(escaped);
                        }
                        _ => {
                            self.position += 1;
                            self.push('\\');
                        }
                    }
                }
                '$' => self.parse_dollar(),
                _ => {
                    self.bump();
                    self.push(ch);
                }
            }
        }
    }

    fn parse_dollar(&mut self) {
        let start = self.position;
        let output_len = self.output.len();
        let stops_len = self.stops.len();
        self.position += 1;
        let Some(next) = self.peek() else {
            self.push('$');
            return;
        };
        if next.is_ascii_digit() {
            let index = self.take_digits();
            self.record(index);
            return;
        }
        if next == '{' {
            self.position += 1;
            if !self.parse_braced() {
                // Not a valid snippet construct: undo everything it may have written and keep
                // the `$` as literal text.
                self.output.truncate(output_len);
                self.stops.truncate(stops_len);
                self.position = start;
                self.bump();
                self.push('$');
            }
            return;
        }
        if is_name_start(next) {
            let name = self.take_name();
            if let Some(value) = (self.variable)(&name) {
                self.output.push_str(&value);
            }
            return;
        }
        self.push('$');
    }

    fn parse_braced(&mut self) -> bool {
        let Some(next) = self.peek() else {
            return false;
        };
        if next.is_ascii_digit() {
            let index = self.take_digits();
            match self.peek() {
                Some('}') => {
                    self.position += 1;
                    self.record(index);
                    true
                }
                Some(':') => {
                    self.position += 1;
                    let start = self.output.len();
                    self.parse(true);
                    if !self.eat('}') {
                        return false;
                    }
                    let text = self.output[start..].to_owned();
                    self.record_text(index, start, text);
                    true
                }
                Some('|') => {
                    self.position += 1;
                    let Some(choice) = self.take_choice() else {
                        return false;
                    };
                    let start = self.output.len();
                    self.output.push_str(&choice);
                    let text = self.output[start..].to_owned();
                    self.record_text(index, start, text);
                    true
                }
                _ => false,
            }
        } else if is_name_start(next) {
            let name = self.take_name();
            match self.peek() {
                Some('}') => {
                    self.position += 1;
                    if let Some(value) = (self.variable)(&name) {
                        self.output.push_str(&value);
                    }
                    true
                }
                Some(':') => {
                    self.position += 1;
                    let start = self.output.len();
                    let stops_before = self.stops.len();
                    self.parse(true);
                    if !self.eat('}') {
                        return false;
                    }
                    if let Some(value) = (self.variable)(&name) {
                        // The resolver wins over the default text.
                        self.output.truncate(start);
                        self.output.push_str(&value);
                        self.stops.truncate(stops_before);
                    }
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    /// Parses the choices of `${1|a,b|}` and appends the first; consumes through the final `}`.
    fn take_choice(&mut self) -> Option<String> {
        let mut choices = vec![String::new()];
        loop {
            match self.peek()? {
                '\\' => {
                    self.position += 1;
                    let escaped = self.bump()?;
                    choices.last_mut()?.push(escaped);
                }
                '|' => {
                    self.position += 1;
                    if !self.eat('}') {
                        return None;
                    }
                    return Some(choices.swap_remove(0));
                }
                ',' => {
                    self.position += 1;
                    choices.push(String::new());
                }
                other => {
                    self.bump();
                    choices.last_mut()?.push(other);
                }
            }
        }
    }

    fn take_digits(&mut self) -> u32 {
        let start = self.position;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        self.source[start..self.position]
            .parse()
            .unwrap_or(u32::MAX)
    }

    fn take_name(&mut self) -> String {
        let start = self.position;
        while self.peek().is_some_and(is_name_char) {
            self.bump();
        }
        self.source[start..self.position].to_owned()
    }

    fn record(&mut self, index: u32) {
        self.record_text(index, self.output.len(), String::new());
    }

    fn record_text(&mut self, index: u32, offset: usize, text: String) {
        let length = self.output.len().saturating_sub(offset);
        self.stops.push(RawStop {
            index,
            offset,
            length,
            placeholder: (!text.is_empty()).then_some(text),
        });
    }
}

fn is_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn finish(output: String, raw: Vec<RawStop>) -> ExpandedSnippet {
    let mut stops: Vec<RawStop> = Vec::new();
    let mut final_stop = None;
    for stop in raw {
        if stop.index == 0 {
            if final_stop.is_none() {
                final_stop = Some(stop);
            }
            continue;
        }
        // Mirrors repeat an index; the first occurrence is the editable one.
        if !stops.iter().any(|existing| existing.index == stop.index) {
            stops.push(stop);
        }
    }
    stops.sort_by_key(|stop| stop.index);
    let convert = |stop: &RawStop| TabStop {
        index: stop.index,
        utf16_offset: utf16_len(&output[..stop.offset]) as u32,
        utf16_length: utf16_len(&output[stop.offset..stop.offset + stop.length]) as u32,
        placeholder: stop.placeholder.clone(),
    };
    let final_stop = final_stop
        .map(|stop| convert(&stop))
        .unwrap_or_else(|| TabStop {
            index: 0,
            utf16_offset: utf16_len(&output) as u32,
            utf16_length: 0,
            placeholder: None,
        });
    ExpandedSnippet {
        stops: stops.iter().map(convert).collect(),
        text: output,
        final_stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_placeholder_and_final_caret() {
        let expanded = expand_snippet("print(${1:value})");
        assert_eq!(expanded.text, "print(value)");
        assert_eq!(expanded.stops.len(), 1);
        assert_eq!(
            expanded.stops[0],
            TabStop {
                index: 1,
                utf16_offset: 6,
                utf16_length: 5,
                placeholder: Some("value".into())
            }
        );
        assert_eq!(expanded.final_stop.utf16_offset, 12);
        assert_eq!(expanded.final_offset(), 12);
    }

    #[test]
    fn stops_are_sorted_and_mirrors_keep_the_first() {
        let expanded = expand_snippet("$2 ${1:x} $2 $0");
        assert_eq!(expanded.text, " x  ");
        assert_eq!(
            expanded
                .stops
                .iter()
                .map(|s| (s.index, s.utf16_offset))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 0)]
        );
        assert_eq!(expanded.final_stop.utf16_offset, 4);
        assert_eq!(expanded.final_stop.index, 0);
    }

    #[test]
    fn choices_take_the_first() {
        let expanded = expand_snippet("${1|one,two,three|} ${2|a\\,b,c|}");
        assert_eq!(expanded.text, "one a,b");
        assert_eq!(expanded.stops[0].placeholder.as_deref(), Some("one"));
        assert_eq!(expanded.stops[1].placeholder.as_deref(), Some("a,b"));
    }

    #[test]
    fn escapes_and_literals() {
        let expanded = expand_snippet(r"a \$1 b { c \\ d \} e $");
        assert_eq!(expanded.text, "a $1 b { c \\ d } e $");
        assert!(expanded.stops.is_empty());
        assert_eq!(expanded.final_stop.utf16_offset, 20);
    }

    #[test]
    fn nested_placeholders() {
        let expanded = expand_snippet("${1:foo ${2:bar}}!");
        assert_eq!(expanded.text, "foo bar!");
        assert_eq!(expanded.stops.len(), 2);
        assert_eq!(expanded.stops[0].utf16_offset, 0);
        assert_eq!(expanded.stops[0].utf16_length, 7);
        assert_eq!(expanded.stops[1].utf16_offset, 4);
        assert_eq!(expanded.stops[1].utf16_length, 3);
    }

    #[test]
    fn variables_and_defaults() {
        let expanded = expand_snippet_with(
            "${TM_FILENAME:untitled.py} $UNKNOWN ${UNKNOWN:fallback}",
            |name| (name == "TM_FILENAME").then(|| "main.py".to_owned()),
        );
        assert_eq!(expanded.text, "main.py  fallback");

        let plain = expand_snippet("${TM_FILENAME:untitled.py}");
        assert_eq!(plain.text, "untitled.py");
    }

    #[test]
    fn utf16_offsets_for_surrogate_pairs() {
        let expanded = expand_snippet("é${1:😀}");
        assert_eq!(expanded.text, "é😀");
        assert_eq!(expanded.stops[0].utf16_offset, 1);
        assert_eq!(expanded.stops[0].utf16_length, 2);
        assert_eq!(expanded.final_stop.utf16_offset, 3);
    }

    #[test]
    fn malformed_snippets_stay_literal() {
        for source in ["${1", "${1:", "$}", "${}", "${1|a}", "a ${} b"] {
            let expanded = expand_snippet(source);
            assert_eq!(expanded.text, source, "source {source:?}");
            assert!(expanded.stops.is_empty(), "source {source:?}");
        }
    }

    #[test]
    fn dollar_at_end_and_empty_snippet() {
        assert_eq!(expand_snippet("ab$").text, "ab$");
        let expanded = expand_snippet("");
        assert_eq!(expanded.text, "");
        assert_eq!(expanded.final_stop.utf16_offset, 0);
        assert!(expanded.stops.is_empty());
    }
}
