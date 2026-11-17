//! The theme model: a TextMate `.tmTheme` with CodeRunner's extras.

use crate::color::Rgba;
use crate::selector::{ScopeSelector, SelectorError};

/// `fontStyle` of a theme item: any combination of bold, italic and
/// underline. An item whose `fontStyle` is present but empty is plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FontStyle {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
}

impl FontStyle {
    /// Plain text.
    pub const PLAIN: FontStyle = FontStyle {
        bold: false,
        italic: false,
        underline: false,
    };

    /// Parses a `fontStyle` string (`"bold italic"`); unknown words (such as
    /// `stippled_underline`) are ignored.
    pub fn parse(text: &str) -> FontStyle {
        let mut style = FontStyle::PLAIN;
        for word in text.split_whitespace() {
            match word {
                "bold" => style.bold = true,
                "italic" => style.italic = true,
                "underline" => style.underline = true,
                _ => {}
            }
        }
        style
    }

    /// The `fontStyle` string, words in the order bold italic underline.
    pub fn to_text(self) -> String {
        let mut words = Vec::new();
        if self.bold {
            words.push("bold");
        }
        if self.italic {
            words.push("italic");
        }
        if self.underline {
            words.push("underline");
        }
        words.join(" ")
    }
}

/// One scoped rule of a theme (an element of the `settings` array).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThemeItem {
    /// The display name ("Comments").
    pub name: Option<String>,
    /// The scope selector text.
    pub scope: String,
    /// Foreground colour.
    pub foreground: Option<Rgba>,
    /// Background colour (drawn behind the text).
    pub background: Option<Rgba>,
    /// `fontStyle`; `Some(PLAIN)` explicitly resets inherited styles.
    pub font_style: Option<FontStyle>,
    /// Unknown keys of the item's `settings` dictionary, kept for
    /// round-tripping.
    pub extra: plist::Dictionary,
}

impl ThemeItem {
    /// An item with a foreground colour and optional font style.
    pub fn new(name: &str, scope: &str, foreground: Rgba, font_style: Option<FontStyle>) -> ThemeItem {
        ThemeItem {
            name: Some(name.to_owned()),
            scope: scope.to_owned(),
            foreground: Some(foreground),
            background: None,
            font_style,
            extra: plist::Dictionary::new(),
        }
    }

    /// The parsed selector.
    ///
    /// # Errors
    /// [`SelectorError`] when the scope text is malformed.
    pub fn selector(&self) -> Result<ScopeSelector, SelectorError> {
        ScopeSelector::parse(&self.scope)
    }
}

/// The global (scope-less) settings of a theme. Colours are optional; the
/// accessors on [`Theme`] and [`EditorColors`] supply fallbacks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GlobalSettings {
    /// Editor background.
    pub background: Option<Rgba>,
    /// Default text colour.
    pub foreground: Option<Rgba>,
    /// Insertion point colour.
    pub caret: Option<Rgba>,
    /// Selection background.
    pub selection: Option<Rgba>,
    /// Selection outline (`selectionBorder`, Monokai/Dusk).
    pub selection_border: Option<Rgba>,
    /// Current-line background.
    pub line_highlight: Option<Rgba>,
    /// Invisible-character marks, indent guides.
    pub invisibles: Option<Rgba>,
    /// Find-match background.
    pub find_highlight: Option<Rgba>,
    /// Find-match text colour.
    pub find_highlight_foreground: Option<Rgba>,
    /// The active indent guide.
    pub active_guide: Option<Rgba>,
    /// Matching-bracket colour (`bracketsForeground`).
    pub brackets_foreground: Option<Rgba>,
    /// Unknown global keys (`bracketsOptions`, `tagsOptions`, ...).
    pub extra: plist::Dictionary,
}

/// The `gutterSettings` dictionary some themes carry (Spacegray, Twilight).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GutterSettings {
    /// Gutter background.
    pub background: Option<Rgba>,
    /// Line-number colour.
    pub foreground: Option<Rgba>,
    /// Separator line colour.
    pub divider: Option<Rgba>,
    /// Background of selected lines' numbers.
    pub selection_background: Option<Rgba>,
    /// Colour of selected lines' numbers.
    pub selection_foreground: Option<Rgba>,
    /// Border of selected lines' numbers.
    pub selection_border: Option<Rgba>,
}

/// Where a theme comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ThemeOrigin {
    /// Shipped with the app.
    #[default]
    Builtin,
    /// Loaded from or saved to a file in the user's themes directory.
    User(std::path::PathBuf),
}

