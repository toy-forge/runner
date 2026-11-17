//! Text commands on the selected editor: comment, indent, move and duplicate
//! lines, case, selection, font size, and the editor display options.
//!
//! Most commands are the NSH editor's own (`EditorCommand`), run on the
//! active editor. Un/Comment Lines uses the language definition's line
//! comment marker when it has one, and the editor's own otherwise. Font size
//! and the display options write the preferences.
//!
//! Motion inventory: none of its own. Edits land at once in the editor
//! (moving lines is the text changing, not a view moving), and the font size
//! relayouts the editor; menus are native.

use std::collections::BTreeSet;
use std::rc::Rc;

use nsh_base::editor::EditorEdit;
use nsh_base::{EditorApi, EditorCommand, EditorPosition, EditorRange, EditorSelection};
use nsh_core::element::desktop::MenuItem;
use nsh_core::keymap::When;
use nsh_core::prelude::*;
use scratch_config::Preferences;
use scratch_language::{MenuEntry, MenuOrder, PLAIN_TEXT_ID};

use super::{bindings, preferences, state};

/// Keyboard shortcuts of this module. Called once from the root.
///
/// The editor binds Cmd-/, Cmd-D and Opt-Cmd-Up/Down itself each time it
/// takes the focus, and the newest binding wins; these are bound again after
/// every focus change so the menu's meaning (DESIGN.md 10.1) wins. Shift
/// Right and Left (Cmd-] / Cmd-[) are the editor's own and not bound here.
/// The keys come from the key bindings: a change re-registers them.
pub fn install() {
    effect(|| {
        let _ = global::<super::EditorFocus>().get();
        let _ = global::<bindings::BindingsRevision>().get();
        for &(id, action, repeat) in KEYS {
            let options = KeyOptions { when: When::Always, repeat, ..KeyOptions::default() };
            bindings::on_with(id, options, action);
        }
        // Shift Right and Left are the editor's own keys; registered for
        // compound-command dispatch without binding.
        bindings::register("text.shiftRight", || run(EditorCommand::Indent));
        bindings::register("text.shiftLeft", || run(EditorCommand::Outdent));
        bindings::register("window.selectTab10", || state::activate_tab(9));
        // A second way to type Bigger (Shift is often held for `+`).
        on_key_with("mod+shift+plus", KeyOptions { when: When::Always, repeat: true, ..KeyOptions::default() }, |_| zoom(1.0));
    });
}

/// The keys this module binds: bound-command id, action, and whether a held
/// key repeats it. Arrows and Backspace are text keys, so they fire with the
/// editor focused only as [`When::Always`].
const KEYS: &[(&str, fn(), bool)] = &[
    ("text.toggleComment", toggle_comment, false),
    ("text.duplicateLine", || run(EditorCommand::DuplicateLinesDown), true),
    ("text.deleteLine", || run(EditorCommand::DeleteLine), true),
    ("text.moveLineUp", || run(EditorCommand::MoveLinesUp), true),
    ("text.moveLineDown", || run(EditorCommand::MoveLinesDown), true),
    ("text.selectLine", || run(EditorCommand::SelectLine), true),
    ("text.font.bigger", || zoom(1.0), true),
    ("text.font.smaller", || zoom(-1.0), true),
    ("sr.actualSize", reset_zoom, false),
    ("text.options.showInvisibles", || toggle(Display::Invisibles), false),
    ("text.options.pageGuide", || toggle(Display::PageGuide), false),
    ("text.options.scrollPastEnd", || toggle(Display::ScrollPastEnd), false),
    ("text.options.lineNumbers", || toggle(Display::LineNumbers), false),
    ("text.options.wrapLines", || toggle(Display::WrapLines), false),
    ("text.options.currentLine", || toggle(Display::CurrentLine), false),
    ("text.showCompletions", || run(EditorCommand::ShowCompletions), false),
    ("text.lookupSymbol", super::docs::look_up_current_symbol, false),
    ("text.autoIndentSelection", auto_indent_selection, false),
    ("text.selectColumnUp", || run(EditorCommand::AddCaretAbove), false),
    ("text.selectColumnDown", || run(EditorCommand::AddCaretBelow), false),
    ("text.splitSelectionByLines", || run(EditorCommand::SplitSelectionIntoLines), false),
    ("text.singleSelection", || run(EditorCommand::SingleSelection), false),
    ("text.selectWord", || run(EditorCommand::SelectWord), false),
    ("text.selectToDelimiters", || select_to_delimiters(Delimiters::Whole), false),
    ("text.selectToDelimitersInwards", || select_to_delimiters(Delimiters::Inwards), false),
    ("text.selectToDelimitersRight", || select_to_delimiters(Delimiters::Right), false),
    ("text.selectToDelimitersLeft", || select_to_delimiters(Delimiters::Left), false),
    ("text.insertSelectionNumber", insert_selection_number, false),
];

