//! TextMate editor themes (`scratch-theme`).
//!
//! The resolved theme does two jobs for every editor: its `editor_colors()`
//! restyle the editor's parts (background, gutter, caret, selection,
//! invisibles, guides) through `.styles(...)`, and its `Highlighter` — an
//! incremental syntect parse — produces styled spans handed to the editor as
//! `decorations`, replacing the built-in highlighting (`.highlighting(false)`
//! keeps the two palettes from mixing).
//!
//! Each `OpenDocument` owns its `Highlighter`; `decorations()` bumps along on
//! the document's `text_rev`, so the editor restyles only after text,
//! language or theme changes. Nothing here writes a signal while the
//! workspace `RefCell` is borrowed.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::text_view::PartStyle;
use nsh_base::{EditorDecoration, EditorPosition, EditorRange, EditorStyles};
use nsh_core::element::{AdaptiveColor, Style, StyleRefinement};
use nsh_core::{Color, PropertyValue, StyleDecl, StyleKey, TextDecoration};
use nsh_core::prelude::*;
use scratch_document::DocumentId;
use scratch_theme::{Highlighter, ResolvedTheme, StyledSpan, ThemeLibrary};

use super::preferences;
use super::state::with;

/// Longest a `decorations()` call parses before answering with guessed spans.
const PARSE_BUDGET: Duration = Duration::from_millis(8);

thread_local! {
    static LIBRARY: RefCell<Option<ThemeLibrary>> = const { RefCell::new(None) };
    static RESOLVED: RefCell<Option<(String, Arc<ResolvedTheme>)>> = const { RefCell::new(None) };
}

/// The theme files' revision: saving, duplicating or deleting a theme
/// bumps it, re-resolving editors.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct ThemesRevision(pub u64);

/// The theme library: the built-ins plus `~/Library/Application Support/
/// ScratchRunner/Themes`.
pub(super) fn with_library<R>(f: impl FnOnce(&ThemeLibrary) -> R) -> R {
    LIBRARY.with_borrow_mut(|slot| f(slot.get_or_insert_with(|| ThemeLibrary::new(with(|workspace| {
        workspace.paths.as_ref().map(|paths| paths.themes())
    })))))
}

/// Mutates the library (save / duplicate / delete a theme), then has the
/// editors resolve again.
pub(super) fn mutate_library<R>(f: impl FnOnce(&mut ThemeLibrary) -> R) -> R {
    let out = LIBRARY.with_borrow_mut(|slot| f(slot.get_or_insert_with(|| ThemeLibrary::new(with(|workspace| {
        workspace.paths.as_ref().map(|paths| paths.themes())
    })))));
    RESOLVED.with_borrow_mut(|slot| *slot = None);
    global::<ThemesRevision>().update(|r| r.0 += 1);
    out
}

fn resolved_for(is_dark: bool) -> Arc<ResolvedTheme> {
    global::<ThemesRevision>().get();
    let name = preferences::settings().with(|prefs| prefs.themes.resolve(is_dark).to_owned());
    let theme = with_library(|library| library.get_or_default(&name, is_dark));
    Arc::new(theme.compile())
}

fn to_nsh_rgba(rgba: scratch_theme::Rgba) -> nsh_core::Rgba {
    nsh_core::Rgba {
        r: rgba.r as f32 / 255.0,
        g: rgba.g as f32 / 255.0,
        b: rgba.b as f32 / 255.0,
        a: rgba.a as f32 / 255.0,
    }
}

fn adaptive(light: scratch_theme::Rgba, dark: scratch_theme::Rgba) -> AdaptiveColor {
    AdaptiveColor::new(to_nsh_rgba(light), to_nsh_rgba(dark))
}

