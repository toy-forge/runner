//! File-type icons.
//!
//! Option A (cheap, themeable): an own SVG set. Every glyph draws with
//! `currentColor`, so `svg(src).text_color(tint)` recolours it per language; the
//! markup is an `ImageSrc::svg_markup` string (no assets directory needed).
//!
//! Option B (system look): `NSWorkspace` icons rendered to RGBA on the main thread
//! (see `native_icons`), shown with `image(ImageSrc::rgba(..))`.

/// The glyph shape of a file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// Closed folder.
    Folder,
    /// Plain page.
    Page,
    /// Page with `</>`.
    Code,
    /// Page with text lines.
    Text,
    /// Page with `{}`.
    Data,
    /// Picture.
    Image,
    /// Terminal prompt.
    Shell,
    /// Archive box.
    Archive,
}

/// What a file looks like in the navigator: glyph + tint token name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IconSpec {
    /// Shape.
    pub glyph: Glyph,
    /// Theme token (`icon-orange`...), resolved with `ui::theme::icon_tint`.
    pub tint: &'static str,
}

/// The icon of a folder.
pub const FOLDER: IconSpec = IconSpec {
    glyph: Glyph::Folder,
    tint: "icon-folder",
};

/// The icon for a file with extension `ext` (lowercase, no dot).
pub fn spec_for_ext(ext: Option<&str>) -> IconSpec {
    use Glyph::*;
    let (glyph, tint) = match ext.unwrap_or("") {
        "rs" => (Code, "icon-orange"),
        "py" | "pyw" => (Code, "icon-blue"),
        "js" | "mjs" | "cjs" | "jsx" => (Code, "icon-yellow"),
        "ts" | "tsx" => (Code, "icon-blue"),
        "swift" => (Code, "icon-orange"),
        "go" => (Code, "icon-teal"),
        "c" | "h" => (Code, "icon-blue"),
        "cpp" | "cc" | "cxx" | "hpp" | "mm" | "m" => (Code, "icon-purple"),
        "java" | "kt" | "scala" => (Code, "icon-red"),
        "rb" | "php" | "pl" | "lua" => (Code, "icon-red"),
        "cs" | "fs" | "hs" | "ml" | "zig" | "dart" => (Code, "icon-purple"),
        "html" | "htm" | "css" | "scss" | "xml" | "svg" => (Code, "icon-green"),
        "sh" | "bash" | "zsh" | "fish" => (Shell, "icon-green"),
        "json" | "toml" | "yaml" | "yml" | "plist" | "ini" | "conf" => (Data, "icon-gray"),
        "md" | "markdown" | "txt" | "rtf" | "log" => (Text, "icon-gray"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tiff" | "bmp" | "ico" | "icns" => {
            (Image, "icon-purple")
        }
        "zip" | "gz" | "tar" | "tgz" | "7z" | "dmg" => (Archive, "icon-gray"),
        _ => (Page, "icon-gray"),
    };
    IconSpec { glyph, tint }
}

const HEAD: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">"#;

/// SVG markup of a glyph; `currentColor` everywhere.
pub fn svg_markup(glyph: Glyph) -> String {
    let body = match glyph {
        Glyph::Folder => {
            r#"<path d="M1.5 4.2c0-.7.5-1.2 1.2-1.2h3.1c.4 0 .7.1 1 .4l.8.9h5.7c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.7c-.7 0-1.2-.5-1.2-1.2z" fill="currentColor"/>"#
        }
        Glyph::Page => PAGE,
        Glyph::Code => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M6.2 7.2 4.6 8.8l1.6 1.6M9.8 7.2l1.6 1.6-1.6 1.6" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            );
        }
        Glyph::Text => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M5.2 7.6h5.6M5.2 9.6h5.6M5.2 11.6h3.4" fill="none" stroke="currentColor" stroke-width="1" stroke-linecap="round"/></svg>"#
            );
        }
        Glyph::Data => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M6.6 7.2c-.9 0-.9.8-.9 1.4s0 .9-.7.9c.7 0 .7.4.7.9s0 1.4.9 1.4M9.4 7.2c.9 0 .9.8.9 1.4s0 .9.7.9c-.7 0-.7.4-.7.9s0 1.4-.9 1.4" fill="none" stroke="currentColor" stroke-width="1" stroke-linecap="round"/></svg>"#
            );
        }
        Glyph::Image => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M4.8 12 7 8.8l1.6 2 1.1-1.3L11.2 12z" fill="currentColor"/><circle cx="9.9" cy="7.4" r=".9" fill="currentColor"/></svg>"#
            );
        }
        Glyph::Shell => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M5.4 7.4 7 8.9 5.4 10.4M8 11h2.6" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            );
        }
        Glyph::Archive => {
            return format!(
                r#"{HEAD}{PAGE}<path d="M8 3v8M6.8 5h2.4M6.8 7h2.4" fill="none" stroke="currentColor" stroke-width="1" stroke-linecap="round"/><rect x="6.5" y="9.6" width="3" height="2.4" rx=".6" fill="currentColor"/></svg>"#
            );
        }
    };
    format!("{HEAD}{body}</svg>")
}

