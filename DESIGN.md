# ScratchRunner design standards

This is the design system and the screen specification of ScratchRunner, a native macOS code scratchpad that re-implements the behaviour of CodeRunner 4.6 in its own look, code and assets. It is normative for every `ui` component and every screen: implementers build from it without guessing, reviewers check against it.

It follows the NSH design guides (`.claude/skills/nsh-design-guides/references/design-guides.md`). Where a number below differs from the guide's starting values, this document wins for ScratchRunner and says why.

Companion files:

| File | What it is |
|---|---|
| `ui/theme.json` | The NSH theme (shadcn `cssVars`): every color and number token in section 3. Set `"theme": "ui/theme.json"` in `nsh.json`. Validated with `nsh_base::Theme::from_json_lenient` (148 colors, 55 numbers, 0 ignored). |
| `assets/icons/*.svg` | The icon set (section 6). 85 files, 16x16 grid, `currentColor`. Each is validated with `nsh_core::svg::drawing`, the function NSH's `svg()` element uses. |
| `docs/design/screens.md` | ASCII wireframes of every screen and state (this document holds the measurements and copy). |

Contents: 1 Thesis, 2 Targets and density, 3 Theme tokens, 4 Typography, 5 Spacing, radius, elevation, 6 Icons, 7 Components, 8 Surfaces (the main window and every other window), 9 Motion, 10 Keyboard, focus and accessibility, 11 NSH limits the design works around, 12 Finishing checklist and verification status.

## 1. Design thesis

ScratchRunner is a tool people open dozens of times a day to type a few lines, press Run and read the answer. The design serves that loop.

1. **The loop is the interface.** Type, Run (⌘R), read the console, change, Run again. The Run group, the editor and the console are the three loudest things in the window. Everything else (sidebar, docs, debugger, preferences) is quiet until asked for.
2. **Native by construction, not by imitation.** The window looks like a macOS 26 document window: a title band with the traffic lights, one unified toolbar row of capsule groups, a translucent sidebar with hairline edges, 24pt bottom bars. Menus, alerts, file dialogs, the menu bar and context menus are the system's own (NSH exposes them). Everything NSH cannot draw natively is built to the same measurements, not to a web look.
3. **Two layers of color that never mix.** The *chrome* (toolbar, sidebar, tab bar, bars, sheets, preferences) follows the system appearance with neutral, slightly cool greys and one blue. The *editor scope* (editor, gutter, find highlights, console, editor status bar) follows the active syntax theme (a `.tmTheme`) and may be dark while the system is light. The chrome never takes the syntax theme's hues; the editor scope never uses chrome tokens. Section 3.4 defines the bridge.
4. **One accent.** `primary` (system-style blue) marks: the default button, the focused selection, a toggled toolbar icon, a breakpoint and a link. Green, orange and red appear only as status (run result, warning, error), always with an icon or word beside them.
5. **Hairlines, not boxes.** Regions are separated by a 1pt hairline owned by one side, fills of a slightly different tone, and spacing. Cards appear only in Preferences forms. Shadows exist only on floating things (menus, popovers, sheets, toasts, the selected tab).
6. **Dense, exact, calm.** 28pt controls, 24pt rows, 12/13pt text, everything on a 4pt grid. Equal gaps are equal to the pixel. Motion is short (100 to 300 ms), explains a change, and is off with Reduce Motion.
7. **Keyboard first.** Every action has a menu item and, if frequent, a shortcut. Focus is always visible. Nothing important lives only in a hover.
8. **Faithful to behaviour, free in look.** Copy, structure and behaviour come from CodeRunner 4.6. Icons, colors, themes, scripts and grammars are authored here. The product name is ScratchRunner everywhere.

## 2. Targets and density

- **Target:** macOS 12+ desktop, pointer and keyboard, Retina. One family (desktop). No touch targets, no TUI, no web. Light and dark appearance both supported, following the system (`nsh.json` `"appearance": "system"`), overridable by the Themes pane (`set_color_scheme`).
- **Density tier:** *compact/standard desktop*. Default control `md` = 28pt (the height of the macOS 26 toolbar capsules). Dense regions (find bar, status popups, sidebar footer, console toolbar) use `sm` 24 or `xs` 20 as the table below says. Sheets and prompts use `lg` 32. The guide's default of 32 is intentionally not used for chrome: this is an information-dense tool and the 28 matches the platform's toolbar.
- **One size drives everything.** A control takes `size` = `xs | sm | md | lg`; the size sets height, horizontal padding, text size, icon size, gap and radius together (table 2.1). A control group uses one size throughout.

### 2.1 Size scale

| Size | Height (`ctl-*`) | Padding x | Text | Icon | Gap icon/label | Radius (rect buttons) | Used for |
|---|---|---|---|---|---|---|---|
| `xs` | 20 | 6 | `type-caption` 12 | `icon-xs` 12 | 4 | `radius-sm` 6 | Console toolbar items, sidebar footer buttons, inline row actions |
| `sm` | 24 | 8 | `type-body` 13 | `icon-sm` 14 | 4 | `radius-sm` 6 | Find bar, status bar popups, tab items, debugger buttons, dense forms |
| `md` | 28 | 12 | `type-body` 13 | `icon-md` 16 | 6 | capsule (`radius-full`) | Default: toolbar, forms, preferences, docs header |
| `lg` | 32 | 16 | `type-body` 13 | `icon-md` 16 | 8 | capsule (`radius-full`) | Sheet and prompt buttons, prompt and QuickNav fields |

Icon buttons are squares of the control height (20, 24, 28, 32) with the icon from the same row. Text fields and popups share the heights; fields use `radius` (8) at `sm`/`md`/`lg`, search fields are capsules.

Row heights: `row-sm` 24 (file tree, language list, completion, menu items), `row-md` 28 (tables, symbol list, form rows), `row-lg` 32, `row-xl` 44 (QuickNav and search results).

### 2.2 Windows and minimum sizes

| Window | Default | Minimum | Resizable | Notes |
|---|---|---|---|---|
| Main (document) window | 1100 x 680 | 500 x 320 | yes | Hidden system title bar (`TitleBarOptions { style: Hidden, height: Some(32) }`), own title band and toolbar. Frame restored (`restore_bounds`). |
| Preferences | per pane (8.13) | per pane | Languages, Themes, Key Bindings only | One window, one pane at a time, title `Preferences – <Pane>`. |
| Help | 760 x 560 | 560 x 360 | yes | |
| About | 320 x auto | = default | no | |
| Console pop-out, Run Settings, QuickNav, prompts, sheets | n/a | n/a | no | In-window modal layers (`dialog()`), except where 8.x says otherwise. |

Main window responsive rules (the layout holds from 500 wide up):

1. At width < 720 the docs panel closes automatically (a user choice to reopen it wins until the next narrowing).
2. The toolbar's language popup shrinks from 200 to a 120 minimum (label truncates with an ellipsis, tooltip shows the full name). At 500 the toolbar needs 447pt, so it fits.
3. Panel minimums: sidebar 180, editor column 240, docs 240, console 96 high (bottom) or 240 wide (right). If the window cannot hold them, panels collapse in this order: docs, sidebar, console. A collapsed panel keeps its toggle and its menu item.
4. Console toolbar items collapse into its `ellipsis` menu from the trailing end: web-inspector segment, debugger-view segment, Live Updating, then the filter field (becomes an icon-only button that expands in place).
5. Only the console, the file tree, the docs page, the preferences panes and the editor scroll. The window never scrolls.

## 3. Theme tokens

### 3.1 Structure

Three layers of color, from the most stable to the most dynamic:

1. **Chrome tokens** (shadcn names plus `window`, `toolbar`, `capsule*`, `tab-*`, `status-bar`, `selection*`, `control*`, `field`, status colors, diff colors, `glyph-*`). They have a light and a dark value and follow the system appearance. They live in `ui/theme.json`.
2. **Editor-scope tokens** (`editor-*`, `gutter-*`, `breakpoint*`, `debug-*`, `diag-*`, `console-*`, `ansi-*`, `editor-bar*`). `ui/theme.json` holds default light and dark values (a plain light and a plain dark syntax theme). At runtime the *ThemeResolver* (3.4) overwrites them from the active syntax theme with `Theme::set_color(name, AdaptiveColor { light: v, dark: v })` followed by `set_tokens`, so both appearances carry the same value (the syntax theme, not the system, decides). They are used only by the editor, gutter, find highlights, console and editor status bar.
3. **Syntax token colors** (keyword, string, comment...). Never tokens: they come from the `.tmTheme` through the app's own highlighter as `EditorDecoration` styles. They are not specified here.