/// Synchronizes NSH's design tokens with the active TextMate theme and font preferences.
pub(super) fn install() {
    static BASE_JSON: &str = include_str!("../../ui/theme.json");
    effect(|| {
        global::<ThemesRevision>().get();
        let editor_font_size = preferences::settings().with(|prefs| prefs.text_editing.appearance.font.size as f32);
        let console_font_size = (editor_font_size - 1.0).clamp(11.0, 20.0);
        let light = resolved_for(false);
        let dark = resolved_for(true);
        let l = light.editor_colors();
        let d = dark.editor_colors();
        let lc = light.chrome();
        let dc = dark.chrome();
        let Ok((mut ui_theme, _)) = nsh_base::Theme::from_json_lenient(BASE_JSON) else {
            return;
        };
        let default_bg = ui_theme.color("background").map(|c| (c.light, c.dark));
        let default_fg = ui_theme.color("foreground").map(|c| (c.light, c.dark));
        let base_bg_light: Hsla = to_nsh_rgba(l.background).into();
        let base_fg_light: Hsla = to_nsh_rgba(l.foreground).into();
        let base_bg_dark: Hsla = to_nsh_rgba(d.background).into();
        let base_fg_dark: Hsla = to_nsh_rgba(d.foreground).into();
        if !light.is_dark() && dark.is_dark() {
            ui_theme.set_color("background", adaptive(l.background, d.background));
            ui_theme.set_color("foreground", adaptive(l.foreground, d.foreground));
        } else if !light.is_dark() {
            ui_theme.set_color("background", AdaptiveColor { light: base_bg_light, dark: default_bg.map(|c| c.1).unwrap_or(base_bg_dark) });
            ui_theme.set_color("foreground", AdaptiveColor { light: base_fg_light, dark: default_fg.map(|c| c.1).unwrap_or(base_fg_dark) });
        } else if dark.is_dark() {
            ui_theme.set_color("background", AdaptiveColor { light: default_bg.map(|c| c.0).unwrap_or(base_bg_light), dark: base_bg_dark });
            ui_theme.set_color("foreground", AdaptiveColor { light: default_fg.map(|c| c.0).unwrap_or(base_fg_light), dark: base_fg_dark });
        }
        let mut set = |name: &str, light_col: scratch_theme::Rgba, dark_col: scratch_theme::Rgba| {
            ui_theme.set_color(name, adaptive(light_col, dark_col));
        };
        set("editor-bg", l.background, d.background);
        set("editor-fg", l.foreground, d.foreground);
        set("editor-caret", l.caret, d.caret);
        set("editor-selection", l.selection, d.selection);
        set("editor-line-highlight", l.line_highlight, d.line_highlight);
        set("editor-invisibles", l.invisibles, d.invisibles);
        set("editor-indent-guide", l.indent_guide, d.indent_guide);
        set("editor-indent-guide-active", l.active_guide, d.active_guide);
        set("editor-ruler", l.ruler, d.ruler);
        set("editor-bracket-match", l.matching_bracket, d.matching_bracket);
        set("editor-find-match", l.find_highlight, d.find_highlight);
        set("editor-bar", lc.tab_bar_background, dc.tab_bar_background);
        set("editor-bar-foreground", lc.muted_foreground, dc.muted_foreground);
        set("editor-bar-border", lc.separator, dc.separator);
        set("gutter-bg", l.gutter_background, d.gutter_background);
        set("gutter-fg", l.gutter_foreground, d.gutter_foreground);
        set("gutter-fg-active", l.gutter_active_foreground, d.gutter_active_foreground);
        set("gutter-separator", l.gutter_divider, d.gutter_divider);
        set("window", lc.window_background, dc.window_background);
        set("toolbar", lc.toolbar_background, dc.toolbar_background);
        set("toolbar-border", lc.separator, dc.separator);
        set(
            "tab-bar",
            lc.background.mix(lc.foreground, 0.06),
            dc.background.mix(dc.foreground, 0.03),
        );
        set("sidebar", lc.sidebar_background.with_alpha(0xB8), dc.sidebar_background.with_alpha(0xB8));
        set("sidebar-foreground", lc.sidebar_foreground, dc.sidebar_foreground);
        set("sidebar-border", lc.separator, dc.separator);
        set("console-bg", lc.console_background, dc.console_background);
        set("console-fg", lc.console_foreground, dc.console_foreground);
        set("tab-active", l.background, dc.background.mix(dc.foreground, 0.13));
        set("tab-active-foreground", l.foreground, d.foreground);
        set("tab-foreground", lc.muted_foreground, dc.muted_foreground);
        ui_theme.set_number("editor-font-size", editor_font_size);
        ui_theme.set_number("console-font-size", console_font_size);
        set_tokens(ui_theme.into_tokens());
    });
}

