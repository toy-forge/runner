//! Headless tests of the navigator: keyboard model, clicks, context menu, rename, drop.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use nsh_core::platform::{Environment, Idiom, Os};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use scratch_navigator::{
    model::{FsSource, SYNTH_ROOT, SynthSource},
    ui::harness::{CONFIG, Harness},
};

static SERIAL: Mutex<()> = Mutex::new(());

fn start(source: Arc<dyn scratch_navigator::model::DirSource>, root: &Path) -> Headless {
    *CONFIG.lock().unwrap() = Some((source, root.to_path_buf()));
    let mut config = EngineConfig::new(Root::of::<Harness>());
    config.viewport = nsh_core::geometry::Size::new(
        nsh_core::geometry::Points(900.0),
        nsh_core::geometry::Points(600.0),
    );
    let env = Environment { os: Os::MacOs, idiom: Idiom::Desktop, ..Default::default() };
    config.environment = env;
    let mut app = Headless::start(config).expect("engine starts");
    app.wait_for("text:rows:20")
        .or_else(|_| app.wait_for("text:rows:6"))
        .ok();
    app
}

fn texts(app: &Headless) -> Vec<String> {
    app.screen().texts().iter().map(|s| s.to_string()).collect()
}

fn has(app: &Headless, t: &str) -> bool {
    texts(app).iter().any(|s| s == t)
}

#[test]
fn mounts_synthetic_rows_and_lazy_expands() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = start(Arc::new(SynthSource::nested_50k()), Path::new(SYNTH_ROOT));
    assert!(has(&app, "rows:20"), "{:?}", texts(&app));
    app.press("text:module_00").unwrap();
    assert!(has(&app, "sel:1") && has(&app, "cursor:module_00"));
    // Right arrow expands the cursor folder (children load on a worker thread).
    app.press_key("right").unwrap();
    app.wait_for("text:rows:45").unwrap();
    app.press_key("left").unwrap();
    app.wait_for("text:rows:20").unwrap();
}

#[test]
fn finder_like_keyboard_model() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = start(Arc::new(SynthSource::nested_50k()), Path::new(SYNTH_ROOT));
    app.press("text:module_00").unwrap();
    app.press_key("down").unwrap();
    assert!(has(&app, "cursor:module_01"), "{:?}", texts(&app));
    app.press_key("shift+down").unwrap();
    assert!(has(&app, "sel:2"));
    app.press_key("up").unwrap();
    assert!(has(&app, "cursor:module_01") && has(&app, "sel:1"));
    // Type to filter, Backspace edits, Escape clears.
    for k in ["l", "e", "shift+-", "0", "3"] {
        app.press_key(k).unwrap();
    }
    assert!(has(&app, "filter:le_03"), "{:?}", texts(&app));
    assert!(has(&app, "rows:1"));
    app.press_key("backspace").unwrap();
    assert!(has(&app, "filter:le_0"));
    app.press_key("escape").unwrap();
    assert!(has(&app, "filter:"));
    assert!(has(&app, "rows:20"));
    // Cmd+Down on a folder re-roots into it; Cmd+Up goes back to the parent.
    app.press("text:module_02").unwrap();
    app.press_key("mod+down").unwrap();
    app.wait_for("text:rows:25").unwrap();
    app.press_key("mod+up").unwrap();
    app.wait_for("text:rows:20").unwrap();
}

#[test]
fn return_renames_in_place_and_escape_cancels() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "").unwrap();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:alpha.txt").unwrap();
    app.press("text:alpha.txt").unwrap();
    app.press_key("enter").unwrap();
    eprintln!("after enter: {:?}", texts(&app));
    assert!(has(&app, "editing:true"));
    app.press_key("escape").unwrap();
    assert!(has(&app, "editing:false"));
    app.press_key("enter").unwrap();
    assert!(has(&app, "editing:true"));
    app.clear("id:rename-input").unwrap();
    app.type_text("id:rename-input", "gamma.txt").unwrap();
    app.submit("id:rename-input").unwrap();
    app.wait_for("text:gamma.txt").unwrap();
    assert!(dir.path().join("gamma.txt").exists() && !dir.path().join("alpha.txt").exists());
    assert!(has(&app, "editing:false"));
    // The rename input is gone and the key bindings are live again (Down moves the cursor).
    app.press_key("down").unwrap();
    assert!(has(&app, "sel:1"), "{:?}", texts(&app));
    // A name that exists asks Replace / Don't Replace; Don't Replace keeps both files.
    app.on_dialog(|_| nsh_testing::DialogAnswer::Press("Don't Replace".into()));
    app.press("text:gamma.txt").unwrap();
    app.press_key("enter").unwrap();
    app.clear("id:rename-input").unwrap();
    app.type_text("id:rename-input", "beta.txt").unwrap();
    app.submit("id:rename-input").unwrap();
    app.wait_for("text:gamma.txt").unwrap();
    assert!(dir.path().join("gamma.txt").exists() && dir.path().join("beta.txt").exists());
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for n in ["a.txt", "b.rs", ".hidden"] {
        std::fs::write(dir.path().join(n), "x").unwrap();
    }
    std::fs::create_dir(dir.path().join("dest")).unwrap();
    std::fs::write(dir.path().join("dest/inner.md"), "x").unwrap();
    dir
}

fn labels(app: &Headless) -> Vec<String> {
    app.menu()
        .unwrap_or_default()
        .iter()
        .map(|e| format!("{e:?}"))
        .collect()
}