/// A colour theme.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Theme {
    /// Unique display name; the identity used by settings.
    pub name: String,
    /// `uuid` metadata.
    pub uuid: Option<String>,
    /// `author` metadata.
    pub author: Option<String>,
    /// `comment` metadata.
    pub comment: Option<String>,
    /// Global settings.
    pub global: GlobalSettings,
    /// `gutterSettings`.
    pub gutter: Option<GutterSettings>,
    /// The scoped items, in file order (later wins ties).
    pub items: Vec<ThemeItem>,
    /// Unknown top-level keys (`semanticClass`, `colorSpaceName`, ...).
    pub extra: plist::Dictionary,
    /// Where it came from.
    pub origin: ThemeOrigin,
    /// Non-fatal problems found while reading (bad colours, bad selectors).
    pub warnings: Vec<String>,
}

/// Editor chrome colours derived from a theme with fallbacks filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorColors {
    /// Editor background (opaque).
    pub background: Rgba,
    /// Default text colour.
    pub foreground: Rgba,
    /// Caret.
    pub caret: Rgba,
    /// Selection background (may be translucent).
    pub selection: Rgba,
    /// Selection outline, if the theme has one.
    pub selection_border: Option<Rgba>,
    /// Current-line background (may be translucent).
    pub line_highlight: Rgba,
    /// Whitespace marks.
    pub invisibles: Rgba,
    /// Indent guides (the invisibles colour, softened).
    pub indent_guide: Rgba,
    /// The active indent guide.
    pub active_guide: Rgba,
    /// Find-match background.
    pub find_highlight: Rgba,
    /// Find-match text colour, if the theme sets one.
    pub find_highlight_foreground: Option<Rgba>,
    /// Matching-bracket colour.
    pub matching_bracket: Rgba,
    /// Gutter background.
    pub gutter_background: Rgba,
    /// Line numbers.
    pub gutter_foreground: Rgba,
    /// Line numbers of the lines with a caret.
    pub gutter_active_foreground: Rgba,
    /// Gutter separator.
    pub gutter_divider: Rgba,
    /// Page-guide ruler.
    pub ruler: Rgba,
}

/// App chrome tints derived from a theme (sidebar, console, toolbar...). All
/// opaque. A heuristic modelled on CodeRunner's
/// `colorByIncreasingContrastForSidebar`: surfaces step from the editor
/// background towards the foreground by a few percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    /// The theme is dark (background luminance below 0.5).
    pub is_dark: bool,
    /// Editor and console background.
    pub background: Rgba,
    /// Default text.
    pub foreground: Rgba,
    /// Secondary text.
    pub muted_foreground: Rgba,
    /// Window background behind panels.
    pub window_background: Rgba,
    /// Toolbar / title bar.
    pub toolbar_background: Rgba,
    /// Tab strip.
    pub tab_bar_background: Rgba,
    /// Sidebar (file navigator).
    pub sidebar_background: Rgba,
    /// Sidebar text.
    pub sidebar_foreground: Rgba,
    /// Selected sidebar row / list selection.
    pub selection: Rgba,
    /// Console background.
    pub console_background: Rgba,
    /// Console default text.
    pub console_foreground: Rgba,
    /// Hairlines between panels.
    pub separator: Rgba,
    /// Accent (caret colour).
    pub accent: Rgba,
}

impl Theme {
    /// An empty theme named `name` with the default scope items. Light or
    /// dark chooses the base colours.
    pub fn template(name: &str, dark: bool) -> Theme {
        crate::builtin::template(name, dark)
    }

    /// Whether the theme is dark (background luminance < 0.5); the
    /// classification behind the Light/Dark theme pair.
    pub fn is_dark(&self) -> bool {
        self.background().luminance() < 0.5
    }

    /// The opaque background (white when the theme has none).
    pub fn background(&self) -> Rgba {
        self.global
            .background
            .map(|c| c.over(Rgba::WHITE))
            .unwrap_or(Rgba::WHITE)
    }

    /// The default text colour (black on a light background, light grey on a
    /// dark one when the theme has none).
    pub fn foreground(&self) -> Rgba {
        self.global.foreground.unwrap_or_else(|| {
            if self.background().luminance() < 0.5 {
                Rgba::rgb(0xDD, 0xDD, 0xDD)
            } else {
                Rgba::BLACK
            }
        })
    }

    /// The editor part colours with fallbacks.
    pub fn editor_colors(&self) -> EditorColors {
        let g = &self.global;
        let bg = self.background();
        let fg = self.foreground();
        let gutter = self.gutter.clone().unwrap_or_default();
        let invisibles = g.invisibles.unwrap_or(fg.with_alpha(0x33));
        let guide_alpha = (invisibles.a as f32 * 0.6) as u8;
        EditorColors {
            background: bg,
            foreground: fg,
            caret: g.caret.unwrap_or(fg),
            selection: g.selection.unwrap_or(fg.with_alpha(0x33)),
            selection_border: g.selection_border,
            line_highlight: g.line_highlight.unwrap_or(fg.with_alpha(0x12)),
            invisibles,
            indent_guide: invisibles.with_alpha(guide_alpha.max(0x18)),
            active_guide: g.active_guide.unwrap_or(fg.with_alpha(0x55)),
            find_highlight: g.find_highlight.unwrap_or(Rgba::new(0xFF, 0xD6, 0x0A, 0x66)),
            find_highlight_foreground: g.find_highlight_foreground,
            matching_bracket: g.brackets_foreground.unwrap_or(fg.with_alpha(0x66)),
            gutter_background: gutter.background.unwrap_or(bg),
            gutter_foreground: gutter.foreground.unwrap_or_else(|| fg.mix(bg, 0.55)),
            gutter_active_foreground: gutter.selection_foreground.unwrap_or(fg),
            gutter_divider: gutter.divider.unwrap_or(fg.with_alpha(0x1A)),
            ruler: fg.with_alpha(0x1F),
        }
    }

