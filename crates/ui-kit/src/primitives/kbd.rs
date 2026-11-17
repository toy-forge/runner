//! `Kbd`: DESIGN.md 7.15. A key or chord as a small keycap: 18 high, at
//! least 18 wide, 5 horizontal padding, `radius-xs`, `muted` fill,
//! caption medium `muted-foreground`.

use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;

use crate::tokens;
use crate::typography::type_caption;

/// A keycap showing `label` (glyphs such as `⇧⌘O`, or a word such as
/// `Return`). Screen readers hear the same text.
pub fn kbd(label: impl IntoReactive<SharedString> + 'static) -> impl IntoElement {
    let label = label.into_reactive();
    let shown = label.clone();
    let spoken = label.clone();
    view()
        .role(AccessibilityRole::Text)
        .label(move || spoken.get())
        .h(px(18.0))
        .min_w(px(18.0))
        .px(px(5.0))
        .items_center()
        .justify_center()
        .rounded(px(tokens::radius_xs()))
        .border_1()
        .border_color(tokens::border)
        .bg(tokens::muted)
        .child(
            type_caption()
                .medium()
                .apply(text(move || shown.get()).text_color(tokens::muted_foreground)),
        )
}

/// A keycap for a keystroke as `nsh_keymap` writes it (`"mod+shift+o"`):
/// [`key_label`]'s glyphs, or the keystroke itself when it does not parse
/// (this never panics).
pub fn kbd_for(keystroke: &str) -> impl IntoElement {
    let label = Keystroke::parse_with(keystroke, apple_keys())
        .map(keystroke_label)
        .unwrap_or_else(|_| keystroke.to_owned());
    kbd(SharedString::from(label))
}
