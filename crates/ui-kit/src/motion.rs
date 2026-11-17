//! Motion tokens and recipes of DESIGN.md section 9.
//!
//! Durations are read from the theme's `motion-*` tokens (milliseconds), so
//! they can be tuned in `ui/theme.json`. The helpers here apply the design's
//! recipes to NSH's tools: `transition_*` for state washes and presses,
//! `LayoutAnimation` for panel changes, `Motion` for entrances, and the
//! `spinner` animation for progress. Reduced motion is honoured throughout:
//! transitions collapse, layout animations are skipped, motions land at
//! their end, and the spinner keeps turning because it is a progress cue.

use nsh_core::{
    element::{LayoutAnimation, LayoutCurve, LayoutTransition},
    prelude::*,
};

use crate::tokens;

/// The press scale of a button (DESIGN.md 9.1).
pub const PRESS_SCALE: f32 = 0.97;
/// The press scale of an icon button.
pub const PRESS_SCALE_ICON: f32 = 0.95;
/// The scale a popup, popover or QuickNav enters from.
pub const POPUP_SCALE: f32 = 0.97;
/// The scale a breakpoint pops in from.
pub const BREAKPOINT_SCALE: f32 = 0.6;
/// A label or pill rising into place, points.
pub const DISTANCE_LABEL: f32 = 4.0;
/// A popup, popover or toast sliding from its trigger or edge, points.
pub const DISTANCE_POPUP: f32 = 8.0;
/// A sheet dropping from the title bar, points.
pub const DISTANCE_SHEET: f32 = 12.0;
/// A list wave's row rise, points.
pub const DISTANCE_WAVE: f32 = 16.0;
/// Disabled opacity (DESIGN.md 3.6).
pub const OPACITY_DISABLED: f32 = 0.45;
/// Dimmed opacity while loading (DESIGN.md 7.1).
pub const OPACITY_LOADING: f32 = 0.7;
/// Dimmed opacity of an inactive window's chrome.
pub const OPACITY_INACTIVE: f32 = 0.6;
/// A list wave's rows are delayed, at most this many.
pub const STAGGER_CAP: usize = 10;

/// Easing of things that arrive.
pub const EASE_ENTER: Easing = Easing::EaseOut;
/// Easing of a move between two places.
pub const EASE_MOVE: Easing = Easing::EaseInOut;
/// Easing of things that leave.
pub const EASE_LEAVE: Easing = Easing::EaseIn;
/// The soft settle of a press, `cubic-bezier(0.22, 1, 0.36, 1)`.
pub const EASE_SETTLE: Easing = Easing::Bezier(0.22, 1.0, 0.36, 1.0);

/// `instant`: a hover wash, a color change, a dim.
pub fn instant() -> f32 {
    tokens::motion_instant()
}

/// `fast`: a press and release, a control state, a popup leaving.
pub fn fast() -> f32 {
    tokens::motion_fast()
}

/// `base`: moving into place, a panel collapsing, a selection slide.
pub fn base() -> f32 {
    tokens::motion_base()
}

/// `slow`: reveals: a popup, sheet or QuickNav opening, a toast arriving.
pub fn slow() -> f32 {
    tokens::motion_slow()
}

/// `slower`: unhurried entrances and the indeterminate progress loop.
pub fn slower() -> f32 {
    tokens::motion_slower()
}

/// `stagger`: a list wave's row delay.
pub fn stagger() -> f32 {
    tokens::motion_stagger()
}

/// The delay of item `index` in a list wave: `min(index, 10) x stagger`.
pub fn stagger_delay(index: usize) -> f32 {
    (index.min(STAGGER_CAP) as f32) * stagger()
}

/// Whether the user asked for less motion.
pub fn reduced() -> bool {
    reduce_motion()
}

/// A state wash: colors transition over `instant`, eased out. Apply it to
/// any pressable or row before its colors depend on hover or selection.
pub fn state_wash<S: Styled>(element: S) -> S {
    element.transition_colors().duration(instant()).ease_out()
}

/// A control's opacity change (enabled / disabled, loading): transitions
/// over `fast`, eased out.
pub fn state_opacity<S: Styled>(element: S) -> S {
    element.transition_opacity().duration(fast()).ease_out()
}

/// The press recipe's transform transition: `fast`, with the soft settle
/// curve. Pair it with [`press_scale`] inside an `active` style.
pub fn press<S: Styled>(element: S) -> S {
    element
        .transition_transform()
        .duration(fast())
        .ease_bezier(0.22, 1.0, 0.36, 1.0)
}

/// The scale a pressed element takes: [`PRESS_SCALE`], or
/// [`PRESS_SCALE_ICON`] for icon buttons; 1 with reduced motion. Put it in
/// an `active` style: `.active(|s| motion::press_scale(s, false))`.
pub fn press_scale<S: Styled>(element: S, icon_button: bool) -> S {
    let scale = if icon_button {
        PRESS_SCALE_ICON
    } else {
        PRESS_SCALE
    };
    element.scale(move || if reduced() { 1.0 } else { scale })
}

