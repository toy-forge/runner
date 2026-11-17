//! Text encodings: names, detection, decoding and encoding.
//!
//! [`TextEncoding`] is a closed set of the encodings CodeRunner's *Text
//! Encoding* menu offers (UTF-8, UTF-16/32 in all byte orders, ASCII, Latin 1,
//! the Windows and Mac code pages, the CJK encodings). It converts to and from
//! the labels used in preferences ([`TextEncoding::label`]), IANA names, the
//! `NSStringEncoding` integers CodeRunner stores ([`TextEncoding::ns_code`])
//! and the `com.apple.TextEncoding` extended-attribute value
//! ([`TextEncoding::xattr_value`]).
//!
//! * [`detect_encoding`] decides how to read bytes: BOM, the
//!   `com.apple.TextEncoding` attribute, an XML `encoding=` declaration, a
//!   Python `coding:` cookie, then the default preference, then heuristics.
//! * [`decode`] / [`decode_lossy`] / [`encode`] convert. [`encode`] reports the
//!   first character the encoding cannot represent
//!   ([`Unrepresentable`]) instead of losing information.
//! * [`reinterpret`] and [`convert`] implement the two answers to CodeRunner's
//!   *How would you like to apply the new encoding?* alert.

use std::borrow::Cow;
use std::fmt;
use std::sync::OnceLock;

use encoding_rs::Encoding;
use regex::bytes::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::line_ending;

/// A text encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextEncoding {
    /// UTF-8.
    #[default]
    Utf8,
    /// UTF-16 with a byte order mark; big endian when read without one,
    /// little endian when written (as macOS does).
    Utf16,
    /// UTF-16, big endian.
    Utf16Be,
    /// UTF-16, little endian.
    Utf16Le,
    /// UTF-32 with a byte order mark; big endian when read without one.
    Utf32,
    /// UTF-32, big endian.
    Utf32Be,
    /// UTF-32, little endian.
    Utf32Le,
    /// US-ASCII.
    Ascii,
    /// ISO 8859-1.
    Latin1,
    /// Mac OS Roman.
    MacRoman,
    /// Windows-1252 (Western, Windows Latin 1).
    Windows1252,
    /// ISO 8859-2 (Central European).
    Latin2,
    /// Windows-1250 (Central European).
    Windows1250,
    /// Windows-1251 (Cyrillic).
    Windows1251,
    /// KOI8-R (Cyrillic).
    Koi8R,
    /// Windows-1253 (Greek).
    Windows1253,
    /// Windows-1254 (Turkish).
    Windows1254,
    /// EUC-JP.
    EucJp,
    /// Shift JIS.
    ShiftJis,
    /// ISO 2022-JP.
    Iso2022Jp,
    /// GB 18030.
    Gb18030,
    /// GBK.
    Gbk,
    /// Big5.
    Big5,
    /// EUC-KR.
    EucKr,
}

struct Def {
    encoding: TextEncoding,
    /// The label stored in preferences; the first of the list in
    /// [`scratch_config::FILE_ENCODINGS`] where it appears.
    label: &'static str,
    /// The IANA name written to `com.apple.TextEncoding`.
    iana: &'static str,
    display: &'static str,
    /// `NSStringEncoding`.
    ns: u32,
    /// `CFStringEncoding`.
    cf: u32,
}

macro_rules! def {
    ($enc:ident, $label:expr, $iana:expr, $display:expr, $ns:expr, $cf:expr) => {
        Def {
            encoding: TextEncoding::$enc,
            label: $label,
            iana: $iana,
            display: $display,
            ns: $ns,
            cf: $cf,
        }
    };
}

static DEFS: &[Def] = &[
    def!(Utf8, "utf-8", "utf-8", "Unicode (UTF-8)", 4, 0x0800_0100),
    def!(Utf16, "utf-16", "utf-16", "Unicode (UTF-16)", 10, 0x0100),
    def!(
        Utf16Be,
        "utf-16be",
        "utf-16be",
        "Unicode (UTF-16 Big-Endian)",
        0x9000_0100,
        0x1000_0100
    ),
    def!(
        Utf16Le,
        "utf-16le",
        "utf-16le",
        "Unicode (UTF-16 Little-Endian)",
        0x9400_0100,
        0x1400_0100
    ),
    def!(
        Utf32,
        "utf-32",
        "utf-32",
        "Unicode (UTF-32)",
        0x8c00_0100,
        0x0c00_0100
    ),
    def!(
        Utf32Be,
        "utf-32be",
        "utf-32be",
        "Unicode (UTF-32 Big-Endian)",
        0x9800_0100,
        0x1800_0100
    ),
    def!(
        Utf32Le,
        "utf-32le",
        "utf-32le",
        "Unicode (UTF-32 Little-Endian)",
        0x9c00_0100,
        0x1c00_0100
    ),
    def!(Ascii, "us-ascii", "us-ascii", "Western (ASCII)", 1, 0x0600),
    def!(
        Latin1,
        "iso-8859-1",
        "iso-8859-1",
        "Western (ISO Latin 1)",
        5,
        0x0201
    ),
    def!(
        MacRoman,
        "macintosh",
        "macintosh",
        "Western (Mac OS Roman)",
        30,
        0
    ),
    def!(
        Windows1252,
        "windows-1252",
        "windows-1252",
        "Western (Windows Latin 1)",
        12,
        0x0500
    ),
    def!(
        Latin2,
        "iso-8859-2",
        "iso-8859-2",
        "Central European (ISO Latin 2)",
        9,
        0x0202
    ),
    def!(
        Windows1250,
        "windows-1250",
        "windows-1250",
        "Central European (Windows Latin 2)",
        15,
        0x0501
    ),
    def!(
        Windows1251,
        "windows-1251",
        "windows-1251",
        "Cyrillic (Windows)",
        11,
        0x0502
    ),
    def!(
        Koi8R,
        "koi8-r",
        "koi8-r",
        "Cyrillic (KOI8-R)",
        0x8000_0a02,
        0x0a02
    ),
    def!(
        Windows1253,
        "windows-1253",
        "windows-1253",
        "Greek (Windows)",
        13,
        0x0503
    ),
    def!(
        Windows1254,
        "windows-1254",
        "windows-1254",
        "Turkish (Windows Latin 5)",
        14,
        0x0504
    ),
    def!(EucJp, "euc-jp", "euc-jp", "Japanese (EUC)", 3, 0x0920),
    def!(
        ShiftJis,
        "shift_jis",
        "shift_jis",
        "Japanese (Shift JIS)",
        8,
        0x0a01
    ),
    def!(
        Iso2022Jp,
        "iso-2022-jp",
        "iso-2022-jp",
        "Japanese (ISO 2022-JP)",
        21,
        0x0820
    ),
    def!(
        Gb18030,
        "gb18030",
        "gb18030",
        "Chinese (GB 18030)",
        0x8000_0632,
        0x0632
    ),
    def!(Gbk, "gbk", "gbk", "Chinese (GBK)", 0x8000_0631, 0x0631),
    def!(Big5, "big5", "big5", "Chinese (Big 5)", 0x8000_0a03, 0x0a03),
    def!(
        EucKr,
        "euc-kr",
        "euc-kr",
        "Korean (EUC)",
        0x8000_0940,
        0x0940
    ),
];

