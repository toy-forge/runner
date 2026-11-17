//! ScratchRunner's command layer: commands, menus, key bindings, the compound-command
//! interpreter, fuzzy matching and the command palette. Pure Rust, no UI dependency.
//!
//! # Main types
//!
//! * [`CommandId`] / [`CommandTable`] (`crate::command`): stable ids and the static metadata
//!   (title, menu path, default shortcuts, enable/check rules) of every command.
//! * [`CommandContext`]: the plain snapshot of application state from which
//!   [`CommandDef::state`] computes enabled / checked / visible / title.
//! * [`MenuModel`] (`crate::menu`): the menu bar and context menus as data, with alternates,
//!   dynamic placeholders, fragment injection and [`MenuModel::resolve`] for the UI.
//! * [`Shortcut`] / [`Stroke`] (`crate::shortcut`): CodeRunner (`@~^$`) and NSH
//!   (`mod+shift+x`) text forms, chords, Mac glyphs, NSH reserved-key validation.
//! * [`bindings::BindingSet`]: key-binding sets in `.crBindings`, JSON and TOML, the shipped
//!   `Default` / `Vim` / `_Global_` sets, the user's set directory and [`bindings::KeyMap`].
//! * [`compound`]: the compound-command parser and resumable interpreter, generic over
//!   [`compound::Host`]; [`compound::fake::FakeHost`] is the test host.
//! * [`fuzzy::fuzzy_match`] and [`palette::palette_items`]: QuickNav `mainMenu` search.
//!
//! # Example
//!
//! ```
//! use scratch_commands::bindings::{default_set, KeyMap};
//! use scratch_commands::{CommandContext, MenuModel, Scope};
//! use scratch_commands::menu::DefaultShortcuts;
//!
//! let menu = MenuModel::standard();
//! let set = default_set();
//! let km = KeyMap::build(&menu, Some(&set), None);
//! assert!(km.issues().is_empty());
//! assert!(km.conflicts().is_empty());
//!
//! let ctx = CommandContext::new();
//! let strokes = [scratch_commands::Stroke::parse_crbindings("@s").unwrap()];
//! assert!(km.lookup(Scope::Editor, &strokes).entry().is_some());
//!
//! let items = scratch_commands::palette::palette_items(
//!     &menu, &ctx, &DefaultShortcuts, "save", scratch_commands::palette::PaletteOptions::default());
//! assert!(items.iter().any(|i| i.title == "Save As…"));
//! ```
pub mod bindings;
pub mod command;
pub mod compound;
pub mod context;
pub mod fuzzy;
pub mod menu;
pub mod palette;
pub mod shortcut;
pub mod table;

pub use command::{CommandDef, CommandId, CommandKind, CommandTable, ItemState, Role, Scope};
pub use context::CommandContext;
pub use menu::MenuModel;
pub use shortcut::{Shortcut, Stroke};
pub use table::ids;
