//! The close and quit decision model (pure).
//!
//! The UI collects the state of the documents that are about to go away
//! ([`DocumentStatus`]) and asks [`plan_close`] what has to be asked and done.
//! The result is a [`ClosePlan`]: the ordered prompts (*"X" is currently
//! running. Are you sure you want to close it?* and *Do you want to save the
//! changes...?*) plus the silent actions (autosave, discarding empty untitled
//! documents, keeping untitled documents for the next launch). A
//! [`CloseFlow`] walks the UI through the prompts; when the last one is
//! answered it yields a [`CloseOutcome`] that the UI executes (stop runs, save,
//! discard, close). A Cancel at any point aborts everything: nothing is
//! executed.
//!
//! The rules (research 01 section 13, report 10 G12, preferences 03):
//!
//! * a running document asks for confirmation first (`Closing the document
//!   will stop the run.`), then for saving, document by document in the given
//!   order;
//! * a document with unsaved edits and a file of its own is saved silently
//!   when *Autosave* is on (NSDocument autosaves in place), else the user is
//!   asked;
//! * an untitled document with content asks on close; when quitting it asks
//!   only if *Ask to save untitled documents when quitting* is on, else it is
//!   kept in the Unsaved directory and restored at the next launch;
//! * an empty untitled document is just deleted;
//! * *Close Tab Without Saving* never asks to save (running still asks).
//!
//! ```
//! use scratch_document::*;
//!
//! let a = DocumentStatus::new(DocumentId::new(), "a.py").dirty();
//! let policy = ClosePolicy { autosave_in_place: false, ask_to_save_untitled_on_quit: false };
//! let plan = plan_close(&[a.clone()], CloseReason::CloseTab, policy);
//! let mut flow = CloseFlow::new(plan);
//! assert!(matches!(flow.current(), Some(Prompt::SaveChanges { .. })));
//! let outcome = flow.answer(Answer::Save).unwrap();
//! assert_eq!(outcome.save, vec![a.id]);
//! ```

use crate::DocumentId;

/// What the app knows about a document that may close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStatus {
    /// The document.
    pub id: DocumentId,
    /// Its name, for the prompts.
    pub title: String,
    /// It has unsaved edits.
    pub dirty: bool,
    /// It has never been saved (its file is in the Unsaved directory).
    pub untitled: bool,
    /// Its text is not empty.
    pub has_content: bool,
    /// A run (or debug session) of it is in progress.
    pub running: bool,
}

impl DocumentStatus {
    /// A clean, saved, idle document.
    pub fn new(id: DocumentId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            dirty: false,
            untitled: false,
            has_content: true,
            running: false,
        }
    }

    /// Marks it as having unsaved edits.
    #[must_use]
    pub fn dirty(mut self) -> Self {
        self.dirty = true;
        self
    }

    /// Marks it as untitled with content.
    #[must_use]
    pub fn untitled(mut self) -> Self {
        self.untitled = true;
        self
    }

    /// Marks its text as empty.
    #[must_use]
    pub fn empty(mut self) -> Self {
        self.has_content = false;
        self
    }

    /// Marks it as running.
    #[must_use]
    pub fn running(mut self) -> Self {
        self.running = true;
        self
    }
}

/// Why documents are going away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// `Close Tab` / `Close Document` / `Close Window`: the given documents.
    CloseTab,
    /// `Close Other Tabs`.
    CloseOthers,
    /// `Close Tab Without Saving`: edits are thrown away.
    WithoutSaving,
    /// Quit the app.
    Quit,
}

/// The preferences the decision depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosePolicy {
    /// `general.documents.autosave`: documents with a file are saved in place
    /// without asking.
    pub autosave_in_place: bool,
    /// `general.documents.ask_to_save_when_quitting`.
    pub ask_to_save_untitled_on_quit: bool,
}

impl ClosePolicy {
    /// From the preferences.
    pub fn from_preferences(prefs: &scratch_config::Preferences) -> Self {
        Self {
            autosave_in_place: prefs.general.documents.autosave,
            ask_to_save_untitled_on_quit: prefs.general.documents.ask_to_save_when_quitting,
        }
    }
}

