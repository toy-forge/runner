//! Find and replace (the find bar above the editors) and Go to Line.
//!
//! Matching, highlighting, stepping and replacing are the NSH editor's own
//! search (`EditorSearch`): this module owns the query and options in one
//! global signal, gives the editors their `search` prop ([`editor_search`])
//! and shows the count each editor reports ([`search_changed`]). Wrap Around
//! is ours: a step that wrapped is taken back when it is off.
//!
//! Motion inventory (DESIGN.md 8.6.4, 8.11, 9.4):
//!
//! | Change | Motion | Reduced motion |
//! |---|---|---|
//! | Find bar opens / closes, replace row shows / hides | `motion::animate_panel()` before the change (editor pushed, `base`, ease-in-out) | instant |
//! | Bar controls hover / press / focus | the kit's washes (`instant`), press scale (`fast`), focus ring | instant |
//! | Find / Replace and Text / Word / Regex segments | the kit's sliding pill (`base`) | instant |
//! | Match count changes (`3 of 12`) | none: it updates per keystroke | none |
//! | Current match changes | none: the editor scrolls to it | none |
//! | Invalid regex | outline and caption turn `destructive`, no motion (a state, not an event) | none |
//! | Go to Line opens | scrim fades, sheet rises 12pt from above, scale 0.97 to 1, `slow`, ease-out | instant |
//! | Go to Line rejects input | `MotionPreset::Shake` on the sheet, field outline `destructive` | outline only |
//! | Go to Line closes | removed with the dialog layer (NSH has no leave motion for a dialog) | none |

use std::rc::Rc;

use nsh_base::{
    EditorApi, EditorCommand, EditorPosition, EditorSearch, EditorSearchResult, EditorSelection, dialog, input,
};
use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::keymap::When;
use scratch_document::DocumentId;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::state;

// ---------------------------------------------------------------- State

/// How the query matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Matching {
    /// The text as written.
    #[default]
    Text,
    /// The text as a whole word.
    Word,
    /// A regular expression (Rust `regex` syntax; `$1` in replacements).
    Regex,
}

/// The find bar: what it looks for, how, and whether it shows.
#[derive(Clone, Debug, PartialEq)]
struct Find {
    open: bool,
    replace: bool,
    query: String,
    replacement: String,
    matching: Matching,
    ignore_case: bool,
    wrap: bool,
    /// Matches only inside the selection (`EditorSearch::in_selection`).
    in_selection: bool,
    /// Bumped when the app sets the query, so the field is made again with
    /// the new text selected.
    generation: u64,
    /// Whether the field takes the focus when it is made.
    focus_field: bool,
}

impl Default for Find {
    fn default() -> Self {
        Self {
            open: false,
            replace: false,
            query: String::new(),
            replacement: String::new(),
            matching: Matching::Text,
            ignore_case: true,
            wrap: true,
            in_selection: false,
            generation: 0,
            focus_field: true,
        }
    }
}

fn find() -> Signal<Find> { global::<Find>() }

/// What each editor's search found, by document. Kept apart from [`Find`]:
/// the editors' `search` prop reads that one, and a result written there
/// would make them search again.
#[derive(Clone, Default, PartialEq)]
struct Results(Vec<(DocumentId, EditorSearchResult)>);

fn results() -> Signal<Results> { global::<Results>() }

/// Whether one of the bar's text fields has the keyboard focus.
#[derive(Clone, Copy, Default, PartialEq)]
struct FieldFocus(bool);

/// Go to Line's sheet.
#[derive(Clone, Default, PartialEq)]
struct GoTo {
    open: bool,
    draft: String,
    error: Option<String>,
    lines: u32,
}

fn goto() -> Signal<GoTo> { global::<GoTo>() }

// ---------------------------------------------------------------- Public API

/// Opens the find bar, with the replace row when `replace` is true, and
/// focuses its field with the query selected.
pub fn open(replace: bool) {
    motion::animate_panel();
    find().update(|f| {
        f.open = true;
        f.replace = replace;
        f.focus_field = true;
        f.generation += 1;
    });
}

/// Closes the find bar and gives the focus back to the editor.
pub fn close() {
    if !find().with_untracked(|f| f.open) { return; }
    motion::animate_panel();
    find().update(|f| f.open = false);
    global::<FieldFocus>().set(FieldFocus(false));
    if let Some(id) = active() { state::focus(id); }
}

/// Opens Go to Line.
pub fn go_to_line() {
    let Some(api) = active_api() else { return; };
    let lines = line_count(&api.value());
    goto().set(GoTo { open: true, draft: String::new(), error: None, lines });
}

/// What the editors search for: `Some` while the find bar is open with a
/// query. The coordinator passes it to every editor's `search` prop.
pub fn editor_search() -> Option<EditorSearch> {
    find().with(|f| (f.open && !f.query.is_empty()).then(|| search_of(f)))
}

/// An editor's search result (its `on_search_change`): the count the bar shows.
pub fn search_changed(id: DocumentId, result: &EditorSearchResult) {
    results().update(|r| {
        match r.0.iter_mut().find(|(doc, _)| *doc == id) {
            Some(entry) => entry.1 = result.clone(),
            None => r.0.push((id, result.clone())),
        }
    });
}

