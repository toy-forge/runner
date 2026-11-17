//! A small structured model of the markup servers send (hover, documentation, signature help):
//! paragraphs, headings, lists, code blocks with their language, and inline code, emphasis and
//! links. The UI renders it with its own components; no HTML, no Markdown engine.
//!
//! The Markdown reader handles what language servers actually produce: fenced code blocks,
//! headings, lists, block quotes, rules, backtick code, `**strong**`, `*emphasis*`,
//! `[text](url)` links, bare URLs, hard line breaks, and a few HTML tags and entities.

/// Structured text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichText {
    /// The blocks, top to bottom.
    pub blocks: Vec<Block>,
}

/// A block of [`RichText`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph.
    Paragraph(Vec<Inline>),
    /// A heading of level 1 to 6.
    Heading {
        /// The level.
        level: u8,
        /// Its text.
        content: Vec<Inline>,
    },
    /// Code, with the language the server names (`python`, `ts`, ...), if any.
    CodeBlock {
        /// The language.
        language: Option<String>,
        /// The code, without the fences and without a trailing newline.
        code: String,
    },
    /// A list.
    List {
        /// Whether the items are numbered.
        ordered: bool,
        /// The items.
        items: Vec<Vec<Inline>>,
    },
    /// A quotation.
    Quote(Vec<Inline>),
    /// A horizontal rule.
    Rule,
}

/// A piece of text inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Plain text.
    Text(String),
    /// Code.
    Code(String),
    /// Strong text.
    Strong(String),
    /// Emphasized text.
    Emphasis(String),
    /// A link.
    Link {
        /// What is shown.
        text: String,
        /// Where it goes.
        url: String,
    },
    /// A forced line break.
    LineBreak,
}

impl RichText {
    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Plain text: paragraphs at blank lines, single line breaks kept.
    pub fn plain(text: &str) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut blocks = Vec::new();
        let mut current: Vec<&str> = Vec::new();
        let flush = |current: &mut Vec<&str>, blocks: &mut Vec<Block>| {
            if current.is_empty() {
                return;
            }
            let mut inlines = Vec::new();
            for (i, line) in current.iter().enumerate() {
                if i > 0 {
                    inlines.push(Inline::LineBreak);
                }
                inlines.push(Inline::Text((*line).to_owned()));
            }
            blocks.push(Block::Paragraph(inlines));
            current.clear();
        };
        for line in normalized.split('\n') {
            if line.trim().is_empty() {
                flush(&mut current, &mut blocks);
            } else {
                current.push(line);
            }
        }
        flush(&mut current, &mut blocks);
        Self { blocks }
    }

    /// A single code block.
    pub fn code(language: Option<&str>, code: &str) -> Self {
        Self {
            blocks: vec![Block::CodeBlock {
                language: language.filter(|l| !l.is_empty()).map(str::to_owned),
                code: code.to_owned(),
            }],
        }
    }

    /// Markdown, as described in the module documentation.
    pub fn markdown(text: &str) -> Self {
        parse_markdown(text)
    }

    /// Appends `other`'s blocks.
    pub fn extend(&mut self, other: RichText) {
        self.blocks.extend(other.blocks);
    }

    /// The text without any formatting. Code blocks keep their lines; blocks are separated by a
    /// blank line.
    pub fn to_plain(&self) -> String {
        let mut parts = Vec::new();
        for block in &self.blocks {
            parts.push(match block {
                Block::Paragraph(inlines)
                | Block::Quote(inlines)
                | Block::Heading {
                    content: inlines, ..
                } => inlines_to_plain(inlines),
                Block::CodeBlock { code, .. } => code.clone(),
                Block::List { ordered, items } => items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let marker = if *ordered {
                            format!("{}.", i + 1)
                        } else {
                            "-".to_owned()
                        };
                        format!("{marker} {}", inlines_to_plain(item))
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Block::Rule => "---".to_owned(),
            });
        }
        parts.join("\n\n")
    }

    /// The links in the text, in order.
    pub fn links(&self) -> Vec<(&str, &str)> {
        let mut links = Vec::new();
        for block in &self.blocks {
            let lists: Vec<&[Inline]> = match block {
                Block::Paragraph(i) | Block::Quote(i) | Block::Heading { content: i, .. } => {
                    vec![i]
                }
                Block::List { items, .. } => items.iter().map(Vec::as_slice).collect(),
                _ => Vec::new(),
            };
            for inlines in lists {
                for inline in inlines {
                    if let Inline::Link { text, url } = inline {
                        links.push((text.as_str(), url.as_str()));
                    }
                }
            }
        }
        links
    }
}

