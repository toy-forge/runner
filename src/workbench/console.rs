//! The console (DESIGN 8.7): the active document's run output, its toolbar
//! and the program-input line. It sits below the editors or on their right,
//! as the window's layout places it.
//!
//! The toolbar holds: the Filter field (substring over the stored lines),
//! the Output popup (`All Output`, `Program Stderr`, …; a stream/level
//! filter), Stop while running, Clear, and Hide. Languages with a
//! `switchable` console (Markdown) get a Preview/Terminal segmented toggle.
//!
//! The output area is the text log for `Terminal` consoles; `Web` and
//! `Web Resource` languages show a `TextView` instead (the in-process
//! `markdown` renderer, the live document source for `live`, or the run's
//! collected stdout), and `PDF` languages rasterize the file the last
//! output line names (CoreGraphics, one image per page).
//!
//! The filter is applied in the view: `FilteredIndex` maps filtered rows to
//! source-line indices and scans only new lines after each append.
//!
//! Motion inventory (DESIGN.md 9): output and status change without
//! animation (the console is a log); the input line appears with the run
//! (instant); Clear and Stop press-scale and wash through ui-kit.

use std::collections::BTreeMap;

use nsh_base::{EditorApi, input};
use nsh_core::element::{ImageSrc, WebNavigation};
use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::keymap::When;
use nsh_core::prelude::*;
use scratch_console_model::{Attrs, Color as ConsoleColor, Console, Level, LinkTarget, Marks, SpanOptions, Stream};
use scratch_language::ConsoleType;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::execution;
use scratch_document::DocumentId;

use super::state::{self, display, with};

/// The output-stream filter of the Output popup (DESIGN 8.7).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum OutputFilter {
    #[default]
    All,
    /// stdout + stderr + typed input.
    Program,
    Stdout,
    Stderr,
    Debugger,
    Errors,
    ErrorsAndWarnings,
    /// Everything but errors and warnings.
    HideErrorsAndWarnings,
}

impl OutputFilter {
    fn title(self) -> &'static str {
        match self {
            OutputFilter::All => "All Output",
            OutputFilter::Program => "Program Output",
            OutputFilter::Stdout => "Program Stdout",
            OutputFilter::Stderr => "Program Stderr",
            OutputFilter::Debugger => "Debugger Output",
            OutputFilter::Errors => "Errors",
            OutputFilter::ErrorsAndWarnings => "Errors and Warnings",
            OutputFilter::HideErrorsAndWarnings => "Hide Errors and Warnings",
        }
    }

    fn accepts(self, level: Level, stream: Stream) -> bool {
        let errorish = matches!(level, Level::Error | Level::Warning);
        match self {
            OutputFilter::All => true,
            OutputFilter::Program => matches!(stream, Stream::Stdout | Stream::Stderr | Stream::Input),
            OutputFilter::Stdout => stream == Stream::Stdout,
            OutputFilter::Stderr => stream == Stream::Stderr,
            OutputFilter::Debugger => stream == Stream::Debugger,
            OutputFilter::Errors => level == Level::Error,
            OutputFilter::ErrorsAndWarnings => errorish,
            OutputFilter::HideErrorsAndWarnings => !errorish,
        }
    }
}

/// Toolbar state: the stream filter and the text query (app-wide, like
/// CodeRunner's single console toolbar per window).
#[derive(Clone, Default, PartialEq)]
struct ConsoleView {
    filter: OutputFilter,
    query: String,
}

fn view_state() -> Signal<ConsoleView> { global::<ConsoleView>() }

/// The console spec of the active document's language.
fn spec() -> Option<scratch_language::ConsoleSpec> {
    display().with(|view| view.active); // resubscribe on tab switches
    with(|workspace| {
        let doc = workspace.active.and_then(|id| workspace.doc(id))?;
        workspace.registry.as_ref()?.get(&doc.document.language).map(|def| def.console.clone())
    })
}

/// Per-document "show the terminal instead" choices of `switchable`
/// consoles (Preview is the default).
#[derive(Clone, Default, PartialEq)]
struct ConsoleToggles(BTreeMap<DocumentId, bool>);

fn forced_terminal() -> bool {
    let Some(id) = with(|workspace| workspace.active) else { return false };
    global::<ConsoleToggles>().with(|t| t.0.get(&id).copied().unwrap_or(false))
}

/// Which output the console shows for the active document.
fn effective_kind() -> ConsoleType {
    match spec() {
        Some(spec) if spec.kind != ConsoleType::Terminal && !(spec.switchable && forced_terminal()) => spec.kind,
        _ => ConsoleType::Terminal,
    }
}

/// Whether the console sits on the right of the editors: the language's
/// `Console Position` override wins, else the global preference.
pub fn console_on_right() -> bool {
    let preferred = match spec().and_then(|s| s.position) {
        Some(scratch_language::ConsolePosition::Right) => true,
        Some(scratch_language::ConsolePosition::Bottom) => false,
        None => display().with(|view| view.console_right),
    };
    // A right console needs 240 pt of its own beside a 240 pt editor; below
    // 860 pt it moves to the bottom so neither column is crushed.
    preferred && window().width.0 >= 860.0
}

/// The console's contents: header, output, and the input line while a run lasts.
pub fn panel() -> impl IntoElement {
    let draft = signal(String::new());
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).flex_col().bg(tokens::console_bg)
        .child(header())
        .child(dynamic(move || match effective_kind() {
            ConsoleType::Terminal => output().into_element(),
            ConsoleType::Pdf => pdf_output().into_element(),
            ConsoleType::Web | ConsoleType::WebResource => web_output().into_element(),
        }))
        .child(when(active_running, move || input_line(draft)))
}

fn header() -> impl IntoElement {
    view().h(px(28.0)).flex_row().items_center().gap_1p5().px_2().border_b_1().border_color(tokens::editor_bar_border)
        .bg(tokens::editor_bar)
        .child(view().w(px(160.0)).min_w(px(120.0)).max_w(px(200.0)).child(
            search_field("Filter",
                move || SharedString::from(view_state().with(|view| view.query.clone())),
                move || None::<SharedString>,
                move |value: &str| view_state().update(|view| view.query = value.to_owned()),
                move || view_state().update(|view| view.query.clear()),
                SearchFieldOptions::new().size(ControlSize::Sm).leading(IconName::Filter))))
        .child(output_button())
        .child(view().flex_1())
        .child(when(move || spec().is_some_and(|s| s.switchable && s.kind != ConsoleType::Terminal), || {
            segmented("Console type",
                [Segment::new("Preview"), Segment::new("Terminal")],
                move || usize::from(forced_terminal()),
                |index| {
                    let Some(id) = with(|workspace| workspace.active) else { return };
                    global::<ConsoleToggles>().update(|t| { t.0.insert(id, index == 1); });
                },
                SegmentedOptions::new().size(ControlSize::Sm))
        }))
        .child(when(move || super::debugger::pane_visible() || debugging_active(), || {
            icon_button(IconName::DebugStack, "Hide / Show Stack and Variables", IconButtonOptions::new().size(ControlSize::Sm), super::debugger::toggle_pane)
        }))
        .child(when(active_running, || icon_button(IconName::Stop, "Stop", IconButtonOptions::new().size(ControlSize::Sm).shortcut("⌘."), execution::stop_active)))
        .child(icon_button(IconName::Eraser, "Clear the Console", IconButtonOptions::new().size(ControlSize::Sm).shortcut("⌘K"), execution::clear_active))
        .child(dynamic(move || {
            icon_button(if console_on_right() { IconName::ChevronRight } else { IconName::ChevronDown },
                "Hide the Console", IconButtonOptions::new().size(ControlSize::Sm), || {
                motion::animate_panel();
                display().update(|view| view.console_visible = false);
                with(|workspace| workspace.changed());
            }).into_element()
        }))
}

