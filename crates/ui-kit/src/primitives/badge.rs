//! `Badge`: DESIGN.md 7.16. A 16pt-tall caption mark: neutral, a count
//! capsule, the primary fill, or a status wash with the status color.

use nsh_core::prelude::*;

use crate::icon::{IconName, Tone, glyph};
use crate::tokens;
use crate::typography::type_caption;

/// The role a [`badge`] communicates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BadgeVariant {
    /// `muted` fill, `muted-foreground` text (the default).
    #[default]
    Neutral,
    /// A capsule of `muted`: a match count.
    Count,
    /// `primary` fill, `primary-foreground` text.
    Primary,
    /// `success-muted` fill, `success` text.
    Success,
    /// `warning-muted` fill, `warning` text.
    Warning,
    /// `destructive-muted` fill, `destructive` text.
    Danger,
}

/// A [`badge`]'s variant and optional icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BadgeOptions {
    /// The variant (default [`BadgeVariant::Neutral`]).
    pub variant: BadgeVariant,
    /// A leading icon, 12pt, when color carries meaning.
    pub icon: Option<IconName>,
}

impl BadgeOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `variant`.
    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    /// With a leading `icon`.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// The badge's text color.
fn text_color(variant: BadgeVariant) -> AdaptiveColor {
    match variant {
        BadgeVariant::Neutral | BadgeVariant::Count => tokens::muted_foreground(),
        BadgeVariant::Primary => tokens::primary_foreground(),
        BadgeVariant::Success => tokens::success(),
        BadgeVariant::Warning => tokens::warning(),
        BadgeVariant::Danger => tokens::destructive(),
    }
}

/// The badge's fill.
fn fill(variant: BadgeVariant) -> AdaptiveColor {
    match variant {
        BadgeVariant::Neutral | BadgeVariant::Count => tokens::muted(),
        BadgeVariant::Primary => tokens::primary(),
        BadgeVariant::Success => tokens::success_muted(),
        BadgeVariant::Warning => tokens::warning_muted(),
        BadgeVariant::Danger => tokens::destructive_muted(),
    }
}

/// A small caption badge. Use sparingly: a `Default` marker, a match count,
/// an item count, an issue count.
pub fn badge(
    label: impl IntoReactive<SharedString> + 'static,
    options: BadgeOptions,
) -> impl IntoElement {
    let variant = options.variant;
    let icon_tone = match variant {
        BadgeVariant::Neutral | BadgeVariant::Count => Tone::Muted,
        BadgeVariant::Primary => Tone::OnPrimary,
        BadgeVariant::Success => Tone::Success,
        BadgeVariant::Warning => Tone::Warning,
        BadgeVariant::Danger => Tone::Danger,
    };
    let badge = view()
        .h(px(16.0))
        .min_w(px(16.0))
        .px(px(5.0))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .bg(move || fill(variant))
        .when_some(options.icon, |v, icon| {
            v.child(glyph(icon, crate::icon::IconSize::Xs, icon_tone))
        })
        .child(
            type_caption()
                .medium()
                .apply(text(label).text_color(move || text_color(variant))),
        );
    match variant {
        BadgeVariant::Count => badge.rounded_full(),
        _ => badge.rounded(px(tokens::radius_xs())),
    }
}
