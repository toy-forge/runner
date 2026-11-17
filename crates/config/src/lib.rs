//! ScratchRunner's storage layout and preferences.
//!
//! Main types:
//!
//! * [`Paths`]: where files live (`~/Library/Application Support/ScratchRunner`,
//!   `~/Library/Caches/ScratchRunner`), constructible from any directory for tests.
//! * [`write_atomic`]: whole-file writes that never leave a partial file.
//! * [`Preferences`]: the fully typed preference tree (`serde(default)`
//!   everywhere), one struct per Preferences pane plus hidden state.
//! * [`PrefSchema`] / [`PrefSpec`]: a static, data-driven table describing every
//!   preference (pane, category, label, help, control, default, parent, ...);
//!   the Preferences window is generated from it.
//! * [`Preferences::get`] / [`Preferences::set`] by id string, with validation,
//!   and [`Preferences::diff`] to find what changed.
//! * [`Loaded`]: loading with warnings; [`Preferences::save`] edits the existing
//!   `settings.toml` in place (comments and unknown keys survive) and writes
//!   only values that differ from the defaults (or are already in the file).
//! * [`import_from_cr_defaults`]: read-only import of CodeRunner's preferences.
//!
//! ```
//! use scratch_config::{PrefValue, Preferences, PrefSchema};
//!
//! let mut prefs = Preferences::default();
//! assert_eq!(prefs.text_editing.indentation.tab_width, 4);
//! let old = prefs.clone();
//! prefs.set("tab_width", PrefValue::Integer(8)).unwrap(); // bare key or full id
//! assert_eq!(Preferences::diff(&old, &prefs), vec!["text_editing.indentation.tab_width"]);
//! assert!(prefs.set("tab_width", PrefValue::Integer(5)).is_err()); // not an option
//! let spec = PrefSchema::get("text_editing.indentation.tab_width").unwrap();
//! assert_eq!(spec.options().len(), 5);
//! ```

mod access;
mod atomic;
mod cr_import;
mod io;
mod model;
mod paths;
mod schema;
mod schema_table;
#[cfg(test)]
mod tests;

pub use access::PrefError;
pub use atomic::{write_atomic, write_atomic_str};
pub use cr_import::{
    CR_BUNDLE_ID, ImportError, ImportReport, SkippedKey, cr_preferences_path, import_from_cr_defaults,
    read_cr_preferences,
};
pub use io::{
    LoadWarning, Loaded, Migration, SETTINGS_VERSION, SaveError, WarningKind, migrate_document,
};
pub use model::*;
pub use paths::{APPLICATION_SUPPORT_PLACEHOLDER, Paths, expand_tilde};
pub use schema::{
    ConstValue, ControlKind, Gate, KeyValueTableSpec, Layout, Pane, PopupOption, PrefId,
    PrefSchema, PrefSpec, RESTART_ALERT_MESSAGE, RESTART_ALERT_TITLE,
};
pub use schema_table::FILE_ENCODINGS;

/// The value of a preference: a TOML value (boolean, integer, float, string,
/// array or table).
pub use toml::Value as PrefValue;
