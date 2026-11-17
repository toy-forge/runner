//! Drives the production manager against the scripted mock server: lifecycle, both sync
//! strategies, every request parser, cancellation, crash restarts, virtual workspaces, and
//! overrides.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use scratch_lsp::{
    CompletionTrigger, Diagnostic, FileEdit, FormattingOptions, LspError, Manager, ManagerConfig,
    ManagerEvent, Position, Range, RestartPolicy, ServerCatalog, ServerOverride, ServerStatus,
    TextEdit, WorkspaceChange, WorkspaceEdit, expand_snippet, path_to_uri,
};

const MOCK: &str = env!("CARGO_BIN_EXE_scratch-lsp-mock-server");

fn catalog(full_sync: bool) -> ServerCatalog {
    let args = if full_sync { "[\"--full\"]" } else { "[]" };
    let text = format!(
        r#"
[[server]]
id = "mock"
name = "Mock Server"
commands = ["{MOCK}"]
args = {args}
install_hint = "the mock server is built by cargo test"
root_markers = []
virtual_path = "main.mock"

[server.languages]
mock = "mock"
"#
    );
    ServerCatalog::from_toml(&text).expect("the test catalogue is valid")
}

fn config_from(root: &Path, catalog: ServerCatalog) -> ManagerConfig {
    ManagerConfig::new(
        root,
        std::env::var("PATH").unwrap_or_default(),
        std::env::vars().collect(),
    )
    .with_catalog(catalog)
    .with_cache_dir(root.join("cache"))
    .with_restart_policy(RestartPolicy {
        initial: Duration::from_millis(100),
        max: Duration::from_millis(300),
        stable_after: Duration::from_secs(10),
    })
}

fn config(root: &Path, full_sync: bool) -> ManagerConfig {
    config_from(root, catalog(full_sync))
}

async fn wait_status(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ManagerEvent>,
    predicate: impl Fn(&ServerStatus) -> bool,
) -> ServerStatus {
    let mut seen = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await {
                Some(ManagerEvent::Status { status, .. }) if predicate(&status) => {
                    return status;
                }
                Some(ManagerEvent::Status { status, .. }) => seen.push(format!("{status:?}")),
                Some(_) => {}
                None => panic!("the manager stopped"),
            }
        }
    })
    .await;
    match result {
        Ok(status) => status,
        Err(_) => panic!("the status arrived in time; saw statuses {seen:?}"),
    }
}

async fn wait_diagnostics(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ManagerEvent>,
    want_problems: bool,
) -> Vec<Diagnostic> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await {
                Some(ManagerEvent::Diagnostics { diagnostics, .. })
                    if diagnostics.is_empty() != want_problems =>
                {
                    return diagnostics;
                }
                Some(_) => {}
                None => panic!("the manager stopped"),
            }
        }
    })
    .await
    .expect("the diagnostics arrived in time")
}

