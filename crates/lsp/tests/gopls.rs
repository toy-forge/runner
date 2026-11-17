//! Talks to a real gopls when it and the Go toolchain are installed; otherwise skips visibly.

use std::time::Duration;

use scratch_lsp::{
    CompletionTrigger, Manager, ManagerConfig, ManagerEvent, Position, ServerStatus, find_program,
};

fn gopls_available() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    find_program("gopls", &path, &[]).is_some() && find_program("go", &path, &[]).is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn gopls_diagnostics_completion_and_symbols() {
    if !gopls_available() {
        eprintln!("skipped: gopls or go is not installed");
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
    let uri = format!("file://{}/main.go", root.path().display());
    let source =
        "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tvar x int = \"bad\"\n\tfmt.Pr()\n}\n";

    manager.sync(&uri, "go", source.to_owned()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match events.recv().await {
                Some(ManagerEvent::Status { status, .. }) if status.is_ready() => return,
                Some(ManagerEvent::Status {
                    status: ServerStatus::Error { message },
                    ..
                }) => panic!("gopls failed to start: {message}"),
                Some(_) => {}
                None => panic!("the manager stopped"),
            }
        }
    })
    .await
    .expect("gopls started");

    // gopls builds a package from the virtual workspace's go.mod and main.go.
    let module = manager.workspace().workspace_dir(&uri).join("go.mod");
    assert!(module.is_file(), "{}", module.display());
    assert!(
        std::fs::read_to_string(&module)
            .unwrap()
            .starts_with("module scratchrunner/")
    );

    let diagnostics = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match events.recv().await {
                Some(ManagerEvent::Diagnostics { diagnostics, .. }) if !diagnostics.is_empty() => {
                    return diagnostics;
                }
                Some(_) => {}
                None => panic!("the manager stopped"),
            }
        }
    })
    .await
    .expect("gopls diagnostics arrived in time");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.message.to_lowercase().contains("cannot use")
                || diagnostic.message.to_lowercase().contains("int")
        }),
        "{diagnostics:?}"
    );

    let symbols = manager.document_symbols(&uri).await.unwrap();
    assert!(
        symbols.iter().any(|symbol| symbol.name == "main"),
        "{symbols:?}"
    );

    let completions = manager
        .completion(
            &uri,
            Position::new(6, 7),
            CompletionTrigger::Character(".".to_owned()),
            Some(".".to_owned()),
        )
        .await
        .unwrap();
    assert!(
        completions
            .items
            .iter()
            .any(|item| item.label.contains("Println")),
        "{:?}",
        completions
            .items
            .iter()
            .map(|item| &item.label)
            .collect::<Vec<_>>()
    );

    manager.close(&uri).await;
    manager.shutdown().await;
}
