//! Spike-local design tokens.
//!
//! The app ships a shadcn `cssVars` theme (see [`TOKENS_JSON`]) with a few extra
//! names; presentation code reads them through the functions below, never as raw
//! colours.

use nsh_base::theme;
use nsh_core::prelude::*;

/// Extra tokens layered over NSH's default theme with
/// `Root::tokens(Theme::from_json(TOKENS_JSON)?.into_tokens())`.
pub const TOKENS_JSON: &str = r##"{
  "cssVars": {
    "theme": { "radius": "0.5rem" },
    "light": {
      "selection": "#0a64d8",
      "selection-foreground": "#ffffff",
      "selection-inactive": "#d4d4d8",
      "row-hover": "rgba(0,0,0,0.05)",
      "current-doc": "rgba(10,100,216,0.14)",
      "drop-target": "rgba(10,100,216,0.18)",
      "match-bg": "rgba(255,196,0,0.45)",
      "icon-folder": "#4aa3f5",
      "icon-orange": "#e8662f",
      "icon-blue": "#2f7bd6",
      "icon-yellow": "#c99a06",
      "icon-green": "#2f9e57",
      "icon-red": "#d6453d",
      "icon-purple": "#8a5cd6",
      "icon-teal": "#1f9aa6",
      "icon-gray": "#7b8088"
    },
    "dark": {
      "selection": "#1a73e8",
      "selection-foreground": "#ffffff",
      "selection-inactive": "#4a4a50",
      "row-hover": "rgba(255,255,255,0.07)",
      "current-doc": "rgba(90,160,255,0.20)",
      "drop-target": "rgba(90,160,255,0.28)",
      "match-bg": "rgba(255,196,0,0.38)",
      "icon-folder": "#5ab0ff",
      "icon-orange": "#f0834f",
      "icon-blue": "#5a9cf0",
      "icon-yellow": "#e6b422",
      "icon-green": "#4cc27a",
      "icon-red": "#ef6b63",
      "icon-purple": "#a98bf0",
      "icon-teal": "#3fc0cc",
      "icon-gray": "#9aa0a8"
    }
  }
}"##;

fn token(name: &str, fallback: impl FnOnce() -> AdaptiveColor) -> AdaptiveColor {
    theme().color(name).unwrap_or_else(fallback)
}

/// Selected row background while the list has the keyboard focus.
pub fn selection() -> AdaptiveColor {
    token("selection", || theme().primary)
}
/// Text on [`selection`].
pub fn selection_fg() -> AdaptiveColor {
    token("selection-foreground", || theme().primary_foreground)
}
/// Selected row background while the list is not focused.
pub fn selection_inactive() -> AdaptiveColor {
    token("selection-inactive", || theme().sidebar_accent)
}
/// Row under the pointer.
pub fn row_hover() -> AdaptiveColor {
    token("row-hover", || theme().sidebar_accent)
}
/// The row of the document open in the editor.
pub fn current_doc() -> AdaptiveColor {
    token("current-doc", || theme().sidebar_accent)
}
/// Drop target row.
pub fn drop_target() -> AdaptiveColor {
    token("drop-target", || theme().sidebar_accent)
}
/// Highlight behind a search match.
pub fn match_bg() -> AdaptiveColor {
    token("match-bg", || theme().accent)
}
/// A named icon tint (`icon-orange`...).
pub fn icon_tint(name: &str) -> AdaptiveColor {
    token(name, || theme().muted_foreground)
}
/// Primary text in the sidebar.
pub fn fg() -> AdaptiveColor {
    theme().sidebar_foreground
}
/// Secondary text.
pub fn muted() -> AdaptiveColor {
    theme().muted_foreground
}
/// Hairlines.
pub fn separator() -> AdaptiveColor {
    theme().sidebar_border
}
