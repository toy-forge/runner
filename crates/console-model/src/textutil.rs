//! Offset conversions between the model's byte offsets, UTF-16 code units
//! (what NSH text hit-testing reports) and display columns (what a monospace
//! renderer measures).

use unicode_width::UnicodeWidthChar;

/// The largest char boundary `<= offset` (clamped to the text).
pub fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while !text.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// The byte offset of UTF-16 offset `utf16` (clamped to the text length).
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// The UTF-16 offset of byte offset `byte`.
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    let byte = floor_char_boundary(text, byte);
    text[..byte].chars().map(char::len_utf16).sum()
}

/// The display width of `text` in terminal cells.
pub fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The byte offset of the character covering display column `col` (clamped);
/// a column inside a wide character gives that character's start.
pub fn display_col_to_byte(text: &str, col: usize) -> usize {
    let mut w = 0;
    for (i, c) in text.char_indices() {
        let cw = c.width().unwrap_or(0);
        if col < w + cw {
            return i;
        }
        w += cw;
    }
    text.len()
}

/// The display column at byte offset `byte`.
pub fn byte_to_display_col(text: &str, byte: usize) -> usize {
    let byte = floor_char_boundary(text, byte);
    display_width(&text[..byte])
}

/// The word around `byte`: runs of alphanumerics/underscore, runs of
/// whitespace, or a single other character.
pub fn word_range(text: &str, byte: usize) -> std::ops::Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let byte = floor_char_boundary(text, byte).min(text.len().saturating_sub(1));
    let byte = floor_char_boundary(text, byte);
    let Some(c) = text[byte..].chars().next() else {
        return text.len()..text.len();
    };
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            1
        } else if c.is_whitespace() {
            2
        } else {
            0
        }
    };
    let k = class(c);
    if k == 0 {
        return byte..byte + c.len_utf8();
    }
    let mut start = byte;
    for (i, ch) in text[..byte].char_indices().rev() {
        if class(ch) == k {
            start = i;
        } else {
            break;
        }
    }
    let mut end = byte + c.len_utf8();
    for ch in text[end..].chars() {
        if class(ch) == k {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip() {
        let s = "a\u{1f600}b\u{4e2d}";
        assert_eq!(byte_to_utf16(s, 0), 0);
        assert_eq!(byte_to_utf16(s, 1), 1);
        assert_eq!(byte_to_utf16(s, 5), 3);
        assert_eq!(utf16_to_byte(s, 3), 5);
        assert_eq!(utf16_to_byte(s, 2), 5); // inside the surrogate pair -> next boundary
        assert_eq!(utf16_to_byte(s, 99), s.len());
    }

    #[test]
    fn columns() {
        let s = "a\u{4e2d}b";
        assert_eq!(display_width(s), 4);
        assert_eq!(display_col_to_byte(s, 0), 0);
        assert_eq!(display_col_to_byte(s, 1), 1);
        assert_eq!(display_col_to_byte(s, 2), 1); // second half of the wide char
        assert_eq!(display_col_to_byte(s, 3), 4);
        assert_eq!(display_col_to_byte(s, 4), 5);
        assert_eq!(display_col_to_byte(s, 40), 5);
        assert_eq!(byte_to_display_col(s, 4), 3);
    }

    #[test]
    fn words() {
        let s = "foo_bar  baz.qux";
        assert_eq!(word_range(s, 2), 0..7);
        assert_eq!(word_range(s, 7), 7..9);
        assert_eq!(word_range(s, 9), 9..12);
        assert_eq!(word_range(s, 12), 12..13);
        assert_eq!(word_range(s, 99), 13..16);
        assert_eq!(word_range("", 0), 0..0);
    }
}
