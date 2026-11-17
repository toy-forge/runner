# scratch-ui-kit

ScratchRunner's component library: DESIGN.md sections 2-10 over `nsh_base`.
One typed accessor per design token, the type steps and the code font, motion
helpers, the icon set, and every control of section 7 as a free function.

The crate owns no app state. A component takes values as
`impl IntoReactive<T>`, reports changes through closures, sets only its own
measurements and states, and leaves layout (width, flex, margins) to the
caller. Every public item is documented.

```toml
[dependencies]
scratch-ui-kit = { path = "crates/ui-kit" }
```

## Wiring

### Theme tokens

The kit reads `ui/theme.json` through `nsh_base::theme()`. Parse it once and
give the tokens to the root; a missing token reads loud magenta, never a panic.

```rust
let (theme, ignored) = nsh_base::Theme::from_json_lenient(
    &std::fs::read_to_string("ui/theme.json")?,
)?;
for value in ignored {
    eprintln!("theme value ignored: {value}");
}
let root = nsh_platform_macos::Root::of::<App>().tokens(theme.into_tokens());
```

With `nsh.json` (`"theme": "ui/theme.json"`) NSH does the same by itself.

### Assets (the SVG icons)

`asset!` publishes the crate's files under
`asset:/@scratch-ui-kit/icons/<name>.svg` (`scratch_ui_kit::ASSETS` lists all
85).

* Packaged apps and `nsh run`: register them with the engine, e.g.
  `registry.assets(scratch_ui_kit::ASSETS)` or
  `scratch_ui_kit::register_assets(&mut registry)`.
* A binary run with `cargo run`: the platform serves `asset:/` from
  `AppOptions::asset_base`; point it at a directory that contains
  `@scratch-ui-kit/icons/...`. The gallery example links
  `target`-adjacent staging to `crates/ui-kit/assets` at startup.

Without either, `svg()` loads nothing and glyphs stay blank.

### Import

```rust
use scratch_ui_kit::prelude::*; // nsh_core prelude + primitives + icons + type steps + motion
use scratch_ui_kit::tokens;     // the design tokens (aliased design_tokens in the prelude)
```

## Foundation

### `tokens`

One function per token of DESIGN.md 3, read from `theme()` inside bindings so
they follow the appearance and runtime theme changes. Colors return
`AdaptiveColor`; numbers return `f32` (points, or milliseconds for `motion-*`)
and fall back to the DESIGN value when the theme does not set them.

| Group | Accessors |
| --- | --- |
| Base | `background foreground card card_foreground popover popover_foreground primary primary_foreground primary_hover primary_active link secondary secondary_foreground secondary_hover secondary_active muted muted_foreground accent accent_foreground destructive destructive_hover destructive_active border input ring chart_1..chart_5 sidebar*` |
| Window chrome | `window toolbar toolbar_border capsule capsule_border capsule_hover capsule_pressed capsule_selected selection selection_foreground selection_inactive selection_inactive_foreground menu_highlight menu_highlight_foreground control control_hover control_pressed field tab_bar tab_active tab_hover tab_foreground tab_active_foreground status_bar scrim shadow match_highlight` |
| Status | `success success_foreground success_muted warning warning_foreground warning_muted info info_foreground info_muted destructive_foreground destructive_muted` |
| Diff | `diff_added diff_added_foreground diff_removed diff_removed_foreground diff_changed diff_changed_foreground` |
| Editor scope | `editor_bg editor_fg editor_caret editor_selection editor_selection_inactive editor_line_highlight editor_invisibles editor_indent_guide editor_indent_guide_active editor_ruler editor_bracket_match editor_token_match editor_find_match editor_find_current editor_bar editor_bar_foreground editor_bar_border gutter_bg gutter_fg gutter_fg_active gutter_separator breakpoint breakpoint_ghost breakpoint_disabled debug_line ... console_* ansi_*` |
| Glyph tiles | `glyph_1() .. glyph_10()` |
| Numbers | `radius radius_sm radius_lg radius_xs ctl_xs ctl_sm ctl_md ctl_lg row_xs row_sm row_md row_lg icon_xs icon_sm icon_md icon_lg icon_xl icon_2xl type_display type_title type_subhead type_body type_caption leading_* hairline editor_font_size editor_line_height_ratio motion_instant motion_fast motion_base motion_slow motion_slower` |