/// The plain text of inline content.
pub fn inlines_to_plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) | Inline::Strong(t) | Inline::Emphasis(t) => {
                out.push_str(t)
            }
            Inline::Link { text, .. } => out.push_str(text),
            Inline::LineBreak => out.push('\n'),
        }
    }
    out
}

/// A parsed `contents` / `documentation` value: `MarkupContent`, `MarkedString`, or an array of
/// marked strings.
pub fn from_markup_json(value: &serde_json::Value) -> Option<RichText> {
    use serde_json::Value;
    let text = match value {
        Value::String(text) => RichText::markdown(text),
        Value::Object(map) => {
            if let Some(language) = map.get("language").and_then(Value::as_str) {
                // A MarkedString: { language, value }.
                RichText::code(Some(language), map.get("value").and_then(Value::as_str)?)
            } else {
                let body = map.get("value").and_then(Value::as_str)?;
                match map.get("kind").and_then(Value::as_str) {
                    Some("plaintext") => RichText::plain(body),
                    _ => RichText::markdown(body),
                }
            }
        }
        Value::Array(items) => {
            let mut all = RichText::default();
            for item in items {
                if let Some(part) = from_markup_json(item) {
                    all.extend(part);
                }
            }
            all
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

type Fence = (char, usize, Option<String>, Vec<String>, usize);

fn parse_markdown(source: &str) -> RichText {
    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut quote: Vec<String> = Vec::new();
    let mut list: Option<(bool, Vec<String>)> = None;
    let mut fence: Option<Fence> = None;

    fn flush_paragraph(paragraph: &mut Vec<String>, blocks: &mut Vec<Block>) {
        if paragraph.is_empty() {
            return;
        }
        let mut inlines: Vec<Inline> = Vec::new();
        let count = paragraph.len();
        for (i, line) in paragraph.iter().enumerate() {
            let hard =
                line.ends_with("  ") || line.ends_with('\\') || line.trim_start().starts_with('|');
            let text = line.trim_end_matches('\\');
            let mut parsed = parse_inlines(text.trim());
            inlines.append(&mut parsed);
            if i + 1 < count {
                if hard || paragraph[i + 1].trim_start().starts_with('|') {
                    inlines.push(Inline::LineBreak);
                } else {
                    inlines.push(Inline::Text(" ".to_owned()));
                }
            }
        }
        merge_text(&mut inlines);
        paragraph.clear();
        if !inlines.is_empty() {
            blocks.push(Block::Paragraph(inlines));
        }
    }
    fn flush_quote(quote: &mut Vec<String>, blocks: &mut Vec<Block>) {
        if quote.is_empty() {
            return;
        }
        let mut inlines = parse_inlines(&quote.join(" "));
        merge_text(&mut inlines);
        quote.clear();
        blocks.push(Block::Quote(inlines));
    }
    fn flush_list(list: &mut Option<(bool, Vec<String>)>, blocks: &mut Vec<Block>) {
        if let Some((ordered, items)) = list.take() {
            let items = items
                .iter()
                .map(|item| {
                    let mut inlines = parse_inlines(item.trim());
                    merge_text(&mut inlines);
                    inlines
                })
                .collect();
            blocks.push(Block::List { ordered, items });
        }
    }

    for line in source.split('\n') {
        if let Some((ch, len, language, lines, indent)) = fence.as_mut() {
            let trimmed = line.trim();
            if trimmed.len() >= *len && trimmed.chars().all(|c| c == *ch) {
                let code = lines.join("\n");
                blocks.push(Block::CodeBlock {
                    language: language.take(),
                    code,
                });
                fence = None;
            } else {
                let strip = line.chars().take_while(|c| *c == ' ').count().min(*indent);
                lines.push(line[strip..].to_owned());
            }
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(opener) = fence_opener(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            flush_list(&mut list, &mut blocks);
            fence = Some((opener.0, opener.1, opener.2, Vec::new(), indent));
            continue;
        }
        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_quote(&mut quote, &mut blocks);
            flush_list(&mut list, &mut blocks);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            quote.push(rest.trim_start().to_owned());
            continue;
        }
        flush_quote(&mut quote, &mut blocks);
        if let Some((level, rest)) = heading(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            let mut content = parse_inlines(rest);
            merge_text(&mut content);
            blocks.push(Block::Heading { level, content });
            continue;
        }
        if is_rule(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some((ordered, rest)) = list_item(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            match &mut list {
                Some((existing, items)) if *existing == ordered => items.push(rest.to_owned()),
                _ => {
                    flush_list(&mut list, &mut blocks);
                    list = Some((ordered, vec![rest.to_owned()]));
                }
            }
            continue;
        }
        if let Some((_, items)) = list.as_mut().filter(|_| indent > 0)
            && let Some(last) = items.last_mut()
        {
            last.push(' ');
            last.push_str(trimmed);
            continue;
        }
        flush_list(&mut list, &mut blocks);
        paragraph.push(line.trim_start().to_owned());
    }
    if let Some((_, _, language, lines, _)) = fence {
        blocks.push(Block::CodeBlock {
            language,
            code: lines.join("\n"),
        });
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    flush_quote(&mut quote, &mut blocks);
    flush_list(&mut list, &mut blocks);
    RichText { blocks }
}

fn fence_opener(line: &str) -> Option<(char, usize, Option<String>)> {
    let ch = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = line.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    let info = line[len..].trim();
    if ch == '`' && info.contains('`') {
        return None;
    }
    let language = info
        .split(|c: char| c.is_whitespace() || c == '{' || c == ',')
        .find(|part| !part.is_empty())
        .map(|part| part.trim_start_matches('.').to_owned())
        .filter(|part| !part.is_empty());
    Some((ch, len, language))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|marker| compact.chars().all(|c| marker.starts_with(c)))
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((false, rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, rest));
        }
    }
    None
}

const HTML_TAGS: &[&str] = &[
    "p",
    "div",
    "span",
    "b",
    "i",
    "em",
    "strong",
    "code",
    "pre",
    "a",
    "br",
    "hr",
    "ul",
    "ol",
    "li",
    "details",
    "summary",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "sup",
    "sub",
    "u",
    "s",
    "small",
    "blockquote",
    "img",
    "kbd",
    "dl",
    "dt",
    "dd",
];

fn merge_text(inlines: &mut Vec<Inline>) {
    let mut merged: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines.drain(..) {
        match (merged.last_mut(), inline) {
            (Some(Inline::Text(previous)), Inline::Text(next)) => previous.push_str(&next),
            (_, other) => merged.push(other),
        }
    }
    if let Some(Inline::Text(first)) = merged.first_mut() {
        *first = first.trim_start().to_owned();
    }
    if let Some(Inline::Text(last)) = merged.last_mut() {
        *last = last.trim_end().to_owned();
    }
    merged.retain(|inline| !matches!(inline, Inline::Text(t) if t.is_empty()));
    *inlines = merged;
}

fn decode_entity(rest: &str) -> Option<(char, usize)> {
    let end = rest.find(';').filter(|end| *end <= 8)?;
    let name = &rest[1..end];
    let ch = match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((ch, end + 1))
}

/// Parses inline Markdown into pieces. Not merged; callers merge adjacent text.
fn parse_inlines(source: &str) -> Vec<Inline> {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            out.push(Inline::Text(std::mem::take(text)));
        }
    };
    while i < chars.len() {
        let (byte, ch) = chars[i];
        let rest = &source[byte..];
        match ch {
            '\\' if chars
                .get(i + 1)
                .is_some_and(|(_, c)| c.is_ascii_punctuation()) =>
            {
                text.push(chars[i + 1].1);
                i += 2;
            }
            '`' => {
                let run = rest.chars().take_while(|c| *c == '`').count();
                let fence = "`".repeat(run);
                if let Some(close) = rest[run..].find(&fence) {
                    flush(&mut text, &mut out);
                    let code = &rest[run..run + close];
                    let code = code
                        .strip_prefix(' ')
                        .and_then(|c| c.strip_suffix(' '))
                        .unwrap_or(code);
                    out.push(Inline::Code(code.to_owned()));
                    let end_byte = byte + run + close + run;
                    while i < chars.len() && chars[i].0 < end_byte {
                        i += 1;
                    }
                } else {
                    text.push_str(&fence);
                    i += run;
                }
            }
            '!' if rest.starts_with("![") => {
                if let Some((label, _url, used)) = link_at(&rest[1..]) {
                    text.push_str(&label);
                    let end_byte = byte + 1 + used;
                    while i < chars.len() && chars[i].0 < end_byte {
                        i += 1;
                    }
                } else {
                    text.push('!');
                    i += 1;
                }
            }
            '[' => {
                if let Some((label, url, used)) = link_at(rest) {
                    flush(&mut text, &mut out);
                    let label = inlines_to_plain(&parse_inlines(&label));
                    out.push(Inline::Link { text: label, url });
                    let end_byte = byte + used;
                    while i < chars.len() && chars[i].0 < end_byte {
                        i += 1;
                    }
                } else {
                    text.push('[');
                    i += 1;
                }
            }
            '<' => {
                let close = rest.find('>').filter(|c| *c < 200);
                let inner = close.map(|c| &rest[1..c]);
                match (close, inner) {
                    (Some(close), Some(inner))
                        if inner.starts_with("http://") || inner.starts_with("https://") =>
                    {
                        flush(&mut text, &mut out);
                        out.push(Inline::Link {
                            text: inner.to_owned(),
                            url: inner.to_owned(),
                        });
                        i = advance(&chars, i, byte + close + 1);
                    }
                    (Some(close), Some(inner)) => {
                        let name: String = inner
                            .trim_start_matches('/')
                            .chars()
                            .take_while(char::is_ascii_alphanumeric)
                            .collect::<String>()
                            .to_ascii_lowercase();
                        if HTML_TAGS.contains(&name.as_str()) && !name.is_empty() {
                            if name == "br"
                                || name == "hr"
                                || (matches!(name.as_str(), "p" | "div" | "li" | "tr")
                                    && inner.starts_with('/'))
                            {
                                flush(&mut text, &mut out);
                                out.push(Inline::LineBreak);
                            }
                            i = advance(&chars, i, byte + close + 1);
                        } else {
                            text.push('<');
                            i += 1;
                        }
                    }
                    _ => {
                        text.push('<');
                        i += 1;
                    }
                }
            }
            '&' => match decode_entity(rest) {
                Some((decoded, used)) => {
                    text.push(decoded);
                    i = advance(&chars, i, byte + used);
                }
                None => {
                    text.push('&');
                    i += 1;
                }
            },
            '*' | '_' => {
                let double = rest.starts_with("**") || rest.starts_with("__");
                let marker_len = if double { 2 } else { 1 };
                let marker = &rest[..marker_len];
                let previous = if i == 0 { None } else { Some(chars[i - 1].1) };
                let opens = !(ch == '_' && previous.is_some_and(char::is_alphanumeric))
                    && rest[marker_len..]
                        .chars()
                        .next()
                        .is_some_and(|c| !c.is_whitespace());
                let close = opens
                    .then(|| find_closer(&rest[marker_len..], marker, ch == '_'))
                    .flatten();
                match close {
                    Some(close) if close > 0 => {
                        flush(&mut text, &mut out);
                        let inner = &rest[marker_len..marker_len + close];
                        let plain = inlines_to_plain(&parse_inlines(inner));
                        out.push(if double {
                            Inline::Strong(plain)
                        } else {
                            Inline::Emphasis(plain)
                        });
                        i = advance(&chars, i, byte + marker_len + close + marker_len);
                    }
                    _ => {
                        text.push_str(marker);
                        i = advance(&chars, i, byte + marker_len);
                    }
                }
            }
            'h' if (rest.starts_with("http://") || rest.starts_with("https://"))
                && (i == 0 || !chars[i - 1].1.is_alphanumeric()) =>
            {
                let end = rest
                    .find(|c: char| {
                        c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`' | ')' | ']')
                    })
                    .unwrap_or(rest.len());
                let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
                flush(&mut text, &mut out);
                out.push(Inline::Link {
                    text: url.to_owned(),
                    url: url.to_owned(),
                });
                i = advance(&chars, i, byte + url.len());
            }
            _ => {
                text.push(ch);
                i += 1;
            }
        }
    }
    flush(&mut text, &mut out);
    out
}

/// The index of the first char at or after byte `target`.
fn advance(chars: &[(usize, char)], mut i: usize, target: usize) -> usize {
    while i < chars.len() && chars[i].0 < target {
        i += 1;
    }
    i
}

/// The byte offset of the closing `marker` in `rest`, if the text before it is a valid span.
fn find_closer(rest: &str, marker: &str, underscore: bool) -> Option<usize> {
    let mut search = 0;
    while let Some(found) = rest[search..].find(marker) {
        let at = search + found;
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace());
        let after = rest[at + marker.len()..].chars().next();
        let after_ok = !underscore || after.is_none_or(|c| !c.is_alphanumeric());
        if at > 0 && before_ok && after_ok {
            return Some(at);
        }
        search = at + marker.len();
    }
    None
}

/// Parses `[label](url)` at the start of `rest`. Returns the label, the URL and the bytes used.
fn link_at(rest: &str) -> Option<(String, String, usize)> {
    let bytes = rest.as_bytes();
    let mut depth = 0;
    let mut close = None;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'\\' => continue,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let after = &rest[close + 1..];
    let inner = after.strip_prefix('(')?;
    let mut paren = 1;
    let mut end = None;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' => paren += 1,
            ')' => {
                paren -= 1;
                if paren == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let target = inner[..end].trim();
    // Drop an optional title: `url "title"`.
    let url = target
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['<', '>']);
    Some((
        rest[1..close].to_owned(),
        url.to_owned(),
        close + 1 + 1 + end + 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_owned())
    }

    #[test]
    fn code_block_with_language_and_paragraph() {
        let rich = RichText::markdown(
            "```python\ndef f(x: int) -> str\n```\n---\nReturns the *name*.\nSecond line.",
        );
        assert_eq!(
            rich.blocks,
            vec![
                Block::CodeBlock {
                    language: Some("python".into()),
                    code: "def f(x: int) -> str".into()
                },
                Block::Rule,
                Block::Paragraph(vec![
                    text("Returns the "),
                    Inline::Emphasis("name".into()),
                    text(". Second line.")
                ]),
            ]
        );
    }

    #[test]
    fn links_code_and_strong() {
        let rich = RichText::markdown(
            "See [the docs](https://example.com/a_b) for `os.path` and **more**. https://x.org/y.",
        );
        let Block::Paragraph(inlines) = &rich.blocks[0] else {
            panic!("{rich:?}")
        };
        assert_eq!(
            inlines,
            &vec![
                text("See "),
                Inline::Link {
                    text: "the docs".into(),
                    url: "https://example.com/a_b".into()
                },
                text(" for "),
                Inline::Code("os.path".into()),
                text(" and "),
                Inline::Strong("more".into()),
                text(". "),
                Inline::Link {
                    text: "https://x.org/y".into(),
                    url: "https://x.org/y".into()
                },
                text("."),
            ]
        );
        assert_eq!(rich.links().len(), 2);
    }

    #[test]
    fn snake_case_is_not_emphasis() {
        let rich = RichText::markdown("call my_func_name and __init__ now");
        assert_eq!(rich.to_plain(), "call my_func_name and init now");
        let rich = RichText::markdown("a * b * c");
        assert_eq!(rich.to_plain(), "a * b * c");
    }

    #[test]
    fn lists_headings_quotes() {
        let rich = RichText::markdown(
            "# Title\n- one\n- two\n  more\n\n1. first\n2. second\n\n> quoted\n> text",
        );
        assert_eq!(rich.blocks.len(), 4);
        assert!(matches!(&rich.blocks[0], Block::Heading { level: 1, .. }));
        let Block::List {
            ordered: false,
            items,
        } = &rich.blocks[1]
        else {
            panic!()
        };
        assert_eq!(items[1], vec![text("two more")]);
        assert!(matches!(&rich.blocks[2], Block::List { ordered: true, .. }));
        assert_eq!(rich.blocks[3], Block::Quote(vec![text("quoted text")]));
    }

    #[test]
    fn html_and_entities() {
        let rich = RichText::markdown("a &lt;b&gt; <b>bold</b><br>next &amp; List<int>");
        let Block::Paragraph(inlines) = &rich.blocks[0] else {
            panic!()
        };
        assert!(inlines.contains(&Inline::LineBreak));
        assert_eq!(inlines_to_plain(inlines), "a <b> bold\nnext & List<int>");
    }

    #[test]
    fn unterminated_fence_and_empty() {
        let rich = RichText::markdown("```rs\nlet x = 1;");
        assert_eq!(
            rich.blocks,
            vec![Block::CodeBlock {
                language: Some("rs".into()),
                code: "let x = 1;".into()
            }]
        );
        assert!(RichText::markdown("   \n").is_empty());
    }

    #[test]
    fn plain_text_keeps_line_breaks() {
        let rich = RichText::plain("one\ntwo\n\nthree");
        assert_eq!(rich.blocks.len(), 2);
        assert_eq!(rich.to_plain(), "one\ntwo\n\nthree");
    }

    #[test]
    fn markup_json_shapes() {
        use serde_json::json;
        let marked = from_markup_json(&json!(["one", {"language": "py", "value": "two"}])).unwrap();
        assert_eq!(marked.blocks.len(), 2);
        assert!(
            matches!(&marked.blocks[1], Block::CodeBlock { language: Some(l), .. } if l == "py")
        );
        let content = from_markup_json(&json!({"kind": "plaintext", "value": "a *b*"})).unwrap();
        assert_eq!(content.to_plain(), "a *b*");
        assert!(from_markup_json(&json!({"kind": "markdown", "value": ""})).is_none());
        assert!(from_markup_json(&json!(null)).is_none());
    }

    #[test]
    fn table_rows_break_lines() {
        let rich = RichText::markdown("| a | b |\n| - | - |\n| 1 | 2 |");
        let Block::Paragraph(inlines) = &rich.blocks[0] else {
            panic!()
        };
        assert_eq!(
            inlines.iter().filter(|i| **i == Inline::LineBreak).count(),
            2
        );
    }
}
