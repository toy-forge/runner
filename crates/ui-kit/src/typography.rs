//! Typography of DESIGN.md section 4: five text steps and the code font.
//!
//! The interface font is the system font; no family is named for it. Code
//! uses [`CODE_FONT_STACK`] (`SF Mono, Menlo, monospace`), which NSH resolves
//! family by family. The steps apply size, line height and weight through
//! [`Styled`] setters; color is the component's decision (`caption` is
//! usually [`crate::tokens::muted_foreground`]).
//!
//! ```ignore
//! use scratch_ui_kit::prelude::*;
//! use nsh_core::prelude::*;
//!
//! type_body().apply(text("Run succeeded"));
//! type_mono().apply(text("fn main() {}"));
//! ```

use nsh_core::prelude::*;

/// The code font stack: SF Mono, else Menlo, else the system monospaced
/// font. NSH resolves the first installed family and cascades the rest.
pub const CODE_FONT_STACK: &str = "SF Mono, Menlo, monospace";

/// One typographic step: a size, a line height, a weight and an optional
/// family, applied together by [`TypeStyle::apply`].
#[derive(Clone, Copy, Debug)]
pub struct TypeStyle {
    size: fn() -> f32,
    leading: fn() -> f32,
    weight: FontWeight,
    family: Option<&'static str>,
}

impl TypeStyle {
    /// The step's font size in points.
    pub fn size(self) -> f32 {
        (self.size)()
    }

    /// The step's line height in points.
    pub fn line_height(self) -> f32 {
        (self.leading)()
    }

    /// The step's font weight.
    pub fn weight(self) -> FontWeight {
        self.weight
    }

    /// The step's font family (`None`: the system font).
    pub fn family(self) -> Option<&'static str> {
        self.family
    }

    /// The same step in [`FontWeight::MEDIUM`].
    pub fn medium(self) -> Self {
        Self {
            weight: FontWeight::MEDIUM,
            ..self
        }
    }

    /// The same step in [`FontWeight::SEMIBOLD`].
    pub fn semibold(self) -> Self {
        Self {
            weight: FontWeight::SEMIBOLD,
            ..self
        }
    }

    /// The same step in [`FontWeight::NORMAL`].
    pub fn regular(self) -> Self {
        Self {
            weight: FontWeight::NORMAL,
            ..self
        }
    }

    /// The same step with its size read by `size` (a token accessor: pass
    /// `crate::tokens::type_body`, not its result).
    pub fn with_size(self, size: fn() -> f32) -> Self {
        Self { size, ..self }
    }

    /// The same step with its line height read by `leading`.
    pub fn with_leading(self, leading: fn() -> f32) -> Self {
        Self { leading, ..self }
    }

    /// The same step in `family` (a single family name or a comma list that
    /// NSH resolves family by family).
    pub fn with_family(self, family: &'static str) -> Self {
        Self {
            family: Some(family),
            ..self
        }
    }

    /// The step applied to `element`: its text size, line height, weight
    /// and, when it has one, its family. Bindings read the size tokens
    /// when they run, so a changed token re-sizes the text.
    pub fn apply<S: Styled>(self, element: S) -> S {
        let size = self.size;
        let leading = self.leading;
        let element = element
            .text_size(move || px(size()))
            .line_height(move || px(leading()))
            .font_weight(self.weight);
        match self.family {
            Some(family) => element.font_family(family),
            None => element,
        }
    }
}

/// The `display` step: 22 / 28, semibold. The welcome heading only.
pub fn type_display() -> TypeStyle {
    TypeStyle {
        size: crate::tokens::type_display,
        leading: crate::tokens::leading_display,
        weight: FontWeight::SEMIBOLD,
        family: None,
    }
}

/// The `title` step: 17 / 24, semibold. Empty-state headings and About.
pub fn type_title() -> TypeStyle {
    TypeStyle {
        size: crate::tokens::type_title,
        leading: crate::tokens::leading_title,
        weight: FontWeight::SEMIBOLD,
        family: None,
    }
}

/// The `subhead` step: 15 / 20, semibold. Sheet and pane headings.
pub fn type_subhead() -> TypeStyle {
    TypeStyle {
        size: crate::tokens::type_subhead,
        leading: crate::tokens::leading_subhead,
        weight: FontWeight::SEMIBOLD,
        family: None,
    }
}

/// The `body` step: 13 / 18, regular. All interface text.
pub fn type_body() -> TypeStyle {
    TypeStyle {
        size: crate::tokens::type_body,
        leading: crate::tokens::leading_body,
        weight: FontWeight::NORMAL,
        family: None,
    }
}

/// The `caption` step: 12 / 16, regular. Status bars, badges, helper text.
pub fn type_caption() -> TypeStyle {
    TypeStyle {
        size: crate::tokens::type_caption,
        leading: crate::tokens::leading_caption,
        weight: FontWeight::NORMAL,
        family: None,
    }
}

/// The code step: the `body` size and line height in [`CODE_FONT_STACK`].
pub fn type_mono() -> TypeStyle {
    type_body().with_family(CODE_FONT_STACK)
}

/// The caption sized code step: 12 / 16 in [`CODE_FONT_STACK`], for console
/// text and inline paths.
pub fn type_code_caption() -> TypeStyle {
    type_caption().with_family(CODE_FONT_STACK)
}
