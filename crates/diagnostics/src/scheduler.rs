//! When diagnostics run: a pure policy object the UI drives.
//!
//! The [`Scheduler`] never spawns anything and never reads a clock: the UI
//! tells it what happened (`text_changed`, `before_run`, `job_finished`, ...)
//! together with the current [`Instant`], and it answers with a [`Plan`]:
//! jobs to cancel and jobs to start. The UI starts the jobs with
//! [`crate::DiagnosticsRunner`] under the given [`JobToken`], feeds the
//! results into an [`crate::AnnotationSet`], and reports back with
//! [`Scheduler::job_finished`].
//!
//! # Rules (CodeRunner's, from `UpdateMode` and `Options` bits)
//!
//! * `UpdateMode` bit 1 (**on edit**): the entry runs when the text has not
//!   changed for the debounce delay (default 1.5 s) and
//!   [`SchedulerConfig::while_typing`] is on. Every edit resets the timer and
//!   cancels the edit-triggered jobs still running (their output would be
//!   stale).
//! * bit 2 (**on run**): the entry runs before a run ([`Scheduler::before_run`]);
//!   the run waits ([`RunGate::Waiting`]) until they are done. Entries whose
//!   results are current for the unchanged text are not run again.
//! * bit 4 (**manual**): the entry only runs when the user asks
//!   ([`Scheduler::run_entry`], "Run Diagnostics" [`Scheduler::run_all`]).
//! * `Options` bits 4/8/16: the run is cancelled ([`RunGate::Cancel`]) when
//!   the entry produces *new* errors / warnings / notices (counted by
//!   [`crate::ChangeReport`]).
//! * At most one job per entry: starting an entry again cancels the running
//!   job of that entry.
//! * Nothing runs when a document is opened; the first lint waits for an edit
//!   or a run.
//! * Console entries (`ConsoleText`, `ConsoleHighlight`) never run as jobs
//!   on their own; [`Scheduler::after_run`] lists the ones to feed with the
//!   run's console output.
//! * Disabled entries never run.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use scratch_language::{DiagnosticDef, DiagnosticKind, DiagnosticOptions, LanguageDef, UpdateMode};

use crate::annotation::Counts;
use crate::model::Severity;
use crate::runner::JobToken;
use crate::settings::{DiagnosticsPrefs, DocumentDiagnosticsData, effective_diagnostics};

/// Scheduler settings (from [`DiagnosticsPrefs`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerConfig {
    /// Lint while typing (`DiagnosticsWhileTyping`).
    pub while_typing: bool,
    /// Debounce delay (`DiagnosticsDelay`).
    pub delay: Duration,
    /// Give up waiting for pre-run diagnostics after this long and run
    /// anyway.
    pub gate_timeout: Duration,
    /// Import console errors into the editor (`DiagnosticsFromConsole`).
    pub from_console: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            while_typing: true,
            delay: Duration::from_millis(1500),
            gate_timeout: Duration::from_secs(20),
            from_console: true,
        }
    }
}

impl From<&DiagnosticsPrefs> for SchedulerConfig {
    fn from(p: &DiagnosticsPrefs) -> Self {
        Self {
            while_typing: p.while_typing,
            delay: p.delay(),
            gate_timeout: Duration::from_secs(20),
            from_console: p.from_console,
        }
    }
}

/// One diagnostic as the scheduler sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The diagnostic's id (unique within the language).
    pub id: String,
    /// What it is.
    pub kind: DiagnosticKind,
    /// Effective update mode (after user overrides).
    pub update: UpdateMode,
    /// Options (cancel-run bits).
    pub options: DiagnosticOptions,
    /// Enabled for this document (see [`DiagnosticsPrefs::is_enabled`]).
    pub enabled: bool,
}

impl Entry {
    /// An entry from a definition.
    pub fn from_def(def: &DiagnosticDef, update: UpdateMode, enabled: bool) -> Self {
        Self {
            id: def.id.clone(),
            kind: def.kind,
            update,
            options: def.options,
            enabled,
        }
    }

    /// Whether the entry runs as a job (everything but console text).
    pub fn is_job(&self) -> bool {
        !self.is_console()
    }

    /// Whether the entry interprets console output.
    pub fn is_console(&self) -> bool {
        matches!(self.kind, DiagnosticKind::ConsoleText | DiagnosticKind::ConsoleHighlight)
    }
}