/// The Text menu's items.
pub fn menu_items() -> Vec<MenuItem> {
    let item = |title: &str, id: &str, action: fn()| {
        let item = MenuItem::new(title).on_select(action);
        let keys = bindings::keys_for(id);
        if keys.is_empty() { item } else { item.shortcut(&keys) }
    };
    let prefs = preferences::settings().get_untracked();
    let check = |which: Display| {
        MenuItem::new(which.title()).checked(which.get(&prefs)).on_select(move || toggle(which))
    };
    vec![
        item("Show Completions", "text.showCompletions", || run(EditorCommand::ShowCompletions)),
        item("Look Up Current Symbol", "text.lookupSymbol", super::docs::look_up_current_symbol),
        MenuItem::separator(),
        item("Auto-Indent Selection", "text.autoIndentSelection", auto_indent_selection),
        item("Shift Right", "text.shiftRight", || run(EditorCommand::Indent)),
        item("Shift Left", "text.shiftLeft", || run(EditorCommand::Outdent)),
        item("Un/Comment Lines", "text.toggleComment", toggle_comment),
        MenuItem::separator(),
        item("Select Column Up", "text.selectColumnUp", || run(EditorCommand::AddCaretAbove)),
        item("Select Column Down", "text.selectColumnDown", || run(EditorCommand::AddCaretBelow)),
        item("Split Selection By Lines", "text.splitSelectionByLines", || run(EditorCommand::SplitSelectionIntoLines)),
        item("Split Selection By Words", "text.splitSelectionByWords", split_selection_by_words),
        item("Single Selection", "text.singleSelection", || run(EditorCommand::SingleSelection)),
        item("Insert Selection Number", "text.insertSelectionNumber", insert_selection_number),
        MenuItem::separator(),
        item("Select Line", "text.selectLine", || run(EditorCommand::SelectLine)),
        item("Select Word", "text.selectWord", || run(EditorCommand::SelectWord)),
        item("Select to Delimiters", "text.selectToDelimiters", || select_to_delimiters(Delimiters::Whole)),
        item("Select to Delimiters Inwards", "text.selectToDelimitersInwards", || select_to_delimiters(Delimiters::Inwards)),
        item("Select to Delimiters Right", "text.selectToDelimitersRight", || select_to_delimiters(Delimiters::Right)),
        item("Select to Delimiters Left", "text.selectToDelimitersLeft", || select_to_delimiters(Delimiters::Left)),
        MenuItem::submenu("Surround Selection With", {
            let surround_item = |title: &str, id: &str, open: &'static str, close: &'static str| {
                let item = MenuItem::new(title).on_select(move || surround(open, close));
                let keys = bindings::keys_for(id);
                if keys.is_empty() { item } else { item.shortcut(&keys) }
            };
            vec![
                surround_item("Single Quotes", "text.surround.singleQuotes", "'", "'"),
                surround_item("Double Quotes", "text.surround.doubleQuotes", "\"", "\""),
                surround_item("Parentheses", "text.surround.parentheses", "(", ")"),
                surround_item("Braces", "text.surround.braces", "{", "}"),
                surround_item("Brackets", "text.surround.brackets", "[", "]"),
            ]
        }),
        MenuItem::separator(),
        item("Duplicate Line", "text.duplicateLine", || run(EditorCommand::DuplicateLinesDown)),
        item("Delete Line", "text.deleteLine", || run(EditorCommand::DeleteLine)),
        item("Delete Line Up", "", delete_line_up),
        item("Swap Lines Up", "text.moveLineUp", || run(EditorCommand::MoveLinesUp)),
        item("Swap Lines Down", "text.moveLineDown", || run(EditorCommand::MoveLinesDown)),
        item("Go to Line…", "text.goToLine", super::find::go_to_line),
        MenuItem::separator(),
        MenuItem::submenu("Editor Options", [
            {
                let keys = bindings::keys_for("text.options.showInvisibles");
                let item = check(Display::Invisibles);
                if keys.is_empty() { item } else { item.shortcut(&keys) }
            },
            check_keyed(Display::LineNumbers, "text.options.lineNumbers"),
            check_keyed(Display::PageGuide, "text.options.pageGuide"),
            check_keyed(Display::WrapLines, "text.options.wrapLines"),
            check_keyed(Display::ScrollPastEnd, "text.options.scrollPastEnd"),
            check_keyed(Display::CurrentLine, "text.options.currentLine"),
            MenuItem::separator(),
            item("Text Editing Preferences…", "text.preferences", || {
                preferences::open_pane(preferences::Category::TextEditing);
            }),
        ]),
        item("Show Color Scopes", "text.showColorScopes", show_color_scopes),
        MenuItem::separator(),
        MenuItem::submenu("Font", [
            item("Show Fonts", "text.showFonts", show_fonts),
            MenuItem::separator(),
            item("Bigger", "text.font.bigger", || zoom(1.0)),
            item("Smaller", "text.font.smaller", || zoom(-1.0)),
            item("Actual Size", "sr.actualSize", reset_zoom),
        ]),
        MenuItem::submenu("Text Encoding", encoding_items()),
        MenuItem::submenu("Syntax Mode", language_items()),
    ]
}

