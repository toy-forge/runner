//! Text styles produced by SGR sequences, and palette resolution.

/// A 24-bit colour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// `0xRRGGBB`.
    pub fn to_u32(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }
}

/// A colour requested by the program.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Color {
    /// The console's default foreground / background.
    #[default]
    Default,
    /// One of the 16 ANSI colours: 0-7 regular (black, red, green, yellow,
    /// blue, magenta, cyan, white), 8-15 bright.
    Ansi(u8),
    /// An xterm 256-colour index (16-255; 0-15 are normalised to [`Color::Ansi`]).
    Indexed(u8),
    /// A true colour.
    Rgb(u8, u8, u8),
}

impl Color {
    /// The colour for 256-colour index `n` (0-15 become [`Color::Ansi`]).
    pub fn from_index(n: u8) -> Self {
        if n < 16 { Color::Ansi(n) } else { Color::Indexed(n) }
    }

    /// Names of the ANSI colours, regular order.
    pub const ANSI_NAMES: [&'static str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];

    /// The theme scope CodeRunner-style themes use for this colour
    /// (`console.ansi.color.foreground.red.bright`), when it is one of the 16.
    pub fn scope(self, foreground: bool) -> Option<String> {
        match self {
            Color::Ansi(n) if n < 16 => Some(format!(
                "console.ansi.color.{}.{}.{}",
                if foreground { "foreground" } else { "background" },
                Self::ANSI_NAMES[(n % 8) as usize],
                if n < 8 { "regular" } else { "bright" }
            )),
            _ => None,
        }
    }
}

/// Text attributes (a small bit set).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Attrs(pub u16);

impl Attrs {
    /// Bold (SGR 1).
    pub const BOLD: Attrs = Attrs(1);
    /// Faint (SGR 2).
    pub const FAINT: Attrs = Attrs(1 << 1);
    /// Italic (SGR 3).
    pub const ITALIC: Attrs = Attrs(1 << 2);
    /// Single underline (SGR 4).
    pub const UNDERLINE: Attrs = Attrs(1 << 3);
    /// Double underline (SGR 21).
    pub const DOUBLE_UNDERLINE: Attrs = Attrs(1 << 4);
    /// Blink (SGR 5/6); parsed but renderers normally ignore it.
    pub const BLINK: Attrs = Attrs(1 << 5);
    /// Inverse video (SGR 7).
    pub const INVERSE: Attrs = Attrs(1 << 6);
    /// Hidden (SGR 8).
    pub const HIDDEN: Attrs = Attrs(1 << 7);
    /// Strike-through (SGR 9).
    pub const STRIKE: Attrs = Attrs(1 << 8);

    /// Whether every bit of `other` is set.
    pub fn contains(self, other: Attrs) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets the bits of `other`.
    pub fn insert(&mut self, other: Attrs) {
        self.0 |= other.0;
    }

    /// Clears the bits of `other`.
    pub fn remove(&mut self, other: Attrs) {
        self.0 &= !other.0;
    }
}

/// The SGR state a run of text was written in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Style {
    /// Foreground.
    pub fg: Color,
    /// Background.
    pub bg: Color,
    /// Attributes.
    pub attrs: Attrs,
}

impl Style {
    /// No colours, no attributes.
    pub const PLAIN: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        attrs: Attrs(0),
    };

    /// Whether this is the default style.
    pub fn is_plain(&self) -> bool {
        *self == Style::PLAIN
    }

    /// Bold?
    pub fn bold(&self) -> bool {
        self.attrs.contains(Attrs::BOLD)
    }
    /// Faint?
    pub fn faint(&self) -> bool {
        self.attrs.contains(Attrs::FAINT)
    }
    /// Italic?
    pub fn italic(&self) -> bool {
        self.attrs.contains(Attrs::ITALIC)
    }
    /// Underlined (single or double)?
    pub fn underline(&self) -> bool {
        self.attrs.contains(Attrs::UNDERLINE) || self.attrs.contains(Attrs::DOUBLE_UNDERLINE)
    }
    /// Struck through?
    pub fn strike(&self) -> bool {
        self.attrs.contains(Attrs::STRIKE)
    }
    /// Inverse video?
    pub fn inverse(&self) -> bool {
        self.attrs.contains(Attrs::INVERSE)
    }
    /// Hidden?
    pub fn hidden(&self) -> bool {
        self.attrs.contains(Attrs::HIDDEN)
    }

    /// The colours to draw with: palette lookup, inverse applied against the
    /// defaults given, `None` background meaning "the console background".
    pub fn resolve(&self, palette: &AnsiPalette, default_fg: Rgb, default_bg: Rgb) -> ResolvedStyle {
        let fg = palette.color(self.fg);
        let bg = palette.color(self.bg);
        let (fg, bg) = if self.inverse() {
            (Some(bg.unwrap_or(default_bg)), Some(fg.unwrap_or(default_fg)))
        } else {
            (fg, bg)
        };
        ResolvedStyle {
            fg: fg.unwrap_or(default_fg),
            bg,
            bold: self.bold(),
            faint: self.faint(),
            italic: self.italic(),
            underline: self.underline(),
            strike: self.strike(),
            hidden: self.hidden(),
        }
    }
}

