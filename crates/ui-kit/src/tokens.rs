//! The theme tokens of DESIGN.md section 3, one typed accessor each.
//!
//! Colors are [`AdaptiveColor`]s read from the app's theme (`ui/theme.json`,
//! a `nsh.json` `"theme"`, or the runtime's `set_tokens`), so light and dark
//! appearance come from the same value. A token the theme does not set
//! returns a loud magenta ([`MISSING`]) instead of panicking, as section 3.3
//! asks. Read an accessor inside a binding (`move || primary()`) where the
//! theme or the appearance may change after the element was made.
//!
//! Numbers are points, except `motion-*`, which are milliseconds. A number
//! the theme does not set falls back to the value DESIGN.md specifies.

use nsh_base::theme;
use nsh_core::element::AdaptiveColor;

/// The color a token missing from the theme reads as: loud magenta, so a
/// forgotten token is impossible to miss (DESIGN.md 3.3).
pub const MISSING: AdaptiveColor = AdaptiveColor::rgb(0xFF00FF, 0xFF00FF);

/// The adaptive color named `name`, or [`MISSING`].
fn color(name: &str) -> AdaptiveColor {
    theme().color(name).unwrap_or(MISSING)
}

/// The number named `name`, or `fallback`.
fn number(name: &str, fallback: f32) -> f32 {
    theme().number(name).unwrap_or(fallback)
}

