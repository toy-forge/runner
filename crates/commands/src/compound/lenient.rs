//! JSON with `//` and `/* */` comments, trailing commas and raw control characters in strings,
//! the dialect compound commands are written in.

use serde_json::Value;

use super::CompoundError;

/// Rewrites the lenient dialect into strict JSON, keeping every byte offset on its line
/// (comments and dropped commas become spaces; raw control characters inside strings become
/// `\uXXXX` escapes).
pub fn to_strict_json(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    let mut in_string = false;
    // Index in `out` of a comma that may turn out to be trailing.
    let mut pending_comma: Option<usize> = None;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            match c {
                '\\' => {
                    out.push(c);
                    if let Some(&n) = chars.get(i + 1) {
                        out.push(n);
                        i += 1;
                    }
                }
                '"' => {
                    in_string = false;
                    out.push(c);
                }
                c if (c as u32) < 0x20 => out.extend(format!("\\u{:04x}", c as u32).chars()),
                c => out.push(c),
            }
            i += 1;
            continue;
        }
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                out.push(' ');
                out.push(' ');
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if i < chars.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                }
                continue;
            }
            '"' => {
                in_string = true;
                pending_comma = None;
                out.push(c);
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
            }
            ']' | '}' => {
                if let Some(p) = pending_comma.take() {
                    out[p] = ' ';
                }
                out.push(c);
            }
            c if c.is_whitespace() => out.push(c),
            c => {
                pending_comma = None;
                out.push(c);
            }
        }
        i += 1;
    }
    out.into_iter().collect()
}

/// Parses the lenient dialect into a JSON value.
pub fn parse(src: &str) -> Result<Value, CompoundError> {
    serde_json::from_str(&to_strict_json(src)).map_err(|e| CompoundError::Json { line: e.line(), column: e.column(), message: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas() {
        let v = parse("[\n // note\n \"a\", /* x */ \"b\",\n [1, 2,],\n]").unwrap();
        assert_eq!(v, serde_json::json!(["a", "b", [1, 2]]));
    }

    #[test]
    fn comment_markers_inside_strings_are_kept() {
        let v = parse(r#"["http://x", "/* y */", "a,]"]"#).unwrap();
        assert_eq!(v, serde_json::json!(["http://x", "/* y */", "a,]"]));
    }

    #[test]
    fn raw_control_characters_in_strings() {
        let v = parse("[\"a\tb\nc\"]").unwrap();
        assert_eq!(v, serde_json::json!(["a\tb\nc"]));
    }

    #[test]
    fn object_trailing_comma() {
        let v = parse("{\"a\": [\"x\",], }").unwrap();
        assert_eq!(v, serde_json::json!({"a": ["x"]}));
    }

    #[test]
    fn reports_line_and_column() {
        match parse("[\n \"a\" \"b\"\n]") {
            Err(CompoundError::Json { line, .. }) => assert_eq!(line, 2),
            other => panic!("{other:?}"),
        }
    }
}
