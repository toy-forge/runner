//! `Separator`: DESIGN.md 7.26. A 1pt `border` line, horizontal in lists
//! and menus (with 4pt vertical margin from the caller) or vertical in
//! capsules (16 high). Decorative: no accessibility role.

use nsh_core::prelude::*;

use crate::tokens;

/// Which way a [`separator`] runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SeparatorOrientation {
    /// A 1pt line across its container.
    #[default]
    Horizontal,
    /// A 1pt line 16 high: between capsule members.
    Vertical,
}

/// A hairline: full width and 1pt high, or 1pt wide and 16pt high.
pub fn separator(orientation: SeparatorOrientation) -> impl IntoElement {
    match orientation {
        SeparatorOrientation::Horizontal => view()
            .w_full()
            .h(px(tokens::hairline()))
            .bg(tokens::border),
        SeparatorOrientation::Vertical => view()
            .w(px(tokens::hairline()))
            .h(px(16.0))
            .bg(tokens::border),
    }
}