/// A page with a folded corner, drawn as an outline at half opacity so the
/// symbols inside read.
const PAGE: &str = r#"<path d="M3.6 1.8h5.2l3.6 3.6v8.2c0 .4-.3.7-.7.7H3.6c-.4 0-.7-.3-.7-.7V2.5c0-.4.3-.7.7-.7z" fill="currentColor" fill-opacity=".16" stroke="currentColor" stroke-width="1" stroke-linejoin="round"/><path d="M8.6 1.9v3.1c0 .3.2.5.5.5h3" fill="none" stroke="currentColor" stroke-width="1" stroke-linejoin="round"/>"#;

/// The disclosure chevron (points right; the row rotates it 90 degrees).
pub const CHEVRON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12" width="12" height="12"><path d="M4.4 2.6 7.8 6 4.4 9.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_glyphs_and_tints() {
        assert_eq!(spec_for_ext(Some("rs")).glyph, Glyph::Code);
        assert_eq!(spec_for_ext(Some("png")).glyph, Glyph::Image);
        assert_eq!(
            spec_for_ext(None),
            IconSpec {
                glyph: Glyph::Page,
                tint: "icon-gray"
            }
        );
    }

    #[test]
    fn markup_is_well_formed() {
        for g in [
            Glyph::Folder,
            Glyph::Page,
            Glyph::Code,
            Glyph::Text,
            Glyph::Data,
            Glyph::Image,
            Glyph::Shell,
            Glyph::Archive,
        ] {
            let s = svg_markup(g);
            assert!(s.starts_with("<svg") && s.ends_with("</svg>"));
            assert_eq!(s.matches("<svg").count(), 1);
        }
    }
}

/// Small chrome icons (12-14 pt strokes, `currentColor`).
pub mod chrome {
    macro_rules! icon {
        ($name:ident, $body:expr) => {
            /// Chrome icon markup.
            pub const $name: &str = concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</svg>"
            );
        };
    }
    icon!(PLUS, r#"<path d="M8 3.5v9M3.5 8h9"/>"#);
    icon!(
        FOLDER_PLUS,
        r#"<path d="M2 4.6c0-.6.4-1 1-1h2.7l1.1 1.2h5.2c.6 0 1 .4 1 1v5.6c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/><path d="M8 7.2v3.2M6.4 8.8h3.2"/>"#
    );
    icon!(
        GOTO,
        r#"<circle cx="8" cy="8" r="5.6"/><path d="M6.4 8h3.4M8.4 6.4 10 8l-1.6 1.6"/>"#
    );
    icon!(BACK, r#"<path d="M10 3.5 5.5 8l4.5 4.5"/>"#);
    icon!(FORWARD, r#"<path d="M6 3.5 10.5 8 6 12.5"/>"#);
    icon!(
        SEARCH,
        r#"<circle cx="7" cy="7" r="4.2"/><path d="m10.2 10.2 3.1 3.1"/>"#
    );
    icon!(CHEVRON_DOWN, r#"<path d="m4 6.2 4 4 4-4"/>"#);
    icon!(CHEVRON_UP, r#"<path d="m4 9.8 4-4 4 4"/>"#);
    icon!(CLOSE, r#"<path d="m4.5 4.5 7 7M11.5 4.5l-7 7"/>"#);
    icon!(REPLACE, r#"<path d="M3 6h9l-2.2-2.2M13 10H4l2.2 2.2"/>"#);
    icon!(
        FOLDER_MINI,
        r#"<path d="M2 4.6c0-.6.4-1 1-1h2.7l1.1 1.2h5.2c.6 0 1 .4 1 1v5.6c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/>"#
    );
}