Special helper: `editor_line_height() -> f32` (size x ratio, rounded).
`MISSING` is the magenta fallback itself.

### `typography`

```rust
pub const CODE_FONT_STACK: &str = "SF Mono, Menlo, monospace";

pub fn type_display() -> TypeStyle;
pub fn type_title() -> TypeStyle;
pub fn type_subhead() -> TypeStyle;
pub fn type_body() -> TypeStyle;
pub fn type_caption() -> TypeStyle;
pub fn type_mono() -> TypeStyle;
pub fn type_code_caption() -> TypeStyle;

impl TypeStyle {
    pub fn size(self) -> f32;
    pub fn line_height(self) -> f32;
    pub fn weight(self) -> FontWeight;
    pub fn family(self) -> Option<&'static str>;
    pub fn medium(self) -> Self;
    pub fn semibold(self) -> Self;
    pub fn regular(self) -> Self;
    pub fn with_size(self, size: fn() -> f32) -> Self;
    pub fn with_leading(self, leading: fn() -> f32) -> Self;
    pub fn with_family(self, family: &'static str) -> Self;
    pub fn apply<S: Styled>(self, element: S) -> S;
}
```

`type_body().apply(text("Run succeeded"))` sets size, line height and weight;
color stays the component's decision.

### `motion`

Section 9 in code; the durations and distances come from the theme.

```rust
pub const PRESS_SCALE: f32 = 0.97;
pub const PRESS_SCALE_ICON: f32 = 0.95;
pub const POPUP_SCALE: f32 = 0.97;
pub const BREAKPOINT_SCALE: f32 = 0.6;
pub const DISTANCE_LABEL: f32 = 4.0;
pub const DISTANCE_POPUP: f32 = 8.0;
pub const DISTANCE_SHEET: f32 = 12.0;
pub const DISTANCE_WAVE: f32 = 16.0;
pub const OPACITY_DISABLED: f32 = 0.45;
pub const OPACITY_LOADING: f32 = 0.7;
pub const OPACITY_INACTIVE: f32 = 0.6;
pub const STAGGER_CAP: usize = 10;
pub const EASE_ENTER/MOVE/LEAVE/SETTLE: Easing;

pub fn instant() -> f32;
pub fn fast() -> f32;
pub fn base() -> f32;
pub fn slow() -> f32;
pub fn slower() -> f32;
pub fn stagger() -> f32;
pub fn stagger_delay(index: usize) -> f32;
pub fn reduced() -> bool;

pub fn state_wash<S: Styled>(element: S) -> S;
pub fn state_opacity<S: Styled>(element: S) -> S;
pub fn press<S: Styled>(element: S) -> S;
pub fn press_scale<S: Styled>(element: S, icon_button: bool) -> S;
pub fn press_scale_icon() -> f32;
pub fn popup_starting<S: Styled>(element: S) -> S;
pub fn popup_ending<S: Styled>(element: S) -> S;
pub fn sliding_segment<S: Styled>(element: S) -> S;
pub fn spinner<S: Styled>(element: S) -> S;
pub fn fade_in_rise(ms: f32) -> MotionSpec;
pub fn enter_popup() -> MotionSpec;
pub fn panel_layout_animation() -> LayoutAnimation;
pub fn animate_panel();
pub fn panel_spring() -> Spring;
pub fn pop_spring() -> Spring;
```

The helpers honour `motion::reduced()` (spinners keep turning: they are
progress cues).

### `icon`

```rust
pub enum IconName { /* 85 variants, one per SVG */ }
impl IconName {
    pub const ALL: &'static [IconName];
    pub const fn path(self) -> &'static str;
    pub fn asset(self) -> ImageAsset;
}

pub enum IconSize { Xs, Sm, Md, Lg, Xl, Xxl }
impl IconSize { pub fn points(self) -> f32; }

pub enum Tone { Default, Muted, Primary, Success, Warning, Danger, Info, OnPrimary, OnSelection, Disabled }
impl Tone { pub fn color(self) -> AdaptiveColor; }

pub fn glyph(name: IconName, size: IconSize, tone: Tone) -> Svg;
pub fn glyph_at(name: IconName, points: f32, tone: Tone) -> Svg;

pub enum GlyphTone { One .. Ten }
impl GlyphTone {
    pub fn for_id(id: &str) -> Self;
    pub fn color(self) -> AdaptiveColor;
}
pub fn language_tile(language_id: &str) -> (&'static str, GlyphTone);
pub fn glyph_tile(label: impl IntoReactive<SharedString> + 'static, tone: GlyphTone) -> impl IntoElement;
pub fn language_glyph_tile(language_id: &str) -> impl IntoElement;
```

