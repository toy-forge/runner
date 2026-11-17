//! `IconButton`: DESIGN.md 7.2. A square press target with an icon, an
//! accessible label (required) and a tooltip that repeats the label.

use nsh_core::prelude::*;

use super::ControlSize;
use crate::icon::{IconName, glyph};
use crate::motion;
use crate::tokens;

/// How an [`icon_button`] is filled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconButtonVariant {
    /// Transparent until hovered; the default.
    #[default]
    Ghost,
    /// A member of a capsule group: capsule hover and pressed washes.
    Capsule,
    /// A quiet filled square (`secondary`).
    Filled,
}

/// An [`icon_button`]'s variant, size and states.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IconButtonOptions {
    /// The variant (default [`IconButtonVariant::Ghost`]).
    pub variant: IconButtonVariant,
    /// The size (default [`ControlSize::Md`]).
    pub size: ControlSize,
    /// Toggled on: `capsule-selected` wash and a `primary` icon.
    pub selected: bool,
    /// No presses and no tooltip interaction; 45% opacity.
    pub disabled: bool,
    /// The width when it is not square (capsule group members are often
    /// 36 or 40 wide at `md`).
    pub width: Option<f32>,
    /// The shortcut shown after the tooltip text, e.g. `"⌘R"`.
    pub shortcut: Option<&'static str>,
}

impl IconButtonOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `variant`.
    pub fn variant(mut self, variant: IconButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Toggled on or not.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// With `width` points (square by default).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// With a tooltip shortcut.
    pub fn shortcut(mut self, shortcut: &'static str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }
}

/// An icon button. `label` is both the accessible name and the tooltip
/// (`shortcut` follows it as a `Kbd`). `on_press` activates it (a click,
/// Space or Return).
pub fn icon_button(
    icon: IconName,
    label: impl IntoReactive<SharedString> + 'static,
    options: IconButtonOptions,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let size = options.size;
    let variant = options.variant;
    let selected = options.selected;
    let disabled = options.disabled;
    let height = match variant {
        IconButtonVariant::Capsule => size.capsule_member_height(),
        _ => size.height(),
    };
    let width = options.width.unwrap_or(height);
    let label = label.into_reactive();
    let a11y_label = label.clone();
    let tooltip_label = label.clone();

    let mut element = nsh_base::button()
        .accessibility_label(move || Some(a11y_label.get()))
        .w(px(width))
        .h(px(height))
        .flex_row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .transition_colors()
        .duration(motion::instant)
        .ease_out()
        .disabled(disabled)
        .on_press(move || {
            if !disabled {
                on_press();
            }
        });

    element = match size {
        ControlSize::Xs if variant == IconButtonVariant::Ghost => {
            element.hit_slop(EdgeInsets::all(px(4.0)))
        }
        _ => element,
    };
    element = match variant {
        IconButtonVariant::Capsule => element.rounded_full().bg(move || {
            if selected { tokens::capsule_selected() } else { AdaptiveColor::TRANSPARENT }
        }),
        _ => element.rounded(px(tokens::radius_sm())),
    };

    element = match variant {
        IconButtonVariant::Ghost => element
            .bg(move || {
                if selected {
                    tokens::capsule_selected()
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(tokens::accent))
            .active(|s| s.bg(tokens::capsule_pressed)),
        IconButtonVariant::Capsule => element
            .bg(move || {
                if selected {
                    tokens::capsule_selected()
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(tokens::capsule_hover))
            .active(|s| s.bg(tokens::capsule_pressed)),
        IconButtonVariant::Filled => element
            .bg(move || {
                if selected {
                    tokens::capsule_selected()
                } else {
                    tokens::secondary()
                }
            })
            .hover(|s| s.bg(tokens::secondary_hover))
            .active(|s| s.bg(tokens::secondary_active)),
    };

    let icon_color = move || {
        if selected {
            tokens::primary()
        } else {
            match variant {
                IconButtonVariant::Ghost => tokens::muted_foreground(),
                IconButtonVariant::Capsule | IconButtonVariant::Filled => tokens::foreground(),
            }
        }
    };
    let drawn = glyph(icon, size.icon_size(), crate::icon::Tone::Default)
        .text_color(icon_color);

    let element = element
        .transition_transform()
        .duration(motion::fast)
        .ease_bezier(0.22, 1.0, 0.36, 1.0)
        .active(|s| motion::press_scale(s, true))
        .child(drawn);

    let hint = super::tooltip::tooltip(tooltip_label, options.shortcut);
    let button = focus_ring!(disabled_look!(element));
    view().child(button).tooltip(hint)
}