/// The theme the preferences resolve for the current appearance.
fn resolved() -> Arc<ResolvedTheme> {
    global::<ThemesRevision>().get();
    let name = preferences::settings().with(|prefs| prefs.themes.resolve(dark()).to_owned());
    RESOLVED.with_borrow_mut(|slot| {
        if slot.as_ref().map(|(key, _)| key.as_str()) != Some(name.as_str()) {
            let theme = with_library(|library| library.get_or_default(&name, dark()));
            *slot = Some((name, Arc::new(theme.compile())));
        }
        slot.as_ref().expect("resolved set above").1.clone()
    })
}

/// Whether the app follows the editor theme's brightness (Preferences >
/// Themes > Match Appearance to Theme).
pub(super) fn match_appearance() -> bool {
    preferences::settings().with(|prefs| prefs.themes.match_appearance_to_theme)
}

/// The editor theme's brightness (its own, not the system's).
pub(super) fn theme_is_dark() -> bool { resolved().is_dark() }

fn color(rgba: scratch_theme::Rgba) -> AdaptiveColor {
    AdaptiveColor::from(to_nsh_rgba(rgba))
}

fn part(style: StyleRefinement) -> Option<PartStyle> { Some(PartStyle(Style::from(style))) }

/// The `styles` of every editor: the theme's editor colors on the gutter,
/// text, caret, selection and marks. `lsp.rs` merges its diagnostic and
/// completion parts on top.
pub(super) fn editor_styles() -> EditorStyles {
    let colors = *resolved().editor_colors();
    let mut styles = EditorStyles::default();
    styles.content = part(StyleRefinement::default()
        .bg(color(colors.background)).text_color(color(colors.foreground)));
    styles.line = part(StyleRefinement::default().text_color(color(colors.foreground)));
    styles.gutter = part(StyleRefinement::default()
        .bg(color(colors.gutter_background)).border_color(color(colors.gutter_divider)));
    styles.line_number = part(StyleRefinement::default().text_color(color(colors.gutter_foreground)));
    styles.active_line_number = part(StyleRefinement::default().text_color(color(colors.gutter_active_foreground)));
    styles.fold_marker = part(StyleRefinement::default().text_color(color(colors.gutter_foreground)));
    styles.caret = part(StyleRefinement::default().bg(color(colors.caret)));
    styles.selection = part(StyleRefinement::default().bg(color(colors.selection)));
    styles.active_line = part(StyleRefinement::default().bg(color(colors.line_highlight)));
    styles.whitespace = part(StyleRefinement::default().text_color(color(colors.invisibles)));
    styles.indent_guide = part(StyleRefinement::default().bg(color(colors.indent_guide)));
    styles.matching_bracket = part(StyleRefinement::default()
        .border_color(color(colors.matching_bracket)).text_color(color(colors.matching_bracket)));
    styles.search_match = part(StyleRefinement::default().bg(color(colors.find_highlight)));
    styles.placeholder = part(StyleRefinement::default().text_color(color(colors.invisibles)));
    styles.ruler = part(StyleRefinement::default().bg(color(colors.ruler)));
    styles
}

fn packed(rgba: scratch_theme::Rgba) -> Color { Color::rgba(rgba.r, rgba.g, rgba.b, rgba.a) }

/// One styled span as an editor decoration. The editor merges decoration
/// styles through `Style::decls()` — typed refinements would not reach the
/// text — so the style is written as declarations.
fn span_decoration(line: u32, span: &StyledSpan) -> EditorDecoration {
    let style = span.style;
    let mut decls = vec![StyleDecl::Set(StyleKey::Color, PropertyValue::Color(Some(packed(style.foreground))))];
    if let Some(background) = style.background {
        decls.push(StyleDecl::Set(StyleKey::BackgroundColor, PropertyValue::Color(Some(packed(background)))));
    }
    if style.bold { decls.push(StyleDecl::Set(StyleKey::FontWeight, PropertyValue::FontWeight(Some(700)))); }
    if style.italic { decls.push(StyleDecl::Set(StyleKey::FontStyle, PropertyValue::OptBool(Some(true)))); }
    if style.underline {
        decls.push(StyleDecl::Set(StyleKey::TextDecorationLine,
            PropertyValue::TextDecoration(Some(TextDecoration { underline: true, line_through: false }))));
    }
    let mut decoration = EditorDecoration::default();
    decoration.range = EditorRange::new(
        EditorPosition::new(line, span.col_start),
        EditorPosition::new(line, span.col_end),
    );
    decoration.style = Some(Style::from_decls(decls));
    decoration
}

