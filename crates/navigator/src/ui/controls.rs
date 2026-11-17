//! Small styled controls (the app's `ui/` layer wraps `nsh_base` once).

use nsh_base::button;
use nsh_core::{element::ImageSrc, prelude::*};

use super::theme;

/// An icon-only 22x22 button with hover/active feedback and an accessibility label.
pub fn icon_button(
    markup: &'static str,
    label: &'static str,
    on_press: impl Fn(&PressEvent) + 'static,
) -> impl IntoElement {
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<&'static str, ImageSrc>> = Default::default();
    }
    let src = CACHE.with(|c| {
        c.borrow_mut()
            .entry(markup)
            .or_insert_with(|| ImageSrc::svg_markup(markup))
            .clone()
    });
    let tip = text(label)
        .text_size(px(11.0))
        .px(px(6.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .bg(move || nsh_base::theme().popover)
        .text_color(move || nsh_base::theme().popover_foreground);
    // `Button` is not `Interactive`: the tooltip goes on a wrapper. A button's press carries no
    // position, so `on_press` here gets a default event; use `view().on_press` where a position is needed.
    view().flex_shrink(0.0).tooltip(tip).child(
        button()
            .accessibility_label(label)
            .w(px(22.0))
            .h(px(22.0))
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .hover(|s| s.bg(theme::row_hover))
            .active(|s| s.opacity(0.6))
            .disabled_style(|s| s.opacity(0.35))
            .on_press(move |_: &()| on_press(&PressEvent::default()))
            .child(svg(src).size(px(14.0)).text_color(theme::muted)),
    )
}

/// A small text toggle (`Aa`, `.*`) with an on state.
pub fn chip(
    label: &'static str,
    tip: &'static str,
    on: impl IntoReactive<bool> + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let on = on.into_reactive();
    let on = computed(move || match &on {
        Reactive::Static(v) => *v,
        Reactive::Bound(f) => f(),
    });
    button()
        .accessibility_label(tip)
        .h(px(20.0))
        .px(px(6.0))
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(move || {
            if on.get() {
                theme::selection()
            } else {
                theme::row_hover()
            }
        })
        .active(|s| s.opacity(0.7))
        .on_press(move |_: &()| on_press())
        .child(
            text(label)
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if on.get() {
                        theme::selection_fg()
                    } else {
                        theme::muted()
                    }
                }),
        )
}

/// A rounded text button.
pub fn pill(label: &'static str, on_press: impl Fn() + 'static) -> impl IntoElement {
    button()
        .accessibility_label(label)
        .h(px(22.0))
        .px(px(8.0))
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .border_1()
        .border_color(theme::separator)
        .hover(|s| s.bg(theme::row_hover))
        .active(|s| s.opacity(0.6))
        .disabled_style(|s| s.opacity(0.4))
        .on_press(move |_: &()| on_press())
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(theme::fg),
        )
}

/// A search-field-like text input.
pub fn field(placeholder: &'static str) -> nsh_base::Input {
    nsh_base::input()
        .placeholder(placeholder)
        .placeholder_color(|| color(nsh_base::theme().muted_foreground))
        .caret_color(|| color(theme::selection()))
        .h(px(22.0))
        .px(px(7.0))
        .text_size(px(12.0))
        .rounded(px(5.0))
        .bg(theme::row_hover)
        .border_1()
        .border_color(theme::separator)
        .focus(|s| s.border_color(theme::selection))
}