The 34 language ids of `crates/language/defs` map to authored 1-3 character
monograms with a stable tint (`fnv1a(id) % 10 + 1`). An unknown id is `?` in
`glyph-10`.

## Primitives

All return `impl IntoElement`, take text as
`impl IntoReactive<SharedString> + 'static`, and take events as closures.

### `ControlSize`

`Xs` 20, `Sm` 24, `Md` 28 (default), `Lg` 32. Methods: `height()`,
`padding_x()`, `text_size()`, `gap()`, `icon_size()`, `rect_radius()`,
`capsule()`, `capsule_member_height()`, `button_min_width()`.

### Buttons

```rust
pub fn button(
    label: impl IntoReactive<SharedString> + 'static,
    options: ButtonOptions,
    on_press: impl Fn() + 'static,
) -> impl IntoElement;

pub enum ButtonVariant { Primary, Default, Secondary, Ghost, Destructive, Link }
pub struct ButtonOptions {
    pub variant: ButtonVariant,
    pub size: ControlSize,
    pub icon: Option<IconName>,
    pub loading: bool,
    pub disabled: bool,
}
// ButtonOptions::new() and chainable .variant() .size() .icon() .loading() .disabled()

pub fn icon_button(
    icon: IconName,
    label: impl IntoReactive<SharedString> + 'static, // accessible name and tooltip
    options: IconButtonOptions,
    on_press: impl Fn() + 'static,
) -> impl IntoElement;

pub enum IconButtonVariant { Ghost, Capsule, Filled }
pub struct IconButtonOptions {
    pub variant: IconButtonVariant,
    pub size: ControlSize,
    pub selected: bool,
    pub disabled: bool,
    pub width: Option<f32>,
    pub shortcut: Option<&'static str>,
}
```

### Capsule groups and segmentation

```rust
pub fn capsule_group(
    label: impl IntoReactive<SharedString> + 'static,
    options: CapsuleGroupOptions,
    members: impl IntoIterator<Item = impl IntoElement>,
) -> impl IntoElement;
pub struct CapsuleGroupOptions { pub size: ControlSize }

pub struct Segment { pub label: Reactive<SharedString>, pub icon: Option<IconName>, pub disabled: bool }
impl Segment {
    pub fn new(label: impl IntoReactive<SharedString> + 'static) -> Self;
    pub fn icon(self, icon: IconName) -> Self;
    pub fn disabled(self, disabled: bool) -> Self;
}

pub fn segmented(
    label: impl IntoReactive<SharedString> + 'static,
    segments: impl IntoIterator<Item = Segment>,
    selected: impl Fn() -> usize + Clone + 'static,
    on_select: impl Fn(usize) + Clone + 'static,
    options: SegmentedOptions,
) -> impl IntoElement;
pub struct SegmentedOptions { pub size: ControlSize, pub equal: bool, pub disabled: bool }
```

The segmented pill slides natively (`tabs().indicator_style(..)`); arrows,
Home and End move and select.

### Popups and selects

```rust
pub fn popup_button(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    items: Vec<MenuItem>,
    options: PopupButtonOptions,
    on_open_change: impl Fn(bool) + 'static,
) -> impl IntoElement;

pub enum PopupButtonVariant { Field, Capsule, Bar }
pub struct PopupButtonOptions { pub variant: PopupButtonVariant, pub size: ControlSize, pub disabled: bool }

pub fn select_popup(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<Option<SharedString>> + 'static,
    placeholder: impl IntoReactive<SharedString> + 'static,
    items: Vec<SelectItem>,
    options: SelectPopupOptions,
    on_change: impl Fn(Option<SharedString>) + 'static,
) -> impl IntoElement;
pub struct SelectPopupOptions { pub popup: PopupButtonOptions, pub searchable: bool, pub cover: bool }
```

`popup_button` measures its trigger and opens the native
`popup_menu_at` under it (falling back to a centred `popup_menu` if the
measurement is unavailable). `select_popup` wraps `nsh_base::select()` with
the same trigger looks, a search field, a covering list and styled popup and
row.

