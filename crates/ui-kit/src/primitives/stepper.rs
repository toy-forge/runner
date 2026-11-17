//! `NumberField` and `Stepper`: DESIGN.md 7.9. A right-aligned number
//! field with a 16pt two-button stepper inside its outline; Up/Down step
//! by `step`, Shift by ten times it.

use nsh_core::prelude::*;

use crate::icon::{IconName, Tone, glyph_at};
use crate::tokens;

/// A number field's bounds and look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberFieldOptions {
    /// The smallest value.
    pub min: Option<f64>,
    /// The largest value.
    pub max: Option<f64>,
    /// One press of the stepper or one arrow key (default 1).
    pub step: f64,
    /// The width in points (default 56).
    pub width: f32,
    /// A unit or label after the number (`pt`, `%`).
    pub suffix: Option<&'static str>,
    /// A unit or label before the number (`§`, `#`).
    pub prefix: Option<&'static str>,
    /// No typing or stepping; 45% opacity.
    pub disabled: bool,
}

impl Default for NumberFieldOptions {
    fn default() -> Self {
        Self {
            min: None,
            max: None,
            step: 1.0,
            width: 56.0,
            suffix: None,
            prefix: None,
            disabled: false,
        }
    }
}

impl NumberFieldOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Between `min` and `max`.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    /// Stepping by `step`.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// `width` points wide.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// With a trailing `suffix` (`pt`).
    pub fn suffix(mut self, suffix: &'static str) -> Self {
        self.suffix = Some(suffix);
        self
    }

    /// With a leading `prefix`.
    pub fn prefix(mut self, prefix: &'static str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A [`NumberField`](number_field) with the default 56pt field.
pub fn stepper(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<f64> + 'static,
    options: NumberFieldOptions,
    on_change: impl Fn(f64) + 'static,
) -> impl IntoElement {
    number_field(label, value, options, on_change)
}

/// A number field (`Label` names it for screen readers) with a two-button
/// stepper. `value` is controlled; `on_change` receives every value the
/// field, the stepper or the arrow keys produce.
pub fn number_field(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<f64> + 'static,
    options: NumberFieldOptions,
    on_change: impl Fn(f64) + 'static,
) -> impl IntoElement {
    let value = value.into_reactive();
    let label = label.into_reactive();
    let root_value = value.clone();
    let field = nsh_base::number_input()
        .value(move || Some(root_value.get()))
        .step(options.step)
        .min(options.min)
        .max(options.max)
        .disabled(options.disabled)
        .controls_right(true)
        .accessibility_label(move || Some(label.get()))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .w(px(options.width))
        .h(px(tokens::ctl_md()))
        .bg(tokens::field)
        .border_1()
        .border_color(tokens::input)
        .rounded(px(tokens::radius()))
        .focus_within(|s| {
            s.outline(px(2.0))
                .outline_color(tokens::ring)
                .outline_offset(px(1.0))
        })
        .input_style(|s| {
            s.flex_1()
                .min_w(px(0.0))
                .px(px(6.0))
                .text_right()
                .tabular_nums()
                .text_size(px(tokens::type_body()))
                .text_color(tokens::foreground)
        })
        .controls_style(|s| s.w(px(16.0)).flex_col().justify_center())
        .increment_button_style(|s| {
            s.w(px(16.0))
                .h(px(10.0))
                .items_center()
                .justify_center()
                .rounded(px(2.0))
        })
        .decrement_button_style(|s| {
            s.w(px(16.0))
                .h(px(10.0))
                .items_center()
                .justify_center()
                .rounded(px(2.0))
        })
        .increment_button(glyph_at(IconName::ChevronUp, 10.0, Tone::Muted))
        .decrement_button(glyph_at(IconName::ChevronDown, 10.0, Tone::Muted))
        .on_change(move |next: &Option<f64>| {
            if let Some(next) = next {
                on_change(*next);
            }
        });
    let unit = |text_value: &'static str| {
        text(SharedString::from(text_value))
            .text_size(px(tokens::type_caption()))
            .line_height(px(tokens::leading_caption()))
            .text_color(tokens::muted_foreground)
    };
    let mut root = view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .opacity(if options.disabled {
            crate::motion::OPACITY_DISABLED
        } else {
            1.0
        });
    if let Some(prefix) = options.prefix {
        root = root.child(unit(prefix));
    }
    root = root.child(field);
    if let Some(suffix) = options.suffix {
        root = root.child(unit(suffix));
    }
    root
}
