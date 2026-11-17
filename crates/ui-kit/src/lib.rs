//! ScratchRunner's styled component library over `nsh_base`, implementing
//! the foundation and primitives of `DESIGN.md` sections 2 to 10.
//!
//! The theme tokens live in `ui/theme.json`; no color, radius, font size or
//! duration is written literally. The modules read tokens ([`tokens`]) and
//! type steps ([`typography`]), use the motion recipes of [`motion`], and
//! keep internal spacing on the 4pt grid. Icons are the app's own SVG set
//! ([`icon`]).
//!
//! ```ignore
//! use scratch_ui_kit::prelude::*;
//! use nsh_core::prelude::*;
//!
//! let pressed = signal(false);
//! view().flex_row().gap(px(8.0))
//!     .child(button(
//!         "Run",
//!         ButtonOptions::new().variant(ButtonVariant::Primary).icon(IconName::Run),
//!         move || pressed.set(true),
//!     ))
//!     .child(icon_button(
//!         IconName::Stop,
//!         "Stop",
//!         IconButtonOptions::new().shortcut("⌘."),
//!         || {},
//!     ))
//!     .child(search_field(
//!         "Filter",
//!         "",
//!         None::<SharedString>,
//!         |_| {},
//!         || {},
//!         SearchFieldOptions::new(),
//!     ))
//! ```
//!
//! # Assets
//!
//! The 85 icons are copied to this crate's `assets/icons/`. A packaged app
//! registers them with its native module (`registry.assets(ASSETS)`); an app
//! run with `cargo run` sets `AppOptions::asset_base` to a directory holding
//! `@scratch-ui-kit/icons/...` (the gallery example does this). See
//! `README.md`.
//!
//! # Components
//!
//! [`primitives`]: `button`, `icon_button`, `capsule_group`, `segmented`,
//! `popup_button`, `select_popup`, `search_field`, `text_field`, `text_area`,
//! `code_field`, `checkbox`, `radio`, `radio_group`, `switch`, `stepper`,
//! `number_field`, `color_well`, `kbd`, `badge`, `section_header`,
//! `form_row`, `separator`, `spinner`, `progress_bar`, `split_handle` and
//! `tooltip`.

#![deny(missing_docs)]

pub mod icon;
pub mod motion;
pub mod primitives;
pub mod tokens;
pub mod typography;

use nsh_core::element::{AssetFile, Registry};

/// Every file of this crate's `assets/` directory (the icon set), for the
/// app's [`Registry::assets`].
pub const ASSETS: &[AssetFile] = nsh_core::element::assets!();

/// Registers this crate's assets with `registry`: the icon files `svg()`
/// loads through `asset:/@scratch-ui-kit/icons/...`.
pub fn register_assets(registry: &mut Registry) {
    registry.assets(ASSETS);
}

/// Everything the app's features use: the [`primitives`], [`icon`],
/// [`typography`], [`motion`], and the `nsh_core` prelude for views and
/// text. Theme tokens stay in [`tokens`] (importing every token function
/// would shadow `window()` and `input()`, for instance).
pub mod prelude {
    pub use nsh_core::prelude::*;

    pub use crate::icon::{
        GlyphTone, IconName, IconSize, Tone, glyph, glyph_at, glyph_tile, language_glyph_tile,
        language_tile,
    };
    pub use crate::motion;
    pub use crate::primitives::*;
    pub use crate::tokens as design_tokens;
    pub use crate::typography::{
        CODE_FONT_STACK, TypeStyle, type_body, type_caption, type_code_caption, type_display,
        type_mono, type_subhead, type_title,
    };
}
