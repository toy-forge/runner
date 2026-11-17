//! Reading and writing `.tmTheme` property lists, and importing legacy
//! `.crTheme` files.

use std::io::Cursor;

use plist::{Dictionary, Value};

use crate::color::Rgba;
use crate::error::ThemeError;
use crate::theme::{FontStyle, GlobalSettings, GutterSettings, Theme, ThemeItem, ThemeOrigin};

fn read_value(data: &[u8]) -> Result<Value, ThemeError> {
    Value::from_reader(Cursor::new(data))
        .or_else(|first| Value::from_reader_ascii(data).map_err(|_| first))
        .map_err(|e| ThemeError::Plist(e.to_string()))
}

/// Takes `key` out of `dict` as a colour, noting a warning when present but
/// invalid.
fn take_color(dict: &mut Dictionary, key: &str, warnings: &mut Vec<String>) -> Option<Rgba> {
    let value = dict.remove(key)?;
    let text = value.as_string()?;
    let parsed = Rgba::parse(text);
    if parsed.is_none() {
        warnings.push(format!("invalid colour `{text}` for `{key}`"));
    }
    parsed
}

fn take_string(dict: &mut Dictionary, key: &str) -> Option<String> {
    match dict.remove(key)? {
        Value::String(s) => Some(s),
        other => {
            dict.insert(key.to_owned(), other);
            None
        }
    }
}

fn color_value(color: Rgba) -> Value {
    Value::String(color.to_hex())
}

impl Theme {
    /// Parses a `.tmTheme` (XML, binary or ASCII property list). A property
    /// list with a `BackgroundColor` key and no `settings` is treated as a
    /// legacy `.crTheme` ([`Theme::from_cr_theme`]).
    ///
    /// The result is NOT merged with the default console/ANSI items; call
    /// [`Theme::merge_default_items`] for that.
    ///
    /// # Errors
    /// [`ThemeError::Plist`] / [`ThemeError::Format`] when the data is not a
    /// theme. Bad colours or selectors inside an otherwise valid theme are
    /// reported in [`Theme::warnings`] instead.
    pub fn from_plist(data: &[u8]) -> Result<Theme, ThemeError> {
        let Value::Dictionary(mut root) = read_value(data)? else {
            return Err(ThemeError::Format("the root is not a dictionary".into()));
        };
        if !root.contains_key("settings") && root.contains_key("BackgroundColor") {
            return Theme::cr_from_dict(&root, "Imported theme");
        }
        let mut theme = Theme {
            origin: ThemeOrigin::Builtin,
            ..Theme::default()
        };
        theme.name = take_string(&mut root, "name").unwrap_or_else(|| "Untitled".to_owned());
        theme.uuid = take_string(&mut root, "uuid");
        theme.author = take_string(&mut root, "author");
        theme.comment = take_string(&mut root, "comment");
        let Some(Value::Array(settings)) = root.remove("settings") else {
            return Err(ThemeError::Format("no `settings` array".into()));
        };
        if let Some(Value::Dictionary(mut g)) = root.remove("gutterSettings") {
            let mut w = Vec::new();
            theme.gutter = Some(GutterSettings {
                background: take_color(&mut g, "background", &mut w),
                foreground: take_color(&mut g, "foreground", &mut w),
                divider: take_color(&mut g, "divider", &mut w),
                selection_background: take_color(&mut g, "selectionBackground", &mut w),
                selection_foreground: take_color(&mut g, "selectionForeground", &mut w),
                selection_border: take_color(&mut g, "selectionBorder", &mut w),
            });
            theme.warnings.extend(w);
        }
        theme.extra = root;
        let mut seen_global = false;
        for element in settings {
            let Value::Dictionary(mut element) = element else {
                continue;
            };
            let name = take_string(&mut element, "name");
            let scope = take_string(&mut element, "scope");
            let Some(Value::Dictionary(mut s)) = element.remove("settings") else {
                continue;
            };
            if s.is_empty() {
                continue;
            }
            let is_global = scope.as_deref().map(str::trim).unwrap_or("").is_empty();
            let mut w = Vec::new();
            if is_global && !seen_global {
                seen_global = true;
                theme.global = GlobalSettings {
                    background: take_color(&mut s, "background", &mut w),
                    foreground: take_color(&mut s, "foreground", &mut w),
                    caret: take_color(&mut s, "caret", &mut w),
                    selection: take_color(&mut s, "selection", &mut w),
                    selection_border: take_color(&mut s, "selectionBorder", &mut w),
                    line_highlight: take_color(&mut s, "lineHighlight", &mut w),
                    invisibles: take_color(&mut s, "invisibles", &mut w),
                    find_highlight: take_color(&mut s, "findHighlight", &mut w),
                    find_highlight_foreground: take_color(&mut s, "findHighlightForeground", &mut w),
                    active_guide: take_color(&mut s, "activeGuide", &mut w),
                    brackets_foreground: take_color(&mut s, "bracketsForeground", &mut w),
                    extra: s,
                };
            } else if !is_global {
                let font_style = take_string(&mut s, "fontStyle").map(|t| FontStyle::parse(&t));
                theme.items.push(ThemeItem {
                    name,
                    scope: scope.unwrap_or_default(),
                    foreground: take_color(&mut s, "foreground", &mut w),
                    background: take_color(&mut s, "background", &mut w),
                    font_style,
                    extra: s,
                });
            }
            theme.warnings.extend(w);
        }
        for item in &theme.items {
            if let Err(error) = item.selector() {
                theme.warnings.push(error.to_string());
            }
        }
        Ok(theme)
    }