#[test]
fn context_menu_has_the_codrunner_item_set_and_actions_work() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:a.txt").unwrap();
    app.right_click("text:a.txt").unwrap();
    let menu = labels(&app).join("|");
    eprintln!("MENU: {menu}");
    for item in [
        "Open",
        "Open with Finder",
        "New File",
        "New Folder",
        "Go to Folder",
        "Refresh",
        "Copy",
        "Paste",
        "Rename",
        "Duplicate",
        "Delete",
        "View Options",
        "Show in Finder",
        "Copy Path",
    ] {
        assert!(menu.contains(item), "{item} missing in {menu}");
    }
    app.choose_menu_item("Duplicate").unwrap();
    app.wait_for("text:a copy.txt").unwrap();
    // View Options > Show Hidden Files
    assert!(!texts(&app).iter().any(|t| t == ".hidden"));
    app.right_click("text:b.rs").unwrap();
    app.choose_menu_item("View Options > Show Hidden Files")
        .unwrap();
    app.wait_for("text:.hidden").unwrap();
    // Sort Folders on Top reorders: dest first.
    app.right_click("text:b.rs").unwrap();
    app.choose_menu_item("View Options > Sort Folders on Top")
        .unwrap();
    let t = texts(&app);
    let pos = |n: &str| t.iter().position(|x| x == n).unwrap();
    assert!(
        pos("dest") < pos(".hidden") && pos("dest") < pos("a.txt"),
        "{t:?}"
    );
}

#[test]
fn files_dropped_from_finder_are_copied_into_the_row_under_the_pointer() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture();
    let outside = tempfile::tempdir().unwrap();
    let dropped = outside.path().join("from finder.txt");
    std::fs::write(&dropped, "hi").unwrap();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:dest").unwrap();
    let url = format!(
        "file://{}",
        dropped.display().to_string().replace(' ', "%20")
    );
    app.drop_files("text:dest", vec![url.clone()]).unwrap();
    app.wait_for_within(
        "text:status:Copied 1 item(s) into dest",
        std::time::Duration::from_secs(3),
    )
    .unwrap();
    assert!(dir.path().join("dest/from finder.txt").exists());
    assert!(dropped.exists(), "a drop from Finder copies");
    // Dropped on a file row: lands in that file's folder (the root here).
    app.drop_files("text:b.rs", vec![url]).unwrap();
    app.wait_for("text:from finder.txt").unwrap();
    assert!(dir.path().join("from finder.txt").exists());
}

#[test]
fn new_folder_row_trash_and_fs_watching() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:a.txt").unwrap();
    app.press("text:a.txt").unwrap();
    app.press_key("mod+shift+n").unwrap();
    assert!(has(&app, "editing:true"));
    app.clear("id:rename-input").unwrap();
    app.type_text("id:rename-input", "fresh").unwrap();
    app.submit("id:rename-input").unwrap();
    app.wait_for("text:fresh").unwrap();
    assert!(dir.path().join("fresh").is_dir());
    // An external change shows up through FSEvents without any user action.
    std::fs::write(dir.path().join("external.txt"), "x").unwrap();
    app.wait_for_within("text:external.txt", std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        texts(&app)
            .iter()
            .any(|t| t.starts_with("batches:") && t != "batches:0")
    );
    // Cmd+Backspace confirms, then trashes.
    app.on_dialog(|d| {
        eprintln!("DIALOG: {d:?}");
        nsh_testing::DialogAnswer::Press("Move to Trash".into())
    });
    app.press("text:external.txt").unwrap();
    app.press_key("mod+backspace").unwrap();
    // Headless has no trash; reaching `move_to_trash` after the confirmation is what we can check.
    app.wait_for_within(
        "text:status:Trash failed: this platform has no file dialogs, file manager or trash",
        std::time::Duration::from_secs(3),
    )
    .unwrap();
}

#[test]
fn rows_drag_into_folders_in_app() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:dest").unwrap();
    // Rows are 24 pt: a.txt, b.rs, dest -> dragging a.txt down two rows lands on dest.
    app.drag_and_hold(
        "text:a.txt",
        nsh_core::geometry::point(nsh_core::geometry::px(0.0), nsh_core::geometry::px(48.0)),
    )
    .unwrap();
    eprintln!("mid drag: {:?}", texts(&app));
    assert!(has(&app, "drag:a.txt"), "{:?}", texts(&app));
    app.drop_held().unwrap();
    app.wait_for_within("text:status:", std::time::Duration::from_secs(2))
        .ok();
    eprintln!("after drop: {:?}", texts(&app));
    assert!(dir.path().join("dest/a.txt").exists(), "{:?}", texts(&app));
    assert!(!dir.path().join("a.txt").exists());
}

#[test]
fn history_buttons_and_path_root() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture();
    let mut app = start(Arc::new(FsSource), dir.path());
    app.wait_for("text:dest").unwrap();
    app.press("text:dest").unwrap();
    app.press_key("mod+down").unwrap();
    app.wait_for("text:inner.md").unwrap();
    assert!(
        texts(&app)
            .iter()
            .any(|t| t.ends_with("/dest") && t.starts_with("root:"))
    );
    app.press("label:Back").unwrap();
    app.wait_for("text:a.txt").unwrap();
    app.press("label:Forward").unwrap();
    app.wait_for("text:inner.md").unwrap();
}
