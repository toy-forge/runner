use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::geometry::{Points, Size};
use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Environment, Root};
use nsh_testing::{DirectoryAssets, Headless};

static DATA_DIR_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var("SCRATCHRUNNER_DATA_DIR", v) },
            None => unsafe { std::env::remove_var("SCRATCHRUNNER_DATA_DIR") },
        }
    }
}

fn start_workbench() -> (EnvGuard, Headless) {
    let lock = DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("create temp data dir");
    let prev = std::env::var_os("SCRATCHRUNNER_DATA_DIR");
    unsafe { std::env::set_var("SCRATCHRUNNER_DATA_DIR", dir.path()) };

    let (theme, _) = nsh_base::Theme::from_json_lenient(include_str!("../ui/theme.json"))
        .expect("ui/theme.json is valid");
    let root = Root::of::<scratchrunner::ScratchRunner>().tokens(theme.into_tokens());
    let mut environment = Environment::default();
    environment.os = Os::MacOs;
    environment.idiom = Idiom::Desktop;
    environment.apple_keyboard = true;

    let dev_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dev-assets");
    let app = Headless::start(EngineConfig {
        viewport: Size::new(Points(1280.0), Points(800.0)),
        environment,
        assets: Some(Arc::new(DirectoryAssets::new(dev_assets, Vec::new()))),
        ..EngineConfig::new(root)
    })
    .expect("start headless ScratchRunner workbench");

    (EnvGuard { _lock: lock, _dir: dir, prev }, app)
}

#[test]
fn boots_with_welcome_document_and_status_bar() -> Result<(), String> {
    let (_env, mut app) = start_workbench();
    app.wait_for("Syntax: Shell Script")?;
    app.wait_for("Line 2, Column 33")?;
    assert!(app.screen().shows("Untitled.sh") || app.screen().texts().iter().any(|t| t.contains("Untitled")));
    app.press_key("mod+r")?;
    app.wait_for("text:/Run Succeeded/")?;
    if app.screen().shows("Show Console") {
        app.press("Show Console")?;
    }
    app.wait_for("Welcome to ScratchRunner!")?;
    Ok(())
}

#[test]
fn creates_tabs_and_opens_run_settings_and_find_bar() -> Result<(), String> {
    let (_env, mut app) = start_workbench();
    app.wait_for("Syntax: Shell Script")?;
    app.wait_for("Line 2, Column 33")?;

    // Open a second tab with Cmd+T and verify it gets numbered `Untitled 2.sh`.
    app.press_key("mod+t")?;
    app.wait_for("Untitled 2.sh")?;
    app.press_key("mod+1")?;

    // Open Run Settings popover with Shift+Cmd+R and verify inputs, then dismiss.
    app.press_key("mod+shift+r")?;
    app.wait_for("Arguments:")?;
    app.wait_for("Program Input:")?;
    app.press("Done")?;
    app.wait_for_gone("Program Input:", Duration::from_secs(2))?;

    // Open Find bar with Cmd+F.
    app.press_key("mod+f")?;
    app.wait_for("label:Search text")?;
    app.press_key("escape")?;
    Ok(())
}

#[test]
fn toggles_workspace_panes_and_opens_secondary_windows() -> Result<(), String> {
    let (_env, mut app) = start_workbench();
    app.wait_for("Syntax: Shell Script")?;

    // Toggle Documentation View (Shift+Cmd+I) on and verify footer renders.
    app.press_key("mod+shift+i")?;
    app.wait_for("Contents")?;
    app.press_key("mod+shift+i")?;
    app.wait_for_gone("Contents", Duration::from_secs(2))?;

    // File Navigator starts visible; switch to Search tab cleanly.
    app.wait_for("label:Search")?;
    app.press("label:Search")?;
    app.wait_for("Replace")?;

    // Open Preferences (Cmd+,) and Help (Shift+Cmd+/) cleanly without panicking.
    app.press_key("mod+,")?;
    app.wait_for("General")?;
    app.press_key("mod+shift+/")?;
    app.wait_for("Documentation Lookup")?;
    Ok(())
}
