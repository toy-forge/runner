//! `ColorWell`: DESIGN.md 7.10. A swatch (44 x 28 `well`, or 24 x 16
//! `chip`) with an `input` outline and E1, which opens a popover holding
//! `color_picker()`: a saturation/brightness area, hue and alpha sliders, a
//! hex field and the system eye dropper.

use nsh_base::{
    ColorChannel, ColorFormat, ColorPickerEvent, color_area, color_eye_dropper, color_field,
    color_picker, color_slider, popover,
};
use nsh_core::prelude::*;

use crate::icon::{IconName, IconSize, Tone, glyph};
use crate::tokens;
use crate::typography::type_caption;

/// The two color-well sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorWellSize {
    /// 44 x 28 with a caption below: the six global theme colors.
    #[default]
    Well,
    /// 24 x 16: the theme element table's FG/BG cells.
    Chip,
}

/// A [`color_well`]'s size and state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorWellOptions {
    /// The size (default [`ColorWellSize::Well`]).
    pub size: ColorWellSize,
    /// No press; 45% opacity.
    pub disabled: bool,
}

impl ColorWellOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `size`.
    pub fn size(mut self, size: ColorWellSize) -> Self {
        self.size = size;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A color well. `name` captions it and names it for screen readers;
/// `value` is the controlled color (`None`: unset, shown with a
/// `destructive` slash); `on_change` receives every color the picker
/// reports.
pub fn color_well(
    name: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<Option<Hsla>> + 'static,
    on_change: impl Fn(Hsla) + 'static,
    options: ColorWellOptions,
) -> impl IntoElement {
    let value = value.into_reactive();
    let name = name.into_reactive();
    let (width, height, radius): (f32, f32, f32) = match options.size {
        ColorWellSize::Well => (44.0, 28.0, tokens::radius()),
        ColorWellSize::Chip => (24.0, 16.0, tokens::radius_xs()),
    };
    let swatch_bg = value.clone();
    let swatch_label_value = value.clone();
    let swatch_label_name = name.clone();
    let slash_value = value.clone();
    let diagonal = (width * width + height * height).sqrt() + 4.0;
    let angle = -(height / width).atan().to_degrees();

    let swatch = view()
        .w(px(width))
        .h(px(height))
        .flex_row()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded(px(radius))
        .border_1()
        .border_color(tokens::input)
        .shadow_xs()
        .shadow_color(tokens::shadow())
        .bg(move || {
            swatch_bg
                .get()
                .map(AdaptiveColor::from)
                .unwrap_or(AdaptiveColor::TRANSPARENT)
        })
        .role(AccessibilityRole::Button)
        .disabled(options.disabled)
        .label(move || {
            let color = swatch_label_value.get();
            let hex = color
                .map(|c| c.to_hex())
                .unwrap_or_else(|| "unset".to_owned());
            SharedString::from(format!("{} color, {hex}", swatch_label_name.get()))
        })
        .when(options.disabled, |v| {
            v.opacity(crate::motion::OPACITY_DISABLED)
        })
        .child(
            when(move || slash_value.get().is_none(), move || {
                view()
                    .w(px(diagonal))
                    .h(px(1.0))
                    .bg(tokens::destructive)
                    .rotate(deg(angle))
            }),
        )
        .into_element();

    let caption_name = name.clone();
    let trigger = match options.size {
        ColorWellSize::Well => view()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .child(swatch)
            .child(
                type_caption()
                    .apply(text(move || caption_name.get()))
                    .text_color(tokens::muted_foreground),
            )
            .into_element(),
        ColorWellSize::Chip => view().child(swatch).into_element(),
    };

    let picker_value = value.clone();
    let picker = color_picker()
        .value(move || picker_value.get())
        .format(ColorFormat::Hex)
        .disabled(options.disabled)
        .on_change(move |event: &ColorPickerEvent| {
            if let ColorPickerEvent::Change(color, _) = event {
                on_change(*color);
            }
        })
        .w(px(216.0))
        .flex_col()
        .gap(px(8.0))
        .child(
            color_area()
                .accessibility_label("Saturation and brightness")
                .w(px(216.0))
                .h(px(140.0))
                .rounded(px(tokens::radius_sm()))
                .thumb_style(|s| {
                    s.size(px(12.0))
                        .rounded_full()
                        .border_1()
                        .border_color(tokens::background)
                        .shadow_xs()
                        .shadow_color(tokens::shadow())
                }),
        )
        .child(
            color_slider()
                .channel(ColorChannel::Hue)
                .accessibility_label("Hue")
                .h(px(12.0))
                .rounded_full()
                .track_style(|s| s.h(px(12.0)).rounded_full())
                .thumb_style(|s| {
                    s.size(px(12.0))
                        .rounded_full()
                        .border_1()
                        .border_color(tokens::background)
                        .shadow_xs()
                        .shadow_color(tokens::shadow())
                }),
        )
        .child(
            color_slider()
                .channel(ColorChannel::Alpha)
                .accessibility_label("Opacity")
                .h(px(12.0))
                .rounded_full()
                .track_style(|s| s.h(px(12.0)).rounded_full())
                .thumb_style(|s| {
                    s.size(px(12.0))
                        .rounded_full()
                        .border_1()
                        .border_color(tokens::background)
                        .shadow_xs()
                        .shadow_color(tokens::shadow())
                }),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    color_field()
                        .format(ColorFormat::Hex)
                        .accessibility_label("Color")
                        .flex_1()
                        .h(px(24.0))
                        .px(px(6.0))
                        .rounded(px(tokens::radius_sm()))
                        .border_1()
                        .border_color(tokens::input)
                        .bg(tokens::field)
                        .text_size(px(tokens::type_caption()))
                        .text_color(tokens::foreground),
                )
                .child(
                    color_eye_dropper()
                        .accessibility_label("Pick a color on the screen")
                        .h(px(24.0))
                        .px(px(6.0))
                        .flex_row()
                        .items_center()
                        .rounded(px(tokens::radius_sm()))
                        .border_1()
                        .border_color(tokens::input)
                        .bg(tokens::control)
                        .hover(|s| s.bg(tokens::control_hover))
                        .child(glyph(IconName::Eye, IconSize::Xs, Tone::Muted)),
                ),
        );

    popover()
        .offset(px(6.0))
        .popup_style(|s| {
            s.w(px(240.0))
                .p(px(12.0))
                .rounded(px(tokens::radius_lg()))
                .border_1()
                .border_color(tokens::border)
                .bg(tokens::popover)
                .shadow_md()
                .shadow_color(tokens::shadow())
        })
        .trigger(trigger)
        .child(picker)
}
