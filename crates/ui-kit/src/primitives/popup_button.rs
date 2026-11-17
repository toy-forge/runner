//! `PopupButton`: DESIGN.md 7.4. The default variant opens the system's own
//! menu ([`popup_button`]) anchored under its trigger; long lists use
//! [`select_popup`], a `select()` with search and a covering list. Three
//! looks: `field`, `capsule` and `bar`.

use std::rc::Rc;

use nsh_base::SelectItem;
use nsh_core::element::{desktop::{MenuItem, popup_menu, popup_menu_at}, EllipsizeMode};
use nsh_core::prelude::*;

use super::ControlSize;
use crate::icon::{IconName, Tone, glyph_at};
use crate::motion;
use crate::tokens;

/// The look of a [`popup_button`] or [`select_popup`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PopupButtonVariant {
    /// `control` fill, 1pt `input` outline, `radius` 8: forms and sheets.
    #[default]
    Field,
    /// A capsule (the toolbar language popup).
    Capsule,
    /// Borderless, caption text: status bar items, console filters.
    Bar,
}

/// A [`popup_button`]'s look, size and state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopupButtonOptions {
    /// The look (default [`PopupButtonVariant::Field`]).
    pub variant: PopupButtonVariant,
    /// The size (default [`ControlSize::Md`]).
    pub size: ControlSize,
    /// No presses; 45% opacity.
    pub disabled: bool,
}

impl PopupButtonOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `variant`.
    pub fn variant(mut self, variant: PopupButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The chevron indicator's size in points: 12, or 10 in a bar.
    fn chevron_size(self) -> f32 {
        match self.variant {
            PopupButtonVariant::Bar => 10.0,
            PopupButtonVariant::Field | PopupButtonVariant::Capsule => 12.0,
        }
    }

    /// The trigger's height in points.
    fn height(self) -> f32 {
        match self.variant {
            PopupButtonVariant::Capsule => self.size.capsule_member_height(),
            _ => self.size.height(),
        }
    }
}

/// The open-state fill of `variant`.
fn open_fill(variant: PopupButtonVariant) -> AdaptiveColor {
    match variant {
        PopupButtonVariant::Field => tokens::control_pressed(),
        PopupButtonVariant::Capsule | PopupButtonVariant::Bar => tokens::capsule_pressed(),
    }
}

