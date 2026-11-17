//! ScratchRunner: a native macOS scratchpad for writing and running code.

#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    env_logger::init();
    // The same theme `nsh.json` names, so `cargo run` looks like the packaged app.
    let (theme, _) = nsh_base::Theme::from_json_lenient(include_str!("../ui/theme.json")).expect("ui/theme.json is valid");
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<scratchrunner::ScratchRunner>().tokens(theme.into_tokens())),
        platform::AppOptions {
            title: "ScratchRunner".to_owned(),
            app_id: "com.tbxark.scratchrunner".to_owned(),
            size: (960, 640),
            // The icon set, for `cargo run` only: a packaged app gets it from the registry.
            asset_base: Some(platform::AssetBase::Directory(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dev-assets"),
            )),
            data_dir: std::env::var_os("SCRATCHRUNNER_DATA_DIR")
                .map(std::path::PathBuf::from),
            ..Default::default()
        },
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ScratchRunner runs on macOS only.");
}
