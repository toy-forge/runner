//! The resumable interpreter: a stack machine over command lists.
//!
//! A [`Machine`] runs one compound command. It never blocks: when the command waits for a
//! keystroke (an object inside a list, `copyTypedCharacterWithName:`, `findTypedCharacter...`)
//! [`Machine::start`] returns [`Status::WaitingForKey`] and the caller feeds the next key with
//! [`Machine::feed_key`].
//!
//! Semantics (from the CodeRunner help book):
//!
//! * A failing test aborts its command list and restores the document (`Host::snapshot`).
//!   The *parent* list continues: a nested list is an "optional step"; only `and:`, `or:`,
//!   `negate:` and `includeCompoundCommand:` pass failure up.
//! * `break: N` exits N enclosing lists successfully, `fail: N` exits N lists as failed
//!   (restoring each), `exit` stops everything and consumes the key. Lists entered by
//!   `includeCompoundCommand:` do not count as a level.
//! * `callCompoundCommand:` runs another command as a list nested at the call site (its result is
//!   ignored); `callEnclosingCommandList: N` re-runs the Nth enclosing list (recursion).
//! * Top level: success (or `break`) consumes the key; failure restores the document and lets the
//!   key through to the next binding.

use std::collections::HashMap;
use std::sync::Arc;

use super::ast::{Arg, Call, CommandList, CompoundCommand, Item, KeyTable, Root};
use super::builtins::{self, Builtin};
use super::host::{CommandOutcome, Host, KeyInput};
use super::registers::Registers;
use super::CompoundError;
use crate::menu::wildcard_match;
use crate::shortcut::Stroke;

const MAX_STEPS: usize = 2_000_000;
const MAX_DEPTH: usize = 4_000;

/// Named command lists available to `callCompoundCommand:` / `includeCompoundCommand:`.
#[derive(Debug, Clone, Default)]
pub struct CommandLibrary {
    lists: HashMap<String, Arc<CommandList>>,
}

impl CommandLibrary {
    /// An empty library.
    pub fn new() -> CommandLibrary {
        CommandLibrary::default()
    }

    /// Registers `list` under `name` (replacing an existing entry).
    pub fn insert(&mut self, name: impl Into<String>, list: Arc<CommandList>) {
        self.lists.insert(name.into(), list);
    }

    /// Registers a compound command: a list root under `title`, every entry of a key-table root
    /// under its key (an existing entry of the same name wins).
    pub fn add_command(&mut self, title: &str, cmd: &CompoundCommand) {
        match &cmd.root {
            Root::List(l) => {
                self.lists.insert(title.to_owned(), l.clone());
            }
            Root::Entries(t) => {
                for e in &t.entries {
                    self.lists.entry(e.key.clone()).or_insert_with(|| e.list.clone());
                }
            }
        }
    }

    /// Looks a command list up by exact name.
    pub fn get(&self, name: &str) -> Option<Arc<CommandList>> {
        self.lists.get(name).cloned()
    }

    /// Number of named lists.
    pub fn len(&self) -> usize {
        self.lists.len()
    }

    /// Whether the library has no lists.
    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }
}

/// Where a run stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// The command succeeded: the triggering key is consumed.
    Consumed,
    /// A test failed at the top level (document restored): offer the key to the next binding.
    PassThrough,
    /// The command waits for the next key: call [`Machine::feed_key`].
    WaitingForKey,
    /// The command is invalid or hit a limit; the document was restored.
    Error(CompoundError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Plain,
    And,
    Or,
    Negate,
    Include,
    Entry,
}

struct Frame<S> {
    list: Arc<CommandList>,
    pc: usize,
    snapshot: Option<S>,
    role: Role,
}

enum Flow {
    Ok,
    Fail,
    Break(u32),
    FailUp(u32),
    Exit,
    NeedKey,
    Push(Arc<CommandList>, Role),
}

/// Runs compound commands against a [`Host`].
///
/// ```
/// use std::sync::Arc;
/// use scratch_commands::compound::{CommandLibrary, CompoundCommand, Machine, Registers, Status, fake::FakeHost};
///
/// let cmd = CompoundCommand::parse(r#"["testSelectedText:", "", "insertText:", "hi"]"#).unwrap();
/// let mut host = FakeHost::new("abc");
/// let mut regs = Registers::new();
/// let mut machine = Machine::new(Arc::new(CommandLibrary::new()));
/// let status = machine.start_command(&cmd, None, &mut host, &mut regs);
/// assert_eq!(status, Status::Consumed);
/// assert_eq!(host.text_string(), "hiabc");
/// ```
pub struct Machine<H: Host> {
    library: Arc<CommandLibrary>,
    stack: Vec<Frame<H::Snapshot>>,
    pending_key: Option<KeyInput>,
    outcome: Option<bool>,
    waiting: bool,
    steps: usize,
    keys_fed: usize,
}

impl<H: Host> Machine<H> {
    /// A machine resolving `callCompoundCommand:` names in `library`.
    pub fn new(library: Arc<CommandLibrary>) -> Machine<H> {
        Machine { library, stack: Vec::new(), pending_key: None, outcome: None, waiting: false, steps: 0, keys_fed: 0 }
    }