/// The trigger's resting look, shared by both popup kinds.
fn trigger_root(
    value: Reactive<SharedString>,
    options: PopupButtonOptions,
    open: impl Fn() -> bool + Clone + 'static,
) -> View {
    let size = options.size;
    let variant = options.variant;
    let body_size = match variant {
        PopupButtonVariant::Bar => tokens::type_caption(),
        _ => size.text_size(),
    };
    let mut root = view()
        .h(px(options.height()))
        .min_w(px(80.0))
        .px(px(size.padding_x()))
        .flex_row()
        .items_center()
        .gap(px(size.gap()))
        .cursor_pointer()
        .transition_colors()
        .duration(motion::instant)
        .ease_out()
        .disabled(options.disabled)
        .disabled_style(|s| s.opacity(motion::OPACITY_DISABLED));
    root = match variant {
        PopupButtonVariant::Field => root
            .rounded(px(tokens::radius()))
            .bg(move || {
                if open() {
                    open_fill(variant)
                } else {
                    tokens::control()
                }
            })
            .border_1()
            .border_color(tokens::input)
            .hover(|s| s.bg(tokens::control_hover)),
        PopupButtonVariant::Capsule => root
            .rounded_full()
            .bg(move || {
                if open() {
                    open_fill(variant)
                } else {
                    tokens::capsule()
                }
            })
            .border_1()
            .border_color(tokens::capsule_border)
            .hover(|s| s.bg(tokens::capsule_hover)),
        PopupButtonVariant::Bar => root
            .rounded(px(tokens::radius_sm()))
            .bg(move || {
                if open() {
                    open_fill(variant)
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(tokens::accent)),
    };
    let shown = value.clone();
    root.child(
        view().flex_1().overflow_hidden().child(
            text(move || shown.get())
                .text_size(px(body_size))
                .line_height(px(if variant == PopupButtonVariant::Bar {
                    tokens::leading_caption()
                } else {
                    tokens::leading_body()
                }))
                .text_overflow(EllipsizeMode::Tail)
                .text_color(tokens::foreground),
        ),
    )
    .child(glyph_at(
        IconName::ChevronsUpDown,
        options.chevron_size(),
        Tone::Muted,
    ))
}

/// A popup trigger that opens `items` as the system's own menu, anchored
/// under the trigger's bottom-left corner, and reports the open state to
/// `on_open_change` (the trigger stays styled open until the menu closes).
/// `label` names the control for screen readers; `value` is what it shows.
pub fn popup_button(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    items: Vec<MenuItem>,
    options: PopupButtonOptions,
    on_open_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let size = options.size;
    let disabled = options.disabled;
    let open = signal(false);
    let open_style = open;
    let value = value.into_reactive();
    let label = label.into_reactive();
    let anchor = element_ref();
    let items = Rc::new(items);
    let on_open_change = Rc::new(on_open_change);
    let press_items = items.clone();
    let press_open = open;
    let press_change = on_open_change.clone();
    let press_anchor = anchor;
    let a11y_label = label.clone();

    let field = view()
        .child(trigger_root(value, options, move || open_style.get()))
        .element_ref(&anchor)
        .role(AccessibilityRole::Button)
        .label(move || a11y_label.get())
        .on_press(move |_: &PressEvent| {
            if disabled || press_open.get_untracked() {
                return;
            }
            let items = (*press_items).clone();
            let open = press_open;
            let change = press_change.clone();
            open.set(true);
            change(true);
            let anchor = press_anchor;
            spawn(async move {
                let at = anchor
                    .measure()
                    .await
                    .map(|m| point(m.page.x, m.page.y + px(size.height())));
                match at {
                    Some(at) => {
                        popup_menu_at(items, at).await;
                    }
                    None => {
                        popup_menu(items).await;
                    }
                }
                open.set(false);
                change(false);
            });
        });
    focus_ring!(
        disabled_look!(field)
            .transition_transform()
            .duration(motion::fast)
            .ease_bezier(0.22, 1.0, 0.36, 1.0)
            .active(|s| motion::press_scale(s, false))
    )
}

/// Options for [`select_popup`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectPopupOptions {
    /// The trigger's look, size and disabled state.
    pub popup: PopupButtonOptions,
    /// A search field in the popup.
    pub searchable: bool,
}

impl SelectPopupOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With the trigger options.
    pub fn popup(mut self, popup: PopupButtonOptions) -> Self {
        self.popup = popup;
        self
    }

    /// Searchable or not.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }
}

/// A select for long lists (Syntax Mode, encodings, fonts, themes): a
/// [`select()`](nsh_base::select) with the same trigger looks, optional
/// search and a covering list. `value` is the chosen item's value.
pub fn select_popup(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<Option<SharedString>> + 'static,
    placeholder: impl IntoReactive<SharedString> + 'static,
    items: Vec<SelectItem>,
    options: SelectPopupOptions,
    on_change: impl Fn(Option<SharedString>) + 'static,
) -> impl IntoElement {
    let value = value.into_reactive();
    let label = label.into_reactive();
    let placeholder = placeholder.into_reactive();
    let shown = value.clone();
    let placeholder_shown = placeholder.clone();
    let trigger_value: Reactive<SharedString> = Reactive::Bound(Rc::new(move || {
        shown.get().unwrap_or_else(|| placeholder_shown.get())
    }));
    let trigger = trigger_root(trigger_value, options.popup, || false);
    nsh_base::select()
        .items(items)
        .value(move || value.get())
        .on_change(move |next: &Option<SharedString>| on_change(next.clone()))
        .accessibility_label(move || Some(label.get()))
        .searchable(options.searchable)
        .disabled(options.popup.disabled)
        .offset(px(4.0))
        .popup_style(|s| {
            s.min_w(px(180.0))
                .p(px(4.0))
                .rounded(px(tokens::radius_lg()))
                .border_1()
                .border_color(tokens::border)
                .bg(tokens::popover)
                .shadow_md()
                .shadow_color(tokens::shadow())
                .text_color(tokens::foreground)
        })
        .item_style(|s| {
            s.h(px(tokens::row_sm()))
                .px(px(8.0))
                .rounded(px(tokens::radius()))
                .text_size(px(tokens::type_body()))
                .selected(|s| {
                    s.bg(tokens::menu_highlight)
                        .text_color(tokens::menu_highlight_foreground)
                })
        })
        .trigger(trigger)
}