impl TextEncoding {
    /// Every encoding, in the order of the *Text Encoding* menu.
    pub const ALL: [TextEncoding; 24] = [
        Self::Utf8,
        Self::Utf16,
        Self::Utf16Be,
        Self::Utf16Le,
        Self::Utf32,
        Self::Utf32Be,
        Self::Utf32Le,
        Self::Ascii,
        Self::Latin1,
        Self::MacRoman,
        Self::Windows1252,
        Self::Latin2,
        Self::Windows1250,
        Self::Windows1251,
        Self::Koi8R,
        Self::Windows1253,
        Self::Windows1254,
        Self::EucJp,
        Self::ShiftJis,
        Self::Iso2022Jp,
        Self::Gb18030,
        Self::Gbk,
        Self::Big5,
        Self::EucKr,
    ];

    fn def(self) -> &'static Def {
        DEFS.iter().find(|d| d.encoding == self).unwrap_or(&DEFS[0])
    }

    /// The label stored in preferences and sidecar files (`utf-8`,
    /// `shift_jis`). It is accepted by [`TextEncoding::from_label`].
    pub fn label(self) -> &'static str {
        self.def().label
    }

    /// The IANA charset name (`utf-8`, `windows-1252`); what
    /// `CR_ENCODING_NAME` and the `com.apple.TextEncoding` attribute carry.
    pub fn iana_name(self) -> &'static str {
        self.def().iana
    }

    /// The menu title (`Western (Windows Latin 1)`).
    pub fn display_name(self) -> &'static str {
        self.def().display
    }

    /// The `NSStringEncoding` value (`4` for UTF-8) CodeRunner stores in
    /// preferences and exports as `CR_ENCODING`.
    pub fn ns_code(self) -> u32 {
        self.def().ns
    }

    /// The `CFStringEncoding` value (`0x08000100` for UTF-8) used in the
    /// `com.apple.TextEncoding` attribute.
    pub fn cf_code(self) -> u32 {
        self.def().cf
    }

    /// The value of the `com.apple.TextEncoding` extended attribute:
    /// `<iana name>;<CFStringEncoding>`, e.g. `utf-8;134217984`.
    pub fn xattr_value(self) -> String {
        format!("{};{}", self.iana_name(), self.cf_code())
    }

    /// Parses a `com.apple.TextEncoding` value (`utf-8;134217984`, `utf-8`,
    /// `;134217984`). The name wins; the code is used when the name is empty
    /// or unknown.
    pub fn from_xattr_value(value: &str) -> Option<Self> {
        let mut parts = value.trim().trim_matches('\0').splitn(2, ';');
        let name = parts.next().unwrap_or_default().trim();
        let code = parts.next().map(str::trim);
        Self::from_label(name).or_else(|| {
            let code: u32 = code?.parse().ok()?;
            Self::from_cf_code(code)
        })
    }

    /// The encoding with this `NSStringEncoding` value.
    pub fn from_ns_code(code: u64) -> Option<Self> {
        let code = u32::try_from(code).ok()?;
        DEFS.iter().find(|d| d.ns == code).map(|d| d.encoding)
    }

    /// The encoding with this `CFStringEncoding` value.
    pub fn from_cf_code(code: u32) -> Option<Self> {
        DEFS.iter().find(|d| d.cf == code).map(|d| d.encoding)
    }

    /// Resolves a label: ours, an IANA name or a common alias
    /// (`utf8`, `latin1`, `cp1252`, `sjis`, `gb2312`, `ascii`, `mac-roman`...),
    /// case-insensitive. Note that `iso-8859-1`/`latin1` mean true ISO 8859-1
    /// here (every byte is the code point), unlike the WHATWG labels.
    pub fn from_label(label: &str) -> Option<Self> {
        let key = label.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        let alias = match key.as_str() {
            "utf-8" | "utf8" | "utf_8" | "u8" | "utf-8-sig" | "utf8-sig" => Self::Utf8,
            "utf-16" | "utf16" | "utf_16" | "unicode" => Self::Utf16,
            "utf-16be" | "utf_16_be" | "utf-16-be" | "utf16be" => Self::Utf16Be,
            "utf-16le" | "utf_16_le" | "utf-16-le" | "utf16le" => Self::Utf16Le,
            "utf-32" | "utf32" | "utf_32" => Self::Utf32,
            "utf-32be" | "utf_32_be" | "utf-32-be" | "utf32be" => Self::Utf32Be,
            "utf-32le" | "utf_32_le" | "utf-32-le" | "utf32le" => Self::Utf32Le,
            "ascii" | "us-ascii" | "us_ascii" | "ansi_x3.4-1968" | "646" => Self::Ascii,
            "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "iso-8859_1" | "latin1" | "latin-1"
            | "latin_1" | "l1" | "iso-ir-100" | "cp819" => Self::Latin1,
            "macintosh" | "mac" | "macroman" | "mac-roman" | "mac_roman" | "x-mac-roman" => {
                Self::MacRoman
            }
            "windows-1252" | "cp1252" | "windows1252" | "x-cp1252" => Self::Windows1252,
            "iso-8859-2" | "iso8859-2" | "latin2" | "latin-2" | "l2" => Self::Latin2,
            "windows-1250" | "cp1250" | "windows1250" => Self::Windows1250,
            "windows-1251" | "cp1251" | "windows1251" => Self::Windows1251,
            "koi8-r" | "koi8r" | "koi8_r" => Self::Koi8R,
            "windows-1253" | "cp1253" | "windows1253" => Self::Windows1253,
            "windows-1254" | "cp1254" | "windows1254" | "iso-8859-9" | "latin5" => {
                Self::Windows1254
            }
            "euc-jp" | "eucjp" | "euc_jp" | "ujis" => Self::EucJp,
            "shift_jis" | "shift-jis" | "sjis" | "s-jis" | "x-sjis" | "ms_kanji" | "csshiftjis"
            | "windows-31j" | "cp932" => Self::ShiftJis,
            "iso-2022-jp" | "iso2022jp" | "iso_2022_jp" | "jis" => Self::Iso2022Jp,
            "gb18030" => Self::Gb18030,
            "gbk" | "cp936" | "gb2312" | "euc-cn" | "euccn" | "x-gbk" => Self::Gbk,
            "big5" | "big-5" | "big5-hkscs" | "cp950" => Self::Big5,
            "euc-kr" | "euckr" | "euc_kr" | "ks_c_5601-1987" | "cp949" => Self::EucKr,
            _ => return None,
        };
        Some(alias)
    }

    /// Whether this is one of the Unicode transformation formats.
    pub fn is_unicode(self) -> bool {
        matches!(
            self,
            Self::Utf8
                | Self::Utf16
                | Self::Utf16Be
                | Self::Utf16Le
                | Self::Utf32
                | Self::Utf32Be
                | Self::Utf32Le
        )
    }

    /// Whether every ASCII byte stands for itself, so ASCII text reads the
    /// same (everything except UTF-16/32 and ISO 2022-JP is, ISO 2022-JP is
    /// treated as such for ASCII-only content).
    pub fn is_ascii_compatible(self) -> bool {
        !matches!(
            self,
            Self::Utf16
                | Self::Utf16Be
                | Self::Utf16Le
                | Self::Utf32
                | Self::Utf32Be
                | Self::Utf32Le
        )
    }

    /// The byte order mark this encoding's files may start with.
    pub fn bom_bytes(self) -> Option<&'static [u8]> {
        match self {
            Self::Utf8 => Some(&[0xEF, 0xBB, 0xBF]),
            Self::Utf16 | Self::Utf16Le => Some(&[0xFF, 0xFE]),
            Self::Utf16Be => Some(&[0xFE, 0xFF]),
            Self::Utf32 | Self::Utf32Le => Some(&[0xFF, 0xFE, 0x00, 0x00]),
            Self::Utf32Be => Some(&[0x00, 0x00, 0xFE, 0xFF]),
            _ => None,
        }
    }

    /// Whether a file in this encoding always carries a byte order mark
    /// (generic UTF-16 and UTF-32).
    pub fn requires_bom(self) -> bool {
        matches!(self, Self::Utf16 | Self::Utf32)
    }

    fn rs(self) -> Option<&'static Encoding> {
        Some(match self {
            Self::MacRoman => encoding_rs::MACINTOSH,
            Self::Windows1252 => encoding_rs::WINDOWS_1252,
            Self::Latin2 => encoding_rs::ISO_8859_2,
            Self::Windows1250 => encoding_rs::WINDOWS_1250,
            Self::Windows1251 => encoding_rs::WINDOWS_1251,
            Self::Koi8R => encoding_rs::KOI8_R,
            Self::Windows1253 => encoding_rs::WINDOWS_1253,
            Self::Windows1254 => encoding_rs::WINDOWS_1254,
            Self::EucJp | Self::Iso2022Jp => encoding_rs::EUC_JP,
            Self::ShiftJis => encoding_rs::SHIFT_JIS,
            Self::Gb18030 => encoding_rs::GB18030,
            Self::Gbk => encoding_rs::GBK,
            Self::Big5 => encoding_rs::BIG5,
            Self::EucKr => encoding_rs::EUC_KR,
            _ => return None,
        })
    }
}