/// The Output popup: the filter's name, the native menu on press.
fn output_button() -> impl IntoElement {
    nsh_base::button()
        .accessibility_label("Console output filter")
        .flex_row().items_center().gap_1().h(px(20.0)).px_1p5().rounded(px(tokens::radius_sm()))
        .cursor_pointer()
        .hover(|s| s.bg(tokens::accent))
        .active(|s| s.bg(tokens::capsule_pressed))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
        .when(!motion::reduced(), motion::state_wash)
        .when(!motion::reduced(), motion::press)
        .active(|s| motion::press_scale(s, true))
        .on_press(move || { spawn(async { let _ = popup_menu(output_menu()).await; }); })
        .child(text(move || SharedString::from(view_state().with(|view| view.filter.title())))
            .text_xs().text_color(tokens::foreground))
        .child(glyph(IconName::ChevronsUpDown, IconSize::Xs, Tone::Muted))
}

/// The stream-filter menu; the current item is checked.
fn output_menu() -> Vec<MenuItem> {
    use OutputFilter::*;
    const FILTERS: &[OutputFilter] = &[All, Program, Stdout, Stderr, Debugger, Errors, ErrorsAndWarnings, HideErrorsAndWarnings];
    let current = view_state().with_untracked(|view| view.filter);
    FILTERS.iter().map(|&filter| {
        MenuItem::new(filter.title()).checked(filter == current)
            .on_select(move || view_state().update(|view| view.filter = filter))
    }).collect()
}

// ------------------------------------------------------------------ output

/// Filtered rows: source-line indices passing the stream filter and query,
/// scanned incrementally as lines arrive.
#[derive(Default)]
struct FilteredIndex {
    /// The document and filter the index is for. The console revision is
    /// deliberately NOT part of the key: it changes on every output chunk,
    /// and invalidating per revision rescans the whole console per chunk —
    /// quadratic on a flooding child. Appends are handled incrementally
    /// (`scanned`), front evictions by shifting (`evicted`).
    key: (Option<DocumentId>, OutputFilter, String),
    /// Total lines evicted when the index was maintained; advancing it
    /// shifts every stored source index down by the delta.
    evicted: u64,
    /// Source lines scanned so far (lines never un-commit).
    scanned: usize,
    /// Passing source-line indices.
    indices: Vec<u32>,
}

impl FilteredIndex {
    /// Advances the index to the console's current state and returns the
    /// filtered row count. Incremental: appends extend the scan, front
    /// evictions shift stored indices, a clear or key change rescans.
    fn maintain(&mut self, console: &Console, active: Option<DocumentId>, filter: OutputFilter, query: &str) -> usize {
        if self.key != (active, filter, query.to_owned()) {
            self.indices.clear();
            self.scanned = 0;
            self.evicted = 0;
            self.key = (active, filter, query.to_owned());
        }
        // Ring-buffer eviction shifts every source index down.
        let evicted = console.truncated().map(|t| t.lines).unwrap_or(0);
        if evicted != self.evicted {
            let delta = (evicted - self.evicted.min(evicted)) as u32;
            self.indices.retain(|i| *i >= delta);
            self.indices.iter_mut().for_each(|i| *i -= delta);
            self.scanned = self.scanned.saturating_sub(delta as usize);
            self.evicted = evicted;
        }
        let total = console.line_count();
        // Clear rewinds the console past the scan position.
        if total < self.scanned { self.scanned = 0; self.indices.clear(); }
        while self.scanned < total {
            if passes(console, self.scanned, filter, query) { self.indices.push(self.scanned as u32); }
            self.scanned += 1;
        }
        self.indices.len()
    }
}

thread_local! { static FILTERED: std::cell::RefCell<FilteredIndex> = std::cell::RefCell::new(FilteredIndex::default()); }

/// Mutates the index; kept off signals so scans under `with` never notify
/// (a notification would re-enter the workspace's `RefCell`).
fn filtered_with<R>(f: impl FnOnce(&mut FilteredIndex) -> R) -> R { FILTERED.with_borrow_mut(f) }

/// Whether line `index` of `console` passes the current filter and query.
fn passes(console: &Console, index: usize, filter: OutputFilter, query: &str) -> bool {
    let Some(line) = console.line(index) else { return false };
    if !filter.accepts(line.level(), line.stream_at(0)) { return false; }
    query.is_empty() || line.text().to_lowercase().contains(query)
}

/// The filtered row count of the active console (subscribes to the revision).
fn filtered_count() -> usize {
    display().with(|view| view.active); // resubscribe on tab switches
    with(|workspace| {
        let Some(doc) = workspace.active.and_then(|id| workspace.doc(id)) else { return 0 };
        let _ = doc.console_rev.get();
        let (filter, query) = view_state().with_untracked(|view| (view.filter, view.query.clone().to_lowercase()));
        let query = query.trim().to_owned();
        filtered_with(|index| index.maintain(&doc.console, workspace.active, filter, &query))
    })
}

/// One filtered row's source line index.
fn filtered_source(at: usize) -> Option<u32> {
    let _ = view_state().get(); // rows re-render when the filter or query changes
    filtered_with(|index| index.indices.get(at).copied())
}

fn output() -> impl IntoElement {
    let list_ref = element_ref();
    let scrolled_away = signal(false);
    let scroll_ref = list_ref.clone();
    let follow_ref = list_ref.clone();
    effect(move || {
        let count = filtered_count();
        if count == 0 {
            scrolled_away.set(false);
        } else if !scrolled_away.get_untracked() {
            let r = follow_ref.clone();
            spawn(async move {
                next_frame().await;
                r.scroll_to_end(false);
            });
        }
    });
    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).relative()
        .child(
            virtual_list(filtered_count, move |at| {
                view().px_3().h(px(18.0)).flex_none().overflow_hidden().child(dynamic(move || {
                    text("").select_text().number_of_lines(1u32)
                        .text_overflow(nsh_core::EllipsizeMode::Clip)
                        .font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                        .text_size(px(13.0))
                        .children(active_spans(at.index()).into_iter().map(span_view))
                }))
            })
            .element_ref(&list_ref)
            .on_scroll(move |ev: &ScrollEvent| {
                let bottom_gap = ev.content_size.height.0 - (ev.offset.y.0 + ev.size.height.0);
                scrolled_away.set(bottom_gap > 24.0 && ev.content_size.height.0 > ev.size.height.0 + 4.0);
            })
            .estimated_item_size(px(18.0))
            .empty(view().flex_1().items_center().justify_center().p_3()
                .child(text("Output appears here when you run the document (⌘R)").text_sm().text_color(tokens::muted_foreground)))
            .flex_1()
            .min_h(px(0.0)),
        )
        .child(when(move || scrolled_away.get() && filtered_count() > 0, move || {
            let r = scroll_ref.clone();
            view().absolute().right_3().bottom_2().rounded_full().shadow_sm()
                .child(button(
                    "Jump to Bottom",
                    ButtonOptions::new()
                        .variant(ButtonVariant::Default)
                        .size(ControlSize::Sm)
                        .icon(IconName::ArrowDown),
                    move || {
                        scrolled_away.set(false);
                        r.scroll_to_end(true);
                    },
                ))
        }))
}

