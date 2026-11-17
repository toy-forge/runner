//! Talks to a real Pyright server when one is installed; otherwise the test says so and passes.

use std::time::Duration;

use scratch_lsp::{
    CompletionTrigger, Manager, ManagerConfig, ManagerEvent, Position, ServerStatus, find_program,
};

fn pyright_program() -> Option<std::path::PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    find_program("pyright-langserver", &path, &[])
        .or_else(|| find_program("basedpyright-langserver", &path, &[]))
}

async fn wait_diagnostics(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ManagerEvent>,
    want_problems: bool,
) -> Vec<scratch_lsp::Diagnostic> {
    tokio::time::timeout(Duration::from_secs(120), async {
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
    .expect("diagnostics arrived in time")
}

#[tokio::test(flavor = "multi_thread")]
async fn pyright_diagnostics_completion_hover_and_sync() {
    if pyright_program().is_none() {
        eprintln!("skipped: pyright-langserver is not installed");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let config = ManagerConfig::new(
        root.path(),
        std::env::var("PATH").unwrap_or_default(),
        std::env::vars().collect(),
    )
    .with_cache_dir(root.path().join("cache"));
    let (manager, mut events) = Manager::new(config);
    let uri = format!("file://{}/scratch.py", root.path().display());

    manager
        .sync(&uri, "python", "import os\nx: int = 'a'\nos.".to_owned())
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match events.recv().await {
                Some(ManagerEvent::Status { status, .. }) if status.is_ready() => return status,
                Some(ManagerEvent::Status {
                    status: ServerStatus::Error { message },
                    ..
                }) => panic!("Pyright failed to start: {message}"),
                Some(_) => {}
                None => panic!("the manager stopped"),
            }
        }
    })
    .await
    .expect("Pyright started");
    assert!(status.is_ready());

    // The unsaved document was materialised for the server.
    let virtual_file = manager.workspace().workspace_dir(&uri).join("main.py");
    assert!(virtual_file.is_file(), "{}", virtual_file.display());
    assert!(
        std::fs::read_to_string(&virtual_file)
            .unwrap()
            .contains("import os")
    );

    let problems = wait_diagnostics(&mut events, true).await;
    assert!(
        problems
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not assignable")),
        "{problems:?}"
    );

    let list = manager
        .completion(
            &uri,
            Position::new(2, 3),
            CompletionTrigger::Character(".".to_owned()),
            Some(".".to_owned()),
        )
        .await
        .unwrap();
    assert!(
        list.items.iter().any(|item| item.label == "path"),
        "{:?}",
        list.items
            .iter()
            .map(|item| &item.label)
            .collect::<Vec<_>>()
    );

    let calls = "import os\nprint(\n";
    manager
        .sync(&uri, "python", calls.to_owned())
        .await
        .unwrap();
    let signature = manager
        .signature_help(&uri, Position::new(1, 6))
        .await
        .unwrap()
        .expect("a signature for print(");
    assert!(
        signature
            .signatures
            .iter()
            .any(|signature| !signature.parameters.is_empty()),
        "{signature:?}"
    );

    let hover = manager
        .hover(&uri, Position::new(0, 8))
        .await
        .unwrap()
        .expect("hover on the module name");
    assert!(
        hover.contents.to_plain().to_lowercase().contains("os"),
        "{hover:?}"
    );

    manager
        .sync(
            &uri,
            "python",
            "import os\nx: int = 1\nprint(os.name)\n".to_owned(),
        )
        .await
        .unwrap();
    let cleared = wait_diagnostics(&mut events, false).await;
    assert!(cleared.is_empty(), "{cleared:?}");

    manager.close(&uri).await;
    manager.shutdown().await;
}
