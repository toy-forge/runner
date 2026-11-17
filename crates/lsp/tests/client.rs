//! The low-level client against the mock server: initialization options, settings, capabilities,
//! and cancellation.

use std::path::PathBuf;

use scratch_lsp::{Client, ClientEvent, LspError, ServerCommand, StartOptions, TextSyncKind};
use serde_json::json;

const MOCK: &str = env!("CARGO_BIN_EXE_scratch-lsp-mock-server");

fn options(root: &std::path::Path) -> StartOptions {
    let mut options = StartOptions::new(
        ServerCommand {
            program: PathBuf::from(MOCK),
            args: vec![],
            cwd: root.to_path_buf(),
            env: std::env::vars().collect(),
        },
        format!("file://{}/workspace", root.display()),
    );
    options.workspace_name = "test-workspace".to_owned();
    options.initialization_options = json!({"marker": 1});
    options.settings = json!({"python": {"analysis": {"typeCheckingMode": "basic"}}});
    options
}

#[tokio::test(flavor = "multi_thread")]
async fn client_initializes_and_sends_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel::<ClientEvent>();
    let client = Client::start(&options(dir.path()), events).await.unwrap();
    assert_eq!(client.server_info().name, "ScratchRunner Mock");
    assert_eq!(client.server_info().version.as_deref(), Some("1.2.3"));
    assert_eq!(
        client.capabilities().text_sync.kind,
        TextSyncKind::Incremental
    );
    assert!(client.capabilities().completion.as_ref().unwrap().resolve);
    assert!(client.capabilities().hover);
    assert!(client.capabilities().diagnostics.is_some());

    let state = client.request("scratch/state", json!({})).await.unwrap();
    assert_eq!(state["initialize"]["initializationOptions"]["marker"], 1);
    assert_eq!(
        state["initialize"]["workspaceFolders"][0]["name"],
        "test-workspace"
    );
    assert_eq!(
        state["initialize"]["capabilities"]["general"]["positionEncodings"][0],
        "utf-16"
    );
    assert_eq!(
        state["settings"]["python"]["analysis"]["typeCheckingMode"], "basic",
        "settings are sent with didChangeConfiguration: {state}"
    );

    // A slow request can be cancelled without waiting for the server.
    let slow = client.request_async("scratch/slow", json!({})).unwrap();
    let id = slow.id();
    client.cancel(id);
    assert!(matches!(slow.receive().await, Err(LspError::Cancelled)));

    client.shutdown().await;
    // The client reports the server stopping.
    let mut stopped = false;
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), received.recv()).await
    {
        if matches!(event, ClientEvent::Stopped { .. }) {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the client reports the server stopping");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_reports_a_server_that_dies_before_initialize() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = options(dir.path());
    options.command.args = vec!["--fail-init".to_owned()];
    let (events, _received) = tokio::sync::mpsc::unbounded_channel::<ClientEvent>();
    let error = match Client::start(&options, events).await {
        Ok(_) => panic!("a server that dies before initialize must not start"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, LspError::Start(message) if message.contains("did not initialize")),
        "{error:?}"
    );
}
