//! The navigator's pure model: no NSH types, fast to unit-test.
//!
//! * [`entry`]: [`Entry`], [`DirSource`] ([`FsSource`], [`SynthSource`]).
//! * [`tree`]: [`FileTree`] (lazy loading, expanded set, filter, flattening to [`Row`]s).
//! * [`select`]: [`Selection`], [`NavCommand`] and the key table.
//! * [`sort`]: natural name ordering and [`ViewOptions`].
//! * [`history`]: root history and path popup chain.
//! * [`fsops`]: create, rename, duplicate, move, copy.

pub mod entry;
pub mod fsops;
pub mod history;
pub mod select;
pub mod sort;
pub mod tree;

pub use entry::{
    DirSource, Entry, EntryId, EntryRef, FsSource, Kind, SYNTH_ROOT, SynthSource, path_id,
};
pub use history::{PathChoice, RootHistory, ancestor_chain, display_name};
pub use select::{ClickMods, NavCommand, Selection, edit_filter, key_bindings};
pub use sort::ViewOptions;
pub use tree::{Disclosure, Draft, Expand, FileTree, Row};