/// A breakpoint's mark in the gutter's strip (a bar in the accent colour,
/// CodeRunner's arrow approximated).
fn breakpoint_mark(breakpoint: &scratch_document::Breakpoint) -> EditorDecoration {
    let mut gutter = nsh_base::EditorGutterMark::default();
    // Disabled breakpoints keep their place but render muted (CodeRunner
    // draws them greyed).
    let color = if breakpoint.enabled {
        scratch_ui_kit::tokens::breakpoint()
    } else {
        scratch_ui_kit::tokens::muted_foreground()
    };
    gutter.style = Some(Style::from(StyleRefinement::default().bg(color)));
    let mut mark = EditorDecoration::default();
    mark.range = EditorRange::new(
        EditorPosition::new(breakpoint.line, 0),
        EditorPosition::new(breakpoint.line, 0),
    );
    mark.gutter = Some(gutter);
    mark
}

/// The debugger's execution marker (DESIGN 8.6.2): the stopped line tinted
/// `debug-line`, a `debug-arrow` mark in the gutter strip.
fn execution_mark(line: u32) -> EditorDecoration {
    let mut gutter = nsh_base::EditorGutterMark::default();
    gutter.style = Some(Style::from(StyleRefinement::default().bg(
        scratch_ui_kit::tokens::debug_arrow(),
    )));
    let mut mark = EditorDecoration::default();
    mark.range = EditorRange::new(EditorPosition::new(line, 0), EditorPosition::new(line, 0));
    mark.gutter = Some(gutter);
    mark.line_style = Some(Style::from(StyleRefinement::default().bg(
        scratch_ui_kit::tokens::debug_line(),
    )));
    mark
}

/// The theme's spans for `id`'s text, as editor decorations. The `text_rev`
/// read subscribes this binding to edits and language changes; `settings()`
/// and `dark()` resubscribe it to theme and appearance changes.
fn diagnostic_line_mark(line: u32, severity: scratch_diagnostics::Severity) -> EditorDecoration {
    let bg = match severity {
        scratch_diagnostics::Severity::Error => scratch_ui_kit::tokens::diag_error_bg(),
        scratch_diagnostics::Severity::Warning => scratch_ui_kit::tokens::diag_warning_bg(),
        scratch_diagnostics::Severity::Notice => scratch_ui_kit::tokens::diag_info_bg(),
    };
    let mut mark = EditorDecoration::default();
    mark.range = EditorRange::new(EditorPosition::new(line, 0), EditorPosition::new(line, 0));
    mark.line_style = Some(Style::from(StyleRefinement::default().bg(bg)));
    mark
}