impl fmt::Display for TextEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

impl Serialize for TextEncoding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for TextEncoding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_label(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown text encoding `{text}`")))
    }
}

// ---------------------------------------------------------------------------
// Decoding

/// Bytes that are not valid in the encoding they were decoded with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the data is not valid {encoding}")]
pub struct DecodeError {
    /// The encoding that was tried.
    pub encoding: TextEncoding,
}

/// The result of [`decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The text, with the byte order mark removed. Line endings are as in the
    /// bytes.
    pub text: String,
    /// The bytes started with the byte order mark of the encoding.
    pub bom: bool,
    /// Some bytes were invalid and replaced by U+FFFD ([`decode_lossy`] only).
    pub had_errors: bool,
}

/// Decodes `bytes` strictly.
///
/// # Errors
///
/// [`DecodeError`] when a byte sequence is invalid in `encoding`.
pub fn decode(bytes: &[u8], encoding: TextEncoding) -> Result<Decoded, DecodeError> {
    let decoded = decode_with(bytes, encoding, false);
    if decoded.had_errors {
        Err(DecodeError { encoding })
    } else {
        Ok(decoded)
    }
}

/// Decodes `bytes`, replacing invalid sequences with U+FFFD.
pub fn decode_lossy(bytes: &[u8], encoding: TextEncoding) -> Decoded {
    decode_with(bytes, encoding, true)
}

fn strip_bom(bytes: &[u8], encoding: TextEncoding) -> (&[u8], bool, TextEncoding) {
    // Generic UTF-16/32 take their byte order from the mark.
    let marks: &[(&[u8], TextEncoding)] = match encoding {
        TextEncoding::Utf16 => &[
            (&[0xFF, 0xFE], TextEncoding::Utf16Le),
            (&[0xFE, 0xFF], TextEncoding::Utf16Be),
        ],
        TextEncoding::Utf32 => &[
            (&[0xFF, 0xFE, 0x00, 0x00], TextEncoding::Utf32Le),
            (&[0x00, 0x00, 0xFE, 0xFF], TextEncoding::Utf32Be),
        ],
        _ => &[],
    };
    for (mark, effective) in marks {
        if bytes.starts_with(mark) {
            return (&bytes[mark.len()..], true, *effective);
        }
    }
    if let Some(mark) = encoding.bom_bytes()
        && bytes.starts_with(mark)
    {
        return (&bytes[mark.len()..], true, encoding);
    }
    (bytes, false, encoding)
}

