//! `FormRow`: DESIGN.md 7.18. A right-aligned 200pt label column, a 16pt
//! gap and a flexible control column; an optional caption sits under the
//! control, at most 340 wide.

use nsh_core::prelude::*;

use super::{IconButtonOptions, IconButtonVariant, ControlSize, icon_button};
use crate::icon::IconName;
use crate::tokens;
use crate::typography::{type_body, type_caption};

/// A [`form_row`]'s label column and caption.
#[derive(Clone, Debug, PartialEq)]
pub struct FormRowOptions {
    /// The label column's width in points (default 200; the Languages pane
    /// uses 120).
    pub label_width: f32,
    /// A caption under the control: what the field means or why it is
    /// needed, at most 340 wide.
    pub caption: Option<SharedString>,
}

impl Default for FormRowOptions {
    fn default() -> Self {
        Self {
            label_width: 200.0,
            caption: None,
        }
    }
}

impl FormRowOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With a `label_width`.
    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }

    /// With a `caption` under the control.
    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

/// One labelled form row. An empty `label` keeps the 200pt column for the
/// controls to share one spine (the boolean-row rule).
pub fn form_row(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    options: FormRowOptions,
) -> impl IntoElement {
    let column = view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .gap(px(4.0))
        .child(control)
        .when_some(options.caption, |v, caption| {
            v.child(
                type_caption()
                    .apply(
                        text(caption)
                            .max_w(px(340.0))
                            .text_color(tokens::muted_foreground),
                    ),
            )
        });
    view()
        .w_full()
        .min_h(px(28.0))
        .flex_row()
        .items_start()
        .gap(px(16.0))
        .child(
            view()
                .w(px(options.label_width))
                .flex_shrink_0()
                .pt(px(4.0))
                .child(
                    type_body().apply(
                        text(label)
                            .text_right()
                            .text_color(tokens::foreground),
                    ),
                ),
        )
        .child(column)
}

/// A [`form_row`] with a caption under its control.
pub fn form_row_with_caption(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    caption: impl Into<SharedString>,
    options: FormRowOptions,
) -> impl IntoElement {
    form_row(label, control, options.caption(caption))
}

/// A [`form_row`] with the 20pt help `?` at its trailing end, which opens
/// the Help window at `on_help`'s topic.
pub fn form_row_with_help(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    on_help: impl Fn() + 'static,
    options: FormRowOptions,
) -> impl IntoElement {
    let row = form_row(label, control, options);
    let help = icon_button(
        IconName::HelpCircle,
        SharedString::from("Help"),
        IconButtonOptions::new()
            .variant(IconButtonVariant::Ghost)
            .size(ControlSize::Xs),
        on_help,
    );
    view()
        .w_full()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(view().flex_1().min_w(px(0.0)).child(row))
        .child(view().pt(px(4.0)).flex_shrink_0().child(help))
}
