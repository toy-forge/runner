//! `Button`: DESIGN.md 7.1. A text button with an optional leading icon,
//! six variants, four sizes, loading and disabled states, hover and press
//! washes and the 0.97 press scale.

use nsh_core::prelude::*;

use super::ControlSize;
use crate::icon::{IconName, Tone, glyph};
use crate::motion;
use crate::tokens;

/// The visual weight of a [`button`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ButtonVariant {
    /// The one default commit of a sheet or prompt.
    Primary,
    /// A bordered ordinary action: Cancel, `Options…`.
    #[default]
    Default,
    /// A quiet filled action inside cards.
    Secondary,
    /// A borderless action in a bar or row.
    Ghost,
    /// The destructive commit of a custom dialog.
    Destructive,
    /// An external URL only, underlined on hover.
    Link,
}

/// A [`button`]'s variants, sizes and states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonOptions {
    /// The variant (default [`ButtonVariant::Default`]).
    pub variant: ButtonVariant,
    /// The size (default [`ControlSize::Md`]).
    pub size: ControlSize,
    /// A leading icon.
    pub icon: Option<IconName>,
    /// Dims the label, spins its leading slot and disables activation.
    pub loading: bool,
    /// No presses, 45% opacity.
    pub disabled: bool,
}

impl ButtonOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `variant`.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// With a leading `icon`.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Loading or not.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The text color of `variant`.
fn foreground(variant: ButtonVariant) -> AdaptiveColor {
    match variant {
        ButtonVariant::Primary => tokens::primary_foreground(),
        ButtonVariant::Destructive => tokens::destructive_foreground(),
        ButtonVariant::Secondary => tokens::secondary_foreground(),
        ButtonVariant::Link => tokens::link(),
        ButtonVariant::Default | ButtonVariant::Ghost => tokens::foreground(),
    }
}

/// The icon tone of `variant`.
fn icon_tone(variant: ButtonVariant) -> Tone {
    match variant {
        ButtonVariant::Primary => Tone::OnPrimary,
        ButtonVariant::Destructive => Tone::Default,
        ButtonVariant::Link => Tone::Primary,
        ButtonVariant::Default | ButtonVariant::Secondary | ButtonVariant::Ghost => Tone::Default,
    }
}

/// A text button: `label`, an optional leading icon, a variant and size,
/// and `on_press` for activation (a click, Space or Return). Layout
/// (width, flex, margin) stays the caller's.
pub fn button(
    label: impl IntoReactive<SharedString> + 'static,
    options: ButtonOptions,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let size = options.size;
    let variant = options.variant;
    let loading = options.loading;
    let disabled = options.disabled || loading;
    let link = variant == ButtonVariant::Link;
    let icon_size = size.icon_size();

    let mut element = nsh_base::button()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(size.gap()))
        .cursor_pointer()
        .transition_colors()
        .duration(motion::instant)
        .ease_out()
        .text_color(move || foreground(variant))
        .disabled(disabled)
        .on_press(move || {
            if !disabled {
                on_press();
            }
        });

    if link {
        element = element
            .min_h(px(0.0))
            .px(px(0.0))
            .bg(move || AdaptiveColor::TRANSPARENT)
            .hover(|s| s.underline());
    } else {
        element = element
            .min_h(px(size.height()))
            .px(px(size.padding_x()))
            .min_w(px(size.button_min_width()));
        element = match size {
            ControlSize::Xs | ControlSize::Sm => element.rounded(px(size.rect_radius())),
            ControlSize::Md | ControlSize::Lg => element.rounded_full(),
        };
        element = match variant {
            ButtonVariant::Primary => element
                .bg(tokens::primary)
                .hover(|s| s.bg(tokens::primary_hover))
                .active(|s| s.bg(tokens::primary_active)),
            ButtonVariant::Default => element
                .bg(tokens::control)
                .border_1()
                .border_color(tokens::input)
                .hover(|s| s.bg(tokens::control_hover))
                .active(|s| s.bg(tokens::control_pressed)),
            ButtonVariant::Secondary => element
                .bg(tokens::secondary)
                .hover(|s| s.bg(tokens::secondary_hover))
                .active(|s| s.bg(tokens::secondary_active)),
            ButtonVariant::Ghost => element
                .bg(move || AdaptiveColor::TRANSPARENT)
                .hover(|s| s.bg(tokens::accent))
                .active(|s| s.bg(tokens::capsule_pressed)),
            ButtonVariant::Destructive => element
                .bg(tokens::destructive)
                .hover(|s| s.bg(tokens::destructive_hover))
                .active(|s| s.bg(tokens::destructive_active)),
            ButtonVariant::Link => element,
        };
        if variant != ButtonVariant::Link {
            element = element
                .transition_transform()
                .duration(motion::fast)
                .ease_bezier(0.22, 1.0, 0.36, 1.0)
                .active(|s| motion::press_scale(s, false));
        }
    }

    let with_icon_color = |icon: IconName| {
        let drawn = glyph(icon, icon_size, icon_tone(variant));
        match variant {
            ButtonVariant::Destructive => {
                drawn.text_color(tokens::destructive_foreground).into_element()
            }
            _ => drawn.into_element(),
        }
    };
    let leading = if loading {
        let spinner = glyph(IconName::Spinner, icon_size, icon_tone(variant));
        let spinner = match variant {
            ButtonVariant::Destructive => {
                spinner.text_color(tokens::destructive_foreground)
            }
            _ => spinner,
        };
        Some(motion::spinner(spinner).into_element())
    } else {
        options.icon.map(with_icon_color)
    };

    let text_color_variant = variant;
    let body = view()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(size.gap()))
        .when_some(leading, |v, icon| v.child(icon))
        .child(
            text(label)
                .text_size(px(size.text_size()))
                .line_height(px(if size == ControlSize::Xs {
                    tokens::leading_caption()
                } else {
                    tokens::leading_body()
                }))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || foreground(text_color_variant)),
        );

    let opacity = if loading {
        motion::OPACITY_LOADING
    } else {
        1.0
    };
    focus_ring!(disabled_look!(element.opacity(opacity).child(body)))
}