/// Selects the next match (or the previous one), opening the bar first
/// when it is closed and a query is known.
pub fn find_next(forward: bool) {
    let (shown, empty) = find().with_untracked(|f| (f.open, f.query.is_empty()));
    if empty { open(false); return; }
    if shown { step(forward); return; }
    // The editors get the search on the next commit; step after it.
    motion::animate_panel();
    find().update(|f| { f.open = true; f.focus_field = false; f.generation += 1; });
    spawn(async move {
        next_frame().await;
        step(forward);
    });
}

/// Makes the selection (a caret: the word at it) the query.
pub fn use_selection() {
    let Some(api) = active_api() else { return; };
    let selected = api.selected_text();
    let query = if selected.is_empty() {
        let Some(caret) = api.selections().last().map(|s| s.head) else { return; };
        let text = api.value();
        let Some(line) = text.split('\n').nth(caret.line as usize) else { return; };
        match word_at(line, caret.column) { Some(word) => word, None => return }
    } else {
        selected
    };
    find().update(|f| {
        f.query = query;
        f.matching = Matching::Text;
        f.focus_field = false;
        f.generation += 1;
    });
}

/// Runs an editor selection command (the Find-and-Select items).
fn select_occurrence(command: EditorCommand) {
    if let Some(api) = active_api() { api.run(command); }
}

/// Rotates the multi-selection to the next/previous caret ("Jump to
/// Selection"): the last selection is the primary one the editor reveals.
fn jump_selection(forward: bool) {
    let Some(api) = active_api() else { return; };
    let mut selections = api.selections();
    if selections.len() < 2 { return; }
    if forward { selections.rotate_left(1); } else { selections.rotate_right(1); }
    let head = selections.last().map(|s| s.head);
    api.set_selections(selections);
    if let Some(head) = head { api.reveal_position(head, true); }
}

/// The selected text, or the word at the caret.
fn selected_word() -> Option<String> {
    let api = active_api()?;
    let selected = api.selected_text();
    if !selected.is_empty() { return Some(selected); }
    let caret = api.selections().last().map(|s| s.head)?;
    let text = api.value();
    let line = text.split('\n').nth(caret.line as usize)?;
    word_at(line, caret.column)
}

/// "Search for Selected Text in File System" (⇧⌘F): opens the sidebar's
/// project search over the navigator root with the selection as the query.
fn search_selected_filesystem() {
    let Some(query) = selected_word() else { return; };
    super::navigator::show_search(true);
    super::search_sidebar::find_now(&query, scratch_search::project::SearchSource::Folder(super::navigator::root_path()));
}

/// "Search for Selected Text in Current Document" (⌃⇧⌘F): the selection
/// becomes the find-bar query and the first match is selected.
fn search_selected_document() {
    if selected_word().is_none() { return; }
    use_selection();
    find_next(true);
}

/// Keyboard shortcuts of this module. Called once from the root.
///
/// The editor binds Cmd-G, Shift-Cmd-G, Cmd-E and Cmd-L itself each time it
/// takes the focus, and the newest binding wins; these are bound again after
/// every focus change so the menu's meaning (DESIGN.md 10.1) wins.
/// The keys come from the key bindings: a change re-registers them.
pub fn install() {
    effect(|| {
        let _ = global::<super::EditorFocus>().get();
        let _ = global::<super::bindings::BindingsRevision>().get();
        super::bindings::on("find.find", || toggle_from_key(false));
        super::bindings::on("find.findAndReplace", || toggle_from_key(true));
        super::bindings::on("find.next", || find_next(true));
        super::bindings::on("find.previous", || find_next(false));
        super::bindings::on("find.selectNext", || select_occurrence(EditorCommand::SelectNextOccurrence));
        super::bindings::on("find.selectPrevious", || select_occurrence(EditorCommand::SelectPreviousOccurrence));
        super::bindings::on("find.selectAll", || select_occurrence(EditorCommand::SelectAllOccurrences));
        super::bindings::on("find.selectWord", || select_occurrence(EditorCommand::SelectWord));
        super::bindings::on("find.useSelection", use_selection);
        super::bindings::on("find.selectNextResult", || find_next(true));
        super::bindings::on("find.selectPreviousResult", || find_next(false));
        super::bindings::on("find.jumpNextSelection", || jump_selection(true));
        super::bindings::on("find.jumpPreviousSelection", || jump_selection(false));
        super::bindings::on("find.searchSelectedFileSystem", search_selected_filesystem);
        super::bindings::on("find.searchSelectedDocument", search_selected_document);
        super::bindings::on("find.pattern", open_pattern);
        super::bindings::on("find.selectAllResults", || select_all_results(false));
        super::bindings::on("find.selectAllResultsInSelection", || select_all_results(true));
        super::bindings::on("find.replaceAndFindNext", || replace_and_find(true));
        super::bindings::on("find.replaceAndFindPrevious", || replace_and_find(false));
        super::bindings::on("find.replaceAllInSelection", replace_all_in_selection);
        super::bindings::on("find.replaceAll", replace_all);
        super::bindings::on("text.goToLine", go_to_line);
    });
    // Escape closes the bar while one of its fields has the focus; in the
    // editor it keeps its editor meaning.
    effect(|| {
        if global::<FieldFocus>().get().0 {
            on_key_with("escape", KeyOptions { when: When::Always, ..KeyOptions::default() }, |_| close());
        }
    });
}

