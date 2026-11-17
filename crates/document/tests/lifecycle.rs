//! End-to-end: a document's life across crashes and saves, using only the
//! public API.

use std::collections::HashSet;
use std::fs;

use scratch_document::*;
use scratch_language::LanguageRegistry;

#[test]
fn untitled_document_survives_a_crash_and_is_saved_as() {
    let root = tempfile::tempdir().unwrap();
    let paths = scratch_config::Paths::in_dir(root.path());
    paths.ensure_dirs().unwrap();
    let registry = LanguageRegistry::builtin();
    let store = UnsavedStore::from_paths(&paths, "");

    // New document, some typing, debounced autosave.
    let mut created = store
        .create_untitled(
            &registry,
            "python",
            InitialContent::Empty,
            &NewDocumentOptions::default(),
        )
        .unwrap();
    let mut scheduler = AutosaveScheduler::standard();
    created.document.mark_edited(0);
    scheduler.note_edit(created.document.id, 0);
    assert!(scheduler.due(100).is_empty());
    let due = scheduler.due(2_000);
    assert_eq!(due, vec![created.document.id]);
    let text = "print('hello')\n";
    let stamp = store.autosave(&created.document, text).unwrap();
    created.document.stamp = stamp;
    scheduler.mark_saved(created.document.id);

    // Crash: nothing else is in memory. Launch again.
    let store = UnsavedStore::from_paths(&paths, "");
    let report = store.recover();
    assert_eq!(report.recovered.len(), 1);
    let mut recovered = report.recovered.into_iter().next().unwrap();
    assert_eq!(
        (recovered.text.as_str(), recovered.kind),
        (text, RecoveredKind::Untitled)
    );
    let live: HashSet<_> = [recovered.document.id].into();
    let cleanup = store.cleanup_orphans(&live);
    assert!(cleanup.removed_drafts.is_empty());

    // The user saves it as a real file.
    let target = root.path().join("hello.py");
    let saved = save_text(&target, &recovered.text, &recovered.document.save_options()).unwrap();
    let unsaved_file = recovered.document.relocate(target.clone()).unwrap();
    recovered.document.mark_saved(&saved, 10);
    store
        .finish_save_as(recovered.document.id, &unsaved_file)
        .unwrap();
    assert!(!unsaved_file.exists(), "the Unsaved copy is gone");
    assert!(target.is_file());
    assert!(!recovered.document.dirty && !recovered.document.untitled);

    // Per-file metadata travels with the file.
    let mut document = recovered.document;
    document.set_language("python");
    document.run.arguments = "--verbose".into();
    document.breakpoints.toggle(0);
    let meta_store = MetaStore::new().with_sidecar_dir(MetaStore::default_sidecar_dir(&paths));
    meta_store
        .write(&target, &document.to_meta(Some(&registry)))
        .unwrap();

    // Reopen the file in a fresh process: everything comes back.
    let loaded = load_text(&target, &ReadOptions::default()).unwrap();
    let meta = meta_store.read(&target);
    let reopened = Document::from_file(&target, &loaded, &meta, &registry, 20);
    assert_eq!(reopened.language, "python");
    assert!(reopened.language_locked);
    assert_eq!(reopened.run.arguments, "--verbose");
    assert!(reopened.breakpoints.contains(0));

    // Somebody else edits the file: clean documents reload silently,
    // dirty ones conflict.
    fs::write(&target, "print('changed')\n").unwrap();
    let stamp = reopened.stamp.unwrap();
    let state = check_external(&target, &stamp).unwrap();
    assert!(matches!(
        decide(state, false),
        ExternalAction::SilentReload(_)
    ));
    assert!(matches!(decide(state, true), ExternalAction::Conflict(_)));

    // Recent documents and the session round-trip.
    let mut recent = RecentDocuments::default();
    recent.add(&target, 30);
    recent.save(&paths.recent()).unwrap();
    assert_eq!(
        RecentDocuments::load(&paths.recent(), 10).existing().len(),
        1
    );

    let mut window = WindowState::default();
    window.insert_tab(
        TabState::for_document(&reopened),
        scratch_config::InsertNewTabs::AfterExisting,
    );
    let session = Session {
        windows: vec![window],
        active_window: Some(0),
        ..Session::default()
    };
    session.save(&paths.session()).unwrap();
    let (loaded_session, warning) = Session::load(&paths.session());
    assert_eq!(warning, None);
    assert_eq!(loaded_session.document_ids(), vec![reopened.id]);
}

#[test]
fn quitting_with_unsaved_work_asks_in_order() {
    let a = DocumentStatus::new(DocumentId::new(), "a.py")
        .dirty()
        .running();
    let b = DocumentStatus::new(DocumentId::new(), "Untitled.py")
        .untitled()
        .dirty();
    let policy = ClosePolicy {
        autosave_in_place: false,
        ask_to_save_untitled_on_quit: false,
    };
    let plan = plan_close(&[a.clone(), b.clone()], CloseReason::Quit, policy);
    let mut flow = CloseFlow::new(plan);
    assert!(matches!(
        flow.current(),
        Some(Prompt::ConfirmRunning { .. })
    ));
    flow.answer(Answer::Close);
    let outcome = flow.answer(Answer::Save).unwrap();
    assert_eq!(outcome.stop, vec![a.id]);
    assert_eq!(outcome.save, vec![a.id]);
    assert_eq!(outcome.keep_for_restore, vec![b.id]);
}