    /// Serialises to an XML `.tmTheme`. Unknown keys read by
    /// [`Theme::from_plist`] are written back, so third-party themes
    /// round-trip.
    ///
    /// # Errors
    /// [`ThemeError::Plist`] when serialisation fails (practically never).
    pub fn to_plist_xml(&self) -> Result<String, ThemeError> {
        let mut root = Dictionary::new();
        root.insert("name".into(), Value::String(self.name.clone()));
        if let Some(u) = &self.uuid {
            root.insert("uuid".into(), Value::String(u.clone()));
        }
        if let Some(a) = &self.author {
            root.insert("author".into(), Value::String(a.clone()));
        }
        if let Some(c) = &self.comment {
            root.insert("comment".into(), Value::String(c.clone()));
        }
        for (k, v) in self.extra.iter() {
            root.insert(k.clone(), v.clone());
        }
        let g = &self.global;
        let mut global = Dictionary::new();
        for (key, color) in [
            ("background", g.background),
            ("foreground", g.foreground),
            ("caret", g.caret),
            ("selection", g.selection),
            ("selectionBorder", g.selection_border),
            ("lineHighlight", g.line_highlight),
            ("invisibles", g.invisibles),
            ("findHighlight", g.find_highlight),
            ("findHighlightForeground", g.find_highlight_foreground),
            ("activeGuide", g.active_guide),
            ("bracketsForeground", g.brackets_foreground),
        ] {
            if let Some(c) = color {
                global.insert(key.into(), color_value(c));
            }
        }
        for (k, v) in g.extra.iter() {
            global.insert(k.clone(), v.clone());
        }
        let mut settings = vec![Value::Dictionary({
            let mut d = Dictionary::new();
            d.insert("settings".into(), Value::Dictionary(global));
            d
        })];
        for item in &self.items {
            let mut s = Dictionary::new();
            if let Some(c) = item.foreground {
                s.insert("foreground".into(), color_value(c));
            }
            if let Some(c) = item.background {
                s.insert("background".into(), color_value(c));
            }
            if let Some(f) = item.font_style {
                s.insert("fontStyle".into(), Value::String(f.to_text()));
            }
            for (k, v) in item.extra.iter() {
                s.insert(k.clone(), v.clone());
            }
            let mut d = Dictionary::new();
            if let Some(n) = &item.name {
                d.insert("name".into(), Value::String(n.clone()));
            }
            d.insert("scope".into(), Value::String(item.scope.clone()));
            d.insert("settings".into(), Value::Dictionary(s));
            settings.push(Value::Dictionary(d));
        }
        root.insert("settings".into(), Value::Array(settings));
        if let Some(gu) = &self.gutter {
            let mut d = Dictionary::new();
            for (key, color) in [
                ("background", gu.background),
                ("foreground", gu.foreground),
                ("divider", gu.divider),
                ("selectionBackground", gu.selection_background),
                ("selectionForeground", gu.selection_foreground),
                ("selectionBorder", gu.selection_border),
            ] {
                if let Some(c) = color {
                    d.insert(key.into(), color_value(c));
                }
            }
            root.insert("gutterSettings".into(), Value::Dictionary(d));
        }
        let mut out = Vec::new();
        plist::to_writer_xml(&mut out, &Value::Dictionary(root))
            .map_err(|e| ThemeError::Plist(e.to_string()))?;
        String::from_utf8(out).map_err(|e| ThemeError::Plist(e.to_string()))
    }

