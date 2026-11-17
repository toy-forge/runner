//! `CapsuleGroup` and `Segmented`: DESIGN.md 7.3. One capsule silhouette,
//! `radius-full`, `capsule` fill, 1pt `capsule-border`, members laid out
//! edge to edge with 1pt dividers. A group of pressables is a
//! `toolbar()` (one Tab stop, arrows inside); an exclusive segmented
//! control is a `tabs()` list whose raised `tab-active` pill slides to the
//! selected segment natively.

use nsh_core::prelude::*;

use super::ControlSize;
use crate::icon::{IconName, Tone, glyph};
use crate::motion;
use crate::tokens;

/// A [`capsule_group`]'s size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapsuleGroupOptions {
    /// The capsule size (default [`ControlSize::Md`], the 28pt toolbar capsule).
    pub size: ControlSize,
}

impl CapsuleGroupOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }
}

/// One capsule: `label` names the group for screen readers; `members` are
/// the buttons or popup triggers, with 1pt dividers added between them.
pub fn capsule_group(
    label: impl IntoReactive<SharedString> + 'static,
    options: CapsuleGroupOptions,
    members: impl IntoIterator<Item = impl IntoElement>,
) -> impl IntoElement {
    let label = label.into_reactive();
    let group_label = label.clone();
    let size = options.size;
    let mut children: Vec<AnyElement> = Vec::new();
    for (index, member) in members.into_iter().enumerate() {
        if index > 0 {
            children.push(
                view()
                    .w(px(1.0))
                    .h(px(16.0))
                    .bg(tokens::capsule_border)
                    .into_element(),
            );
        }
        children.push(member.into_element());
    }
    nsh_base::toolbar()
        .accessibility_label(move || Some(group_label.get()))
        .flex_row()
        .items_center()
        .h(px(size.height()))
        .p(px(2.0))
        .rounded_full()
        .overflow_hidden()
        .bg(tokens::capsule)
        .border_1()
        .border_color(tokens::capsule_border)
        .shadow_xs()
        .shadow_color(tokens::shadow())
        .dark(|s| s.shadow_none())
        .child(nsh_base::toolbar_group().child(view().flex_row().items_center().children(children)))
}

/// One option of a [`segmented`] control.
#[derive(Clone)]
pub struct Segment {
    /// The visible label (also what screen readers say).
    pub label: Reactive<SharedString>,
    /// An icon before the label.
    pub icon: Option<IconName>,
    /// This member cannot be chosen or focused.
    pub disabled: bool,
}

impl Segment {
    /// A segment labelled `label`.
    pub fn new(label: impl IntoReactive<SharedString> + 'static) -> Self {
        Self {
            label: label.into_reactive(),
            icon: None,
            disabled: false,
        }
    }

    /// With an `icon`.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A [`segmented`] control's size and layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SegmentedOptions {
    /// The capsule size (default [`ControlSize::Md`]).
    pub size: ControlSize,
    /// Segments share the width equally instead of hugging their labels.
    pub equal: bool,
    /// Every segment is disabled.
    pub disabled: bool,
}

impl SegmentedOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Equal-width segments or not.
    pub fn equal(mut self, equal: bool) -> Self {
        self.equal = equal;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// An exclusive choice as a capsule of segments: one raised `tab-active`
/// pill slides to `selected()`; Left/Right (and Up/Down), Home and End move
/// and select; Tab leaves. `on_select` receives the chosen index.
pub fn segmented(
    label: impl IntoReactive<SharedString> + 'static,
    segments: impl IntoIterator<Item = Segment>,
    selected: impl Fn() -> usize + Clone + 'static,
    on_select: impl Fn(usize) + Clone + 'static,
    options: SegmentedOptions,
) -> impl IntoElement {
    let label = label.into_reactive();
    let group_label = label.clone();
    let size = options.size;
    let equal = options.equal;
    let group_disabled = options.disabled;
    let mut items = Vec::new();
    for (index, segment) in segments.into_iter().enumerate() {
        let is_selected = {
            let selected = selected.clone();
            move || selected() == index
        };
        let on_press = {
            let on_select = on_select.clone();
            move || on_select(index)
        };
        let shown = segment.label.clone();
        let spoken = segment.label.clone();
        let mut tab = nsh_base::tab()
            .selected(is_selected)
            .disabled(segment.disabled || group_disabled)
            .accessibility_label(move || Some(spoken.get()))
            .h(px(size.capsule_member_height()))
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(size.gap()))
            .rounded_full()
            .transition_colors()
            .duration(motion::instant)
            .ease_out()
            .selected_style(|s| s.font_weight(FontWeight::MEDIUM))
            .hover(|s| s.bg(tokens::capsule_hover))
            .on_press(on_press);
        tab = tab.px(px(size.padding_x()));
        if equal {
            tab = tab.flex_1();
        }
        if let Some(icon) = segment.icon {
            tab = tab.child(glyph(icon, size.icon_size(), Tone::Default));
        }
        items.push(
            tab.child(
                text(move || shown.get())
                    .text_size(px(size.text_size()))
                    .line_height(px(tokens::leading_body()))
                    .text_color(tokens::foreground),
            )
            .into_element(),
        );
    }
    // The label sits on a wrapping view: `Tabs` has no label prop in the
    // pinned nsh.
    let mut group = view()
        .optional_label(move || Some(group_label.get()))
        .child(
            nsh_base::tabs()
                .flex_row()
                .items_center()
                .h(px(size.height()))
                .p(px(2.0))
                .rounded_full()
                .bg(tokens::capsule)
                .border_1()
                .border_color(tokens::capsule_border)
                .shadow_xs()
                .shadow_color(tokens::shadow())
                .dark(|s| s.shadow_none())
                .indicator_style(motion::sliding_segment)
                .children(items),
        );
    if group_disabled {
        group = group.opacity(motion::OPACITY_DISABLED);
    }
    group
}
