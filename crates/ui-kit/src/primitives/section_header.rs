//! `SectionHeader`: DESIGN.md 7.17. Three variants: `form` (body
//! semibold, 12 above and 8 below, a 1pt top hairline), `list` (a 24pt
//! caption band with trailing actions) and `pane` (subhead semibold).

use nsh_core::prelude::*;

use crate::tokens;
use crate::typography::{type_body, type_caption, type_subhead};

/// Where a [`section_header`] is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SectionHeaderVariant {
    /// A preference group's heading, separated by a hairline.
    #[default]
    Form,
    /// A 24pt band above a list, with trailing actions.
    List,
    /// A pane or sheet title.
    Pane,
}

/// A [`section_header`]'s hairline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionHeaderOptions {
    /// Draw the form variant's 1pt top hairline (default on; off for the
    /// first group on a surface).
    pub divider: bool,
}

impl SectionHeaderOptions {
    /// The default options (hairline on).
    pub fn new() -> Self {
        Self { divider: true }
    }

    /// The hairline or not.
    pub fn divider(mut self, divider: bool) -> Self {
        self.divider = divider;
        self
    }
}

/// A section heading, with no trailing actions.
pub fn section_header(
    title: impl IntoReactive<SharedString> + 'static,
    variant: SectionHeaderVariant,
    options: SectionHeaderOptions,
) -> impl IntoElement {
    section_header_with_actions(title, variant, std::iter::empty::<AnyElement>(), options)
}

/// A section heading with `actions` at the trailing end (xs ghost icon
/// buttons in a list band, say).
pub fn section_header_with_actions(
    title: impl IntoReactive<SharedString> + 'static,
    variant: SectionHeaderVariant,
    actions: impl IntoIterator<Item = impl IntoElement>,
    options: SectionHeaderOptions,
) -> impl IntoElement {
    let title_element = match variant {
        SectionHeaderVariant::Form => type_body().semibold().apply(
            text(title)
                .text_color(tokens::foreground),
        ),
        SectionHeaderVariant::List => type_caption().medium().apply(
            text(title)
                .text_color(tokens::muted_foreground),
        ),
        SectionHeaderVariant::Pane => type_subhead().apply(
            text(title)
                .text_color(tokens::foreground),
        ),
    };
    let actions: Vec<AnyElement> = actions
        .into_iter()
        .map(IntoElement::into_element)
        .collect();
    let root = match variant {
        SectionHeaderVariant::Form => {
            let root = view()
                .w_full()
                .pt(px(12.0))
                .pb(px(8.0))
                .child(title_element);
            if options.divider {
                root
                    .border_t_1()
                    .border_t_color(tokens::border)
            } else {
                root
            }
        }
        SectionHeaderVariant::List => view()
            .w_full()
            .h(px(24.0))
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .child(view().flex_1().child(title_element))
            .children(actions),
        SectionHeaderVariant::Pane => view().w_full().child(title_element),
    };
    root.role(AccessibilityRole::Header)
}
