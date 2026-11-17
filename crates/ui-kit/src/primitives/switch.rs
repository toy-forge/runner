//! `Switch`: DESIGN.md 7.8. A 36 x 20 capsule track (`input` off,
//! `primary` on) with a 16pt `background` thumb that translates 16pt,
//! over `fast`.

use nsh_core::prelude::*;

use crate::motion;
use crate::tokens;
use crate::typography::type_body;

/// An on/off control that applies at once, with its label in the hit
/// target. `checked` is controlled; `on_change` receives the next value.
pub fn switch(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    disabled: bool,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let checked = checked.into_reactive();
    let element = nsh_base::switch()
        .checked(move || checked.get())
        .disabled(disabled)
        .track_style(|s| {
            s.w(px(36.0))
                .h(px(20.0))
                .p(px(2.0))
                .rounded_full()
                .bg(tokens::input)
                .transition_colors()
                .duration(motion::fast)
                .ease_out()
                .checked(|s| s.bg(tokens::primary))
        })
        .thumb_style(|s| {
            s.w(px(16.0))
                .h(px(16.0))
                .rounded_full()
                .bg(tokens::background)
                .shadow_xs()
                .shadow_color(tokens::shadow())
                .transition_transform()
                .duration(motion::fast)
                .ease_in_out()
                .checked(|s| s.translate_x(px(16.0)))
        })
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .on_change(move |next: &bool| on_change(*next))
        .child(type_body().apply(text(label).text_color(tokens::foreground)));
    focus_ring!(disabled_look!(element))
}
