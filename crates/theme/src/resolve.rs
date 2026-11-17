//! A theme compiled for scope resolution: parsed selectors, the default
//! style, and `style_for(scope stack)`.

use crate::color::Rgba;
use crate::selector::{Score, ScopeSelector, StackView};
use crate::theme::{Chrome, EditorColors, FontStyle, Theme};

/// The resolved look of a run of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextStyle {
    /// Text colour (never transparent: the theme foreground at least).
    pub foreground: Rgba,
    /// Background behind the text, if an item sets one.
    pub background: Option<Rgba>,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
}

/// An item that matched a scope stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedItem {
    /// Index into [`Theme::items`].
    pub index: usize,
    /// The item's display name, if any.
    pub name: Option<String>,
    /// The item's scope selector text.
    pub scope: String,
    /// How strongly it matched.
    pub score: Score,
}

struct Compiled {
    index: usize,
    selector: ScopeSelector,
    foreground: Option<Rgba>,
    background: Option<Rgba>,
    font_style: Option<FontStyle>,
}

/// A theme ready for fast, thread-safe scope resolution. Immutable; share it
/// in an `Arc` between the UI and the highlighting thread.
pub struct ResolvedTheme {
    name: String,
    is_dark: bool,
    default_style: TextStyle,
    items: Vec<Compiled>,
    labels: Vec<(Option<String>, String)>,
    editor: EditorColors,
    chrome: Chrome,
    warnings: Vec<String>,
}

impl std::fmt::Debug for ResolvedTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedTheme")
            .field("name", &self.name)
            .field("items", &self.items.len())
            .finish()
    }
}

impl ResolvedTheme {
    /// Compiles `theme`. Items with an unparsable selector are skipped and
    /// reported by [`ResolvedTheme::warnings`].
    pub fn new(theme: &Theme) -> ResolvedTheme {
        let editor = theme.editor_colors();
        let mut warnings = Vec::new();
        let mut items = Vec::with_capacity(theme.items.len());
        for (index, item) in theme.items.iter().enumerate() {
            match item.selector() {
                Ok(selector) => items.push(Compiled {
                    index,
                    selector,
                    foreground: item.foreground,
                    background: item.background,
                    font_style: item.font_style,
                }),
                Err(error) => warnings.push(error.to_string()),
            }
        }
        ResolvedTheme {
            name: theme.name.clone(),
            is_dark: theme.is_dark(),
            default_style: TextStyle {
                foreground: editor.foreground.over(editor.background),
                background: None,
                bold: false,
                italic: false,
                underline: false,
            },
            labels: theme
                .items
                .iter()
                .map(|i| (i.name.clone(), i.scope.clone()))
                .collect(),
            items,
            chrome: theme.chrome(),
            editor,
            warnings,
        }
    }

    /// The theme's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the theme is dark.
    pub fn is_dark(&self) -> bool {
        self.is_dark
    }

    /// The style of text no item matches.
    pub fn default_style(&self) -> TextStyle {
        self.default_style
    }

    /// Editor part colours.
    pub fn editor_colors(&self) -> &EditorColors {
        &self.editor
    }

    /// App chrome tints.
    pub fn chrome(&self) -> &Chrome {
        &self.chrome
    }

    /// Selector parse problems found while compiling.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn ranked(&self, view: &StackView<'_>) -> Vec<(Score, usize)> {
        let mut hits: Vec<(Score, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.selector.score_in(view).map(|s| (s, i)))
            .collect();
        // Ascending: the best match is applied last and wins each property.
        hits.sort_unstable();
        hits
    }

    fn apply(&self, hits: &[(Score, usize)]) -> TextStyle {
        let mut style = self.default_style;
        for &(_, i) in hits {
            let item = &self.items[i];
            if let Some(fg) = item.foreground {
                style.foreground = fg.over(self.editor.background);
            }
            if let Some(bg) = item.background {
                style.background = Some(bg);
            }
            if let Some(font) = item.font_style {
                style.bold = font.bold;
                style.italic = font.italic;
                style.underline = font.underline;
            }
        }
        style
    }

    /// The style of text under `stack` (outermost scope first): every
    /// matching item applies in ascending specificity, so the most specific
    /// item wins each of foreground, background and font style.
    pub fn style_for<S: AsRef<str>>(&self, stack: &[S]) -> TextStyle {
        let view = StackView::new(stack);
        self.apply(&self.ranked(&view))
    }

    /// Like [`ResolvedTheme::style_for`] plus the matched items, best match
    /// first (for the Color Scopes panel).
    pub fn explain<S: AsRef<str>>(&self, stack: &[S]) -> (TextStyle, Vec<MatchedItem>) {
        let view = StackView::new(stack);
        let hits = self.ranked(&view);
        let style = self.apply(&hits);
        let matched = hits
            .iter()
            .rev()
            .map(|&(score, i)| {
                let index = self.items[i].index;
                let (name, scope) = self.labels[index].clone();
                MatchedItem {
                    index,
                    name,
                    scope,
                    score,
                }
            })
            .collect();
        (style, matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeItem;

    fn theme() -> Theme {
        let mut t = Theme::template("t", true);
        t.items.clear();
        t.items.push(ThemeItem::new("c", "comment", Rgba::rgb(1, 2, 3), Some(FontStyle::parse("italic"))));
        t.items.push(ThemeItem::new("s", "string", Rgba::rgb(9, 9, 9), None));
        t.items.push(ThemeItem::new("sq", "string.quoted", Rgba::rgb(7, 7, 7), Some(FontStyle::parse("bold"))));
        t.items.push(ThemeItem::new("later", "string", Rgba::rgb(5, 5, 5), None));
        t
    }

    #[test]
    fn most_specific_wins_then_later() {
        let r = theme().compile();
        let style = r.style_for(&["source.x", "string.quoted.double.x"]);
        assert_eq!(style.foreground, Rgba::rgb(7, 7, 7));
        assert!(style.bold);
        let plain = r.style_for(&["source.x", "string.unquoted.x"]);
        assert_eq!(plain.foreground, Rgba::rgb(5, 5, 5));
        assert!(!plain.bold);
    }

    #[test]
    fn properties_combine_across_items() {
        let mut t = theme();
        t.items.push(ThemeItem {
            scope: "string.quoted.double".into(),
            background: Some(Rgba::rgb(1, 1, 1)),
            ..Default::default()
        });
        let style = t.compile().style_for(&["string.quoted.double.x"]);
        assert_eq!(style.foreground, Rgba::rgb(7, 7, 7));
        assert_eq!(style.background, Some(Rgba::rgb(1, 1, 1)));
    }

    #[test]
    fn unmatched_uses_default() {
        let r = theme().compile();
        assert_eq!(r.style_for(&["source.x"]), r.default_style());
    }

    #[test]
    fn explain_lists_best_first() {
        let r = theme().compile();
        let (_, matched) = r.explain(&["source.x", "string.quoted.double.x"]);
        assert_eq!(matched.first().and_then(|m| m.name.as_deref()), Some("sq"));
        assert_eq!(matched.len(), 3);
    }

    #[test]
    fn bad_selector_is_skipped() {
        let mut t = theme();
        t.items.push(ThemeItem {
            scope: "(oops".into(),
            ..Default::default()
        });
        let r = t.compile();
        assert_eq!(r.warnings().len(), 1);
    }
}
