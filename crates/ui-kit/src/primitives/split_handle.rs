//! `SplitHandle`: DESIGN.md 7.25. `resize_handle()` with a 9pt hit band,
//! a 1pt `border` line (`input` on hover, `ring` while dragging) and the
//! resize cursor; callers place it in a relative container and resize from
//! `on_drag_move`.

use nsh_core::{geometry::Axis, prelude::*};

use crate::tokens;

/// A 1pt line for a split boundary: vertical when the split resizes
/// horizontally, horizontal when it resizes vertically. It follows the
/// handle's hover and press (the handle is its group).
pub fn split_line(axis: Axis) -> impl IntoElement {
    let mut line = view()
        .bg(tokens::border)
        .group_hover(|s| s.bg(tokens::input))
        .group_active(|s| s.bg(tokens::ring));
    line = match axis {
        Axis::Horizontal => line.w(px(tokens::hairline())).h_full(),
        Axis::Vertical => line.w_full().h(px(tokens::hairline())),
    };
    line
}

/// A resize handle along `axis`, named `label` for screen readers
/// (`Resize sidebar`). Its hit area is `handle-hit` 9pt; the drawn line is
/// 1pt. It positions itself absolutely along the container's edge.
pub fn split_handle(
    axis: Axis,
    label: impl IntoReactive<SharedString> + 'static,
    disabled: bool,
) -> impl IntoElement {
    let label = label.into_reactive();
    nsh_base::resize_handle()
        .axis(axis)
        .accessibility_label(move || Some(label.get()))
        .disabled(disabled)
        .child(split_line(axis))
}