/// The Find menu's items.
pub fn menu_items() -> Vec<MenuItem> {
    let mode = |replace: bool| {
        MenuItem::new(if replace { "Find and Replace" } else { "Find" })
            .checked(find().with_untracked(|f| f.replace) == replace)
            .on_select(move || open(replace))
    };
    MENU.iter().map(|&(title, id, action)| match title {
        "-" => MenuItem::separator(),
        "%" => MenuItem::submenu("Find Mode", [mode(false), mode(true)]),
        _ => {
            let keys = super::bindings::keys_for(id);
            let title = if id == "find.searchSelectedDocument" {
                let doc = state::with(|w| w.active.and_then(|id| w.doc(id)).map(|d| d.title.clone()))
                    .unwrap_or_else(|| "Current Document".to_owned());
                format!("Search for Selected Text in “{doc}”")
            } else {
                title.to_owned()
            };
            let item = MenuItem::new(title).on_select(action);
            if keys.is_empty() { item } else { item.shortcut(&keys) }
        }
    }).collect()
}

/// The Find menu: title, bound-command id, action (`"-"` is a separator,
/// `"%"` the Find Mode submenu).
const MENU: &[(&str, &str, fn())] = &[
    ("Find…", "find.find", || open(false)),
    ("Find and Replace…", "find.findAndReplace", || open(true)),
    ("Find Next", "find.next", || find_next(true)),
    ("Find Previous", "find.previous", || find_next(false)),
    ("Find and Select Next", "find.selectNext", || select_occurrence(EditorCommand::SelectNextOccurrence)),
    ("Find and Select Previous", "find.selectPrevious", || select_occurrence(EditorCommand::SelectPreviousOccurrence)),
    ("Find and Select All", "find.selectAll", || select_occurrence(EditorCommand::SelectAllOccurrences)),
    ("Find and Select Word", "find.selectWord", || select_occurrence(EditorCommand::SelectWord)),
    ("Find Pattern…", "find.pattern", open_pattern),
    ("-", "", || {}),
    ("%", "", || {}),
    ("-", "", || {}),
    ("Select Next Result", "find.selectNextResult", || find_next(true)),
    ("Select Previous Result", "find.selectPreviousResult", || find_next(false)),
    ("Select All Results", "find.selectAllResults", || select_all_results(false)),
    ("Select All Results in Selection", "find.selectAllResultsInSelection", || select_all_results(true)),
    ("-", "", || {}),
    ("Replace and Find Next", "find.replaceAndFindNext", || replace_and_find(true)),
    ("Replace and Find Previous", "find.replaceAndFindPrevious", || replace_and_find(false)),
    ("Replace All in Selection", "find.replaceAllInSelection", replace_all_in_selection),
    ("Replace All", "find.replaceAll", replace_all),
    ("-", "", || {}),
    ("Jump to Next Selection", "find.jumpNextSelection", || jump_selection(true)),
    ("Jump to Previous Selection", "find.jumpPreviousSelection", || jump_selection(false)),
    ("-", "", || {}),
    ("Search in File System…", "find.searchInFileSystem", super::navigator::toggle_search),
    ("Search for Selected Text in File System", "find.searchSelectedFileSystem", search_selected_filesystem),
    ("Search for Selected Text in Current Document", "find.searchSelectedDocument", search_selected_document),
];

// ---------------------------------------------------------------- Actions

fn active() -> Option<DocumentId> { state::with(|workspace| workspace.active) }

fn active_api() -> Option<Rc<EditorApi>> { active().and_then(state::editor_api) }

/// Cmd-F (Opt-Cmd-F): opens the bar, or closes it when its field has the
/// focus in the same mode.
fn toggle_from_key(replace: bool) {
    let (open_now, mode) = find().with_untracked(|f| (f.open, f.replace));
    let focused = global::<FieldFocus>().get_untracked().0;
    if open_now && focused && mode == replace { close(); } else { open(replace); }
}

/// One step through the active editor's matches, taken back when it
/// wrapped and Wrap Around is off.
fn step(forward: bool) {
    let Some(api) = active_api() else { return; };
    let before = api.selections();
    api.run(if forward { EditorCommand::FindNext } else { EditorCommand::FindPrevious });
    let after = api.selections();
    let wrap = find().with_untracked(|f| f.wrap);
    if let (Some(from), Some(to)) = (before.last(), after.last())
        && !wrap && wrapped(forward, from, to) {
            api.set_selections(before.clone());
            state::notice(if forward { "Reached the end of the document." } else { "Reached the start of the document." });
        }
}

fn replace_one() {
    let Some(api) = active_api() else { return; };
    let with = find().with_untracked(|f| f.replacement.clone());
    api.replace(with);
}