macro_rules! color_tokens {
    ($(
        $(#[doc = $doc:literal])*
        $func:ident => $name:literal
    ),* $(,)?) => {$(
        $(#[doc = $doc])*
        #[doc = concat!("Theme token `", $name, "`.")]
        pub fn $func() -> AdaptiveColor {
            color($name)
        }
    )*};
}

macro_rules! number_tokens {
    ($(
        $(#[doc = $doc:literal])*
        $func:ident => $name:literal = $fallback:literal
    ),* $(,)?) => {$(
        $(#[doc = $doc])*
        #[doc = concat!("Theme token `", $name, "` (fallback ", stringify!($fallback), ").")]
        pub fn $func() -> f32 {
            number($name, $fallback)
        }
    )*};
}

color_tokens! {
    // Base surfaces and text (shadcn roles).
    /// The canvas of preference panes, sheets, forms and empty states.
    background => "background",
    /// Primary text and icons.
    foreground => "foreground",
    /// A grouped form section sitting on [`background`].
    card => "card",
    /// Text on [`card`].
    card_foreground => "card-foreground",
    /// Menus, popovers, tooltips, completion lists and QuickNav panels.
    popover => "popover",
    /// Text on [`popover`].
    popover_foreground => "popover-foreground",
    /// The system-style blue: default button, focus accent, toggled icon.
    primary => "primary",
    /// Text and icons on [`primary`].
    primary_foreground => "primary-foreground",
    /// [`primary`], hovered.
    primary_hover => "primary-hover",
    /// [`primary`], pressed.
    primary_active => "primary-active",
    /// Underlined link text.
    link => "link",
    /// A quiet filled button.
    secondary => "secondary",
    /// Text on [`secondary`].
    secondary_foreground => "secondary-foreground",
    /// [`secondary`], hovered.
    secondary_hover => "secondary-hover",
    /// [`secondary`], pressed.
    secondary_active => "secondary-active",
    /// Quiet fills: kbd, neutral badge, track, code chip.
    muted => "muted",
    /// Secondary text, placeholders and inactive icons.
    muted_foreground => "muted-foreground",
    /// The translucent hover wash.
    accent => "accent",
    /// Text on a hovered row.
    accent_foreground => "accent-foreground",
    /// Destructive actions and errors.
    destructive => "destructive",
    /// [`destructive`], hovered.
    destructive_hover => "destructive-hover",
    /// [`destructive`], pressed.
    destructive_active => "destructive-active",
    /// Hairline separators and outlines (1pt).
    border => "border",
    /// The outline of fields and bordered controls.
    input => "input",
    /// The keyboard focus ring, drawn outside the control.
    ring => "ring",
    /// The first chart series.
    chart_1 => "chart-1",
    /// The second chart series.
    chart_2 => "chart-2",
    /// The third chart series.
    chart_3 => "chart-3",
    /// The fourth chart series.
    chart_4 => "chart-4",
    /// The fifth chart series.
    chart_5 => "chart-5",
    /// The file navigator and search view fill.
    sidebar => "sidebar",
    /// Text in the sidebar.
    sidebar_foreground => "sidebar-foreground",
    /// The accent inside the sidebar.
    sidebar_primary => "sidebar-primary",
    /// Text on [`sidebar_primary`].
    sidebar_primary_foreground => "sidebar-primary-foreground",
    /// A selected sidebar row.
    sidebar_accent => "sidebar-accent",
    /// Text on a selected sidebar row.
    sidebar_accent_foreground => "sidebar-accent-foreground",
    /// Sidebar hairlines.
    sidebar_border => "sidebar-border",
    /// The focus ring inside the sidebar.
    sidebar_ring => "sidebar-ring",

    // Window chrome.
    /// The window base behind toolbar, tab bar, bars and sheets.
    window => "window",
    /// The title band and toolbar band.
    toolbar => "toolbar",
    /// The hairline under the toolbar.
    toolbar_border => "toolbar-border",
    /// The capsule group fill (toolbar groups, segmented tracks).
    capsule => "capsule",
    /// The capsule group outline.
    capsule_border => "capsule-border",
    /// A hovered button inside a capsule.
    capsule_hover => "capsule-hover",
    /// A pressed button inside a capsule.
    capsule_pressed => "capsule-pressed",
    /// A toggled-on capsule segment.
    capsule_selected => "capsule-selected",
    /// A selected row of a focused list or table.
    selection => "selection",
    /// Text on [`selection`].
    selection_foreground => "selection-foreground",
    /// A selected row while its list or window is not key.
    selection_inactive => "selection-inactive",
    /// Text on [`selection_inactive`].
    selection_inactive_foreground => "selection-inactive-foreground",
    /// A highlighted item of a menu or popup list.
    menu_highlight => "menu-highlight",
    /// Text on [`menu_highlight`].
    menu_highlight_foreground => "menu-highlight-foreground",
    /// The fill of a bordered button and a checkbox.
    control => "control",
    /// A bordered control, hovered.
    control_hover => "control-hover",
    /// A bordered control, pressed.
    control_pressed => "control-pressed",
    /// The text field fill.
    field => "field",
    /// The tab bar track.
    tab_bar => "tab-bar",
    /// A raised selection: the selected tab and segment.
    tab_active => "tab-active",
    /// An unselected tab, hovered.
    tab_hover => "tab-hover",
    /// An unselected tab label.
    tab_foreground => "tab-foreground",
    /// A selected tab label.
    tab_active_foreground => "tab-active-foreground",
    /// The bottom bars of the sidebar and documentation panel.
    status_bar => "status-bar",
    /// The dimmer behind sheets and QuickNav.
    scrim => "scrim",
    /// The color of every drop shadow.
    shadow => "shadow",
    /// The search-term highlight in results and docs find.
    match_highlight => "match-highlight",

    // Status.
    /// Run Succeeded, enabled, passed.
    success => "success",
    /// Text on [`success`].
    success_foreground => "success-foreground",
    /// The success banner wash.
    success_muted => "success-muted",
    /// Warnings and unsaved attention.
    warning => "warning",
    /// Text on [`warning`].
    warning_foreground => "warning-foreground",
    /// The warning banner wash.
    warning_muted => "warning-muted",
    /// Notices and informational banners.
    info => "info",
    /// Text on [`info`].
    info_foreground => "info-foreground",
    /// The informational banner wash.
    info_muted => "info-muted",
    /// Text on [`destructive`].
    destructive_foreground => "destructive-foreground",
    /// The error banner wash.
    destructive_muted => "destructive-muted",

    // Diff.
    /// Added text background (replace preview).
    diff_added => "diff-added",
    /// Added text.
    diff_added_foreground => "diff-added-foreground",
    /// Removed text background.
    diff_removed => "diff-removed",
    /// Removed text.
    diff_removed_foreground => "diff-removed-foreground",
    /// Changed text background.
    diff_changed => "diff-changed",
    /// Changed text.
    diff_changed_foreground => "diff-changed-foreground",

    // Editor scope. The active syntax theme re-tints these at run time.
    /// The editor and console background.
    editor_bg => "editor-bg",
    /// The editor and console text.
    editor_fg => "editor-fg",
    /// The insertion point.
    editor_caret => "editor-caret",
    /// Selected text (focused).
    editor_selection => "editor-selection",
    /// Selected text (editor not focused).
    editor_selection_inactive => "editor-selection-inactive",
    /// The current line's fill.
    editor_line_highlight => "editor-line-highlight",
    /// Invisible-character marks.
    editor_invisibles => "editor-invisibles",
    /// An indent guide.
    editor_indent_guide => "editor-indent-guide",
    /// The indent guide of the caret's block.
    editor_indent_guide_active => "editor-indent-guide-active",
    /// The page guide column.
    editor_ruler => "editor-ruler",
    /// The balancing bracket highlight.
    editor_bracket_match => "editor-bracket-match",
    /// Token-aware Find and Select All matches.
    editor_token_match => "editor-token-match",
    /// Find matches.
    editor_find_match => "editor-find-match",
    /// The current find match.
    editor_find_current => "editor-find-current",
    /// The editor status bar and console toolbar.
    editor_bar => "editor-bar",
    /// Text and icons on [`editor_bar`].
    editor_bar_foreground => "editor-bar-foreground",
    /// The hairline above [`editor_bar`].
    editor_bar_border => "editor-bar-border",
    /// The line-number gutter fill.
    gutter_bg => "gutter-bg",
    /// Line numbers.
    gutter_fg => "gutter-fg",
    /// The line number of the current line.
    gutter_fg_active => "gutter-fg-active",
    /// The gutter's right edge.
    gutter_separator => "gutter-separator",
    /// A breakpoint tag.
    breakpoint => "breakpoint",
    /// A breakpoint preview on gutter hover.
    breakpoint_ghost => "breakpoint-ghost",
    /// A disabled breakpoint.
    breakpoint_disabled => "breakpoint-disabled",
    /// The current execution line.
    debug_line => "debug-line",
    /// The execution arrow in the gutter.
    debug_arrow => "debug-arrow",
    /// An error squiggle, icon and scroll marker.
    diag_error => "diag-error",
    /// A warning squiggle, icon and scroll marker.
    diag_warning => "diag-warning",
    /// A notice squiggle, icon and scroll marker.
    diag_info => "diag-info",
    /// A hint's dotted underline.
    diag_hint => "diag-hint",
    /// An error line tint and annotation fill.
    diag_error_bg => "diag-error-bg",
    /// A warning line tint and annotation fill.
    diag_warning_bg => "diag-warning-bg",
    /// A notice line tint and annotation fill.
    diag_info_bg => "diag-info-bg",
    /// A hint annotation fill.
    diag_hint_bg => "diag-hint-bg",
    /// The console body (runtime: equals `editor-bg`).
    console_bg => "console-bg",
    /// Program stdout text.
    console_fg => "console-fg",
    /// stderr and error text.
    console_error => "console-error",
    /// Informative, notice and debugger text.
    console_info => "console-info",
    /// The debugger prompt.
    console_prompt => "console-prompt",
    /// Links and `file:line` references in the console.
    console_link => "console-link",

    // ANSI colors.
    /// ANSI 30 (black).
    ansi_black => "ansi-black",
    /// ANSI 31 (red).
    ansi_red => "ansi-red",
    /// ANSI 32 (green).
    ansi_green => "ansi-green",
    /// ANSI 33 (yellow).
    ansi_yellow => "ansi-yellow",
    /// ANSI 34 (blue).
    ansi_blue => "ansi-blue",
    /// ANSI 35 (magenta).
    ansi_magenta => "ansi-magenta",
    /// ANSI 36 (cyan).
    ansi_cyan => "ansi-cyan",
    /// ANSI 37 (white).
    ansi_white => "ansi-white",
    /// ANSI 90 (bright black).
    ansi_bright_black => "ansi-bright-black",
    /// ANSI 91 (bright red).
    ansi_bright_red => "ansi-bright-red",
    /// ANSI 92 (bright green).
    ansi_bright_green => "ansi-bright-green",
    /// ANSI 93 (bright yellow).
    ansi_bright_yellow => "ansi-bright-yellow",
    /// ANSI 94 (bright blue).
    ansi_bright_blue => "ansi-bright-blue",
    /// ANSI 95 (bright magenta).
    ansi_bright_magenta => "ansi-bright-magenta",
    /// ANSI 96 (bright cyan).
    ansi_bright_cyan => "ansi-bright-cyan",
    /// ANSI 97 (bright white).
    ansi_bright_white => "ansi-bright-white",

    // Glyph tiles.
    /// Glyph tile color 1 (blue).
    glyph_1 => "glyph-1",
    /// Glyph tile color 2 (teal).
    glyph_2 => "glyph-2",
    /// Glyph tile color 3 (green).
    glyph_3 => "glyph-3",
    /// Glyph tile color 4 (orange).
    glyph_4 => "glyph-4",
    /// Glyph tile color 5 (red).
    glyph_5 => "glyph-5",
    /// Glyph tile color 6 (pink).
    glyph_6 => "glyph-6",
    /// Glyph tile color 7 (purple).
    glyph_7 => "glyph-7",
    /// Glyph tile color 8 (indigo).
    glyph_8 => "glyph-8",
    /// Glyph tile color 9 (yellow).
    glyph_9 => "glyph-9",
    /// Glyph tile color 10 (gray).
    glyph_10 => "glyph-10",
}

number_tokens! {
    /// The default control and menu-item corner radius.
    radius => "radius" = 8.0,
    /// The checkbox, kbd, badge and tag corner radius.
    radius_xs => "radius-xs" = 4.0,
    /// The tab, xs/sm button and code chip corner radius.
    radius_sm => "radius-sm" = 6.0,
    /// The menu, popover, tooltip and card corner radius.
    radius_lg => "radius-lg" = 12.0,
    /// The sheet, QuickNav and dialog corner radius.
    radius_xl => "radius-xl" = 16.0,
    /// The `xs` control height (inline actions).
    ctl_xs => "ctl-xs" = 20.0,
    /// The `sm` control height (dense forms and bars).
    ctl_sm => "ctl-sm" = 24.0,
    /// The `md` control height: the toolbar capsule.
    ctl_md => "ctl-md" = 28.0,
    /// The `lg` control height (sheet and prompt commits).
    ctl_lg => "ctl-lg" = 32.0,
    /// The dense list row height.
    row_sm => "row-sm" = 24.0,
    /// The table and form row height.
    row_md => "row-md" = 28.0,
    /// The two-line preference row height.
    row_lg => "row-lg" = 32.0,
    /// The QuickNav result row height.
    row_xl => "row-xl" = 44.0,
    /// The inline glyph size.
    icon_xs => "icon-xs" = 12.0,
    /// The small control and tree glyph size.
    icon_sm => "icon-sm" = 14.0,
    /// The toolbar and default button glyph size.
    icon_md => "icon-md" = 16.0,
    /// The prompt and alert glyph size.
    icon_lg => "icon-lg" = 20.0,
    /// The preferences strip glyph size.
    icon_xl => "icon-xl" = 24.0,
    /// The empty-state glyph size.
    icon_2xl => "icon-2xl" = 32.0,
    /// The title band height.
    titlebar_h => "titlebar-h" = 32.0,
    /// The toolbar band height.
    toolbar_h => "toolbar-h" = 40.0,
    /// The document tab bar height.
    tab_bar_h => "tab-bar-h" = 32.0,
    /// The editor status bar and bottom bar height.
    bar_h => "bar-h" = 24.0,
    /// The console toolbar height.
    console_bar_h => "console-bar-h" = 28.0,
    /// The sidebar header height.
    sidebar_header_h => "sidebar-header-h" = 32.0,
    /// The find bar height (one row).
    find_bar_h => "find-bar-h" = 36.0,
    /// The sidebar default width.
    sidebar_w => "sidebar-w" = 240.0,
    /// The sidebar minimum width.
    sidebar_min_w => "sidebar-min-w" = 180.0,
    /// The sidebar maximum width.
    sidebar_max_w => "sidebar-max-w" = 400.0,
    /// The documentation panel default width.
    docs_w => "docs-w" = 300.0,
    /// The documentation panel minimum width.
    docs_min_w => "docs-min-w" = 240.0,
    /// The console minimum height at the bottom.
    console_min_h => "console-min-h" = 96.0,
    /// The console minimum width at the right.
    console_min_w => "console-min-w" = 240.0,
    /// The editor column minimum width.
    editor_min_w => "editor-min-w" = 240.0,
    /// The resize handle's hit width.
    handle_hit => "handle-hit" = 9.0,
    /// A separator's thickness.
    hairline => "hairline" = 1.0,
    /// The caption type size.
    type_caption => "type-caption" = 12.0,
    /// The body type size.
    type_body => "type-body" = 13.0,
    /// The subhead type size.
    type_subhead => "type-subhead" = 15.0,
    /// The title type size.
    type_title => "type-title" = 17.0,
    /// The display type size.
    type_display => "type-display" = 22.0,
    /// The caption line height.
    leading_caption => "leading-caption" = 16.0,
    /// The body line height.
    leading_body => "leading-body" = 18.0,
    /// The subhead line height.
    leading_subhead => "leading-subhead" = 20.0,
    /// The title line height.
    leading_title => "leading-title" = 24.0,
    /// The display line height.
    leading_display => "leading-display" = 28.0,
    /// The default editor font size.
    editor_font_size => "editor-font-size" = 13.0,
    /// The editor line height as a multiple of its font size.
    editor_line_height_ratio => "editor-line-height-ratio" = 1.45,
    /// The console font size.
    console_font_size => "console-font-size" = 12.0,
    /// Hover wash and color changes, milliseconds.
    motion_instant => "motion-instant" = 100.0,
    /// Presses and control state, milliseconds.
    motion_fast => "motion-fast" = 150.0,
    /// Moving into place, milliseconds.
    motion_base => "motion-base" = 220.0,
    /// Reveals, milliseconds.
    motion_slow => "motion-slow" = 300.0,
    /// Unhurried entrances, milliseconds.
    motion_slower => "motion-slower" = 450.0,
    /// A list wave's row delay, milliseconds.
    motion_stagger => "motion-stagger" = 40.0,
}

/// The editor's row height in points: its font size times the line-height
/// ratio, rounded to whole points (DESIGN.md 8.6.1).
pub fn editor_line_height() -> f32 {
    (editor_font_size() * editor_line_height_ratio()).round()
}