/// One rendered piece of a console line (uniform style, link, marks).
struct ConsoleSpan {
    text: String,
    color: AdaptiveColor,
    bold: bool,
    italic: bool,
    underline: bool,
    link: Option<LinkTarget>,
}

/// One span's text run; links open their target on press (the
/// text-child press pattern: press lives on the text child itself).
fn span_view(span: ConsoleSpan) -> AnyElement {
    let mut t = text(span.text).text_color(span.color)
        .font_weight(if span.bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL });
    if span.italic { t = t.italic(); }
    if span.underline { t = t.underline(); }
    if let Some(target) = span.link {
        t = t.cursor_pointer().on_press(move || open_link(target.clone()));
    }
    t.into_element()
}

/// Opens a console link (CodeRunner's clickable `file:line` and URLs).
fn open_link(target: LinkTarget) {
    match target {
        LinkTarget::Url(url) => { spawn(async move { let _ = open_url(url).await; }); }
        LinkTarget::File { path, line, column: _ } => {
            // Errors from a run point at the temporary copy under
            // `ScratchRunner/run-*/src/`; jump inside the document instead of
            // opening that throwaway file.
            if path.contains("/ScratchRunner/run-") {
                if let Some(id) = with(|workspace| workspace.active)
                    && let Some(api) = state::editor_api(id) {
                        let target = line.unwrap_or(1).saturating_sub(1);
                        api.go_to_line(target);
                        api.reveal_position(nsh_base::EditorPosition::new(target, 0), true);
                    }
                return;
            }
            let resolved = with(|workspace| {
                let doc = workspace.active.and_then(|id| workspace.doc(id))?;
                let raw = if let Some(rest) = path.strip_prefix("~/") {
                    std::env::var("HOME").map(|home| std::path::PathBuf::from(home).join(rest)).unwrap_or_else(|_| std::path::PathBuf::from(&path))
                } else {
                    std::path::PathBuf::from(&path)
                };
                Some(if raw.is_absolute() { raw } else {
                    state::saved_path(&doc.document)
                        .and_then(|dir| dir.parent().map(|dir| dir.join(&raw)))
                        .unwrap_or(raw)
                })
            });
            if let Some(path) = resolved {
                spawn(async move { super::files::open_path_at(path, line.unwrap_or(1)).await; });
            }
        }
    }
}

/// The 16-color ANSI palette as theme tokens (`ansi-*`).
fn ansi_color(index: u8) -> AdaptiveColor {
    match index % 16 {
        0 => tokens::ansi_black(), 1 => tokens::ansi_red(),
        2 => tokens::ansi_green(), 3 => tokens::ansi_yellow(),
        4 => tokens::ansi_blue(), 5 => tokens::ansi_magenta(),
        6 => tokens::ansi_cyan(), 7 => tokens::ansi_white(),
        8 => tokens::ansi_bright_black(), 9 => tokens::ansi_bright_red(),
        10 => tokens::ansi_bright_green(), 11 => tokens::ansi_bright_yellow(),
        12 => tokens::ansi_bright_blue(), 13 => tokens::ansi_bright_magenta(),
        14 => tokens::ansi_bright_cyan(), _ => tokens::ansi_bright_white(),
    }
}

/// The xterm 256-color palette entries past the named 16.
fn xterm_color(index: u8) -> AdaptiveColor {
    let cube = |n: u8| -> u32 {
        let v = n as u32;
        let r = v / 36; let g = v / 6 % 6; let b = v % 6;
        let level = |c: u32| if c == 0 { 0 } else { 55 + c * 40 };
        level(r) << 16 | level(g) << 8 | level(b)
    };
    let v = match index {
        n @ 16..=231 => cube(n - 16),
        n @ 232..=255 => { let g = 8 + (n as u32 - 232) * 10; g << 16 | g << 8 | g }
        n => return ansi_color(n),
    };
    AdaptiveColor::rgb(v, v)
}

/// One visible (filtered) line of the active console, cut into spans.
fn active_spans(index: usize) -> Vec<ConsoleSpan> {
    let Some(source) = filtered_source(index).map(|i| i as usize) else { return Vec::new() };
    with(|workspace| {
        let doc = workspace.active.and_then(|id| workspace.doc(id))?;
        let _ = doc.console_rev.get();
        let line = doc.console.line(source)?;
        let base = match (line.level(), line.stream_at(0)) {
            (Level::Error, _) | (_, Stream::Stderr) => tokens::destructive(),
            (Level::Warning, _) => tokens::warning(),
            (_, Stream::Input) => tokens::primary(),
            (_, Stream::System | Stream::Debugger) => tokens::muted_foreground(),
            _ => tokens::console_fg(),
        };
        // Diagnostics > Console Text > Disabled hides the highlights.
        let key = format!("{}/console", doc.document.language);
        let diagnostics = super::preferences::settings().with_untracked(|prefs| {
            if prefs.text_editing.diagnostics.disabled_diagnostics.contains(&key) { None }
            else { doc.console_highlights.get(&source).cloned() }
        }).unwrap_or_default();
        let links = line.links();
        let spans = line.spans(&SpanOptions {
            links: &links,
            search: &[],
            current_search: None,
            selection: None,
            diagnostics: &diagnostics,
        });
        Some(spans.into_iter().map(|span| {
            let link = span.link.map(|i| links[i].target.clone());
            let diagnostic = span.marks.contains(Marks::DIAGNOSTIC);
            let (mut color, mut underline) = (base, false);
            if diagnostic { color = tokens::console_error(); underline = true; }
            if link.is_some() { color = tokens::console_link(); underline = true; }
            else if !diagnostic
                && let Some(ansi) = match span.style.fg {
                    ConsoleColor::Default => None,
                    ConsoleColor::Ansi(n) => Some(ansi_color(n)),
                    ConsoleColor::Indexed(n) => Some(xterm_color(n)),
                    ConsoleColor::Rgb(r, g, b) => Some(AdaptiveColor::rgb((r as u32) << 16 | (g as u32) << 8 | b as u32, (r as u32) << 16 | (g as u32) << 8 | b as u32)),
                } { color = ansi; }
            ConsoleSpan {
                text: span.text.to_owned(),
                color,
                bold: span.style.attrs.contains(Attrs::BOLD),
                italic: span.style.attrs.contains(Attrs::ITALIC),
                underline: underline || span.style.attrs.contains(Attrs::UNDERLINE),
                link,
            }
        }).collect())
    }).unwrap_or_default()
}