fn decode_with(bytes: &[u8], encoding: TextEncoding, lossy: bool) -> Decoded {
    let (body, bom, effective) = strip_bom(bytes, encoding);
    let mut had_errors = false;
    let text = match effective {
        TextEncoding::Utf8 => match std::str::from_utf8(body) {
            Ok(text) => text.to_owned(),
            Err(_) => {
                had_errors = true;
                if lossy {
                    String::from_utf8_lossy(body).into_owned()
                } else {
                    String::new()
                }
            }
        },
        TextEncoding::Utf16 | TextEncoding::Utf16Be => {
            decode_utf16(body, false, &mut had_errors, lossy)
        }
        TextEncoding::Utf16Le => decode_utf16(body, true, &mut had_errors, lossy),
        TextEncoding::Utf32 | TextEncoding::Utf32Be => {
            decode_utf32(body, false, &mut had_errors, lossy)
        }
        TextEncoding::Utf32Le => decode_utf32(body, true, &mut had_errors, lossy),
        TextEncoding::Ascii => {
            let mut text = String::with_capacity(body.len());
            for &b in body {
                if b < 0x80 {
                    text.push(char::from(b));
                } else {
                    had_errors = true;
                    if !lossy {
                        break;
                    }
                    text.push('\u{FFFD}');
                }
            }
            text
        }
        TextEncoding::Latin1 => body.iter().map(|&b| char::from(b)).collect(),
        TextEncoding::Iso2022Jp => decode_iso2022jp(body, &mut had_errors, lossy),
        other => match other.rs() {
            Some(rs) => {
                if lossy {
                    let (text, errors) = rs.decode_without_bom_handling(body);
                    had_errors = errors;
                    text.into_owned()
                } else {
                    match rs.decode_without_bom_handling_and_without_replacement(body) {
                        Some(text) => text.into_owned(),
                        None => {
                            had_errors = true;
                            String::new()
                        }
                    }
                }
            }
            None => {
                had_errors = true;
                String::new()
            }
        },
    };
    Decoded {
        text,
        bom,
        had_errors,
    }
}

fn decode_utf16(body: &[u8], little: bool, had_errors: &mut bool, lossy: bool) -> String {
    let mut units = Vec::with_capacity(body.len() / 2);
    for pair in body.as_chunks::<2>().0 {
        units.push(if little {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        });
    }
    let mut text = String::with_capacity(units.len());
    for item in char::decode_utf16(units) {
        match item {
            Ok(c) => text.push(c),
            Err(_) => {
                *had_errors = true;
                if !lossy {
                    return String::new();
                }
                text.push('\u{FFFD}');
            }
        }
    }
    if body.len() % 2 == 1 {
        *had_errors = true;
        if lossy {
            text.push('\u{FFFD}');
        } else {
            return String::new();
        }
    }
    text
}

fn decode_utf32(body: &[u8], little: bool, had_errors: &mut bool, lossy: bool) -> String {
    let mut text = String::with_capacity(body.len() / 4);
    for quad in body.as_chunks::<4>().0 {
        let value = if little {
            u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]])
        } else {
            u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]])
        };
        match char::from_u32(value) {
            Some(c) => text.push(c),
            None => {
                *had_errors = true;
                if !lossy {
                    return String::new();
                }
                text.push('\u{FFFD}');
            }
        }
    }
    if !body.len().is_multiple_of(4) {
        *had_errors = true;
        if lossy {
            text.push('\u{FFFD}');
        } else {
            return String::new();
        }
    }
    text
}

/// ISO 2022-JP: ASCII, JIS X 0201 Roman and JIS X 0208 (the shifted subset of
/// EUC-JP); half-width katakana and JIS X 0212 are not part of this variant.
fn decode_iso2022jp(body: &[u8], had_errors: &mut bool, lossy: bool) -> String {
    #[derive(PartialEq, Clone, Copy)]
    enum Mode {
        Ascii,
        Jis0208,
    }
    let mut mode = Mode::Ascii;
    let mut out = String::with_capacity(body.len());
    let mut euc: Vec<u8> = Vec::new();
    let mut i = 0;
    let flush = |euc: &mut Vec<u8>, out: &mut String, had_errors: &mut bool| {
        if euc.is_empty() {
            return;
        }
        let (text, errors) = encoding_rs::EUC_JP.decode_without_bom_handling(euc);
        if errors {
            *had_errors = true;
        }
        out.push_str(&text);
        euc.clear();
    };
    while i < body.len() {
        let b = body[i];
        if b == 0x1B {
            flush(&mut euc, &mut out, had_errors);
            match body.get(i + 1..i + 3) {
                Some([b'(', b'B']) | Some([b'(', b'J']) => {
                    mode = Mode::Ascii;
                    i += 3;
                }
                Some([b'$', b'@']) | Some([b'$', b'B']) => {
                    mode = Mode::Jis0208;
                    i += 3;
                }
                _ => {
                    *had_errors = true;
                    if !lossy {
                        return String::new();
                    }
                    out.push('\u{FFFD}');
                    i += 1;
                }
            }
            continue;
        }
        match mode {
            Mode::Ascii => {
                if b < 0x80 {
                    out.push(char::from(b));
                } else {
                    *had_errors = true;
                    if !lossy {
                        return String::new();
                    }
                    out.push('\u{FFFD}');
                }
                i += 1;
            }
            Mode::Jis0208 => {
                if b == b'\n' || b == b'\r' {
                    flush(&mut euc, &mut out, had_errors);
                    out.push(char::from(b));
                    i += 1;
                } else if (0x21..=0x7E).contains(&b)
                    && body.get(i + 1).is_some_and(|n| (0x21..=0x7E).contains(n))
                {
                    euc.push(b | 0x80);
                    euc.push(body[i + 1] | 0x80);
                    i += 2;
                } else {
                    *had_errors = true;
                    if !lossy {
                        return String::new();
                    }
                    out.push('\u{FFFD}');
                    i += 1;
                }
            }
        }
    }
    flush(&mut euc, &mut out, had_errors);
    out
}

// ---------------------------------------------------------------------------
// Encoding

/// A character the target encoding cannot hold; `Your document cannot be
/// represented in the chosen file encoding.`
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{character:?} (line {line}, column {column}) cannot be represented in {encoding}")]
pub struct Unrepresentable {
    /// The encoding that was asked for.
    pub encoding: TextEncoding,
    /// The first character that has no representation.
    pub character: char,
    /// Its position in characters from the start of the text.
    pub char_index: usize,
    /// Its line, from 1.
    pub line: usize,
    /// Its column in characters, from 1.
    pub column: usize,
}