impl Default for ClosePolicy {
    /// CodeRunner's defaults: autosave on, do not ask for untitled documents.
    fn default() -> Self {
        Self {
            autosave_in_place: true,
            ask_to_save_untitled_on_quit: false,
        }
    }
}

/// A question for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// A running document is about to close.
    ConfirmRunning {
        /// The document.
        id: DocumentId,
        /// Its name.
        title: String,
    },
    /// A document with unsaved changes is about to close.
    SaveChanges {
        /// The document.
        id: DocumentId,
        /// Its name.
        title: String,
        /// It has never been saved (Save asks for a name and a place).
        untitled: bool,
    },
}

impl Prompt {
    /// The document the prompt is about.
    pub fn document(&self) -> DocumentId {
        match self {
            Self::ConfirmRunning { id, .. } | Self::SaveChanges { id, .. } => *id,
        }
    }

    /// The alert's message text.
    pub fn message(&self) -> String {
        match self {
            Self::ConfirmRunning { title, .. } => {
                format!("\"{title}\" is currently running. Are you sure you want to close it?")
            }
            Self::SaveChanges { title, .. } => {
                format!("Do you want to save the changes made to the document \"{title}\"?")
            }
        }
    }

    /// The alert's informative text.
    pub fn informative_text(&self) -> &'static str {
        match self {
            Self::ConfirmRunning { .. } => "Closing the document will stop the run.",
            Self::SaveChanges { .. } => "Your changes will be lost if you don't save them.",
        }
    }

    /// The button titles in display order (the first is the default button).
    pub fn buttons(&self) -> &'static [&'static str] {
        match self {
            Self::ConfirmRunning { .. } => &["Close", "Cancel"],
            Self::SaveChanges { .. } => &["Save", "Cancel", "Don't Save"],
        }
    }
}

/// An answer to a [`Prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// `Save` (for a running prompt: `Close`).
    Save,
    /// `Don't Save`.
    DontSave,
    /// `Close` a running document.
    Close,
    /// `Cancel`: nothing is closed.
    Cancel,
}

/// What [`plan_close`] decided.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClosePlan {
    /// The questions, in the order they are asked.
    pub prompts: Vec<Prompt>,
    /// Documents saved without asking.
    pub auto_save: Vec<DocumentId>,
    /// Empty untitled documents to delete.
    pub discard: Vec<DocumentId>,
    /// Untitled documents left in the Unsaved directory for the next launch
    /// (quit without asking).
    pub keep_for_restore: Vec<DocumentId>,
    /// Every document the plan covers (running ones get stopped).
    pub documents: Vec<DocumentId>,
    /// The subset that is running.
    pub running: Vec<DocumentId>,
}

/// Decides what closing `documents` (in tab order, front window first when
/// quitting) requires. See the module documentation of `closing` for the rules.
pub fn plan_close(
    documents: &[DocumentStatus],
    reason: CloseReason,
    policy: ClosePolicy,
) -> ClosePlan {
    let mut plan = ClosePlan::default();
    for doc in documents {
        plan.documents.push(doc.id);
        if doc.running {
            plan.running.push(doc.id);
            plan.prompts.push(Prompt::ConfirmRunning {
                id: doc.id,
                title: doc.title.clone(),
            });
        }
        if doc.untitled && !doc.has_content {
            plan.discard.push(doc.id);
            continue;
        }
        if !doc.dirty || reason == CloseReason::WithoutSaving {
            if doc.untitled && reason == CloseReason::Quit && !policy.ask_to_save_untitled_on_quit {
                plan.keep_for_restore.push(doc.id);
            } else if doc.untitled && reason == CloseReason::WithoutSaving {
                plan.discard.push(doc.id);
            }
            continue;
        }
        if doc.untitled {
            if reason == CloseReason::Quit && !policy.ask_to_save_untitled_on_quit {
                plan.keep_for_restore.push(doc.id);
            } else {
                plan.prompts.push(Prompt::SaveChanges {
                    id: doc.id,
                    title: doc.title.clone(),
                    untitled: true,
                });
            }
        } else if policy.autosave_in_place {
            plan.auto_save.push(doc.id);
        } else {
            plan.prompts.push(Prompt::SaveChanges {
                id: doc.id,
                title: doc.title.clone(),
                untitled: false,
            });
        }
    }
    plan
}

