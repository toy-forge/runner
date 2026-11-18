//! The built-in themes, authored here as compact palette tables and expanded
//! into full [`Theme`]s with one shared set of scope rules, plus the default
//! console/ANSI items every theme receives.
//!
//! The classic palettes (Solarized, Monokai, Tomorrow, Twilight, Dracula,
//! Nord, an Xcode-like light theme) are re-created from their published
//! colour palettes; none of the files shipped by any other app is copied.

use crate::color::Rgba;
use crate::theme::{FontStyle, GlobalSettings, GutterSettings, Theme, ThemeItem, ThemeOrigin};

/// Name of the default light theme.
pub const DEFAULT_LIGHT: &str = "ScratchRunner Light";
/// Name of the default dark theme.
pub const DEFAULT_DARK: &str = "ScratchRunner Dark";

/// The colours a built-in theme is generated from.
struct Palette {
    name: &'static str,
    author: &'static str,
    bg: &'static str,
    fg: &'static str,
    caret: &'static str,
    selection: &'static str,
    line: &'static str,
    invisibles: &'static str,
    comment: &'static str,
    string: &'static str,
    keyword: &'static str,
    constant: &'static str,
    function: &'static str,
    ty: &'static str,
    variable: &'static str,
    parameter: &'static str,
    operator: &'static str,
    tag: &'static str,
    attribute: &'static str,
    escape: &'static str,
    preproc: &'static str,
    heading: &'static str,
    link: &'static str,
    invalid: &'static str,
    keyword_bold: bool,
    comment_italic: bool,
    gutter: Option<(&'static str, &'static str)>,
}

const LIGHT: Palette = Palette {
    name: DEFAULT_LIGHT,
    author: "ScratchRunner",
    bg: "#FFFFFF",
    fg: "#3A3F47",
    caret: "#1A1D21",
    selection: "#B5D3F5B3",
    line: "#0000000D",
    invisibles: "#00000026",
    comment: "#8F98A5",
    string: "#2D8A57",
    keyword: "#B33F7E",
    constant: "#C4622D",
    function: "#2D68B5",
    ty: "#1F7A8C",
    variable: "#566580",
    parameter: "#6D5CA8",
    operator: "#5A6270",
    tag: "#B33F7E",
    attribute: "#8A6A1F",
    escape: "#1F8A7A",
    preproc: "#8A5A2B",
    heading: "#2D68B5",
    link: "#2D68B5",
    invalid: "#D1242F",
    keyword_bold: false,
    comment_italic: true,
    gutter: None,
};

const DARK: Palette = Palette {
    name: DEFAULT_DARK,
    author: "ScratchRunner",
    bg: "#1B1E24",
    fg: "#C3C9D4",
    caret: "#FFFFFF",
    selection: "#6C95D640",
    line: "#FFFFFF0F",
    invisibles: "#FFFFFF2B",
    comment: "#667085",
    string: "#8CC58F",
    keyword: "#E0709F",
    constant: "#E49B6B",
    function: "#62A8E8",
    ty: "#67C6D8",
    variable: "#8EA2C4",
    parameter: "#B59CE0",
    operator: "#AAB3C5",
    tag: "#E0709F",
    attribute: "#D9B166",
    escape: "#6FD0C0",
    preproc: "#D79A6A",
    heading: "#62A8E8",
    link: "#62A8E8",
    invalid: "#FF6B6B",
    keyword_bold: false,
    comment_italic: true,
    gutter: None,
};

const SOLARIZED_DARK: Palette = Palette {
    name: "Solarized Dark",
    author: "Palette by Ethan Schoonover",
    bg: "#002B36",
    fg: "#839496",
    caret: "#93A1A1",
    selection: "#0A4A5A",
    line: "#07364280",
    invisibles: "#586E75",
    comment: "#586E75",
    string: "#2AA198",
    keyword: "#859900",
    constant: "#D33682",
    function: "#268BD2",
    ty: "#B58900",
    variable: "#839496",
    parameter: "#6C71C4",
    operator: "#859900",
    tag: "#268BD2",
    attribute: "#93A1A1",
    escape: "#CB4B16",
    preproc: "#CB4B16",
    heading: "#268BD2",
    link: "#6C71C4",
    invalid: "#DC322F",
    keyword_bold: false,
    comment_italic: true,
    gutter: Some(("#002B36", "#586E75")),
};