/// A [`Style`] with colours looked up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResolvedStyle {
    /// Text colour.
    pub fg: Rgb,
    /// Cell background; `None` = the console's own.
    pub bg: Option<Rgb>,
    /// Bold.
    pub bold: bool,
    /// Faint (draw dimmer).
    pub faint: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Strike-through.
    pub strike: bool,
    /// Do not draw the text.
    pub hidden: bool,
}

/// The 16 ANSI colours; indices 16-255 are computed (6x6x6 cube, grey ramp).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnsiPalette {
    /// Regular 0-7 then bright 8-15.
    pub ansi: [Rgb; 16],
}

impl Default for AnsiPalette {
    fn default() -> Self {
        let c = |v: u32| Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        AnsiPalette {
            ansi: [
                c(0x000000),
                c(0xc23621),
                c(0x25bc24),
                c(0xadad27),
                c(0x4a5fe0),
                c(0xd338d3),
                c(0x33bbc8),
                c(0xcbcccd),
                c(0x686a6b),
                c(0xfc391f),
                c(0x31e722),
                c(0xeaec23),
                c(0x7a8cff),
                c(0xf935f8),
                c(0x14f0f0),
                c(0xf2f3f3),
            ],
        }
    }
}

impl AnsiPalette {
    /// The colour of `c`; `None` for [`Color::Default`].
    pub fn color(&self, c: Color) -> Option<Rgb> {
        match c {
            Color::Default => None,
            Color::Ansi(n) => Some(self.ansi[(n & 15) as usize]),
            Color::Indexed(n) => Some(self.indexed(n)),
            Color::Rgb(r, g, b) => Some(Rgb(r, g, b)),
        }
    }

    /// The colour of xterm index `n`.
    pub fn indexed(&self, n: u8) -> Rgb {
        match n {
            0..=15 => self.ansi[n as usize],
            16..=231 => {
                let n = n - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
            }
            _ => {
                let v = 8 + (n - 232) * 10;
                Rgb(v, v, v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grey() {
        let p = AnsiPalette::default();
        assert_eq!(p.indexed(16), Rgb(0, 0, 0));
        assert_eq!(p.indexed(231), Rgb(255, 255, 255));
        assert_eq!(p.indexed(196), Rgb(255, 0, 0));
        assert_eq!(p.indexed(232), Rgb(8, 8, 8));
        assert_eq!(p.indexed(255), Rgb(238, 238, 238));
    }

    #[test]
    fn inverse_swaps_against_defaults() {
        let p = AnsiPalette::default();
        let mut s = Style::PLAIN;
        s.attrs.insert(Attrs::INVERSE);
        let r = s.resolve(&p, Rgb(1, 1, 1), Rgb(9, 9, 9));
        assert_eq!(r.fg, Rgb(9, 9, 9));
        assert_eq!(r.bg, Some(Rgb(1, 1, 1)));
    }

    #[test]
    fn scope_names() {
        assert_eq!(
            Color::Ansi(9).scope(true).as_deref(),
            Some("console.ansi.color.foreground.red.bright")
        );
        assert_eq!(
            Color::Ansi(2).scope(false).as_deref(),
            Some("console.ansi.color.background.green.regular")
        );
        assert_eq!(Color::Rgb(1, 2, 3).scope(true), None);
    }
}