/// What the UI has to do after all prompts were answered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloseOutcome {
    /// Stop the runs of these documents first.
    pub stop: Vec<DocumentId>,
    /// Save these (silently, or after `Save`; untitled ones need the save
    /// panel, and a cancelled panel cancels the close).
    pub save: Vec<DocumentId>,
    /// Throw these away (their edits; an untitled one's file too).
    pub discard: Vec<DocumentId>,
    /// Leave these untitled documents in the Unsaved directory.
    pub keep_for_restore: Vec<DocumentId>,
    /// Close the views of all of these.
    pub close: Vec<DocumentId>,
}

/// Walks through a [`ClosePlan`]'s prompts.
#[derive(Debug, Clone)]
pub struct CloseFlow {
    plan: ClosePlan,
    next: usize,
    stop: Vec<DocumentId>,
    save: Vec<DocumentId>,
    discard: Vec<DocumentId>,
    cancelled: bool,
}

impl CloseFlow {
    /// Starts a flow at the first prompt.
    pub fn new(plan: ClosePlan) -> Self {
        Self {
            save: plan.auto_save.clone(),
            discard: plan.discard.clone(),
            plan,
            next: 0,
            stop: Vec::new(),
            cancelled: false,
        }
    }

    /// The prompt to show now; `None` when the flow is over (everything was
    /// answered, or it was cancelled) or the plan has no prompts: call
    /// [`CloseFlow::finish`].
    pub fn current(&self) -> Option<&Prompt> {
        if self.cancelled {
            None
        } else {
            self.plan.prompts.get(self.next)
        }
    }

    /// Answers the current prompt. Returns the outcome once the last prompt is
    /// answered (or `None` after a Cancel, which aborts the close, and
    /// `None` while prompts remain; see [`CloseFlow::is_cancelled`]).
    pub fn answer(&mut self, answer: Answer) -> Option<CloseOutcome> {
        let prompt = self.plan.prompts.get(self.next)?.clone();
        match (&prompt, answer) {
            (_, Answer::Cancel) => {
                self.cancelled = true;
                return None;
            }
            (Prompt::ConfirmRunning { id, .. }, _) => self.stop.push(*id),
            (Prompt::SaveChanges { id, .. }, Answer::Save) => self.save.push(*id),
            (Prompt::SaveChanges { id, .. }, Answer::DontSave | Answer::Close) => {
                self.discard.push(*id)
            }
        }
        self.next += 1;
        if self.next >= self.plan.prompts.len() {
            self.finish()
        } else {
            None
        }
    }

    /// The outcome, if the flow was not cancelled and all prompts are done
    /// (also for plans without prompts).
    pub fn finish(&self) -> Option<CloseOutcome> {
        if self.cancelled || self.next < self.plan.prompts.len() {
            return None;
        }
        Some(CloseOutcome {
            stop: self.stop.clone(),
            save: self.save.clone(),
            discard: self.discard.clone(),
            keep_for_restore: self.plan.keep_for_restore.clone(),
            close: self.plan.documents.clone(),
        })
    }

    /// The user cancelled: close nothing.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

/// The title of `File > Close Window` (⌘W) for the window's state: `Close Tab`
/// when the window has several tabs and shows the tab bar, `Close Document`
/// when several tabs share a hidden tab bar, else `Close Window`.
pub fn close_command_title(tab_count: usize, tab_bar_hidden: bool) -> &'static str {
    match (tab_count, tab_bar_hidden) {
        (0 | 1, _) => "Close Window",
        (_, false) => "Close Tab",
        (_, true) => "Close Document",
    }
}