/// Shell/program input history navigated with Up/Down while the input line
/// has focus.
#[derive(Clone, Default, PartialEq)]
struct InputHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    saved_draft: String,
}

fn input_line(draft: Signal<String>) -> impl IntoElement {
    let focused = signal(false);
    effect(move || {
        if !focused.get() { return; }
        let opts = KeyOptions { when: When::Always, repeat: true, pass_through: false };
        on_key_with("up", opts, move |_| {
            global::<InputHistory>().update(|h| {
                if h.entries.is_empty() { return; }
                let next = match h.cursor {
                    None => {
                        h.saved_draft = draft.get_untracked();
                        h.entries.len() - 1
                    }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                h.cursor = Some(next);
                draft.set(h.entries[next].clone());
            });
        });
        on_key_with("down", opts, move |_| {
            global::<InputHistory>().update(|h| {
                let Some(idx) = h.cursor else { return; };
                if idx + 1 < h.entries.len() {
                    let next = idx + 1;
                    h.cursor = Some(next);
                    draft.set(h.entries[next].clone());
                } else {
                    h.cursor = None;
                    let saved = std::mem::take(&mut h.saved_draft);
                    draft.set(saved);
                }
            });
        });
    });
    input()
        .value(move || draft.get())
        .placeholder(move || {
            if debugging_active() { "Debugger console — type a command and press Return" }
            else { "Type program input and press Return" }
        })
        .placeholder_color(|| color(tokens::muted_foreground()))
        .caret_color(|| color(tokens::primary()))
        .on_focus(move || focused.set(true))
        .on_blur(move || focused.set(false))
        .on_change(move |value: &str| draft.set(value.to_owned()))
        .on_submit(move |value: &str| {
            if !value.trim().is_empty() {
                global::<InputHistory>().update(|h| {
                    if h.entries.last().map(String::as_str) != Some(value) {
                        h.entries.push(value.to_owned());
                        if h.entries.len() > 200 {
                            let overflow = h.entries.len() - 200;
                            h.entries.drain(..overflow);
                        }
                    }
                    h.cursor = None;
                    h.saved_draft.clear();
                });
            }
            if debugging_active() {
                super::debugger::send_input(value);
            } else {
                execution::send_input(value);
            }
            draft.set(String::new());
        })
        .h(px(28.0))
        .px_3()
        .text_sm()
        .font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
        .border_t_1()
        .border_color(tokens::border)
        .bg(tokens::console_bg)
        .text_color(tokens::console_fg)
}

/// Whether a debug session is live on the active document.
fn debugging_active() -> bool {
    display().with(|view| view.active); // resubscribe on tab switches
    with(|workspace| workspace.active).is_some_and(super::debugger::debugging)
}

/// Whether the active document's run is in progress.
fn active_running() -> bool {
    display().with(|view| view.active); // resubscribe on tab switches
    with(|workspace| workspace.active.and_then(|id| workspace.doc(id)).is_some_and(|doc| doc.running.get()))
}

// ------------------------------------------------------------ web output

/// Per-document state of the embedded `WKWebView` preview (custom output URL,
/// live mode toggle, navigation flags, and JS console inspector).
#[derive(Clone, Default, PartialEq)]
struct WebPreviewState {
    custom_url: String,
    editing_url: bool,
    url_draft: String,
    live_override: Option<bool>,
    inspector_open: bool,
    can_back: bool,
    can_forward: bool,
    loading: bool,
    title: String,
    current_url: String,
    error: Option<String>,
    logs: Vec<(String, String)>,
}

#[derive(Clone, Default, PartialEq)]
struct WebPreviews(BTreeMap<DocumentId, WebPreviewState>);

fn web_previews() -> Signal<WebPreviews> { global::<WebPreviews>() }

fn update_preview(id: DocumentId, f: impl FnOnce(&mut WebPreviewState)) {
    web_previews().update(|all| f(all.0.entry(id).or_default()));
}

const CONSOLE_HOOK_JS: &str = r#"(function(){
  if (window.__crHooked) return;
  window.__crHooked = true;
  function send(level, args) {
    try {
      var text = Array.prototype.map.call(args, function(a) {
        if (typeof a === 'string') return a;
        try { return JSON.stringify(a); } catch(_) { return String(a); }
      }).join(' ');
      if (window.nsh && window.nsh.postMessage) {
        window.nsh.postMessage(JSON.stringify({ kind: 'console', level: level, text: text }));
      }
    } catch (_) {}
  }
  ['log','info','warn','error'].forEach(function(lv) {
    var orig = console[lv];
    console[lv] = function() {
      send(lv, arguments);
      return orig ? orig.apply(console, arguments) : undefined;
    };
  });
  window.addEventListener('error', function(e) {
    var loc = e.filename ? (' (' + e.filename + ':' + e.lineno + ')') : '';
    send('error', [String(e.message || e) + loc]);
  });
})();"#;

fn active_doc_base_url(id: DocumentId) -> Option<String> {
    with(|workspace| {
        let doc = workspace.doc(id)?;
        let saved = state::saved_path(&doc.document)?;
        let dir = saved.parent()?;
        let mut uri = scratch_lsp::path_to_uri(dir);
        if !uri.ends_with('/') { uri.push('/'); }
        Some(uri)
    })
}

fn source_text_for(id: DocumentId) -> String {
    with(|workspace| {
        let doc = workspace.doc(id)?;
        let _ = doc.text_rev.get();
        doc.editor
            .and_then(|reference| reference.api::<EditorApi>())
            .map(|api| api.value())
            .or_else(|| Some(doc.text.clone()))
    }).unwrap_or_default()
}

fn stdout_text_for(id: DocumentId) -> String {
    with(|workspace| {
        let doc = workspace.doc(id)?;
        let _ = doc.console_rev.get();
        let mut out = String::new();
        for index in 0..doc.console.line_count() {
            if let Some(line) = doc.console.line(index) && line.stream_at(0) == Stream::Stdout {
                out.push_str(line.text());
                out.push('\n');
            }
        }
        Some(out)
    }).unwrap_or_default()
}

fn doc_spec(id: DocumentId) -> Option<scratch_language::ConsoleSpec> {
    with(|workspace| {
        let doc = workspace.doc(id)?;
        workspace.registry.as_ref()?.get(&doc.document.language).map(|def| def.console.clone())
    })
}