/// Highlights for `id`'s current text (syntax spans, breakpoints, stopped-line
/// execution arrow, and optional diagnostic line tints).
pub(super) fn decorations(id: DocumentId) -> Vec<EditorDecoration> {
    let (enabled, highlight_diag_lines, from_console) = preferences::settings().with(|prefs| (
        prefs.text_editing.appearance.syntax_highlighting,
        prefs.text_editing.diagnostics.highlight_line,
        prefs.text_editing.diagnostics.from_console,
    ));
    let _ = dark();
    let theme = resolved();
    let lsp_diags = if highlight_diag_lines { super::lsp::diagnostics(id) } else { Vec::new() };
    with(|workspace| {
        let syntax = workspace.doc(id).and_then(|doc| {
            workspace.registry.as_ref()
                .and_then(|registry| registry.get(&doc.document.language))
                .map(|definition| definition.editor_syntax.clone())
        }).unwrap_or_else(|| "plain text".to_owned());
        let Some(doc) = workspace.doc_mut(id) else { return Vec::new(); };
        let _ = doc.text_rev.get();
        let mut out: Vec<EditorDecoration> = if enabled {
            let highlighter = doc.highlighter.get_or_insert_with(|| Highlighter::new(&syntax, theme.clone()));
            highlighter.set_language(&syntax);
            highlighter.set_theme(theme);
            let text = doc.text.clone();
            highlighter.update_text(text);
            highlighter.advance(PARSE_BUDGET);
            let lines = highlighter.line_count();
            highlighter.highlight_lines(0, lines, true, PARSE_BUDGET).lines.iter()
                .flat_map(|line| line.spans.iter().map(move |span| span_decoration(line.line, span)))
                .collect()
        } else {
            Vec::new()
        };
        if highlight_diag_lines {
            let _ = doc.diag_rev.get();
            let mut by_line: std::collections::BTreeMap<u32, scratch_diagnostics::Severity> = std::collections::BTreeMap::new();
            let rank = |s: scratch_diagnostics::Severity| match s {
                scratch_diagnostics::Severity::Notice => 1u8,
                scratch_diagnostics::Severity::Warning => 2,
                scratch_diagnostics::Severity::Error => 3,
            };
            let mut record = |line: u32, sev: scratch_diagnostics::Severity| {
                by_line.entry(line)
                    .and_modify(|cur| if rank(sev) > rank(*cur) { *cur = sev; })
                    .or_insert(sev);
            };
            let index = scratch_diagnostics::LineIndex::new(&doc.text);
            for annotation in doc.annotations.visible() {
                if !from_console && annotation.origin == scratch_diagnostics::Origin::Console {
                    continue;
                }
                if let Some((start, _)) = annotation.positions(&index) {
                    record(start.line as u32, annotation.severity);
                }
            }
            for diag in lsp_diags {
                let sev = match diag.severity {
                    nsh_base::EditorDiagnosticSeverity::Error => scratch_diagnostics::Severity::Error,
                    nsh_base::EditorDiagnosticSeverity::Warning => scratch_diagnostics::Severity::Warning,
                    _ => scratch_diagnostics::Severity::Notice,
                };
                record(diag.range.start.line, sev);
            }
            out.extend(by_line.into_iter().map(|(line, sev)| diagnostic_line_mark(line, sev)));
        }
        out.extend(doc.document.breakpoints.as_slice().iter().map(breakpoint_mark));
        if let Some(line) = super::debugger::stopped_line(id) {
            out.push(execution_mark(line.saturating_sub(1)));
        }
        out
    })
}

// ------------------------------------------------- Copy with Colors

/// The longest a `colored_html`/`scopes_at` call may parse.
const COPY_BUDGET: Duration = Duration::from_millis(50);