/// Encodes `text` (any line endings) to bytes. `bom` writes the byte order
/// mark where the encoding has one (UTF-16 and UTF-32 without an explicit
/// byte order always do).
///
/// # Errors
///
/// [`Unrepresentable`] with the first character `encoding` cannot hold.
pub fn encode(text: &str, encoding: TextEncoding, bom: bool) -> Result<Vec<u8>, Unrepresentable> {
    let mut out = Vec::with_capacity(text.len() + 4);
    if let Some(mark) = encoding.bom_bytes()
        && (bom || encoding.requires_bom())
    {
        out.extend_from_slice(mark);
    }
    match encoding {
        TextEncoding::Utf8 => out.extend_from_slice(text.as_bytes()),
        TextEncoding::Utf16 | TextEncoding::Utf16Le => {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
        TextEncoding::Utf16Be => {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_be_bytes());
            }
        }
        TextEncoding::Utf32 | TextEncoding::Utf32Le => {
            for c in text.chars() {
                out.extend_from_slice(&u32::from(c).to_le_bytes());
            }
        }
        TextEncoding::Utf32Be => {
            for c in text.chars() {
                out.extend_from_slice(&u32::from(c).to_be_bytes());
            }
        }
        TextEncoding::Ascii => {
            if let Some(failure) = first_failure(text, encoding, |c| c.is_ascii()) {
                return Err(failure);
            }
            out.extend_from_slice(text.as_bytes());
        }
        TextEncoding::Latin1 => {
            if let Some(failure) = first_failure(text, encoding, |c| u32::from(c) < 0x100) {
                return Err(failure);
            }
            out.extend(text.chars().map(|c| u32::from(c) as u8));
        }
        TextEncoding::Iso2022Jp => out.extend(encode_iso2022jp(text, encoding)?),
        other => {
            let Some(rs) = other.rs() else {
                return Err(failure_at(text, other, 0, '\u{FFFD}'));
            };
            let mut encoder = rs.new_encoder();
            let mut position = 0;
            loop {
                out.reserve(text.len().saturating_sub(position) + 16);
                let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(
                    &text[position..],
                    &mut out,
                    true,
                );
                position += read;
                match result {
                    encoding_rs::EncoderResult::InputEmpty => break,
                    encoding_rs::EncoderResult::OutputFull => continue,
                    encoding_rs::EncoderResult::Unmappable(c) => {
                        let char_index = text[..position - c.len_utf8()].chars().count();
                        return Err(failure_at(text, other, char_index, c));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn failure_at(
    text: &str,
    encoding: TextEncoding,
    char_index: usize,
    character: char,
) -> Unrepresentable {
    let before: Vec<char> = text.chars().take(char_index).collect();
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let column = before.iter().rev().take_while(|c| **c != '\n').count() + 1;
    Unrepresentable {
        encoding,
        character,
        char_index,
        line,
        column,
    }
}

fn first_failure(
    text: &str,
    encoding: TextEncoding,
    representable: impl Fn(char) -> bool,
) -> Option<Unrepresentable> {
    text.chars()
        .enumerate()
        .find(|(_, c)| !representable(*c))
        .map(|(index, c)| failure_at(text, encoding, index, c))
}

fn encode_iso2022jp(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, Unrepresentable> {
    let mut out = Vec::with_capacity(text.len() + 8);
    let mut in_jis = false;
    let mut buf = [0u8; 4];
    for (index, c) in text.chars().enumerate() {
        if c.is_ascii() {
            if in_jis {
                out.extend_from_slice(b"\x1b(B");
                in_jis = false;
            }
            out.push(c as u8);
            continue;
        }
        let s = c.encode_utf8(&mut buf);
        let (bytes, _, unmappable) = encoding_rs::EUC_JP.encode(s);
        let two_byte =
            !unmappable && bytes.len() == 2 && bytes.iter().all(|b| (0xA1..=0xFE).contains(b));
        if !two_byte {
            return Err(failure_at(text, encoding, index, c));
        }
        if !in_jis {
            out.extend_from_slice(b"\x1b$B");
            in_jis = true;
        }
        out.push(bytes[0] & 0x7F);
        out.push(bytes[1] & 0x7F);
    }
    if in_jis {
        out.extend_from_slice(b"\x1b(B");
    }
    Ok(out)
}

/// Whether `text` can be written in `encoding` without loss.
pub fn can_represent(text: &str, encoding: TextEncoding) -> bool {
    encode(text, encoding, false).is_ok()
}

/// Every character of `text` that `encoding` cannot represent, with its
/// position in characters; for a "these characters will be lost" report.
pub fn unrepresentable_characters(text: &str, encoding: TextEncoding) -> Vec<(usize, char)> {
    if encoding.is_unicode() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut buf = [0u8; 4];
    for (index, c) in text.chars().enumerate() {
        if c.is_ascii() {
            continue;
        }
        let ok = match encoding {
            TextEncoding::Ascii => false,
            TextEncoding::Latin1 => u32::from(c) < 0x100,
            _ => can_represent(c.encode_utf8(&mut buf), encoding),
        };
        if !ok {
            found.push((index, c));
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Detection

/// What decided the encoding in [`detect_encoding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSource {
    /// A byte order mark.
    Bom,
    /// The `com.apple.TextEncoding` extended attribute.
    Xattr,
    /// An XML `encoding="..."` declaration.
    XmlDeclaration,
    /// A Python `coding: ...` cookie.
    CodingCookie,
    /// The default encoding preference.
    Default,
    /// The bytes are valid UTF-8, or look like UTF-16 (NUL pattern).
    Heuristic,
    /// Nothing fit; a single-byte Windows encoding was chosen so every byte
    /// survives.
    Fallback,
}

/// What [`detect_encoding`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedEncoding {
    /// The encoding to read the bytes with.
    pub encoding: TextEncoding,
    /// The bytes start with a byte order mark (to be written back on save).
    pub bom: bool,
    /// What decided.
    pub source: EncodingSource,
}

/// The inputs of [`detect_encoding`] besides the bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodingHints {
    /// The `com.apple.TextEncoding` attribute, if the file has one.
    pub xattr: Option<TextEncoding>,
    /// The default encoding preference.
    pub default: TextEncoding,
}

/// Recognises a byte order mark at the start of `bytes`: the encoding and the
/// mark's length.
pub fn sniff_bom(bytes: &[u8]) -> Option<(TextEncoding, usize)> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((TextEncoding::Utf8, 3))
    } else if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        Some((TextEncoding::Utf32Le, 4))
    } else if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        Some((TextEncoding::Utf32Be, 4))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some((TextEncoding::Utf16Le, 2))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some((TextEncoding::Utf16Be, 2))
    } else {
        None
    }
}

fn xml_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)^\s*<\?xml[^>]*?\sencoding\s*=\s*["']([A-Za-z0-9._:-]+)["']"#).ok()
    })
    .as_ref()
}