fn rendered_web_html(id: DocumentId) -> String {
    let Some(spec) = doc_spec(id) else { return String::new(); };
    let live_override = web_previews().with(|all| all.0.get(&id).and_then(|p| p.live_override));
    let is_markdown = spec.renderer.as_deref() == Some("markdown");
    let is_live = live_override.unwrap_or(is_markdown || spec.live);
    if is_markdown {
        let raw = if is_live { source_text_for(id) } else { stdout_text_for(id) };
        if raw.trim().is_empty() {
            return themed_placeholder("Markdown preview appears here as you edit.");
        }
        return markdown_to_html(&raw);
    }
    match spec.kind {
        ConsoleType::Web => {
            let html = if is_live { source_text_for(id) } else { stdout_text_for(id) };
            if html.trim().is_empty() {
                return themed_placeholder("HTML preview appears here when you edit or run the document (⌘R).");
            }
            html
        }
        ConsoleType::WebResource => {
            let out = stdout_text_for(id);
            if out.trim().is_empty() {
                return themed_placeholder("Run the document (⌘R) to inspect its web output.");
            }
            host_page(&out)
        }
        _ => String::new(),
    }
}

fn themed_placeholder(message: &str) -> String {
    let bg = color(tokens::background()).to_hex();
    let muted = color(tokens::muted_foreground()).to_hex();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
        html,body{{height:100%;margin:0;background:{bg};color:{muted};\
        font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:13px;\
        display:flex;align-items:center;justify-content:center;text-align:center;padding:16px;box-sizing:border-box;}}\
        </style></head><body><div>{message}</div></body></html>"
    )
}

/// Converts Markdown to self-contained HTML styled with the active light/dark tokens.
fn markdown_to_html(md: &str) -> String {
    let body = markdown::to_html_with_options(md, &markdown::Options::gfm())
        .unwrap_or_else(|_| markdown::to_html(md));
    let bg = color(tokens::background()).to_hex();
    let fg = color(tokens::foreground()).to_hex();
    let muted = color(tokens::muted_foreground()).to_hex();
    let surface = color(tokens::card()).to_hex();
    let code_bg = color(tokens::muted()).to_hex();
    let border = color(tokens::border()).to_hex();
    let link = color(tokens::link()).to_hex();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
        :root{{color-scheme:light dark;}}\
        body{{margin:0;padding:18px 22px;background:{bg};color:{fg};\
        font-family:-apple-system,BlinkMacSystemFont,'SF Pro Text',sans-serif;\
        font-size:14px;line-height:1.6;word-wrap:break-word;}}\
        h1,h2,h3,h4,h5,h6{{margin:1.1em 0 .45em;font-weight:600;line-height:1.25;}}\
        h1{{font-size:1.65em;border-bottom:1px solid {border};padding-bottom:.28em;}}\
        h2{{font-size:1.32em;border-bottom:1px solid {border};padding-bottom:.22em;}}\
        h3{{font-size:1.12em;}}\
        p,ul,ol,blockquote,table,pre{{margin:0 0 .85em;}}\
        a{{color:{link};text-decoration:none;}}a:hover{{text-decoration:underline;}}\
        code{{font-family:ui-monospace,'SF Mono',Menlo,monospace;font-size:.9em;\
        background:{code_bg};padding:.14em .38em;border-radius:4px;}}\
        pre{{background:{surface};border:1px solid {border};border-radius:6px;\
        padding:10px 12px;overflow-x:auto;}}\
        pre code{{background:transparent;padding:0;font-size:12.5px;line-height:1.5;}}\
        blockquote{{margin-left:0;padding:2px 12px;border-left:3px solid {border};color:{muted};}}\
        table{{border-collapse:collapse;width:100%;}}\
        th,td{{border:1px solid {border};padding:6px 10px;text-align:left;}}\
        th{{background:{surface};font-weight:600;}}\
        hr{{border:0;border-top:1px solid {border};margin:1.2em 0;}}\
        img{{max-width:100%;height:auto;}}\
        </style></head><body>{body}</body></html>"
    )
}

fn push_web_log(id: DocumentId, level: String, text: String) {
    update_preview(id, |p| {
        p.logs.push((level, text));
        if p.logs.len() > 200 {
            let extra = p.logs.len() - 200;
            p.logs.drain(..extra);
        }
    });
}

fn parse_console_message(id: DocumentId, data: &str) {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(data)
        && value.get("kind").and_then(|k| k.as_str()) == Some("console")
    {
        let level = value.get("level").and_then(|v| v.as_str()).unwrap_or("log").to_owned();
        let msg = value.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        push_web_log(id, level, msg);
    }
}

fn has_custom_preview_url(state: &WebPreviewState) -> bool {
    !state.custom_url.trim().is_empty()
}

fn commit_custom_url(state: &mut WebPreviewState, url: &str) {
    state.custom_url = url.trim().to_owned();
    state.editing_url = false;
    state.error = None;
}

/// Real `WKWebView` console preview with a 26pt Web Resource Bar and an
/// expandable bottom JS Console Inspector.
fn web_output() -> impl IntoElement {
    let active_id = display().with(|view| view.active);
    let Some(doc_id) = active_id else {
        return view().flex_1().min_h(px(0.0)).items_center().justify_center()
            .child(text("No active document").text_sm().text_color(tokens::muted_foreground))
            .into_element();
    };
    let web_ref = element_ref();
    let js_draft = signal(String::new());
    let resource_bar = web_resource_bar(doc_id, web_ref.clone());
    let inspector = web_inspector_drawer(doc_id, web_ref.clone(), js_draft);

    let base_url = active_doc_base_url(doc_id);
    let has_custom = computed(move || {
        web_previews().with(|all| all.0.get(&doc_id).is_some_and(has_custom_preview_url))
    });
    let web_host = dynamic(move || {
        let has_custom = has_custom.get();
        let r = web_ref.clone();
        let wv = if has_custom {
            web_view(move || {
                web_previews().with(|all| {
                    all.0.get(&doc_id).map(|p| p.custom_url.clone()).unwrap_or_default()
                })
            })
        } else {
            web_view_html(move || rendered_web_html(doc_id), base_url.as_deref())
        };
        wv.element_ref(&r)
            .injected_java_script_before_content(CONSOLE_HOOK_JS)
            .on_load_start(move |nav: &WebNavigation| {
                update_preview(doc_id, |p| {
                    p.loading = true;
                    p.error = None;
                    p.current_url = nav.url.clone();
                    p.can_back = nav.can_go_back;
                    p.can_forward = nav.can_go_forward;
                });
            })
            .on_load(move |nav: &WebNavigation| {
                update_preview(doc_id, |p| {
                    p.loading = false;
                    p.current_url = nav.url.clone();
                    p.title = nav.title.clone();
                    p.can_back = nav.can_go_back;
                    p.can_forward = nav.can_go_forward;
                });
            })
            .on_navigation_state_change(move |nav: &WebNavigation| {
                update_preview(doc_id, |p| {
                    p.loading = nav.loading;
                    p.current_url = nav.url.clone();
                    p.title = nav.title.clone();
                    p.can_back = nav.can_go_back;
                    p.can_forward = nav.can_go_forward;
                });
            })
            .on_error(move |msg: &str| {
                let message = msg.to_owned();
                update_preview(doc_id, |p| {
                    p.loading = false;
                    p.error = Some(message.clone());
                });
                push_web_log(doc_id, "error".into(), msg.to_owned());
            })
            .on_message(move |data: &str| parse_console_message(doc_id, data))
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .into_element()
    });

    view().flex_1().min_h(px(0.0)).min_w(px(0.0)).flex_col()
        .child(resource_bar)
        .child(view().flex_1().min_h(px(0.0)).min_w(px(0.0)).relative()
            .child(web_host)
            .child(when(move || web_previews().with(|all| all.0.get(&doc_id).and_then(|p| p.error.clone()).is_some()), move || {
                let msg = web_previews().with(|all| all.0.get(&doc_id).and_then(|p| p.error.clone()).unwrap_or_default());
                view().px_3().py_1p5().border_t_1().border_color(tokens::border).bg(tokens::destructive_muted)
                    .flex_row().items_center().gap_2()
                    .child(glyph(IconName::WarningTriangle, IconSize::Xs, Tone::Danger))
                    .child(type_caption().apply(text(msg)).text_color(tokens::destructive).flex_1().min_w(px(0.0)))
            })))
        .child(inspector)
        .into_element()
}