/// A checked Editor Options item that also shows its bound key.
fn check_keyed(which: Display, id: &str) -> MenuItem {
    let prefs = preferences::settings().get_untracked();
    let item = MenuItem::new(which.title()).checked(which.get(&prefs)).on_select(move || toggle(which));
    let keys = bindings::keys_for(id);
    if keys.is_empty() { item } else { item.shortcut(&keys) }
}

/// Text > Syntax Mode / Run > Language / the toolbar popup: `None` (Plain
/// Text), then one item per enabled language, similar languages grouped
/// into a submenu (CodeRunner's "Group similar languages"). The active
/// document's language is checked. Built at menu-build time; callers
/// rebuild when the registry or the document changes.
pub(super) fn language_items() -> Vec<MenuItem> {
    state::with(|workspace| {
        let active = workspace.active.and_then(|id| workspace.doc(id).map(|doc| doc.document.language.clone()));
        let item = |id: &str, title: &str| {
            let id = id.to_owned();
            MenuItem::new(title.to_owned())
                .checked(active.as_deref() == Some(id.as_str()))
                .on_select(move || {
                    if let Some(doc) = state::with(|workspace| workspace.active) {
                        state::set_language(doc, &id);
                    }
                })
        };
        // `None` is the plain-text document, as CodeRunner's language menu has it.
        let mut items = vec![item(PLAIN_TEXT_ID, "None"), MenuItem::separator()];
        for entry in workspace.registry.as_ref().map(|registry| registry.menu_entries(true, MenuOrder::Alphabetical)).unwrap_or_default() {
            match entry {
                MenuEntry::Language(def) => {
                    // Plain Text's slot is `None`, above the separator.
                    if def.id != PLAIN_TEXT_ID {
                        items.push(item(&def.id, &def.name));
                    }
                }
                MenuEntry::Group { name, languages } => {
                    items.push(MenuItem::submenu(name, languages.iter().map(|def| {
                        item(&def.id, def.variant_name().unwrap_or(&def.name))
                    })));
                }
            }
        }
        items
    })
}

// ---------------------------------------------------------------- Commands

fn active_api() -> Option<Rc<EditorApi>> {
    let id = state::with(|workspace| workspace.active)?;
    state::editor_api(id)
}

/// Runs one of the editor's own commands on the active editor.
pub(super) fn run(command: EditorCommand) {
    if let Some(api) = active_api() { api.run(command); }
}

/// Un/Comment Lines with the language's line comment marker, as one undo
/// step; the editor's own command when the language defines none.
pub(super) fn toggle_comment() {
    let marker = state::with(|workspace| {
        let id = workspace.active?;
        let language = workspace.doc(id)?.document.language.clone();
        workspace.registry.as_ref()?.get(&language)?.comments.line.clone()
    });
    let Some(api) = active_api() else { return; };
    let Some(marker) = marker.filter(|m| !m.trim().is_empty()) else {
        api.run(EditorCommand::ToggleComment);
        return;
    };
    let text = api.value();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut touched = BTreeSet::new();
    for selection in api.selections() {
        let (start, end) = ordered(selection.anchor, selection.head);
        // A selection ending at a line's start leaves that line alone.
        let last = if end.column == 0 && end.line > start.line { end.line - 1 } else { end.line };
        touched.extend(start.line..=last);
    }
    let chosen: Vec<(u32, &str)> = touched.into_iter()
        .filter_map(|line| lines.get(line as usize).map(|text| (line, *text)))
        .collect();
    let edits = comment_edits(&chosen, marker.trim_end());
    if !edits.is_empty() { api.edit(edits); }
}