fn replace_all() {
    let Some(api) = active_api() else { return; };
    let with = find().with_untracked(|f| f.replacement.clone());
    let done = api.replace_all(with);
    state::notice(match done.count {
        0 => "Nothing was replaced.".to_owned(),
        1 => "Replaced 1 match.".to_owned(),
        n => format!("Replaced {n} matches."),
    });
}

/// "Find Pattern…": the find bar in regular-expression mode.
fn open_pattern() {
    find().update(|f| f.matching = Matching::Regex);
    open(false);
}

/// "Select All Results": a selection on every occurrence of the selected
/// text (a caret: its word). "in Selection" keeps only the occurrences
/// inside what was selected.
fn select_all_results(in_selection: bool) {
    let Some(api) = active_api() else { return; };
    let bounds: Vec<_> = api
        .selections()
        .iter()
        .map(|s| (api.position_to_offset(s.anchor), api.position_to_offset(s.head)))
        .map(|(a, h)| a.min(h)..a.max(h))
        .collect();
    api.run(EditorCommand::SelectAllOccurrences);
    if !in_selection { return; }
    spawn(async move {
        next_frame().await;
        let inside: Vec<_> = api
            .selections()
            .into_iter()
            .filter(|s| {
                let (a, h) = (api.position_to_offset(s.anchor), api.position_to_offset(s.head));
                let (lo, hi) = (a.min(h), a.max(h));
                bounds.iter().any(|b| b.start <= lo && hi <= b.end)
            })
            .collect();
        if !inside.is_empty() { api.set_selections(inside); }
    });
}

/// "Replace and Find Next/Previous": replaces the current match with the
/// bar's replacement and steps to the next (or the one before it).
fn replace_and_find(forward: bool) {
    let Some(api) = active_api() else { return; };
    let with = find().with_untracked(|f| f.replacement.clone());
    api.replace(with);
    if !forward {
        spawn(async move {
            next_frame().await;
            api.run(EditorCommand::FindPrevious);
        });
    }
}

/// "Replace All in Selection": `replace_all` under the search's
/// in-selection scope, restored when the commit is done.
fn replace_all_in_selection() {
    find().update(|f| f.in_selection = true);
    spawn(async move {
        next_frame().await;
        replace_all();
        find().update(|f| f.in_selection = false);
    });
}

fn commit_go_to() {
    let (draft, lines) = goto().with_untracked(|g| (g.draft.clone(), g.lines));
    match parse_line_spec(&draft, lines) {
        Ok((line, column)) => {
            goto().update(|g| { g.open = false; g.error = None; });
            let Some(id) = active() else { return; };
            state::record_jump();
            if let Some(api) = state::editor_api(id) {
                let target = line - 1;
                api.go_to_line(target);
                if let Some(column) = column {
                    let text = api.value();
                    let width = text.split('\n').nth(target as usize).map_or(0, |l| l.encode_utf16().count() as u32);
                    let at = EditorPosition::new(target, (column - 1).min(width));
                    api.set_selections(vec![EditorSelection::caret(at)]);
                    api.reveal_position(at, true);
                }
            }
            state::focus(id);
        }
        Err(reason) => goto().update(|g| g.error = Some(reason)),
    }
}

fn cancel_go_to() {
    goto().update(|g| g.open = false);
    if let Some(id) = active() { state::focus(id); }
}

// ---------------------------------------------------------------- Pure logic

fn search_of(f: &Find) -> EditorSearch {
    let mut search = EditorSearch::new(f.query.clone());
    search.case_sensitive = !f.ignore_case;
    search.whole_word = f.matching == Matching::Word;
    search.regex = f.matching == Matching::Regex;
    search.in_selection = f.in_selection;
    search
}

/// The count shown in the field: `3 of 12`, `12 matches`, `No matches`, or
/// `Regex Invalid`; nothing while the query is empty.
fn count_label(query_empty: bool, result: Option<&EditorSearchResult>) -> Option<String> {
    if query_empty { return None; }
    let result = result?;
    if result.error.is_some() { return Some("Regex Invalid".into()); }
    let more = if result.count >= 10_000 { "+" } else { "" };
    Some(match (result.count, result.current) {
        (0, _) => "No matches".into(),
        (count, Some(current)) => format!("{} of {count}{more}", current + 1),
        (1, None) => "1 match".into(),
        (count, None) => format!("{count}{more} matches"),
    })
}

/// Whether a step from `from` to `to` went past the end (forward) or the
/// start (backward) and came round.
fn wrapped(forward: bool, from: &EditorSelection, to: &EditorSelection) -> bool {
    let start = |s: &EditorSelection| {
        let (a, h) = ((s.anchor.line, s.anchor.column), (s.head.line, s.head.column));
        a.min(h)
    };
    if forward { start(to) < start(from) } else { start(to) > start(from) }
}

