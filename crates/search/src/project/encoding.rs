//! Encoding-aware reading and writing of text files.
//!
//! Detection order: BOM (UTF-8/16), valid UTF-8, then a configurable fallback
//! (default windows-1252, which decodes every byte). Writing back re-encodes
//! with the same encoding and BOM and refuses anything that would not
//! round-trip (unmappable characters, lossy decodes).

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use thiserror::Error;

/// An encoding plus whether the file starts with a BOM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEncoding {
    /// The encoding.
    pub encoding: &'static Encoding,
    /// The file starts with a byte order mark (kept on write).
    pub bom: bool,
}

impl TextEncoding {
    /// UTF-8 without BOM.
    pub const UTF8: TextEncoding = TextEncoding {
        encoding: UTF_8,
        bom: false,
    };

    /// The encoding's WHATWG name, e.g. `UTF-8`, `windows-1252`.
    pub fn name(&self) -> &'static str {
        self.encoding.name()
    }

    /// Looks up an encoding by a WHATWG label (`"utf-8"`, `"utf-16le"`,
    /// `"windows-1252"`, `"shift_jis"`, ...), without a BOM.
    pub fn for_name(label: &str) -> Option<Self> {
        Encoding::for_label(label.trim().as_bytes()).map(|encoding| Self {
            encoding,
            bom: false,
        })
    }
}

/// A decoded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The text (BOM removed). Line endings are untouched.
    pub text: String,
    /// The detected encoding.
    pub encoding: TextEncoding,
    /// Some bytes were malformed and replaced by U+FFFD: do not write back.
    pub lossy: bool,
}

/// Failure while encoding text back to bytes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// The text has characters the encoding cannot represent.
    #[error("the text contains characters that cannot be represented in {0}")]
    Unmappable(&'static str),
    /// The encoding cannot be written (stateful ISO-2022 variants).
    #[error("{0} cannot be written")]
    Unsupported(&'static str),
}

/// Quick binary sniff: a NUL byte in the first 8 KiB (and no UTF-16 BOM).
pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    let head = &bytes[..bytes.len().min(8192)];
    memchr::memchr(0, head).is_some()
}

/// Detects the encoding of `bytes` and decodes them. `fallback` is used when
/// the bytes are neither BOM-marked nor valid UTF-8.
pub fn decode(bytes: &[u8], fallback: &'static Encoding) -> Decoded {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let (text, had_errors) = enc.decode_without_bom_handling(&bytes[bom_len..]);
        return Decoded {
            text: text.into_owned(),
            encoding: TextEncoding {
                encoding: enc,
                bom: true,
            },
            lossy: had_errors,
        };
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Decoded {
            text: s.to_string(),
            encoding: TextEncoding::UTF8,
            lossy: false,
        };
    }
    let (text, had_errors) = fallback.decode_without_bom_handling(bytes);
    Decoded {
        text: text.into_owned(),
        encoding: TextEncoding {
            encoding: fallback,
            bom: false,
        },
        lossy: had_errors,
    }
}

/// Decodes `bytes` as `enc` (skipping a BOM when `enc.bom`).
pub fn decode_as(bytes: &[u8], enc: TextEncoding) -> Decoded {
    let skip = if enc.bom {
        if enc.encoding == UTF_8 { 3 } else { 2 }
    } else {
        0
    };
    let body = bytes.get(skip..).unwrap_or(&[]);
    let (text, had_errors) = enc.encoding.decode_without_bom_handling(body);
    Decoded {
        text: text.into_owned(),
        encoding: enc,
        lossy: had_errors,
    }
}

/// Encodes `text` as `enc` (BOM included when `enc.bom`).
pub fn encode(text: &str, enc: TextEncoding) -> Result<Vec<u8>, EncodeError> {
    let e = enc.encoding;
    let mut out: Vec<u8> = Vec::with_capacity(text.len() + 4);
    if e == UTF_8 {
        if enc.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    if e == UTF_16LE || e == UTF_16BE {
        let le = e == UTF_16LE;
        if enc.bom {
            out.extend_from_slice(if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for u in text.encode_utf16() {
            out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        return Ok(out);
    }
    let (bytes, used, unmappable) = e.encode(text);
    if used != e {
        return Err(EncodeError::Unsupported(e.name()));
    }
    if unmappable {
        return Err(EncodeError::Unmappable(e.name()));
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}

/// Whether decoding `bytes` as `decoded.encoding` and encoding again gives
/// the same bytes (so an edit-and-write does not disturb the rest of the file).
pub fn roundtrips(bytes: &[u8], decoded: &Decoded) -> bool {
    !decoded.lossy && encode(&decoded.text, decoded.encoding).is_ok_and(|b| b == bytes)
}

/// FNV-1a 64 bit hash of `bytes` (cheap content fingerprint, not a security hash).
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The default fallback encoding (windows-1252: decodes any byte).
pub fn default_fallback() -> &'static Encoding {
    WINDOWS_1252
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_plain_and_bom() {
        let d = decode("héllo".as_bytes(), WINDOWS_1252);
        assert_eq!(d.encoding, TextEncoding::UTF8);
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("héllo".as_bytes());
        let d = decode(&with_bom, WINDOWS_1252);
        assert_eq!(d.text, "héllo");
        assert!(d.encoding.bom);
        assert_eq!(encode(&d.text, d.encoding).unwrap(), with_bom);
        assert!(roundtrips(&with_bom, &d));
    }

    #[test]
    fn utf16_roundtrip() {
        for le in [true, false] {
            let enc = TextEncoding {
                encoding: if le { UTF_16LE } else { UTF_16BE },
                bom: true,
            };
            let bytes = encode("日本😀\r\nx", enc).unwrap();
            let d = decode(&bytes, WINDOWS_1252);
            assert_eq!(d.text, "日本😀\r\nx");
            assert_eq!(d.encoding, enc);
            assert!(roundtrips(&bytes, &d));
            assert!(!looks_binary(&bytes));
        }
    }

    #[test]
    fn fallback_and_unmappable() {
        let bytes = [b'c', b'a', b'f', 0xE9]; // latin-1 "café"
        let d = decode(&bytes, WINDOWS_1252);
        assert_eq!(d.text, "café");
        assert_eq!(d.encoding.name(), "windows-1252");
        assert!(roundtrips(&bytes, &d));
        assert_eq!(
            encode("日", d.encoding),
            Err(EncodeError::Unmappable("windows-1252"))
        );
        let sjis = encoding_rs::SHIFT_JIS;
        let b = encode(
            "日本語",
            TextEncoding {
                encoding: sjis,
                bom: false,
            },
        )
        .unwrap();
        assert_eq!(decode(&b, sjis).text, "日本語");
    }

    #[test]
    fn binary_sniff() {
        assert!(looks_binary(b"abc\0def"));
        assert!(!looks_binary(b"plain text"));
        assert!(!looks_binary(&[]));
    }

    #[test]
    fn decode_as_skips_bom() {
        let enc = TextEncoding {
            encoding: UTF_8,
            bom: true,
        };
        let d = decode_as(&[0xEF, 0xBB, 0xBF, b'a'], enc);
        assert_eq!(d.text, "a");
    }

    #[test]
    fn encoding_by_name() {
        let utf16 = TextEncoding::for_name("utf-16le").expect("utf-16le");
        assert_eq!(utf16.name(), "UTF-16LE");
        assert!(!utf16.bom);
        assert_eq!(
            TextEncoding::for_name("shift_jis").map(|e| e.name()),
            Some("Shift_JIS")
        );
        assert!(TextEncoding::for_name("not-an-encoding").is_none());
    }
}