fn ordered(a: EditorPosition, b: EditorPosition) -> (EditorPosition, EditorPosition) {
    if (a.line, a.column) <= (b.line, b.column) { (a, b) } else { (b, a) }
}

/// Text > Auto-Indent Selection: the touched lines reindented by bracket
/// depth, one indent unit per unmatched `(`/`[`/`{` before them; a line
/// starting with a closer outdents first. Blank lines are left alone.
fn auto_indent_selection() {
    let Some(api) = active_api() else { return; };
    let text = api.value();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut touched = BTreeSet::new();
    for selection in api.selections() {
        let (start, end) = ordered(selection.anchor, selection.head);
        let last = if end.column == 0 && end.line > start.line { end.line - 1 } else { end.line };
        touched.extend(start.line..=last);
    }
    if touched.is_empty() { return; }
    let unit = preferences::settings().with_untracked(|p| {
        let ind = &p.text_editing.indentation;
        if ind.soft_tabs { " ".repeat(ind.tab_width.max(1) as usize) } else { "\t".to_owned() }
    });
    let edits = auto_indent_edits(&lines, &touched, &unit);
    if !edits.is_empty() { api.edit(edits); }
}

/// The edits that normalize `touched` lines' indentation to the bracket
/// level, tracking depth from the document's first line.
fn auto_indent_edits(lines: &[&str], touched: &BTreeSet<u32>, unit: &str) -> Vec<EditorEdit> {
    let mut level = 0i64;
    let mut edits = Vec::new();
    for (i, text) in lines.iter().enumerate() {
        let i = i as u32;
        let trimmed = text.trim_start();
        if trimmed.is_empty() { continue; }
        let lead_closers = trimmed.chars().take_while(|c| matches!(c, ')' | ']' | '}')).count() as i64;
        let own = (level - lead_closers).max(0);
        if touched.contains(&i) {
            let want = unit.repeat(own as usize);
            let ws_chars = text.chars().take_while(|c| *c == ' ' || *c == '\t').count() as u32;
            let current: String = text.chars().take(ws_chars as usize).collect();
            if current != want {
                let start = EditorPosition::new(i, 0);
                let end = EditorPosition::new(i, ws_chars);
                edits.push(EditorEdit::new(EditorRange::new(start, end), want));
            }
        }
        let rest = &trimmed[lead_closers as usize..];
        let net: i64 = rest.chars().map(|c| match c {
            '(' | '[' | '{' => 1,
            ')' | ']' | '}' => -1,
            _ => 0,
        }).sum();
        level = (own + net).max(0);
    }
    edits
}

/// Changes the font size preference by `step` points.
fn zoom(step: f64) {
    preferences::settings().update(|prefs| {
        let font = &mut prefs.text_editing.appearance.font;
        font.size = zoomed(font.size, step);
    });
    preferences::save();
}

fn reset_zoom() {
    preferences::settings().update(|prefs| prefs.text_editing.appearance.font.size = scratch_config::defaults::FONT_SIZE);
    preferences::save();
}

/// Text > Font > Show Fonts (and Preferences > Text Editing > Select…):
/// the system font panel seeded with the editor font; picks drain through
/// `native::take_font_picks`.
pub(super) fn show_fonts() {
    let (family, size) = preferences::settings().with_untracked(|p| {
        (p.text_editing.appearance.font.family.clone(), p.text_editing.appearance.font.size)
    });
    super::native::show_fonts(family, size);
}

/// Which part of the delimiter pair around the carets to select.
#[derive(Clone, Copy)]
enum Delimiters {
    /// The pair and what it wraps.
    Whole,
    /// What it wraps.
    Inwards,
    /// From the caret to the closing delimiter, included.
    Right,
    /// From the caret back to the opening delimiter, included.
    Left,
}