/// The title of `Close Tab Without Saving` (⌃⌥⌘W).
pub fn close_without_saving_title(tab_count: usize, tab_bar_hidden: bool) -> &'static str {
    if tab_count > 1 && !tab_bar_hidden {
        "Close Tab Without Saving"
    } else {
        "Close Document Without Saving"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(title: &str) -> DocumentStatus {
        DocumentStatus::new(DocumentId::new(), title)
    }

    fn manual() -> ClosePolicy {
        ClosePolicy {
            autosave_in_place: false,
            ask_to_save_untitled_on_quit: false,
        }
    }

    #[test]
    fn clean_documents_close_silently() {
        let plan = plan_close(&[doc("a"), doc("b")], CloseReason::CloseTab, manual());
        assert!(plan.prompts.is_empty() && plan.auto_save.is_empty());
        assert_eq!(plan.documents.len(), 2);
        let outcome = CloseFlow::new(plan).finish().unwrap();
        assert_eq!(outcome.close.len(), 2);
        assert!(outcome.save.is_empty());
    }

    #[test]
    fn dirty_documents_ask_when_autosave_is_off_and_save_silently_when_on() {
        let a = doc("a.py").dirty();
        let plan = plan_close(std::slice::from_ref(&a), CloseReason::CloseTab, manual());
        assert_eq!(plan.prompts.len(), 1);
        let auto = ClosePolicy {
            autosave_in_place: true,
            ..manual()
        };
        let plan = plan_close(std::slice::from_ref(&a), CloseReason::CloseTab, auto);
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.auto_save, vec![a.id]);
    }

    #[test]
    fn running_documents_ask_first_then_save_per_document_in_order() {
        let a = doc("a").dirty().running();
        let b = doc("b").dirty();
        let plan = plan_close(&[a.clone(), b.clone()], CloseReason::Quit, manual());
        let kinds: Vec<_> = plan
            .prompts
            .iter()
            .map(|p| match p {
                Prompt::ConfirmRunning { id, .. } => ("run", *id),
                Prompt::SaveChanges { id, .. } => ("save", *id),
            })
            .collect();
        assert_eq!(kinds, [("run", a.id), ("save", a.id), ("save", b.id)]);
        assert_eq!(plan.running, vec![a.id]);
    }

    #[test]
    fn untitled_documents_on_quit_are_kept_unless_asked() {
        let u = doc("Untitled.py").untitled().dirty();
        let plan = plan_close(std::slice::from_ref(&u), CloseReason::Quit, manual());
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.keep_for_restore, vec![u.id]);
        let ask = ClosePolicy {
            ask_to_save_untitled_on_quit: true,
            ..manual()
        };
        let plan = plan_close(std::slice::from_ref(&u), CloseReason::Quit, ask);
        assert_eq!(plan.prompts.len(), 1);
        assert!(plan.keep_for_restore.is_empty());
        // closing the tab always asks
        let plan = plan_close(std::slice::from_ref(&u), CloseReason::CloseTab, manual());
        assert!(matches!(
            plan.prompts[0],
            Prompt::SaveChanges { untitled: true, .. }
        ));
    }

    #[test]
    fn clean_untitled_documents_are_kept_on_quit_and_empty_ones_deleted() {
        let clean = doc("Untitled.py").untitled();
        let empty = doc("Untitled 2.py").untitled().empty();
        let plan = plan_close(&[clean.clone(), empty.clone()], CloseReason::Quit, manual());
        assert_eq!(plan.keep_for_restore, vec![clean.id]);
        assert_eq!(plan.discard, vec![empty.id]);
        let plan = plan_close(
            std::slice::from_ref(&clean),
            CloseReason::CloseTab,
            manual(),
        );
        assert!(plan.keep_for_restore.is_empty() && plan.prompts.is_empty());
    }

    #[test]
    fn close_without_saving_never_asks_to_save_but_still_asks_about_runs() {
        let a = doc("a").dirty().running();
        let u = doc("Untitled.py").untitled().dirty();
        let plan = plan_close(
            &[a.clone(), u.clone()],
            CloseReason::WithoutSaving,
            manual(),
        );
        assert_eq!(plan.prompts.len(), 1);
        assert!(matches!(plan.prompts[0], Prompt::ConfirmRunning { .. }));
        assert_eq!(
            plan.discard,
            vec![u.id],
            "an untitled document is deleted with its edits"
        );
        assert!(plan.auto_save.is_empty());
    }

    #[test]
    fn flow_collects_the_answers() {
        let (a, b, c) = (
            doc("a").dirty().running(),
            doc("b").dirty(),
            doc("c").dirty(),
        );
        let plan = plan_close(
            &[a.clone(), b.clone(), c.clone()],
            CloseReason::CloseOthers,
            manual(),
        );
        let mut flow = CloseFlow::new(plan);
        assert!(matches!(
            flow.current(),
            Some(Prompt::ConfirmRunning { .. })
        ));
        assert_eq!(flow.answer(Answer::Close), None);
        assert_eq!(flow.answer(Answer::Save), None); // a
        assert_eq!(flow.answer(Answer::DontSave), None); // b
        let outcome = flow.answer(Answer::Save).unwrap(); // c
        assert_eq!(outcome.stop, vec![a.id]);
        assert_eq!(outcome.save, vec![a.id, c.id]);
        assert_eq!(outcome.discard, vec![b.id]);
        assert_eq!(outcome.close, vec![a.id, b.id, c.id]);
        assert!(flow.current().is_none());
    }

    #[test]
    fn cancel_aborts_everything() {
        let (a, b) = (doc("a").dirty(), doc("b").dirty());
        let plan = plan_close(&[a, b], CloseReason::Quit, manual());
        let mut flow = CloseFlow::new(plan);
        assert_eq!(flow.answer(Answer::Save), None);
        assert_eq!(flow.answer(Answer::Cancel), None);
        assert!(flow.is_cancelled());
        assert!(flow.current().is_none() && flow.finish().is_none());
    }

    #[test]
    fn silent_actions_travel_with_the_outcome() {
        let auto = ClosePolicy {
            autosave_in_place: true,
            ask_to_save_untitled_on_quit: false,
        };
        let a = doc("a").dirty();
        let u = doc("Untitled.py").untitled().dirty();
        let plan = plan_close(&[a.clone(), u.clone()], CloseReason::Quit, auto);
        let outcome = CloseFlow::new(plan).finish().unwrap();
        assert_eq!(outcome.save, vec![a.id]);
        assert_eq!(outcome.keep_for_restore, vec![u.id]);
    }

    #[test]
    fn prompt_texts_match_codrunner() {
        let id = DocumentId::new();
        let run = Prompt::ConfirmRunning {
            id,
            title: "a.py".into(),
        };
        assert_eq!(
            run.message(),
            "\"a.py\" is currently running. Are you sure you want to close it?"
        );
        assert_eq!(
            run.informative_text(),
            "Closing the document will stop the run."
        );
        assert_eq!(run.buttons(), ["Close", "Cancel"]);
        let save = Prompt::SaveChanges {
            id,
            title: "a.py".into(),
            untitled: false,
        };
        assert_eq!(save.buttons(), ["Save", "Cancel", "Don't Save"]);
        assert_eq!(save.document(), id);
    }

    #[test]
    fn menu_titles() {
        assert_eq!(close_command_title(1, false), "Close Window");
        assert_eq!(close_command_title(3, false), "Close Tab");
        assert_eq!(close_command_title(3, true), "Close Document");
        assert_eq!(
            close_without_saving_title(3, false),
            "Close Tab Without Saving"
        );
        assert_eq!(
            close_without_saving_title(1, false),
            "Close Document Without Saving"
        );
    }

    #[test]
    fn policy_follows_the_preferences() {
        let mut prefs = scratch_config::Preferences::default();
        assert_eq!(
            ClosePolicy::from_preferences(&prefs),
            ClosePolicy::default()
        );
        prefs.general.documents.autosave = false;
        prefs.general.documents.ask_to_save_when_quitting = true;
        let policy = ClosePolicy::from_preferences(&prefs);
        assert!(!policy.autosave_in_place && policy.ask_to_save_untitled_on_quit);
    }
}
