//! A small sRGB colour type with the operations themes need: hex parsing and
//! printing (tmTheme alpha is the LAST byte), blending, luminance and
//! lightness inversion.

use std::fmt;

/// An sRGB colour with 8-bit channels and straight (non-premultiplied) alpha.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha, 255 = opaque.
    pub a: u8,
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    /// Opaque white.
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    /// Fully transparent.
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);

    /// A colour from all four channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    /// Parses `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA` (the `#` is optional;
    /// alpha is the last byte, as in tmThemes). `None` for anything else.
    pub fn parse(text: &str) -> Option<Rgba> {
        let hex = text.trim().trim_start_matches('#');
        if !hex.is_ascii() {
            return None;
        }
        let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 17);
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => Some(Rgba::rgb(nibble(0)?, nibble(1)?, nibble(2)?)),
            4 => Some(Rgba::new(nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?)),
            6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// `#RRGGBB`, or `#RRGGBBAA` when not opaque (upper case).
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }

    /// `0xRRGGBBAA`, the packing NSH's `Color` uses.
    pub const fn to_u32(self) -> u32 {
        (self.r as u32) << 24 | (self.g as u32) << 16 | (self.b as u32) << 8 | self.a as u32
    }

    /// The same colour with alpha `a`.
    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba { a, ..self }
    }

    /// Perceived brightness in `0.0..=1.0` (0.299 R + 0.587 G + 0.114 B,
    /// ignoring alpha): CodeRunner's light/dark classification.
    pub fn luminance(self) -> f32 {
        (0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32) / 255.0
    }

    /// Linear interpolation towards `other` (`t` 0 = self, 1 = other), on
    /// every channel including alpha.
    pub fn mix(self, other: Rgba, t: f32) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
        Rgba::new(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
            lerp(self.a, other.a),
        )
    }

    /// `self` composited over the opaque-or-not `background` (source-over).
    pub fn over(self, background: Rgba) -> Rgba {
        let sa = self.a as f32 / 255.0;
        let ba = background.a as f32 / 255.0;
        let oa = sa + ba * (1.0 - sa);
        if oa <= 0.0 {
            return Rgba::TRANSPARENT;
        }
        let ch = |s: u8, b: u8| {
            ((s as f32 * sa + b as f32 * ba * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8
        };
        Rgba::new(
            ch(self.r, background.r),
            ch(self.g, background.g),
            ch(self.b, background.b),
            (oa * 255.0).round() as u8,
        )
    }

    /// Hue (degrees), saturation and lightness (both `0.0..=1.0`); alpha is
    /// ignored.
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d <= f32::EPSILON {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } * 60.0;
        (h, s, l)
    }

    /// Builds an opaque colour from hue (degrees), saturation and lightness.
    pub fn from_hsl(h: f32, s: f32, l: f32) -> Rgba {
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = (h.rem_euclid(360.0)) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r1, g1, b1) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let to8 = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba::rgb(to8(r1), to8(g1), to8(b1))
    }

    /// The colour with its HSL lightness mirrored (`l` -> `1 - l`), hue,
    /// saturation and alpha kept: what "Invert" in the Themes tab does.
    pub fn invert_lightness(self) -> Rgba {
        let (h, s, l) = self.to_hsl();
        Rgba::from_hsl(h, s, 1.0 - l).with_alpha(self.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_hex_forms() {
        assert_eq!(Rgba::parse("#FFF"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse("#1C1C20"), Some(Rgba::rgb(0x1C, 0x1C, 0x20)));
        assert_eq!(Rgba::parse("#FFFFFF33"), Some(Rgba::new(255, 255, 255, 0x33)));
        assert_eq!(Rgba::parse(" 00ff00 "), Some(Rgba::rgb(0, 255, 0)));
        assert_eq!(Rgba::parse("#12345"), None);
        assert_eq!(Rgba::parse("#GGGGGG"), None);
        assert_eq!(Rgba::parse("é"), None);
    }

    #[test]
    fn hex_round_trips() {
        for text in ["#1C1C20", "#B3D8FDCC", "#000000"] {
            assert_eq!(Rgba::parse(text).map(Rgba::to_hex).as_deref(), Some(text));
        }
    }

    #[test]
    fn invert_lightness_flips_light_and_dark() {
        let dark = Rgba::rgb(0x1C, 0x1C, 0x20);
        assert!(dark.invert_lightness().luminance() > 0.8);
        assert!(Rgba::WHITE.invert_lightness().luminance() < 0.01);
        let c = Rgba::new(200, 40, 40, 128);
        assert_eq!(c.invert_lightness().a, 128);
    }

    #[test]
    fn over_composites() {
        let half = Rgba::new(255, 255, 255, 128);
        let out = half.over(Rgba::BLACK);
        assert!((out.r as i32 - 128).abs() <= 1);
        assert_eq!(out.a, 255);
    }
}
