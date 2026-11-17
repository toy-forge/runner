//! `ProgressSpinner` and `ProgressBar`: DESIGN.md 7.24.
//!
//! The spinner is the `spinner` icon turning once per second, which keeps
//! turning under reduced motion because it is a progress cue. The bar is
//! 2pt (search, docs) or 4pt (sheet operations) high, `muted` track,
//! `primary` indicator, `radius-full`; indeterminate shows a 30% segment
//! (40%, static, with reduced motion).

use nsh_core::prelude::*;

use crate::icon::{IconName, IconSize, Tone, glyph};
use crate::motion;
use crate::tokens;
use crate::typography::type_caption;

/// A progress bar's height and state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressOptions {
    /// The bar's thickness in points: 2 or 4 (default 2).
    pub height: f32,
    /// The work's extent is unknown.
    pub indeterminate: bool,
    /// What screen readers say, and the caption under a static
    /// indeterminate bar with reduced motion.
    pub label: Option<&'static str>,
}

impl Default for ProgressOptions {
    fn default() -> Self {
        Self {
            height: 2.0,
            indeterminate: false,
            label: None,
        }
    }
}

impl ProgressOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// A `height` points thick bar.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Indeterminate or not.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    /// With an accessible `label`.
    pub fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }
}

/// A spinner at `size` in `tone`, labeled `label` for screen readers.
pub fn spinner(size: IconSize, tone: Tone, label: &'static str) -> impl IntoElement {
    view()
        .role(AccessibilityRole::Progressbar)
        .label(SharedString::from(label))
        .child(motion::spinner(glyph(IconName::Spinner, size, tone)))
}

/// A progress bar: `value` is 0 to 100; indeterminate ignores it.
pub fn progress_bar(value: impl IntoReactive<f32>, options: ProgressOptions) -> impl IntoElement {
    let bar = nsh_base::progress()
        .value(value)
        .indeterminate(options.indeterminate)
        .accessibility_label(options.label.map(SharedString::from))
        .w_full()
        .h(px(options.height))
        .rounded_full()
        .overflow_hidden()
        .bg(tokens::muted)
        .track_style(|s| s.w_full().h_full().rounded_full().bg(tokens::muted))
        .indicator_style(move |s| {
            let s = s.h_full().rounded_full().bg(tokens::primary);
            if options.indeterminate {
                // A travelling segment; reduced motion shows a static 40%.
                let s = s.w(relative(if motion::reduced() { 0.4 } else { 0.3 }));
                if motion::reduced() {
                    s
                } else {
                    s.animate_pulse()
                }
            } else {
                // The progress element sizes the indicator from `value`.
                s
            }
        });
    match options.label {
        Some(label) if options.indeterminate && motion::reduced() => view()
            .flex_col()
            .gap(px(4.0))
            .child(bar)
            .child(
                type_caption()
                    .apply(text(SharedString::from(label)))
                    .text_color(tokens::muted_foreground),
            )
            .into_element(),
        _ => bar.into_element(),
    }
}
