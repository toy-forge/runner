//! ScratchRunner's colour themes. No UI dependency: everything here is plain
//! data and `Send` types, testable without a window.
//!
//! # Main types
//!
//! * [`Theme`]: a TextMate `.tmTheme` (global colours, scoped items, gutter
//!   settings, CodeRunner's console/ANSI items). [`Theme::from_plist`],
//!   [`Theme::to_plist_xml`], [`Theme::from_cr_theme`] (legacy `.crTheme`),
//!   [`Theme::inverted`], [`Theme::duplicate`], [`Theme::is_dark`],
//!   [`Theme::editor_colors`] (selection, caret, line highlight, gutter, ...)
//!   and [`Theme::chrome`] (sidebar/console/toolbar tints).
//! * [`ThemeLibrary`]: the 11 built-in themes plus the user directory, with
//!   the Themes tab operations (new, duplicate, rename, invert, delete/reset,
//!   import, export).
//! * [`ScopeSelector`]: the TextMate selector language with CodeRunner's
//!   extensions (`( )`, `|`, `,`, `&`, `-`, `>`, `*`, `**`, `$`, `=`).
//! * [`ResolvedTheme`]: a compiled theme; `style_for(scope stack)`.
//! * [`Highlighter`]: the incremental highlighting engine (syntect + the
//!   theme): `update_text`, `advance`, `highlight_lines`, `scopes_at`.
//!
//! # Example
//!
//! ```
//! use std::{sync::Arc, time::Duration};
//! use scratch_theme::{Highlighter, ThemeLibrary, DEFAULT_DARK};
//!
//! let library = ThemeLibrary::new(None);
//! let theme = library.get_or_default(DEFAULT_DARK, true);
//! let resolved = Arc::new(theme.compile());
//!
//! let mut highlighter = Highlighter::new("Rust", resolved);
//! highlighter.update_text("fn main() { let x = \"hi\"; }\n");
//! let range = highlighter.highlight_lines(0, 1, true, Duration::from_millis(50));
//! assert!(range.exact);
//! // Spans carry UTF-16 columns, ready for the editor's decorations.
//! let first = &range.lines[0].spans[0];
//! assert_eq!((first.col_start, first.col_end), (0, 2));
//! ```

mod builtin;
mod color;
mod error;
mod highlight;
mod library;
mod plist_io;
mod resolve;
mod selector;
mod syntax;
mod theme;

pub use builtin::{DEFAULT_DARK, DEFAULT_LIGHT, builtin_themes};
pub use color::Rgba;
pub use error::ThemeError;
pub use highlight::{
    CHECKPOINT_INTERVAL, GUESS_CONTEXT_LINES, HighlightedRange, Highlighter, LineSpans, MAX_LINE_BYTES, ScopeInfo,
    StyledSpan, TextChange,
};
pub use library::{ThemeEntry, ThemeLibrary};
pub use resolve::{MatchedItem, ResolvedTheme, TextStyle};
pub use selector::{Score, ScopeSelector, SelectorError, StackView};
pub use syntax::{find_syntax, is_plain, syntax_names, syntax_set};
pub use theme::{
    Chrome, EditorColors, FontStyle, GlobalSettings, GutterSettings, Theme, ThemeItem, ThemeOrigin,
};