fn web_resource_bar(doc_id: DocumentId, web_ref: ElementRef) -> impl IntoElement {
    let spec = doc_spec(doc_id);
    let is_markdown = spec.as_ref().is_some_and(|s| s.renderer.as_deref() == Some("markdown"));
    let can_live = is_markdown || spec.as_ref().is_some_and(|s| s.kind == ConsoleType::Web);
    let default_live = spec.as_ref().is_some_and(|s| is_markdown || s.live);

    let r_back = web_ref.clone();
    let r_fwd = web_ref.clone();
    let r_rel = web_ref.clone();
    let can_back = computed(move || web_previews().with(|all| all.0.get(&doc_id).is_some_and(|p| p.can_back)));
    let can_fwd = computed(move || web_previews().with(|all| all.0.get(&doc_id).is_some_and(|p| p.can_forward)));
    let editing = computed(move || web_previews().with(|all| all.0.get(&doc_id).is_some_and(|p| p.editing_url)));
    let custom_url_label = computed(move || web_previews().with(|all| all.0.get(&doc_id).map(|p| p.custom_url.clone()).unwrap_or_default()));
    let live_state = computed(move || {
        web_previews().with(|all| {
            let p = all.0.get(&doc_id);
            (
                p.is_some_and(has_custom_preview_url),
                p.and_then(|p| p.live_override).unwrap_or(default_live),
            )
        })
    });
    let inspector_open = computed(move || web_previews().with(|all| all.0.get(&doc_id).is_some_and(|p| p.inspector_open)));

    view().h(px(26.0)).flex_shrink(0.0).flex_row().items_center().gap_1().px_2()
        .border_b_1().border_color(tokens::editor_bar_border).bg(tokens::editor_bar)
        .child(dynamic(move || {
            let can_back = can_back.get();
            let r = r_back.clone();
            icon_button(IconName::ChevronLeft, "Back", IconButtonOptions::new().size(ControlSize::Sm).disabled(!can_back), move || r.go_back()).into_element()
        }))
        .child(dynamic(move || {
            let can_fwd = can_fwd.get();
            let r = r_fwd.clone();
            icon_button(IconName::ChevronRight, "Forward", IconButtonOptions::new().size(ControlSize::Sm).disabled(!can_fwd), move || r.go_forward()).into_element()
        }))
        .child(icon_button(IconName::Refresh, "Reload Web View", IconButtonOptions::new().size(ControlSize::Sm), move || {
            update_preview(doc_id, |p| p.error = None);
            r_rel.reload();
        }))
        .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
            if editing.get() {
                view().flex_row().items_center().gap_1()
                    .child(view().flex_1().min_w(px(0.0)).child(
                        input()
                            .value(move || web_previews().with(|all| all.0.get(&doc_id).map(|p| p.url_draft.clone()).unwrap_or_default()))
                            .placeholder("http://localhost:3000 or Custom Output URL…")
                            .placeholder_color(|| color(tokens::muted_foreground()))
                            .caret_color(|| color(tokens::primary()))
                            .on_change(move |v: &str| {
                                let next = v.to_owned();
                                update_preview(doc_id, |p| p.url_draft = next);
                            })
                            .on_submit(move |v: &str| {
                                update_preview(doc_id, |p| commit_custom_url(p, v));
                            })
                            .h(px(20.0)).px_2().text_xs()
                            .bg(tokens::field).text_color(tokens::foreground)
                            .rounded(px(tokens::radius_sm()))
                            .border_1().border_color(tokens::border),
                    ))
                    .child(button("Done", ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm), move || {
                        update_preview(doc_id, |p| {
                            let url = p.url_draft.clone();
                            commit_custom_url(p, &url);
                        });
                    }))
                    .child(button("Reset", ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm), move || {
                        update_preview(doc_id, |p| {
                            p.custom_url.clear();
                            p.url_draft.clear();
                            p.editing_url = false;
                            p.error = None;
                        });
                    }))
                    .into_element()
            } else {
                let custom = custom_url_label.get();
                let label = if !custom.is_empty() {
                    custom
                } else if is_markdown {
                    "Live Markdown (Custom Output…)".to_owned()
                } else {
                    "Custom Output…".to_owned()
                };
                button(
                    SharedString::from(label),
                    ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm).icon(IconName::Globe),
                    move || {
                        update_preview(doc_id, |p| {
                            p.url_draft = p.custom_url.clone();
                            p.editing_url = true;
                        });
                    },
                ).into_element()
            }
        })))
        .child(when(move || can_live, move || {
            dynamic(move || {
                let (custom_active, is_live) = live_state.get();
                button(
                    "Live",
                    ButtonOptions::new()
                        .variant(if is_live && !custom_active { ButtonVariant::Default } else { ButtonVariant::Ghost })
                        .size(ControlSize::Sm)
                        .disabled(custom_active),
                    move || {
                        update_preview(doc_id, |p| {
                            let cur = p.live_override.unwrap_or(default_live);
                            p.live_override = Some(!cur);
                        });
                    },
                ).into_element()
            })
        }))
        .child(dynamic(move || {
            let open = inspector_open.get();
            icon_button(
                IconName::WebInspectorWindow,
                "Web Console Inspector",
                IconButtonOptions::new().size(ControlSize::Sm).selected(open),
                move || update_preview(doc_id, |p| p.inspector_open = !p.inspector_open),
            ).into_element()
        }))
        .child(icon_button(IconName::ArrowUpRight, "Open in Browser", IconButtonOptions::new().size(ControlSize::Sm), move || {
            let custom = web_previews().with(|all| all.0.get(&doc_id).map(|p| p.custom_url.trim().to_owned()).unwrap_or_default());
            let target = if !custom.is_empty() {
                custom
            } else {
                let html = rendered_web_html(doc_id);
                let path = std::env::temp_dir().join(format!("scratchrunner-preview-{doc_id}.html"));
                if std::fs::write(&path, html).is_err() { return; }
                scratch_lsp::path_to_uri(&path)
            };
            spawn(async move { let _ = open_url(target).await; });
        }))
}

