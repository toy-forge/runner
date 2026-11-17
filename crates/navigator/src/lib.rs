//! ScratchRunner's file navigator: a lazy folder tree model, FSEvents watching,
//! and the sidebar UI (header, rows, menus, footer) built on `scratch-ui-kit`.
//!
//! The model ([`model`]) is NSH-free; the UI ([`ui`]) centers on
//! [`ui::store::Store`], a `Copy` bag of signals created inside a component:
//!
//! ```ignore
//! let store = Store::new(FileTree::new(Arc::new(FsSource), root));
//! store.on_open(|path| { /* open in the editor */ });
//! store.start_watching();
//! view().child(chrome::header(store, on_search))
//!     .child(navigator(store))
//!     .child(chrome::footer(store))
//! ```

pub mod icons;
pub mod model;
pub mod ui;
pub mod watch;