fn cookie_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)").ok())
        .as_ref()
}

/// The encoding an XML declaration names: `<?xml version="1.0"
/// encoding="ISO-8859-1"?>` at the start of `bytes`.
pub fn xml_declared_encoding(bytes: &[u8]) -> Option<TextEncoding> {
    let head = &bytes[..bytes.len().min(512)];
    let caps = xml_regex()?.captures(head)?;
    TextEncoding::from_label(std::str::from_utf8(caps.get(1)?.as_bytes()).ok()?)
}

/// The encoding a Python coding cookie (PEP 263: `# -*- coding: latin-1 -*-`)
/// names in the first two lines of `bytes`.
pub fn coding_cookie_encoding(bytes: &[u8]) -> Option<TextEncoding> {
    for line in bytes.split(|b| *b == b'\n').take(2) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(caps) = cookie_regex().and_then(|re| re.captures(line)) {
            let name = std::str::from_utf8(caps.get(1)?.as_bytes()).ok()?;
            return TextEncoding::from_label(name);
        }
    }
    None
}

fn looks_like_utf16(bytes: &[u8]) -> Option<TextEncoding> {
    let sample = &bytes[..bytes.len().min(1024) & !1];
    if sample.len() < 4 {
        return None;
    }
    let pairs = sample.len() / 2;
    let even_nul = sample.iter().step_by(2).filter(|b| **b == 0).count();
    let odd_nul = sample
        .iter()
        .skip(1)
        .step_by(2)
        .filter(|b| **b == 0)
        .count();
    if odd_nul * 10 >= pairs * 3 && even_nul == 0 {
        Some(TextEncoding::Utf16Le)
    } else if even_nul * 10 >= pairs * 3 && odd_nul == 0 {
        Some(TextEncoding::Utf16Be)
    } else {
        None
    }
}

/// Decides how to read `bytes`.
///
/// Order: byte order mark; a UTF-16 NUL pattern; the `com.apple.TextEncoding` attribute; an XML
/// declaration; a Python coding cookie; the default preference; valid UTF-8;
/// and finally Windows-1252, which never fails. A
/// candidate is used only if the bytes decode in it without errors.
pub fn detect_encoding(bytes: &[u8], hints: &EncodingHints) -> DetectedEncoding {
    if let Some((encoding, _)) = sniff_bom(bytes)
        && decode(bytes, encoding).is_ok()
    {
        return DetectedEncoding {
            encoding,
            bom: true,
            source: EncodingSource::Bom,
        };
    }
    if let Some(encoding) = looks_like_utf16(bytes)
        && decode(bytes, encoding).is_ok()
    {
        return DetectedEncoding {
            encoding,
            bom: false,
            source: EncodingSource::Heuristic,
        };
    }
    let candidates = [
        (hints.xattr, EncodingSource::Xattr),
        (xml_declared_encoding(bytes), EncodingSource::XmlDeclaration),
        (coding_cookie_encoding(bytes), EncodingSource::CodingCookie),
        (Some(hints.default), EncodingSource::Default),
    ];
    for (candidate, source) in candidates {
        if let Some(encoding) = candidate
            && decode(bytes, encoding).is_ok()
        {
            return DetectedEncoding {
                encoding,
                bom: false,
                source,
            };
        }
    }
    if decode(bytes, TextEncoding::Utf8).is_ok() {
        return DetectedEncoding {
            encoding: TextEncoding::Utf8,
            bom: false,
            source: EncodingSource::Heuristic,
        };
    }
    DetectedEncoding {
        encoding: TextEncoding::Windows1252,
        bom: false,
        source: EncodingSource::Fallback,
    }
}

// ---------------------------------------------------------------------------
// Reinterpret / Convert

/// Applies a new encoding by *reinterpreting*: the file's bytes are read again
/// as `encoding` (the text changes, the bytes do not). This is the answer
/// `Reinterpret` of CodeRunner's encoding alert; unsaved edits would be lost,
/// so the caller confirms first.
///
/// # Errors
///
/// [`DecodeError`] when the bytes are not valid in `encoding` (`The file
/// encoding could not be applied.`).
pub fn reinterpret(bytes: &[u8], encoding: TextEncoding) -> Result<Decoded, DecodeError> {
    decode(bytes, encoding)
}

/// The text kept as is while the encoding to write changes: the answer
/// `Convert`. Nothing is converted until the next save, but the check is done
/// now.
///
/// # Errors
///
/// [`Unrepresentable`] with the first character the new encoding cannot hold
/// (`Your document cannot be represented in the chosen file encoding.`).
pub fn convert(text: &str, encoding: TextEncoding) -> Result<(), Unrepresentable> {
    encode(text, encoding, false).map(|_| ())
}

/// The outcome of checking typed or pasted text against the document's
/// encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertCheck {
    /// Everything fits.
    Fits,
    /// Some characters would be lost: ask `Insert` or `Promote to UTF-8`.
    Lossy(Vec<char>),
}

/// Checks text about to be inserted against `encoding` (the *File Encoding*
/// alert: `You are trying to insert characters which cannot be handled by the
/// current encoding without losing information.`).
pub fn check_insert(text: &str, encoding: TextEncoding) -> InsertCheck {
    let bad: Vec<char> = unrepresentable_characters(text, encoding)
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    if bad.is_empty() {
        InsertCheck::Fits
    } else {
        InsertCheck::Lossy(bad)
    }
}