fn web_inspector_drawer(doc_id: DocumentId, web_ref: ElementRef, js_draft: Signal<String>) -> impl IntoElement {
    when(move || web_previews().with(|all| all.0.get(&doc_id).is_some_and(|p| p.inspector_open)), move || {
        let r_eval = web_ref.clone();
        view().h(px(148.0)).flex_shrink(0.0).flex_col().border_t_1().border_color(tokens::border).bg(tokens::console_bg)
            .child(view().h(px(24.0)).flex_shrink(0.0).flex_row().items_center().justify_between().px_2()
                .border_b_1().border_color(tokens::editor_bar_border).bg(tokens::editor_bar)
                .child(type_caption().medium().apply(text(move || {
                    let count = web_previews().with(|all| all.0.get(&doc_id).map(|p| p.logs.len()).unwrap_or(0));
                    format!("JS Console ({count})")
                })).text_color(tokens::muted_foreground))
                .child(button("Clear", ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm), move || {
                    update_preview(doc_id, |p| p.logs.clear());
                })))
            .child(scroll().flex_1().min_h(px(0.0)).px_2().py_1().child(dynamic(move || {
                let logs = web_previews().with(|all| all.0.get(&doc_id).map(|p| p.logs.clone()).unwrap_or_default());
                if logs.is_empty() {
                    return type_caption().apply(text("No console messages yet. Right-click preview for Inspect Element.")).text_color(tokens::muted_foreground).into_element();
                }
                view().flex_col().gap_0p5().children(logs.into_iter().map(|(lv, msg)| {
                    let fg = match lv.as_str() {
                        "error" => tokens::destructive(),
                        "warn" => tokens::warning(),
                        "eval" => tokens::primary(),
                        _ => tokens::console_fg(),
                    };
                    let prefix = match lv.as_str() {
                        "eval" => "> ",
                        "error" => "✕ ",
                        "warn" => "▲ ",
                        _ => "  ",
                    };
                    text(format!("{prefix}{msg}"))
                        .text_xs()
                        .font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                        .text_color(fg)
                        .select_text()
                })).into_element()
            })))
            .child(
                input()
                    .value(move || js_draft.get())
                    .placeholder("> Evaluate JavaScript in preview…")
                    .placeholder_color(|| color(tokens::muted_foreground()))
                    .caret_color(|| color(tokens::primary()))
                    .on_change(move |v: &str| js_draft.set(v.to_owned()))
                    .on_submit(move |code: &str| {
                        let trimmed = code.trim();
                        if trimmed.is_empty() { return; }
                        push_web_log(doc_id, "eval".into(), trimmed.to_owned());
                        let quoted = serde_json::to_string(trimmed).unwrap_or_else(|_| "\"\"".to_owned());
                        let js = format!(
                            "try {{ var __v = (0, eval)({quoted}); if (__v !== undefined) console.log(__v); }} catch (e) {{ console.error(String(e)); }}"
                        );
                        r_eval.inject_java_script(js);
                        js_draft.set(String::new());
                    })
                    .h(px(24.0)).px_2().text_xs()
                    .font_family(scratch_ui_kit::typography::CODE_FONT_STACK)
                    .border_t_1().border_color(tokens::border)
                    .bg(tokens::console_bg).text_color(tokens::console_fg),
            )
    })
}

/// The host page a `Web Resource` fragment is injected into.
fn host_page(fragment: &str) -> String {
    format!("<!doctype html><html><head><meta charset=\"utf-8\"></head><body>{fragment}</body></html>")
}

// ------------------------------------------------------------ pdf output

/// Rasterized pages of the PDF each document's last run produced.
#[derive(Clone, Default, PartialEq)]
struct PdfPages(BTreeMap<DocumentId, PdfPreview>);

#[derive(Clone, PartialEq)]
struct PdfPreview {
    /// The path the pages were rendered from (output's last line).
    path: String,
    /// Page images with their pixel size (rasterized at 2× point size).
    pages: Vec<(u32, u32, ImageSrc)>,
}

/// The path the `PDF` console displays: the last console line naming an
/// existing `.pdf` file — what the compile step emitted for `produces`.
fn pdf_path(console: &Console) -> Option<String> {
    (0..console.line_count()).rev().find_map(|index| {
        let text = console.line(index)?.text().trim().to_owned();
        (text.to_lowercase().ends_with(".pdf")
            && std::path::Path::new(&text).is_file())
        .then_some(text)
    })
}

/// The `PDF` console: page images of the file the run's last line names.
fn pdf_output() -> impl IntoElement {
    dynamic(move || {
        let Some(id) = with(|workspace| workspace.active) else {
            return pdf_message("Open a document to preview its PDF.");
        };
        let (running, path) = with(|workspace| {
            let doc = workspace.doc(id)?;
            let _ = doc.console_rev.get();
            Some((doc.running.get(), pdf_path(&doc.console)))
        }).unwrap_or((false, None));
        if let Some(path) = path.as_deref() { ensure_pages(id, path); }
        let pages = global::<PdfPages>().with(|m| {
            m.0.get(&id).filter(|p| Some(&p.path) == path.as_ref()).map(|p| p.pages.clone())
        }).unwrap_or_default();
        if !pages.is_empty() {
            return scroll().flex_1().min_h(px(0.0))
                .child(view().flex_col().items_center().gap_3().p_3()
                    .children(pages.into_iter().map(|(w, h, src)| {
                        image(src).w(px(w as f32 / 2.0)).h(px(h as f32 / 2.0))
                            .border_1().border_color(tokens::border)
                            .rounded(px(2.0))
                    })))
                .into_element();
        }
        if running { pdf_message("Rendering PDF…") } else { pdf_message("Run the document to produce a PDF (⌘R).") }
    })
}

fn pdf_message(message: &'static str) -> AnyElement {
    view().flex_1().items_center().justify_center().p_3()
        .child(text(message).text_sm().text_color(tokens::muted_foreground))
        .into_element()
}

/// Starts the rasterization of `path` for `id` when it is not done yet.
fn ensure_pages(id: DocumentId, path: &str) {
    if global::<PdfPages>().with(|m| m.0.get(&id).is_some_and(|p| p.path == path)) { return; }
    let path = path.to_owned();
    global::<PdfPages>().update(|m| { m.0.insert(id, PdfPreview { path: path.clone(), pages: Vec::new() }); });
    Owner::detached().run(|| spawn(async move {
        let render = path.clone();
        let pages = blocking(move || rasterize_pdf(std::path::Path::new(&render))).await.unwrap_or_default();
        global::<PdfPages>().update(|m| {
            if let Some(preview) = m.0.get_mut(&id) && preview.path == path {
                preview.pages = pages;
            }
        });
    }));
}

/// Renders every page of the PDF at `path` to an RGBA image (2× scale).
#[cfg(target_os = "macos")]
fn rasterize_pdf(path: &std::path::Path) -> Vec<(u32, u32, ImageSrc)> {
    rasterize_pdf_pixels(path).into_iter()
        .map(|(w, h, px)| (w, h, ImageSrc::rgba(w, h, px)))
        .collect()
}