    /// App chrome tints derived from the theme.
    pub fn chrome(&self) -> Chrome {
        let bg = self.background();
        let fg = self.foreground().over(bg);
        let editor = self.editor_colors();
        let step = |t: f32| bg.mix(fg, t);
        Chrome {
            is_dark: self.is_dark(),
            background: bg,
            foreground: fg,
            muted_foreground: fg.mix(bg, 0.45),
            window_background: step(0.03),
            toolbar_background: step(0.07),
            tab_bar_background: step(0.04),
            sidebar_background: step(0.05),
            sidebar_foreground: fg.mix(bg, 0.1),
            selection: editor.selection.over(bg).with_alpha(255),
            console_background: bg,
            console_foreground: fg,
            separator: fg.with_alpha(0x1F).over(bg),
            accent: editor.caret.over(bg),
        }
    }

    /// A copy named `name` that is a user theme (no file yet), for
    /// "Duplicate".
    pub fn duplicate(&self, name: &str) -> Theme {
        let mut copy = self.clone();
        copy.name = name.to_owned();
        copy.uuid = None;
        copy.origin = ThemeOrigin::User(std::path::PathBuf::new());
        copy
    }

    /// A copy with every colour's HSL lightness mirrored ("Invert": a light
    /// theme becomes dark and vice versa). Named `name`.
    pub fn inverted(&self, name: &str) -> Theme {
        let mut out = self.duplicate(name);
        let flip = |c: &mut Option<Rgba>| *c = c.map(Rgba::invert_lightness);
        let g = &mut out.global;
        for c in [
            &mut g.background,
            &mut g.foreground,
            &mut g.caret,
            &mut g.selection,
            &mut g.selection_border,
            &mut g.line_highlight,
            &mut g.invisibles,
            &mut g.find_highlight,
            &mut g.find_highlight_foreground,
            &mut g.active_guide,
            &mut g.brackets_foreground,
        ] {
            flip(c);
        }
        if let Some(gutter) = &mut out.gutter {
            for c in [
                &mut gutter.background,
                &mut gutter.foreground,
                &mut gutter.divider,
                &mut gutter.selection_background,
                &mut gutter.selection_foreground,
                &mut gutter.selection_border,
            ] {
                flip(c);
            }
        }
        for item in &mut out.items {
            flip(&mut item.foreground);
            flip(&mut item.background);
        }
        out
    }

    /// Adds the default items (links, console and ANSI scopes) whose scope
    /// text is missing: what CodeRunner's `addMissingEntriesToCurrentTheme`
    /// does, so third-party tmThemes get console colours. Returns how many
    /// were added.
    pub fn merge_default_items(&mut self) -> usize {
        let have: std::collections::HashSet<String> =
            self.items.iter().map(|i| i.scope.trim().to_owned()).collect();
        let mut added = 0;
        for item in crate::builtin::default_items(self.is_dark()) {
            if !have.contains(item.scope.trim()) {
                self.items.push(item);
                added += 1;
            }
        }
        added
    }

    /// Compiles the theme for fast scope resolution.
    pub fn compile(&self) -> crate::resolve::ResolvedTheme {
        crate::resolve::ResolvedTheme::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_style_round_trips() {
        let s = FontStyle::parse("bold  underline stippled_underline");
        assert!(s.bold && s.underline && !s.italic);
        assert_eq!(s.to_text(), "bold underline");
        assert_eq!(FontStyle::parse(""), FontStyle::PLAIN);
    }

    #[test]
    fn classification_follows_background() {
        let dark = Theme::template("d", true);
        let light = Theme::template("l", false);
        assert!(dark.is_dark());
        assert!(!light.is_dark());
        assert!(dark.inverted("x").background().luminance() > 0.5);
    }

    #[test]
    fn chrome_steps_towards_foreground() {
        let dark = Theme::template("d", true);
        let chrome = dark.chrome();
        assert!(chrome.sidebar_background.luminance() > chrome.background.luminance());
        assert!(chrome.muted_foreground.luminance() < chrome.foreground.luminance());
    }
}