/// The word around UTF-16 column `column` of `line` (letters, digits, `_`).
pub(super) fn word_at(line: &str, column: u32) -> Option<String> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<(u32, char)> = line.chars().scan(0u32, |at, c| {
        let start = *at;
        *at += c.len_utf16() as u32;
        Some((start, c))
    }).collect();
    // The character after the caret, else the one before it.
    let index = chars.iter().position(|&(at, c)| at == column && is_word(c))
        .or_else(|| chars.iter().position(|&(at, c)| at + c.len_utf16() as u32 == column && is_word(c)))?;
    let mut first = index;
    while first > 0 && is_word(chars[first - 1].1) { first -= 1; }
    let mut last = index;
    while last + 1 < chars.len() && is_word(chars[last + 1].1) { last += 1; }
    Some(chars[first..=last].iter().map(|&(_, c)| c).collect())
}

/// The lines of `text` as the editor counts them.
fn line_count(text: &str) -> u32 { text.split('\n').count() as u32 }

/// Parses Go to Line's `line` or `line:column` (both from 1) for a text of
/// `lines` lines.
fn parse_line_spec(input: &str, lines: u32) -> Result<(u32, Option<u32>), String> {
    let usage = || "Enter a line number, or line:column.".to_owned();
    let input = input.trim();
    let (line, column) = match input.split_once(':') {
        Some((line, column)) => (line.trim(), Some(column.trim())),
        None => (input, None),
    };
    let number = |s: &str| s.parse::<u32>().ok().filter(|n| *n > 0);
    let line = number(line).ok_or_else(usage)?;
    let column = match column {
        Some(column) => Some(number(column).ok_or_else(usage)?),
        None => None,
    };
    if line > lines {
        return Err(if lines == 1 { "The document has 1 line.".into() } else { format!("The document has {lines} lines.") });
    }
    Ok((line, column))
}

// ---------------------------------------------------------------- Views

/// The find bar, shown above the editors while it is open.
pub fn bar() -> impl IntoElement {
    when(move || find().with(|f| f.open), find_bar)
}

fn find_bar() -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(6.0))
        .bg(tokens::editor_bar)
        .border_b_1()
        .border_color(tokens::editor_bar_border)
        .role(AccessibilityRole::Search)
        .label("Find")
        .child(when(editor_is_narrow, narrow_body).otherwise(wide_body))
}

/// Whether the editor column is too narrow for the bar's single row.
/// DESIGN 8.6.4 starts the stacked variant below 560 pt; this bar's control
/// set (a three-segment matching choice and two labelled checkboxes) needs
/// about 820 pt before the row stops fitting, so the breakpoint follows the
/// content rather than the spec's narrower control set.
fn editor_is_narrow() -> bool {
    let width = window().width.0;
    let (sidebar_visible, docs_visible, console_visible, console_width) = state::display()
        .with(|view| (view.sidebar_visible, view.docs_visible, view.console_visible, view.console_width));
    let sidebar = if sidebar_visible { if width < 720.0 { 200.0 } else { 240.0 } } else { 0.0 };
    let docs = if docs_visible { 300.0 } else { 0.0 };
    let console = if console_visible && super::console::console_on_right() { console_width } else { 0.0 };
    width - sidebar - docs - console < 820.0
}

/// The wide bar: one row of controls, then the replace row.
fn wide_body() -> impl IntoElement {
    let replacing = computed(move || find().with(|f| f.replace));
    let sm = ControlSize::Sm;
    view().flex_col().gap(px(8.0))
        .child(view().flex_row().items_center().gap(px(8.0))
            .child(find_replace_segmented(sm))
            .child(find_input(160.0))
            .child(matching_segmented(sm))
            .child(ignore_case_checkbox())
            .child(wrap_checkbox())
            .child(prev_next(sm))
            .child(done_button(sm)))
        .child(when(move || replacing.get(), move || replace_row(sm, 160.0)))
}

/// The narrow bar: the field keeps its own row, matching becomes a popup,
/// and the two toggles move into an options menu.
fn narrow_body() -> impl IntoElement {
    let replacing = computed(move || find().with(|f| f.replace));
    view().flex_col().gap(px(6.0))
        .child(view().flex_row().items_center().gap(px(6.0))
            .child(find_replace_segmented(ControlSize::Sm))
            .child(matching_popup()))
        .child(view().flex_row().items_center().gap(px(6.0))
            .child(find_input(100.0))
            .child(prev_next(ControlSize::Sm))
            .child(options_button())
            .child(done_button(ControlSize::Sm)))
        .child(when(move || replacing.get(), move || replace_row(ControlSize::Sm, 100.0)))
}

fn find_replace_segmented(size: ControlSize) -> impl IntoElement {
    let replacing = computed(move || find().with(|f| f.replace));
    segmented(
        "Find or replace",
        [Segment::new("Find"), Segment::new("Replace")],
        move || usize::from(replacing.get()),
        move |index| {
            motion::animate_panel();
            find().update(|f| f.replace = index == 1);
        },
        SegmentedOptions::new().size(size),
    )
}

fn matching_segmented(size: ControlSize) -> impl IntoElement {
    segmented(
        "Matching",
        [Segment::new("Text"), Segment::new("Word"), Segment::new("Regex")],
        move || match find().with(|f| f.matching) { Matching::Text => 0, Matching::Word => 1, Matching::Regex => 2 },
        move |index| find().update(|f| f.matching = [Matching::Text, Matching::Word, Matching::Regex][index.min(2)]),
        SegmentedOptions::new().size(size),
    )
}

