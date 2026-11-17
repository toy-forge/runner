//! Line endings: detection and normalisation.
//!
//! The editor works with `\n` only. Files are normalised on load
//! ([`normalize`]) and the original style is restored on save
//! ([`apply`]).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// A line-ending style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    /// `\n` (Unix, macOS).
    #[default]
    Lf,
    /// `\r\n` (Windows).
    CrLf,
    /// `\r` (classic Mac OS).
    Cr,
}

impl LineEnding {
    /// The terminator text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }

    /// The name shown in menus.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Lf => "macOS / Unix (LF)",
            Self::CrLf => "Windows (CRLF)",
            Self::Cr => "Classic Mac (CR)",
        }
    }
}

/// What [`detect`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineEndingInfo {
    /// The most frequent style (`Lf` when the text has no line break; ties
    /// prefer `Lf`, then `CrLf`).
    pub dominant: LineEnding,
    /// Number of `\n` terminators.
    pub lf: usize,
    /// Number of `\r\n` terminators.
    pub crlf: usize,
    /// Number of lone `\r` terminators.
    pub cr: usize,
}

impl LineEndingInfo {
    /// More than one style occurs.
    pub fn is_mixed(&self) -> bool {
        [self.lf, self.crlf, self.cr]
            .iter()
            .filter(|n| **n > 0)
            .count()
            > 1
    }

    /// The text has no line break at all.
    pub fn is_empty(&self) -> bool {
        self.lf + self.crlf + self.cr == 0
    }
}

/// Counts the terminators of `text` and picks the dominant style.
pub fn detect(text: &str) -> LineEndingInfo {
    let bytes = text.as_bytes();
    let (mut lf, mut crlf, mut cr) = (0, 0, 0);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => lf += 1,
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    crlf += 1;
                    i += 1;
                } else {
                    cr += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let dominant = if lf >= crlf && lf >= cr {
        LineEnding::Lf
    } else if crlf >= cr {
        LineEnding::CrLf
    } else {
        LineEnding::Cr
    };
    LineEndingInfo {
        dominant,
        lf,
        crlf,
        cr,
    }
}

/// Converts every terminator to `\n`.
pub fn normalize(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Converts `\n` terminators of `text` (already normalised) to `ending`.
pub fn apply(text: &str, ending: LineEnding) -> Cow<'_, str> {
    match ending {
        LineEnding::Lf => Cow::Borrowed(text),
        other => Cow::Owned(text.replace('\n', other.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_style() {
        assert_eq!(detect("a\nb\n").dominant, LineEnding::Lf);
        assert_eq!(detect("a\r\nb\r\n").dominant, LineEnding::CrLf);
        assert_eq!(detect("a\rb\r").dominant, LineEnding::Cr);
        assert!(detect("single line").is_empty());
        assert_eq!(detect("").dominant, LineEnding::Lf);
    }

    #[test]
    fn mixed_text_reports_counts_and_majority() {
        let info = detect("a\r\nb\r\nc\nd\r");
        assert!(info.is_mixed());
        assert_eq!((info.lf, info.crlf, info.cr), (1, 2, 1));
        assert_eq!(info.dominant, LineEnding::CrLf);
        assert!(!detect("a\r\nb\r\n").is_mixed());
    }

    #[test]
    fn crlf_is_one_terminator_not_two() {
        let info = detect("\r\n\r\n");
        assert_eq!((info.lf, info.crlf, info.cr), (0, 2, 0));
    }

    #[test]
    fn normalizes_and_restores() {
        assert_eq!(normalize("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert!(matches!(normalize("plain\n"), Cow::Borrowed(_)));
        assert_eq!(apply("a\nb\n", LineEnding::CrLf), "a\r\nb\r\n");
        assert_eq!(apply("a\nb", LineEnding::Cr), "a\rb");
        assert!(matches!(apply("a\n", LineEnding::Lf), Cow::Borrowed(_)));
    }

    #[test]
    fn round_trip_of_uniform_text_is_lossless() {
        let original = "x\r\ny\r\n\r\nz";
        let info = detect(original);
        let normal = normalize(original).into_owned();
        assert_eq!(apply(&normal, info.dominant), original);
    }
}
