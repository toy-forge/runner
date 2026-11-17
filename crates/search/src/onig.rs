//! Translation of Oniguruma (Ruby syntax, what CodeRunner's find bar uses)
//! pattern constructs that `fancy-regex` does not know into ones it does.
//!
//! Handled: `\h` / `\H` (hex digit), `\Z`, `\R`, `\e`, `(?m)` (Ruby: dot
//! matches newline, so it becomes `(?s)`), and, for text with CRLF line
//! ends, `$`, `.` and `\n`.
//!
//! Everything else is passed through unchanged. Not supported (compile
//! error): subexpression calls `\g<name>`, absent operators `(?~...)`.

/// Translates `pattern`. With `crlf`, `$`, `.` and `\n` also understand `\r\n`
/// line ends (the regex engine only knows `\n`).
pub fn translate(pattern: &str, crlf: bool) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut class_depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let in_class = class_depth > 0;
        match c {
            '\\' => {
                let Some(&n) = chars.get(i + 1) else {
                    out.push('\\');
                    break;
                };
                i += 2;
                match (n, in_class) {
                    ('h', false) => out.push_str("[0-9a-fA-F]"),
                    ('h', true) => out.push_str("0-9a-fA-F"),
                    ('H', false) => out.push_str("[^0-9a-fA-F]"),
                    ('e', _) => out.push_str("\\x1B"),
                    ('Z', false) => out.push_str(if crlf {
                        "(?=\\r?\\n?\\z)"
                    } else {
                        "(?=\\n?\\z)"
                    }),
                    ('R', false) => {
                        out.push_str("(?:\\r\\n|[\\n\\x0B\\x0C\\r\\u{85}\\u{2028}\\u{2029}])")
                    }
                    ('n', false) if crlf => out.push_str("\\r?\\n"),
                    ('n', true) if crlf => out.push_str("\\n\\r"),
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            }
            '[' if in_class && chars.get(i + 1) == Some(&':') => {
                // POSIX class `[:alpha:]`: copy through `:]`.
                if let Some(rel) = find_posix_end(&chars[i..]) {
                    out.extend(&chars[i..i + rel]);
                    i += rel;
                } else {
                    class_depth += 1;
                    out.push('[');
                    i += 1;
                }
            }
            '[' => {
                class_depth += 1;
                out.push('[');
                i += 1;
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                // A leading `]` is a literal member.
                if chars.get(i) == Some(&']') {
                    out.push_str("\\]");
                    i += 1;
                }
            }
            ']' if in_class => {
                class_depth -= 1;
                out.push(']');
                i += 1;
            }
            '(' if !in_class && chars.get(i + 1) == Some(&'?') => {
                // Inline flags `(?imx-imx)` / `(?imx-imx:`.
                let mut j = i + 2;
                while j < chars.len() && "imsxuU-R".contains(chars[j]) {
                    j += 1;
                }
                if j > i + 2 && matches!(chars.get(j), Some(')') | Some(':')) {
                    out.push_str("(?");
                    for &f in &chars[i + 2..j] {
                        out.push(if f == 'm' { 's' } else { f });
                    }
                    i = j;
                } else {
                    out.push('(');
                    i += 1;
                }
            }
            '.' if !in_class && crlf => {
                out.push_str("[^\\r\\n]");
                i += 1;
            }
            '$' if !in_class && crlf => {
                out.push_str("(?:$|(?=\\r\\n))");
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Length of a `[:name:]` run at the start of `s`, if it is one.
fn find_posix_end(s: &[char]) -> Option<usize> {
    let mut j = 2;
    if s.get(j) == Some(&'^') {
        j += 1;
    }
    while j < s.len() && s[j].is_ascii_alphabetic() {
        j += 1;
    }
    (s.get(j) == Some(&':') && s.get(j + 1) == Some(&']')).then_some(j + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough() {
        assert_eq!(
            translate(r"(\w+)\s*=\s*(?<v>\d+)", false),
            r"(\w+)\s*=\s*(?<v>\d+)"
        );
        assert_eq!(translate(r"(?<=a)b(?!c)", false), r"(?<=a)b(?!c)");
        assert_eq!(translate(r"[[:alpha:]]+", false), r"[[:alpha:]]+");
    }

    #[test]
    fn onig_escapes() {
        assert_eq!(translate(r"\h+", false), "[0-9a-fA-F]+");
        assert_eq!(translate(r"[\h_]", false), "[0-9a-fA-F_]");
        assert_eq!(translate(r"(?m)a.b", false), "(?s)a.b");
        assert_eq!(translate(r"(?mi:a)", false), "(?si:a)");
        assert_eq!(translate(r"a\Z", false), "a(?=\\n?\\z)");
    }

    #[test]
    fn crlf_variant() {
        assert_eq!(translate(r"a.*$", true), "a[^\\r\\n]*(?:$|(?=\\r\\n))");
        assert_eq!(translate(r"a\nb", true), "a\\r?\\nb");
        assert_eq!(translate(r"[^\n]*", true), "[^\\n\\r]*");
        assert_eq!(translate(r"\.\$", true), r"\.\$");
        assert_eq!(translate(r"[.$]", true), "[.$]");
    }
}
