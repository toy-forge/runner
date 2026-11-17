//! The styled primitives of DESIGN.md section 7: every control the app's
//! features compose instead of styling `nsh_base` themselves.
//!
//! Each public free function returns an element and takes `size`, `variant`
//! and state through an options struct, text as `impl IntoReactive<SharedString>`
//! and events as closures. Components keep no app state: the caller owns
//! values and receives change events.
//!
//! Layout (width, flex, margins) comes from the caller; a primitive sets
//! only its own measurements and states.

use crate::tokens;

/// The four control sizes of DESIGN.md table 2.1. One size sets the
/// control's height, horizontal padding, text size, icon size, gap and
/// corner shape together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ControlSize {
    /// 20 high: console toolbar items, inline row actions.
    Xs,
    /// 24 high: find bar, status popups, tab items, dense forms.
    Sm,
    /// 28 high: the default (toolbar capsules, buttons, fields).
    #[default]
    Md,
    /// 32 high: sheet and prompt buttons and fields.
    Lg,
}

impl ControlSize {
    /// The control height in points (`ctl-*`).
    pub fn height(self) -> f32 {
        match self {
            ControlSize::Xs => tokens::ctl_xs(),
            ControlSize::Sm => tokens::ctl_sm(),
            ControlSize::Md => tokens::ctl_md(),
            ControlSize::Lg => tokens::ctl_lg(),
        }
    }

    /// The horizontal padding in points.
    pub fn padding_x(self) -> f32 {
        match self {
            ControlSize::Xs => 6.0,
            ControlSize::Sm => 8.0,
            ControlSize::Md => 12.0,
            ControlSize::Lg => 16.0,
        }
    }

    /// The interface text size in points: 12 at `xs`, 13 otherwise.
    pub fn text_size(self) -> f32 {
        match self {
            ControlSize::Xs => tokens::type_caption(),
            ControlSize::Sm | ControlSize::Md | ControlSize::Lg => tokens::type_body(),
        }
    }

    /// The gap between an icon and its label in points.
    pub fn gap(self) -> f32 {
        match self {
            ControlSize::Xs | ControlSize::Sm => 4.0,
            ControlSize::Md => 6.0,
            ControlSize::Lg => 8.0,
        }
    }

    /// The icon size.
    pub fn icon_size(self) -> crate::icon::IconSize {
        match self {
            ControlSize::Xs => crate::icon::IconSize::Xs,
            ControlSize::Sm => crate::icon::IconSize::Sm,
            ControlSize::Md | ControlSize::Lg => crate::icon::IconSize::Md,
        }
    }

    /// The rectangular corner radius: `radius-sm` at `xs` and `sm`; `md`
    /// and `lg` text buttons are capsules, so their radius is full.
    pub fn rect_radius(self) -> f32 {
        tokens::radius_sm()
    }

    /// Whether a text button or field of this size is a capsule.
    pub fn capsule(self) -> bool {
        matches!(self, ControlSize::Md | ControlSize::Lg)
    }

    /// The height of a member inside a capsule group: the control height
    /// less the group's 2pt inset on each side.
    pub fn capsule_member_height(self) -> f32 {
        (self.height() - 4.0).max(1.0)
    }

    /// The minimum width of a text button: 64 at `md`, 80 at `lg`.
    pub fn button_min_width(self) -> f32 {
        match self {
            ControlSize::Lg => 80.0,
            _ => 64.0,
        }
    }
}

/// The focus-visible rule of DESIGN.md 7: a 2pt `ring` outline, 1pt
/// outside the control. Ancestors leave 3pt and never clip it. A macro,
/// not a function, because views and component builders reach state styles
/// through different traits.
macro_rules! focus_ring {
    ($element:expr) => {
        $element.focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(move || $crate::tokens::ring())
                .outline_offset(px(1.0))
        })
    };
}

/// A disabled element's look: 45% opacity, unchanged otherwise. A macro
/// for the same reason as [`focus_ring`].
macro_rules! disabled_look {
    ($element:expr) => {
        $element.disabled_style(|s| s.opacity($crate::motion::OPACITY_DISABLED))
    };
}

pub mod badge;
pub mod button;
pub mod capsule;
pub mod checkbox;
pub mod color_well;
pub mod form_row;
pub mod icon_button;
pub mod kbd;
pub mod popup_button;
pub mod progress;
pub mod search_field;
pub mod section_header;
pub mod separator;
pub mod split_handle;
pub mod stepper;
pub mod switch;
pub mod text_field;
pub mod tooltip;

pub use badge::{BadgeOptions, BadgeVariant, badge};
pub use button::{ButtonOptions, ButtonVariant, button};
pub use capsule::{CapsuleGroupOptions, Segment, SegmentedOptions, capsule_group, segmented};
pub use checkbox::{CheckboxOptions, checkbox, radio, radio_group};
pub use color_well::{ColorWellOptions, ColorWellSize, color_well};
pub use form_row::{
    FormRowOptions, form_row, form_row_with_caption, form_row_with_help,
};
pub use icon_button::{IconButtonOptions, IconButtonVariant, icon_button};
pub use kbd::{kbd, kbd_for};
pub use popup_button::{
    PopupButtonOptions, PopupButtonVariant, SelectPopupOptions, popup_button, select_popup,
};
pub use progress::{ProgressOptions, progress_bar, spinner};
pub use search_field::{SearchFieldOptions, search_field};
pub use section_header::{
    SectionHeaderOptions, SectionHeaderVariant, section_header, section_header_with_actions,
};
pub use separator::{SeparatorOrientation, separator};
pub use split_handle::{split_handle, split_line};
pub use stepper::{NumberFieldOptions, number_field, stepper};
pub use switch::switch;
pub use text_field::{
    CodeFieldOptions, TextFieldOptions, TextFieldVariant, code_caption, code_field, text_area,
    text_field,
};
pub use tooltip::tooltip;
