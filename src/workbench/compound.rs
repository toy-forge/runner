//! Compound commands: executes the editor scripts of a `.crBindings` set
//! against the active document via `scratch_commands::compound::Machine`, and
//! feeds the `makeEditable` / `makeNotEditable` read-only locks to the editor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use nsh_base::{EditorApi, EditorCommand, EditorPosition, EditorSelection};
use nsh_core::keymap::When;
use nsh_core::prelude::*;
use scratch_commands::bindings::{KeyMap, Target};
use scratch_commands::compound::{
    Arg, CommandLibrary, CommandList, CommandOutcome, Edge, Host, HostMenuItem, KeyInput, Machine,
    Registers, Selection, Status, fake::FakeHost,
};
use scratch_commands::menu::MenuModel;
use scratch_commands::shortcut::{Key, Stroke};
use scratch_document::DocumentId;

use super::{bindings, preferences, state};

/// What `KeyMap` reported resolving the sets (invalid keys, unknown paths,
/// conflicts), for the pane's bottom bar.
#[derive(Clone, Default, PartialEq)]
pub struct KeyMapIssues(pub Arc<Vec<String>>);

/// `makeEditable`/`makeNotEditable` locks (the Vim set's normal mode), by
/// document. Read by the editor's `read_only` prop.
#[derive(Clone, Copy, Default, PartialEq)]
struct LocksRevision(u64);

/// Bumped when a multi-stroke compound machine starts or finishes waiting for a follow-up key.
#[derive(Clone, Copy, Default, PartialEq)]
struct PendingRevision(u64);

thread_local! {
    static LOCKS: RefCell<HashMap<DocumentId, bool>> = RefCell::new(HashMap::new());
    static REGISTERS: RefCell<Registers> = RefCell::new(Registers::new());
    static PENDING: RefCell<Option<(DocumentId, Machine<EditorHost>)>> = RefCell::new(None);
}

fn set_pending(next: Option<(DocumentId, Machine<EditorHost>)>) {
    let next_doc = next.as_ref().map(|(id, _)| *id);
    let prev_doc = PENDING.with_borrow_mut(|slot| {
        let prev = slot.as_ref().map(|(id, _)| *id);
        *slot = next;
        prev
    });
    if prev_doc != next_doc {
        global::<PendingRevision>().update(|r| r.0 += 1);
    }
}

fn clear_locks_and_pending() {
    let had_locks = LOCKS.with(|locks| {
        let mut map = locks.borrow_mut();
        let any = map.values().any(|&v| v);
        map.clear();
        any
    });
    let had_pending = PENDING.with_borrow_mut(|slot| slot.take().is_some());
    if had_locks {
        global::<LocksRevision>().update(|r| r.0 += 1);
    }
    if had_pending {
        global::<PendingRevision>().update(|r| r.0 += 1);
    }
}

/// Whether the document's editor refuses typing (Vim's normal mode).
pub fn locked(id: DocumentId) -> bool {
    global::<LocksRevision>().get();
    LOCKS.with(|locks| locks.borrow().get(&id).copied().unwrap_or(false))
}

fn set_locked(id: DocumentId, read_only: bool) {
    let changed = LOCKS.with(|locks| {
        let mut map = locks.borrow_mut();
        let prev = map.get(&id).copied().unwrap_or(false);
        if prev == read_only {
            false
        } else {
            map.insert(id, read_only);
            true
        }
    });
    if changed {
        global::<LocksRevision>().update(|r| r.0 += 1);
    }
}

/// Converts a UTF-8 byte offset in `text` to an `EditorPosition` (`line`, UTF-16 `column`).
fn offset_to_position(text: &str, offset: usize) -> EditorPosition {
    let clamped = offset.min(text.len());
    let prefix = &text[..clamped];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |idx| idx + 1);
    let col = prefix[line_start..].encode_utf16().count() as u32;
    EditorPosition::new(line, col)
}

