//! `Checkbox`, `Radio` and `RadioGroup`: DESIGN.md 7.7. A 16x16
//! indicator (`radius-xs`, or a full circle for a radio) on `control` with
//! a 1pt `input` outline; checked is `primary` with a `check` (or a dot).

use nsh_core::prelude::*;

use crate::icon::{IconName, IconSize, Tone, glyph};
use crate::tokens;
use crate::typography::type_body;

/// A checkbox's states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckboxOptions {
    /// Neither checked nor unchecked: a dash, announced as mixed.
    pub indeterminate: bool,
    /// No presses; 45% opacity.
    pub disabled: bool,
}

impl CheckboxOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indeterminate or not.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A checkbox with `label` inside its hit target. `checked` is the
/// controlled value; `on_change` receives the value a press asks for.
pub fn checkbox(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    options: CheckboxOptions,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let checked = checked.into_reactive();
    let mark_shown = checked.clone();
    let indeterminate = options.indeterminate;
    let mark = view()
        .w(px(16.0))
        .h(px(16.0))
        .items_center()
        .justify_center()
        .child(
            when(move || mark_shown.get() && !indeterminate, || {
                glyph(IconName::Check, IconSize::Xs, Tone::OnPrimary)
            })
            .otherwise(move || {
                when(indeterminate, || {
                    glyph(IconName::Minus, IconSize::Xs, Tone::OnPrimary)
                })
            }),
        )
        .into_element();

    let element = nsh_base::checkbox()
        .checked(move || checked.get())
        .indeterminate(indeterminate)
        .disabled(options.disabled)
        .indicator_style(|s| {
            s.w(px(16.0))
                .h(px(16.0))
                .items_center()
                .justify_center()
                .rounded(px(tokens::radius_xs()))
                .border_1()
                .border_color(tokens::input)
                .bg(tokens::control)
                .checked(|s| {
                    s.bg(tokens::primary)
                        .border_color(tokens::primary)
                })
                .indeterminate(|s| {
                    s.bg(tokens::primary)
                        .border_color(tokens::primary)
                })
        })
        .indicator(mark)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .transition_colors()
        .duration(crate::motion::instant)
        .ease_out()
        .on_change(move |next: &bool| on_change(*next))
        .child(
            type_body().apply(text(label).text_color(tokens::foreground)),
        );
    focus_ring!(disabled_look!(element))
}

/// A radio group: `label` names the set; its radios are one Tab stop whose
/// arrow keys move and select.
pub fn radio_group(
    label: impl IntoReactive<SharedString> + 'static,
    children: impl IntoIterator<Item = impl IntoElement>,
) -> impl IntoElement {
    let label = label.into_reactive();
    // The label sits on a wrapping view: `RadioGroup` has no label prop in
    // the pinned nsh.
    view()
        .optional_label(move || Some(label.get()))
        .child(
            nsh_base::radio_group()
                .flex_col()
                .gap(px(8.0))
                .children(children.into_iter().map(IntoElement::into_element).collect::<Vec<_>>()),
        )
}

/// One radio of a [`radio_group`]: `checked` is controlled and `on_select`
/// asks to check it.
pub fn radio(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    disabled: bool,
    on_select: impl Fn() + 'static,
) -> impl IntoElement {
    let checked = checked.into_reactive();
    let border_on = checked.clone();
    let fill_on = checked.clone();
    let dot_on = checked.clone();
    let indicator = view()
        .w(px(16.0))
        .h(px(16.0))
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(move || {
            if border_on.get() {
                tokens::primary()
            } else {
                tokens::input()
            }
        })
        .bg(move || {
            if fill_on.get() {
                tokens::primary()
            } else {
                tokens::control()
            }
        })
        .child(
            view()
                .w(px(6.0))
                .h(px(6.0))
                .rounded_full()
                .bg(move || {
                    if dot_on.get() {
                        tokens::primary_foreground()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                }),
        )
        .into_element();
    let element = nsh_base::radio()
        .checked(move || checked.get())
        .disabled(disabled)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .transition_colors()
        .duration(crate::motion::instant)
        .ease_out()
        .on_change(move |_: &bool| on_select())
        .child(indicator)
        .child(type_body().apply(text(label).text_color(tokens::foreground)));
    focus_ring!(disabled_look!(element))
}
