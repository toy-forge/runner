//! `SearchField`: DESIGN.md 7.5. A capsule `input()` with a leading
//! `search` (or `filter`) glyph, an optional trailing count and a clear
//! button, and an invalid state for a bad regular expression.

use nsh_base::input;
use nsh_core::prelude::*;

use super::ControlSize;
use crate::icon::{IconName, IconSize, Tone, glyph};
use crate::motion;
use crate::tokens;
use crate::typography::type_caption;

/// A [`search_field`]'s size and states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchFieldOptions {
    /// The size (default [`ControlSize::Sm`], the find bar).
    pub size: ControlSize,
    /// The leading glyph (default `search`; `filter` in the console).
    pub leading: IconName,
    /// A bad pattern: `destructive` outline and trailing text.
    pub invalid: bool,
    /// No typing; 45% opacity.
    pub disabled: bool,
    /// No outline or fill (QuickNav's `lg` field).
    pub borderless: bool,
}

impl Default for SearchFieldOptions {
    fn default() -> Self {
        Self {
            size: ControlSize::Sm,
            leading: IconName::Search,
            invalid: false,
            disabled: false,
            borderless: false,
        }
    }
}

impl SearchFieldOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// With a leading `leading` glyph (`Search` or `Filter`).
    pub fn leading(mut self, leading: IconName) -> Self {
        self.leading = leading;
        self
    }

    /// Invalid or not.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Borderless or not.
    pub fn borderless(mut self, borderless: bool) -> Self {
        self.borderless = borderless;
        self
    }
}

/// A search field: `value` is the controlled text, `trailing` an optional
/// caption (a match count or a reason), `on_change` receives every edit
/// and `on_clear` clears the text (the caller sets its value).
pub fn search_field(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    trailing: impl IntoReactive<Option<SharedString>> + 'static,
    on_change: impl Fn(&str) + 'static,
    on_clear: impl Fn() + 'static,
    options: SearchFieldOptions,
) -> impl IntoElement {
    let size = options.size;
    let invalid = options.invalid;
    let disabled = options.disabled;
    let value = value.into_reactive();
    let trailing = trailing.into_reactive();
    let text_value = value.clone();
    let clear_shown = value.clone();
    let count_shown = trailing.clone();
    let count_visible = trailing.clone();
    let on_clear = std::rc::Rc::new(on_clear);
    let text_size = if size == ControlSize::Lg {
        tokens::type_subhead()
    } else {
        size.text_size()
    };
    let line_height = if size == ControlSize::Lg {
        tokens::leading_subhead()
    } else if size == ControlSize::Xs {
        tokens::leading_caption()
    } else {
        tokens::leading_body()
    };

    let placeholder = placeholder.into_reactive();
    let field = input()
        .value(move || text_value.get().to_string())
        .on_change(move |text: &str| on_change(text))
        .placeholder(move || placeholder.get().to_string())
        .placeholder_color(move || color(tokens::muted_foreground()))
        .caret_color(move || color(tokens::primary()))
        .selection_color(move || color(tokens::selection()))
        .disabled(disabled)
        .flex_1()
        .min_w(px(0.0))
        .text_size(px(text_size))
        .line_height(px(line_height))
        .text_color(tokens::foreground)
        .focus(|s| {
            s.outline(px(2.0))
                .outline_color(tokens::ring)
                .outline_offset(px(1.0))
        });

    let mut root = view()
        .h(px(size.height()))
        .px(px(size.padding_x()))
        .flex_row()
        .items_center()
        .gap(px(size.gap()))
        .rounded_full()
        .when(!options.borderless, |v| {
            v.bg(tokens::field)
                .border_1()
                .border_color(move || {
                    if invalid {
                        tokens::destructive()
                    } else {
                        tokens::input()
                    }
                })
        })
        .when(disabled, |v| v.opacity(motion::OPACITY_DISABLED))
        .child(glyph(options.leading, IconSize::Xs, Tone::Muted))
        .child(field)
        .child(
            when(
                move || count_visible.get().is_some(),
                move || {
                    let count = count_shown.clone();
                    type_caption()
                        .apply(text(move || count.get().unwrap_or_default()))
                        .text_color(move || {
                            if invalid {
                                tokens::destructive()
                            } else {
                                tokens::muted_foreground()
                            }
                        })
                },
            ),
        )
        .child(
            when(
                move || !clear_shown.get().is_empty(),
                move || {
                    let on_clear = on_clear.clone();
                    nsh_base::button()
                        .accessibility_label("Clear")
                        .size(px(16.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(tokens::radius_xs()))
                        .cursor_pointer()
                        .hover(|s| s.bg(tokens::accent))
                        .on_press(move || on_clear())
                        .child(glyph(IconName::CloseCircle, IconSize::Md, Tone::Muted))
                },
            ),
        );
    if invalid {
        root = root.outline(px(1.0)).outline_color(move || {
            tokens::destructive()
        });
    }
    focus_ring!(root)
}