const SOLARIZED_LIGHT: Palette = Palette {
    name: "Solarized Light",
    author: "Palette by Ethan Schoonover",
    bg: "#FDF6E3",
    fg: "#657B83",
    caret: "#586E75",
    selection: "#EEE8D5",
    line: "#EEE8D580",
    invisibles: "#93A1A1",
    comment: "#93A1A1",
    string: "#2AA198",
    keyword: "#859900",
    constant: "#D33682",
    function: "#268BD2",
    ty: "#B58900",
    variable: "#657B83",
    parameter: "#6C71C4",
    operator: "#859900",
    tag: "#268BD2",
    attribute: "#586E75",
    escape: "#CB4B16",
    preproc: "#CB4B16",
    heading: "#268BD2",
    link: "#6C71C4",
    invalid: "#DC322F",
    keyword_bold: false,
    comment_italic: true,
    gutter: Some(("#FDF6E3", "#93A1A1")),
};

const MONOKAI: Palette = Palette {
    name: "Monokai",
    author: "Palette by Wimer Hazenberg",
    bg: "#272822",
    fg: "#F8F8F2",
    caret: "#F8F8F0",
    selection: "#49483E",
    line: "#3E3D3299",
    invisibles: "#F8F8F230",
    comment: "#75715E",
    string: "#E6DB74",
    keyword: "#F92672",
    constant: "#AE81FF",
    function: "#A6E22E",
    ty: "#66D9EF",
    variable: "#F8F8F2",
    parameter: "#FD971F",
    operator: "#F92672",
    tag: "#F92672",
    attribute: "#A6E22E",
    escape: "#AE81FF",
    preproc: "#66D9EF",
    heading: "#A6E22E",
    link: "#66D9EF",
    invalid: "#F92672",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const TOMORROW: Palette = Palette {
    name: "Tomorrow",
    author: "Palette by Chris Kempson",
    bg: "#FFFFFF",
    fg: "#4D4D4C",
    caret: "#4D4D4C",
    selection: "#D6D6D6",
    line: "#EFEFEF",
    invisibles: "#00000026",
    comment: "#8E908C",
    string: "#718C00",
    keyword: "#8959A8",
    constant: "#F5871F",
    function: "#4271AE",
    ty: "#C99E00",
    variable: "#C82829",
    parameter: "#F5871F",
    operator: "#3E999F",
    tag: "#C82829",
    attribute: "#F5871F",
    escape: "#3E999F",
    preproc: "#3E999F",
    heading: "#4271AE",
    link: "#4271AE",
    invalid: "#C82829",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const TOMORROW_NIGHT: Palette = Palette {
    name: "Tomorrow Night",
    author: "Palette by Chris Kempson",
    bg: "#1D1F21",
    fg: "#C5C8C6",
    caret: "#AEAFAD",
    selection: "#373B41",
    line: "#282A2E",
    invisibles: "#5A5E66",
    comment: "#969896",
    string: "#B5BD68",
    keyword: "#B294BB",
    constant: "#DE935F",
    function: "#81A2BE",
    ty: "#F0C674",
    variable: "#CC6666",
    parameter: "#DE935F",
    operator: "#8ABEB7",
    tag: "#CC6666",
    attribute: "#DE935F",
    escape: "#8ABEB7",
    preproc: "#8ABEB7",
    heading: "#81A2BE",
    link: "#81A2BE",
    invalid: "#CC6666",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const TWILIGHT: Palette = Palette {
    name: "Twilight",
    author: "After the TextMate classic",
    bg: "#141414",
    fg: "#F8F8F8",
    caret: "#A7A7A7",
    selection: "#DDF0FF33",
    line: "#FFFFFF0A",
    invisibles: "#FFFFFF40",
    comment: "#5F5A60",
    string: "#8F9D6A",
    keyword: "#CDA869",
    constant: "#CF6A4C",
    function: "#DAD085",
    ty: "#F9EE98",
    variable: "#7587A6",
    parameter: "#9B859D",
    operator: "#CDA869",
    tag: "#AC885B",
    attribute: "#9B703F",
    escape: "#AFC4DB",
    preproc: "#8996A8",
    heading: "#CF6A4C",
    link: "#8F9D6A",
    invalid: "#FF6666",
    keyword_bold: false,
    comment_italic: true,
    gutter: Some(("#2F2F31", "#7A7A7C")),
};

const XCODE: Palette = Palette {
    name: "Xcode Light",
    author: "In the spirit of Xcode's default light theme",
    bg: "#FFFFFF",
    fg: "#000000",
    caret: "#000000",
    selection: "#B5D5FF",
    line: "#00000012",
    invisibles: "#00000026",
    comment: "#5D6C79",
    string: "#D12F1B",
    keyword: "#AD3DA4",
    constant: "#272AD8",
    function: "#326D74",
    ty: "#703DAA",
    variable: "#000000",
    parameter: "#000000",
    operator: "#000000",
    tag: "#AD3DA4",
    attribute: "#4B21B0",
    escape: "#272AD8",
    preproc: "#78492A",
    heading: "#AD3DA4",
    link: "#0E0EFF",
    invalid: "#D12F1B",
    keyword_bold: true,
    comment_italic: false,
    gutter: None,
};

const DRACULA: Palette = Palette {
    name: "Dracula",
    author: "Palette by Zeno Rocha",
    bg: "#282A36",
    fg: "#F8F8F2",
    caret: "#F8F8F2",
    selection: "#44475A",
    line: "#FFFFFF10",
    invisibles: "#FFFFFF2B",
    comment: "#6272A4",
    string: "#F1FA8C",
    keyword: "#FF79C6",
    constant: "#BD93F9",
    function: "#50FA7B",
    ty: "#8BE9FD",
    variable: "#F8F8F2",
    parameter: "#FFB86C",
    operator: "#FF79C6",
    tag: "#FF79C6",
    attribute: "#50FA7B",
    escape: "#FF79C6",
    preproc: "#FF79C6",
    heading: "#BD93F9",
    link: "#8BE9FD",
    invalid: "#FF5555",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const NORD: Palette = Palette {
    name: "Nord",
    author: "Palette by Arctic Ice Studio",
    bg: "#2E3440",
    fg: "#D8DEE9",
    caret: "#D8DEE9",
    selection: "#434C5ECC",
    line: "#3B4252",
    invisibles: "#4C566A",
    comment: "#616E88",
    string: "#A3BE8C",
    keyword: "#81A1C1",
    constant: "#B48EAD",
    function: "#88C0D0",
    ty: "#8FBCBB",
    variable: "#D8DEE9",
    parameter: "#D8DEE9",
    operator: "#81A1C1",
    tag: "#81A1C1",
    attribute: "#8FBCBB",
    escape: "#EBCB8B",
    preproc: "#5E81AC",
    heading: "#88C0D0",
    link: "#88C0D0",
    invalid: "#BF616A",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const XCODE_DARK: Palette = Palette {
    name: "Xcode Dark",
    author: "In the spirit of Xcode's default dark theme",
    bg: "#1F1F24",
    fg: "#FFFFFF",
    caret: "#FFFFFF",
    selection: "#264F78",
    line: "#FFFFFF0D",
    invisibles: "#FFFFFF26",
    comment: "#6C7986",
    string: "#FC6A5D",
    keyword: "#FF7AB2",
    constant: "#D9C97C",
    function: "#D0A8FF",
    ty: "#9EF1DD",
    variable: "#FFFFFF",
    parameter: "#67B7A4",
    operator: "#FFFFFF",
    tag: "#FF7AB2",
    attribute: "#D0A8FF",
    escape: "#FC6A5D",
    preproc: "#FD8F3F",
    heading: "#FF7AB2",
    link: "#5482FF",
    invalid: "#FF5C57",
    keyword_bold: true,
    comment_italic: false,
    gutter: None,
};

const GITHUB_LIGHT: Palette = Palette {
    name: "GitHub Light",
    author: "Palette in the style of GitHub Light",
    bg: "#FFFFFF",
    fg: "#1F2328",
    caret: "#0969DA",
    selection: "#BBDFFF",
    line: "#F6F8FA",
    invisibles: "#00000026",
    comment: "#6E7781",
    string: "#0A3069",
    keyword: "#CF222E",
    constant: "#0550AE",
    function: "#8250DF",
    ty: "#953800",
    variable: "#1F2328",
    parameter: "#24292F",
    operator: "#CF222E",
    tag: "#116329",
    attribute: "#0550AE",
    escape: "#0550AE",
    preproc: "#CF222E",
    heading: "#0550AE",
    link: "#0969DA",
    invalid: "#82071E",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const GITHUB_DARK: Palette = Palette {
    name: "GitHub Dark",
    author: "Palette in the style of GitHub Dark",
    bg: "#0D1117",
    fg: "#E6EDF3",
    caret: "#2F81F7",
    selection: "#264F7880",
    line: "#FFFFFF0D",
    invisibles: "#FFFFFF26",
    comment: "#8B949E",
    string: "#A5D6FF",
    keyword: "#FF7B72",
    constant: "#79C0FF",
    function: "#D2A8FF",
    ty: "#FFA657",
    variable: "#E6EDF3",
    parameter: "#FFA657",
    operator: "#FF7B72",
    tag: "#7EE787",
    attribute: "#79C0FF",
    escape: "#79C0FF",
    preproc: "#FF7B72",
    heading: "#79C0FF",
    link: "#A5D6FF",
    invalid: "#F85149",
    keyword_bold: false,
    comment_italic: false,
    gutter: None,
};

const ONE_DARK: Palette = Palette {
    name: "One Dark",
    author: "In the spirit of Atom One Dark",
    bg: "#282C34",
    fg: "#ABB2BF",
    caret: "#528BFF",
    selection: "#3E4451",
    line: "#2C313C",
    invisibles: "#FFFFFF26",
    comment: "#5C6370",
    string: "#98C379",
    keyword: "#C678DD",
    constant: "#D19A66",
    function: "#61AFEF",
    ty: "#E5C07B",
    variable: "#E06C75",
    parameter: "#D19A66",
    operator: "#56B6C2",
    tag: "#E06C75",
    attribute: "#D19A66",
    escape: "#56B6C2",
    preproc: "#C678DD",
    heading: "#E06C75",
    link: "#61AFEF",
    invalid: "#E06C75",
    keyword_bold: false,
    comment_italic: true,
    gutter: None,
};

const ALL: &[&Palette] = &[
    &LIGHT,
    &DARK,
    &SOLARIZED_LIGHT,
    &SOLARIZED_DARK,
    &TOMORROW,
    &TOMORROW_NIGHT,
    &XCODE,
    &XCODE_DARK,
    &GITHUB_LIGHT,
    &GITHUB_DARK,
    &ONE_DARK,
    &MONOKAI,
    &TWILIGHT,
    &DRACULA,
    &NORD,
];

fn c(hex: &str) -> Rgba {
    // The tables are authored here; a typo must not panic at runtime.
    Rgba::parse(hex).unwrap_or(Rgba::TRANSPARENT)
}

fn font(bold: bool, italic: bool) -> Option<FontStyle> {
    Some(FontStyle {
        bold,
        italic,
        underline: false,
    })
}

fn from_palette(p: &Palette) -> Theme {
    let item = |name: &str, scope: &str, color: &str, style: Option<FontStyle>| {
        ThemeItem::new(name, scope, c(color), style)
    };
    let declaring = [
        "function", "class", "struct", "enum", "interface", "trait", "impl", "type", "module", "namespace",
    ];
    let keyword_scope = {
        let mut s = String::from("keyword - keyword.operator - constant.numeric, storage.modifier");
        for d in declaring {
            s.push_str(", storage.type.");
            s.push_str(d);
        }
        s
    };
    let type_scope = {
        let mut s = String::from(
            "entity.name.class, entity.name.type, entity.other.inherited-class, support.class, support.type, \
             support.protocol, storage.type",
        );
        for d in declaring {
            s.push_str(" - storage.type.");
            s.push_str(d);
        }
        s
    };
    let items = vec![
        item("Comments", "comment, punctuation.definition.comment", p.comment, font(false, p.comment_italic)),
        item("Operators", "keyword.operator", p.operator, None),
        item("Keywords", &keyword_scope, p.keyword, font(p.keyword_bold, false)),
        item("Numbers and Constants", "constant, support.constant, variable.other.constant", p.constant, None),
        item("Units", "keyword.other.unit", p.constant, None),
        item("Strings", "(string | punctuation.definition.string) - comment", p.string, None),
        item("Escape Characters", "constant.character.escape, constant.other.color", p.escape, None),
        item("Regular Expressions", "string.regexp", p.escape, None),
        item(
            "Functions and Methods",
            "(entity.name.function, entity.name.macro, support.function, support.macro, variable.function, \
             meta.function-call.generic) - punctuation",
            p.function,
            None,
        ),
        item("Types and Classes", &type_scope, p.ty, None),
        item("Variables", "variable - variable.function", p.variable, None),
        item("Parameters", "variable.parameter", p.parameter, None),
        item("Language Variables", "variable.language", p.keyword, font(false, true)),
        item("Tags", "entity.name.tag, punctuation.definition.tag", p.tag, None),
        item("Attributes", "entity.other.attribute-name", p.attribute, None),
        item("Preprocessor and Annotations", "meta.preprocessor, meta.attribute, meta.annotation", p.preproc, None),
        item("Embedded", "punctuation.section.embedded", p.keyword, None),
        item("Headings", "markup.heading, entity.name.section", p.heading, font(true, false)),
        item("Bold", "markup.bold, punctuation.definition.bold", p.fg, font(true, false)),
        item("Italic", "markup.italic, punctuation.definition.italic", p.fg, font(false, true)),
        item("Code", "markup.raw", p.string, None),
        item("Link Text", "string.other.link, meta.link", p.link, None),
        item("Lists", "markup.list", p.attribute, None),
        item("Quotes", "markup.quote", p.comment, font(false, true)),
        item("Inserted", "markup.inserted", p.string, None),
        item("Deleted", "markup.deleted", p.invalid, None),
        item("Changed", "markup.changed", p.keyword, None),
        item("Invalid", "invalid", p.invalid, None),
        item("Deprecated", "invalid.deprecated", p.invalid, font(false, true)),
    ];
    let mut theme = Theme {
        name: p.name.to_owned(),
        uuid: None,
        author: Some(p.author.to_owned()),
        comment: None,
        global: GlobalSettings {
            background: Some(c(p.bg)),
            foreground: Some(c(p.fg)),
            caret: Some(c(p.caret)),
            selection: Some(c(p.selection)),
            line_highlight: Some(c(p.line)),
            invisibles: Some(c(p.invisibles)),
            ..GlobalSettings::default()
        },
        gutter: p.gutter.map(|(bg, fg)| GutterSettings {
            background: Some(c(bg)),
            foreground: Some(c(fg)),
            ..GutterSettings::default()
        }),
        items,
        extra: plist::Dictionary::new(),
        origin: ThemeOrigin::Builtin,
        warnings: Vec::new(),
    };
    theme.merge_default_items();
    theme
}

/// All built-in themes, light default first, then dark default, then the
/// classics. Built on every call; keep the result.
pub fn builtin_themes() -> Vec<Theme> {
    ALL.iter().map(|p| from_palette(p)).collect()
}

/// A fresh theme named `name` for "New Light Theme" / "New Dark Theme":
/// the default theme of that brightness without author metadata.
pub(crate) fn template(name: &str, dark: bool) -> Theme {
    let mut theme = from_palette(if dark { &DARK } else { &LIGHT });
    theme.name = name.to_owned();
    theme.author = None;
    theme
}

const ANSI: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/// The items every theme gets when it lacks them: links, console
/// informative/error/prompt, and the ANSI bold/italic/underline and colour
/// scopes (`console.ansi.color.foreground.red.regular`, ...).
pub(crate) fn default_items(dark: bool) -> Vec<ThemeItem> {
    let regular = [
        "#000000", "#990000", "#00A600", "#999900", "#0000B2", "#B200B2", "#00A6B2", "#BFBFBF",
    ];
    let bright = [
        "#666666", "#E50000", "#00D900", "#E5E500", "#0000FF", "#E500E5", "#00E5E5", "#E5E5E5",
    ];
    // On a light background the palest ANSI colours get unreadable.
    let regular_light = [
        "#000000", "#990000", "#00A600", "#8A7B00", "#0000B2", "#B200B2", "#00A6B2", "#7F7F7F",
    ];
    let bright_light = [
        "#555555", "#E50000", "#00A000", "#B5A100", "#0000FF", "#E500E5", "#00A5A5", "#9A9A9A",
    ];
    let (regular, bright) = if dark {
        (regular, bright)
    } else {
        (regular_light, bright_light)
    };
    let mut items = vec![
        ThemeItem {
            name: Some("Links".into()),
            scope: "markup.underline.link".into(),
            font_style: Some(FontStyle {
                underline: true,
                ..FontStyle::PLAIN
            }),
            ..ThemeItem::default()
        },
        ThemeItem::new(
            "Console Informative Text",
            "console.warning, console.notice, console.debugger, meta.coderunner.console-informative",
            c("#898B8D"),
            None,
        ),
        ThemeItem::new(
            "Console Error Text",
            "console.error, meta.coderunner.console-error",
            c("#FF0028"),
            None,
        ),
        ThemeItem::new(
            "Console Debugger Prompt",
            "console.prompt, meta.coderunner.debugger-prompt",
            c("#00AE67"),
            font(true, false),
        ),
    ];
    for (scope, style) in [
        ("console.ansi.bold", FontStyle { bold: true, ..FontStyle::PLAIN }),
        ("console.ansi.italic", FontStyle { italic: true, ..FontStyle::PLAIN }),
        ("console.ansi.underline", FontStyle { underline: true, ..FontStyle::PLAIN }),
    ] {
        items.push(ThemeItem {
            name: Some(format!("ANSI {}", &scope[13..])),
            scope: scope.into(),
            font_style: Some(style),
            ..ThemeItem::default()
        });
    }
    for (kind, colors) in [("regular", regular), ("bright", bright)] {
        for (i, name) in ANSI.iter().enumerate() {
            items.push(ThemeItem::new(
                &format!("ANSI foreground {name} {kind}"),
                &format!("console.ansi.color.foreground.{name}.{kind}"),
                c(colors[i]),
                None,
            ));
        }
    }
    for (kind, colors) in [("regular", regular), ("bright", bright)] {
        for (i, name) in ANSI.iter().enumerate() {
            items.push(ThemeItem {
                name: Some(format!("ANSI background {name} {kind}")),
                scope: format!("console.ansi.color.background.{name}.{kind}"),
                background: Some(c(colors[i])),
                ..ThemeItem::default()
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_colour_parses() {
        // `c` swallows typos as transparent; make that visible here.
        for theme in builtin_themes() {
            let transparent = |color: Option<Rgba>| color.is_some_and(|c| c == Rgba::TRANSPARENT);
            assert!(!transparent(theme.global.background), "{}", theme.name);
            assert!(!transparent(theme.global.selection), "{}", theme.name);
            for item in &theme.items {
                assert!(!transparent(item.foreground), "{}: {}", theme.name, item.scope);
            }
        }
    }

    #[test]
    fn every_selector_parses_and_names_are_unique() {
        let themes = builtin_themes();
        let mut names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), themes.len());
        for theme in &themes {
            for item in &theme.items {
                assert!(item.selector().is_ok(), "{}: {}", theme.name, item.scope);
            }
            assert!(theme.compile().warnings().is_empty());
        }
    }

    #[test]
    fn classification_matches_names() {
        for theme in builtin_themes() {
            let expect_dark = !(theme.name.contains("Light") || theme.name == "Tomorrow");
            assert_eq!(theme.is_dark(), expect_dark, "{}", theme.name);
        }
    }

    #[test]
    fn default_items_cover_console_and_ansi() {
        let theme = from_palette(&DARK);
        let r = theme.compile();
        let error = r.style_for(&["console.error"]);
        assert_eq!(error.foreground, c("#FF0028"));
        let red = r.style_for(&["console", "console.ansi.color.foreground.red.regular"]);
        assert_eq!(red.foreground, c("#990000"));
        let bg = r.style_for(&["console.ansi.color.background.blue.bright"]);
        assert_eq!(bg.background, Some(c("#0000FF")));
        assert!(r.style_for(&["console.ansi.bold"]).bold);
    }

    #[test]
    fn rust_scopes_resolve_to_distinct_colours() {
        let r = from_palette(&DARK).compile();
        let kw = r.style_for(&["source.rust", "storage.type.function.rust"]);
        let string = r.style_for(&["source.rust", "string.quoted.double.rust"]);
        let comment = r.style_for(&["source.rust", "comment.line.double-slash.rust"]);
        let ty = r.style_for(&["source.rust", "entity.name.type.rust"]);
        let colours = [kw.foreground, string.foreground, comment.foreground, ty.foreground];
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(comment.italic);
        // A struct's type scope is a type, `i32` (storage.type.rust) too, `fn` a keyword.
        assert_eq!(r.style_for(&["source.rust", "storage.type.rust"]).foreground, ty.foreground);
    }

    #[test]
    fn template_is_named_and_plain() {
        let t = Theme::template("Mine", false);
        assert_eq!(t.name, "Mine");
        assert!(t.author.is_none());
        assert!(!t.is_dark());
    }
}