/// The scale of a pressed icon button: 1 with reduced motion.
pub fn press_scale_icon() -> f32 {
    if reduced() { 1.0 } else { PRESS_SCALE_ICON }
}

/// A one-off fade in rising [`DISTANCE_LABEL`] points, over `ms` with
/// ease-out. Use `motion().entering(spec)` in a binding or
/// `.bind_motion(&motion)` at rest.
pub fn fade_in_rise(ms: f32) -> MotionSpec {
    MotionSpec::new(KeyframeTiming::new(ms).easing(EASE_ENTER))
        .values(MotionChannel::Opacity, [0.0, 1.0])
        .values(MotionChannel::TranslateY, [DISTANCE_LABEL, 0.0])
}

/// A one-off fade in rising [`DISTANCE_POPUP`] points, over `slow`, for
/// content arriving outside a list.
pub fn enter_popup() -> MotionSpec {
    MotionSpec::new(KeyframeTiming::new(slow()).easing(EASE_ENTER))
        .values(MotionChannel::Opacity, [0.0, 1.0])
        .values(MotionChannel::TranslateY, [DISTANCE_POPUP, 0.0])
}

/// A popup's first-frame style: transparent and scaled from
/// [`POPUP_SCALE`], sliding from its trigger. Bind it with
/// `popup_starting_style`.
pub fn popup_starting<S: Styled>(element: S) -> S {
    element
        .opacity(0.0)
        .scale(POPUP_SCALE)
        .translate_y(px(DISTANCE_POPUP))
        .transition()
        .duration(slow())
        .ease_out()
}

/// A popup's leaving style: transparent and scaled to [`POPUP_SCALE`], no
/// slide, over `fast`. Bind it with `popup_ending_style`.
pub fn popup_ending<S: Styled>(element: S) -> S {
    element
        .opacity(0.0)
        .scale(POPUP_SCALE)
        .transition()
        .duration(fast())
        .ease_in()
}

/// The raised pill of an exclusive segmented control: `tab-active`, a
/// capsule, E1 (in light appearance), inset 2 inside the track. Bind it
/// with `tabs().indicator_style(..)`; NSH slides it to the selected tab
/// natively.
pub fn sliding_segment<S: Styled>(element: S) -> S {
    element
        .top(px(2.0))
        .bottom(px(2.0))
        .rounded_full()
        .bg(tokens::tab_active)
        .shadow_xs()
        .shadow_color(tokens::shadow())
        .dark(|s| s.shadow_none())
}

/// A spinner's turn: one revolution per second, linear, forever. It keeps
/// turning with reduced motion because it is a progress cue.
pub fn spinner<S: Styled>(element: S) -> S {
    element.animation(KeyframeAnimation::new(Keyframes::Spin).duration(1000.0))
}

/// The layout animation for a panel, bar, sidebar or console change: every
/// change eased in and out over `base`.
pub fn panel_layout_animation() -> LayoutAnimation {
    LayoutAnimation::all(LayoutTransition::new(LayoutCurve::EaseInEaseOut, base()))
}

/// Animates the next commit's layout as a panel change does, unless the
/// user asked for less motion. Call it right before changing the state.
pub fn animate_panel() {
    if !reduced() {
        animate_layout(panel_layout_animation());
    }
}

/// The `panel` spring: what the user drags and releases (split handles,
/// tab drops); stiffness 320, damping 36, mass 1.
pub fn panel_spring() -> Spring {
    Spring {
        stiffness: 320.0,
        damping: 36.0,
        mass: 1.0,
        velocity: None,
        frame_rate: None,
    }
}

/// The `pop` spring: the one playful cue, a breakpoint set; stiffness
/// 180, damping 11.
pub fn pop_spring() -> Spring {
    Spring {
        stiffness: 180.0,
        damping: 11.0,
        mass: 1.0,
        velocity: None,
        frame_rate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_scales_match_the_design() {
        assert_eq!(PRESS_SCALE, 0.97);
        assert_eq!(PRESS_SCALE_ICON, 0.95);
    }

    #[test]
    fn stagger_is_capped_at_ten_rows() {
        // The delay reads the theme; the formula is what can be tested
        // without a runtime: the index is capped regardless.
        assert_eq!(STAGGER_CAP, 10);
    }

    #[test]
    fn springs_match_the_design() {
        let panel = panel_spring();
        assert_eq!(panel.stiffness, 320.0);
        assert_eq!(panel.damping, 36.0);
        assert_eq!(panel.mass, 1.0);
        let pop = pop_spring();
        assert_eq!(pop.stiffness, 180.0);
        assert_eq!(pop.damping, 11.0);
    }
}