    /// Whether the machine waits for a key.
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    fn reset(&mut self) {
        self.stack.clear();
        self.pending_key = None;
        self.outcome = None;
        self.waiting = false;
        self.steps = 0;
        self.keys_fed = 0;
    }

    /// Starts `list`.
    pub fn start(&mut self, list: Arc<CommandList>, host: &mut H, regs: &mut Registers) -> Status {
        self.reset();
        self.push(list, Role::Plain, host);
        self.run(host, regs)
    }

    /// Starts a parsed compound command: its root list, or the entry of a key table named `entry`.
    /// An unknown entry (or a list root with an entry name) is a pass-through.
    pub fn start_command(&mut self, cmd: &CompoundCommand, entry: Option<&str>, host: &mut H, regs: &mut Registers) -> Status {
        match cmd.entry(entry) {
            Some(list) => self.start(list, host, regs),
            None => Status::PassThrough,
        }
    }

    /// Delivers the awaited key.
    pub fn feed_key(&mut self, key: KeyInput, host: &mut H, regs: &mut Registers) -> Status {
        if !self.waiting {
            return Status::PassThrough;
        }
        self.waiting = false;
        self.pending_key = Some(key);
        self.keys_fed += 1;
        self.run(host, regs)
    }

    /// Abandons a waiting run, restoring the document to its state at [`Machine::start`].
    pub fn cancel(&mut self, host: &mut H) {
        if let Some(bottom) = self.stack.drain(..).next()
            && let Some(s) = bottom.snapshot
        {
            host.restore(s);
        }
        self.waiting = false;
        self.pending_key = None;
    }

    fn push(&mut self, list: Arc<CommandList>, role: Role, host: &mut H) {
        let snapshot = Some(host.snapshot());
        self.stack.push(Frame { list, pc: 0, snapshot, role });
    }

    fn finish(&mut self) -> Status {
        let consumed = self.outcome.unwrap_or(true) || self.keys_fed > 0;
        self.outcome = None;
        if consumed { Status::Consumed } else { Status::PassThrough }
    }

    fn abort(&mut self, host: &mut H, e: CompoundError) -> Status {
        self.cancel(host);
        Status::Error(e)
    }

    fn run(&mut self, host: &mut H, regs: &mut Registers) -> Status {
        loop {
            self.steps += 1;
            if self.steps > MAX_STEPS || self.stack.len() > MAX_DEPTH {
                return self.abort(host, CompoundError::LimitExceeded);
            }
            let Some(frame) = self.stack.last_mut() else { return self.finish() };
            if frame.pc >= frame.list.items.len() {
                let ok = frame.role != Role::Or;
                self.end_frame(host, ok);
                continue;
            }
            let list = frame.list.clone();
            let idx = frame.pc;
            frame.pc += 1;
            match &list.items[idx] {
                Item::List(l) => self.push(l.clone(), Role::Plain, host),
                Item::Wait(table) => match self.pending_key.take() {
                    Some(k) => match match_key(table, k) {
                        Some(l) => self.push(l, Role::Entry, host),
                        None => self.item_failed(host),
                    },
                    None => {
                        self.back_up();
                        self.waiting = true;
                        return Status::WaitingForKey;
                    }
                },
                Item::Call(call) => {
                    let key = self.pending_key.take();
                    match self.call(call, key, host, regs) {
                        Err(e) => return self.abort(host, e),
                        Ok(Flow::Ok) => self.item_ok(host),
                        Ok(Flow::Fail) => self.item_failed(host),
                        Ok(Flow::Break(n)) => self.unwind(host, true, n),
                        Ok(Flow::FailUp(n)) => self.unwind(host, false, n),
                        Ok(Flow::Exit) => {
                            self.stack.clear();
                            self.outcome = Some(true);
                        }
                        Ok(Flow::NeedKey) => {
                            self.back_up();
                            self.waiting = true;
                            return Status::WaitingForKey;
                        }
                        Ok(Flow::Push(l, role)) => self.push(l, role, host),
                    }
                }
            }
        }
    }

    /// Re-executes the item that asked for a key when the key arrives.
    fn back_up(&mut self) {
        if let Some(f) = self.stack.last_mut() {
            f.pc -= 1;
        }
    }

    fn item_ok(&mut self, host: &mut H) {
        if self.stack.last().is_some_and(|f| f.role == Role::Or) {
            self.end_frame(host, true);
        }
    }

    fn item_failed(&mut self, host: &mut H) {
        if self.stack.last().is_some_and(|f| f.role == Role::Or) {
            return;
        }
        self.end_frame(host, false);
    }

    /// Pops the top frame with result `ok`, restoring the document if it failed.
    fn end_frame(&mut self, host: &mut H, ok: bool) {
        let Some(frame) = self.stack.pop() else { return };
        if !ok && let Some(s) = frame.snapshot {
            host.restore(s);
        }
        self.child_done(frame.role, ok, host);
    }

