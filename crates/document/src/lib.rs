//! ScratchRunner's document, session and persistence layer (NSH-free).
//!
//! Everything about documents except their text (which lives in the editor)
//! and the UI that presents them. Languages come from [`scratch_language`],
//! paths and preferences from [`scratch_config`].
//!
//! # Main types
//!
//! | Area | Types and functions |
//! |--|--|
//! | Identity and model | [`DocumentId`], [`Document`] (title, path, language, encoding, line ending, dirty, run settings, breakpoints, view state), [`RunSettings`], [`Breakpoints`], [`ViewState`] ([`Selection`], [`Position`], [`FoldRange`]) |
//! | Encodings | [`TextEncoding`], [`detect_encoding`], [`decode`], [`encode`], [`reinterpret`], [`convert`], [`check_insert`], [`Unrepresentable`] |
//! | Line endings | [`LineEnding`], [`detect`](line_ending::detect) / [`normalize`](line_ending::normalize) / [`apply`](line_ending::apply) |
//! | File IO | [`load_text`] ([`ReadOptions`] -> [`LoadedFile`]), [`save_text`] ([`SaveOptions`] -> [`Saved`]), [`file_access`], [`run_location`], [`FileStamp`] |
//! | Per-file metadata | [`MetaStore`], [`FileMeta`], [`MetaKey`], [`CodeRunnerCompat`] (xattrs `ScratchRunner:*`, read-compat `CodeRunner:*`, JSON sidecar fallback) |
//! | Unsaved store | [`UnsavedStore`] (untitled documents as real files, recovery, cleanup), [`AutosaveScheduler`], [`DraftStore`], [`DraftRecord`] |
//! | Session | [`Session`], [`WindowState`], [`TabState`], [`Chrome`], [`SidebarState`], [`WindowDefaults`], [`clamp_to_screens`], [`cascade_frame`] |
//! | Files and names | [`rename`], [`move_to`], [`duplicate`], [`create_file`], [`trash_item`], [`next_untitled_name`], [`duplicate_name`], [`save_panel_defaults`], [`RecentDocuments`] |
//! | External changes | [`check_external`] -> [`ExternalState`] -> [`decide`] -> [`ExternalAction`]; [`WatchService`] emits [`FileChanged`] |
//! | Navigation | [`NavigationHistory`], [`NavigationEntry`] |
//! | Closing | [`plan_close`] -> [`ClosePlan`] -> [`CloseFlow`] -> [`CloseOutcome`] |
//!
//! # Example: open, edit, save
//!
//! ```
//! use scratch_document::*;
//! use scratch_language::LanguageRegistry;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = std::env::temp_dir().join(format!("scratch-doc-example-{}", std::process::id()));
//! std::fs::create_dir_all(&dir)?;
//! let path = dir.join("hello.py");
//! std::fs::write(&path, "print('hi')\r\n")?;
//!
//! // Open: bytes -> text (always `\n` in memory) + the file's state.
//! let registry = LanguageRegistry::builtin();
//! let loaded = load_text(&path, &ReadOptions::default())?;
//! let meta = MetaStore::new().read(&path);
//! let mut doc = Document::from_file(&path, &loaded, &meta, &registry, now_ms());
//! assert_eq!(doc.language, "python");
//! assert_eq!(doc.line_ending, LineEnding::CrLf);
//!
//! // Edit and save: encoding, BOM and line endings come back as they were.
//! doc.mark_edited(now_ms());
//! let saved = save_text(&path, "print('bye')\n", &doc.save_options())?;
//! doc.mark_saved(&saved, now_ms());
//! assert_eq!(std::fs::read(&path)?, b"print('bye')\r\n");
//!
//! // Did somebody else change it? Silent reload when clean, conflict when dirty.
//! let state = check_external(&path, &doc.stamp.unwrap())?;
//! assert_eq!(decide(state, doc.dirty), ExternalAction::Nothing);
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok(()) }
//! ```
//!
//! # Conventions
//!
//! * Text in memory uses `\n`; the original line ending is a property of the
//!   [`Document`] and is restored on save.
//! * Positions are NSH's: lines and columns from 0, columns in UTF-16 units.
//! * Times are milliseconds since the Unix epoch ([`now_ms`]); scheduling code
//!   takes the current time as an argument so it can be tested.
//! * Nothing here blocks on the UI thread except plain file IO; the watcher
//!   runs its own thread and reports on a channel.

mod closing;
mod document;
mod draft;
mod encoding;
mod external;
mod fileio;
mod fileops;
mod history;
mod id;
pub mod line_ending;
mod meta;
mod naming;
mod recent;
mod session;
mod state;
mod unsaved;
mod watch;

pub use closing::*;
pub use document::*;
pub use draft::*;
pub use encoding::*;
pub use external::*;
pub use fileio::*;
pub use fileops::*;
pub use history::*;
pub use id::DocumentId;
pub use line_ending::{LineEnding, LineEndingInfo};
pub use meta::*;
pub use naming::*;
pub use recent::*;
pub use session::*;
pub use state::*;
pub use unsaved::*;
pub use watch::*;
