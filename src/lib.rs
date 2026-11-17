//! ScratchRunner's UI as a library, so the NSH toolchain can package it
//! (`nsh.json` names [`ScratchRunner`] as the root). `main.rs` runs the same
//! root directly for `cargo run`.

pub mod workbench;

use nsh_core::prelude::*;

pub use workbench::ScratchRunner;

/// The app's UI set (`nsh.json` `"rust"`): the root component and the icon assets.
#[native_module("scratchrunner/native")]
fn native(registry: &mut Registry) {
    registry.register::<ScratchRunner>();
    scratch_ui_kit::register_assets(registry);
}