/// The pages of `path` as `(width, height, rgba pixels)`.
#[cfg(target_os = "macos")]
fn rasterize_pdf_pixels(path: &std::path::Path) -> Vec<(u32, u32, Vec<u8>)> {
    use objc2_core_foundation::{CFURL, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::*;
    let Some(url) = CFURL::from_file_path(path) else { return Vec::new() };
    let Some(document) = CGPDFDocument::with_url(Some(&url)) else { return Vec::new() };
    let space = CGColorSpace::new_device_rgb();
    let info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0);
    let scale = 2.0;
    (1..=CGPDFDocument::number_of_pages(Some(&document)).min(100)).filter_map(|index| {
        let page = CGPDFDocument::page(Some(&document), index)?;
        let media = CGPDFPage::box_rect(Some(&page), CGPDFBox::MediaBox);
        let (width, height) = (
            (media.size.width * scale).ceil() as usize,
            (media.size.height * scale).ceil() as usize,
        );
        if width == 0 || height == 0 { return None; }
        let mut pixels = vec![0u8; width * height * 4];
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(), width, height, 8, width * 4,
                space.as_deref(), info.0,
            )?
        };
        let dest = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_rect(Some(&context), dest);
        CGContext::scale_ctm(Some(&context), scale, scale);
        let page_dest = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(media.size.width, media.size.height),
        );
        CGContext::concat_ctm(Some(&context), CGPDFPage::drawing_transform(Some(&page), CGPDFBox::MediaBox, page_dest, 0, true));
        CGContext::draw_pdf_page(Some(&context), Some(&page));
        Some((width as u32, height as u32, pixels))
    }).collect()
}

#[cfg(not(target_os = "macos"))]
fn rasterize_pdf(_path: &std::path::Path) -> Vec<(u32, u32, ImageSrc)> { Vec::new() }

#[cfg(not(target_os = "macos"))]
fn rasterize_pdf_pixels(_path: &std::path::Path) -> Vec<(u32, u32, Vec<u8>)> { Vec::new() }

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2_core_foundation::{CFURL, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::*;

    /// Writes a one-page PDF with a filled rect to a temp file.
    fn write_pdf(path: &std::path::Path) {
        let media = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 100.0));
        let url = CFURL::from_file_path(path).expect("url");
        let context = unsafe {
            CGPDFContextCreateWithURL(Some(&url), &media, None)
        }.expect("pdf context");
        unsafe { CGPDFContextBeginPage(Some(&context), None) };
        CGContext::set_rgb_fill_color(Some(&context), 1.0, 0.0, 0.0, 1.0);
        CGContext::fill_rect(Some(&context), media);
        CGPDFContextEndPage(Some(&context));
        CGPDFContextClose(Some(&context));
    }

    #[test]
    fn rasterizes_pdf_pages() {
        let path = std::env::temp_dir().join(format!("sr-test-{}.pdf", std::process::id()));
        write_pdf(&path);
        let pages = rasterize_pdf_pixels(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(pages.len(), 1);
        let (w, h, pixels) = &pages[0];
        assert_eq!((*w, *h), (400, 200)); // 200×100pt at 2×
        // A red page rasterizes to mostly-red pixels, not a blank image.
        let red = pixels.chunks(4).filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60).count();
        assert!(red > pixels.len() / 8);
    }

    /// A console capped small enough to force eviction in tests.
    fn tiny_console() -> Console {
        let cfg = scratch_console_model::ConsoleConfig {
            max_lines: 5,
            max_bytes: usize::MAX,
            ..Default::default()
        };
        Console::new(cfg)
    }

    #[test]
    fn the_filtered_index_scans_incrementally() {
        let mut console = tiny_console();
        let id = Some(DocumentId::new());
        let mut index = FilteredIndex::default();
        console.append(Stream::Stdout, b"one\ntwo\n");
        console.finish_streams();
        assert_eq!(index.maintain(&console, id, OutputFilter::All, ""), 2);
        assert_eq!(index.scanned, 2);
        // Appending rescans nothing already scanned.
        console.append(Stream::Stdout, b"three\nfour\n");
        console.finish_streams();
        assert_eq!(index.maintain(&console, id, OutputFilter::All, ""), 4);
        assert_eq!(index.scanned, 4);
        // The same revision rescans nothing at all.
        index.maintain(&console, id, OutputFilter::All, "");
        assert_eq!(index.scanned, 4);
        // A filter change rescans.
        index.maintain(&console, id, OutputFilter::Stderr, "");
        assert_eq!(index.indices.len(), 0);
    }

    #[test]
    fn the_filtered_index_survives_eviction() {
        let mut console = tiny_console(); // keeps 5 lines
        let id = Some(DocumentId::new());
        let mut index = FilteredIndex::default();
        console.append(Stream::Stdout, b"err a\nb\nerr c\nd\ne\n");
        console.finish_streams();
        index.maintain(&console, id, OutputFilter::All, "err");
        assert_eq!(index.indices, vec![0, 2]);
        // Two more lines evict the first two: "err a" is gone, "err c"
        // moves 2→0.
        console.append(Stream::Stdout, b"f\nerr g\n");
        console.finish_streams();
        index.maintain(&console, id, OutputFilter::All, "err");
        assert_eq!(index.indices, vec![0, 4]);
        assert_eq!(index.scanned, 5);
    }

    #[test]
    fn the_filtered_index_resets_on_clear_and_doc_switch() {
        let mut console = tiny_console();
        let a = Some(DocumentId::new());
        let b = Some(DocumentId::new());
        let mut index = FilteredIndex::default();
        console.append(Stream::Stdout, b"x\ny\nz\n");
        console.finish_streams();
        index.maintain(&console, a, OutputFilter::All, "");
        assert_eq!(index.scanned, 3);
        // Clearing the console rewinds past the scan position.
        console.clear();
        console.append(Stream::Stdout, b"new\n");
        console.finish_streams();
        index.maintain(&console, a, OutputFilter::All, "");
        assert_eq!(index.indices, vec![0]);
        // Switching documents never reuses another console's scan.
        index.maintain(&console, b, OutputFilter::All, "");
        assert_eq!(index.scanned, 1);
    }

    #[test]
    fn custom_url_predicate_and_commit_clear_error() {
        let mut preview = WebPreviewState {
            editing_url: true,
            url_draft: "  http://localhost:3000  ".to_owned(),
            error: Some("stale error".to_owned()),
            ..WebPreviewState::default()
        };
        assert!(!has_custom_preview_url(&preview));
        preview.logs.push(("log".to_owned(), "hello".to_owned()));
        preview.loading = true;
        assert!(!has_custom_preview_url(&preview));

        let url = preview.url_draft.clone();
        commit_custom_url(&mut preview, &url);
        assert!(has_custom_preview_url(&preview));
        assert_eq!(preview.custom_url, "http://localhost:3000");
        assert!(!preview.editing_url);
        assert_eq!(preview.error, None);
    }
}