/// The entries of a language for one document: the language's diagnostics
/// plus the built-ins ([`effective_diagnostics`]), with the user's update
/// mode overrides and enablement applied. Returns the effective
/// definitions too, so the caller can build [`crate::DiagnosticJob`]s.
pub fn entries_for(
    language: &LanguageDef,
    prefs: &DiagnosticsPrefs,
    doc: &DocumentDiagnosticsData,
) -> (Vec<DiagnosticDef>, Vec<Entry>) {
    let defs = effective_diagnostics(language, prefs);
    let entries = defs
        .iter()
        .map(|d| {
            Entry::from_def(
                d,
                prefs.update_mode(&language.id, d),
                prefs.is_enabled(&language.id, &d.id, doc),
            )
        })
        .collect();
    (defs, entries)
}

/// Why a job was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The debounce timer elapsed.
    Edit,
    /// Before a run.
    Run,
    /// The user ran one entry ("Run \"Clang\" Now").
    Manual,
    /// "Check Document Now" (edit and run entries).
    CheckNow,
    /// "Run Diagnostics" (every enabled entry including manual ones).
    All,
    /// Interpreting the console output of a run.
    Console,
}

/// A job the UI should start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRequest {
    /// Token to start it under and to report back with.
    pub token: JobToken,
    /// The entry to run.
    pub entry_id: String,
    /// The text generation it runs against (put it in
    /// [`crate::DiagnosticJob::generation`]).
    pub generation: u64,
    /// Why it runs.
    pub trigger: Trigger,
}

/// What the UI must do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Jobs to cancel (their results will be discarded or arrive as
    /// cancelled).
    pub cancel: Vec<JobToken>,
    /// Jobs to start.
    pub start: Vec<JobRequest>,
}

impl Plan {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.cancel.is_empty() && self.start.is_empty()
    }
}

/// Why a run is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelReason {
    /// The entry that produced the issue.
    pub entry_id: String,
    /// The severity that triggered it.
    pub severity: Severity,
}

/// State of the before-run gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunGate {
    /// Pre-run diagnostics are still running: wait (show a spinner).
    Waiting,
    /// Go ahead and run.
    Proceed,
    /// Do not run: new issues were produced.
    Cancel(CancelReason),
}

/// [`Scheduler::run_gate`] result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateStatus {
    /// The state.
    pub state: RunGate,
    /// Jobs to cancel now (the rest after a decision or a timeout).
    pub cancel: Vec<JobToken>,
}

/// What a finished job reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobOutcome {
    /// New issues it added to the annotation set
    /// ([`crate::ChangeReport::new`]); zero when the job failed.
    pub new: Counts,
    /// The job completed (not an error, not cancelled).
    pub ok: bool,
}

/// What to do with a finished job's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FinishAction {
    /// Apply the result to the annotation set.
    pub accept: bool,
    /// The text changed since the job started: positions may be off (only
    /// for accepted manual/run results; edit results that went stale are not
    /// accepted).
    pub stale_text: bool,
    /// Jobs to cancel as a consequence (the run was cancelled).
    pub cancel: Vec<JobToken>,
}

#[derive(Debug, Clone)]
struct InFlight {
    entry_id: String,
    generation: u64,
    trigger: Trigger,
}

#[derive(Debug, Clone)]
struct Gate {
    waiting: BTreeSet<JobToken>,
    started: Instant,
    cancelled: Option<CancelReason>,
}

/// The scheduling policy of one document. See the module docs.
#[derive(Debug, Clone)]
pub struct Scheduler {
    config: SchedulerConfig,
    entries: Vec<Entry>,
    generation: u64,
    edit_due: Option<Instant>,
    in_flight: BTreeMap<JobToken, InFlight>,
    completed: BTreeMap<String, u64>,
    gate: Option<Gate>,
    next_token: u64,
}