    /// Imports a legacy CodeRunner 1/2 `.crTheme` property list (keys
    /// `BackgroundColor`, `TextColor`, `SelectionColor`, `CommentColor`,
    /// `KeywordColor`, `StringColor`, `NumberColor`, `FunctionColor`,
    /// `ClassColor`, `VariableColor`, `PreprocessorColor`). The colour
    /// encoding of the real files was not verified (no sample on disk):
    /// strings (`#RRGGBB[AA]` or `"r g b [a]"` floats), dictionaries with
    /// `red/green/blue/alpha` (or `r/g/b/a`) and arrays of 3-4 numbers are
    /// accepted; archived `NSColor` data is not (reported as a warning).
    ///
    /// # Errors
    /// [`ThemeError::Format`] when the data is not a `.crTheme`.
    pub fn from_cr_theme(data: &[u8], name: &str) -> Result<Theme, ThemeError> {
        let Value::Dictionary(root) = read_value(data)? else {
            return Err(ThemeError::Format("the root is not a dictionary".into()));
        };
        if !root.contains_key("BackgroundColor") {
            return Err(ThemeError::Format("no `BackgroundColor`: not a .crTheme".into()));
        }
        Theme::cr_from_dict(&root, name)
    }

    fn cr_from_dict(root: &Dictionary, name: &str) -> Result<Theme, ThemeError> {
        let mut warnings = Vec::new();
        let mut get = |key: &str| -> Option<Rgba> {
            let value = root.get(key)?;
            let color = cr_color(value);
            if color.is_none() {
                warnings.push(format!("unsupported colour encoding for `{key}`"));
            }
            color
        };
        let background = get("BackgroundColor")
            .ok_or_else(|| ThemeError::Format("unreadable `BackgroundColor`".into()))?;
        let mut theme = Theme::template(name, background.luminance() < 0.5);
        theme.items.clear();
        theme.global.background = Some(background);
        if let Some(c) = get("TextColor") {
            theme.global.foreground = Some(c);
            theme.global.caret = Some(c);
        }
        if let Some(c) = get("SelectionColor") {
            theme.global.selection = Some(c);
        }
        for (key, label, scope) in [
            ("CommentColor", "Comments", "comment, punctuation.definition.comment"),
            ("KeywordColor", "Keywords", "keyword - keyword.operator - constant.numeric, storage.modifier"),
            ("StringColor", "Strings", "(string | punctuation.definition.string) - comment"),
            ("NumberColor", "Numbers and Constants", "constant, support.constant"),
            ("FunctionColor", "Functions and Methods", "entity.name.function, support.function"),
            ("ClassColor", "Classes and Types", "entity.name.class, entity.name.type, support.class, support.type"),
            ("VariableColor", "Variables", "variable"),
            ("PreprocessorColor", "Preprocessor", "meta.preprocessor"),
        ] {
            if let Some(c) = get(key) {
                theme.items.push(ThemeItem::new(label, scope, c, None));
            }
        }
        theme.merge_default_items();
        theme.warnings = warnings;
        theme.comment = Some("Imported from a legacy .crTheme".into());
        Ok(theme)
    }
}