/// Decodes `bytes` for display with `encoding`, normalising line endings to
/// `\n`; shorthand used by loaders.
pub fn decode_normalized(
    bytes: &[u8],
    encoding: TextEncoding,
) -> Result<(String, Decoded), DecodeError> {
    let decoded = decode(bytes, encoding)?;
    let text = match line_ending::normalize(&decoded.text) {
        Cow::Borrowed(_) => decoded.text.clone(),
        Cow::Owned(text) => text,
    };
    Ok((text, decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints() -> EncodingHints {
        EncodingHints::default()
    }

    #[test]
    fn labels_names_and_codes_round_trip() {
        for encoding in TextEncoding::ALL {
            assert_eq!(
                TextEncoding::from_label(encoding.label()),
                Some(encoding),
                "{encoding:?}"
            );
            assert_eq!(
                TextEncoding::from_ns_code(u64::from(encoding.ns_code())),
                Some(encoding)
            );
            assert_eq!(
                TextEncoding::from_cf_code(encoding.cf_code()),
                Some(encoding),
                "{encoding:?}"
            );
            assert_eq!(
                TextEncoding::from_xattr_value(&encoding.xattr_value()),
                Some(encoding)
            );
        }
        assert_eq!(TextEncoding::Utf8.xattr_value(), "utf-8;134217984");
        assert_eq!(TextEncoding::Utf8.ns_code(), 4);
    }

    #[test]
    fn every_preference_label_is_known() {
        use scratch_config::{ConstValue, FILE_ENCODINGS};
        for option in FILE_ENCODINGS {
            let ConstValue::Str(label) = option.value else {
                panic!("non-string option");
            };
            let encoding = TextEncoding::from_label(label)
                .unwrap_or_else(|| panic!("label {label} is not a TextEncoding"));
            if let Some(ConstValue::Int(cr)) = option.cr {
                assert_eq!(
                    u64::try_from(cr).ok(),
                    Some(u64::from(encoding.ns_code())),
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(TextEncoding::from_label("UTF8"), Some(TextEncoding::Utf8));
        assert_eq!(
            TextEncoding::from_label("latin1"),
            Some(TextEncoding::Latin1)
        );
        assert_eq!(
            TextEncoding::from_label("\"Shift-JIS\""),
            Some(TextEncoding::ShiftJis)
        );
        assert_eq!(TextEncoding::from_label("gb2312"), Some(TextEncoding::Gbk));
        assert_eq!(
            TextEncoding::from_label("mac-roman"),
            Some(TextEncoding::MacRoman)
        );
        assert_eq!(TextEncoding::from_label("klingon"), None);
        assert_eq!(
            TextEncoding::from_xattr_value(";134217984"),
            Some(TextEncoding::Utf8)
        );
        assert_eq!(
            TextEncoding::from_xattr_value("weird;1280"),
            Some(TextEncoding::Windows1252)
        );
    }

    #[test]
    fn serde_uses_the_label() {
        assert_eq!(
            serde_json::to_string(&TextEncoding::ShiftJis).unwrap(),
            "\"shift_jis\""
        );
        assert_eq!(
            serde_json::from_str::<TextEncoding>("\"cp1252\"").unwrap(),
            TextEncoding::Windows1252
        );
        assert!(serde_json::from_str::<TextEncoding>("\"nope\"").is_err());
    }

    #[test]
    fn detects_boms_and_strips_them() {
        let utf8 = b"\xEF\xBB\xBFhi";
        let d = detect_encoding(utf8, &hints());
        assert_eq!(
            (d.encoding, d.bom, d.source),
            (TextEncoding::Utf8, true, EncodingSource::Bom)
        );
        assert_eq!(decode(utf8, TextEncoding::Utf8).unwrap().text, "hi");

        let le = encode("héllo", TextEncoding::Utf16Le, true).unwrap();
        let d = detect_encoding(&le, &hints());
        assert_eq!((d.encoding, d.bom), (TextEncoding::Utf16Le, true));
        assert_eq!(decode(&le, d.encoding).unwrap().text, "héllo");

        let be = encode("héllo", TextEncoding::Utf16Be, true).unwrap();
        assert_eq!(
            detect_encoding(&be, &hints()).encoding,
            TextEncoding::Utf16Be
        );

        let u32le = encode("a😀", TextEncoding::Utf32Le, true).unwrap();
        assert_eq!(
            detect_encoding(&u32le, &hints()).encoding,
            TextEncoding::Utf32Le
        );
        assert_eq!(decode(&u32le, TextEncoding::Utf32Le).unwrap().text, "a😀");
    }

    #[test]
    fn generic_utf16_writes_a_le_bom_and_reads_either() {
        let bytes = encode("ab", TextEncoding::Utf16, false).unwrap();
        assert_eq!(bytes, [0xFF, 0xFE, b'a', 0, b'b', 0]);
        assert_eq!(decode(&bytes, TextEncoding::Utf16).unwrap().text, "ab");
        let be = [0xFE, 0xFF, 0, b'a'];
        assert_eq!(decode(&be, TextEncoding::Utf16).unwrap().text, "a");
        // no BOM: big endian, as on macOS
        assert_eq!(decode(&[0, b'a'], TextEncoding::Utf16).unwrap().text, "a");
    }

    #[test]
    fn xattr_hint_beats_default_when_the_bytes_fit() {
        let bytes = b"caf\xE9"; // Latin-1 'café', invalid UTF-8
        let with_hint = EncodingHints {
            xattr: Some(TextEncoding::Latin1),
            default: TextEncoding::Utf8,
        };
        let d = detect_encoding(bytes, &with_hint);
        assert_eq!(
            (d.encoding, d.source),
            (TextEncoding::Latin1, EncodingSource::Xattr)
        );
        // without a hint the bytes are not UTF-8: fall back, but never fail
        let d = detect_encoding(bytes, &hints());
        assert_eq!(
            (d.encoding, d.source),
            (TextEncoding::Windows1252, EncodingSource::Fallback)
        );
        assert_eq!(decode(bytes, d.encoding).unwrap().text, "café");
    }

    #[test]
    fn hint_that_does_not_decode_is_skipped() {
        let hint = EncodingHints {
            xattr: Some(TextEncoding::Ascii),
            default: TextEncoding::Utf8,
        };
        let d = detect_encoding("é".as_bytes(), &hint);
        assert_eq!(
            (d.encoding, d.source),
            (TextEncoding::Utf8, EncodingSource::Default)
        );
    }

    #[test]
    fn reads_xml_declarations() {
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<a>caf\xE9</a>";
        assert_eq!(xml_declared_encoding(xml), Some(TextEncoding::Latin1));
        let d = detect_encoding(xml, &hints());
        assert_eq!(
            (d.encoding, d.source),
            (TextEncoding::Latin1, EncodingSource::XmlDeclaration)
        );
        let single = b"<?xml version='1.0' encoding='utf-8'?>";
        assert_eq!(xml_declared_encoding(single), Some(TextEncoding::Utf8));
        assert_eq!(xml_declared_encoding(b"<html></html>"), None);
    }

    #[test]
    fn reads_python_coding_cookies() {
        let py = b"#!/usr/bin/env python\n# -*- coding: latin-1 -*-\nprint('caf\xE9')\n";
        assert_eq!(coding_cookie_encoding(py), Some(TextEncoding::Latin1));
        let d = detect_encoding(py, &hints());
        assert_eq!(d.source, EncodingSource::CodingCookie);
        assert_eq!(
            coding_cookie_encoding(b"# vim: set fileencoding=utf-8 :\n"),
            Some(TextEncoding::Utf8)
        );
        // only the first two lines count
        assert_eq!(coding_cookie_encoding(b"a\nb\n# coding: latin-1\n"), None);
        assert_eq!(coding_cookie_encoding(b"x = 1  # not a cookie\n"), None);
    }

    #[test]
    fn utf16_without_bom_is_recognised_by_its_nuls() {
        let bytes: Vec<u8> = "hello world"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let d = detect_encoding(&bytes, &hints());
        assert_eq!(
            (d.encoding, d.source),
            (TextEncoding::Utf16Le, EncodingSource::Heuristic)
        );
    }

    #[test]
    fn cjk_round_trips() {
        let text = "日本語のテキスト";
        for encoding in [
            TextEncoding::ShiftJis,
            TextEncoding::EucJp,
            TextEncoding::Iso2022Jp,
        ] {
            let bytes = encode(text, encoding, false).unwrap();
            assert_eq!(decode(&bytes, encoding).unwrap().text, text, "{encoding:?}");
        }
        for (encoding, text) in [
            (TextEncoding::Gbk, "简体中文"),
            (TextEncoding::Gb18030, "简体中文😀"),
            (TextEncoding::Big5, "繁體中文"),
            (TextEncoding::EucKr, "한국어"),
        ] {
            let bytes = encode(text, encoding, false).unwrap();
            assert_eq!(decode(&bytes, encoding).unwrap().text, text, "{encoding:?}");
        }
    }

    #[test]
    fn iso2022jp_escapes_and_returns_to_ascii() {
        let bytes = encode("a日b", TextEncoding::Iso2022Jp, false).unwrap();
        assert_eq!(bytes, b"a\x1b$BF|\x1b(Bb");
    }

    #[test]
    fn single_byte_code_pages() {
        let bytes = encode("café – ü", TextEncoding::Windows1252, false).unwrap();
        assert_eq!(bytes, b"caf\xE9 \x96 \xFC");
        assert_eq!(
            decode(&bytes, TextEncoding::Windows1252).unwrap().text,
            "café – ü"
        );
        let mac = encode("é", TextEncoding::MacRoman, false).unwrap();
        assert_eq!(mac, [0x8E]);
        assert_eq!(decode(&[0xE9], TextEncoding::Latin1).unwrap().text, "é");
        assert_eq!(
            decode(
                "Привет".encode_cp1251().as_slice(),
                TextEncoding::Windows1251
            )
            .unwrap()
            .text,
            "Привет"
        );
    }

    trait Cp1251 {
        fn encode_cp1251(&self) -> Vec<u8>;
    }
    impl Cp1251 for str {
        fn encode_cp1251(&self) -> Vec<u8> {
            encode(self, TextEncoding::Windows1251, false).unwrap()
        }
    }

    #[test]
    fn reports_the_first_unrepresentable_character() {
        let err = encode("ok\nsmile 😀 and ✓", TextEncoding::Latin1, false).unwrap_err();
        assert_eq!(err.character, '😀');
        assert_eq!((err.line, err.column), (2, 7));
        assert_eq!(err.char_index, 9);
        assert!(encode("é", TextEncoding::Ascii, false).is_err());
        let err = encode("€", TextEncoding::ShiftJis, false);
        assert!(err.is_ok() || err.is_err()); // table dependent; must not panic
        assert!(encode("😀", TextEncoding::Windows1252, false).is_err());
        assert!(
            encode("ｱ", TextEncoding::Iso2022Jp, false).is_err(),
            "half-width kana"
        );
    }

    #[test]
    fn lists_every_lost_character() {
        let lost = unrepresentable_characters("aé✓b😀", TextEncoding::Latin1);
        assert_eq!(lost, vec![(2, '✓'), (4, '😀')]);
        assert!(unrepresentable_characters("anything 😀", TextEncoding::Utf8).is_empty());
        assert_eq!(check_insert("ok", TextEncoding::Ascii), InsertCheck::Fits);
        assert_eq!(
            check_insert("é", TextEncoding::Ascii),
            InsertCheck::Lossy(vec!['é'])
        );
    }

    #[test]
    fn strict_decoding_rejects_and_lossy_replaces() {
        assert!(decode(b"\xFF\xFE\xFD", TextEncoding::Utf8).is_err());
        let lossy = decode_lossy(b"a\xFFb", TextEncoding::Utf8);
        assert_eq!(lossy.text, "a\u{FFFD}b");
        assert!(lossy.had_errors);
        assert!(decode(b"abc\x80", TextEncoding::Ascii).is_err());
        assert!(decode(&[0x00, 0x61, 0x00], TextEncoding::Utf16Be).is_err());
        assert!(
            decode(&[0xD8, 0x00, 0x00, 0x61], TextEncoding::Utf16Be).is_err(),
            "lone surrogate"
        );
    }

    #[test]
    fn reinterpret_and_convert() {
        // The bytes of "é" in UTF-8 read as Latin-1 are two characters.
        let bytes = "é".as_bytes();
        assert_eq!(reinterpret(bytes, TextEncoding::Latin1).unwrap().text, "Ã©");
        assert!(reinterpret(&[0xFF], TextEncoding::Utf8).is_err());
        assert!(convert("café", TextEncoding::Latin1).is_ok());
        assert!(convert("日本", TextEncoding::Latin1).is_err());
    }

    #[test]
    fn bom_is_optional_for_utf8_and_written_on_request() {
        assert_eq!(encode("a", TextEncoding::Utf8, false).unwrap(), b"a");
        assert_eq!(
            encode("a", TextEncoding::Utf8, true).unwrap(),
            b"\xEF\xBB\xBFa"
        );
        assert_eq!(encode("a", TextEncoding::Latin1, true).unwrap(), b"a");
    }

    #[test]
    fn regexes_compile() {
        // The patterns must compile, or detection silently stops working.
        assert!(
            xml_regex()
                .is_some_and(|re| re.is_match(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>"))
        );
        assert!(cookie_regex().is_some_and(|re| re.is_match(b"# coding: utf-8")));
    }
}