impl Scheduler {
    /// A scheduler with no entries.
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            generation: 0,
            edit_due: None,
            in_flight: BTreeMap::new(),
            completed: BTreeMap::new(),
            gate: None,
            next_token: 1,
        }
    }

    /// Replaces the settings.
    pub fn set_config(&mut self, config: SchedulerConfig) {
        if !config.while_typing {
            self.edit_due = None;
        }
        self.config = config;
    }

    /// The settings.
    pub fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    /// Replaces the entries (language changed, prefs changed). Running jobs
    /// of entries that vanished or became disabled are cancelled.
    pub fn set_entries(&mut self, entries: Vec<Entry>) -> Plan {
        self.entries = entries;
        let dead: Vec<JobToken> = self
            .in_flight
            .iter()
            .filter(|(_, f)| !self.entries.iter().any(|e| e.id == f.entry_id && e.enabled))
            .map(|(t, _)| *t)
            .collect();
        for t in &dead {
            self.in_flight.remove(t);
            if let Some(g) = &mut self.gate {
                g.waiting.remove(t);
            }
        }
        self.completed
            .retain(|id, _| self.entries.iter().any(|e| &e.id == id));
        Plan {
            cancel: dead,
            start: Vec::new(),
        }
    }

    /// The entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The current text generation (bumped by every edit).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn token(&mut self) -> JobToken {
        let t = JobToken(self.next_token);
        self.next_token += 1;
        t
    }

    fn start(&mut self, entry_id: &str, trigger: Trigger, plan: &mut Plan) -> Option<JobToken> {
        let generation = self.generation;
        // At most one job per entry.
        let old: Vec<JobToken> = self
            .in_flight
            .iter()
            .filter(|(_, f)| f.entry_id == entry_id)
            .map(|(t, _)| *t)
            .collect();
        for t in old {
            self.in_flight.remove(&t);
            if let Some(g) = &mut self.gate {
                g.waiting.remove(&t);
            }
            plan.cancel.push(t);
        }
        let token = self.token();
        self.in_flight.insert(
            token,
            InFlight {
                entry_id: entry_id.to_owned(),
                generation,
                trigger,
            },
        );
        plan.start.push(JobRequest {
            token,
            entry_id: entry_id.to_owned(),
            generation,
            trigger,
        });
        Some(token)
    }

    fn job_entries<'a>(&'a self, pred: impl Fn(&Entry) -> bool + 'a) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.enabled && e.is_job() && pred(e))
            .map(|e| e.id.clone())
            .collect()
    }

    // ---------------------------------------------------------------- edits

    /// The text changed at `now`: resets the debounce timer (when linting
    /// while typing and an on-edit entry exists) and cancels running
    /// edit-triggered jobs, whose output is now stale.
    pub fn text_changed(&mut self, now: Instant) -> Plan {
        self.generation += 1;
        let mut plan = Plan::default();
        let stale: Vec<JobToken> = self
            .in_flight
            .iter()
            .filter(|(_, f)| f.trigger == Trigger::Edit)
            .map(|(t, _)| *t)
            .collect();
        for t in stale {
            self.in_flight.remove(&t);
            plan.cancel.push(t);
        }
        let has_edit = self.entries.iter().any(|e| e.enabled && e.is_job() && e.update.on_edit);
        self.edit_due = (self.config.while_typing && has_edit).then(|| now + self.config.delay);
        plan
    }

    /// The next moment [`Scheduler::poll`] has something to do (set a
    /// timer); `None` when idle.
    pub fn next_deadline(&self) -> Option<Instant> {
        let gate = self.gate.as_ref().map(|g| g.started + self.config.gate_timeout);
        match (self.edit_due, gate) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Timer tick: starts the on-edit entries when the debounce elapsed.
    pub fn poll(&mut self, now: Instant) -> Plan {
        let mut plan = Plan::default();
        if let Some(due) = self.edit_due
            && now >= due
        {
            self.edit_due = None;
            for id in self.job_entries(|e| e.update.on_edit) {
                self.start(&id, Trigger::Edit, &mut plan);
            }
        }
        plan
    }

    // ------------------------------------------------------------ explicit

    /// "Check Document Now": every enabled entry that runs on edit or on
    /// run (not manual ones, not console ones), immediately.
    pub fn check_now(&mut self) -> Plan {
        let mut plan = Plan::default();
        self.edit_due = None;
        for id in self.job_entries(|e| e.update.on_edit || e.update.on_run) {
            self.start(&id, Trigger::CheckNow, &mut plan);
        }
        plan
    }

    /// "Run Diagnostics": every enabled entry, manual ones included.
    pub fn run_all(&mut self) -> Plan {
        let mut plan = Plan::default();
        self.edit_due = None;
        for id in self.job_entries(|_| true) {
            self.start(&id, Trigger::All, &mut plan);
        }
        plan
    }

    /// "Run \"X\" Now": one entry, whatever its update mode. Unknown or
    /// disabled entries do nothing.
    pub fn run_entry(&mut self, entry_id: &str) -> Plan {
        let mut plan = Plan::default();
        if self.entries.iter().any(|e| e.id == entry_id && e.enabled && e.is_job()) {
            self.start(entry_id, Trigger::Manual, &mut plan);
        }
        plan
    }

    // ----------------------------------------------------------------- run

    /// A run is about to start: starts the on-run entries whose results are
    /// not current, and arms the gate. Poll [`Scheduler::run_gate`] until it
    /// is not [`RunGate::Waiting`].
    pub fn before_run(&mut self, now: Instant) -> Plan {
        let mut plan = Plan::default();
        self.edit_due = None;
        let mut waiting = BTreeSet::new();
        for id in self.job_entries(|e| e.update.on_run) {
            let current = self.completed.get(&id) == Some(&self.generation)
                && !self.in_flight.values().any(|f| f.entry_id == id);
            if current {
                continue;
            }
            // An in-flight job for the current text can be waited for.
            let running_current = self
                .in_flight
                .iter()
                .find(|(_, f)| f.entry_id == id && f.generation == self.generation)
                .map(|(t, _)| *t);
            if let Some(t) = running_current {
                waiting.insert(t);
                continue;
            }
            if let Some(t) = self.start(&id, Trigger::Run, &mut plan) {
                waiting.insert(t);
            }
        }
        self.gate = Some(Gate {
            waiting,
            started: now,
            cancelled: None,
        });
        plan
    }

    /// The state of the gate armed by [`Scheduler::before_run`]. Returns
    /// `Proceed` when no gate is armed. A decision (`Proceed`/`Cancel`) is
    /// reported once and disarms the gate.
    pub fn run_gate(&mut self, now: Instant) -> GateStatus {
        let Some(gate) = &mut self.gate else {
            return GateStatus {
                state: RunGate::Proceed,
                cancel: Vec::new(),
            };
        };
        if let Some(reason) = gate.cancelled.clone() {
            let cancel = gate.waiting.iter().copied().collect();
            self.gate = None;
            return GateStatus {
                state: RunGate::Cancel(reason),
                cancel,
            };
        }
        if gate.waiting.is_empty() {
            self.gate = None;
            return GateStatus {
                state: RunGate::Proceed,
                cancel: Vec::new(),
            };
        }
        if now >= gate.started + self.config.gate_timeout {
            let cancel = gate.waiting.iter().copied().collect();
            self.gate = None;
            return GateStatus {
                state: RunGate::Proceed,
                cancel,
            };
        }
        GateStatus {
            state: RunGate::Waiting,
            cancel: Vec::new(),
        }
    }

    /// Abandons the gate (the user pressed Stop while waiting); returns the
    /// jobs to cancel.
    pub fn abort_gate(&mut self) -> Vec<JobToken> {
        self.gate
            .take()
            .map(|g| g.waiting.into_iter().collect())
            .unwrap_or_default()
    }

    /// The run finished: the console entries to feed with its output (start
    /// them with [`crate::DiagnosticJob::console_output`] set). Console text
    /// entries are skipped when the "errors from the console" preference is
    /// off; highlight-only entries always run.
    pub fn after_run(&mut self) -> Plan {
        let mut plan = Plan::default();
        let ids: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.enabled && e.is_console() && e.update.on_run)
            .filter(|e| e.kind == DiagnosticKind::ConsoleHighlight || self.config.from_console)
            .map(|e| e.id.clone())
            .collect();
        for id in ids {
            self.start(&id, Trigger::Console, &mut plan);
        }
        plan
    }

    // ------------------------------------------------------------- results

    /// A job finished (whatever the outcome). Say what it produced; get back
    /// whether to apply it.
    pub fn job_finished(&mut self, token: JobToken, outcome: JobOutcome) -> FinishAction {
        let Some(flight) = self.in_flight.remove(&token) else {
            // Cancelled or unknown: never apply.
            if let Some(g) = &mut self.gate {
                g.waiting.remove(&token);
            }
            return FinishAction::default();
        };
        let stale = flight.generation != self.generation;
        let mut action = FinishAction {
            accept: !(stale && flight.trigger == Trigger::Edit),
            stale_text: stale,
            cancel: Vec::new(),
        };
        if outcome.ok && !stale {
            self.completed.insert(flight.entry_id.clone(), flight.generation);
        }
        if let Some(gate) = &mut self.gate
            && gate.waiting.remove(&token)
            && outcome.ok
            && gate.cancelled.is_none()
            && let Some(entry) = self.entries.iter().find(|e| e.id == flight.entry_id)
        {
            let o = entry.options;
            let reason = if o.cancel_on_errors && outcome.new.errors > 0 {
                Some(Severity::Error)
            } else if o.cancel_on_warnings && outcome.new.warnings > 0 {
                Some(Severity::Warning)
            } else if o.cancel_on_notices && outcome.new.notices > 0 {
                Some(Severity::Notice)
            } else {
                None
            };
            if let Some(severity) = reason {
                gate.cancelled = Some(CancelReason {
                    entry_id: flight.entry_id.clone(),
                    severity,
                });
                action.cancel = gate.waiting.iter().copied().collect();
            }
        }
        action
    }

    // ---------------------------------------------------------------- misc

    /// Forget results (the language or settings changed, "Clear
    /// Diagnostics"): the next run re-lints everything.
    pub fn invalidate_results(&mut self) {
        self.completed.clear();
    }

    /// Cancels everything (document closed).
    pub fn cancel_all(&mut self) -> Vec<JobToken> {
        self.edit_due = None;
        self.gate = None;
        let tokens = self.in_flight.keys().copied().collect();
        self.in_flight.clear();
        tokens
    }

    /// Whether any job is running (spinner).
    pub fn is_busy(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// Whether the entry has a job running.
    pub fn is_running(&self, entry_id: &str) -> bool {
        self.in_flight.values().any(|f| f.entry_id == entry_id)
    }

    /// Whether the debounce timer is armed.
    pub fn edit_pending(&self) -> bool {
        self.edit_due.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: DiagnosticKind, update_bits: i64, options_bits: i64) -> Entry {
        Entry {
            id: id.to_owned(),
            kind,
            update: UpdateMode::from_bits(update_bits),
            options: DiagnosticOptions::from_bits(options_bits),
            enabled: true,
        }
    }

    fn sched() -> (Scheduler, Instant) {
        let mut s = Scheduler::new(SchedulerConfig::default());
        s.set_entries(vec![
            entry("clang", DiagnosticKind::Clang, 3, 4),
            entry("shellcheck", DiagnosticKind::Command, 4, 0),
            entry("console", DiagnosticKind::ConsoleText, 2, 0),
            entry("conflict", DiagnosticKind::Conflict, 1, 0),
        ]);
        (s, Instant::now())
    }

    fn ids(plan: &Plan) -> Vec<&str> {
        plan.start.iter().map(|r| r.entry_id.as_str()).collect()
    }

    #[test]
    fn debounce_waits_for_quiet_and_resets() {
        let (mut s, t0) = sched();
        assert!(s.poll(t0 + Duration::from_secs(10)).is_empty(), "nothing runs on open");
        s.text_changed(t0);
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_millis(1500)));
        assert!(s.poll(t0 + Duration::from_millis(1499)).is_empty());
        // Typing again resets the timer.
        s.text_changed(t0 + Duration::from_millis(1400));
        assert!(s.poll(t0 + Duration::from_millis(1600)).is_empty());
        let plan = s.poll(t0 + Duration::from_millis(2900));
        assert_eq!(ids(&plan), ["clang", "conflict"], "edit bit only; manual and console never");
        assert!(s.poll(t0 + Duration::from_secs(60)).is_empty(), "fires once");
        assert!(plan.start.iter().all(|r| r.trigger == Trigger::Edit && r.generation == 2));
    }

    #[test]
    fn edit_cancels_stale_edit_jobs_and_discards_their_results() {
        let (mut s, t0) = sched();
        s.text_changed(t0);
        let plan = s.poll(t0 + Duration::from_secs(2));
        let clang = plan.start[0].token;
        assert!(s.is_running("clang") && s.is_busy());
        let p2 = s.text_changed(t0 + Duration::from_secs(3));
        assert_eq!(p2.cancel.len(), 2);
        assert!(p2.cancel.contains(&clang));
        let a = s.job_finished(clang, JobOutcome::default());
        assert!(!a.accept, "cancelled job result is dropped");
        assert!(!s.is_busy());
    }

    #[test]
    fn while_typing_off_disables_edit_runs() {
        let (mut s, t0) = sched();
        s.set_config(SchedulerConfig {
            while_typing: false,
            ..SchedulerConfig::default()
        });
        s.text_changed(t0);
        assert!(!s.edit_pending());
        assert!(s.poll(t0 + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn one_job_per_entry() {
        let (mut s, _) = sched();
        let a = s.run_entry("shellcheck");
        let first = a.start[0].token;
        let b = s.run_entry("shellcheck");
        assert_eq!(b.cancel, vec![first]);
        assert_ne!(b.start[0].token, first);
        assert!(s.run_entry("console").is_empty(), "console entries are not jobs");
        assert!(s.run_entry("nope").is_empty());
    }

    #[test]
    fn check_now_and_run_all() {
        let (mut s, _) = sched();
        assert_eq!(ids(&s.check_now()), ["clang", "conflict"]);
        let mut all = ids(&s.run_all()).into_iter().map(str::to_owned).collect::<Vec<_>>();
        all.sort();
        assert_eq!(all, ["clang", "conflict", "shellcheck"]);
    }

    #[test]
    fn disabled_entries_do_not_run() {
        let (mut s, t0) = sched();
        let mut entries = s.entries().to_vec();
        entries[0].enabled = false;
        s.set_entries(entries);
        s.text_changed(t0);
        assert_eq!(ids(&s.poll(t0 + Duration::from_secs(2))), ["conflict"]);
        assert!(s.run_entry("clang").is_empty());
    }

    #[test]
    fn run_gate_waits_then_proceeds() {
        let (mut s, t0) = sched();
        s.text_changed(t0);
        let plan = s.before_run(t0);
        assert_eq!(ids(&plan), ["clang"], "only on-run entries");
        let token = plan.start[0].token;
        assert_eq!(s.run_gate(t0).state, RunGate::Waiting);
        let a = s.job_finished(token, JobOutcome { ok: true, ..JobOutcome::default() });
        assert!(a.accept && !a.stale_text);
        assert_eq!(s.run_gate(t0).state, RunGate::Proceed);
        assert_eq!(s.run_gate(t0).state, RunGate::Proceed, "disarmed");
    }

    #[test]
    fn up_to_date_results_are_not_rerun() {
        let (mut s, t0) = sched();
        s.text_changed(t0);
        let plan = s.before_run(t0);
        let token = plan.start[0].token;
        s.job_finished(token, JobOutcome { ok: true, ..JobOutcome::default() });
        assert_eq!(s.run_gate(t0).state, RunGate::Proceed);
        let again = s.before_run(t0);
        assert!(again.start.is_empty(), "text unchanged: nothing to lint");
        assert_eq!(s.run_gate(t0).state, RunGate::Proceed);
        s.text_changed(t0);
        assert_eq!(ids(&s.before_run(t0)), ["clang"], "text changed: lint again");
    }

    #[test]
    fn run_waits_for_an_edit_job_already_running_on_the_same_text() {
        let (mut s, t0) = sched();
        s.text_changed(t0);
        let edit = s.poll(t0 + Duration::from_secs(2));
        let clang = edit.start[0].token;
        let plan = s.before_run(t0 + Duration::from_secs(2));
        assert!(plan.start.is_empty() && plan.cancel.is_empty(), "reuses the running job");
        assert_eq!(s.run_gate(t0 + Duration::from_secs(2)).state, RunGate::Waiting);
        s.job_finished(clang, JobOutcome { ok: true, ..JobOutcome::default() });
        assert_eq!(s.run_gate(t0 + Duration::from_secs(2)).state, RunGate::Proceed);
    }

    #[test]
    fn new_errors_cancel_the_run_per_option_bits() {
        let mut s = Scheduler::new(SchedulerConfig::default());
        s.set_entries(vec![
            entry("errs", DiagnosticKind::Command, 2, 4),
            entry("warns", DiagnosticKind::Command, 2, 8),
            entry("notes", DiagnosticKind::Command, 2, 16),
            entry("plain", DiagnosticKind::Command, 2, 0),
        ]);
        let t0 = Instant::now();
        let new = |e, w, n| JobOutcome {
            ok: true,
            new: Counts { errors: e, warnings: w, notices: n },
        };
        for (entry_idx, outcome, expected) in [
            (0, new(1, 0, 0), Some(Severity::Error)),
            (0, new(0, 3, 0), None),
            (1, new(0, 1, 0), Some(Severity::Warning)),
            (1, new(2, 0, 0), None),
            (2, new(0, 0, 1), Some(Severity::Notice)),
            (3, new(5, 5, 5), None),
        ] {
            s.invalidate_results();
            s.text_changed(t0);
            let plan = s.before_run(t0);
            assert_eq!(plan.start.len(), 4);
            let token = plan.start[entry_idx].token;
            let action = s.job_finished(token, outcome);
            let status = s.run_gate(t0);
            match expected {
                Some(severity) => {
                    assert_eq!(action.cancel.len(), 3, "the other jobs are cancelled at once");
                    assert!(matches!(status.state, RunGate::Cancel(CancelReason { severity: sv, .. }) if sv == severity), "{status:?}");
                }
                None => {
                    assert!(action.cancel.is_empty());
                    assert_eq!(status.state, RunGate::Waiting);
                    s.abort_gate();
                }
            }
            s.cancel_all();
        }
    }

    #[test]
    fn gate_times_out() {
        let (mut s, t0) = sched();
        s.text_changed(t0);
        let plan = s.before_run(t0);
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_secs(20)));
        let status = s.run_gate(t0 + Duration::from_secs(21));
        assert_eq!(status.state, RunGate::Proceed);
        assert_eq!(status.cancel, vec![plan.start[0].token]);
    }

    #[test]
    fn manual_results_are_kept_even_if_text_changed() {
        let (mut s, t0) = sched();
        let plan = s.run_entry("shellcheck");
        s.text_changed(t0);
        let a = s.job_finished(plan.start[0].token, JobOutcome { ok: true, ..JobOutcome::default() });
        assert!(a.accept && a.stale_text, "manual result still shown, marked stale");
    }

    #[test]
    fn after_run_lists_console_entries() {
        let mut s = Scheduler::new(SchedulerConfig::default());
        s.set_entries(vec![
            entry("text", DiagnosticKind::ConsoleText, 2, 0),
            entry("hl", DiagnosticKind::ConsoleHighlight, 2, 0),
            entry("clang", DiagnosticKind::Clang, 3, 0),
        ]);
        let plan = s.after_run();
        assert_eq!(ids(&plan), ["text", "hl"]);
        assert!(plan.start.iter().all(|r| r.trigger == Trigger::Console));
        s.set_config(SchedulerConfig { from_console: false, ..SchedulerConfig::default() });
        assert_eq!(ids(&s.after_run()), ["hl"], "errors from console off: highlight only");
    }

    #[test]
    fn set_entries_cancels_vanished_jobs() {
        let (mut s, _) = sched();
        let plan = s.run_entry("shellcheck");
        let token = plan.start[0].token;
        let cancel = s.set_entries(vec![entry("clang", DiagnosticKind::Clang, 3, 0)]);
        assert_eq!(cancel.cancel, vec![token]);
        assert!(!s.is_busy());
    }

    #[test]
    fn entries_follow_language_and_prefs() {
        let reg = scratch_language::LanguageRegistry::builtin();
        let c = reg.get("c").unwrap();
        let mut prefs = DiagnosticsPrefs::default();
        let doc = DocumentDiagnosticsData::default();
        let (defs, entries) = entries_for(c, &prefs, &doc);
        assert!(defs.iter().any(|d| d.kind == DiagnosticKind::Clang));
        assert!(defs.iter().any(|d| d.kind == DiagnosticKind::Conflict), "built-in conflict checker added");
        assert!(entries.iter().all(|e| e.enabled));
        prefs.disable(&crate::settings::DisableScope::Diagnostic { language: "c".into(), id: "clang".into() });
        let (_, entries) = entries_for(c, &prefs, &doc);
        assert!(!entries.iter().find(|e| e.id == "clang").unwrap().enabled);
        let json = reg.get("json").unwrap();
        let (defs, _) = entries_for(json, &DiagnosticsPrefs::default(), &doc);
        assert!(defs.iter().any(|d| d.kind == DiagnosticKind::InvalidSyntax));
        let p = DiagnosticsPrefs { invalid_syntax: false, ..DiagnosticsPrefs::default() };
        let (defs, _) = entries_for(json, &p, &doc);
        assert!(!defs.iter().any(|d| d.kind == DiagnosticKind::InvalidSyntax));
    }
}