/// Renders `id`'s full source text into a print-ready light HTML document
/// honoring `Preferences.printing` (`with_colors`, `with_line_numbers`,
/// `font_size`, `line_height`, margins), paired with its plain text fallback.
pub(super) fn print_html(id: DocumentId, title: &str) -> Option<(String, String, (f64, f64, f64, f64))> {
    let (printing, font_family) = preferences::settings().with_untracked(|prefs| (
        prefs.printing.clone(),
        prefs.text_editing.appearance.font.family.clone(),
    ));
    let print_theme = if printing.with_colors {
        Some(Arc::new(with_library(|library| library.get_or_default(scratch_theme::DEFAULT_LIGHT, false)).compile()))
    } else {
        None
    };
    let margins = (
        f64::from(printing.top_margin),
        f64::from(printing.bottom_margin),
        f64::from(printing.left_margin),
        f64::from(printing.right_margin),
    );
    let (syntax, text) = with(|workspace| {
        let syntax = workspace
            .registry
            .as_ref()
            .and_then(|registry| {
                workspace.doc(id).and_then(|doc| registry.get(&doc.document.language))
            })
            .map(|definition| definition.editor_syntax.clone())
            .unwrap_or_else(|| "plain text".to_owned());
        let doc = workspace.doc(id)?;
        Some((syntax, doc.text.clone()))
    })?;
    let lines: Vec<&str> = text.split('\n').collect();
    let highlighted = if let Some(theme) = print_theme {
        let mut h = Highlighter::new(&syntax, theme);
        h.update_text(text.clone());
        h.advance(COPY_BUDGET);
        Some(h.highlight_lines(0, lines.len(), false, COPY_BUDGET))
    } else {
        None
    };
    let font_size = printing.font_size.max(6);
    let line_height = printing.line_height.max(1.0);
    let num_width = lines.len().max(1).to_string().len();
    let mut html = format!(
        "<!DOCTYPE html><html><body style=\"background:#ffffff;color:#1a1d21;font-family:'{}',Menlo,monospace;font-size:{font_size}pt;line-height:{line_height};margin:0;\">",
        escape(&font_family),
    );
    html.push_str(&format!(
        "<div style=\"font-family:-apple-system,sans-serif;font-size:10pt;font-weight:600;color:#4a505a;border-bottom:1px solid #d8dce3;padding-bottom:4pt;margin-bottom:8pt;\">{}</div>",
        escape(title),
    ));
    html.push_str("<pre style=\"margin:0;white-space:pre-wrap;word-wrap:break-word;font-family:inherit;font-size:inherit;line-height:inherit;\">");
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if printing.with_line_numbers {
            html.push_str(&format!(
                "<span style=\"color:#8f98a5;user-select:none;\">{:>width$}  </span>",
                index + 1,
                width = num_width,
            ));
        }
        let boundaries = utf16_boundaries(line);
        let columns = boundaries.len() - 1;
        let spans = highlighted
            .as_ref()
            .and_then(|hl| hl.lines.iter().find(|spans| spans.line as usize == index));
        let mut at = 0u32;
        if let Some(spans) = spans {
            for span in &spans.spans {
                let end = span.col_end.min(columns as u32);
                let start = span.col_start.min(end);
                if start > at {
                    html.push_str(&escape(&line[boundaries[at as usize]..boundaries[start as usize]]));
                }
                if end > start {
                    html.push_str(&styled_span(&line[boundaries[start as usize]..boundaries[end as usize]], &span.style));
                }
                at = end;
            }
        }
        if (at as usize) < columns {
            html.push_str(&escape(&line[boundaries[at as usize]..boundaries[columns]]));
        }
        if index + 1 < lines.len() {
            html.push('\n');
        }
    }
    html.push_str("</pre></body></html>");
    Some((html, text, margins))
}

/// "Copy with Colors": `start..end` of `id` as theme-styled HTML plus the
/// plain text, for the pasteboard's `public.html` flavor.
pub(super) fn colored_html(
    id: DocumentId,
    start: EditorPosition,
    end: EditorPosition,
) -> Option<(String, String)> {
    let theme = resolved();
    with(|workspace| {
        let syntax = workspace
            .registry
            .as_ref()
            .and_then(|registry| {
                workspace.doc(id).and_then(|doc| registry.get(&doc.document.language))
            })
            .map(|definition| definition.editor_syntax.clone())
            .unwrap_or_else(|| "plain text".to_owned());
        let doc = workspace.doc_mut(id)?;
        let highlighter = doc
            .highlighter
            .get_or_insert_with(|| Highlighter::new(&syntax, theme.clone()));
        highlighter.set_language(&syntax);
        highlighter.set_theme(theme.clone());
        highlighter.update_text(doc.text.clone());
        highlighter.advance(COPY_BUDGET);
        Some(build_html(&doc.text, start, end, highlighter))
    })
}

/// The scope stack and matched theme items at `position` of `id` ("Show
/// Color Scopes").
pub(super) fn scopes_at(
    id: DocumentId,
    position: EditorPosition,
) -> Option<scratch_theme::ScopeInfo> {
    let theme = resolved();
    with(|workspace| {
        let syntax = workspace
            .registry
            .as_ref()
            .and_then(|registry| {
                workspace.doc(id).and_then(|doc| registry.get(&doc.document.language))
            })
            .map(|definition| definition.editor_syntax.clone())
            .unwrap_or_else(|| "plain text".to_owned());
        let doc = workspace.doc_mut(id)?;
        let highlighter = doc
            .highlighter
            .get_or_insert_with(|| Highlighter::new(&syntax, theme.clone()));
        highlighter.set_language(&syntax);
        highlighter.set_theme(theme);
        highlighter.update_text(doc.text.clone());
        highlighter.scopes_at(position.line as usize, position.column, COPY_BUDGET)
    })
}