async fn ready(
    _manager: &Manager,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ManagerEvent>,
) {
    wait_status(events, ServerStatus::is_ready).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_sync_requests_and_virtual_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), false));
    let uri = "scratch://doc-one";

    manager
        .sync(uri, "mock", "x = bad\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;
    let diagnostics = wait_diagnostics(&mut events, true).await;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "bad value");
    assert!(diagnostics[0].unnecessary, "the mock sets tag 1");

    let path = manager.workspace().workspace_dir(uri).join("main.mock");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "x = bad\n",
        "the virtual workspace holds the document"
    );

    let list = manager
        .completion(
            uri,
            Position::new(0, 4),
            CompletionTrigger::Character(".".to_owned()),
            Some(".".to_owned()),
        )
        .await
        .unwrap();
    assert!(!list.is_incomplete);
    let item = &list.items[0];
    assert_eq!(item.label, "print");
    assert_eq!(item.label_detail.as_deref(), Some("(value: Any)"));
    assert!(item.detail.as_deref().unwrap().contains("x = bad"));
    assert!(
        item.documentation
            .as_ref()
            .unwrap()
            .to_plain()
            .contains("Prints value")
    );
    assert_eq!(item.insert_format, scratch_lsp::InsertFormat::Snippet);
    let expanded = expand_snippet(&item.text_edit.as_ref().unwrap().new_text);
    assert_eq!(expanded.text, "print(value)");
    assert_eq!(expanded.stops[0].placeholder.as_deref(), Some("value"));
    assert_eq!(item.additional_edits.len(), 1);
    assert_eq!(item.command.as_ref().unwrap().command, "scratch.after");

    let resolved = manager.resolve_completion(uri, item).await.unwrap();
    assert_eq!(resolved.detail.as_deref(), Some("resolved detail"));
    assert!(resolved.documentation.is_some());

    let hover = manager
        .hover(uri, Position::new(0, 1))
        .await
        .unwrap()
        .expect("a hover");
    assert!(matches!(
        hover.contents.blocks[0],
        scratch_lsp::Block::CodeBlock { .. }
    ));
    assert_eq!(hover.contents.links()[0].1, "https://example.com/print");

    let signature = manager
        .signature_help(uri, Position::new(0, 1))
        .await
        .unwrap()
        .expect("signature help");
    assert_eq!(signature.current_parameter(), Some(1));
    assert_eq!(signature.signatures[0].parameters.len(), 3);

    let definition = manager.definition(uri, Position::new(0, 1)).await.unwrap();
    assert_eq!(definition.targets.len(), 2);
    assert_eq!(
        definition.targets[0].uri, uri,
        "virtual URIs are translated"
    );
    assert!(definition.targets[1].uri.ends_with(".other"));
    assert!(definition.origin.is_some());
    assert_eq!(
        manager
            .declaration(uri, Position::new(0, 1))
            .await
            .unwrap()
            .targets
            .len(),
        2
    );
    assert_eq!(
        manager
            .type_definition(uri, Position::new(0, 1))
            .await
            .unwrap()
            .targets
            .len(),
        2
    );
    let triggers = manager.trigger_characters(uri).await;
    assert!(triggers.completion.contains(&".".to_owned()));
    assert!(triggers.signature_help.contains(&"(".to_owned()));
    assert_eq!(
        manager.status_for_language("mock").await,
        Some(ServerStatus::Ready {
            version: Some("1.2.3".to_owned())
        })
    );

    let references = manager
        .references(uri, Position::new(0, 1), true)
        .await
        .unwrap();
    assert_eq!(references.len(), 2);
    assert_eq!(references[0].uri, uri);

    let symbols = manager.document_symbols(uri).await.unwrap();
    assert_eq!(symbols[0].name, "main");
    assert_eq!(symbols[0].children[0].name, "value");

    let folds = manager.folding_ranges(uri).await.unwrap();
    assert_eq!(folds.len(), 1);
    assert_eq!(folds[0].kind, Some(scratch_lsp::FoldKind::Comment));

    let edits = manager
        .format(uri, FormattingOptions::default())
        .await
        .unwrap();
    assert_eq!(edits[0].new_text, "# formatted\n");
    let edits = manager
        .format_range(uri, Range::new(0, 0, 0, 3), FormattingOptions::default())
        .await
        .unwrap();
    assert_eq!(edits[0].new_text, "# formatted\n");

    let prepared = manager
        .prepare_rename(uri, Position::new(0, 1))
        .await
        .unwrap()
        .expect("renameable");
    assert_eq!(prepared.placeholder.as_deref(), Some("old_name"));
    let rename = manager
        .rename(uri, Position::new(0, 1), "new_name")
        .await
        .unwrap();
    assert_eq!(rename.changes.len(), 1);

    let actions = manager
        .code_actions_with_kinds(
            uri,
            Range::new(0, 0, 0, 3),
            &diagnostics,
            Some(&["quickfix".to_owned()]),
        )
        .await
        .unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Fix it");
    assert_eq!(actions[0].diagnostics.len(), 1);
    assert!(actions[0].raw.is_some());
    let resolved = manager.resolve_code_action(uri, &actions[0]).await.unwrap();
    assert!(resolved.edit.is_some());

    let applied = manager.apply_code_action(uri, &actions[0]).await.unwrap();
    assert_eq!(applied.edits.len(), 1);
    assert_eq!(applied.edits[0].uri, uri);
    assert_eq!(applied.edits[0].version, Some(2));
    assert!(applied.edits[0].new_text.starts_with("# fixed\n"));

    let applied = manager.apply_code_action(uri, &actions[1]).await.unwrap();
    assert!(
        applied
            .edits
            .iter()
            .any(|edit| edit.new_text.contains("# command")),
        "{applied:?}"
    );

    manager.save(uri).await.unwrap();
    let list = manager
        .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(
        list.items[0].detail.as_deref().unwrap().contains("saves=1"),
        "{:?}",
        list.items[0].detail
    );

    manager
        .sync(uri, "mock", "y = 2\n".to_owned())
        .await
        .unwrap();
    let list = manager
        .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    let detail = list.items[0].detail.as_deref().unwrap();
    assert!(detail.contains("y = 2"), "{detail}");
    assert!(
        !detail.contains("# fixed"),
        "incremental sync must replace the old text: {detail}"
    );

    let uri2 = "scratch://doc-two";
    manager
        .sync(uri2, "mock", "bad again\n".to_owned())
        .await
        .unwrap();
    let report = manager.pull_diagnostics(uri2).await.unwrap();
    assert!(!report.unchanged);
    assert_eq!(report.diagnostics.len(), 1);
    assert!(report.result_id.is_some());

    manager.close(uri).await;
    assert!(
        !path.exists(),
        "closing a virtual document removes its workspace"
    );
    assert!(matches!(
        manager
            .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
            .await,
        Err(LspError::UnknownDocument(_))
    ));
    assert!(manager.status("mock").await.unwrap().is_ready());
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn full_sync_is_used_when_the_server_asks() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), true));
    let uri = "scratch://full-sync";
    manager.sync(uri, "mock", "one\n".to_owned()).await.unwrap();
    ready(&manager, &mut events).await;
    manager
        .sync(uri, "mock", "two\nthree\n".to_owned())
        .await
        .unwrap();
    let list = manager
        .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(
        list.items[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("two\\nthree"),
        "{:?}",
        list.items[0].detail
    );
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_completion_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), false));
    let uri = "scratch://cancel";
    manager
        .sync(uri, "mock", "SLOW\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;

    let manager_for_first = manager.clone();
    let uri_for_first = uri.to_owned();
    let first = tokio::spawn(async move {
        manager_for_first
            .completion(
                &uri_for_first,
                Position::new(0, 0),
                CompletionTrigger::Invoked,
                None,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = manager
        .completion(uri, Position::new(0, 1), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(!second.items.is_empty());
    let first = first.await.unwrap();
    assert!(
        matches!(first, Err(LspError::Cancelled)),
        "the stale request must be cancelled, got {first:?}"
    );
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_restarts_with_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), false));
    let uri = "scratch://crash";
    manager
        .sync(uri, "mock", "hello\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;
    manager
        .sync(uri, "mock", "CRASH\n".to_owned())
        .await
        .unwrap();
    wait_status(&mut events, |status| {
        matches!(status, ServerStatus::Stopped { .. })
    })
    .await;
    ready(&manager, &mut events).await;
    let list = manager
        .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(
        list.items[0].detail.as_deref().unwrap().contains("CRASH"),
        "the restarted server got the document again: {:?}",
        list.items[0].detail
    );
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_server_is_not_started() {
    let dir = tempfile::tempdir().unwrap();
    let mut overrides = HashMap::new();
    overrides.insert(
        "mock".to_owned(),
        ServerOverride {
            enabled: Some(false),
            ..ServerOverride::default()
        },
    );
    let config = config(dir.path(), false).with_overrides(overrides);
    let (manager, mut events) = Manager::new(config);
    let uri = "scratch://disabled";
    manager.sync(uri, "mock", "x\n".to_owned()).await.unwrap();
    assert_eq!(
        manager.status_for_language("mock").await,
        Some(ServerStatus::Disabled)
    );
    assert!(matches!(
        manager
            .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
            .await,
        Err(LspError::NotRunning)
    ));
    // No server process started, but the manager still reports the status change.
    let status = wait_status(&mut events, |status| *status == ServerStatus::Disabled).await;
    assert_eq!(status, ServerStatus::Disabled);
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn on_disk_project_documents_keep_their_real_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("project.marker"), "").unwrap();
    let file = dir.path().join("doc.mock");
    std::fs::write(&file, "on disk\n").unwrap();
    let catalog = ServerCatalog::from_toml(&format!(
        r#"
[[server]]
id = "mock"
name = "Mock Server"
commands = ["{MOCK}"]
args = []
root_markers = ["project.marker"]
virtual_path = "main.mock"

[server.languages]
mock = "mock"
"#
    ))
    .unwrap();
    let (manager, mut events) = Manager::new(config_from(dir.path(), catalog));
    let uri = path_to_uri(&file);

    manager
        .sync(&uri, "mock", "on disk\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;
    let list = manager
        .completion(&uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(
        list.items[0]
            .detail
            .as_deref()
            .unwrap()
            .contains(&format!("uri={uri}")),
        "the server must see the real file: {:?}",
        list.items[0].detail
    );
    assert!(
        !manager.workspace().workspace_dir(&uri).exists(),
        "a file inside a project needs no virtual copy"
    );
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn single_file_projects_get_their_scaffolding() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("main.rs");
    std::fs::write(&real, "fn main() {}\n").unwrap();
    let catalog = ServerCatalog::from_toml(&format!(
        r#"
[[server]]
id = "mock"
name = "Mock Server"
commands = ["{MOCK}"]
args = []
root_markers = ["Cargo.toml"]
virtual_path = "src/main.rs"
scaffold = "cargo"

[server.languages]
mock = "mock"
"#
    ))
    .unwrap();
    let (manager, mut events) = Manager::new(config_from(dir.path(), catalog));
    let uri = path_to_uri(&real);
    manager
        .sync(&uri, "mock", "fn main() {}\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;
    let workspace = manager.workspace().workspace_dir(&uri);
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/main.rs")).unwrap(),
        "fn main() {}\n"
    );
    let manifest = std::fs::read_to_string(workspace.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("[package]"));
    let list = manager
        .completion(&uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    let detail = list.items[0].detail.as_deref().unwrap();
    assert!(detail.contains("/src/main.rs"), "{detail}");
    assert!(!detail.contains(&format!("uri={uri}")), "{detail}");
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_edits_are_validated_and_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), false));
    let uri = "scratch://edits";
    manager.sync(uri, "mock", "abc\n".to_owned()).await.unwrap();
    ready(&manager, &mut events).await;

    let file_edit = |version: Option<i32>, new_text: &str| WorkspaceEdit {
        changes: vec![WorkspaceChange::Edit(FileEdit {
            uri: uri.to_owned(),
            version,
            edits: vec![TextEdit {
                range: Range::new(0, 0, 0, 3),
                new_text: new_text.to_owned(),
            }],
        })],
    };

    let stale = manager
        .apply_workspace_edit(&file_edit(Some(99), "nope"))
        .await;
    assert!(matches!(stale, Err(LspError::InvalidEdit(_))), "{stale:?}");

    let applied = manager
        .apply_workspace_edit(&file_edit(Some(1), "xyz"))
        .await
        .unwrap();
    assert_eq!(applied.edits.len(), 1);
    assert_eq!(applied.edits[0].new_text, "xyz\n");
    assert_eq!(applied.edits[0].version, Some(2));
    let virtual_file = manager.workspace().workspace_dir(uri).join("main.mock");
    assert_eq!(std::fs::read_to_string(&virtual_file).unwrap(), "xyz\n");
    let list = manager
        .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
        .await
        .unwrap();
    assert!(list.items[0].detail.as_deref().unwrap().contains("xyz"));

    // A file that is not open in the editor is read, edited and written back.
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "hello\n").unwrap();
    let notes_uri = path_to_uri(&notes);
    let applied = manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Edit(FileEdit {
                uri: notes_uri.clone(),
                version: None,
                edits: vec![TextEdit {
                    range: Range::new(0, 0, 0, 5),
                    new_text: "bye".to_owned(),
                }],
            })],
        })
        .await
        .unwrap();
    assert_eq!(applied.edits[0].version, None);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "bye\n");

    let created = dir.path().join("created.txt");
    let created_uri = path_to_uri(&created);
    let applied = manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Create {
                uri: created_uri,
                overwrite: false,
                ignore_if_exists: false,
            }],
        })
        .await
        .unwrap();
    assert!(created.is_file());
    assert_eq!(applied.created.len(), 1);

    std::fs::write(&created, "data").unwrap();
    let renamed = dir.path().join("renamed.txt");
    let renamed_uri = path_to_uri(&renamed);
    let applied = manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Rename {
                old_uri: path_to_uri(&created),
                new_uri: renamed_uri.clone(),
                overwrite: false,
            }],
        })
        .await
        .unwrap();
    assert!(!created.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "data");
    assert_eq!(applied.renamed.len(), 1);

    manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Delete {
                uri: renamed_uri,
                recursive: false,
            }],
        })
        .await
        .unwrap();
    assert!(!renamed.exists());

    let folder = dir.path().join("folder");
    std::fs::create_dir_all(folder.join("inner")).unwrap();
    std::fs::write(folder.join("inner/file"), "x").unwrap();
    manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Delete {
                uri: path_to_uri(&folder),
                recursive: true,
            }],
        })
        .await
        .unwrap();
    assert!(!folder.exists());

    let outside = manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Edit(FileEdit {
                uri: "file:///etc/hosts".to_owned(),
                version: None,
                edits: vec![TextEdit {
                    range: Range::new(0, 0, 0, 1),
                    new_text: String::new(),
                }],
            })],
        })
        .await;
    assert!(
        matches!(outside, Err(LspError::InvalidEdit(_))),
        "{outside:?}"
    );

    // A symlink inside the workspace must not let a server edit files outside it.
    let escape = dir.path().join("escape");
    std::os::unix::fs::symlink("/etc", &escape).unwrap();
    let through_link = manager
        .apply_workspace_edit(&WorkspaceEdit {
            changes: vec![WorkspaceChange::Edit(FileEdit {
                uri: path_to_uri(&escape.join("hosts")),
                version: None,
                edits: vec![TextEdit {
                    range: Range::new(0, 0, 0, 1),
                    new_text: String::new(),
                }],
            })],
        })
        .await;
    assert!(
        matches!(through_link, Err(LspError::InvalidEdit(_))),
        "{through_link:?}"
    );
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_requested_workspace_edit_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, mut events) = Manager::new(config(dir.path(), false));
    let uri = "scratch://apply-edit";
    manager
        .sync(uri, "mock", "APPLYEDIT\n".to_owned())
        .await
        .unwrap();
    ready(&manager, &mut events).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let list = manager
            .completion(uri, Position::new(0, 0), CompletionTrigger::Invoked, None)
            .await
            .unwrap();
        let detail = list.items[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        if detail.contains("# applied") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server-requested edit was not applied: {detail}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_and_failing_servers_report_typed_status() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = ServerCatalog::from_toml(
        r#"
[[server]]
id = "missing"
name = "Missing Server"
commands = ["definitely-not-a-real-lsp-xyz"]
install_hint = "install the missing server"
virtual_path = "main.mock"

[server.languages]
mock = "mock"
"#,
    )
    .unwrap();
    let (manager, mut events) = Manager::new(config_from(dir.path(), catalog));
    manager
        .sync("scratch://missing", "mock", "x\n".to_owned())
        .await
        .unwrap();
    let status = wait_status(&mut events, |status| {
        matches!(status, ServerStatus::NotInstalled { .. })
    })
    .await;
    assert!(
        matches!(&status, ServerStatus::NotInstalled { hint } if hint.contains("install the missing server")),
        "{status:?}"
    );
    assert!(matches!(
        manager
            .completion(
                "scratch://missing",
                Position::new(0, 0),
                CompletionTrigger::Invoked,
                None
            )
            .await,
        Err(LspError::NotRunning)
    ));
    manager.shutdown().await;

    let dir = tempfile::tempdir().unwrap();
    let catalog = ServerCatalog::from_toml(&format!(
        r#"
[[server]]
id = "broken"
name = "Broken Server"
commands = ["{MOCK}"]
args = ["--fail-init"]
root_markers = []
virtual_path = "main.mock"

[server.languages]
mock = "mock"
"#
    ))
    .unwrap();
    let (manager, mut events) = Manager::new(config_from(dir.path(), catalog));
    manager
        .sync("scratch://broken", "mock", "x\n".to_owned())
        .await
        .unwrap();
    let status = wait_status(&mut events, |status| {
        matches!(status, ServerStatus::Error { .. })
    })
    .await;
    assert!(
        matches!(&status, ServerStatus::Error { message } if !message.is_empty()),
        "{status:?}"
    );
    manager.shutdown().await;
}
