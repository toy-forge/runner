//! Compound commands: the editor scripts of a `.crBindings` set, run against
//! the active editor as CodeRunner runs them.
//!
//! The pinned nsh has no raw key-event surface (`capture_keys` and
//! `KeyEvent` came after it), so the scripts cannot run against the editor:
//! the pane still resolves the sets for its issues list, and the
//! `makeEditable` locks — only a running script could set them — still feed
//! the editor's `read_only` prop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use nsh_core::prelude::*;
use scratch_commands::bindings::KeyMap;
use scratch_commands::menu::MenuModel;
use scratch_document::DocumentId;

use super::{bindings, preferences};

/// What `KeyMap` reported resolving the sets (invalid keys, unknown paths,
/// conflicts), for the pane's bottom bar.
#[derive(Clone, Default, PartialEq)]
pub struct KeyMapIssues(pub Arc<Vec<String>>);

/// `makeEditable`/`makeNotEditable` locks (the Vim set's normal mode), by
/// document. Read by the editor's `read_only` prop.
#[derive(Clone, Copy, Default, PartialEq)]
struct LocksRevision(u64);

thread_local! {
    static LOCKS: RefCell<HashMap<DocumentId, bool>> = RefCell::new(HashMap::new());
}

/// Whether the document's editor refuses typing (Vim's normal mode).
pub fn locked(id: DocumentId) -> bool {
    global::<LocksRevision>().get();
    LOCKS.with(|locks| locks.borrow().get(&id).copied().unwrap_or(false))
}

// --------------------------------------------------------------- resolution

/// Re-resolves the compound bindings of the active and global sets for the
/// pane's issues list. Called from the revision effect in `install_keys`.
pub fn refresh() {
    let enabled = preferences::settings().with_untracked(|p| p.key_bindings.compound_commands_enabled);
    let active = bindings::active_set();
    let global_set = enabled.then(bindings::global_set);
    let map = KeyMap::build(&MenuModel::standard(), Some(&active), global_set.as_ref());
    let issues: Vec<String> = map
        .issues()
        .iter()
        .map(ToString::to_string)
        .chain(map.conflicts().iter().map(|c| format!("{} conflicts with {}", c.first, c.second)))
        .collect();
    global::<KeyMapIssues>().set(KeyMapIssues(Arc::new(issues)));
}