### Fields

```rust
pub fn search_field(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    trailing: impl IntoReactive<Option<SharedString>> + 'static, // a count or a reason
    on_change: impl Fn(&str) + 'static,
    on_clear: impl Fn() + 'static, // the caller clears its value
    options: SearchFieldOptions,
) -> impl IntoElement;
pub struct SearchFieldOptions {
    pub size: ControlSize, // default Sm
    pub leading: IconName, // default Search
    pub invalid: bool,
    pub disabled: bool,
    pub borderless: bool,
}

pub fn text_field(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    on_change: impl Fn(&str) + 'static,
    options: TextFieldOptions,
    error: Option<SharedString>,
) -> impl IntoElement;
pub fn text_area(/* the same parameters */) -> impl IntoElement;

pub struct TextFieldOptions {
    pub size: ControlSize,
    pub invalid: bool,
    pub readonly: bool,
    pub disabled: bool,
    pub mono: bool,
    pub accessibility_label: Option<&'static str>,
}

pub fn code_field(
    value: impl IntoReactive<Option<SharedString>> + 'static,
    on_change: impl Fn(&SharedString) + 'static,
    options: CodeFieldOptions,
) -> impl IntoElement;
pub struct CodeFieldOptions {
    pub language: SharedString, // default "plaintext"
    pub min_rows: u32,          // default 2
    pub max_rows: u32,          // default 8
    pub line_numbers: bool,     // default true
    pub readonly: bool,
    pub invalid: bool,
    pub accessibility_label: Option<&'static str>,
    pub placeholder: Option<SharedString>,
}
// CodeFieldOptions::new(language) and chainable setters

pub fn code_caption(label: impl IntoReactive<SharedString> + 'static) -> impl IntoElement;
```

`code_field` uses `nsh_base::editor()` with the code font stack, the
editor-scope colors, line numbers and min/max rows.

### Checkboxes, radios, switches

```rust
pub fn checkbox(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    options: CheckboxOptions,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement;
pub struct CheckboxOptions { pub indeterminate: bool, pub disabled: bool }

pub fn radio_group(
    label: impl IntoReactive<SharedString> + 'static,
    children: impl IntoIterator<Item = impl IntoElement>,
) -> impl IntoElement;
pub fn radio(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    disabled: bool,
    on_select: impl Fn() + 'static,
) -> impl IntoElement;

pub fn switch(
    label: impl IntoReactive<SharedString> + 'static,
    checked: impl IntoReactive<bool> + 'static,
    disabled: bool,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement;
```

### Numbers and colors

```rust
pub fn number_field(
    label: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<f64> + 'static,
    options: NumberFieldOptions,
    on_change: impl Fn(f64) + 'static,
) -> impl IntoElement;
pub fn stepper(/* same parameters */) -> impl IntoElement; // number_field

pub struct NumberFieldOptions {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: f64,       // default 1
    pub width: f32,      // default 56
    pub suffix: Option<&'static str>,
    pub prefix: Option<&'static str>,
    pub disabled: bool,
}
// NumberFieldOptions::new() and chainable .range() .step() .width() .suffix() .prefix() .disabled()

pub fn color_well(
    name: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<Option<Hsla>> + 'static,
    on_change: impl Fn(Hsla) + 'static,
    options: ColorWellOptions,
) -> impl IntoElement;
pub enum ColorWellSize { Well, Chip }
pub struct ColorWellOptions { pub size: ColorWellSize, pub disabled: bool }
```

`NumberField` is right-aligned, tabular, with a 16pt stepper; arrows step by
`step`, Shift+arrow by ten times it. `ColorWell` opens a popover with
`color_picker()` (area, hue and alpha sliders, hex field, eye dropper).

### Marks and structure