fn matching_popup() -> impl IntoElement {
    view().flex_shrink(0.0).child(dynamic(move || {
        let _ = find().with(|f| f.matching);
        popup_button(
            "Choose how the search matches",
            move || SharedString::from(matching_label()),
            matching_items(),
            PopupButtonOptions::new().size(ControlSize::Sm),
            |_open: bool| {},
        )
        .into_element()
    }))
}

fn matching_label() -> &'static str {
    match find().with(|f| f.matching) {
        Matching::Text => "Text",
        Matching::Word => "Word",
        Matching::Regex => "Regex",
    }
}

fn matching_items() -> Vec<MenuItem> {
    let current = find().with_untracked(|f| f.matching);
    [Matching::Text, Matching::Word, Matching::Regex]
        .into_iter()
        .map(|matching| {
            MenuItem::new(matching_label_of(matching))
                .checked(current == matching)
                .on_select(move || find().update(|f| f.matching = matching))
        })
        .collect()
}

fn matching_label_of(matching: Matching) -> &'static str {
    match matching {
        Matching::Text => "Text",
        Matching::Word => "Word",
        Matching::Regex => "Regex",
    }
}

fn ignore_case_checkbox() -> impl IntoElement {
    checkbox(
        "Ignore Case",
        move || find().with(|f| f.ignore_case),
        CheckboxOptions::new(),
        move |on| find().update(|f| f.ignore_case = on),
    )
}

fn wrap_checkbox() -> impl IntoElement {
    checkbox(
        "Wrap Around",
        move || find().with(|f| f.wrap),
        CheckboxOptions::new(),
        move |on| find().update(|f| f.wrap = on),
    )
}

fn prev_next(size: ControlSize) -> impl IntoElement {
    view().flex_row().gap(px(2.0))
        .child(icon_button(IconName::ChevronUp, "Find Previous", IconButtonOptions::new().size(size).shortcut("⇧⌘G"), || find_next(false)))
        .child(icon_button(IconName::ChevronDown, "Find Next", IconButtonOptions::new().size(size).shortcut("⌘G"), || find_next(true)))
}

fn done_button(size: ControlSize) -> impl IntoElement {
    button("Done", ButtonOptions::new().variant(ButtonVariant::Default).size(size), close)
}

fn options_button() -> impl IntoElement {
    icon_button(IconName::Ellipsis, "Find options", IconButtonOptions::new().size(ControlSize::Sm), || {
        spawn(async {
            let _ = popup_menu(options_items()).await;
        });
    })
}

fn options_items() -> Vec<MenuItem> {
    let (ignore, wrap) = find().with_untracked(|f| (f.ignore_case, f.wrap));
    vec![
        MenuItem::new("Ignore Case")
            .checked(ignore)
            .on_select(|| find().update(|f| f.ignore_case = !f.ignore_case)),
        MenuItem::new("Wrap Around")
            .checked(wrap)
            .on_select(|| find().update(|f| f.wrap = !f.wrap)),
    ]
}

fn find_input(min_w: f32) -> impl IntoElement {
    let generation = computed(move || find().with(|f| f.generation));
    let invalid = computed(move || current_result().is_some_and(|r| r.error.is_some()));
    view().flex_1().min_w(px(min_w)).child(dynamic(move || {
        let _ = generation.get();
        let (initial, focus) = find().with_untracked(|f| (f.query.clone(), f.focus_field));
        find_field(initial, focus, invalid)
    }))
}

fn replace_row(size: ControlSize, min_w: f32) -> impl IntoElement {
    view().flex_row().items_center().gap(px(8.0))
        .child(view().flex_1().min_w(px(min_w)).child(replace_field()))
        .child(button("Replace", ButtonOptions::new().variant(ButtonVariant::Default).size(size), replace_one))
        .child(button("All", ButtonOptions::new().variant(ButtonVariant::Default).size(size), replace_all))
}

/// The active editor's last search result.
fn current_result() -> Option<EditorSearchResult> {
    let id = state::display().with(|view| view.active)?;
    results().with(|r| r.0.iter().find(|(doc, _)| *doc == id).map(|(_, result)| result.clone()))
}

fn focus_changed(focused: bool) { global::<FieldFocus>().set(FieldFocus(focused)); }