/// Converts an `EditorPosition` (`line`, UTF-16 `column`) in `text` to a UTF-8 byte offset.
fn position_to_offset(text: &str, pos: EditorPosition) -> usize {
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (idx, ch) in text.char_indices() {
        if line == pos.line {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    if line < pos.line {
        return text.len();
    }
    let slice = &text[line_start..];
    let mut utf16 = 0u32;
    let mut bytes = 0usize;
    for ch in slice.chars() {
        if ch == '\n' {
            break;
        }
        let next_u16 = utf16 + ch.len_utf16() as u32;
        if next_u16 > pos.column {
            break;
        }
        utf16 = next_u16;
        bytes += ch.len_utf8();
    }
    line_start + bytes
}

struct EditorHost {
    inner: FakeHost,
    pending_menu_actions: Vec<String>,
}

impl EditorHost {
    fn new(text: &str, selection: Selection, read_only: bool) -> Self {
        let mut inner = FakeHost::new(text);
        inner.set_selection(selection);
        inner.set_editable(!read_only);
        REGISTERS.with_borrow(|regs| {
            if let Some(clip) = regs.texts.get("clipboard") {
                inner.set_clipboard(clip);
            }
        });
        Self {
            inner,
            pending_menu_actions: Vec::new(),
        }
    }
}

impl Host for EditorHost {
    type Snapshot = (<FakeHost as Host>::Snapshot, usize);

    fn snapshot(&mut self) -> Self::Snapshot {
        (self.inner.snapshot(), self.pending_menu_actions.len())
    }

    fn restore(&mut self, (snap, actions_len): Self::Snapshot) {
        self.inner.restore(snap);
        self.pending_menu_actions.truncate(actions_len);
    }

    fn text_len(&self) -> usize {
        self.inner.text_len()
    }

    fn text(&self, range: Range<usize>) -> String {
        self.inner.text(range)
    }

    fn selection(&self) -> Selection {
        self.inner.selection()
    }

    fn set_selection(&mut self, selection: Selection) {
        self.inner.set_selection(selection);
    }

    fn insert_text(&mut self, text: &str) {
        self.inner.insert_text(text);
    }

    fn revision(&self) -> u64 {
        self.inner.revision()
    }

    fn selection_count(&self) -> usize {
        self.inner.selection_count()
    }

    fn is_editable(&self) -> bool {
        self.inner.is_editable()
    }

    fn set_editable(&mut self, editable: bool) {
        self.inner.set_editable(editable);
    }

    fn clipboard(&self) -> String {
        self.inner.clipboard()
    }

    fn set_clipboard(&mut self, text: &str) {
        self.inner.set_clipboard(text);
    }

    fn menu_items(&self) -> Vec<HostMenuItem> {
        bindings::COMMANDS
            .iter()
            .map(|b| HostMenuItem {
                path: b.path.strip_prefix("MainMenu/").unwrap_or(b.path).to_owned(),
                enabled: true,
            })
            .collect()
    }

    fn perform_menu_item(&mut self, path: &str) -> bool {
        let full = if path.starts_with("MainMenu/") {
            path.to_owned()
        } else {
            format!("MainMenu/{path}")
        };
        if bindings::COMMANDS.iter().any(|b| b.path == full) {
            self.pending_menu_actions.push(full);
            true
        } else {
            false
        }
    }

    fn execute(&mut self, name: &str, args: &[Arg]) -> CommandOutcome {
        let prev_executed = self.inner.executed.len();
        let outcome = self.inner.execute(name, args);
        if self.inner.executed.len() == prev_executed {
            return outcome;
        }
        self.inner.executed.truncate(prev_executed);
        match name {
            "undo" | "redo" => {
                self.pending_menu_actions.push(name.to_owned());
                return CommandOutcome::Done;
            }
            "indent" => {
                self.pending_menu_actions.push("MainMenu/Text/Shift Right".to_owned());
                return CommandOutcome::Done;
            }
            "outdent" => {
                self.pending_menu_actions.push("MainMenu/Text/Shift Left".to_owned());
                return CommandOutcome::Done;
            }
            "deleteLine" => {
                self.pending_menu_actions.push("MainMenu/Text/Delete Line".to_owned());
                return CommandOutcome::Done;
            }
            _ => {}
        }
        if bindings::COMMANDS.iter().any(|b| b.path == name || b.names.contains(&name)) {
            self.pending_menu_actions.push(name.to_owned());
            return CommandOutcome::Done;
        }
        CommandOutcome::Unknown
    }

    fn debug(&mut self, message: &str) {
        state::notice(format!("Compound: {message}"));
    }
}

fn editor_selection_from_host(text: &str, sel: Selection) -> EditorSelection {
    let start = offset_to_position(text, sel.start);
    let end = offset_to_position(text, sel.end);
    match sel.edge {
        Edge::Start => EditorSelection::new(end, start),
        Edge::End => EditorSelection::new(start, end),
    }
}

fn host_selection_from_editor(api: &EditorApi, text: &str) -> Selection {
    let Some(primary) = api.selections().last().cloned() else {
        return Selection::caret(0);
    };
    let anchor = position_to_offset(text, primary.anchor).min(text.len());
    let head = position_to_offset(text, primary.head).min(text.len());
    let mut sel = Selection::new(anchor.min(head), anchor.max(head));
    sel.edge = if head < anchor { Edge::Start } else { Edge::End };
    sel
}

fn commit_host_run(
    id: DocumentId,
    api: &EditorApi,
    original_text: &str,
    initial_read_only: bool,
    initial_clipboard: &str,
    host: EditorHost,
) {
    let next_clipboard = host.clipboard();
    if next_clipboard != initial_clipboard {
        REGISTERS.with_borrow_mut(|regs| {
            regs.texts.insert("clipboard".to_owned(), next_clipboard.clone());
        });
        set_clipboard(next_clipboard);
    }
    let next_text = host.inner.text_string().to_owned();
    let next_sel = host.inner.selection();
    let next_read_only = !host.is_editable();
    if next_text != original_text {
        api.set_value(next_text.clone());
        state::edited(id, &next_text);
    }
    let ed_sel = editor_selection_from_host(&next_text, next_sel);
    api.set_selections(vec![ed_sel]);
    if next_read_only != initial_read_only {
        set_locked(id, next_read_only);
    }
    for action in host.pending_menu_actions {
        if action == "undo" {
            api.run(EditorCommand::Undo);
        } else if action == "redo" {
            api.run(EditorCommand::Redo);
        } else {
            let _ = bindings::dispatch_command(&action);
        }
    }
}

/// Runs candidate compound commands in precedence order until one consumes or fails.
fn execute_candidates(candidates: &[(String, Arc<CommandList>)], library: &Arc<CommandLibrary>) {
    let Some(id) = state::with(|workspace| workspace.active) else { return; };
    let Some(api) = state::editor_api(id) else { return; };
    for (title, list) in candidates {
        let original_text = api.value();
        let initial_sel = host_selection_from_editor(&api, &original_text);
        let initial_read_only = LOCKS.with(|locks| locks.borrow().get(&id).copied().unwrap_or(false));
        let mut host = EditorHost::new(&original_text, initial_sel, initial_read_only);
        let initial_clipboard = host.clipboard();
        let mut machine = Machine::new(library.clone());
        let status = REGISTERS.with_borrow_mut(|regs| machine.start(list.clone(), &mut host, regs));
        match status {
            Status::Error(err) => {
                state::notice(format!("{title}: {err}"));
                return;
            }
            Status::PassThrough => continue,
            Status::WaitingForKey => {
                commit_host_run(id, &api, &original_text, initial_read_only, &initial_clipboard, host);
                set_pending(Some((id, machine)));
                return;
            }
            Status::Consumed => {
                commit_host_run(id, &api, &original_text, initial_read_only, &initial_clipboard, host);
                return;
            }
        }
    }
}

/// Delivers `stroke` to a waiting multi-stroke compound machine, or starts matching candidates.
fn feed_stroke_or_candidates(
    stroke: Stroke,
    candidates: &[(String, Arc<CommandList>)],
    library: &Arc<CommandLibrary>,
) {
    let Some(id) = state::with(|workspace| workspace.active) else { return; };
    let (waiting, stale_other) = PENDING.with_borrow_mut(|slot| {
        if slot.as_ref().is_some_and(|(doc_id, _)| *doc_id == id) {
            (slot.take().map(|(_, m)| m), false)
        } else {
            (None, slot.take().is_some())
        }
    });
    if stale_other {
        global::<PendingRevision>().update(|r| r.0 += 1);
    }
    if let Some(mut machine) = waiting {
        let Some(api) = state::editor_api(id) else {
            global::<PendingRevision>().update(|r| r.0 += 1);
            return;
        };
        let original_text = api.value();
        let initial_sel = host_selection_from_editor(&api, &original_text);
        let initial_read_only = LOCKS.with(|locks| locks.borrow().get(&id).copied().unwrap_or(false));
        let mut host = EditorHost::new(&original_text, initial_sel, initial_read_only);
        let initial_clipboard = host.clipboard();
        let status = REGISTERS.with_borrow_mut(|regs| {
            machine.feed_key(KeyInput::from_stroke(stroke), &mut host, regs)
        });
        match status {
            Status::WaitingForKey => {
                commit_host_run(id, &api, &original_text, initial_read_only, &initial_clipboard, host);
                PENDING.with_borrow_mut(|slot| *slot = Some((id, machine)));
                return;
            }
            Status::Consumed => {
                commit_host_run(id, &api, &original_text, initial_read_only, &initial_clipboard, host);
                global::<PendingRevision>().update(|r| r.0 += 1);
                return;
            }
            Status::Error(err) => {
                global::<PendingRevision>().update(|r| r.0 += 1);
                state::notice(format!("Compound: {err}"));
                return;
            }
            Status::PassThrough => {
                global::<PendingRevision>().update(|r| r.0 += 1);
            }
        }
    }
    execute_candidates(candidates, library);
}

fn set_uses_modal_locks(set: &scratch_commands::bindings::BindingSet) -> bool {
    set.bindings.iter().any(|b| {
        b.enabled
            && b.compound_code
                .as_deref()
                .is_some_and(|code| code.contains("makeNotEditable") || code.contains("makeEditable"))
    })
}

// --------------------------------------------------------------- resolution

/// Re-resolves the compound bindings of the active and global sets for the
/// pane's issues list and binds eligible compound triggers in the editor.
/// Called from the revision effect in `install_keys`.
pub fn refresh() {
    let focused_doc = super::lsp::focused_document();
    let _ = global::<LocksRevision>().get();
    let _ = global::<PendingRevision>().get();
    let find_focused = super::find::is_field_focused();

    let enabled = preferences::settings().with(|p| p.key_bindings.compound_commands_enabled);
    let active = bindings::active_set();
    let global_set = enabled.then(bindings::global_set);
    let modal_capable =
        enabled && (set_uses_modal_locks(&active) || global_set.as_ref().is_some_and(set_uses_modal_locks));
    if !modal_capable {
        clear_locks_and_pending();
    }
    let doc_locked = focused_doc.is_some_and(|id| {
        LOCKS.with(|locks| locks.borrow().get(&id).copied().unwrap_or(false))
    });
    let pending_for_focused = focused_doc.is_some_and(|id| {
        PENDING.with_borrow(|slot| slot.as_ref().is_some_and(|(doc_id, _)| *doc_id == id))
    });
    let map = KeyMap::build(&MenuModel::standard(), Some(&active), global_set.as_ref());

    let compound_labels: std::collections::HashSet<String> = map
        .entries()
        .iter()
        .filter(|e| matches!(e.target, Target::CompoundList { .. }))
        .map(|e| e.label())
        .collect();
    let issues: Vec<String> = map
        .issues()
        .iter()
        .map(ToString::to_string)
        .chain(
            map.conflicts()
                .iter()
                .filter(|c| !compound_labels.contains(&c.first) && !compound_labels.contains(&c.second))
                .map(|c| format!("{} conflicts with {}", c.first, c.second)),
        )
        .collect();
    global::<KeyMapIssues>().set(KeyMapIssues(Arc::new(issues)));

    let library = map.library().clone();
    let mut grouped: Vec<(String, Stroke, Vec<(String, Arc<CommandList>)>)> = Vec::new();
    let mut index_by_key: HashMap<String, usize> = HashMap::new();
    for entry in map.entries() {
        let Target::CompoundList { title, list } = &entry.target else { continue; };
        if entry.shortcut.strokes().len() != 1 {
            continue;
        }
        let Ok(nsh_key) = entry.shortcut.to_nsh() else { continue; };
        let stroke = entry.shortcut.strokes()[0];
        if let Some(&idx) = index_by_key.get(&nsh_key) {
            grouped[idx].2.push((title.clone(), list.clone()));
        } else {
            index_by_key.insert(nsh_key.clone(), grouped.len());
            grouped.push((nsh_key, stroke, vec![(title.clone(), list.clone())]));
        }
    }

    for (nsh_key, stroke, candidates) in grouped {
        let modified = stroke.modifiers.ctrl
            || stroke.modifiers.cmd
            || stroke.modifiers.alt
            || matches!(stroke.key, Key::F(_));
        if modified {
            if focused_doc.is_none() {
                continue;
            }
            let library = library.clone();
            on_key(&nsh_key, move |_| execute_candidates(&candidates, &library));
        } else if doc_locked || pending_for_focused {
            let library = library.clone();
            on_key_with(
                &nsh_key,
                KeyOptions { when: When::Always, ..KeyOptions::default() },
                move |_| feed_stroke_or_candidates(stroke, &candidates, &library),
            );
        } else if stroke.key == Key::Escape && stroke.modifiers.is_empty() && !find_focused && focused_doc.is_some() {
            let library = library.clone();
            on_key_with(
                "escape",
                KeyOptions { when: When::Always, ..KeyOptions::default() },
                move |_| execute_candidates(&candidates, &library),
            );
        }
    }

    if doc_locked || pending_for_focused {
        let mut bound: std::collections::HashSet<String> = index_by_key.into_keys().collect();
        for ch in std::iter::once(' ').chain('\x21'..='\x7e') {
            let (key, shift) = Key::from_char(ch);
            let mut stroke = Stroke::plain(key);
            stroke.modifiers.shift = shift;
            if stroke.check_nsh().is_err() {
                continue;
            }
            let nsh_key = stroke.to_nsh();
            if bound.insert(nsh_key.clone()) {
                let library = library.clone();
                on_key_with(
                    &nsh_key,
                    KeyOptions { when: When::Always, ..KeyOptions::default() },
                    move |_| feed_stroke_or_candidates(stroke, &[], &library),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_position_round_trip_with_unicode() {
        let text = "ab\n😀c\nd";
        let off = offset_to_position(text, "ab\n😀".len());
        assert_eq!(off, EditorPosition::new(1, 2));
        assert_eq!(position_to_offset(text, off), "ab\n😀".len());
        let off_c = offset_to_position(text, "ab\n😀c".len());
        assert_eq!(off_c, EditorPosition::new(1, 3));
        assert_eq!(position_to_offset(text, off_c), "ab\n😀c".len());
    }

    #[test]
    fn editor_host_dispatches_workbench_commands_and_rejects_unknown() {
        let mut host = EditorHost::new("alpha beta\ngamma", Selection::caret(0), false);
        assert_eq!(host.execute("undo", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("redo", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("indent", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("outdent", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("deleteLine", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("nonexistentSelectorForTest", &[]), CommandOutcome::Unknown);
        assert!(host.inner.executed.is_empty());
        assert_eq!(
            host.pending_menu_actions,
            vec![
                "undo",
                "redo",
                "MainMenu/Text/Shift Right",
                "MainMenu/Text/Shift Left",
                "MainMenu/Text/Delete Line",
            ]
        );
    }

    #[test]
    fn editor_host_supports_vim_motion_and_paragraph_selectors() {
        let text = "one two\nthree four\nfive";
        let mut host = EditorHost::new(text, Selection::caret(2), false);
        assert_eq!(host.execute("moveWordForward", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::caret(3));
        assert_eq!(host.execute("moveWordForward", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::caret(7));
        assert_eq!(host.execute("moveWordBackward", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::caret(4));
        assert_eq!(host.execute("moveDown", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::caret(12));
        assert_eq!(host.execute("moveUp", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::caret(4));
        assert_eq!(host.execute("selectParagraph", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::new(0, 8));
        assert_eq!(host.execute("moveToBeginningOfDocument", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("moveWordForwardAndModifySelection", &[]), CommandOutcome::Done);
        assert_eq!(host.selection(), Selection::new(0, 3));
        assert_eq!(host.execute("moveParagraphDown", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("moveParagraphUp", &[]), CommandOutcome::Done);
        assert_eq!(host.execute("moveParagraphUp", &[]), CommandOutcome::Failed);
    }
}