/// The offset of the nearest unmatched opener left of `caret` (`()`/`[]`/`{}`).
fn open_delimiter(units: &[u16], caret: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut i = caret.min(units.len());
    while i > 0 {
        i -= 1;
        match units[i] {
            u if u == u16::from(b')') || u == u16::from(b']') || u == u16::from(b'}') => depth += 1,
            u if u == u16::from(b'(') || u == u16::from(b'[') || u == u16::from(b'{') => {
                if depth == 0 { return Some(i); }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// The offset of the nearest unmatched closer at or right of `caret`.
fn close_delimiter(units: &[u16], caret: usize) -> Option<usize> {
    let mut depth = 0u32;
    for (i, &u) in units.iter().enumerate().skip(caret) {
        match u {
            u if u == u16::from(b'(') || u == u16::from(b'[') || u == u16::from(b'{') => depth += 1,
            u if u == u16::from(b')') || u == u16::from(b']') || u == u16::from(b'}') => {
                if depth == 0 { return Some(i); }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// "Select to Delimiters" and its Inwards/Right/Left variants: the
/// innermost `()`/`[]`/`{}` pair around each caret, or one side of it.
fn select_to_delimiters(which: Delimiters) {
    let Some(api) = active_api() else { return; };
    let units: Vec<u16> = api.value().encode_utf16().collect();
    let pos = |off: usize| api.offset_to_position(off as u32);
    let mut selections = Vec::new();
    for selection in api.selections() {
        let caret = api.position_to_offset(selection.head) as usize;
        let range = match which {
            Delimiters::Left => open_delimiter(&units, caret).map(|o| o..caret),
            Delimiters::Right => close_delimiter(&units, caret).map(|c| caret..c + 1),
            Delimiters::Whole | Delimiters::Inwards => {
                open_delimiter(&units, caret).zip(close_delimiter(&units, caret)).map(|(o, c)| {
                    if matches!(which, Delimiters::Inwards) { o + 1..c } else { o..c + 1 }
                })
            }
        };
        if let Some(range) = range {
            selections.push(EditorSelection::new(pos(range.start), pos(range.end)));
        }
    }
    if !selections.is_empty() {
        api.set_selections(selections);
    }
}

/// "Split Selection By Words": every selection becomes one selection per
/// word (`[A-Za-z0-9_]+` run) it covers.
pub fn split_selection_by_words() {
    let Some(api) = active_api() else { return; };
    let units: Vec<u16> = api.value().encode_utf16().collect();
    let is_word = |unit: u16| u8::try_from(unit).is_ok_and(|b| b.is_ascii_alphanumeric()) || unit == u16::from(b'_');
    let mut words = Vec::new();
    for selection in api.selections() {
        let anchor = api.position_to_offset(selection.anchor) as usize;
        let head = api.position_to_offset(selection.head) as usize;
        let (lo, hi) = (anchor.min(head), anchor.max(head).min(units.len()));
        let mut at = lo;
        while at < hi {
            if !is_word(units[at]) {
                at += 1;
                continue;
            }
            let start = at;
            while at < hi && is_word(units[at]) {
                at += 1;
            }
            words.push(EditorSelection::new(
                api.offset_to_position(start as u32),
                api.offset_to_position(at as u32),
            ));
        }
    }
    if !words.is_empty() {
        api.set_selections(words);
    }
}

/// "Delete Line Up": deletes the line above every caret line (the
/// caret line itself stays).
fn delete_line_up() {
    let Some(api) = active_api() else { return; };
    let length = api.value().encode_utf16().count() as u32;
    let mut targets: BTreeSet<u32> = api
        .selections()
        .iter()
        .map(|selection| selection.head.line)
        .filter(|line| *line > 0)
        .map(|line| line - 1)
        .collect();
    if targets.is_empty() {
        return;
    }
    let mut edits = Vec::new();
    while let Some(line) = targets.pop_last() {
        let start = EditorPosition::new(line, 0);
        // Deleting to the next line's start removes the newline too; the
        // last line without one deletes to the document's end.
        let end = EditorPosition::new(line + 1, 0);
        let end = if api.position_to_offset(end) <= api.position_to_offset(start) {
            api.offset_to_position(length)
        } else {
            end
        };
        edits.push(EditorEdit::new(EditorRange::new(start, end), ""));
    }
    api.edit(edits);
}

/// Sets the active document's text encoding (Text > Text Encoding); used by
/// the next save.
fn set_encoding(encoding: scratch_document::TextEncoding) {
    let Some(id) = state::with(|workspace| workspace.active) else {
        return;
    };
    state::with(|workspace| {
        if let Some(doc) = workspace.doc_mut(id) {
            doc.document.encoding = encoding;
            doc.document.mark_edited(scratch_document::now_ms());
            workspace.changed();
        }
    });
    state::publish();
    state::notice(format!("Text Encoding: {}", encoding.display_name()));
}

/// The Text Encoding submenu's items, checked on the document's encoding.
fn encoding_items() -> Vec<MenuItem> {
    let current = state::with(|workspace| {
        workspace.active.and_then(|id| workspace.doc(id)).map(|doc| doc.document.encoding)
    });
    scratch_document::TextEncoding::ALL
        .iter()
        .map(|&encoding| {
            MenuItem::new(encoding.display_name())
                .checked(Some(encoding) == current)
                .on_select(move || set_encoding(encoding))
        })
        .collect()
}

/// "Surround Selection With": inserts `left`/`right` around every selection.
pub fn surround(left: &'static str, right: &'static str) {
    let Some(api) = active_api() else { return; };
    let mut edits: Vec<EditorEdit> = Vec::new();
    for selection in api.selections() {
        let (start, end) = ordered(selection.anchor, selection.head);
        edits.push(EditorEdit::new(EditorRange::new(end, end), right));
        edits.push(EditorEdit::new(EditorRange::new(start, start), left));
    }
    edits.sort_by_key(|a| std::cmp::Reverse((a.range.start.line, a.range.start.column)));
    api.edit(edits);
}

/// "Insert Selection Number": every selection is replaced by its 1-based
/// number in caret order.
pub fn insert_selection_number() {
    let Some(api) = active_api() else { return; };
    let mut edits: Vec<EditorEdit> = api
        .selections()
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            let (start, end) = ordered(selection.anchor, selection.head);
            EditorEdit::new(EditorRange::new(start, end), (index + 1).to_string())
        })
        .collect();
    edits.sort_by_key(|a| std::cmp::Reverse((a.range.start.line, a.range.start.column)));
    api.edit(edits);
}

/// "Copy with Colors": the selection as theme-styled HTML and plain text on
/// the pasteboard (Edit menu).
pub(super) fn copy_with_colors() {
    let Some(id) = state::with(|workspace| workspace.active) else { return };
    let Some(api) = state::editor_api(id) else { return };
    let range = api
        .selections()
        .iter()
        .map(|selection| ordered(selection.anchor, selection.head))
        .find(|(start, end)| start != end);
    let Some((start, end)) = range else {
        state::notice("Nothing is selected.");
        return;
    };
    if let Some((plain, html)) = super::theme::colored_html(id, start, end) {
        super::native::write_colored(&html, &plain);
    }
}

/// The editor's right-click menu (CodeRunner's): clipboard and case
/// transforms over the selection, completions, spelling and speech.
pub(super) fn context_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Cut").on_select(|| run(EditorCommand::Cut)),
        MenuItem::new("Copy").on_select(|| run(EditorCommand::Copy)),
        MenuItem::new("Copy with Colors").on_select(copy_with_colors),
        MenuItem::new("Paste").on_select(|| run(EditorCommand::Paste)),
        MenuItem::new("Paste and Preserve Formatting").on_select(|| run(EditorCommand::Paste)),
        MenuItem::new("Delete").on_select(|| run(EditorCommand::DeleteForward)),
        MenuItem::new("Select All").on_select(|| run(EditorCommand::SelectAll)),
        MenuItem::separator(),
        MenuItem::new("Show Completions").on_select(|| run(EditorCommand::ShowCompletions)),
        MenuItem::separator(),
        MenuItem::submenu("Transformations", [
            MenuItem::new("Make Upper Case").on_select(|| run(EditorCommand::TransformToUppercase)),
            MenuItem::new("Make Lower Case").on_select(|| run(EditorCommand::TransformToLowercase)),
            MenuItem::new("Capitalize").on_select(|| run(EditorCommand::TransformToTitleCase)),
        ]),
        MenuItem::submenu("Speech", [
            MenuItem::new("Start Speaking").on_select(start_speaking),
            MenuItem::new("Stop Speaking").disabled(!super::native::is_speaking()).on_select(super::native::stop_speaking),
        ]),
        MenuItem::separator(),
        MenuItem::new("Start Dictation…").on_select(super::native::start_dictation),
        MenuItem::new("Emoji & Symbols").on_select(super::native::emoji_and_symbols),
    ]
}

/// Edit > Speech > Start Speaking: the selection, or the whole document
/// when nothing is selected.
pub(super) fn start_speaking() {
    let Some(api) = active_api() else { return };
    let text = api.selected_text();
    let text = if text.is_empty() { api.value() } else { text };
    if !text.is_empty() {
        super::native::speak(&text);
    }
}

/// Text > Show Color Scopes: the syntax scope stack and the theme items
/// matching at the caret, as CodeRunner's HUD lists them.
pub fn show_color_scopes() {
    let Some(id) = state::with(|workspace| workspace.active) else { return };
    let Some(api) = state::editor_api(id) else { return };
    let Some(head) = api.selections().last().map(|s| s.head) else { return };
    let Some(info) = super::theme::scopes_at(id, head) else {
        state::notice("No color scopes at the caret.");
        return;
    };
    let stack = info.stack.join("\n");
    let matched = info
        .matched
        .iter()
        .take(6)
        .map(|item| {
            format!(
                "{}  ·  {}",
                item.name.clone().unwrap_or_else(|| "(unnamed)".into()),
                item.scope
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    spawn(async move {
        let _ = nsh_base::alert_dialog()
            .title("Color Scopes")
            .description(format!("Scope stack:\n{stack}\n\nTheme matches:\n{matched}"))
            .action("ok", "OK")
            .show()
            .await;
    });
}

/// An editor display option of the Editor Options menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Display {
    Invisibles,
    LineNumbers,
    WrapLines,
    CurrentLine,
    PageGuide,
    ScrollPastEnd,
}

impl Display {
    fn title(self) -> &'static str {
        match self {
            Display::Invisibles => "Show Invisibles",
            Display::LineNumbers => "Show Line Numbers",
            Display::WrapLines => "Wrap Lines",
            Display::CurrentLine => "Highlight Current Line",
            Display::PageGuide => "Show Page Guide",
            Display::ScrollPastEnd => "Allow Scroll Past End",
        }
    }

    fn get(self, prefs: &Preferences) -> bool {
        let appearance = &prefs.text_editing.appearance;
        match self {
            Display::Invisibles => appearance.invisibles.show,
            Display::LineNumbers => appearance.line_numbers.enabled,
            Display::WrapLines => appearance.wrap_lines,
            Display::CurrentLine => appearance.highlight_current_line,
            Display::PageGuide => appearance.page_guide.show,
            Display::ScrollPastEnd => appearance.allow_scroll_past_end,
        }
    }

    fn set(self, prefs: &mut Preferences, on: bool) {
        let appearance = &mut prefs.text_editing.appearance;
        match self {
            Display::Invisibles => appearance.invisibles.show = on,
            Display::LineNumbers => appearance.line_numbers.enabled = on,
            Display::WrapLines => appearance.wrap_lines = on,
            Display::CurrentLine => appearance.highlight_current_line = on,
            Display::PageGuide => appearance.page_guide.show = on,
            Display::ScrollPastEnd => appearance.allow_scroll_past_end = on,
        }
    }
}

/// Flips a display option and saves the preferences.
fn toggle(which: Display) {
    preferences::settings().update(|prefs| {
        let on = which.get(prefs);
        which.set(prefs, !on);
    });
    preferences::save();
}

// ---------------------------------------------------------------- Pure logic

/// The font size after a step, within 6 to 72 points.
fn zoomed(size: f64, step: f64) -> f64 { (size.round() + step).clamp(6.0, 72.0) }

/// The edits that comment `lines` (number, text) out with `marker`, or back
/// in when every non-blank one already is. Commenting puts `marker` and a
/// space at the shallowest indentation of the non-blank lines; uncommenting
/// removes the marker and one space after it. Blank lines are left alone.
/// Columns are UTF-16 units, as the editor counts them.
fn comment_edits(lines: &[(u32, &str)], marker: &str) -> Vec<EditorEdit> {
    let utf16 = |s: &str| s.encode_utf16().count() as u32;
    let indent = |s: &str| s.len() - s.trim_start_matches([' ', '\t']).len();
    let code: Vec<&(u32, &str)> = lines.iter().filter(|(_, text)| !text.trim().is_empty()).collect();
    if code.is_empty() { return Vec::new(); }
    let commented = code.iter().all(|(_, text)| text.trim_start_matches([' ', '\t']).starts_with(marker));
    if commented {
        code.iter().map(|&&(line, text)| {
            let at = indent(text);
            let after = &text[at + marker.len()..];
            let removed = marker.len() + usize::from(after.starts_with(' '));
            let start = EditorPosition::new(line, utf16(&text[..at]));
            let end = EditorPosition::new(line, utf16(&text[..at + removed]));
            EditorEdit::new(EditorRange::new(start, end), "")
        }).collect()
    } else {
        let column = code.iter().map(|(_, text)| utf16(&text[..indent(text)])).min().unwrap_or(0);
        code.iter().map(|&&(line, _)| {
            let at = EditorPosition::new(line, column);
            EditorEdit::new(EditorRange::new(at, at), format!("{marker} "))
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies single-line edits to `text`, as the editor would.
    fn apply(text: &str, edits: &[EditorEdit]) -> String {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.column)));
        for edit in edits {
            assert_eq!(edit.range.start.line, edit.range.end.line);
            let line = &mut lines[edit.range.start.line as usize];
            let units: Vec<u16> = line.encode_utf16().collect();
            let (start, end) = (edit.range.start.column as usize, edit.range.end.column as usize);
            let mut out = String::from_utf16(&units[..start]).unwrap();
            out.push_str(&edit.text);
            out.push_str(&String::from_utf16(&units[end..]).unwrap());
            *line = out;
        }
        lines.join("\n")
    }

    fn toggle_all(text: &str, marker: &str) -> String {
        let lines: Vec<(u32, &str)> = text.split('\n').enumerate().map(|(i, l)| (i as u32, l)).collect();
        apply(text, &comment_edits(&lines, marker))
    }

    #[test]
    fn comments_at_the_shallowest_indentation() {
        let code = "def f():\n    x = 1\n\n    return x";
        let commented = toggle_all(code, "#");
        assert_eq!(commented, "# def f():\n#     x = 1\n\n#     return x");
        assert_eq!(toggle_all(&commented, "#"), code);
        assert_eq!(toggle_all("    a\n      b", "//"), "    // a\n    //   b");
    }

    #[test]
    fn mixed_lines_comment_rather_than_uncomment() {
        assert_eq!(toggle_all("// a\nb", "//"), "// // a\n// b");
    }

    #[test]
    fn uncomments_without_a_space_too() {
        assert_eq!(toggle_all("\t--a\n\t-- b", "--"), "\ta\n\tb");
    }

    #[test]
    fn blank_lines_and_wide_characters() {
        assert!(comment_edits(&[(0, "   "), (1, "")], "#").is_empty());
        // Columns count UTF-16 units: the emoji line still uncomments.
        assert_eq!(toggle_all("# 😀 = 1", "#"), "😀 = 1");
        assert_eq!(toggle_all("  é\n  😀", "#"), "  # é\n  # 😀");
    }

    #[test]
    fn auto_indent_tracks_bracket_depth() {
        // Ragged input — every line touched, 4-space unit.
        let lines = ["fn f() {", "x = 1;", "if y {", "z = 2;", "}", "}"];
        let touched: BTreeSet<u32> = (0..lines.len() as u32).collect();
        let edits = auto_indent_edits(&lines, &touched, "    ");
        // Apply and compare.
        let indented = apply(&lines.join("\n"), &edits);
        assert_eq!(
            indented,
            "fn f() {\n    x = 1;\n    if y {\n        z = 2;\n    }\n}"
        );
        // Untouched lines are left as they are.
        let touched: BTreeSet<u32> = [2, 3].into_iter().collect();
        let edits = auto_indent_edits(&lines, &touched, "    ");
        let indented = apply(&lines.join("\n"), &edits);
        assert_eq!(
            indented,
            "fn f() {\nx = 1;\n    if y {\n        z = 2;\n}\n}"
        );
        // Closing bracket outdents before its own line's depth counts.
        let lines = ["a {", "b", "} c {", "d", "}"];
        let touched: BTreeSet<u32> = (0..5).collect();
        let indented = apply(&lines.join("\n"), &auto_indent_edits(&lines, &touched, "  "));
        assert_eq!(indented, "a {\n  b\n} c {\n  d\n}");
    }

    #[test]
    fn delimiter_scanners_find_the_innermost_pair() {
        let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        // f(g(x)) — caret inside the inner pair.
        let code = units("f(g(x))");
        assert_eq!(open_delimiter(&code, 4), Some(3)); // 'g(' opener
        assert_eq!(close_delimiter(&code, 4), Some(5)); // inner ')'
        // Caret inside the second pair.
        let code = units("a(b)c(d)e");
        assert_eq!(open_delimiter(&code, 6), Some(5));
        assert_eq!(close_delimiter(&code, 6), Some(7));
        // Nested depth is skipped, not matched.
        let code = units("[{x}]");
        assert_eq!(open_delimiter(&code, 2), Some(1));
        assert_eq!(close_delimiter(&code, 2), Some(3));
        // No pair → None.
        let code = units("abc");
        assert_eq!(open_delimiter(&code, 1), None);
        assert_eq!(close_delimiter(&code, 1), None);
        // Mixed bracket kinds pair by match order, not kind.
        let code = units("(a[b]c)");
        assert_eq!(open_delimiter(&code, 4), Some(2));
        assert_eq!(close_delimiter(&code, 4), Some(4));
    }

    #[test]
    fn zoom_stays_in_range() {
        assert_eq!(zoomed(12.0, 1.0), 13.0);
        assert_eq!(zoomed(12.4, -1.0), 11.0);
        assert_eq!(zoomed(72.0, 1.0), 72.0);
        assert_eq!(zoomed(6.0, -1.0), 6.0);
    }

    #[test]
    fn every_key_is_a_bound_command() {
        for &(id, _, _) in KEYS {
            assert!(bindings::command(id).is_some(), "{id} is not in bindings::COMMANDS");
        }
    }
}