Rules: no raw hex, `rgb()` or `hsl()` in presentation code (only in `ui/theme.json` and in the ThemeResolver's theme data). Every color is read through one function per token in `ui/theme.rs` (`fn surface_window() -> AdaptiveColor` and so on; a missing token returns a loud magenta fallback, never `unwrap`). Translucent tokens (`accent`, `capsule*`, `sidebar*`, diff, `*-muted`) are drawn over their parent surface; they are designed to read on both `window` and `background`.

### 3.2 Token tables

Names follow shadcn where a role exists (`primary`, `muted`, `accent`...), so base NSH components that read the theme keep working. Note the shadcn meaning: `accent` is the quiet *hover wash*, not the blue; the blue is `primary`.

#### 3.2.1 Color tokens (`cssVars.light` / `cssVars.dark`)

Values are `#RRGGBB` or `#RRGGBBAA` (alpha last). Read through `ui/theme.rs` as `theme().color("name")` (custom) or `theme().<field>` (shadcn base names).


**Base surfaces and text (shadcn roles)**

| Token | Light | Dark | Role |
|---|---|---|---|
| `background` | `#FFFFFF` | `#1E1E21` | Canvas of preference panes, sheets, forms and empty states |
| `foreground` | `#1D1D1F` | `#ECECF0` | Primary text and icons |
| `card` | `#FFFFFF` | `#27272B` | Grouped form section sitting on `background` |
| `card-foreground` | `#1D1D1F` | `#ECECF0` | Text on `card` |
| `popover` | `#FFFFFF` | `#2C2C30` | Menus, popovers, tooltips, completion list, QuickNav panel |
| `popover-foreground` | `#1D1D1F` | `#ECECF0` | Text on `popover` |
| `primary` | `#0071E3` | `#0A6FEA` | System-style blue: default button, focused selection accent, toggled icon tint, links |
| `primary-foreground` | `#FFFFFF` | `#FFFFFF` | Text and icons on `primary` |
| `primary-hover` | `#0066CC` | `#2A82F0` | `primary`, hovered |
| `primary-active` | `#005AB3` | `#075DCB` | `primary`, pressed |
| `link` | `#0066CC` | `#64B5FF` | Underlined link text (docs, About, alerts) |
| `secondary` | `#ECECF0` | `#3A3A3F` | Quiet filled button |
| `secondary-foreground` | `#1D1D1F` | `#ECECF0` | Text on `secondary` |
| `secondary-hover` | `#E3E3E8` | `#45454B` | `secondary`, hovered |
| `secondary-active` | `#D9D9DF` | `#505057` | `secondary`, pressed |
| `muted` | `#F0F0F3` | `#2E2E32` | Quiet fills: kbd, neutral badge, track, code chip |
| `muted-foreground` | `#6B6B72` | `#9A9AA2` | Secondary text, placeholders, inactive icons |
| `accent` | `#0000000F` | `#FFFFFF14` | Hover wash (translucent, composes on any surface) |
| `accent-foreground` | `#1D1D1F` | `#ECECF0` | Text on a hovered row |
| `destructive` | `#D70015` | `#FF453A` | Destructive actions and errors |
| `destructive-hover` | `#BF0012` | `#FF5C52` | `destructive`, hovered |
| `destructive-active` | `#A6000F` | `#E63B31` | `destructive`, pressed |
| `border` | `#0000001A` | `#FFFFFF1A` | Hairline separators and outlines (1pt) |
| `input` | `#00000026` | `#FFFFFF2E` | Outline of fields and bordered controls |
| `ring` | `#0071E38C` | `#0A84FF99` | Keyboard focus ring (drawn outside the control) |
| `chart-1` | `#0071E3` | `#0A84FF` | Series 1 (CPU/memory sparkline, stats) |
| `chart-2` | `#0C817B` | `#40CBE0` | Series 2 |
| `chart-3` | `#248A3D` | `#32D74B` | Series 3 |
| `chart-4` | `#B25E00` | `#FFB340` | Series 4 |
| `chart-5` | `#D30F45` | `#FF6482` | Series 5 |
| `sidebar` | `#F2F2F6B8` | `#232327B8` | File navigator and search view fill, translucent over the Sidebar backdrop |
| `sidebar-foreground` | `#1D1D1F` | `#ECECF0` | Text in the sidebar |
| `sidebar-primary` | `#0071E3` | `#0A6FEA` | Accent inside the sidebar (filter focus, toggles) |
| `sidebar-primary-foreground` | `#FFFFFF` | `#FFFFFF` | Text on `sidebar-primary` |
| `sidebar-accent` | `#0000001C` | `#FFFFFF21` | Selected sidebar row |
| `sidebar-accent-foreground` | `#1D1D1F` | `#ECECF0` | Text on a selected sidebar row |
| `sidebar-border` | `#0000001A` | `#FFFFFF14` | Sidebar hairlines |
| `sidebar-ring` | `#0071E38C` | `#0A84FF99` | Focus ring inside the sidebar |

**Window chrome**

| Token | Light | Dark | Role |
|---|---|---|---|
| `window` | `#F5F5F7` | `#28282B` | Window base (behind toolbar, tab bar, bars, sheets header) |
| `toolbar` | `#F5F5F7` | `#2A2A2E` | Title band and toolbar band |
| `toolbar-border` | `#00000026` | `#00000066` | Hairline under the toolbar |
| `capsule` | `#FFFFFFB8` | `#FFFFFF17` | Capsule group fill (toolbar groups, segmented tracks) |
| `capsule-border` | `#00000014` | `#FFFFFF1A` | Capsule group outline |
| `capsule-hover` | `#0000000D` | `#FFFFFF1A` | Hovered button inside a capsule |
| `capsule-pressed` | `#0000001A` | `#FFFFFF26` | Pressed button inside a capsule |
| `capsule-selected` | `#0071E31F` | `#0A84FF38` | Toggled-on segment (view toggles, active mode) |
| `selection` | `#0071E3` | `#0B62CC` | Selected row of a focused list or table |
| `selection-foreground` | `#FFFFFF` | `#FFFFFF` | Text on `selection` |
| `selection-inactive` | `#D9D9DF` | `#46464B` | Selected row when the list or window is not key |
| `selection-inactive-foreground` | `#1D1D1F` | `#ECECF0` | Text on `selection-inactive` |
| `menu-highlight` | `#0071E3` | `#0B62CC` | Highlighted item of a menu or popup list |
| `menu-highlight-foreground` | `#FFFFFF` | `#FFFFFF` | Text on `menu-highlight` |
| `control` | `#FFFFFF` | `#FFFFFF14` | Bordered button and checkbox fill |
| `control-hover` | `#F7F7F9` | `#FFFFFF1F` | Bordered control, hovered |
| `control-pressed` | `#EDEDF0` | `#FFFFFF2B` | Bordered control, pressed |
| `field` | `#FFFFFF` | `#0000003D` | Text field fill |
| `tab-bar` | `#E8E8EC` | `#202023` | Tab bar track |
| `tab-active` | `#FFFFFF` | `#3A3A3F` | Raised selection: selected tab and selected segment |
| `tab-hover` | `#0000000A` | `#FFFFFF0F` | Hovered unselected tab |
| `tab-foreground` | `#5F5F66` | `#9A9AA2` | Unselected tab label |
| `tab-active-foreground` | `#1D1D1F` | `#ECECF0` | Selected tab label |
| `status-bar` | `#F5F5F7` | `#26262A` | Bottom bars of the sidebar and docs panel |
| `scrim` | `#00000040` | `#00000073` | Dimmer behind sheets and QuickNav |
| `shadow` | `#00000033` | `#00000099` | Color of every drop shadow |
| `match-highlight` | `#FFD60A80` | `#FFD60A4D` | Search-term highlight in sidebar results and docs find |

**Status**

| Token | Light | Dark | Role |
|---|---|---|---|
| `success` | `#1F7A35` | `#32D74B` | Run Succeeded, enabled, passed |
| `success-foreground` | `#FFFFFF` | `#06210F` | Text on `success` |
| `success-muted` | `#1F7A351F` | `#32D74B2E` | Success banner wash |
| `warning` | `#A85F00` | `#FFB340` | Warnings, unsaved/attention |
| `warning-foreground` | `#FFFFFF` | `#2B1A00` | Text on `warning` |
| `warning-muted` | `#A85F001F` | `#FFB3402E` | Warning banner wash |
| `info` | `#0A6EBD` | `#64B5FF` | Notices, informational banners |
| `info-foreground` | `#FFFFFF` | `#00182B` | Text on `info` |
| `info-muted` | `#0A6EBD1F` | `#64B5FF2E` | Info banner wash |
| `destructive-foreground` | `#FFFFFF` | `#2B0503` | Text on `destructive` |
| `destructive-muted` | `#D700151A` | `#FF453A2E` | Error banner wash |

**Diff**

| Token | Light | Dark | Role |
|---|---|---|---|
| `diff-added` | `#248A3D26` | `#32D74B2E` | Added text background (replace preview) |
| `diff-added-foreground` | `#1B6E31` | `#7CE38B` | Added text |
| `diff-removed` | `#D7001521` | `#FF453A2E` | Removed text background |
| `diff-removed-foreground` | `#A4101F` | `#FF8A82` | Removed text |
| `diff-changed` | `#A85F0026` | `#FFB34033` | Changed text background |
| `diff-changed-foreground` | `#8A4E00` | `#FFC970` | Changed text |

**Editor scope (re-tinted at runtime by the active syntax theme; values here are the defaults for a light / dark syntax theme)**

| Token | Light | Dark | Role |
|---|---|---|---|
| `editor-bg` | `#FFFFFF` | `#1B1C21` | Editor and console background |
| `editor-fg` | `#3A3D42` | `#C5CAD3` | Editor and console text |
| `editor-caret` | `#1D1D1F` | `#FFFFFF` | Insertion point |
| `editor-selection` | `#0071E333` | `#FFFFFF2E` | Text selection (focused) |
| `editor-selection-inactive` | `#0000001F` | `#FFFFFF1A` | Text selection (editor not focused) |
| `editor-line-highlight` | `#00000008` | `#FFFFFF0F` | Current line |
| `editor-invisibles` | `#00000026` | `#FFFFFF33` | Invisible-character marks |
| `editor-indent-guide` | `#0000001A` | `#FFFFFF1A` | Indent guide |
| `editor-indent-guide-active` | `#00000040` | `#FFFFFF40` | Indent guide of the caret's block |
| `editor-ruler` | `#00000014` | `#FFFFFF14` | Page guide column |
| `editor-bracket-match` | `#0071E32E` | `#0A84FF47` | Balancing bracket highlight |
| `editor-token-match` | `#0071E333` | `#0A84FF47` | Token-aware Find and Select All matches |
| `editor-find-match` | `#FFD60A73` | `#FFD60A47` | Find matches |
| `editor-find-current` | `#FF9F0AA6` | `#FF9F0A8C` | The current find match |
| `editor-bar` | `#F5F5F7` | `#25262B` | Editor status bar and console toolbar |
| `editor-bar-foreground` | `#6B6B72` | `#9AA0AB` | Text and icons on `editor-bar` |
| `editor-bar-border` | `#00000014` | `#FFFFFF14` | Hairline above `editor-bar` |
| `gutter-bg` | `#FAFAFB` | `#1E1F25` | Line-number gutter |
| `gutter-fg` | `#8C9097` | `#707887` | Line numbers |
| `gutter-fg-active` | `#3A3D42` | `#C5CAD3` | Line number of the current line |
| `gutter-separator` | `#0000000F` | `#FFFFFF12` | Gutter's right edge (when enabled) |
| `breakpoint` | `#0071E3` | `#0A84FF` | Breakpoint tag |
| `breakpoint-ghost` | `#0071E359` | `#0A84FF59` | Breakpoint preview on gutter hover |
| `breakpoint-disabled` | `#A0A0A6` | `#6C6C72` | Disabled breakpoint |
| `debug-line` | `#0071E324` | `#0A84FF33` | Current execution line |
| `debug-arrow` | `#0071E3` | `#0A84FF` | Execution arrow in the gutter |
| `diag-error` | `#D70015` | `#FF5A52` | Error squiggle, icon, scroll marker |
| `diag-warning` | `#C98500` | `#FFD60A` | Warning squiggle, icon, scroll marker |
| `diag-info` | `#0071E3` | `#5AB1FF` | Notice squiggle, icon, scroll marker |
| `diag-hint` | `#8E8E93` | `#8E8E99` | Hint dotted underline |
| `diag-error-bg` | `#D700151A` | `#FF453A29` | Error line tint and annotation fill |
| `diag-warning-bg` | `#C985001F` | `#FFD60A24` | Warning line tint and annotation fill |
| `diag-info-bg` | `#0071E31A` | `#0A84FF29` | Notice line tint and annotation fill |
| `diag-hint-bg` | `#8E8E9314` | `#8E8E931F` | Hint annotation fill |
| `console-bg` | `#FFFFFF` | `#1B1C21` | Console body (runtime: equals `editor-bg`) |
| `console-fg` | `#3A3D42` | `#C5CAD3` | Program stdout text |
| `console-error` | `#D70015` | `#FF5A60` | stderr and error text |
| `console-info` | `#6E7073` | `#898B8D` | Informative, notice and debugger text |
| `console-prompt` | `#00804A` | `#00AE67` | Debugger prompt |
| `console-link` | `#0071E3` | `#5AB1FF` | Links and file:line references |
| `ansi-black` | `#1D1D1F` | `#6B6B75` | ANSI 30 |
| `ansi-red` | `#C41E3A` | `#FF5F5F` | ANSI 31 |
| `ansi-green` | `#1A7F37` | `#5FD068` | ANSI 32 |
| `ansi-yellow` | `#8C6D00` | `#E5C07B` | ANSI 33 |
| `ansi-blue` | `#1F5FD6` | `#61AFEF` | ANSI 34 |
| `ansi-magenta` | `#A02FB5` | `#C678DD` | ANSI 35 |
| `ansi-cyan` | `#0A7E8C` | `#56B6C2` | ANSI 36 |
| `ansi-white` | `#86868C` | `#D0D0D6` | ANSI 37 |
| `ansi-bright-black` | `#5C5C61` | `#7C7C86` | ANSI 90 |
| `ansi-bright-red` | `#E5384F` | `#FF8080` | ANSI 91 |
| `ansi-bright-green` | `#2FA84F` | `#7FE08A` | ANSI 92 |
| `ansi-bright-yellow` | `#B38600` | `#F5D78F` | ANSI 93 |
| `ansi-bright-blue` | `#3F7DF0` | `#80C0FF` | ANSI 94 |
| `ansi-bright-magenta` | `#BF4BD6` | `#DD94EE` | ANSI 95 |
| `ansi-bright-cyan` | `#1A9AAA` | `#7ADBE6` | ANSI 96 |
| `ansi-bright-white` | `#6B6B72` | `#FFFFFF` | ANSI 97 |

**Glyph tiles (language monograms, symbol kinds, stack-frame and variable badges)**

| Token | Light | Dark | Role |
|---|---|---|---|
| `glyph-1` | `#0071E3` | `#4DA3FF` | Blue tile |
| `glyph-2` | `#0C817B` | `#40CBE0` | Teal tile |
| `glyph-3` | `#1F7A35` | `#4CD964` | Green tile |
| `glyph-4` | `#A85F00` | `#FFB340` | Orange tile |
| `glyph-5` | `#D70015` | `#FF6961` | Red tile |
| `glyph-6` | `#D30F45` | `#FF6482` | Pink tile |
| `glyph-7` | `#8944AB` | `#DA8FFF` | Purple tile |
| `glyph-8` | `#3634A3` | `#8E8CF2` | Indigo tile |
| `glyph-9` | `#7C6A00` | `#E6CF4A` | Yellow tile |
| `glyph-10` | `#6B6B72` | `#9A9AA2` | Gray tile |

#### 3.2.2 Number tokens (`cssVars.theme`, both appearances)

Read with `theme().number("name")` (points, or milliseconds for `motion-*`). `radius` is also `theme().radius`.


**Radius scale**

| Token | Value | Use |
|---|---|---|
| `radius` | `8px` | `rounded(theme().radius)`: default control, row, menu item |
| `radius-xs` | `4px` | Checkbox, kbd, badge, scroller thumb, inline tag |
| `radius-sm` | `6px` | Tab item, xs/sm buttons, small icon buttons, code chip |
| `radius-lg` | `12px` | Menu, popover, tooltip, card/grouped section, find bar, toast |
| `radius-xl` | `16px` | Sheet, QuickNav panel, custom dialog, welcome card |

**Control sizes (height = width of an icon button)**

| Token | Value | Use |
|---|---|---|
| `ctl-xs` | `20px` | Inline icon buttons in rows, console toolbar items |
| `ctl-sm` | `24px` | Dense forms, find bar controls, status bar popups, tab close |
| `ctl-md` | `28px` | Default: toolbar capsule height, buttons, fields, selects |
| `ctl-lg` | `32px` | Sheet and dialog commit buttons, prompt fields |

**Row heights**

| Token | Value | Use |
|---|---|---|
| `row-sm` | `24px` | File tree, language list, completion rows, menu items, status popups |
| `row-md` | `28px` | Table rows, symbol list, form rows |
| `row-lg` | `32px` | Preference list rows with 2-line content |
| `row-xl` | `44px` | QuickNav result, search-result match row |

**Icon sizes**

| Token | Value | Use |
|---|---|---|
| `icon-xs` | `12px` | Inline glyph inside text, disclosure, badge icon |
| `icon-sm` | `14px` | Icon inside buttons of size xs and sm, tree rows, tab icon |
| `icon-md` | `16px` | Toolbar and default button icons (the 16px grid, 1:1) |
| `icon-lg` | `20px` | Prompt and alert glyphs |
| `icon-xl` | `24px` | Preferences tab strip |
| `icon-2xl` | `32px` | Empty states |

**Layout**

| Token | Value | Use |
|---|---|---|
| `titlebar-h` | `32px` | Title band (traffic lights, document title) |
| `toolbar-h` | `40px` | Toolbar band: 28 capsules + 6 padding each side |
| `tab-bar-h` | `32px` | Document tab bar (tab items are `ctl-sm`) |
| `bar-h` | `24px` | Editor status bar, sidebar and docs bottom bars |
| `console-bar-h` | `28px` | Console toolbar |
| `sidebar-header-h` | `32px` | Sidebar header (path popup row) |
| `find-bar-h` | `36px` | Find bar, one row (replace row adds 32) |
| `sidebar-w` | `240px` | Sidebar default width |
| `sidebar-min-w` | `180px` | Sidebar minimum |
| `sidebar-max-w` | `400px` | Sidebar maximum |
| `docs-w` | `300px` | Docs panel default width |
| `docs-min-w` | `240px` | Docs panel minimum |
| `console-min-h` | `96px` | Console minimum height (bottom position) |
| `console-min-w` | `240px` | Console minimum width (right position) |
| `editor-min-w` | `240px` | Editor column minimum width |
| `handle-hit` | `9px` | Resize handle hit width (line stays 1px) |
| `hairline` | `1px` | Separator thickness |

**Type scale (size)**

| Token | Value | Use |
|---|---|---|
| `type-caption` | `12px` | Status bar, badges, helper text, kbd |
| `type-body` | `13px` | Default interface text |
| `type-subhead` | `15px` | Pane headings, sheet titles |
| `type-title` | `17px` | Empty-state and About headings |
| `type-display` | `22px` | Welcome heading |

**Type scale (line height)**

| Token | Value | Use |
|---|---|---|
| `leading-caption` | `16px` | Line height for `type-caption` |
| `leading-body` | `18px` | Line height for `type-body` |
| `leading-subhead` | `20px` | Line height for `type-subhead` |
| `leading-title` | `24px` | Line height for `type-title` |
| `leading-display` | `28px` | Line height for `type-display` |

**Editor defaults**

| Token | Value | Use |
|---|---|---|
| `editor-font-size` | `13px` | Default Font size (user preference overrides) |
| `editor-line-height-ratio` | `1.45` | Line height = font size x ratio (Line Spacing 1.2 x natural 1.2) |
| `console-font-size` | `12px` | Console text size relative default (`editor-font-size` - 1) |

**Motion (milliseconds)**

| Token | Value | Use |
|---|---|---|
| `motion-instant` | `100` | Hover wash, color change |
| `motion-fast` | `150` | Press, control state |
| `motion-base` | `220` | Moving into place, glyph swap, popup leaving, selection slide |
| `motion-slow` | `300` | Reveals: popup, sheet, panel opening |
| `motion-slower` | `450` | Unhurried entrances: welcome, list wave |
| `motion-stagger` | `40` | Row delay step of a list wave (cap 10 rows) |

### 3.3 Reading and extending the tokens

- Base names are fields of `nsh_base::Theme` (`theme().primary`, `theme().muted_foreground`, `theme().sidebar_accent` ...). Everything else is read by name through the wrappers in `ui/theme.rs`: `color(name)`, `number(name)` (points), `ms(name)` (motion tokens).
- State colors per variant exist as tokens: `primary` / `primary-hover` / `primary-active`, `secondary` / `-hover` / `-active`, `destructive` / `-hover` / `-active`, `control` / `-hover` / `-pressed`, `capsule` / `-hover` / `-pressed` / `-selected`. Tinted and ghost variants use the translucent washes `accent` (hover, 6% / 8%) and `capsule-pressed` or `accent` at double strength for pressed (about 6% rest, 12% pressed); the guide's 10 / 20 / 40% ladder is not used because it reads as colored on neutral chrome.
- A selected button and a button whose menu is open share one style (`capsule-selected` for toggles, `control-pressed` for popups).
- To add a role: add it to `ui/theme.json` first with light and dark values, then to `ui/theme.rs`, then use it. Never name a token after a palette position.
- Appearance override: `set_color_scheme(Appearance::System | Light | Dark)` is global to all windows (G15). Per-window themes are not supported.
- No system accent bridge exists in NSH: the accent is fixed blue. A future "Accent color" pref would rewrite `primary*`, `ring`, `selection*`, `capsule-selected`, `menu-highlight`, `link` and `sidebar-primary` with `set_color`.

### 3.4 ThemeResolver: from a syntax theme to the editor-scope tokens

Input: the active `.tmTheme` global settings `background B`, `foreground F`, `caret`, `selection`, `lineHighlight`, `invisibles`, optional `findHighlight`, `activeGuide`, `gutterSettings`; the scope table for `console.*`. Output: the *editor-scope* group of tokens, written for both appearances. `mix(a, b, t)` is straight sRGB interpolation, `a` at `1 - t`. `dark(B)` means the relative luminance of `B` is below 0.5 (the same test CodeRunner uses for `isDarkTheme`).

| Token | Rule |
|---|---|
| `editor-bg`, `console-bg` | `B` |
| `editor-fg`, `console-fg` | `F` |
| `editor-caret` | theme `caret` (else `F`) |
| `editor-selection` | theme `selection`; if opaque it is drawn behind the text as is |
| `editor-selection-inactive` | `mix(B, selection, 0.55)` |
| `editor-line-highlight` | theme `lineHighlight` |
| `editor-invisibles` | theme `invisibles` |
| `gutter-bg` | `gutterSettings.background`, else `mix(B, F, 0.03)` |
| `gutter-fg` | `gutterSettings.foreground`, else `mix(B, F, 0.45)` (guarded to at least 3:1 on `gutter-bg`) |
| `gutter-fg-active` | `F` |
| `gutter-separator` | `mix(B, F, 0.08)` |
| `editor-indent-guide` / `-active` | `mix(B, F, 0.10)` / theme `activeGuide` else `mix(B, F, 0.25)` |
| `editor-ruler` | `mix(B, F, 0.08)` |
| `editor-bracket-match` | `F` at 14% (plus an underline in `F`) |
| `editor-find-match` / `-current` | theme `findHighlight` (else the default row of the table for `dark(B)`), current match = orange default |
| `editor-token-match` | the default row for `dark(B)` |
| `editor-bar` | `mix(B, F, 0.05)` |
| `editor-bar-foreground` | `mix(B, F, 0.62)` |
| `editor-bar-border` | `mix(B, F, 0.10)` |
| `console-error` / `-info` / `-prompt` / `-link` | foreground of scope `console.error` / `console.warning, console.notice, console.debugger` / `console.prompt` / `markup.underline.link`, else the default for `dark(B)` |
| `ansi-*` | foreground of `console.ansi.color.foreground.<color>.regular` / `.bright`, else the default for `dark(B)` |
| `diag-*`, `diag-*-bg`, `breakpoint*`, `debug-*` | the default row for `dark(B)`; each foreground color is nudged in lightness in 4% steps until it has 3:1 against `B` |
| `tab-bar`, `tab-active`, `toolbar`, `sidebar*`, `window` | never touched: the chrome does not take theme hues |

The "default row for `dark(B)`" is the light or dark column of the editor-scope table in 3.2.1. A theme whose `F` on `B` has less than 4.5:1 still applies, with a warning line in the Themes pane (`Low contrast`).

**Appearance matching.** Preference `UseSeparateColorThemes` (default on) with `ColorThemeLight` / `ColorThemeDark` picks the theme from the system appearance. With "Use the chosen theme for all appearances", `MatchAppearanceToTheme` decides whether the system color scheme is forced to the theme's darkness (`set_color_scheme(Dark | Light)`) so native scrollbars, menus and the chrome match the editor; off leaves `System`.

**Vibrancy needs a transparent window.** The sidebar's `NSVisualEffectView` (`macos_backdrop(MacOsBackdrop { material: Sidebar, blending: BehindWindow, state: FollowsWindowActive })`) blends only behind a transparent window (`WindowAttributes.transparent(true)`). Therefore every region except the sidebar paints an opaque fill from the chrome or editor tokens, and the root paints `window` behind the title band and toolbar. If vibrancy is unavailable, use the opaque `sidebar-solid` computation: `sidebar` composited over `window` at runtime.

### 3.5 Contrast (measured)

Computed from the token values (alpha composited over the stated surface), both appearances; WCAG 2.x ratios. Text pairs meet 4.5:1 unless listed under "graphics and secondary".

| Pair | Light | Dark |
|---|---|---|
| `foreground` / `background` | 16.8 | 14.1 |
| `muted-foreground` / `background` · `muted` · `window` | 5.3 · 4.7 · 4.9 | 6.0 · 4.8 · 5.3 |
| `primary-foreground` / `primary` | 4.7 | 4.7 |
| `destructive-foreground` / `destructive` | 5.4 | 5.5 |
| `success-foreground` / `success` · `warning-…` · `info-…` | 5.4 · 4.9 · 5.3 | 8.9 · 9.4 · 8.2 |
| `selection-foreground` / `selection` | 4.7 | 5.8 |
| `tab-foreground` / `tab-bar` · `tab-active-foreground` / `tab-active` | 5.2 · 16.8 | 5.8 · 9.6 |
| `editor-fg` / `editor-bg` | 10.9 | 10.3 |
| `editor-bar-foreground` / `editor-bar` | 4.9 | 5.8 |
| `console-error` · `-info` · `-prompt` · `-link` / `console-bg` | 5.4 · 5.0 · 5.0 · 4.7 | 5.6 · 5.0 · 5.9 · 7.4 |
| `ansi-red … cyan` / `console-bg` | 4.8 to 5.9 | 5.7 to 9.9 |
| Graphics and secondary: `gutter-fg` / `gutter-bg` | 3.1 | 3.7 |
| Graphics and secondary: `diag-warning`, `ansi-white` (light), `ansi-black` (dark) | 3.1, 3.6 | 12.1, 11.1, 3.2 |
| Graphics: `primary` as icon or toggle tint on `background` | 4.7 | 3.5 |

In dark, `primary` is a *fill and icon* color. Text that must read as a link uses `link` (7+ : 1 on dark). White on dark `primary` is 4.7:1.

### 3.6 Theme-aware states in one table

| State | Surface | Foreground | Edge |
|---|---|---|---|
| Rest | variant fill | variant foreground | variant border |
| Hover | `*-hover` fill or wash `accent` | unchanged | unchanged |
| Pressed | `*-active` fill or `capsule-pressed` / `control-pressed`; control scales to 0.97 (icon buttons 0.95) | unchanged | unchanged |
| Open (popup) / toggled on | same as pressed (popups) / `capsule-selected` (toggles) | `primary` icon for toggles | unchanged |
| Selected (list) | `selection` focused, `selection-inactive` not key; `sidebar-accent` in the sidebar | `selection-foreground` / `selection-inactive-foreground` / `sidebar-accent-foreground` | none |
| Focus-visible | unchanged | unchanged | 2pt `ring` outline, offset 1pt, radius = control radius + 2 |
| Disabled | unchanged | 45% opacity | no hover, no pressed, no focus ring |
| Loading | dimmed to 70%, spinner replaces icon, size unchanged | | |
| Invalid | unchanged | message in `destructive` | 1pt `destructive` outline (fields), shake once |

## 4. Typography

- **UI font:** the system font (SF Pro). No `font-sans` token is set, so NSH draws the system font; do not name a family. CJK and emoji fall back through the system cascade.
- **Code font stack** (editor, console, run-command editors, kbd in code context, hex fields): constant `CODE_FONT_STACK` in `ui/typography.rs`.
  - Default: `"SF Mono, Menlo, monospace"` (NSH resolves the first installed family and cascades the rest; `monospace` is the system monospaced font).
  - When the preference `Ligatures` is on: `"Fira Code, SF Mono, Menlo, monospace"`. Fira Code (SIL OFL, downloaded from its upstream release by the app's build, not taken from CodeRunner) is registered with `Root::font(bytes)`. CoreText applies Fira Code's `calt` ligatures by default; there is no per-feature control in NSH (open question in 12.3).
  - A font chosen with "Choose Font…" replaces the stack's head: `"<Family>, SF Mono, Menlo, monospace"`.
  - Size: `editor-font-size` 13 (preference), console `editor-font-size` − 1 (12), line height `size × editor-line-height-ratio` (1.45; this is Line Spacing 1.2 times the font's natural 1.2).
- **Scale** (a step, never a literal size; `type-*` and `leading-*` tokens):

| Step | Size / line | Weight | Color | Use |
|---|---|---|---|---|
| `caption` | 12 / 16 | regular, medium for badges | `muted-foreground` | Status bar, badges, helper text under fields, kbd, match counts, secondary row text |
| `body` | 13 / 18 | regular; medium for buttons, tabs, selected tab, section labels | `foreground` | All interface text: rows, labels, buttons, menus, fields |
| `subhead` | 15 / 20 | semibold | `foreground` | Sheet and prompt titles, preference pane headings, QuickNav query |
| `title` | 17 / 24 | semibold | `foreground` | Empty-state headings, About name |
| `display` | 22 / 28 | semibold | `foreground` | Welcome heading only |

- Two weights per screen (regular + medium) plus semibold for headings only. Bold is not used in chrome. Nothing below 12pt.
- Numbers that change or align (line and column, counts, CPU, memory, time, line numbers, steppers, table numbers) use `tabular_nums()` and align to the end.
- Sentence case everywhere: `Reset layout`, `New file`. Exceptions: menu titles are title case as in the menu bar (`New from Template`), proper nouns, acronyms and file names. No ALL CAPS except the `.tmTheme` scope and key glyph labels. Labels and buttons take no final period; complete sentences do. One ellipsis character `…` for commands that need more input.
- Text is truncated with `text_overflow(Ellipsis)` (middle for paths and file names, end elsewhere) and always has a tooltip when truncated. Text follows the user's text size: heights are `min_h` of the token, not fixed.

## 5. Spacing, radius, elevation, hit targets

### 5.1 Spacing

The 4pt Tailwind step (`p-1` = 4). Relationships:

| Relationship | Step | Points | Examples |
|---|---|---|---|
| Optical correction | `0.5` | 2 | Capsule inner inset, icon baseline |
| Parts of a control | `1`, `1.5` | 4, 6 | Icon and label (xs, sm: 4; md: 6; lg: 8), menu item icon slot gap |
| Related controls | `2` | 8 | Toolbar capsule gap, button row gap, find bar controls, sheet footer buttons |
| One content group | `3` | 12 | Row text inset (`inset-content`), form label to control, console text inset, toast padding |
| Groups in a section | `4` | 16 | Sheet body gap, preference group gap, panel padding in docs |
| Sheet padding | `5` | 20 | Sheet, prompt and alert-like dialog outer padding |
| Separate sections | `6` | 24 | Preference sections, empty-state blocks |
| Region breathing room | `8` | 32 | Empty-state vertical spacing |

Named insets (used instead of raw numbers): `inset-chrome` 8 (toolbar, bars, headers: the left edge of every capsule and bar item), `inset-content` 12 (text in rows, console, empty states, form groups), `inset-sheet` 20.

### 5.2 Alignment spines (global)

1. **Chrome spine, x = 8** from each region edge: toolbar capsules, tab bar items, sidebar header items, sidebar footer items, console toolbar items, docs header items, status bar items.
2. **Content spine, x = 12**: tree row selection fill starts at 8 (row inset) and text of a level-0 row starts at 8 + 8 padding + 16 disclosure slot + 16 icon slot + 6 gap; the editor's gutter has 8 left padding; console text starts at 12.
3. **Bands share a center line:** every bar centers its items vertically (no per-item offsets); icon slots are fixed squares so labels never move when icons differ.
4. **Hairlines are drawn once by the owner:** toolbar owns the line under it, the tab bar owns the line under the tab bar, the sidebar owns its right edge, the docs panel its left edge, the status bar its top edge, the console toolbar its bottom edge, the split handle owns the console's top edge (when the console is bottom) or its left edge (right). Neighbors never also draw it.
5. **Scrollbars** sit at the trailing edge of the scrolling region (native overlay scrollers); pad content, not the scroll view.
6. **Return to the spine:** nested content (tree levels, form children) indents by exact steps (16 per tree level; 20 per dependent form child) and returns exactly.

### 5.3 Radius

Tokens: `radius-xs` 4, `radius-sm` 6, `radius` 8, `radius-lg` 12, `radius-xl` 16, `radius-full` (capsules, switches, search fields, spinners, dots).

| Radius | Surfaces |
|---|---|
| `radius-xs` 4 | Checkbox, kbd, scroller thumb, inline code chip, tag |
| `radius-sm` 6 | xs/sm rectangular buttons, icon buttons inside rows, tab item, tooltips, annotation pills, code chips |
| `radius` 8 | Text fields, popup (field variant), rows (sidebar rows, menu items), segmented *inner* pill when the group is not a capsule, color wells, code editors in forms |
| `radius-lg` 12 | Menus, popovers, completion panel, toast, cards and grouped form sections, banners |
| `radius-xl` 16 | Sheets, QuickNav, welcome card, custom dialogs |
| `radius-full` | Toolbar capsule groups and their members, segmented controls, md/lg buttons, search fields, switches, count badges, spinners |

Concentricity: inner = outer − gap, to the nearest token: a menu (12) with 4 padding holds items of 8; a sheet (16) with a 12-inset card holds radius 12; a capsule group (28 high, full) with a 2pt inset holds members (24 high, full). A radius governs the whole surface: children are clipped (`overflow_hidden`) or carry their own radius; the focus ring is drawn outside, so clipped ancestors leave 3pt of room.

### 5.4 Elevation

Only floating things have shadows; they all use the `shadow` token as the shadow color (`.shadow_color(token)`) and NSH's presets.

| Level | Preset | Used by |
|---|---|---|
| E0 flat | none | All chrome regions, editor, console, bars, tree, cards in Preferences (hairline only) |
| E1 | `shadow_xs` + 1pt `capsule-border` or `border` | Selected tab, selected segment pill, capsule groups (light appearance only), color wells |
| E2 | `shadow_md` + 1pt `border` | Menus, popovers, tooltips, completion panel, symbol popover, hover popover |
| E3 | `shadow_lg` + 1pt `border` | Sheets, QuickNav, toasts, Help/About overlays |

Scrim: `scrim` behind sheets and QuickNav (NSH `backdrop_style`). Menus and popovers have no scrim.

### 5.5 Hit targets

- Pointer targets are never smaller than `ctl-xs` 20 x 20; icon-only buttons of 20 get `hit_slop` 4. Rows are full-width targets.
- Resize handles: 9pt hit area (`handle-hit`), 1pt drawn line, centered on the boundary; cursor `col-resize` / `row-resize`.
- Disclosure chevrons in trees have a 16 x 16 hit area inside the 24pt row; double-click and Return still open rows.
- Close buttons on tabs: 16 x 16 drawn, 20 x 20 hit.
- Text fields hit their whole box; the clear button of a search field is a 16 x 16 hit area at the trailing edge.
- Window drag: title band and toolbar background (`window_drag(true)`), all controls `window_drag(false)`.


## 6. Icons

### 6.1 Strategy

- **One own family:** `assets/icons/<name>.svg`, 85 files authored for this app (no SF Symbols, no CodeRunner artwork). 16 x 16 `viewBox`, `fill="none"`, `stroke="currentColor"`, `stroke-width="1.5"`, round caps and joins; solid glyphs (play, stop, pause, record, breakpoint, dot) use `fill="currentColor"`. Optical padding: artwork stays inside a 2pt margin (1.5 at most for the pointed shapes). Name = file name, kebab-case, a noun or noun-verb (`folder-plus`, `debug-step-over`).
- **Subset NSH draws:** `svg()` replays the document as vector canvas operations: paths, basic shapes, fills, strokes, caps, joins, `fill-rule`, opacity. It does not draw masks, filters, patterns, `<text>` or `<image>`, and a gradient stop of `currentColor` is black. The set uses only `path`, `circle`, `rect`, `g`, no gradients, no masks, no text. All 85 files are accepted by `nsh_core::svg::drawing` (the element's own code path). The `svg` cargo feature of `nsh-platform-macos` must be enabled.
- **Template tinting:** `currentColor` is the element's text color (`text_color(...)`), so an icon follows the theme and the control's state. Menu icons (native `NSMenu` items) take the menu's text color. Never bake a color into a file.
- **Size:** drawn 1:1 at 16 (`icon-md`), or at a step of the scale: 12, 14, 16, 20, 24, 32. The 1.5pt stroke scales with the icon; at 12 and 14 it is acceptable, at 24 and 32 (preferences strip, empty states) it reads as a light line, which is intended. Icons sit in a fixed square slot (the control's icon size) and are centered by the slot, never offset.
- **One component:** `ui::Icon::new(IconName::Run).size(IconSize::Md).tone(Tone::Muted)`. `IconName` is an enum generated from the directory listing (a build-time check fails if a file and a variant disagree). Assets are loaded with `asset!("icons/run.svg")`; nothing loads an icon by string at run time.
- **State by tone, never by swapping files:** `Tone::{Default (foreground), Muted (muted-foreground), Primary, Success, Warning, Danger, Info, OnPrimary, OnSelection, Disabled}`. A glyph that changes meaning (Run to Stop, eye to eye-off, chevron-right to down) changes by swap motion (section 9) or rotation.
- **Alias rule:** one file may serve several roles; the table lists the aliases (for example the Web Inspector "docked to the bottom" segment uses `console-bottom`).

### 6.2 Glyph tiles (no per-language SVGs)

Languages, symbol kinds, stack frames and variables do not get files. They use `ui::GlyphTile`: a 16 x 16 rounded square (`radius-xs` 4), fill = the tile color at 16% opacity, 1 or 2 letters in the tile color, `type-caption` medium, centered, `tabular_nums` off.

| Tile | Letter source | Color |
|---|---|---|
| Language (tab icon, file tree, QuickNav, language list, run-status) | `short` field of the language definition (`Py`, `Rs`, `C`, `C++`, `JS`, `Sh`, `Go`...); at most 2 characters (3 for `C++`, `C#` at size 16 use the 12pt caption) | `glyph-N` where N = stable hash of the language id mod 10 plus 1; the same language is always the same color, in both appearances |
| Symbol kind (status bar symbol popover, completion list, QuickNav symbols) | `f` function, `M` method, `C` class, `S` struct, `E` enum, `P` protocol/interface, `T` type alias, `V` variable, `K` constant, `N` namespace/module, `#` section/pragma mark, `A` attribute/property | function and method `glyph-1`, class `glyph-4`, struct `glyph-2`, enum `glyph-7`, protocol `glyph-8`, type `glyph-9`, variable `glyph-3`, constant `glyph-6`, namespace `glyph-10`, section `glyph-10`, property `glyph-5` |
| Stack frame | `{}` rendered with the `braces` icon at 12 inside the tile | `glyph-1` |
| Variable | `V` | `glyph-3` (green, as in CodeRunner) |

A tile never carries meaning by color alone: the letter is always there. File type for non-language files uses the icons `file-text`, `file-binary`, `file-image`, `file` (tone Muted); folders `folder` / `folder-open` (tone `Muted`; `Primary` for the navigator root and for the folder that contains the current document).

### 6.3 The icon list

Every icon the app uses. "Where" lists the main uses. Sizes: tb = toolbar 16, bar = bars 12 to 14, row = rows 14, pref = preferences strip 24, empty = empty state 32.

**Transport and toolbar**

| Name | Where |
|---|---|
| `run` | Toolbar Run; tab context menu; Run status; Continue in debugger menu text only |
| `stop` | Toolbar Stop; console Stop; search progress Stop |
| `wrench` | Toolbar Run Settings (tone `Primary` when the file has custom run settings) |
| `bug` | Run hold-menu "Run and Debug" (menu text has no icon; used in the debug start banner and the Language Debugger popup row) |
| `sidebar-left` | Toolbar view toggle "Navigator" |
| `console-bottom` | Toolbar view toggle "Console" (position bottom); Web Inspector docked to the bottom |
| `console-right` | Toolbar view toggle "Console" when the console position is Right |
| `sidebar-right` | Toolbar view toggle "Documentation"; Web Inspector docked to the side |
| `web-inspector-window` | Web Inspector in separate window; "Open in separate window" actions |

**Chevrons and arrows**

| Name | Where |
|---|---|
| `chevron-left`, `chevron-right` | Back / Forward (toolbar, sidebar root history, docs history, console web history); submenu arrow; tree disclosure (right, rotated 90° when expanded) |
| `chevron-up`, `chevron-down` | Find previous / next; stepper arrows; tab overflow; console Hide; popover disclosure |
| `chevrons-up-down` | Popup and select trailing indicator (language popup, filters, encodings, status bar popups) |
| `chevrons-left`, `chevrons-right` | Docs panel expand grabber (left chevrons to widen, right to restore) |
| `arrow-up`, `arrow-down` | Sort direction in tables; Jump to previous / next diagnostic buttons |
| `arrow-up-right` | Open in Browser; external link marker; "Open in New Window" |

**Basic actions**

| Name | Where |
|---|---|
| `plus` | New tab; add (+) in list managers; New File |
| `minus` | Remove (-) in list managers; checkbox indeterminate; zoom reset |
| `close` | Tab close; popover and banner dismiss; sheet cancel glyph |
| `close-circle` | Clear button in search fields and text fields (solid, tone Muted) |
| `check` | Checkbox mark; menu check; success marker in lists |
| `search` | Search fields; toolbar-less sidebar search toggle; Find on Page |
| `filter` | Console filter field prefix |
| `trash` | Delete in the Key Bindings and Docs tables when a row action is needed (list managers themselves use `minus`); file deletions use native menu text |
| `eraser` | Clear Console (console toolbar button, tooltip `Clear the Console`) |
| `copy` | Copy path/URL; Copy to Clipboard buttons |
| `clipboard` | New from Clipboard (sidebar New File menu header) |
| `refresh` | Refresh navigator, Refresh PATH, Reload Page |
| `ellipsis` | Overflow menus (console toolbar, docs actions when collapsed) |
| `ellipsis-circle` | Pull-down "more actions" button of list managers (Languages, Themes, Key Bindings) |
| `pencil` | Rename, Edit Script, Edit CSS/JS buttons, Edit Value |
| `lock` | Locked/read-only document marker in the tab and status bar |
| `drag-handle` | Reorder handle in sortable rows (Docs entries, shell variables) |

**Status**

| Name | Where |
|---|---|
| `help-circle` | Help `?` buttons in Preferences and Run Settings |
| `info-circle` | Notice severity icon; Print Description row action; info banner |
| `warning-triangle` | Warning severity icon; warning banner; Run Issues |
| `error-circle` | Error severity icon; Compile Error status; error banner |
| `success-circle` | Run Succeeded status; success banner |
| `pause-circle` | Status "Paused" (debugger) |
| `stop-circle` | Status "Run Stopped" / "Compile Stopped" |
| `pause` | Debugger Pause Execution button |
| `dot` | Edited marker on a tab; radio indicator; separators in status text |
| `spinner` | Running tab, Run status, loading rows, QuickNav progress (rotates, 9.2) |
| `wifi-off` | Offline and network-error states |

**Debugger**

| Name | Where |
|---|---|
| `debug-continue` | Continue Execution (when paused) |
| `debug-step-over`, `debug-step-in`, `debug-step-out` | Debugger toolbar |
| `breakpoint` | Gutter breakpoint (tone `breakpoint`, ghost, disabled) |
| `debug-arrow` | Gutter execution arrow (tone `debug-arrow`) |
| `debug-stack` | Console debugger segment "Hide / Show Stack and Variables" |
| `braces` | Variables glyph; Languages pane icon source; stack-frame tile |
| `terminal` | Console Terminal Mode segment; Debugger console segment; Shell pane icon |

**Docs, web and files**

| Name | Where |
|---|---|
| `eye`, `eye-off` | Automatic Lookups toggle; Live Updating toggle; Show Values on Hover |
| `link` | Copy Current URL; link rows |
| `bookmark` | Bookmark This Page; Bookmark-type docs entries |
| `book-open` | Documentation empty state; Help window; docs source popup |
| `globe` | Console Web Mode segment; Website source type |
| `zoom-in`, `zoom-out` | Docs zoom actions |
| `folder`, `folder-open` | File tree, path popup, QuickNav folders |
| `folder-plus` | Sidebar footer New Folder |
| `go-to-folder` | Sidebar footer Go to Folder |
| `file`, `file-plus` | Plain file; Sidebar footer New File; empty state New File |
| `file-text`, `file-code`, `file-binary`, `file-image` | Non-language file glyphs in the tree and QuickNav (`file-code` = a source file whose language has no tile) |

**Editing, keys, themes**

| Name | Where |
|---|---|
| `keyboard` | Shortcut recorder cell empty state ("Click to set a keyboard shortcut") |
| `record` | Shortcut recorder while recording (tone `Danger`, pulsing) |
| `command` | Key Bindings pane icon; Key glyph help |
| `gear` | Docs actions button; Advanced pane icon |
| `sun`, `moon` | Light / dark marker in the theme list |

**Preferences strip (24)**

| Name | Pane |
|---|---|
| `pref-general` | General |
| `pref-text` | Text Editing |
| `pref-languages` | Languages |
| `pref-themes` | Themes |
| `command` (alias `pref-keys`) | Key Bindings |
| `terminal` (alias `pref-shell`) | Shell |
| `gear` (alias `pref-advanced`) | Advanced |

**App**

| Name | Where |
|---|---|
| `app-mark` | About window (64pt in a `primary` rounded square, `primary-foreground` glyph), Help window header |

Files, alphabetical (85): `app-mark` `arrow-down` `arrow-up` `arrow-up-right` `book-open` `bookmark` `braces` `breakpoint` `bug` `check` `chevron-down` `chevron-left` `chevron-right` `chevron-up` `chevrons-left` `chevrons-right` `chevrons-up-down` `clipboard` `close` `close-circle` `command` `console-bottom` `console-right` `copy` `debug-arrow` `debug-continue` `debug-stack` `debug-step-in` `debug-step-out` `debug-step-over` `dot` `drag-handle` `ellipsis` `ellipsis-circle` `eraser` `error-circle` `eye` `eye-off` `file` `file-binary` `file-code` `file-image` `file-plus` `file-text` `filter` `folder` `folder-open` `folder-plus` `gear` `globe` `go-to-folder` `help-circle` `info-circle` `keyboard` `link` `lock` `minus` `moon` `pause` `pause-circle` `pencil` `plus` `pref-general` `pref-languages` `pref-text` `pref-themes` `record` `refresh` `run` `search` `sidebar-left` `sidebar-right` `spinner` `stop` `stop-circle` `success-circle` `sun` `terminal` `trash` `warning-triangle` `web-inspector-window` `wifi-off` `wrench` `zoom-in` `zoom-out`. A build-time check lists the directory and fails if an `IconName` variant has no file or a file has no variant; no other icon may be referenced.

## 7. Components (the app's `ui` layer)

All features compose these; none imports a control from `nsh_base`, writes a color, radius, padding or font size, or builds a control from a pressable view. A look a feature needs becomes a variant or size here. `ui/` layout: `tokens.rs` (reads `theme.json`), `typography.rs`, `motion.rs`, `icon.rs`, one file per component or family, `gallery.rs` (7.30). Each component takes `size`, `variant`, states (`selected`, `disabled`, `loading`, `invalid`, `open`), slots (label, icon, trailing) and events; layout (width, flex, margin) comes from the caller. States follow table 3.6 unless a row says otherwise. Every pressable has hover, pressed and focus-visible; a nonpressable never reacts to hover.

Common focus rule: `focus_visible` draws a 2pt `ring` outline with 1pt offset outside the control, so ancestors must leave 3pt and must not `overflow_hidden` an unclipped ring (capsule groups clip their members' fills, not their rings: the ring is drawn on the member's outer silhouette).

### 7.1 Button

NSH base: `button()`. Roles: `Button`.

| Variant | Fill (rest / hover / pressed) | Text | Edge | Use |
|---|---|---|---|---|
| `primary` | `primary` / `primary-hover` / `primary-active` | `primary-foreground`, medium | none | The one default commit of a sheet or prompt (Return): `Close` in Run Settings, `Go`, `Save`, `Delete` is *destructive*, not primary |
| `default` | `control` / `control-hover` / `control-pressed` | `foreground`, medium | 1pt `input` | Cancel and ordinary visible actions (`Edit Script`, `Options…`, `Choose Font…`) |
| `secondary` | `secondary` / `-hover` / `-active` | `secondary-foreground`, medium | none | Quiet filled action inside cards (Revert, Make Default strip) |
| `ghost` | transparent / `accent` / `accent` at 2x | `foreground` | none | Toolbar-less actions in bars and rows (xs, sm) |
| `destructive` | `destructive` / `-hover` / `-active` | `destructive-foreground`, medium | none | The destructive commit of a custom dialog (`Delete`, `Discard Changes`) |
| `link` | transparent | `link`, underline on hover | none | External URLs only (docs, website, attribution). Pointing-hand cursor. Never an in-app command |

Sizes per table 2.1; md/lg are capsules, xs/sm are `radius-sm`. Label = sentence case verb or `Verb object`, one or two words, ellipsis if it opens something that asks more. Icon slot optional leading (`icon-sm` at xs/sm, 16 at md/lg), 4/6/8 gap. Minimum width: md 64, lg 80. Pressed also scales the whole button to 0.97 (`motion-fast`, settle ease). Loading: the leading slot becomes a 12/14/16 spinner, label stays, button disabled for activation, 70% opacity. Disabled: 45% opacity. Primary and destructive: at most one per surface.

### 7.2 IconButton

NSH base: `button()` with `accessibility_label` (required). Roles: `Button` (or `Toggle` semantics via `selected`).

| Variant | Rest | Hover | Pressed | Toggled on |
|---|---|---|---|---|
| `ghost` (default) | transparent, icon `muted-foreground` | `accent`, icon `foreground` | `accent` at 2x | `capsule-selected` wash, icon `primary` |
| `capsule` (member of a CapsuleGroup) | transparent, icon `foreground` | `capsule-hover` | `capsule-pressed` | `capsule-selected`, icon `primary` |
| `filled` | `secondary` | `secondary-hover` | `secondary-active` | `capsule-selected`, icon `primary` |

Square at the control height (20/24/28/32), `radius-sm` (ghost, filled) or `radius-full` (capsule). Icon size from table 2.1. Press scale 0.95. Tooltip = the accessible label plus the shortcut as a `Kbd` where one exists (500 ms delay). A toggle's accessible state is `selected`/pressed, announced by VoiceOver. Disabled icon buttons keep their tooltip (it says why when the reason is not obvious).

### 7.3 CapsuleGroup and Segmented

NSH base: `toolbar()` + `toolbar_group()` for groups of buttons (roving focus: one Tab stop, arrow keys inside); `toggle_group()` / `tabs()` for exclusive choices. Roles: `Toolbar`/`Group` for groups, `Radiogroup` or `Tablist` for exclusive segmented controls.

- **Silhouette:** one capsule, `ctl-md` 28 high, `radius-full`, fill `capsule`, 1pt `capsule-border`, E1 shadow in light. Members are `capsule` IconButtons or PopupButtons sized `md` minus a 2pt inset (24 high) laid out edge to edge with a 1pt vertical divider (`capsule-border`, 16 high) between members. Only the outer ends are round (the capsule), middle members are rectangular hit areas with a `radius-full` wash inside the inset, so hover never leaks past the capsule.
- **Widths measured from CodeRunner's pills** (macOS 26): Run group 108 (3 x 36); Back/Forward 57 (2 x 28 + 1 divider); View toggles 122 (3 x 40 + 2 dividers); Language popup 200 (flex 120 to 240).
- **Exclusive Segmented** (Terminal/Web mode, Docs source type, Find/Replace mode, Text Editing categories, Languages sub-tabs, Key Bindings kind): same capsule; one raised pill (`tab-active`, E1, `radius-full`) slides to the selected segment (9.2). Segments equal width or content-sized with 12 padding; label `type-body` in `foreground`, medium when selected, regular otherwise; icon-only segments are 36 wide at md. Keyboard: Left/Right (and Up/Down) move and select (roving, selection follows focus); Home/End; Tab leaves the group.
- **Hover of one member** shows the others at half wash (composite rule).
- **Disabled member:** 45% opacity, skipped by arrows.

### 7.4 PopupButton (Select)

NSH base: native menu via `popup_menu_at` for ordinary lists (system look, checkmarks, submenus, separators, headers; the trigger stays in the *open* state until the menu closes); `select()` with `searchable` and `cover` for long lists (Syntax Mode 270 items, Text Encoding, fonts, theme pickers with more than 20 entries). Roles: `Button` with `haspopup` (native) or `Combobox`.

| Variant | Look | Use |
|---|---|---|
| `field` (default) | `control` fill, 1pt `input` outline, `radius` 8, label left (body), `chevrons-up-down` 12 `muted-foreground` right | Forms, preferences, sheets |
| `capsule` | capsule member look (7.3), label left, chevrons right | Toolbar: language popup |
| `bar` | ghost, no outline, `radius-sm`, caption text, chevrons-up-down 10 | Status bar items (indentation, symbol), console filter, docs TOC |

Sizes xs/sm/md/lg. Min width 80. Label truncates (end), tooltip shows full text. Open: `control-pressed` (field), `capsule-pressed` (capsule), `accent` at 2x (bar). Value shown is always the current choice (`Syntax: Rust`, `All Output`, `Tabs: 4`). An empty/unset state shows the placeholder in `muted-foreground`. Keyboard: Return/Space/Down opens; type-select in native menus; Esc closes.

### 7.5 SearchField

NSH base: `input()` (native text input, IME safe). Role `Search`.

Capsule (`radius-full`), `field` fill, 1pt `input` outline; leading `search` (or `filter` in the console filter) 12 `muted-foreground`; trailing slot: result count caption (find, search), then `close-circle` 16 (visible only when text is present); placeholder `muted-foreground`. Sizes xs (sidebar footer, 20), sm (find bar, 24), md (docs header, 28), lg (QuickNav, 32 with `subhead` text, no outline). Focus: the outline becomes 2pt `ring` (outside). Esc clears when it has text, else leaves the field. Return = submit where the surface defines it. Invalid regex: outline `destructive`, trailing `Regex Invalid` caption in `destructive`; no matches: `No matches` caption.

### 7.6 TextField and TextArea

NSH base: `input()` (single or `multiline`). `field` fill, 1pt `input` outline, `radius` 8, horizontal padding 8, height per size (md default, lg in prompts); placeholder `muted-foreground`; code-like fields (paths, scope selectors, flags) use `CODE_FONT_STACK` at 12. Focus: 2pt `ring` outside, outline stays. Invalid: `destructive` outline plus caption below in `destructive`, shake on submit. Read-only: `muted` fill, selectable text. Disabled: 45%. `TextArea`: min 3 rows (80 high for Program Input), vertical scroll inside, same chrome. `CodeField` = a small `Editor` (min 2, max 8 rows, line numbers optional, `radius` 8, `editor-bg` fill, 1pt `input` outline, gutter 8 padding) for Run Command, Template, regex, custom PATH, JSON compound command.

### 7.7 Checkbox and Radio

NSH base: `checkbox()`, `radio_group()` + `radio()`. Roles `Checkbox`, `Radio`/`Radiogroup`.

Indicator 16 x 16, `radius-xs` 4 (radio: full), 1pt `input` outline on `control` fill. Checked: fill `primary`, `check` 12 `primary-foreground`. Indeterminate: `primary` fill, `minus` 12. Pressed: `control-pressed` / `primary-active`. Focus ring outside. Label `type-body` 8 to the right; label is part of the hit target. Disabled: 45%. Dependent children (Preferences): when the parent is off, children are `disabled` and indented 20. Radio checked: `primary` fill with a 6pt `primary-foreground` dot.

### 7.8 Switch

NSH base: `switch()`. Role `Switch`. Track 36 x 20 `radius-full`, `input` fill off / `primary` on, thumb 16 `background` with E1 shadow, translate 16 on, `transition_colors` + `transition_transform` `motion-fast`. Reserved for settings in popovers that apply immediately; Preferences uses checkboxes (CodeRunner's convention). Focus ring outside the track.

### 7.9 Stepper and NumberField

NSH base: `number_input()` (`controls_right`). Role `Spinbutton`. A TextField (width 56, text right-aligned, `tabular_nums`) with a 16-wide two-button stepper (`chevron-up` / `chevron-down` 10, `radius-xs`) attached at the trailing edge inside the field's outline; Up/Down arrow keys step, Shift-arrow steps by 10x. `min`, `max`, `step` from the setting (Line Spacing step 0.05, wrapped indent step 0.5). Suffix/prefix text in `muted-foreground`. Inline `%@` use: the field sits in a sentence baseline-aligned with its words.

### 7.10 ColorWell

NSH base: `color_picker()` in a `popover()`. Two sizes: `well` 44 x 28 with caption below (the six global theme colors) and `chip` 24 x 16 (element table FG/BG cells). A swatch with 1pt `input` outline, `radius` 8 / `radius-xs`, E1; transparent colors show a checkerboard-free diagonal slash in `destructive` for "unset". Click opens a Popover (E2, 240 wide): color area 216 x 140, hue slider, alpha slider, hex `TextField` (`#RRGGBB` / `#RRGGBBAA`), eye dropper button (`color_eye_dropper`). Role `Button` labeled `Foreground color, #C5CAD3`.

### 7.11 ListRow (tables and lists)

NSH base: `virtual_list()` / `data_table()` / `tree()`. Roles `List`/`Row`/`Table`/`Row`/`Cell`/`ColumnHeader`.

Height `row-sm` 24 (lists), `row-md` 28 (tables), `row-xl` 44 (QuickNav, search matches). Row selection fill spans the row inset by 8 horizontally (so it floats inside the list), `radius` 8 for lists, square for tables (tables are bordered cards, 1pt `border`, `radius` 8, rows clip). States: hover `accent`; selected focused `selection` + `selection-foreground`; selected unfocused `selection-inactive`; keyboard-focused-but-unselected row: 2pt `ring` inset. Tables: header 24 high, `type-caption` medium `muted-foreground`, 1pt bottom hairline, sortable headers show `arrow-up`/`arrow-down` 10 and are buttons; column resize handles 9pt; stripe off. Cell padding 8. Numbers end-aligned with `tabular_nums`. Row context menu native. Empty list: `EmptyState` compact.

### 7.12 SidebarRow (file tree)

NSH base: `tree()` with `render_item`. Roles `Tree`/`Treeitem` (expanded state exposed).

Row 24; level indent 16 per level; slots: disclosure 16 (chevron-right 12, rotates 90° when expanded; hidden for files but the slot is kept), icon 16 (`folder`, `folder-open`, GlyphTile language, `file-*`), gap 6, label `body` (truncate middle), trailing slot 16 for status (lock, spinner, error). Fill states: hover `accent`; selected `sidebar-accent` with `sidebar-accent-foreground`; the row of the *current document* gets a medium-weight label and a `primary` icon tone and is not filled; dragging-over (drop target) 2pt `primary` outline on the folder row. Inline rename: the label becomes a TextField (sm, in place, text preselected without extension). Filtering highlights the matched characters with `match-highlight`.

### 7.13 TabItem

NSH base: `tabs()` + `tab()` + a close `button()`. Roles `Tablist`/`Tab` (selected exposed).

Item `ctl-sm` 24 high in the 32pt bar (4pt inset), `radius-sm` 6, equal width: `clamp(96, available / n, 220)`; padding x 8; slots: icon 14 (GlyphTile 16 for the language, or spinner 12 while running), label `body` medium selected / regular unselected (truncate middle), trailing 16 slot: `dot` (edited) that becomes `close` on hover or when selected; selected: fill `tab-active`, E1, `tab-active-foreground`; unselected: transparent, `tab-foreground`, hover `tab-hover`. Pressed deepens the hover wash. A drag lifts the tab (E1, 0.97 scale not applied, opacity 0.95) and others slide to make room (9.3). Overflow: when items would be narrower than 96 the bar scrolls horizontally and shows an overflow `chevron-down` button (28) at the trailing end next to `plus` (28). Context menu: `Run` / `Stop` only.

### 7.14 StatusItem

NSH base: `button()` / `text()`. Heights 24 bar, hit 20. Variants: `label` (static text, caption `editor-bar-foreground`), `button` (ghost on hover, `radius-sm`, optional leading icon 12), `popup` (a `bar` PopupButton), `status` (leading icon 12 or spinner + text, color by tone; clickable). Items are separated by 12 (`inset-content`), no vertical dividers. The status item for the run state uses Tone `Success/Danger/Warning/Muted`; the icon and text always accompany the color. Truncation: the right group (symbol, indentation, position) never shrinks below its content; the left group truncates in order: `showConsoleItem`, memory/CPU, time, status text.

### 7.15 Kbd

Not an NSH component: a styled `view()`: height 18, min width 18, padding x 5, `radius-xs`, 1pt `border`, fill `muted`, text caption medium `muted-foreground`, content from `key_label("mod+shift+o")` (glyphs `⇧⌘O`) or a word (`Return`, `Esc`). In tooltips and menus the shortcut is plain text in `muted-foreground` (native menus draw their own). Role `Text` with the spoken form as label (`Shift Command O`).

### 7.16 Badge

A styled `view()`: height 16, min width 16, padding x 5, caption medium. Variants: `neutral` (`muted` fill, `muted-foreground`, default), `count` (capsule, `muted`), `primary`, `success`, `warning`, `danger` (their `*-muted` fill with the status color text, with an icon when color carries meaning). Use sparingly: `Default` marker in the Templates table, match counts, `N items` in variables, issue counts (`2 Errors`). Never a row of colored badges.

### 7.17 SectionHeader

Variants: `form` (type-body semibold, 12 above, 8 below, followed by the group; separated from the previous group by a 1pt `border`), `list` (24 high band, caption medium `muted-foreground`, trailing actions as xs ghost IconButtons: debug panes `Stack`, `Variables`; file groups), `pane` (subhead semibold; preference pane headings and sheet titles). Role `Header` (screen-reader landmark).

### 7.18 FormRow

Grid: label column 200 right-aligned (`body`, `foreground`; a 16pt gap), control column flex, row min height 28, row gap 8, group gap 16, caption under a control (`type-caption`, `muted-foreground`, max width 340, 4 below). Boolean rows have an empty label column and the checkbox with its label in the control column (so all controls share one spine). Dependent children indent 20 and disable with the parent. A row with `%@` inline controls reads as a sentence on one baseline (label, number field, label). Label is associated with its control (`accessibility_label`); help `?` is a ghost IconButton (`help-circle`) 20 at the row end that opens the Help window at an anchor.

### 7.19 Sheet and Dialog

NSH base: `dialog()` (modal layer, `presentation`), `alert_dialog()` for native alerts (<= 2 buttons in a row, no accessory). Role: `Dialog` (modal), labelled by its title.

- **Sheet** (Run Settings, Go to Line, Prompt, Edit File Type Associations, Ignored/Auto-fix lists, Docs configuration): fill `background`, `radius-xl` 16, 1pt `border`, E3 shadow, `scrim` behind, anchored top-center 12pt under the toolbar band's top edge (mimicking a macOS sheet), width by content (Run Settings 500, prompts 420, lists 560), padding `inset-sheet` 20, title `subhead` semibold, optional caption `muted-foreground`, body gap 12, footer right-aligned gap 8 (Cancel `default` lg, commit `primary` lg), 16 above the footer. Return = default commit, Esc = Cancel.
- **Alert** (native `NSAlert` through `alert_dialog()`): used for confirmations; title = the condition or decision, body = new information only, buttons `Cancel` plus the specific result (7.1 rules). Destructive result uses the `destructive` role. Alerts with an accessory (checkbox, text field) are Sheets instead (G14).
- A modal Preferences-style *window* is a real window (`open_window`), not a Sheet.

### 7.20 Popover

NSH base: `popover()`. Surface: `popover` fill, `radius-lg` 12, 1pt `border`, E2, padding 12, offset 6 from the trigger, flips to stay in the window, no arrow. Closes on Esc and outside press; returns focus to the trigger. Roles `Dialog` (non-modal) labelled by content. Used for: ColorWell, search Options, diagnostics hover, variable hover, symbol navigator, key trigger editor, Docs bookmark.

### 7.21 Menu and MenuItem (in-window fallback)

Real menus (menu bar, context menus, popup buttons) are native (`set_application_menu`, `popup_menu_at`). `ui::Menu` exists for lists that need non-native content and for `select()` popups: `popover` surface `radius-lg`, padding 4, min width 180; rows `row-sm` 24, padding x 8, `radius` 8, icon/check slot 16 + 6 gap, label body, shortcut right-aligned in `muted-foreground`, submenu `chevron-right` 12; separators 1pt `border` with 4 vertical margin; section headers caption `muted-foreground` 24 high; highlighted row `menu-highlight` + `menu-highlight-foreground` (native look); disabled 45%; destructive items `destructive` text (highlight keeps white). Roles `Menu`, `Menuitem` (checked exposed).

### 7.22 Tooltip

NSH base: element `tooltip(...)` (0.5 s delay, desktop). `popover` fill, `radius-sm` 6, 1pt `border`, E2, padding 6 x 4, caption text `foreground`, optional trailing Kbd (8 gap), max width 280, 1 to 2 lines. Reserved for icon-only controls, truncated text and hidden shortcuts; it never restates a visible label.

### 7.23 Toast and Banner

- **Toast** (NSH `toast_stack` + `ToastManager`): E3 `popover` surface, `radius-lg`, padding 12 x 10, max 360 wide, icon 16 (tone), body text, optional ghost sm action, bottom-center of the window, 16 above the status bar, auto-dismiss after 4 s (paused on hover, never while an action is focused), one at a time. Used for results not visible elsewhere (`Reloaded from disk`, `Imported theme "Monokai"`, `Copied`). No toast for a run result or a saved document.
- **Banner**: inline strip inside a region (not floating): min 32 high, fill the status `*-muted` token, 1pt hairline on the far side, icon 14 in the status tone, text `body`, actions as ghost sm Buttons (`Revert`, `Make Default`, `Show Error`), dismiss `close` 16 when dismissible. Role `Alert` for errors and warnings (announced), `Summary` otherwise.

### 7.24 ProgressSpinner and ProgressBar

- **Spinner**: the `spinner` icon at 12, 16 or 20, tone `muted-foreground` (or the context tone), rotating once per second (`MotionPreset::Spin`, linear, `play_ignoring_reduce_motion` because it is a progress cue). Role `Progressbar` (indeterminate), label `Running` or the context.
- **ProgressBar** (`progress()`): 2pt high (search, docs loading) or 4pt (sheet operations), `muted` track, `primary` indicator, `radius-full`; indeterminate = 30% wide segment sliding (`motion-slower` loop). Reduced motion: a static 40% segment with a text label.

### 7.25 SplitHandle

NSH base: `resize_handle()` inside `h_resizable()` / `v_resizable()` (`handle_class`, `handle_thumb_class`). Hit 9pt, drawn line 1pt `border`; hover: line `input`; dragging: line `ring`; cursor `col-resize`/`row-resize`; the element is `Adjustable` (arrow keys 8pt, Shift 32pt, Home/End to minimum/maximum). Double-click toggles collapse (restores the previous size). Collapse by dragging 40pt past the minimum. Sizes are persisted per window (`on_resize`) and clamped on restore.

### 7.26 Separator

1pt `border` line, `role=None`. Horizontal in lists and menus (4 margin), vertical in capsules (16 high).

### 7.27 EmptyState

Centered column in its region: icon (`icon-2xl` 32; 24 in narrow regions) `muted-foreground`, 12 gap, title `subhead` semibold, 4 gap, message `body` `muted-foreground` max 320 centered, 16 gap, 0 to 2 actions (`default` md, a `primary` only when a single default action exists). No card, no border. Role `Group` labelled by the title.

### 7.28 KeyRecorder

The `Keyboard Shortcut` cell of the Key Bindings table: shows the shortcut as `Kbd` glyphs or `keyboard` + `Click to set a keyboard shortcut` placeholder (`muted-foreground`); click arms it (outline `ring`, `record` icon pulsing `Danger`, text `Type shortcut`), the next chord is captured, Esc cancels, Delete clears. Role `Button`, label `Keyboard shortcut for <command>, <current>`.

### 7.29 ListManager

The master-detail list with the `+ - gear` strip below it (Languages, Themes, Key Bindings sets): list (7.11) in a 1pt bordered `radius` 8 box, below it a strip of three capsule-less `filled` IconButtons sm: `plus` (a menu for Themes: `New Light Theme`, `New Dark Theme`), `minus`, `ellipsis-circle` (pull-down menu of management actions). All three have accessible labels and tooltips (`Create a new language`, `Delete the selected theme`, `More theme actions…`).

### 7.30 Gallery

A hidden window (Window menu item `UI Gallery`, debug builds and `--gallery`) shows every component above in every variant, size and state, in light and dark side by side, plus the token swatch table. New components are reviewed there before features use them. It doubles as the target of `nsh test` screenshots.


## 8. Surfaces

Conventions in this section: measurements are in points; "token" names refer to section 3; component names to section 7; "copy" tables are the exact strings (sentence case as CodeRunner writes them, with `CodeRunner` replaced by `ScratchRunner`); `%lu`/`%@` are format placeholders. Wireframes of each screen and state are in `docs/design/screens.md`.

### 8.1 Main window

#### Anatomy

Root: `view().flex_col().bg(window)`, with `window_drag` on the bands. Transparent window (vibrancy), so every region paints its own fill.

```
y 0    Title band        32   toolbar fill, traffic lights, centered title            hairline: none
y 32   Toolbar band      40   toolbar fill, capsule groups                            hairline below: toolbar-border
y 72   Tab bar           32   tab-bar fill, tab items 24, plus, overflow              hairline below: border
y 104  Body             flex  h_resizable
         Sidebar    240 (180..400)  sidebar fill over Sidebar vibrancy; right edge hairline sidebar-border
           header 32 / list flex / footer 24
         Center     flex (>= 240)
           Editor stack (flex)   find bar (36 or 68, pushes) + editor with gutter
           Console   (bottom: 40% of the center, min 96; right: 40%, min 240)
           Status bar 24         editor-bar fill, top hairline editor-bar-border
         Docs       300 (240..600) background fill; left edge hairline border
           header 68 (search row 40 + sources row 28) / web page flex / footer 24
```

- The three 24pt bars (sidebar footer, editor status bar, docs footer) share one baseline and read as one strip; each draws its own top hairline.
- The console splits the center column only (the status bar stays below it, as in CodeRunner).
- The tab bar spans the whole window width .
- Defaults: sidebar visible (`SidebarVisible` true), console hidden until output arrives (Hide console automatically), docs hidden, tab bar visible (pref `TabBarHidden` false), toolbar visible.
- Persisted per window (and restored, clamped to the current screen): window frame, sidebar width, docs width, console size and position, panel visibility, tab bar and toolbar visibility, debugger pane widths.
- Window inactive: the title and toolbar icons dim to 60%, selections become `selection-inactive`/unfilled; the traffic lights are the system's.

#### Panel visibility behaviour

Show/hide of the navigator, console and docs from the toolbar toggles, View menu, shortcuts (⇧⌘H, ⇧⌘C, ⇧⌘I) and the chevron buttons; each is an animated layout change (9.3). Dragging a panel past its minimum collapses it; the toggle state follows. Console auto-show: when a run produces output while the console is hidden and "Hide console automatically" is on, the console opens and is marked *temporary*; it closes again at the next edit or run start if it was auto-shown (a console the user opened stays).

### 8.2 Title band

32pt, `toolbar` fill. Traffic lights are system (left, centered at y = 16; `controls_offset` set so they center on y = 16). Center: `[language tile 16] [document name] [ – Edited]`, 6 gap, `body` medium `foreground`; the suffix ` – Edited` is `muted-foreground`; the whole title is `muted-foreground` when the window is inactive. The name follows the selected tab (`Untitled.swift`, `permute.py`). Right edge: empty. Not interactive; the band is a drag region and double-click zooms (platform). Tooltip on the name: the full path.

### 8.3 Toolbar band

40pt, `toolbar` fill, 1pt `toolbar-border` below. Padding x 8, vertical center, capsules 28 high, gap 8. Order and measurements:

| Item | Component | Size | Content | Tooltip (+ shortcut) | States |
|---|---|---|---|---|---|
| Language | PopupButton `capsule` | 200 (flex 120..240) | `Syntax: <Language>` e.g. `Syntax: Python`; `Syntax: None` when unset; trailing `chevrons-up-down` | `Choose the language of this document` | Open = pressed; disabled never |
| Run group | CapsuleGroup, 3 IconButtons | 108 | `run`, `stop`, `wrench` (36 each) | `Run` ⌘R · `Stop` ⌘. · `Edit Run Settings…` ⇧⌘R (customized: `Custom run settings in effect. Click to edit…`, wrench tone `Primary`) | Run disabled while running or when the file has no run command; Stop disabled while idle; both dimmed 45% |
| spacer | | flex | | | |
| Back/Forward | CapsuleGroup, 2 IconButtons | 57 | `chevron-left`, `chevron-right` | `Go Back to Previous Caret Location` ⌃⌘← · `Go Forward to Next Caret Location` ⌃⌘→ (dynamic: `Go Back to Tab or Document "<name>"`, `Go Back to Search Results`) | Disabled when the history is empty in that direction |
| View toggles | CapsuleGroup, 3 toggle IconButtons | 122 | `sidebar-left`, `console-bottom` (or `console-right`), `sidebar-right` (40 each) | `Hide or show the Navigator` ⇧⌘H · `Hide or show the Console` ⇧⌘C · `Hide or show the Documentation View` ⇧⌘I | Toggled on = panel visible (`capsule-selected` wash, `primary` icon) |

- **Run button hold menu:** click runs; press and hold 400 ms, right-click, or Control-click opens a native menu at the button's bottom-left: `Run` (or `Run Again "<doc>"`), `Run and Debug`, `Run Without Debugging`, and `Compile` (languages with a compile script) or `Run Diagnostics`. A `chevron-down` 8 appears at the button's lower right on hover to advertise it. Items mirror the Run menu's enabled state and shortcuts.
- **Language popup menu** (native): `None` (checkmark when unset), the enabled languages (check on the current; grouped into submenus named by the part before ` (…)` when "Group similar languages in menus" is on, with the variants inside), a separator, `Syntax Mode` submenu (grammar list, check on current, then `Import Syntax File…`), a separator, `Edit Languages…` (⌥⌘,). ⌥⌘L (`Select Language…`) opens QuickNav in language mode for fuzzy search.
- Show Toolbar (⌥⌘T) hides the toolbar band (LayoutAnimation); the title band stays. `Customize Toolbar…` is not offered (no NSToolbar in NSH).
- Narrow window: only the language popup shrinks (8 rule 2 in section 2.2).

### 8.4 Sidebar

NSH: `h_resizable` panel, `macos_backdrop(Sidebar)` under a `sidebar`-filled view. Two modes switched in place (no tabs, a cross-fade): **File navigator** and **Search**. Min 180, default 240, max 400. Collapsible. Right edge hairline `sidebar-border` drawn by the sidebar.

#### 8.4.1 File navigator

```
header 32:  [path popup flex ▾] [<][>] (capsule xs group) [search]      padding x 8, gap 6
list:       tree rows 24, level indent 16, scrolls
footer 24:  [+] [folder+] [go-to]   [Filter ____ (SearchField xs, flex)]  padding x 8, gap 4
```

- **Path popup** (`bar` PopupButton, sm): folder icon 14 (`Primary` tone) + folder name (`body` medium) + `chevrons-up-down`. Native menu = ancestor chain of the root (like Finder's path popup, root first, parents below), separator, `Go to Folder…`, then up to 8 recent roots under a `Recent Folders` header. Acts as a drop target for folders: while dragging a folder over it the popup shows a 2pt `primary` outline and its label changes to `Open "<name>"`. iCloud Drive paths show as `iCloud Drive`.
- **Back/Forward** (root-folder history): xs capsule, tooltips `Go back` / `Go forward`; long-press shows the history menu.
- **Search toggle** (`search` IconButton sm, toggled on when the Search view is showing): tooltip `Hide or show the search view`.
- **Footer buttons** (xs ghost IconButtons, tooltips): `plus` `Create a new file…` (click = new file in the selected folder; hold = native menu `New <Language>` for each default template, `Other Languages`, `New from Template`), `folder-plus` `Create a new folder`, `go-to-folder` `Go to Folder…` (opens the prompt sheet, 8.11). Filter field placeholder `Filter`; typing anywhere in the tree starts filtering (the field takes focus with the typed text); Esc clears.
- **Context menu** (native, exact order): `Open`, `Open in New Window`, `Open with Finder`, `Open Terminal Window Here` (title `Open Terminal Window` for files), separator, `Search…` (or `Search "<name>"…`, `Search <n> Items…`), separator, `New File`, `New Folder`, `Go to Folder…`, `Refresh`, separator, `Copy`, `Paste`, `Rename`, `Duplicate`, `Delete`, separator, `View Options` ▸ (`Show Hidden Files`, `Sort Folders on Top`), separator, `Show in Finder`. Also `Open With` ▸ and `Copy Path` where applicable.
- **Rows:** SidebarRow (7.12). Return starts rename, ⌘↓ opens, ⌘↑ goes to the parent folder, ⌘⌫ deletes (confirm), ⇧⌘N new folder, Esc clears the filter, Left/Right collapse/expand, type-to-filter. Drag rows out to Finder or the editor; drop files in to move (⌥ copies); name collisions get a ` copy` suffix.
- **States and copy:**

| State | Presentation | Copy |
|---|---|---|
| Loading a large folder | One row with a 12 `spinner` | `Loading…` |
| Empty folder | EmptyState compact in the list | `This folder is empty` |
| Unreadable folder | Banner (error, not dismissible) under the header | `Couldn't read this folder.` + `Try Again` |
| Filter with no matches | EmptyState compact | title `No Filter Results`; buttons `Search "<text>"…`, `Search Subdirectories`, `Search File Contents` |
| Hidden files | Shown at 60% opacity when `Show hidden files and folders` is on | |
| External change | Row list refreshes (FSEvents), selection and scroll kept | |
| Large file open | Native alert | `The file you selected is too large to be opened safely in the editor.` buttons `Open Anyway`, `Cancel` |
| Delete | Native alert | `Are you sure you want to delete the <file or folder> "<name>"?` / `The <file or folder> will be moved to the Trash.` buttons `Delete` (destructive), `Cancel`. Several: `Are you sure you want to delete the <n> selected items?` |
| Rename conflict | Native alert | `A file or folder with the same name already exists in this directory. Replacing it will overwrite its current contents.` `Replace` / `Don't Replace` |

#### 8.4.2 Search view

```
header 32:  "Search:" [where popup flex ▾]                         [search toggle (on)]
row 32:     [what popup ▾] item                                     [Options…]
find bar:   narrow stacked variant (8.6.4), Find/Replace, Text, Ignore Case, Search
results:    outline: file header rows 28 + match rows 44 (2-line context), scrolls
progress:   when searching: 2pt bar + status text + Stop (above the footer)
footer 24:  [<][>] (xs capsule)   "%lu matches"   [Done]
```

- **Where popup** (native menu): `Browse…`, separator, `Open Documents in Window`, `All Open Documents`, `Custom List of Paths…`, plus dynamic items: `Selected Folder in File Navigator` / `<n> Selected Folders in File Navigator` / `Selected File…` / `<n> Selected Files…` / `<n> Selected Items…`, then recent folders. Default title `Browse…`. `Custom List of Paths…` opens a Sheet titled `Custom Search Paths`, caption `Choose directories and/or files to search.`, a TextArea with hint `(Absolute paths, one per line)`.
- **What popup:** `File Contents + Filenames`, `File Contents`, `Filenames` (title shows the current; `item` when unset). **Options…** opens a Popover (E2, 340 wide) with: `Search Subdirectories`, `Hidden files`, `Binary file contents` (checkboxes); fields `Search File Extensions` (placeholder `Example: txt, html`), `Exclude File Extensions` (`Example: css, svg`), `Exclude Directories` (`Example: .git, vendor`, trailing `Make Default` button), `Search Scope Selector` (`Example: comment, variable, string`; caption `Limiting by scope may slow down a large search considerably.`), checkbox `Ask before overwriting existing files`.
- **Results:** file header row: language tile 16 / file icon, file name (`body` medium), path (`caption` `muted-foreground`, middle truncation), match count Badge; collapsible. Match row: line number `tabular_nums` caption right-aligned in a 40 lane, context in `CODE_FONT_STACK` 12, 1 to 2 lines, the match range in `match-highlight` with `foreground` text. Single click shows the match in the editor (preview), double-click or Return opens it. Context menu: `Replace…` / `Replace Selected` / `Replace All`, `Copy Path`, `Show in File Navigator`, `Show in Finder`, `Open in New Window`; `Undo Replace`.
- **Progress strip:** status `Searching in Folder…` / `Searching in <name>…` / `Searching <n> files…` / `Parsing <name>…` / `Processing <n> match(es)…`, a 2pt indeterminate ProgressBar, Stop (`xs` ghost, label `Stop`). Large result alert: `The search is producing a very large number of matches. Do you want to continue the search?` `Stop Searching` / `Continue Searching`.
- **Empty and errors:** before a search: EmptyState compact, `search` icon 24, title `Search`, message `Search for text in files, folders and open documents.`; no results: `No Results`; replace errors (`File could not be read for replace operation.`, `Characters to be replaced have changed since the search was performed. Please restart the search.`) as a Banner (error) above the results.

### 8.5 Tab bar

32pt, `tab-bar` fill, 1pt `border` below. Padding x 8. Items per 7.13: `[language tile][name ⎵][dot/close]`; the selected tab raised (`tab-active`, E1). `plus` IconButton sm after the last tab (tooltip `Create a new tab`) and overflow `chevron-down` sm (tooltip `Show all tabs`, opens a native menu listing every tab with a check on the selected one and a spinner marker for running ones).

- Right-click a tab (native menu): `Run` (if run enabled), `Stop` (if stop enabled), nothing else.
- Close button tooltip `Close Tab` (`Close Tab Without Saving` with ⌥). Closing the last tab leaves the empty window state (8.16).
- Drag to reorder (the dragged tab follows the pointer 1:1; others make room), drag out beyond 24pt vertically detaches to a new window; drop on another window's tab bar moves it (`Move Tab to New Window` ⌥⌘N, `Merge All Windows`). Double-click empty bar area = new tab. Insertion position per `InsertNewTabs`.
- Hidden by View > `Hide Tab Bar` (⌥⇧⌘T) or the `TabBarHidden` pref (LayoutAnimation).
- Running tab: the icon slot shows a 12 spinner; an edited tab shows the `dot` (tone `Muted`) replaced by `close` on hover.
- Tooltips on tabs: the full path (or `Unsaved document` for untitled).


### 8.6 Editor column

The editor column stacks: find bar (when open), the editor, then (below the console split) the status bar. All colors are editor-scope tokens, so a syntax theme re-tints this whole column and the console.

#### 8.6.1 Editor and gutter

NSH `Editor` (`nsh_base::editor`), one mounted per document (inactive ones hidden to keep undo and scroll). Defaults: `font_family` = code stack, `font_size` 13, `line_height` = size x 1.45 rounded to whole points, `line_numbers` per pref (default on), `soft_wrap` per `WrapLinesEnabled` (default on), `scroll_beyond_last_line` per `AllowScrollPastEnd`, `active_line` per `HighlightCurrentLine`, `folding(false)`, `indent_guides(false)`, `rulers` from the page guide pref, `highlighting(false)` (the app's highlighter provides token colors as decorations), `auto_close`/`auto_indent` per prefs.

| Part | Spec |
|---|---|
| Content | Fill `editor-bg`, text `editor-fg`. Left padding 8 from the gutter, top and bottom padding 8, caret `editor-caret` 1.5pt wide (bar style; block/underline are not available in NSH Editor), selection `editor-selection` (`-inactive` when unfocused), current line `editor-line-highlight` (full row), invisibles `editor-invisibles`, page guide `editor-ruler` 1pt |
| Gutter | Fill `gutter-bg`. Lanes left to right: breakpoint lane 16, line numbers (right-aligned, `tabular_nums`, size = editor size − 1, color `gutter-fg`, the caret's line `gutter-fg-active`), right padding 8. Width = lane + digits for the document's line count (minimum 3 digits) + padding. Optional 1pt right separator `gutter-separator` (pref `DrawLineNumberSeparator`, default on). Relative numbers per pref (`RelativeLineNumbers`) |
| Breakpoint | `breakpoint` icon 14 (tone `breakpoint`) in the lane, vertically centered on the line. Hover over the lane shows a ghost (`breakpoint-ghost`, 100 ms). Click toggles; drag the tag off the gutter to remove it; right-click shows a native menu `Set Breakpoint` / `Remove Breakpoint`. Temporary breakpoints use the hollow tone (`breakpoint-disabled` outline). Persisted per file |
| Execution marker | `debug-arrow` in the lane (tone `debug-arrow`), whole line tinted `debug-line`, plus the annotation `Stopped at Breakpoint` / `Stopped at Temporary Breakpoint` / `Paused Execution` (8.6.2 pill, tone Info, `pause-circle` for paused) |
| Overview ruler | 8pt strip at the trailing edge (inside the scrollbar's region) with marks: errors `diag-error`, warnings `diag-warning`, notices `diag-info`, find matches `editor-find-match`, breakpoints `breakpoint`; marks 2pt high, minimum 4pt wide lane per kind. Pref `ScrollMarkersEnabled`; `Keep markers visible when scroller is hidden` keeps it visible with overlay scrollers |
| Scrollers | Native overlay scrollers |

#### 8.6.2 Diagnostic annotations

NSH has no inline widgets, so annotations are a separate overlay layer positioned from `range_bounds()`/`scroll_offset()`, aligned to the editor's trailing edge, clipped to the viewport, pointer-transparent except the pills.

- **Line effects** (pref-controlled): underline wavy `diag-*` on the range (`EditorDiagnostic`), line tint `diag-*-bg` (`DiagnosticsHighlightLine`), overview mark.
- **Pill:** height = line height − 2, `radius-sm` 6, fill `diag-*-bg` over `editor-bg`, 1pt outline at 30% of the severity color, right-aligned 8 from the edge, max width 60% of the editor and at most the free space right of the line's text (below 56 wide it becomes icon-only with a tooltip). Content: severity icon 12 (`error-circle` / `warning-triangle` / `info-circle`, tone from `diag-*`), message `type-caption` in `editor-fg` (single line, end ellipsis), optional `Fix` button (xs `secondary`). Icon-only mode via pref `DiagnosticsIconOnly`.
- **Expand:** click expands the pill in place to up to 6 lines (overlaying the next lines, E2); Esc or click away collapses.
- **Context menu** (native): `Dismiss Message`, `Dismiss All Messages In Document`, `Automatically Apply Fix "<fix>"`, `Ignore Messages Like…`, `Disable Messages For Document "<name>"`, `Disable Diagnostics`.
- **Copy examples:** `Expected ';' at end of declaration` + `Fix`; severities named `Error`, `Warning`, `Notice`.

#### 8.6.3 Completion list and hover popovers

- **Completion** (NSH `completion(true)`, built-in list parts restyled through `EditorClasses`): panel `popover` fill, `radius-lg`, E2, width auto 280..520, max 8 rows of `row-sm` 24 then scrolls, padding 4. Row: kind GlyphTile 16, label (`CODE_FONT_STACK` 12, matched characters in semibold), detail/return type right-aligned `caption` `muted-foreground`. Selected row `menu-highlight` + `menu-highlight-foreground` (so native-looking). To the right (or below if no room) a documentation Popover (320 max, E2, `body` text, signature in the code font) for the selected item; a `book-open` xs button on the selected row opens the docs panel for it. Trigger: after `CodeCompletionWordLength` characters, Esc or ⌃Space on demand. Motion: see the editor table in 9.4.
- **Hover popover** (diagnostics under the pointer, variable values while paused): Popover `radius-lg`, 320 max, anchored below the token (above if no room), 300 ms delay, dismissed when the pointer leaves both token and popover. Variable popover: an outline row list identical to the Variables view rows (8.9) with `Reveal in Variables View` on click.
- **Go to definition / link underline:** ⌘-hover underlines in `console-link`-like `editor-fg` underline (no color change), pointer cursor.

#### 8.6.4 Find bar

In-flow bar between the tab bar region and the editor (pushes the editor down; LayoutAnimation `motion-base`). Fill `editor-bar`, 1pt `editor-bar-border` below, padding x 8, controls `sm` (24), vertical padding 6 (single row 36, with replace 68). Shown by ⌘F (Find…), ⌥⌘F (Find and Replace…), hidden by Done, Esc, or ⌘F toggling when focused.

Row 1 (36): `[Find ▾]` operation PopupButton `field` sm 72 (items `Find`, `Replace`) · SearchField sm (flex, min 160, accessible label `Search text`, placeholder `Find`, trailing count) · matching PopupButton `field` sm 136 · `Ignore Case` Checkbox · prev/next capsule (`chevron-up` / `chevron-down`) · `Done` Button `default` sm.
Row 2 (replace mode, 32): Replace TextField sm (flex, placeholder `Replace`) · Segmented capsule sm (`Replace`, `All In Selection`, `All`).

| Element | Copy |
|---|---|
| Matching menu (native) | `Text`, `Exact Word`, `Word Starts With`, `Word Ends With`, `Regular Expression`, separator, `Wrap Around` (check), separator, `Insert Pattern` ▸ (`Any Characters`, `Any Word Characters`, `Any Non-Word Characters`, `Word Boundary`, `Whitespace And Newline`, `Leading Whitespace`, `Trailing Whitespace`, `Current Search As Regular Expression`, separator, `Add Pattern…`, `Clear Added Patterns…`; with Replace active also `Entire Match`, `Capture Group <n>`, `Named Capture Group "<name>"`, `Escaped Current Replacement`, `Make Next Character Upper Case`, `Make Next Character Lower Case`, `Make Remaining Characters Upper Case`, `Make Remaining Characters Lower Case`) |
| Prev / next tooltips | `Find previous match` + newline + `⌥-click to add to current selection`; `Find next match` + newline + `⌥-click to add to current selection` |
| Replace segmented tooltips | `Replace the selected match and find the next match`; `Replace all matches in the current selection`; `Replace all matches` + newline + `Click and hold to see more options` |
| Count (trailing in the field) | `3 of 12`, `12 matches`, `No matches`; invalid regex: `Regex Invalid` in `destructive` and a `destructive` outline |
| Recent searches (field menu) | `Recent Searches`, `No Recent Searches`, `Clear Recent Searches` |
| Pattern prompts | `Enter A Name For Your Pattern`, `Enter A Regular Expression` |

**Narrow variant** (editor column narrower than 560, and always in the sidebar Search view): three stacked rows: `[Find ▾] [Text ▾] [Ignore Case]`; `[search field flex] [prev/next]`; `[replace field flex] [Replace | All In Selection | All]` (replace row only in Replace mode). Row height 32, gap 6.

Matches in the editor: `editor-find-match`, the current one `editor-find-current`; token-aware matches (Find and Select All on a token) `editor-token-match`. The count and highlights update per keystroke.

#### 8.6.5 Status bar

24pt, fill `editor-bar`, 1pt `editor-bar-border` above, padding x 8, items separated by 12, caption text `editor-bar-foreground`. It spans the whole center column (below the console). Left group (run state), spacer, right group (editor state).

Left group, in order (each shown only when applicable):

| Item | Content |
|---|---|
| Run state (status item) | see the state table below |
| Exit status | `Exit Status 1` (tone `Danger`), only for a non-zero exit |
| Info 1 | `Time 42 ms` (after a run; `CPU Time` variant per language) |
| Info 2 | while running: `CPU 12%` and `Memory 10.3M` (live, `tabular_nums`, units `K`/`M`/`G`); after: `Peak Memory 5.6M` |
| Show the Console | status button `Show the Console` when the console is hidden and output arrived |

| Run state | Icon (tone) | Text |
|---|---|---|
| Idle | none | none |
| Preparing | spinner 12 (Muted) | `Preparing Run…` |
| Compiling | spinner | `Compiling…` |
| Running | spinner | `Running…` |
| Paused (debugging) | `pause-circle` (Warning) | `Paused` |
| Succeeded | `success-circle` (Success) | `Run Succeeded` (`Compile Succeeded` for compile-only) |
| Compile failed | `error-circle` (Danger) | `Compile Error` |
| Stopped | `stop-circle` (Muted) | `Run Stopped` / `Compile Stopped` |
| Ended | `dot` (Muted) | `Run Ended` |
| Invalid command | `warning-triangle` (Warning) | `Invalid Run Command` |
| Diagnostics present | `error-circle` / `warning-triangle` / `info-circle` | `1 Error`, `2 Errors, 1 Warning`, `3 Issues` (replaces Idle; also shown beside the run state as a second item) |

Right group:

| Item | Content and behaviour |
|---|---|
| Symbol | `bar` PopupButton: symbol tile 14 + name of the symbol at the caret (`permute`), empty = `No Symbols`, loading = `Loading Symbols…`. Opens the symbol Popover (320 wide): SearchField md (placeholder `Search symbols`), list rows 28 (tile, name, kind caption), hierarchy indented 16, diagnostics included when `Include diagnostics in symbol navigator` is on; Return jumps and closes |
| Indentation | `bar` PopupButton: `Tabs: 4` or `Spaces: 4`; native menu `Indent Using` header, `Tabs`, `Spaces`, `Size` header, `2`, `3`, `4`, `6`, `8` (checks on the current). Conversion prompt: `Would you like to convert the existing indentation in this document to <Tabs or Spaces>?` `Convert` / `Don't Convert` |
| Position | status button `Line 9, Column 44` (or `Line 9, Character 44` per the `Use columns in selection label` pref). A selection shows `<n> Characters`, `<n> Lines`; multiple: `3 Selections, Identical, 3 Lines, 0 Characters`; off-screen: `<n> Above, <m> Below Visible Area`. Click opens Go to Line |
| Language server | When a language server fails the left group gets a Warning status item `<Language> · <message>` (for example `Rust · rust-analyzer failed: the server closed its output`) with a `Show Error` action; the editor keeps working |

### 8.7 Console

The console belongs to the document (each tab has its own). Position bottom (default) or right; a `v_resizable`/`h_resizable` split under the editor with the handle (7.25) owning the divider. Default size 40% of the center column, min 96 high or 240 wide; collapses by dragging past the minimum; double-click the handle toggles.

```
toolbar 28 (editor-bar fill, bottom hairline editor-bar-border, padding x 8, gap 6, controls sm)
  [Terminal|Web] [debugger group] [web resource bar]  ····  [Live] [Filter] [All Output ▾] [Clear] [stack|text] [inspector] [Hide]
body (console-bg): [debug pane (8.9)] | console text   or   web view   or   PDF view
```

| Toolbar item | Component | Copy | Shown when |
|---|---|---|---|
| Console type | Segmented capsule sm, 2 icon segments (`terminal`, `globe`) | tooltips `Terminal Mode`, `Web Mode` | the language allows toggling console types |
| Debugger group | CapsuleGroup sm: `debug-continue`/`pause` (toggle), `debug-step-over`, `debug-step-in`, `debug-step-out` | `Continue Execution` / `Pause Execution`, `Step Over`, `Step In`, `Step Out` (⌃1..⌃4) | a debug session exists |
| Web resource bar | Back/Forward capsule, source PopupButton `bar`, `Edit` Button sm, URL TextField sm (placeholder `URL`), `Inject Into:` popup | tooltips `Go Back`, `Go Forward`; source menu `Empty Page`, `Website`, `Custom HTML`, `HTML File` ▸ (open HTML documents), separator, `Inject Scripts At Start`, `Inject Scripts At End`; Custom HTML prompt `Enter your custom HTML string below:`; extras in the overflow menu: `Open in Browser`, `Resolve Relative URLs`, `Kill Web Process` | Web mode |
| Live Updating | toggle IconButton sm (`eye`) | tooltip `Live Updating`; first use alert `Prevent additional dialogs during live updates` (checkbox, sheet variant) | Web mode with live HTML |
| Filter | SearchField sm 120 to 200 (`filter` icon) | placeholder `Filter`; filters by scope selector over the stored scope-tagged ranges | always (Terminal mode) |
| Output | `bar` PopupButton, title = current filter | menu: `All Output`, `Program Output`, `Program Stdout`, `Program Stderr`, `Debugger Output`, `Errors`, `Errors and Warnings`, `Hide Errors and Warnings`, `Custom Output` (prompt `Enter a scope selector to filter the console text.`, field `Scope Selector`); items check and combine | always |
| Clear | IconButton sm `eraser` | tooltip `Clear the Console` ⌘K | always |
| Debugger view | Segmented capsule sm, 2 toggle segments (`debug-stack`, `terminal`) | `Hide Stack and Variables` / `Show Stack and Variables`, `Hide Console Text` / `Show Console Text` | debugging |
| Web Inspector | Segmented capsule sm, 3 icon segments (`web-inspector-window`, `console-bottom`, `sidebar-right`) | `Web Inspector in separate window`, `Web Inspector docked to the bottom`, `Web Inspector docked to the side` | Web mode |
| Hide | IconButton sm `chevron-down` (`chevron-right` when the console is at the right) | tooltip `Hide the Console` ⇧⌘C | always |

Console text body: fill `console-bg`, text `console-fg` in the code stack at `console-font-size` 12 with line height 1.45, padding x 12 and y 8, selectable and copyable, lines are scope-tagged ranges (stdout, stderr, compile, debugger, errors, notices) so filters and error coloring work retroactively. stderr and errors `console-error`; informative lines (notices, warnings from tools, debugger notes, `Terminated due to signal: SIGTERM (15)`) `console-info`; the debugger prompt `console-prompt` medium; links and `file:line` references `console-link`, underlined on hover, click jumps to the location; ANSI colors `ansi-*` and bold/italic/underline. Interactive stdin: the last line is editable; typed text is sent on Return; while paused in the debugger, typed commands go to the debugger with Up/Down history and Tab completion. Output sticks to the bottom while the user is at the bottom; scrolling up pauses following and shows a `Scroll to bottom` IconButton (`chevron-down` in a `popover` capsule, bottom-right, 12 inset) while new output arrives.

| Console state | Presentation / copy |
|---|---|
| Idle, never run | If opened manually: centered caption `muted-foreground`: `Output appears here when you run the document (⌘R)` |
| Running | Output streams; toolbar shows the Stop state through the toolbar Run group; status bar shows `Running…`, CPU, Memory |
| Finished | Output stays; status bar shows the result; console hides again only if it was auto-shown and is empty |
| Large output | Native alert `Large output to the console detected. Would you like to stop the run?` / `Continuing the run may hang the application.` buttons `Stop`, `Continue Run`; then the run stops with the line `Run stopped due to excessively large output.` in `console-info` |
| Too many characters | Native alert `The console contains too many characters to be displayed safely. What would you like to do?` buttons `Copy To Clipboard`, `Force Display` |
| Web mode failed | Banner (error) above the page: `Failed to connect to server on port <port>.` + `Try Again` |
| Process missing tool (exit 127) | After the output, a `console-info` hint line from the language's `Errors` table (the install hint for the missing command) |

### 8.8 Documentation panel

Right panel, `background` fill, 1pt `border` on its left edge, default 300 (min 240, max 600 or 50% of the window), toggled by ⇧⌘I or the toolbar. It is a web page in a native frame.

```
header row 1 (40):  [SearchField md "Search Docs" (flex)] [‹ ›] [eye] [gear]       padding x 8, gap 6
header row 2 (28):  [Python] [Google] [Stack Overflow] [Find On Page]  (chips, scroll x)
loading bar (2):    indeterminate ProgressBar while loading, fades out
page (flex):        web_view  | EmptyState | error view
footer (24):        [Item ▾]  table-of-contents popup (status-bar style)
```

- **Search field:** placeholder `Search Docs`; in Find On Page mode the placeholder is `Find On Page` and the trailing count is `1/5`. Typing looks up in the selected source after a short delay.
- **Back/Forward** capsule sm: tooltips `Go Back`, `Go Forward` (`Go Back to Search Results` after an auto-redirect); long-press history menus.
- **Automatic Lookups:** toggle IconButton sm (`eye`), tooltip `Automatic Lookups`; toggled on tints the icon `primary`.
- **Actions** (`gear`, native menu): `Open in Browser`, `Copy Current URL`, separator, `Bookmark This Page…` (prompt `Enter a name for your bookmark:` `Save`), `Define Docs Configuration…` (prompts `Enter the URL of the new documentation source:`, `Enter a name for the new documentation source:`), `Edit Docs Configurations`, separator, `Go to URL…` (prompt `Go to URL:`), `Find…`, `Reload Page`, separator, `Stylize Web Content` (check), `Zoom` ▸ (`Zoom In`, `Zoom Out`, a `<n>%` reset item).
- **Source chips:** Button `ghost` sm with `radius-full`, label = source name; selected chip `capsule-selected` fill, `primary` text, medium; tooltip `Search With <name>`. Bookmark-type sources show `bookmark` 12 before the name.
- **Expand grabber:** a 16 x 64 capsule tab straddling the panel's left handle (`chevrons-left`; `chevrons-right` when expanded), visible on hover and focus within; tooltip `Expand the documentation view`. Expanded = the panel temporarily widens over the editor (to 70% of the window). The View menu item remains the non-hover path.
- **Page recoloring:** the page is recolored to the app appearance (injected script, `Stylize Web Content`); background transparent so `background` shows through.

| State | Presentation / copy |
|---|---|
| No sources for the language | EmptyState: `book-open` 24, title `No Documentation Sources`, button `Configure…` (opens Preferences > Languages > Docs) |
| Loading | 2pt ProgressBar under the header |
| Error page | EmptyState: title `Error`, message `The requested URL was not found on this server. Please try again later.` |
| Offline | EmptyState: `wifi-off` 24, title `No Internet Connection`, message `An internet connection is required to search the documentation. Check your internet connection and try again.`, button `Try Again` |
| Moved page (404) | Native alert `The <source or bookmark> "<name>" appears to have moved to a new URL or is no longer available. (404)` / `Would you like to update the URL associated with the <source or bookmark>?` buttons `Update URL…`, `Cancel` |

### 8.9 Debug pane

Inside the console body while a debug session exists (`AutomaticallyHideDebuggerView`: hidden when the session ends). Three `h_resizable` panels with 1pt handles: **Stack** (default 30%, min 160), **Variables** (35%, min 200), **Console text** (rest, min 160). The console grows to the larger split (60% of the center column) when the session starts (LayoutAnimation). Each of Stack and Variables has a list SectionHeader (24: `Stack`, `Variables`; caption medium) and a list (rows 24).

- **Stack rows:** GlyphTile `{}` 16, `0 permute()` (`body`; the frame index is `muted-foreground` `tabular_nums`), trailing file:line `caption` `muted-foreground`; frames without source in italic `muted-foreground`; thread parents `Thread: <description>` caption medium; selecting a frame opens that file at that line (read-only for foreign files).
- **Variable rows:** disclosure 16, `V` GlyphTile 16, name `body` medium, `= value` in the code font 12 `muted-foreground` (strings quoted), `<n> items` caption for containers; trailing 2 xs ghost IconButtons visible on hover or selection: `info-circle` (`Print Description`), `ellipsis` (menu). Values are editable in place (double-click). Changed values since the last step briefly flash (9.2).
- **Context menu** (native): `Print Description of "<var>"`, `Copy`, `Edit Value…`, `Watch "<var>"`, separator, `View` ▸ (`Show Types`, `Show All Variables`), `Sort By` ▸ (`Name`, `Natural Order`, `Reverse Natural Order`), separator, `Show Values on Hover`, `Enable Variables View`.
- **Console text** pane: the normal console text with the debugger prompt (`(Pdb) `, `(lldb) `) in `console-prompt`.
- **Editor side:** gutter arrow + line tint + annotation (8.6.1); status bar `Paused`.
- **States and copy:** not paused (running): Stack and Variables show caption `Running…` centered; session ended: panes hide; start failure: native alert `Something may have gone wrong when starting the debugger.` with informative text (`If you were asked to enter your password and allow developer tools to take control of the debugged process, you must allow this for debugging to work.`) buttons `Try Again`, `Run Without Debugging`, `Show Debugger Log`; start message in the console: `Starting Debugger…`.


### 8.10 Run Settings sheet

Opened by the toolbar wrench or Run > `Edit Run Settings…` (⇧⌘R). A Sheet (7.19), 500 wide, height to content (about 378, minimum 304), anchored to the document window. Title `Run Settings`, caption = the document name (`permute.py`) in `muted-foreground`. Padding 20, field gap 12, label gap 4.

| Order | Control | Copy |
|---|---|---|
| 1 | Label row: `Run Command:` + help IconButton (`help-circle`, opens Help at `Run Command`) | |
| 2 | CodeField (480 x 50, 2 rows, line numbers on, shell grammar, shell completion) | Default text = the language's run command, e.g. `php $filename` |
| 3 | Banner (neutral `muted`, hidden until the command differs from the language default) | `Run command is specific to this file.` buttons `Revert`, `Make Default`. `Make Default` confirms with an alert: `Are you sure you want to change the default run command for this language?` / `If you later want to reset a language to the default configuration, you can do so in Language Preferences.` buttons `Make Default`, `Cancel` |
| 4 | `Arguments:` TextField md (480 x 28) | Tooltip `Argument list preview:` + newline + `Arguments formatted as on the command line.` |
| 5 | `Compile Flags:` TextField md (only for languages with a compile script) + the same banner pattern | `Compile flags are specific to this file.` `Revert`, `Make Default` (alert `Are you sure you want to change the default compile flags for this language?`) |
| 6 | `Program Input:` TextArea (480 x 80, code font) | |
| 7 | Footer: `Close` Button `primary` lg (92 wide), Return and Esc both close | |

The toolbar wrench is tinted `primary` while the file has any custom setting. Settings persist per document (and per language for defaults). Opening focuses the Run Command field. Changes apply as typed (no Apply button).

### 8.11 Go to Line and the Prompt sheet

**Prompt sheet** (the shared single-field component): a Sheet 420 wide: title (`subhead`), optional message (`body` `muted-foreground`), optional accessory view, a TextField lg (32) with optional inline completion ghost text, footer `Cancel` + `primary` lg commit (`Go`, `Save`, `Add`...). The commit is disabled until the validator passes; an invalid submission shakes the sheet (`MotionPreset::Shake`), outlines the field in `destructive` and shows the reason below in `destructive` caption. Optional path completer: Tab accepts the ghost suggestion, `~` and absolute paths supported.

| Prompt | Title / message | Field (label, placeholder) | Commit |
|---|---|---|---|
| Go to Folder | `Go to Folder` / `Go to the file or folder:` | `Open Path:` | `Go` |
| Go to Line | `Go to Line` / `Enter a line number, or line:column.` | `Line Number` | `Go` |
| Bookmark | `Bookmark This Page` / `Enter a name for your bookmark:` | | `Save` |
| Docs source URL | `Define Docs Configuration` / `Enter the URL of the new documentation source:` then `Enter a name for the new documentation source:` | | `Next`, `Add` |
| Go to URL | `Go to URL` / `Go to URL:` | | `Go` |
| Custom HTML | `Custom HTML` / `Enter your custom HTML string below:` (TextArea) | | `Save` |
| Scope selector | `Custom Output` / `Enter a scope selector to filter the console text.` | `Scope Selector` | `Filter` |
| Find pattern | `Add Pattern` / `Enter A Name For Your Pattern`, `Enter A Regular Expression` | | `Add` |
| Line comment | `Line Comment Characters` | | `Save` |

### 8.12 QuickNav (Open Quickly)

Opened by ⇧⌘O (`Open Quickly…`), ⌥⌘L (language mode), New from template palette. An in-window modal layer (`dialog()`), not a separate window.

- **Panel:** 480 wide, `popover` fill, `radius-xl` 16, 1pt `border`, E3, `scrim` behind, horizontally centered, top edge at 22% of the window height. No title. Padding 0; the field row is 56 high.
- **Field row:** SearchField lg (32, no outline, `subhead` text, placeholder `Open Quickly`) with 12 inset, a 16 spinner at the trailing edge while results are being generated.
- **Results:** a hairline under the field, then rows `row-xl` 44, `inset 8`, selection `selection`. Row: icon 28 slot (GlyphTile or file icon 20), title `body` (matched characters medium + `match-highlight` underline-free), subtitle `caption` (`muted-foreground`; `selection-foreground` at 80% when selected). Height = 56 + min(rows, 6) x 44 (+ 8 padding); the panel animates its height as results change. Empty query: only the field row (collapsed).
- **Providers** (async, cancellable, fuzzy): open documents, files under the navigator root, recent documents, symbols of the current document, `line` (`12`, `l12`, `line 12`), absolute paths (query starts with `/` or `~`), languages (`Set Language: <name>` / subtitle `Set the language of the current document`, `New Document: <name>` / `Create a new untitled document`), menu commands (subtitle = shortcut).
- **No results:** a centered `No Results` caption (`muted-foreground`) in a 44pt row.
- **Keys:** typing filters; Up/Down move (selection follows, wrapping); Return opens; Esc or an outside click dismisses; rows can be dragged into the editor or Finder. The last selected result per query is remembered.
- **Focus:** opens with the field focused; closing returns focus to where it was.

### 8.13 Preferences window

One window, id `prefs`, `WindowKind::Normal`, hidden system title bar. Title band 32 shows `Preferences – <Pane>` (en dash) centered; a 72pt pane strip follows; the content area (fill `background`) fills the rest. Remembers the last pane (`NKPreferencesSelectedIdentifier` equivalent). Opens with ⌘,; `Edit Languages…` (⌥⌘,) and `Diagnostics Preferences…` open a specific pane/tab. Switching panes resizes the window to the pane's size (animated, 9.3) and cross-fades the content.

**Pane strip:** items 76 x 60 (icon 24 over a `caption` label, 4 gap), 4 between, centered; selected item fill `muted`, `radius-lg` 12, icon and label `foreground`, medium; unselected `muted-foreground`, hover `accent`. One roving Tab stop, Left/Right move and select. Order and sizes:

| # | Pane (label) | Icon | Content size (w x h) | Window resizable |
|---|---|---|---|---|
| 1 | General | `pref-general` | 560 x 500 | no |
| 2 | Text Editing | `pref-text` | 620 x 560 | no |
| 3 | Languages | `pref-languages` | 760 x 520 (min 700 x 460) | yes |
| 4 | Themes | `pref-themes` | 720 x 540 (min 680 x 480) | yes |
| 5 | Key Bindings | `command` | 760 x 520 (min 700 x 440) | yes |
| 6 | Shell | `terminal` | 560 x 520 | no |
| 7 | Advanced | `gear` | 560 x 440 | no |

Window height = 32 + 72 + content height. Content is padded 20; scrolls vertically inside when taller than the window (never the whole window).

#### 8.13.1 Data-driven settings view (General, Text Editing, Advanced)

One builder renders the settings schema (identifier path, title, type, default, options, children). FormRow (7.18) layout: label column 200 right-aligned, control column; sections by the identifier's second path component with `form` SectionHeaders. Boolean = Checkbox in the control column; options = PopupButton; number = NumberField; string = TextField; button = Button `default` sm. Children are indented 20 and disabled when the parent is off. `%@` titles embed the control inline. Informative text is a caption under its control. Values are stored under the last path component. Two settings (`AskToSaveWhenQuitting`, `AllowSuggestedFilenames`) show the alert `App restart required before this change takes effect.` / `Your new setting will take effect the next time ScratchRunner is launched.` (button `OK`). The `Updates` section and `Check for Updates` are omitted (no updater in v1).

**General** (stacked headings): *Documents*: `Enable Auto Save and Versions`, `Show all filename extensions`, `Ask to save untitled documents when quitting`, `Use templates for new files`, `Allow filename suggestions from macOS`, `Default File Encoding:` popup (default `Unicode (UTF-8)`), button `Edit File Type Associations…`. *Tabs*: `Prefer opening new documents in tabs`, `Hide tab bar by default`, `Insert New Tabs:` popup (`After existing tabs`, `Before existing tabs`, `After the selected tab`). *Console*: `Console Position:` popup (`Bottom`, `Right`), `Hide console automatically`. *File Navigator*: `Show hidden files and folders`, `Sort folders on top`. *Notifications*: `Show notifications while the app is inactive`.

*Edit File Type Associations…* opens a Sheet 520: message `Select the file types you would like ScratchRunner to open by default.`, a table with checkboxes (`Plain Text`, `Source Code`, added extensions), buttons `Select All`, `Deselect All`, `Add File Extensions…` (prompt `Enter file extensions, comma separated.`), footer `Cancel`, `Set as Default`. First-run variant: alert `Would you like to set ScratchRunner as your default editor?` buttons `Set as Default Editor`, `Don't Set`.

**Text Editing**: a Segmented capsule md (`Appearance | Indentation | Behavior | Completion | Diagnostics`) centered at the top (16 below the strip), the selected category's rows below.

- *Appearance*: `Font:` (current font name + size as text, button `Choose Font…`, checkboxes `Ligatures`, `Antialiasing`, `Smoothing`), `Syntax highlighting`, `Line Spacing:` NumberField (0 to 100, step 0.05), `Show navigational markers in scroller` (child `Keep markers visible when scroller is hidden`), `Wrap lines`, `Allow scrolling past end of document`, `Show page guide at column` + NumberField (80), `Show invisible characters:` + TextField + mode popup (`Everywhere`, `At beginning and end of lines`, `Nonconforming indentation`, separator, `In selected text`, `In selected text + nonconforming indentation`), `Insertion Point:` popup (`Bar`, `Block`, `Underline`; only Bar renders, others are disabled with the tooltip `Not available`), children `Blink` (child `Animate`), `Visible with selection`, `Alternate Insertion Point:` with the same children, `Highlight current line`, `Show line numbers` (child `Relative to selection`), `Draw separator at left margin`.
- *Indentation*: `Indent Using:` popup (`Tabs`, `Spaces`) + size popup (`2 3 4 6 8`), caption `Indentation can be customized on a per-language basis under the Languages tab.`; `Indent wrapped lines` + NumberField + `indent levels`; `Treat consecutive spaces indentation as single unit`; `Auto-Indent while typing`; `Auto-Indent pasted code`.
- *Behavior*: `Automatically close character pairs` (children `{`, `(`, `[`, `<`, `"`, `'`, `` ` `` as 7 checkboxes in one row of mono labels), `Jump past ending character when matching key pressed` (children `}`, `)`, `]`, `>`, `"`, `'`, `` ` ``), `Automatically highlight balancing characters`, `Double-click on word also selects relevant special chars`, `Enable code tokenizers` (children `Clang`, `PHP`, `Python`, `Shell`, `Swift`, `Generic Source Code`), `Underline tokens at insertion point`, `Kill-Ring Size:` NumberField with caption `The kill-ring is similar to the pasteboard. Text that is "killed" (default key binding ^K) is added to the buffer and can be inserted again later ("yanked", default key binding ^Y).`, `Check spelling while typing` (children `Check grammar with spelling`, `Correct spelling automatically`).
- *Completion*: `Suggest completions while typing` (children `Display completions after typing` + NumberField + `characters`, `Ignore with only low-quality results`), `Fuzzy search` (child `Force correct character order in matches`), `Prioritize previously chosen completions`, `Use word-completions for unsupported languages`, `Enable short string completions`, `Automatically insert import statements`, `Animate completion panel`.
- *Diagnostics*: `Generate diagnostics while typing` (child `Delay` + NumberField + `seconds`, step 0.5), `Show errors derived from syntax definitions` (child `Only show first syntax error in document`), `Include errors from the console in the editor`, `Animate diagnostic messages`, `Highlight lines with diagnostics`, `Underline text associated with diagnostics`, `Include diagnostics in symbol navigator`, `Show:` popup (`Full diagnostic messages`, `Icon only`), `Auto-fixes:` button `Configure…` (Sheet `Automatically Applied Fixes`, TextArea with hint `(Regular expressions, one per line)`), `Ignored messages:` button `Configure…` (Sheet `Globally Ignored Diagnostic Messages`).

**Advanced**: `Use columns in selection label`, `Automatically resolve relative file URLs in webpages`, `Hide debugger termination message`, `Close new unedited document when opening another`, `Write compiler output files to temporary directory`, `Group similar languages in menus`, `Use custom file icons`, `Enable three-finger tap in editor`; section *Clang completion and diagnostics*: `Automatically determine appropriate system compile flags`, `Apply user-defined compile flags`, `Extra Clang Flags:` TextField, `Extra Swift Flags:` TextField; section *Debugging*: `Center current location during stepping`.

#### 8.13.2 Languages

```
+ list 220 --------------+ + segmented: Settings | Templates | Docs | Diagnostics ------+
| AppleScript        [x] | | card (radius-lg, 1pt border, padding 16)                    |
| C                  [x] | |   tab content                                                |
| ...                    | |                                                              |
+------------------------+ +--------------------------------------------------------------+
[+][−][⋯]
```

ListManager (7.29) on the left, 220 wide: rows `row-sm`, language name (`body`) and a trailing Checkbox (Enabled); selection `selection`; empty `No Items`; inline rename via `Rename`. Strip menu: `Duplicate`, `Rename`, `Delete…`, separator, `Import…`, `Export…`, separator, `Reset…`, `Reset All…`, `Show in Finder`. Alerts: delete `Are you sure you want to delete the language "<name>"?` / `The language will be moved to the Trash.`; reset `Are you sure you want to reset the language "<name>" to the default?` / `The settings for this language will be reset to the default.`; not resettable `The language "<name>" cannot be reset.` / `This language was not bundled with ScratchRunner and cannot be reset.`; reset all `Are you sure you want to reset all languages to the defaults?` / `This action cannot be undone.`

The segmented capsule (md) sits centered above a `card` that holds the tab content.

- **Settings:** (FormRow, label column 120 here) `Language uses compile script` Checkbox + help IconButton + `Edit Script` Button (disabled unless checked); `Compile Flags:` TextField; separator; `Run Command:` + help, right-aligned `I/O Encoding:` popup (`Automatic (UTF-8)` + encodings); CodeField 72 (shell grammar); legend caption `$filename  The filename of the source file` and `$compiler  Compile script output` (the variable in a code chip; `$compiler` `muted-foreground` when no compile script); separator; `File Extensions:` TextField (placeholder `h, m`); `Syntax Mode:` PopupButton (searchable; `Plain Text`, grammars, `Import Syntax File…`); `Console Type:` popup (`Terminal`, `Web`, `Web Resource`, `PDF`, separator, header `Console Position`: `Same as Default`, `Bottom`, `Right`, separator, header `Additional Console Options`: `Allow Toggling of Console Types`, `Live Updating`); `Debugger:` popup (`None`, debugger names, separator, `Show Debuggers Directory`); `Indentation:` popup (`Default`, header `Indent Using`: `Tabs`, `Spaces`, header `Size`: `2 3 4 6 8`; closed title `Default`, `Tabs, <n> wide`, `<n> spaces`).
- **Templates:** Table (columns `Name` 140, `Default` 55, `Extension` 60, `Name on Creation` 104 with a Checkbox; `Default` shows a Badge) with `+` / `−` (sm `filled` IconButtons, `Create a new template`), Button `Set as Default` (reads `Unset as Default` for the default row), help IconButton (`Show template help…`); label `Template:` with the right-aligned hint `Use <##> to represent text insertion point.`; CodeField (185, line numbers, the language's grammar). Delete: `Are you sure you want to delete the template "<name>"?`.
- **Docs:** caption `Below you can add online documentation resources for this language. Use {query} in the URL to represent the search string.`; Table columns: Enabled Checkbox, `Name`, `Type` (popup cell `Search`, `Bookmark`, `Text Find`), `URL`, `POST` (placeholder `None`), `CSS` (Button `Edit`), `JS` (Button `Edit`); `+` (`Add a documentation resource`) / `−`; label `Global entries (available for all languages):` with an identical Table and `+ −`; Checkbox `When invoked from the editor, always use:` with a popup of the global entries. Delete: `Are you sure you want to delete the documentation entry?`.
- **Diagnostics:** stacked collapsible rows (disclosure, editable name field default `My Diagnostic`, update-mode `bar` popup titled with the mode: items `Run Diagnostic`, `On Edit`, `On Run`, `Manually`, separator, `Cancel Run If New Errors Produced`, `Cancel Run If New Warnings Produced`, `Cancel Run If New Notices Produced`; delete via row menu with alert `Are you sure you want to delete the diagnostic "<name>"?` / `The associated files will be moved to the Trash.`). Row body: `Generate diagnostics using:` popup (`Command`, `Compile Script`, `Console Text`, `Console Text (Highlight Only)`); for Compile Script `Use custom compile flags:` Checkbox + `Add to existing flags` + TextField; for Command a CodeField; `Mode to interpret output:` popup (`Text (Unix/GCC style messages)`, `Regular Expression`, `Custom Script`, `CheckStyle`); regex CodeField (default `^(?<file>[^\s].*?):(?<line>\d+):(?<column>\d+):? (?<message>.*)$`) with caption `Possible capture groups: file, line, column, message, messageID, error, warning, note, zeroLine, zeroColumn, endLine, endColumn, offset, endOffset, length, code`; `Edit Script` Button; `Keyboard shortcut:` KeyRecorder; `Perform Test Run…` Button (alert `Choose a document for the test run:`; results `The diagnostics test run produced <n> diagnostic(s).`, `Output from the diagnostic run:`). Above the rows: Checkbox `Automatically add built-in diagnostics`. Footer Buttons `Options…` (Popover), `Auto-Fixes…`, `Ignored…`.

#### 8.13.3 Themes

```
+ Color Themes 180 -+ + six wells: Foreground Background Selection Current Line Cursor Invisibles +
| [ ] CodeRunner ☾  | + element table: Element | FG | BG | B | I | U                              +
| [ ] ... ☼         | +                                                                          +
+-------------------+ + [+][−]  Scope selector: [____________]                  [x] Show all     +
[+][−][⋯]             + caption: Use ⌃⌘P in the editor to show the active scope                   +
                      + (•) Use the chosen theme for all appearances                               +
                      + ( ) Use separate themes for different appearances:  Light: [▾]  Dark: [▾] +
```

- **Theme list** (ListManager): SectionHeader `Color Themes`; rows with a 14 x 14 swatch of the theme background (1pt `border`), name, and a trailing `sun` or `moon` 12 `muted-foreground`. `+` menu: `New Light Theme`, `New Dark Theme` (tooltip `Create a new theme`); `−` (`Delete the selected theme`); `ellipsis-circle` (`More theme actions…`): `Duplicate`, `Rename`, `Invert`, `Delete…`, separator, `Import…`, `Export…`, `Get More Themes`, separator, `Reset…`, `Reset All…`, `Show in Finder`. Alerts: `Are you sure you want to delete the theme "<name>"?` / `The theme file will be moved to the Trash.`; `Are you sure you want to reset the theme "<name>" to the default?` / `Any changes you have made to this theme will be erased.`; `Are you sure you want to reset all color themes to the defaults?`.
- **Wells:** six ColorWells `well` in one row, 16 apart, caption below each: `Foreground`, `Background`, `Selection`, `Current Line`, `Cursor`, `Invisibles`.
- **Element table:** columns `Element` (flex), `FG`, `BG` (ColorWell `chip` cells, 40), `B`, `I`, `U` (Checkboxes, 28; header tooltips `Bold`, `Italic`, `Underline`). The Element cell draws its name in the element's own foreground, background, weight, italic and underline on the theme's background, in the code font 12. Height flex (min 200). `Show all` Checkbox lists the default element names too (`Comment`, `String`, `Number`, `Built-in constant`, `User-defined constant`, `Variable`, `Keyword`, `Storage`, `Class name`, `Inherited class`, `Function name`, `Function argument`, `Tag name`, `Tag attribute`, `Library function`, `Library constant`, `Library class/type`, `Library variable`, `Invalid`, plus the console and ANSI items).
- **Below the table:** `+` (`Create a new theme element…`), `−` (`Delete the selected theme element`), `Scope selector:` TextField (code font), `Show all` Checkbox at the right; caption `Use ⌃⌘P in the editor to show the active scope`.
- **Appearance group:** RadioGroup `Use the chosen theme for all appearances` / `Use separate themes for different appearances:` (default) with `Light:` and `Dark:` PopupButtons (disabled and dimmed when the first radio is selected); Checkbox `Match the app appearance to the theme` (design addition: exposes the `MatchAppearanceToTheme` default).
- **Live:** edits are applied to every open editor and console within 100 ms (debounced), and the ThemeResolver (3.4) rewrites the editor-scope tokens. A Banner `Low contrast between text and background.` (warning) appears above the table when `F` on `B` is below 4.5:1.
- **Scope panel (⌃⌘P):** a small Popover anchored at the caret listing the scope stack (display names), following the mouse while open; each row has a ghost sm Button `Edit Color` (existing rule) or `Set Color` (creates a rule and opens this pane on that element with its well active). Empty: `No Scopes`.

#### 8.13.4 Key Bindings

Top row (56): `Key Bindings Set:` PopupButton (`Default`, `Vim`, user sets; separator; `Import ScratchRunner Key Bindings…`, `Export ScratchRunner Key Bindings…`, `Import macOS KeyBindings.dict File…`) + Buttons `Duplicate`, `Rename`, `Delete` (disabled for read-only sets). For the fourth segment this row is replaced by the text `Commands listed under "Global Compound Commands" are active independently of the chosen key binding set.` and a `Reset…` button. Then the Segmented capsule md: `Menu Items`, `Commands`, `Compound Commands`, `Global Compound Commands`. Main area: `h_resizable` split, left table (min 330: enabled Checkbox 24, `Title` flex, `Reset` ghost xs button 56, `Keyboard Shortcut` KeyRecorder 120), right CodeField (JSON, `source.json` grammar) shown for compound commands, EmptyState compact otherwise (`Select a compound command to edit its JSON.`). Bottom bar (36): SearchField sm 220 (placeholder `Filter`, accessible hint `To filter by keyboard shortcut, use @ for command, ^ for control, ~ for option, $ for shift`), `+` (`Add a new keyboard shortcut`), `−` (`Delete the selected keyboard shortcut`), help IconButton (anchor `Compound Commands`), and `Invalid JSON` in `destructive` caption when the JSON fails to parse.

**Trigger editor** (Popover from the shortcut cell, 320): sections `Key Events`, `Mouse Events`, `Other`; Buttons `Add…` and `Record` (message `Click "Record" to begin recording a sequence of keystrokes. To end the recording, click anywhere on-screen.`; state labels `Waiting`, `Recording`, `Sequence…`); mouse triggers `Click`, `Double Click`, `Selection Change`, `Any Key` with the note `(Optionally hold modifier keys when selecting)`; timing radios `Perform before the triggering key or mouse event`, `Consume the triggering key or mouse event`, `Perform after the triggering key or mouse event`; empty cell hint `Click to set a trigger`. Error alert: `An error occurred when performing <command>.` / `Check that all commands called have arguments that are valid and of the correct number and type. Configure key binding commands in ScratchRunner Preferences > Key Bindings > Compound Commands.`

#### 8.13.5 Shell

FormRow layout, label column 140. RadioGroup: `Automatically determine PATH environment variable` with a `refresh` IconButton sm at the row end (tooltip `Refresh PATH`, enabled only for the automatic choice) / `Use custom PATH:`; CodeField (437 x 82, shell grammar; read-only and showing the determined path while Automatic is chosen); Checkbox `Invoke shell in login mode when running code` with caption `When invoked in login mode, the shell executes startup scripts such as ~/.profile.`; `Shell:` TextField 150 (placeholder `/path/to/shell`) with caption `Using a shell other than bash is not recommended and may cause undesirable behavior.`; `Shell Variables:` Table (`Variable` 144, `Value` 249, both editable, sorted by name) with `+` (`Add a shell variable`), `−` (`Delete the selected shell variable`), Button `Reset` (`Reset all shell variables`; alert `Reset All Variables`). Defaults: `TERM = dumb`, `TERM_PROGRAM = ScratchRunner`, `LC_CTYPE = UTF-8`.

### 8.14 Alerts catalog

Native alerts (`alert_dialog()`): at most two buttons in a row (the last regular one is the default), more are stacked; the destructive result uses the `destructive` role; an alert never has an accessory (those are Sheets, 7.19). Titles state the condition; bodies add new information only.

| Situation | Title | Body | Buttons |
|---|---|---|---|
| Close window with changes | `This window has <n> documents with unsaved changes. Would you like to review these changes before closing?` | | `Review Changes…`, `Discard Changes` (destructive), `Cancel` |
| Close one document | `Do you want to save the changes made to "<name>"?` | `Your changes will be lost if you don't save them.` | `Save`, `Don't Save`, `Cancel` |
| Close running document | `"<name>" is currently running. Are you sure you want to close it?` | `Closing the document will stop the run.` | `Close`, `Cancel` |
| Quit with changes | `ScratchRunner has <n> document(s) with unsaved changes. Would you like to review <them or it> before quitting?` | | `Review Changes…`, `Discard Changes`, `Cancel` |
| Delete files | see 8.4.1 | | `Delete`, `Cancel` |
| Large file | see 8.4.1 | | `Open Anyway`, `Cancel` |
| Large console output | see 8.7 | | `Stop`, `Continue Run` |
| Change default run command / compile flags | see 8.10 | | `Make Default`, `Cancel` |
| Restart required | `App restart required before this change takes effect.` | `Your new setting will take effect the next time ScratchRunner is launched.` | `OK` |
| Encoding conflict | `How would you like to apply the new encoding?` | | `Reinterpret`, `Convert`, `Cancel` |
| Characters not representable | `File Encoding` | `You are trying to insert characters which cannot be handled by the current encoding without losing information.` | `Insert`, `Promote to UTF-8` |
| Locked document | `The document cannot be saved because it is locked.` | `The document will be copied to a temporary folder for running.` | `OK` |
| Python missing | `Python is not installed on your system.` | `It is recommended that you download and install from python.org. Alternatively, you can install Apple's command line developer tools in the dialog that appears.` | `Download at python.org`, `Cancel` |
| Developer tools missing | `The command line developer tools are required to run this document.` | `To install Command Line Tools, run the command in the Terminal: xcode-select --install` | `OK` |
| Debugger start failure | see 8.9 | | `Try Again`, `Run Without Debugging`, `Show Debugger Log` |
| Save failure | `Couldn't save "<name>".` | `<system reason>` | `OK` |
| Auto-fix prompt | `Would you like to apply this fix automatically on run?` | | `Fix Automatically`, `Don't Fix Automatically` |
| Reset (language, theme, all) | see 8.13 | | `Reset`, `Cancel` |

### 8.15 About, Help, Attributions

- **About** (`About ScratchRunner`): a 320-wide window, hidden title bar (close button only), centered column with 24 padding: `app-mark` 40 in an 80 x 80 `primary` tile (`radius-xl`, glyph `primary-foreground`), 12 gap, `ScratchRunner` (`title`), `Version <semver> (<build>)` (`caption` `muted-foreground`, selectable), 16 gap, `Run code in any language, instantly.` (`body` `muted-foreground`, centered, max 240), 16 gap, Buttons `link` md `Software Attributions`, `Visit ScratchRunner Website`; copyright line `caption`.
- **Help** (`ScratchRunner Help`, ⌘?): 760 x 560 window; left topic list 200 (SectionHeader `Topics`; rows `Key Bindings`, `Run Command`, `Environment Variables`, `Compile Script`, `Scope Selectors`, `Templates`, `Multiple Selections`, `Compound Commands`, `Run Issues`), right a read-only rich text view (`text_view`, markdown) with max width 640 centered, SearchField md above it (`Search Help`). Anchors let the `?` buttons open a topic. Back/Forward in the window's top row.
- **Software Attributions:** a Help-style window listing licences, `text_view`, 560 x 480.

### 8.16 Empty, first-run and error states

| State | Where | Presentation / copy |
|---|---|---|
| First launch | Editor | An untitled Shell Script document with the sample `#!/bin/bash` newline `echo "Welcome to ScratchRunner!"`, caret at the end; sidebar rooted at `~/`; no welcome window |
| New document | Editor | The language's default template, `<##>` selecting the insertion point |
| No open document | Editor column (all tabs closed) | EmptyState: `file-plus` 32, title `No Open Document`, message `Press ⌘N to create a file or ⌘O to open one.`, buttons `New File` (`default`), `Open…` (`default`) |
| Empty sidebar folder, filter, search | 8.4 | as listed there |
| Console idle | 8.7 | caption |
| Docs: none / loading / error / offline / moved | 8.8 | as listed there |
| QuickNav no results | 8.12 | `No Results` |
| Symbol list | 8.6.5 | `No Symbols`, `Loading Symbols…` |
| Language list empty | Preferences | `No Items` |
| External change of an open file | Editor | Clean document: silent reload plus a Toast `Reloaded from disk`; dirty document: Banner (warning) above the editor `"<name>" was changed on disk while you have unsaved changes.` buttons `Reload from Disk`, `Keep My Version` |
| Language server failed | Status bar | Warning item `<Language> · <message>` with `Show Error` (8.6.5); editing continues |
| Run failed to start | Status bar + console | `Invalid Run Command`; console line from the language's `Errors` table for exit 127 |
| Missing external tool for a diagnostic | Banner (warning) in the editor top | `<Tool> isn't installed.` button `Install…` (prompt text from the language's `Installer`) |
| Network offline (docs, web console) | 8.8 / 8.7 | `wifi-off` state with `Try Again` |
| Permission denied on save or read | Alert (8.14 `Couldn't save`) or sidebar banner | `Couldn't read this folder.` |

Errors say what happened and the next action; success is not announced when the result is visible (a saved document simply stops being edited).

### 8.17 Context menus and the menu bar

The menu bar, Dock menu and every context menu are native (`set_application_menu`, `popup_menu_at`); their structure and shortcuts are specified by the command layer, not redrawn here. The design requires only: title-case menu titles as CodeRunner has them (`New from Template`, `Find and Select Next`), shortcut glyphs from `key_label`, dynamic titles (`Show/Hide File Navigator`, `Close Tab` / `Close Document` / `Close Window`, `Compile` / `Run Diagnostics`), checkmarks bound to preferences, and the Option-revealed alternates. Menu bar top level: ScratchRunner, File, Edit, Find, Text, Run, Diagnostics, View, Window, Help.


## 9. Motion

Motion explains a change: where something came from, where it went, what the press did, what is live. It is short, native (UI thread, `AnimatedValue`, transitions, motions, `LayoutAnimation`, popup starting/ending styles) and never needed to understand state. Desktop pointer feedback stays under 200 ms; leaving is faster than entering. ScratchRunner is a tool used in a loop, so: typing, scrolling, caret movement, console output and any value that updates many times a second never animate.

### 9.1 Motion tokens

Defined once in `ui/motion.rs`, read from `ui/theme.json` (`motion-*`, milliseconds) so they can be tuned in one place. Components take no raw durations.

| Token | Value | For |
|---|---|---|
| `instant` | 100 ms | Hover wash, color change, a disabled/enabled dim, ghost breakpoint |
| `fast` | 150 ms | Press and release, control state, toggle on/off, menu/popup leaving, tooltip |
| `base` | 220 ms | Moving into place: a selection sliding, an icon swap, a panel or sidebar collapsing, a bar appearing, a pane cross-fade, window resize |
| `slow` | 300 ms | Reveals: a popup, popover, sheet or QuickNav opening, a toast arriving |
| `slower` | 450 ms | Unhurried entrances (empty states, the first-run document), indeterminate progress loop, value-change flash |
| `stagger` | 40 ms | Row step of a list wave (cap: 10 rows, so at most 400 ms) |

| Kind | Value |
|---|---|
| Easing, entering | ease-out (`.ease_out()`; `Easing::EaseOut`) |
| Easing, between two places | ease-in-out (`.ease_in_out()`; `LayoutCurve::EaseInEaseOut`; `Easing::EaseInOut`) |
| Easing, leaving | ease-in (`.ease_in()`) |
| Soft settle (presses) | `cubic-bezier(0.22, 1, 0.36, 1)` (`.ease_bezier(0.22, 1.0, 0.36, 1.0)`) |
| Distances | 4 (a label or pill rising), 8 (a popup, popover, toast sliding from its trigger or edge), 12 (a sheet dropping from the title bar), 16 (rows of a wave) |
| Scales | press 0.97 (icon buttons 0.95); popup, popover, QuickNav enter from 0.97; breakpoint pop from 0.6 |
| Springs | `panel`: stiffness 320, damping 36, mass 1 (critically damped: 2 x sqrt(320) = 35.8) for what the user drags and releases (split handles, tab drops); `pop`: stiffness 180, damping 11 for the one playful cue (a breakpoint set) |
| Opacity | enter from 0, leave to 0; disabled 45%; dimmed (inactive window) 60% |

**Reduced motion:** `reduce_motion()` (true when macOS "Reduce motion" is on; NSH reads `accessibilityDisplayShouldReduceMotion`) makes every transition instant (`transition_*` collapse by themselves; motions land at their end by themselves), every `LayoutAnimation` is skipped (`animate_layout` is not called), springs become `set`, popups appear at once, and slides become cross-fades under `fast`. Spinners keep rotating (progress cue, `play_ignoring_reduce_motion`) and the indeterminate bar shows a static segment with a label. The shake of an invalid prompt becomes a 1pt `destructive` outline flash. Every state is understandable with all motion off.

### 9.2 Recipes (what each tool means here)

| Recipe | NSH call | Tokens |
|---|---|---|
| State wash (hover, selected, enabled/disabled, toggled) | `.transition_colors().duration(instant).ease_out()`; opacity changes `.transition_opacity()` | `instant` (hover), `fast` (toggle, press release) |
| Press | `.transition_transform().duration(fast).ease_bezier(0.22, 1.0, 0.36, 1.0)` + `.active(\|s\| s.scale(0.97))` wrapped in `.when(!reduce_motion(), ..)`; the wash deepens through `.active` colors | `fast`, 0.97 / 0.95 |
| Glyph swap (Run state glyph, eye / eye-off, chevron direction, check) | The old icon shrinks to 0.25 and fades while the new one grows from 0.25: two stacked icons driven by one `animated_value` (`timing`, ease-in-out); rotation for chevrons (`bind_rotate`, `timing`) | `base` |
| Label change | New text fades in rising 4pt (`motion().entering` with a custom `MotionSpec`: opacity 0..1, translateY 4..0, ease-out) | `fast` |
| One sliding selection | One selection view (raised pill or underline) bound to an `animated_value` x (and width); `timing(to, base, EaseInOut)`; on a drag it follows the pointer 1:1 and on release a `panel` spring settles it | `base` |
| Popup / popover / completion opening | `popup_starting_style` (opacity 0, scale 0.97, translateY toward the trigger 8) and `popup_ending_style` (opacity 0, scale 0.97, no slide), `transform-origin` at the corner nearest the trigger; open `slow`, leave `fast` | `slow` / `fast` |
| Sheet / QuickNav | The dialog's `motion().entering` (opacity 0..1, translateY -12..0 for sheets or 8..0 for QuickNav, scale 0.97..1) with the scrim fading under it; leaves faster and without overshoot | `slow` in, `fast` out |
| Panel, bar, sidebar, console, docs, find bar, tab bar show/hide | `animate_layout(LayoutAnimation::all(LayoutTransition::new(LayoutCurve::EaseInEaseOut, base)))` called right before the state change by the user's action (never for a window resize) | `base` |
| List wave (only when a list is replaced as a whole: search results arriving, QuickNav results after a provider finishes the first batch) | `motion().entering(FadeInUp 16 → 4pt)` per row with delay `min(index, 10) x stagger`; recycled cells, later rows and scroll never replay it | `fast` per row |
| Content arriving that is not a list | Fade in with a 4pt rise (`FadeInUp` at 4pt), once | `slow` |
| Value change flash (debugger variable changed) | `animated_value` 1 → 0 `timing(slower, ease-out)` interpolating the value text color from `warning` to `foreground` (`interpolate_colors`) | `slower` |
| Spinner | `MotionPreset::Spin`, linear, 1000 ms loop, ignoring reduce motion | |
| Attention (invalid prompt) | `MotionPreset::Shake` (500 ms) on the sheet | |

### 9.3 Layout changes the user causes (all use `base` + ease-in-out, all instant under reduced motion)

Sidebar, docs panel, console show/hide and position change; console enlarging for the debug pane; find bar open/close (push); replace row open/close; tab bar and toolbar band hide/show; docs expand; Run Settings banner strip (`Run command is specific to this file.`) appearing; sidebar Filter/Search mode swap (cross-fade `fast`, no layout slide); Preferences pane switch (window resize with `set_bounds(animated)` plus content cross-fade); disclosure of Diagnostics rows; QuickNav height as the result count changes (`fast`).

### 9.4 Motion inventory (required)

Notation: **Tool** names the NSH tool, **Tokens** the values, **RM** what reduced motion does. "none" is always with a reason. Every line must be verified in the running app before the screen is done; the status column is filled in during review (section 12.2).

#### Main window shell

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Hover on any toolbar, bar or row control | State wash | `instant`, ease-out | instant | [ ] |
| Press on any button or icon button | Press recipe | `fast`, 0.97 (0.95 icon) | instant, no scale | [ ] |
| Run button enabled / disabled | `transition_opacity` | `fast`, 45% | instant | [ ] |
| Stop button enabled / disabled | `transition_opacity` | `fast` | instant | [ ] |
| Language popup opens (native menu) | native | n/a | native | [ ] |
| Run button hold-menu opens | native | n/a | native | [ ] |
| Run-button `chevron-down` affordance on hover | `transition_opacity` | `instant` | instant | [ ] |
| View toggle on / off (sidebar, console, docs) | `transition_colors` of the wash and icon tint | `fast` | instant | [ ] |
| Back / Forward enabled / disabled | `transition_opacity` | `instant` | instant | [ ] |
| Sidebar, docs, console shown / hidden | `animate_layout` | `base`, ease-in-out | instant | [ ] |
| Console moved bottom <-> right | `animate_layout` | `base` | instant | [ ] |
| Split handle hover / drag | line color wash; drag follows pointer 1:1 (no easing); on release past the collapse threshold a `panel` spring settles to collapsed | `instant`; spring `panel` | instant | [ ] |
| Window title changes (tab switch, edited suffix appears) | New title fades with a 4pt rise | `fast` | instant | [ ] |
| Toolbar band shown / hidden | `animate_layout` | `base` | instant | [ ] |
| Inactive window dims the chrome | `transition_opacity` / colors | `base` | instant | [ ] |

#### Tab bar

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Hover on an unselected tab | wash `tab-hover` | `instant` | instant | [ ] |
| Close button appears on hover, dot becomes close | glyph swap | `fast` | instant | [ ] |
| Selected tab changes | One sliding raised pill (animated x and width) | `base`, ease-in-out | jumps | [ ] |
| New tab opens | The tab grows from 0 width (layout animation) and fades in; the selection pill slides to it | `base` | instant | [ ] |
| Tab closes | The tab fades out then neighbors close the gap (`animate_layout`) | `fast` + `base` | instant | [ ] |
| Tab dragged to reorder | Follows the pointer 1:1; neighbors slide to make room (`animate_layout`); release settles with a `panel` spring | `base`; spring `panel` | jumps | [ ] |
| Tab starts / stops running (icon slot becomes a spinner) | glyph swap into the 12 spinner | `base` | instant swap, spinner still spins | [ ] |
| Edited marker toggles | glyph swap | `fast` | instant | [ ] |
| Overflow button appears | fade | `fast` | instant | [ ] |
| Tab bar shown / hidden | `animate_layout` | `base` | instant | [ ] |

#### Sidebar (navigator and search)

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Row hover | wash | `instant` | instant | [ ] |
| Row selected | color wash (no slide: rows are virtualized and move when folders expand) | `instant` | instant | [ ] |
| Disclosure expands / collapses | chevron rotate 0 to 90 (`bind_rotate`, `timing`); children appear by `animate_layout` | `base`, ease-in-out | instant | [ ] |
| Navigator <-> Search mode | cross-fade of the two bodies; header controls swap by glyph swap | `fast` | instant | [ ] |
| Filter results change while typing | none: updates every keystroke (a value that changes many times a second) | | | [ ] |
| Search results arrive | List wave on the first batch only (rows fade in rising 4pt); later batches append without motion | `fast`, `stagger` 40, cap 10 | instant | [ ] |
| Search progress bar | indeterminate segment loop | `slower` loop | static segment | [ ] |
| Folder row as drop target | outline wash | `instant` | instant | [ ] |
| Path popup drop hover label change | label fade-rise | `fast` | instant | [ ] |
| Inline rename starts | field fade-in over the label | `instant` | instant | [ ] |
| Row deleted / created | `animate_layout` (rows close the gap or open it) | `base` | instant | [ ] |

#### Editor, gutter, find, status bar

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Typing, caret movement, scrolling, selection | none: direct manipulation and per-keystroke state | | | [ ] |
| Current line highlight moves | none: follows the caret every keystroke | | | [ ] |
| Breakpoint ghost on gutter hover | opacity wash | `instant` | instant | [ ] |
| Breakpoint set | pop: scale 0.6 to 1 with the `pop` spring and fade | spring `pop` | instant, no scale | [ ] |
| Breakpoint removed | scale 1 to 0.6 with fade | `fast`, ease-in | instant | [ ] |
| Execution line moves (stepping) | `debug-line` tint cross-fades from the old line to the new; the arrow slides vertically between lines when within 8 lines, else fades | `fast` (tint), `base` (arrow) | instant | [ ] |
| Diagnostic annotation appears (pref `Animate diagnostic messages`) | `FadeInUp` at 4pt; the underline and line tint fade | `fast` | instant | [ ] |
| Diagnostic annotation expands / collapses | height by `animate_layout` on the overlay | `fast` | instant | [ ] |
| Diagnostics cleared | fade out | `fast` | instant | [ ] |
| Completion panel opens / closes (pref `Animate completion panel`) | popup start / end styles (scale 0.97, opacity, 4pt) | `fast` in (completion is latency-sensitive), `instant` out | instant | [ ] |
| Completion selection moves | none: keyboard stepping through rows (instant highlight); the panel's doc popover fades `fast` | | instant | [ ] |
| Hover popover (diagnostic, variable) | popup styles | `slow` in, `fast` out | instant | [ ] |
| Find bar opens / closes | `animate_layout` (editor pushed down) and bar content fade | `base` | instant | [ ] |
| Replace row opens / closes | `animate_layout` | `base` | instant | [ ] |
| Find match count changes (`3 of 12`) | none: value updating per keystroke | | | [ ] |
| Invalid regex | field outline color wash; trailing `Regex Invalid` label rise | `fast` | instant | [ ] |
| Current match changes | none: editor scroll jumps (`go_to_line`) | | | [ ] |
| Status bar run state changes (icon and text) | glyph swap for the icon; text label rise | `base`, `fast` | instant | [ ] |
| Status bar CPU / Memory live values | none: updates ~2 per second | | | [ ] |
| `Show the Console` item appears | fade, rise 4pt | `fast` | instant | [ ] |
| Symbol / indentation popover opens | popup styles | `slow` / `fast` | instant | [ ] |
| Language-server warning item appears | fade | `fast` | instant | [ ] |

#### Console and debug pane

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Toolbar control hover / press | State wash / press recipe | `instant` / `fast` | instant | [ ] |
| Output lines appended | none: high-frequency data | | | [ ] |
| Console cleared | none: immediate (the action's result is visible) | | | [ ] |
| Filter applied / output popup changed | none: instant re-filter | | | [ ] |
| Console auto-shows when output arrives | `animate_layout` | `base` | instant | [ ] |
| `Scroll to bottom` button appears / disappears | opacity + 4pt rise | `fast` | instant | [ ] |
| Console type switch (Terminal <-> Web) | cross-fade of the two bodies | `fast` | instant | [ ] |
| Web resource bar appears (Web mode) | `animate_layout` | `base` | instant | [ ] |
| Live Updating toggled | wash and tint | `fast` | instant | [ ] |
| Debug pane appears / disappears | `animate_layout` (console enlarges to 60%) | `base` | instant | [ ] |
| Debugger buttons enabled when paused | opacity | `instant` | instant | [ ] |
| Continue / Pause glyph | glyph swap | `base` | instant | [ ] |
| Stack frame selection | wash | `instant` | instant | [ ] |
| Variable expand / collapse | chevron rotate, `animate_layout` of children | `base` | instant | [ ] |
| Variable value changed by a step | value color flash `warning` to `foreground` | `slower` | no flash; changed values keep a `warning` marker dot until the next step | [ ] |
| Variable row hover actions appear | opacity | `instant` | instant | [ ] |

#### Docs panel

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Source chip selected | One sliding selection fill behind the chips | `base` | jumps | [ ] |
| Automatic Lookups toggled | tint wash | `fast` | instant | [ ] |
| Page loading | 2pt indeterminate bar, fades out when the page is ready | loop `slower`; fade `fast` | static segment | [ ] |
| Page content arrives | none: the web view paints its own page; the wrapper does not animate over it | | | [ ] |
| Error / offline / empty state appears | EmptyState fade with 4pt rise | `slow` | instant | [ ] |
| Expand grabber appears on hover | opacity | `instant` | instant | [ ] |
| Panel expands over the editor | `animate_layout` | `base` | instant | [ ] |
| TOC popup | native | n/a | native | [ ] |

#### Sheets, QuickNav, prompts, popovers, toasts

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Run Settings / prompt sheet opens | dialog motion: translateY -12 to 0, opacity, scale 0.97 to 1; scrim fades | `slow`, ease-out | instant | [ ] |
| Sheet closes | opacity and scale to 0.97, scrim fades | `fast`, ease-in | instant | [ ] |
| `Revert` / `Make Default` strip appears in Run Settings | `animate_layout` | `base` | instant | [ ] |
| Prompt rejects input | `MotionPreset::Shake`, field outline wash | 500 ms | outline flash only | [ ] |
| QuickNav opens | dialog motion (8pt rise, scale 0.97), scrim | `slow` | instant | [ ] |
| QuickNav results arrive | first batch list wave; height by `animate_layout` | `fast` per row, `stagger` 40; height `fast` | instant | [ ] |
| QuickNav selection moves (Up / Down) | One sliding selection fill | `instant` | jumps | [ ] |
| QuickNav spinner | spin loop | | still spins | [ ] |
| Popover opens (options, color, trigger editor, scope panel) | popup styles from the trigger corner | `slow` / `fast` | instant | [ ] |
| Tooltip | native-like fade | `fast` in after the 500 ms delay, `instant` out | instant | [ ] |
| Toast arrives / leaves | slides 8pt up from the bottom edge and fades; leaves fading | `slow` in, `fast` out | cross-fade only | [ ] |
| Banner appears / dismissed | `animate_layout` | `base` | instant | [ ] |
| Native alert | native | n/a | native | [ ] |

#### Preferences, Help, About

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| Pane strip hover / press | wash / press | `instant` / `fast` | instant | [ ] |
| Pane selected | One sliding selection fill under the icons | `base` | jumps | [ ] |
| Pane switch | Window resize (`set_bounds` animated) and content cross-fade (sibling sections cross-fade, never slide) | `base` | instant | [ ] |
| Text Editing category (segmented) | sliding pill; the rows cross-fade | `base` / `fast` | jumps | [ ] |
| Languages sub-tab | sliding pill; content cross-fade | `base` / `fast` | jumps | [ ] |
| Parent checkbox toggles dependents | dependents' opacity (45% when disabled) | `instant` | instant | [ ] |
| Dependent child appears (inline) | none: children are always laid out | | | [ ] |
| Language / theme list selection | wash | `instant` | instant | [ ] |
| Theme applied | editor-scope colors and the chrome transition cross-fade; all `transition_colors` | `base` | instant | [ ] |
| Shortcut recorder arms / records | outline `ring` wash; the `record` glyph pulses (`MotionPreset::Pulse` loop 500 ms) while recording | `fast`; loop | outline only | [ ] |
| Diagnostics row expands | `animate_layout` | `base` | instant | [ ] |
| Switch toggled (if used) | thumb translate + track color | `fast` | instant | [ ] |
| Help topic switch | content cross-fade | `fast` | instant | [ ] |
| About opens | window default | | | [ ] |

#### Empty and first-run

| Change | Tool | Tokens | RM | Verified |
|---|---|---|---|---|
| `No Open Document` appears | EmptyState fade with 4pt rise | `slower` | instant | [ ] |
| First-run document shown | none: it is the document | | | [ ] |


## 10. Keyboard, focus and accessibility

Every action is reachable by keyboard and by VoiceOver. Mouse-only paths do not exist: each hover action also lives in a menu, a shortcut or a context menu. Accessibility strings are English even when the UI is localized (guide: names may remain English); an item's own label stays as is.

### 10.1 Model

- **Regions.** The main window has these focus regions, in order: Toolbar, Tab bar, Sidebar, Editor (with Find bar), Console (with Debug pane), Documentation panel, Status bar. **⌃F6 / ⌃⇧F6** move to the next / previous visible region (the Cocoa convention); focus lands on the region's primary control (selected tab, tree, editor, console text, docs search field, first status item). Hidden panels are skipped.
- **Tab / Shift-Tab** move between the *controls* of the focused region (each capsule group, list, tree and table is one Tab stop; arrows move inside it). Inside the Editor and the Console text, Tab is text (indent / completion accept); use ⌃F6 to leave. Inside text fields Tab moves to the next control.
- **Return / Space** activate a button, toggle a checkbox/switch, open a popup; Return commits the default button of a sheet; Space also toggles list checkboxes.
- **Esc ladder** (topmost first): native menu / popup, popover or completion list, QuickNav, sheet or prompt (= Cancel), find bar (when focus is in it: closes, focus to the editor), search filter text (clears), then in the editor Esc keeps its editor meaning (single selection); in the sidebar, console or docs with nothing to dismiss Esc returns focus to the editor.
- **Focus is always visible.** Keyboard-moved focus shows the 2pt `ring` outline (`focus_visible`); list rows show it as an inset ring; the editor shows its caret; a click never shows the ring. Focus never lands on something hidden: when a focused panel collapses focus moves to the editor; when a tab closes to the next tab; when a sheet closes back to its trigger or the control that was focused.
- **Opening a surface moves focus into it:** sheet (first field, or the default button when it has no field), QuickNav (field), find bar (find field, selected text), Run Settings (Run Command), popover (first control), menu (first item). Closing returns focus to the trigger.
- **Reserved shortcuts** (cannot be bound by an app): ⌘Q, ⌘H, ⌥⌘H, ⌘M, ⌃⌘F. Menu key equivalents take priority over editor keymap entries that use the same chord (CodeRunner semantics), so ⌘L is Go to Line and ⌘D is Duplicate Line; the command layer owns that resolution.
- **Text size:** rows and controls use `min_h`, text scales with the user's size (`font_scale`); panels' minimums account for it; nothing truncates a label without a tooltip.

### 10.2 Shortcut map (from the menus)

| Group | Commands |
|---|---|
| App | Preferences… ⌘, · Edit Languages… ⌥⌘, · Help ⌘? · Hide ⌘H · Quit ⌘Q |
| Files and windows | New Window ⌘N · New Tab ⌘T · New from Clipboard ⌃⌘V · Open… ⌘O · Open Quickly… ⇧⌘O · Go to Current Document ⌥⌘J · Close Tab/Document/Window ⌘W · Close Window ⇧⌘W · Close Other Tabs ⌥⌘W · Close Tab Without Saving ⌃⌥⌘W · Save ⌘S · Save As… ⇧⌘S · Page Setup… ⇧⌘P · Print… ⌘P |
| Tabs and history | Select Previous Tab ⇧⌘[ (⌘{) · Select Next Tab ⇧⌘] (⌘}) · Move Tab Left/Right ⌥⌘{ / ⌥⌘} · Move Tab to New Window ⌥⌘N · Tab 1 to 9 ⌘1 to ⌘9, Tab 10 ⌘0 · Go Back ⌃⌘← · Go Forward ⌃⌘→ |
| Edit | Undo ⌘Z · Redo ⇧⌘Z · Cut ⌘X · Copy ⌘C · Copy with Colors ⌥⌘C · Paste ⌘V · Paste and Preserve Formatting ⌥⇧⌘V · Select All ⌘A |
| Find | Find… ⌘F · Find and Replace… ⌥⌘F · Find Next ⌘G · Find Previous ⇧⌘G · Find and Select Next ⌘E · Find and Select Previous ⇧⌘E · Find and Select All ⌃⌘E · Find and Select Word ⌥⇧⌘E · Find Pattern… ⌃⌥⌘P · Select Next Result ⌥⌘G · Select Previous Result ⌥⇧⌘G · Jump to Next Selection ⌘J · Jump to Previous Selection ⇧⌘J · Search in File System… ⌥⇧⌘F · Search for Selected Text in File System ⇧⌘F · Search for Selected Text in Current Document ⌃⇧⌘F |
| Text | Show Completions ⎋ / ⌃Space · Look Up Current Symbol ⌘I · Auto-Indent Selection ⌃I · Shift Right ⌘] · Shift Left ⌘[ · Un/Comment Lines ⌘/ · Select Column Up/Down ⌃⇧↑ / ⌃⇧↓ · Split Selection By Lines ⌃⇧↩ · Single Selection ⌃J · Select Line ⌃S · Select Word ⌃W · Select to Delimiters ⌃⌥↑ ↓ ← → · Duplicate Line ⌘D · Delete Line ⇧⌘⌫ · Swap Lines Up/Down ⌥⌘↑ / ⌥⌘↓ · Go to Line… ⌘L · Show Invisibles ⌥⌘I · Show Page Guide ⌥⌘P · Show Color Scopes ⌃⌘P · Show Fonts ⇧⌘T · Bigger ⌘+ · Smaller ⌘- |
| Run | Run ⌘R · Run and Debug ⌥⌘R · Run Without Debugging ⌃⌘R · Run Diagnostics / Compile ⌘B · Stop ⌘. · Edit Run Settings… ⇧⌘R · Clear Console ⌘K · Select Language… ⌥⌘L |
| Debugger | Toggle Breakpoint at Current Line ⌘\ · Clear Breakpoints ⌘Y · Pause / Continue Execution ⌃1 · Step Over ⌃2 · Step In ⌃3 · Step Out ⌃4 |
| Diagnostics | Apply Next Fix ⌥⌘; · Apply All Fixes ⌥⇧⌘; · Jump to Next / Previous Diagnostic ⌥⌘→ / ⌥⌘← · Next / Previous Error ⌥⇧⌘→ / ⌥⇧⌘← |
| View | Show/Hide File Navigator ⇧⌘H · Console ⇧⌘C · Documentation View ⇧⌘I · Tab Bar ⌥⇧⌘T · Toolbar ⌥⌘T · Enter Full Screen ⌃⌘F |
| Focus (this design) | Next / previous region ⌃F6 / ⌃⇧F6 · Cycle windows ⌘` (system) |

Every shortcut is shown in its menu item (native) and in the tooltip of the matching toolbar control as plain `muted-foreground` text or a Kbd. Shortcuts are rebindable in Key Bindings; the UI always shows the current binding.

### 10.3 Tab order per surface

Numbers are Tab stops; arrow groups inside a stop are in 10.4.

1. **Main window.** (1) Toolbar: Language popup, Run group, Back/Forward, View toggles (4 stops). (2) Tab bar: the selected tab (stop), `plus`, overflow. (3) Sidebar: path popup, root Back/Forward group, search toggle, tree, footer `+`, `folder-plus`, `go-to-folder`, Filter. In Search mode: where popup, what popup, `Options…`, find controls, results outline, prev/next, `Done`. (4) Editor: find bar (if open: operation, field, matching, Ignore Case, previous, next, Done; row 2: replace field, replace segments), then the editor itself. (5) Console: toolbar controls left to right, then (debugging) Stack list, Variables list, then the console text. (6) Docs: search field, back/forward, Automatic Lookups, actions, source chips (one stop), page, TOC popup. (7) Status bar: left items that are buttons, then symbol, indentation, position.
2. **Run Settings sheet.** Run Command, help, Revert, Make Default (when shown), Arguments, Compile Flags (when shown), its Revert / Make Default, Program Input, Close. Focus starts in Run Command; Esc and Return close.
3. **Prompt sheet.** Field, Cancel, commit.
4. **QuickNav.** The field is the only Tab stop; the results are reached with Up/Down while focus stays in the field (`aria`-style active descendant: the selected row is exposed).
5. **Preferences.** (1) Pane strip (one stop, Left/Right selects the pane). (2) The pane's content in reading order: Text Editing category segmented; each FormRow's control in order, label to control; lists, tables, then their `+ − ⋯` strips. Languages: list, strip, sub-tab segmented, tab content. Themes: theme list, strip, wells (six stops), element table, `+`, `−`, scope selector, Show all, radios, popups. Key Bindings: set popup, Duplicate, Rename, Delete, kind segmented, table (rows: enabled checkbox, shortcut recorder are cells reached with Right arrow), JSON editor, filter, `+`, `−`, help.
6. **Alerts and native menus** follow the system.
7. **Help.** Topic list, search field, content.

### 10.4 Arrow-key groups

| Group | Keys |
|---|---|
| Capsule group (toolbar, bars) | Left/Right between members (roving), Home/End, Tab leaves |
| Segmented / tab strip / pane strip | Left/Right (Up/Down too) move and select, Home/End |
| Tab bar | Left/Right move focus across tabs and select (roving), Delete or ⌘W closes the focused tab, ⌘⌥ arrows reorder |
| Tree (navigator) | Up/Down, Left collapses (or goes to the parent), Right expands (or goes to the first child), Home/End, type-ahead, Return renames, ⌘↓ opens |
| List, table, outline (search results, stack, variables, languages, themes, key bindings) | Up/Down, Home/End, PageUp/PageDown, Left/Right (outlines: collapse/expand; tables: move between cells), Space toggles a row checkbox, Return activates, type-ahead |
| Radio group | Up/Down/Left/Right move and select |
| Menus (native and `ui::Menu`) | Up/Down, Left/Right (submenus), Return, Esc, first-letter |
| Completion list, QuickNav results | Up/Down (wrap), Return/Tab accept, PageUp/PageDown |
| Split handle | Left/Right (or Up/Down) 8pt, Shift 32pt, Home/End to min/max, Return toggles collapse |
| Stepper | Up/Down by step, Shift 10x |
| Color picker area | arrows move the cursor 1 unit, Shift 10 |

### 10.5 Roles, names and states by control class

NSH `AccessibilityRole` values; the label is set with `accessibility_label`; state through the component (`selected`, `checked`, `expanded`, `disabled`, `has_popup`).

| Control class | NSH role | Accessible name | State exposed |
|---|---|---|---|
| Window regions | `Toolbar` (toolbar band), `Tablist` (tab bar), `Region` (sidebar: `Navigator`, editor column: `Editor area`, console: `Console`, docs: `Documentation`), `Group` for status bar labeled `Status bar` | as quoted | |
| Button, IconButton | `Button` | visible label, or the tooltip text without the shortcut (`Run`, `Stop`, `Edit Run Settings`) | disabled; pressed (toggles) |
| Toggle icon button | `Button` (toggle) | `Navigator`, `Console`, `Documentation`, `Automatic Lookups`, `Live Updating` | on / off (selected) |
| CapsuleGroup | `Group` (roving toolbar) | `Run controls`, `History navigation`, `View` | |
| Segmented control | `Radiogroup` + `Radio` (or `Tablist` + `Tab` when it switches content) | the control's purpose (`Console type`, `Text editing category`, `Language settings`) and each segment's label | selected |
| PopupButton | `Button`, `has_popup` (native menu) / `Combobox` (select) | `<Label>, <current value>`: `Syntax, Python`; `Output filter, All Output` | expanded when open |
| SearchField | `Search` | `Search text` (find), `Filter` (navigator, console, key bindings), `Search Docs`, `Open Quickly` | |
| TextField / TextArea | text input | the FormRow label (`Compile Flags`, `Arguments`, `Program Input`) | invalid, read-only |
| Checkbox / Radio / Switch | `Checkbox` / `Radio` / `Switch` | the visible label | checked, mixed, disabled |
| NumberField | `Spinbutton` | the label (`Line Spacing`) | value, min, max |
| ColorWell | `Button` | `<Name> color, <hex>`: `Foreground color, #C5CAD3` | |
| List, outline, table | `List` / `Tree` / `Table` with `Row`, `Cell`, `ColumnHeader` | `Open documents`, `File navigator`, `Search results`, `Stack`, `Variables`, `Languages`, `Color themes`, `Key bindings` | selected, expanded, row/column index |
| SidebarRow | `TreeItem` | file or folder name, kind | expanded, selected, `current document` |
| TabItem | `Tab` | document name; `, edited` appended when dirty; `, running` when running | selected |
| StatusItem | `Button` or `Text` | its text (`Run Succeeded`, `Line 9, Column 44`) | |
| Editor | text area | `Editor for <document name>` | read-only |
| Gutter | `Text`/`Group` | `Line numbers, current line <n>` | breakpoints announced as `Breakpoint on line <n>` |
| Console text | text area | `Console text view for <document name>` | |
| Diagnostic annotation | `Alert` for errors, `Text` otherwise | `<Severity>: <message>` | |
| Sheet / dialog | `Dialog` (modal) | its title (`Run Settings`) | |
| Alert | `AlertDialog` (native) | title + message | |
| Popover | `Dialog` (non-modal) | its purpose (`Search options`, `Symbols`) | |
| Menu, MenuItem | `Menu`, `Menuitem` / `MenuItemCheckbox` | item title | checked, disabled |
| Tooltip | n/a (the label already carries it) | | |
| Toast, Banner | `Alert` (error, warning), `Summary` (info, success) | the message | |
| Progress | `Progressbar` | `Running`, `Searching`, `Loading page` | indeterminate or value |
| Split handle | `Adjustable` | `Resize sidebar`, `Resize console`, `Resize documentation` | value (points) |
| Separator, decorative icons | none (hidden) | | |
| Status icons | none (the adjacent text names the state); standalone severity icons are `Image` with `Error`, `Warning`, `Notice` | | |
| Kbd | `Text` | spoken form `Shift Command O` | |
| GlyphTile | none (hidden when text follows); otherwise `Image` named after the language | | |

### 10.6 Never by color, motion or sound alone

Run state has icon + text; diagnostics have icon + underline style (wavy error, dotted hint) + the overview mark + a count; the selected tab, segment and toggled icons use fill and tint (not tint alone); breakpoints differ in shape from the execution arrow; the editor annotation text states the severity; focus, selection and disabled differ by outline, fill and opacity respectively; nothing requires animation; there is no sound.

### 10.7 Verification

`nsh test` selects by role and label (`role:button[Run]`, `role:tab[permute.py]`, `label:Editor for permute.py`), so each test that drives the screen also checks the name. The main task (new file, type, Run, read output, change language, open Preferences, change a setting) must complete with the keyboard alone and with VoiceOver on; both are in the Finishing checklist (12.1).


## 11. NSH limits the design works around

From the NSH capability survey and source. The design uses only what NSH can build; where it cannot, the specified alternative is the design, not a fallback.

| Limit (07) | Design decision |
|---|---|
| G1 No `NSToolbar` with items, no customization, no sidebar-tracking separators | Hidden system title bar; title band (32) and toolbar band (40) are the app's own views; capsule groups from `toolbar()` + `button()`; `Customize Toolbar…` not offered; show/hide only. `controls_offset` places the traffic lights |
| G2 No `NSOutlineView` source list (inline rename, drag reorder) | `tree()` + `render_item` (7.12), inline rename as a TextField in the row, drag in/out limited to files dropped in (G12); vibrancy via `macos_backdrop(Sidebar)` under a translucent `sidebar` fill |
| G3 No native window tabs | Own tab bar (7.13), as CodeRunner itself does (`NSWindowTabbingMode = disallowed`); tab drag out/in is in-app (pan gestures), not to Finder |
| G4 No NSDocument chrome (edited dot in the close button, proxy icon, Versions) | ` – Edited` in the title band, `dot` in the tab, language tile instead of a proxy icon; Versions omitted |
| G5 Only 15 menu roles; no Services, Help search, Open Recent role | Native `MenuItem`s; `Open Recent` is an app-built submenu; no Services |
| G7 Syntax colors fixed to 8 kinds | The app's highlighter paints token colors as `EditorDecoration` styles from the `.tmTheme`; chrome of the editor from `EditorStyles` and the editor-scope tokens (3.4) |
| G8 No minimap, inline widgets, block caret, relative numbers (relative numbers are an app option), bracket coloring | No minimap in the design; annotations are an overlay layer (8.6.2); Bar caret only (other insertion-point styles disabled in Preferences with the tooltip `Not available`); relative numbers drawn by the app's gutter layer or hidden |
| G9 No find bar UI | Own find bar (8.6.4) over `EditorSearch` and the editor API |
| G10 No PTY console view | Console text is app-drawn scoped text; `terminal()` is not used for the console |
| G12 No drag out | Tabs and files drag within the app; "Show in Finder" and Copy Path replace dragging to Finder |
| G14 `NSAlert` has no accessory | Alerts needing a checkbox or field are Sheets (7.19); native alerts hold at most two buttons in a row |
| G15 Appearance is global | One appearance for all windows; the Themes pane documents it; per-window themes are not offered |
| G16 No native popup, segmented, stepper, search field | Drawn components (7.3 to 7.9) built to the platform's measurements; popups open native menus |
| G17 No toolbar-style Preferences | Pane strip (8.13) with roving tabs, in a window with a hidden title bar |
| `shadow` values in a theme file are dropped | Shadow color is the `shadow` token, geometry is NSH's presets (5.4) |
| `font-sans` / `font-mono` read only the first family | No font token is set; the code stack is a constant passed to `Editor.font_family` and `text()` styles (section 4) |
| Vibrancy needs a transparent window | Every region except the sidebar paints an opaque fill (3.4) |
| `svg()` draws no masks, filters, patterns or text | The icon set uses paths and basic shapes only (6.1) |
| No `Kbd`, `Badge`, `Banner`, `Toast` one-call APIs | Styled in `ui` (7.15, 7.16, 7.23) |
| `Cmd+R` is reload in the `nsh run` dev host | Develop with `cargo run` / the packaged app; not a design change |

## 12. Finishing checklist and verification status

### 12.1 Checklist (run on every screen the work adds or changes, in light and dark; report each group)

The checklist of the NSH design guides applies in full. The ScratchRunner-specific points are:

- **Tokens:** no raw color, radius, padding, font size or duration outside `ui/theme.json`, `ui/theme.rs`, `ui/tokens.rs`, `ui/typography.rs`, `ui/motion.rs` and the ThemeResolver data; every control takes `size` and `variant`; the Gallery (7.30) shows the control before a feature uses it.
- **Two layers:** chrome never uses `editor-*`, `gutter-*`, `console-*`, `diag-*` tokens; the editor, gutter, console and editor status bar never use chrome tokens except `popover` surfaces they float. A dark syntax theme under a light system appearance (and the reverse) must look right.
- **Radius order:** capsule groups and md/lg buttons are the only pills; xs/sm buttons are `radius-sm`; fields `radius`; floating surfaces `radius-lg`/`radius-xl`; nested radii concentric.
- **Alignment:** the chrome spine (x = 8) and content spine (x = 12) hold across toolbar, tab bar, sidebar, bars; icons in fixed slots; hairlines drawn once; numbers `tabular_nums` end-aligned.
- **States:** every pressable has hover, pressed, focus-visible, disabled; popups keep the trigger open-styled; selection `selection` vs `selection-inactive` verified by deactivating the window.
- **Motion:** the inventory in 9.4 is complete and every line verified in the running app; Reduce Motion on checked.
- **Keyboard and VoiceOver:** the main task completes without a mouse (10.7), focus visible and returned after every overlay, every control has a role and a name.
- **Content states:** empty, loading, error, offline and long-content states exist for each surface (8.16); checked at 500 x 320, at a larger text size, and with long file and language names.

### 12.2 What was verified for this document

| Item | Status |
|---|---|
| `ui/theme.json` | Parsed by `nsh_base::Theme::from_json_lenient`: 148 colors and 55 numbers read back (spot-checked values: `primary`, `accent`, `border`, `editor-bg`, `radius` 8, `ctl-md` 28, `type-body` 13, `motion-instant` 100, `editor-line-height-ratio` 1.45), 0 values ignored, 0 missing |
| Contrast | Computed (3.5); every text pair at least 4.5:1, graphics and secondary at least 3:1 (the exceptions are listed there) |
| Icons | All 85 SVGs accepted by `nsh_core::svg::drawing` (the code `svg()` uses); rendered to a contact sheet and inspected; they use only `path`, `circle`, `rect`, `g` |
| Screens, motion, accessibility | Specified, not built: nothing here is verified in a running app. Section 9.4 and 12.1 must be run by the implementer; each `[ ]` in 9.4 is a line to check |

### 12.3 Open questions (decide while building the `ui` layer; none blocks the design)

1. **Traffic-light offset.** `TitleBarOptions { height: 32 }` vs `controls_offset`: confirm in the first build that the lights center on y = 16 and that `window_drag` and double-click zoom work on both bands.
2. **Vibrancy and the transparent window.** Confirm that `transparent(true)` plus opaque fills elsewhere has no hairline or shadow artifacts; if it does, fall back to the opaque `sidebar` computation (3.4).
3. **Ligatures.** NSH exposes `font_variant` features but ligature (`calt`) control for a given family is unverified; the design treats `Ligatures` as "Fira Code leads the stack".
4. **Native menu anchoring.** `popup_menu_at` anchoring under a button and the open state while the menu is up are specified; verify the trigger can stay pressed until the menu closes (the awaited `Answer<()>`).
5. **Overlay annotations** (8.6.2) need `range_bounds` per visible diagnostic line at scroll rates; cap at 40 visible pills and use the overview ruler beyond that.
6. **System accent** is not readable from NSH; `primary` is fixed blue.
7. **Window-level shadow and rounding** of the transparent main window follow the platform; verify no clipped corner artifacts at the title band.
