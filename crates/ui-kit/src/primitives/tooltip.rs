//! `Tooltip`: DESIGN.md 7.22. The hint bubble of icon-only controls,
//! truncated text and hidden shortcuts: `popover` fill, `radius-sm`, 1pt
//! border, E2, caption text, an optional trailing `Kbd`, 280 wide at most,
//! after the platform's half-second delay.

use nsh_base::TooltipElement;
use nsh_core::prelude::*;

use crate::tokens;
use crate::typography::type_caption;

/// A styled tooltip showing `text` and, after it, `shortcut` as a [`Kbd`].
/// Pass it to an element's `.tooltip(..)`: `element.tooltip(hint("Run",
/// Some("⌘R")))`.
///
/// [`Kbd`]: super::kbd
pub fn tooltip(
    label: impl IntoReactive<SharedString> + 'static,
    shortcut: Option<&'static str>,
) -> TooltipElement {
    let body = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .max_w(px(280.0))
        .child(type_caption().apply(
            text(label)
                .text_color(tokens::foreground)
                .number_of_lines(2u32),
        ))
        .when_some(shortcut, |v, shortcut| {
            v.child(super::kbd::kbd(SharedString::from(shortcut)))
        });
    nsh_base::tooltip()
        .delay(500.0)
        .bg(tokens::popover)
        .rounded(px(tokens::radius_sm()))
        .border_1()
        .border_color(tokens::border)
        .shadow_md()
        .shadow_color(tokens::shadow())
        .px(px(6.0))
        .py(px(4.0))
        .child(body)
}