/// Reads a legacy colour value in any of the accepted encodings.
fn cr_color(value: &Value) -> Option<Rgba> {
    let unit = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let num = |v: &Value| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64));
    // Components above 1 are taken as 0-255.
    let scale = |v: f64| if v > 1.0 { (v.clamp(0.0, 255.0)).round() as u8 } else { unit(v) };
    match value {
        Value::String(s) => Rgba::parse(s).or_else(|| {
            let parts: Vec<f64> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            match parts.as_slice() {
                [r, g, b] => Some(Rgba::rgb(scale(*r), scale(*g), scale(*b))),
                [r, g, b, a] => Some(Rgba::new(scale(*r), scale(*g), scale(*b), unit(*a))),
                _ => None,
            }
        }),
        Value::Dictionary(d) => {
            let pick = |a: &str, b: &str| d.get(a).or_else(|| d.get(b)).and_then(num);
            let (r, g, b) = (pick("red", "r")?, pick("green", "g")?, pick("blue", "b")?);
            let a = pick("alpha", "a").unwrap_or(1.0);
            Some(Rgba::new(scale(r), scale(g), scale(b), unit(a)))
        }
        Value::Array(items) => {
            let parts: Vec<f64> = items.iter().filter_map(num).collect();
            match parts.as_slice() {
                [r, g, b] => Some(Rgba::rgb(scale(*r), scale(*g), scale(*b))),
                [r, g, b, a] => Some(Rgba::new(scale(*r), scale(*g), scale(*b), unit(*a))),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>name</key><string>Sample</string>
<key>semanticClass</key><string>theme.dark.sample</string>
<key>settings</key><array>
 <dict><key>settings</key><dict>
   <key>background</key><string>#1C1C20</string>
   <key>foreground</key><string>#C0C5CE</string>
   <key>selection</key><string>#FFFFFF33</string>
   <key>bracketsOptions</key><string>underline</string>
 </dict></dict>
 <dict><key>name</key><string>Comments</string><key>scope</key><string>comment, punctuation.definition.comment</string>
   <key>settings</key><dict><key>foreground</key><string>#5C697F</string><key>fontStyle</key><string>italic</string></dict></dict>
 <dict><key>name</key><string>Broken</string></dict>
 <dict><key>scope</key><string>(string | punctuation.definition.string) - comment</string>
   <key>settings</key><dict><key>foreground</key><string>#NOPE</string><key>background</key><string>#10203040</string></dict></dict>
</array>
<key>gutterSettings</key><dict><key>background</key><string>#2F2F31</string><key>divider</key><string>#414143</string></dict>
</dict></plist>"##;

    #[test]
    fn parses_global_items_gutter_and_extras() {
        let t = Theme::from_plist(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(t.name, "Sample");
        assert_eq!(t.global.background, Rgba::parse("#1C1C20"));
        assert_eq!(t.global.selection, Rgba::parse("#FFFFFF33"));
        assert!(t.global.extra.contains_key("bracketsOptions"));
        assert!(t.extra.contains_key("semanticClass"));
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[0].font_style, Some(FontStyle::parse("italic")));
        assert_eq!(t.items[1].foreground, None);
        assert_eq!(t.items[1].background, Rgba::parse("#10203040"));
        assert_eq!(t.gutter.as_ref().and_then(|g| g.divider), Rgba::parse("#414143"));
        assert!(t.warnings.iter().any(|w| w.contains("#NOPE")));
    }

    #[test]
    fn round_trips_through_xml() {
        let a = Theme::from_plist(SAMPLE.as_bytes()).expect("parse");
        let xml = a.to_plist_xml().expect("write");
        let mut b = Theme::from_plist(xml.as_bytes()).expect("reparse");
        b.warnings = a.warnings.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_non_themes() {
        assert!(Theme::from_plist(b"hello").is_err());
        assert!(Theme::from_plist(b"<plist><dict>").is_err());
        let array = b"<?xml version=\"1.0\"?><plist version=\"1.0\"><array/></plist>";
        assert!(matches!(Theme::from_plist(array), Err(ThemeError::Format(_))));
    }

    #[test]
    fn imports_legacy_cr_theme() {
        let xml = r##"<?xml version="1.0"?><plist version="1.0"><dict>
<key>BackgroundColor</key><string>#101010</string>
<key>TextColor</key><string>1 1 1 1</string>
<key>SelectionColor</key><dict><key>red</key><real>0.2</real><key>green</key><real>0.4</real><key>blue</key><real>0.6</real><key>alpha</key><real>0.5</real></dict>
<key>CommentColor</key><array><real>0.5</real><real>0.5</real><real>0.5</real></array>
<key>KeywordColor</key><data>AAEC</data>
</dict></plist>"##;
        let t = Theme::from_plist(xml.as_bytes()).expect("crTheme");
        assert!(t.is_dark());
        assert_eq!(t.global.foreground, Some(Rgba::WHITE));
        assert_eq!(t.global.selection, Some(Rgba::new(51, 102, 153, 128)));
        assert!(t.items.iter().any(|i| i.name.as_deref() == Some("Comments")));
        assert!(t.items.iter().any(|i| i.scope.starts_with("console.error")));
        assert!(t.warnings.iter().any(|w| w.contains("KeywordColor")));
    }
}