/// The find field: SearchField sm (DESIGN.md 7.5) drawn from the kit's
/// tokens. Built here rather than with `search_field` because the bar needs
/// the field's focus, Return and its text selected on opening, which the kit
/// does not expose yet. Made again (keyed by `generation`) whenever the app
/// sets the query.
fn find_field(initial: String, focus: bool, invalid: Memo<bool>) -> impl IntoElement {
    let size = ControlSize::Sm;
    let length = initial.encode_utf16().count() as u32;
    let field = input()
        .default_value(initial)
        .selected_range((0, length))
        .auto_focus(focus)
        .placeholder("Find")
        .label("Search text")
        .submit_behavior(SubmitBehavior::Submit)
        .on_change(move |text: &str| find().update(|f| f.query = text.to_owned()))
        .on_submit(move |_: &str| find_next(true))
        .on_focus(|| focus_changed(true))
        .on_blur(|| focus_changed(false))
        .placeholder_color(move || color(tokens::muted_foreground()))
        .caret_color(move || color(tokens::primary()))
        .selection_color(move || color(tokens::selection()))
        .flex_1()
        .min_w(px(0.0))
        .text_size(px(size.text_size()))
        .line_height(px(tokens::leading_body()))
        .text_color(tokens::foreground);
    view()
        .h(px(size.height()))
        .px(px(size.padding_x()))
        .flex_row()
        .items_center()
        .gap(px(size.gap()))
        .rounded_full()
        .bg(tokens::field)
        .border_1()
        .border_color(move || if invalid.get() { tokens::destructive() } else { tokens::input() })
        .transition_colors()
        .duration(motion::instant)
        .focus_within(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
        .child(glyph(IconName::Search, IconSize::Xs, Tone::Muted))
        .child(field)
        .child(type_caption().apply(text(move || {
            let empty = find().with(|f| f.query.is_empty());
            count_label(empty, current_result().as_ref()).unwrap_or_default()
        })).text_color(move || if invalid.get() { tokens::destructive() } else { tokens::muted_foreground() }))
        .child(when(move || find().with(|f| !f.query.is_empty()), || {
            icon_button(IconName::CloseCircle, "Clear", IconButtonOptions::new().size(ControlSize::Xs), || {
                find().update(|f| { f.query.clear(); f.focus_field = true; f.generation += 1; });
            })
        }))
}

/// The replace field: TextField sm (DESIGN.md 7.6), from the kit's tokens
/// for the same reason as [`find_field`].
fn replace_field() -> impl IntoElement {
    let size = ControlSize::Sm;
    input()
        .value(move || find().with(|f| f.replacement.clone()))
        .on_change(move |text: &str| find().update(|f| f.replacement = text.to_owned()))
        .on_submit(move |_: &str| replace_one())
        .on_focus(|| focus_changed(true))
        .on_blur(|| focus_changed(false))
        .submit_behavior(SubmitBehavior::Submit)
        .placeholder("Replace")
        .label("Replacement text")
        .placeholder_color(move || color(tokens::muted_foreground()))
        .caret_color(move || color(tokens::primary()))
        .selection_color(move || color(tokens::selection()))
        .w_full()
        .h(px(size.height()))
        .px(px(8.0))
        .text_size(px(size.text_size()))
        .line_height(px(tokens::leading_body()))
        .text_color(tokens::foreground)
        .bg(tokens::field)
        .border_1()
        .border_color(tokens::input)
        .rounded(px(tokens::radius()))
        .focus(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
}

/// Dialogs of this module, mounted once at the root.
pub fn overlay() -> impl IntoElement {
    when(move || goto().with(|g| g.open), go_to_sheet)
}

/// Go to Line: the prompt sheet of DESIGN.md 8.11.
fn go_to_sheet() -> impl IntoElement {
    let valid = computed(move || goto().with(|g| parse_line_spec(&g.draft, g.lines).is_ok()));
    let error = computed(move || goto().with(|g| g.error.clone()));
    let sheet_motion = motion().entering(
        MotionSpec::new(KeyframeTiming::new(motion::slow()).easing(motion::EASE_ENTER))
            .values(MotionChannel::Opacity, [0.0, 1.0])
            .values(MotionChannel::TranslateY, [-motion::DISTANCE_SHEET, 0.0])
            .values(MotionChannel::Scale, [motion::POPUP_SCALE, 1.0]),
    );
    let scrim_motion = motion().entering(
        MotionSpec::new(KeyframeTiming::new(motion::slow()).easing(motion::EASE_ENTER))
            .values(MotionChannel::Opacity, [0.0, 1.0]),
    );
    let submit = move || {
        commit_go_to();
        if goto().with_untracked(|g| g.open) { let _ = sheet_motion.play(MotionPreset::Shake); }
    };
    let field = input()
        .value(move || goto().with(|g| g.draft.clone()))
        .on_change(move |text: &str| goto().update(|g| { g.draft = text.to_owned(); g.error = None; }))
        .on_submit(move |_: &str| submit())
        .submit_behavior(SubmitBehavior::Submit)
        .auto_focus(true)
        .placeholder("Line Number")
        .label("Line Number")
        .placeholder_color(move || color(tokens::muted_foreground()))
        .caret_color(move || color(tokens::primary()))
        .selection_color(move || color(tokens::selection()))
        .w_full()
        .h(px(ControlSize::Lg.height()))
        .px(px(8.0))
        .text_size(px(ControlSize::Lg.text_size()))
        .line_height(px(tokens::leading_body()))
        .text_color(tokens::foreground)
        .bg(tokens::field)
        .border_1()
        .border_color(move || if error.get().is_some() { tokens::destructive() } else { tokens::input() })
        .rounded(px(tokens::radius()))
        .focus(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)));
    dialog()
        .label("Go to Line")
        .on_cancel(cancel_go_to)
        .flex_1()
        .child(view().absolute().inset_0().bg(tokens::scrim).bind_motion(&scrim_motion).on_press(cancel_go_to))
        .child(view().flex_1().items_center().pt(px(44.0))
            .child(view()
                .w(px(420.0))
                .p(px(20.0))
                .gap(px(12.0))
                .rounded(px(tokens::radius_xl()))
                .bg(tokens::background)
                .border_1()
                .border_color(tokens::border)
                .shadow_lg()
                .shadow_color(tokens::shadow())
                .bind_motion(&sheet_motion)
                .child(type_subhead().semibold().apply(text("Go to Line")).text_color(tokens::foreground))
                .child(type_body().apply(text("Enter a line number, or line:column.")).text_color(tokens::muted_foreground))
                .child(view().gap(px(4.0))
                    .child(field)
                    .child(when(move || error.get().is_some(), move || {
                        type_caption().apply(text(move || error.get().unwrap_or_default())).text_color(tokens::destructive)
                    })))
                .child(view().flex_row().justify_end().gap(px(8.0)).mt(px(4.0))
                    .child(button("Cancel", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Lg), cancel_go_to))
                    .child(dynamic(move || {
                        let ready = valid.get();
                        button("Go", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Lg).disabled(!ready), submit)
                    })))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(count: u32, current: Option<u32>) -> EditorSearchResult {
        let mut r = EditorSearchResult::default();
        r.count = count;
        r.current = current;
        r
    }

    #[test]
    fn counts_read_as_the_design_says() {
        assert_eq!(count_label(true, Some(&result(3, None))), None);
        assert_eq!(count_label(false, None), None);
        assert_eq!(count_label(false, Some(&result(0, None))).as_deref(), Some("No matches"));
        assert_eq!(count_label(false, Some(&result(12, Some(2)))).as_deref(), Some("3 of 12"));
        assert_eq!(count_label(false, Some(&result(12, None))).as_deref(), Some("12 matches"));
        assert_eq!(count_label(false, Some(&result(1, None))).as_deref(), Some("1 match"));
        assert_eq!(count_label(false, Some(&result(10_000, None))).as_deref(), Some("10000+ matches"));
        let mut bad = result(0, None);
        bad.error = Some("unclosed group".into());
        assert_eq!(count_label(false, Some(&bad)).as_deref(), Some("Regex Invalid"));
    }

    #[test]
    fn options_map_to_the_editor_search() {
        let mut f = Find { query: "a.b".into(), ..Find::default() };
        let s = search_of(&f);
        assert!(!s.regex && !s.whole_word && !s.case_sensitive);
        f.matching = Matching::Regex;
        f.ignore_case = false;
        let s = search_of(&f);
        assert!(s.regex && s.case_sensitive && !s.whole_word);
        f.matching = Matching::Word;
        assert!(search_of(&f).whole_word);
    }

    #[test]
    fn wrapping_is_seen_from_the_selections() {
        let at = |line, column| EditorSelection::new(EditorPosition::new(line, column), EditorPosition::new(line, column + 3));
        assert!(!wrapped(true, &at(1, 0), &at(4, 2)));
        assert!(wrapped(true, &at(4, 2), &at(1, 0)));
        assert!(!wrapped(false, &at(4, 2), &at(1, 0)));
        assert!(wrapped(false, &at(1, 0), &at(4, 2)));
        // A backwards selection counts from its start.
        let backwards = EditorSelection::new(EditorPosition::new(2, 5), EditorPosition::new(2, 1));
        assert!(!wrapped(true, &backwards, &at(2, 4)));
    }

    #[test]
    fn the_word_at_the_caret() {
        assert_eq!(word_at("let foo_bar = 1;", 4).as_deref(), Some("foo_bar"));
        assert_eq!(word_at("let foo_bar = 1;", 11).as_deref(), Some("foo_bar"));
        assert_eq!(word_at("let foo_bar = 1;", 12), None);
        assert_eq!(word_at("", 0), None);
        // Columns are UTF-16 units: the emoji is two.
        assert_eq!(word_at("😀 héllo", 3).as_deref(), Some("héllo"));
    }

    #[test]
    fn go_to_line_input() {
        assert_eq!(parse_line_spec("12", 40), Ok((12, None)));
        assert_eq!(parse_line_spec(" 3:7 ", 40), Ok((3, Some(7))));
        assert_eq!(parse_line_spec("3 : 7", 40), Ok((3, Some(7))));
        assert_eq!(parse_line_spec("40", 40), Ok((40, None)));
        assert_eq!(parse_line_spec("41", 40), Err("The document has 40 lines.".into()));
        assert_eq!(parse_line_spec("2", 1), Err("The document has 1 line.".into()));
        for bad in ["", "0", "-1", "abc", "3:", "3:0", ":4", "1.5", "3:x"] {
            assert_eq!(parse_line_spec(bad, 40), Err("Enter a line number, or line:column.".into()), "{bad:?}");
        }
        assert_eq!(line_count("a\nb\n"), 3);
        assert_eq!(line_count(""), 1);
    }

    #[test]
    fn menu_items_are_bound_commands() {
        for &(title, id, _) in MENU {
            if matches!(title, "-" | "%") { continue; }
            assert!(super::super::bindings::command(id).is_some(), "{title}: {id} is not in bindings::COMMANDS");
        }
    }
}