/// Builds the (plain, html) pair of `text`'s `start..end` range.
fn build_html(
    text: &str,
    start: EditorPosition,
    end: EditorPosition,
    highlighter: &mut Highlighter,
) -> (String, String) {
    let lines: Vec<&str> = text.split('\n').collect();
    let first = (start.line as usize).min(lines.len().saturating_sub(1));
    let last = (end.line as usize).min(lines.len().saturating_sub(1)).max(first);
    let highlighted = highlighter.highlight_lines(first, last + 1, false, COPY_BUDGET);
    let mut plain = String::new();
    let mut html = String::from(
        "<pre style=\"font-family:Menlo,monospace;font-size:12px\">",
    );
    for (index, raw) in lines.iter().copied().enumerate().take(last + 1).skip(first) {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let boundaries = utf16_boundaries(line);
        let columns = boundaries.len() - 1;
        let low = if index == first { start.column.min(columns as u32) } else { 0 };
        let high = if index == last { end.column.min(columns as u32) } else { columns as u32 };
        let (low_b, high_b) = (boundaries[low as usize], boundaries[high as usize]);
        plain.push_str(&line[low_b..high_b]);
        if index < last {
            plain.push('\n');
        }
        let spans = highlighted
            .lines
            .iter()
            .find(|spans| spans.line as usize == index);
        let mut at = low;
        if let Some(spans) = spans {
            for span in &spans.spans {
                if span.col_end <= low || span.col_start >= high {
                    continue;
                }
                if span.col_start > at {
                    html.push_str(&escape(&line[boundaries[at as usize]..boundaries[span.col_start.min(high) as usize]]));
                }
                let (s0, s1) = (span.col_start.max(low), span.col_end.min(high));
                html.push_str(&styled_span(&line[boundaries[s0 as usize]..boundaries[s1 as usize]], &span.style));
                at = span.col_end.min(high);
            }
        }
        if at < high {
            html.push_str(&escape(&line[boundaries[at as usize]..boundaries[high as usize]]));
        }
        if index < last {
            html.push('\n');
        }
    }
    html.push_str("</pre>");
    (plain, html)
}

/// Byte offset after each UTF-16 column of `line` (`boundaries[c]` is the
/// byte offset of column `c`; surrogate pairs occupy two columns).
fn utf16_boundaries(line: &str) -> Vec<usize> {
    let mut boundaries: Vec<usize> = Vec::with_capacity(line.len() + 1);
    boundaries.push(0);
    for (byte, ch) in line.char_indices() {
        // Columns inside a surrogate pair land on the char's end, so a
        // sliced range never splits a scalar.
        boundaries.extend(std::iter::repeat_n(byte + ch.len_utf8(), ch.len_utf16() - 1));
        boundaries.push(byte + ch.len_utf8());
    }
    boundaries
}

/// HTML-escaped `text`.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `text` in a `style`-colored `<span>`.
fn styled_span(text: &str, style: &scratch_theme::TextStyle) -> String {
    let mut css = format!("color:{}", style.foreground.to_hex());
    if let Some(background) = style.background {
        css.push_str(&format!(";background-color:{}", background.to_hex()));
    }
    if style.bold {
        css.push_str(";font-weight:bold");
    }
    if style.italic {
        css.push_str(";font-style:italic");
    }
    if style.underline {
        css.push_str(";text-decoration:underline");
    }
    format!("<span style=\"{css}\">{}</span>", escape(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_boundaries_map_columns_to_bytes() {
        // `a` 1 unit, `😀` 2 units, `b` 1 unit → 4 columns over 6 bytes.
        let boundaries = utf16_boundaries("a😀b");
        assert_eq!(boundaries, vec![0, 1, 5, 5, 6]);
        // Selecting the emoji's second half alone yields nothing.
        assert_eq!(&"a😀b"[boundaries[2]..boundaries[3]], "");
        // The whole emoji.
        assert_eq!(&"a😀b"[boundaries[1]..boundaries[3]], "😀");
    }

    #[test]
    fn escape_html() {
        assert_eq!(
            escape("<a> & \"b\" 'c' </a>"),
            "&lt;a&gt; &amp; &quot;b&quot; &#39;c&#39; &lt;/a&gt;"
        );
    }
}