    /// A frame of `role` finished with `ok`; tell its parent.
    fn child_done(&mut self, role: Role, ok: bool, host: &mut H) {
        let effective = if role == Role::Negate { !ok } else { ok };
        let Some(parent) = self.stack.last() else {
            self.outcome = Some(effective);
            return;
        };
        if parent.role == Role::Or {
            if effective {
                self.end_frame(host, true);
            }
            return;
        }
        if role != Role::Plain && !effective {
            self.item_failed(host);
        }
    }

    /// Exits `levels` enclosing lists (includes do not count) as a break (`ok`) or a failure.
    fn unwind(&mut self, host: &mut H, ok: bool, levels: u32) {
        let mut remaining = levels.max(1);
        while let Some(frame) = self.stack.pop() {
            if !ok && let Some(s) = frame.snapshot {
                host.restore(s);
            }
            if frame.role == Role::Include {
                continue;
            }
            remaining -= 1;
            if remaining == 0 {
                self.child_done(frame.role, ok, host);
                return;
            }
        }
        self.outcome = Some(ok);
    }

    fn call(&mut self, call: &Call, key: Option<KeyInput>, host: &mut H, regs: &mut Registers) -> Result<Flow, CompoundError> {
        let name = call.name.as_str();
        let bad = |i: usize, expected: &'static str| CompoundError::BadArgument { command: name.to_owned(), index: i, expected };
        match name {
            "break:" | "fail:" => {
                let n = call.args.first().and_then(Arg::as_number).ok_or_else(|| bad(1, "a number"))?;
                let n = if n >= 999.0 { u32::MAX } else { n.max(1.0) as u32 };
                Ok(if name == "break:" { Flow::Break(n) } else { Flow::FailUp(n) })
            }
            "exit" => Ok(Flow::Exit),
            "callCompoundCommand:" | "includeCompoundCommand:" => {
                let target = call.args.first().and_then(Arg::as_text).ok_or_else(|| bad(1, "a string"))?;
                let list = self.library.get(target).ok_or_else(|| CompoundError::UnknownCompound(target.to_owned()))?;
                Ok(Flow::Push(list, if name == "callCompoundCommand:" { Role::Plain } else { Role::Include }))
            }
            "callEnclosingCommandList:" => {
                let n = call.args.first().and_then(Arg::as_number).ok_or_else(|| bad(1, "a number"))?.max(1.0) as usize;
                let target = self.stack.iter().rev().filter(|f| f.role != Role::Include).nth(n - 1).map(|f| f.list.clone());
                Ok(match target {
                    Some(l) => Flow::Push(l, Role::Plain),
                    None => Flow::Fail,
                })
            }
            "and:" | "or:" | "negate:" => {
                let Some(Arg::Commands(l)) = call.args.first() else { return Err(bad(1, "a command list")) };
                let role = match name {
                    "and:" => Role::And,
                    "or:" => Role::Or,
                    _ => Role::Negate,
                };
                Ok(Flow::Push(l.clone(), role))
            }
            "selectMenuItem:" => {
                let pattern = call.args.first().and_then(Arg::as_text).ok_or_else(|| bad(1, "a string"))?;
                Ok(if select_menu_item(host, pattern) { Flow::Ok } else { Flow::Fail })
            }
            _ => match builtins::run(call, host, regs, key)? {
                Builtin::Done => Ok(Flow::Ok),
                Builtin::Fail => Ok(Flow::Fail),
                Builtin::NeedKey => Ok(Flow::NeedKey),
                Builtin::Other => match host.execute(name, &call.args) {
                    CommandOutcome::Done => Ok(Flow::Ok),
                    CommandOutcome::Failed => Ok(Flow::Fail),
                    CommandOutcome::Unknown => Err(CompoundError::UnknownCommand(name.to_owned())),
                },
            },
        }
    }
}

fn match_key(table: &KeyTable, key: KeyInput) -> Option<Arc<CommandList>> {
    let mut any = None;
    for e in &table.entries {
        if e.key == "any" {
            any = Some(e.list.clone());
            continue;
        }
        if let Ok(stroke) = Stroke::parse_crbindings(&e.key)
            && stroke == key.stroke
        {
            return Some(e.list.clone());
        }
    }
    any
}

fn select_menu_item<H: Host>(host: &mut H, pattern: &str) -> bool {
    let pat = pattern.strip_prefix("MainMenu/").unwrap_or(pattern);
    let segs: Vec<&str> = pat.split('/').collect();
    let items = host.menu_items();
    let found = items.iter().find(|i| {
        let path: Vec<&str> = i.path.split('/').collect();
        if segs.len() == 1 {
            path.last().is_some_and(|l| wildcard_match(segs[0], l))
        } else {
            path.len() == segs.len() && segs.iter().zip(&path).all(|(p, s)| wildcard_match(p, s))
        }
    });
    match found {
        Some(i) if i.enabled => {
            let path = i.path.clone();
            host.perform_menu_item(&path)
        }
        _ => false,
    }
}

// Silence an unused import when `Item` patterns change.
#[allow(unused)]
fn _assert_send(_: &KeyTable) {}