```rust
pub fn kbd(label: impl IntoReactive<SharedString> + 'static) -> impl IntoElement;
pub fn kbd_for(keystroke: &str) -> impl IntoElement; // "mod+shift+o"; never panics

pub fn badge(
    label: impl IntoReactive<SharedString> + 'static,
    options: BadgeOptions,
) -> impl IntoElement;
pub enum BadgeVariant { Neutral, Count, Primary, Success, Warning, Danger }
pub struct BadgeOptions { pub variant: BadgeVariant, pub icon: Option<IconName> }

pub fn section_header(
    title: impl IntoReactive<SharedString> + 'static,
    variant: SectionHeaderVariant,
    options: SectionHeaderOptions,
) -> impl IntoElement;
pub fn section_header_with_actions(
    title: impl IntoReactive<SharedString> + 'static,
    variant: SectionHeaderVariant,
    actions: impl IntoIterator<Item = impl IntoElement>,
    options: SectionHeaderOptions,
) -> impl IntoElement;
pub enum SectionHeaderVariant { Form, List, Pane }
pub struct SectionHeaderOptions { pub divider: bool }

pub fn form_row(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    options: FormRowOptions,
) -> impl IntoElement;
pub fn form_row_with_caption(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    caption: impl Into<SharedString>,
    options: FormRowOptions,
) -> impl IntoElement;
pub fn form_row_with_help(
    label: impl IntoReactive<SharedString> + 'static,
    control: impl IntoElement,
    on_help: impl Fn() + 'static,
    options: FormRowOptions,
) -> impl IntoElement;
pub struct FormRowOptions { pub label_width: f32, pub caption: Option<SharedString> }

pub fn separator(orientation: SeparatorOrientation) -> impl IntoElement;
pub enum SeparatorOrientation { Horizontal, Vertical }
```

### Feedback, tooltips and split handles

```rust
pub fn spinner(size: IconSize, tone: Tone, label: &'static str) -> impl IntoElement;

pub fn progress_bar(
    value: impl IntoReactive<f32>, // 0..=100
    options: ProgressOptions,
) -> impl IntoElement;
pub struct ProgressOptions {
    pub height: f32, // default 2; sheets use 4
    pub indeterminate: bool,
    pub label: Option<&'static str>,
}

pub fn tooltip(
    label: impl IntoReactive<SharedString> + 'static,
    shortcut: Option<&'static str>,
) -> nsh_base::TooltipElement;
// element.tooltip(tooltip("Run", Some("⌘R")))

pub fn split_line(axis: Axis) -> impl IntoElement;
pub fn split_handle(
    axis: Axis,
    label: impl IntoReactive<SharedString> + 'static,
    disabled: bool,
) -> impl IntoElement;
```

## Gallery

`crates/ui-kit/examples/gallery.rs` renders every component in four pages
(Foundation, Controls, Chrome, Icons), in both appearances:

```sh
cargo run -p scratch-ui-kit --example gallery
GALLERY_PAGE=2 GALLERY_APPEARANCE=dark cargo run -p scratch-ui-kit --example gallery
GALLERY_SCROLL=600 cargo run -p scratch-ui-kit --example gallery
```

Screenshots live in `docs/design/shots/`.

## Notes and deviations

* **`Size` is `ControlSize`.** `nsh_core::prelude` already exports a
  `Size` (a geometry size), and the prelude re-exports both; naming ours
  `ControlSize` keeps `use scratch_ui_kit::prelude::*` free of an ambiguous
  name.
* **Language monograms are authored.** `crates/language/defs` has no `short`
  field, so a 1-3 character monogram per language id is in `icon.rs` and
  tested. Two- and three-letter monograms scale down from the caption step
  (0.875 and 0.66) and are measured against the tile's full 16pt width, with
  clipped overflow; at the full caption step they wrapped or overflowed in
  SF Pro, and `C++` needs the smaller of the two steps.
* **Number field units sit outside the outline.** `prefix` and `suffix` are
  muted captions on the field's row, so the 56pt field keeps its designed
  width and the number and stepper are never clipped.
* **`popup_button` anchors natively.** It measures its trigger
  (`ElementRef::measure`) and calls `popup_menu_at`; when no measurement is
  available it falls back to `popup_menu`.
* **Asset registration is the app's job.** The kit exports `ASSETS` and
  `register_assets` but cannot register itself with the engine; see
  [Assets](#assets-the-svg-icons).
* **Theme vs DESIGN.md.** `ui/theme.json` and DESIGN.md 3.2 agree: both
  appearances carry the same 148 color names and the 55 numeric tokens, so no
  mismatch was found. `theme.json` remains the source of truth.
* **Reduced motion.** Helpers read `motion::reduced()`; spinners keep turning
  and indeterminate progress becomes a static 40% segment with a caption.
* **No panics.** Token reads fall back to magenta or the DESIGN number,
  `kbd_for` falls back to the raw keystroke, and no component unwraps.
