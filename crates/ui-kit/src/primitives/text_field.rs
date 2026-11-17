//! `TextField`, `TextArea` and `CodeField`: DESIGN.md 7.6.
//!
//! A text field is `field` filled, outlined in `input`, `radius` 8, with
//! the focus ring outside; invalid adds a `destructive` outline and a
//! caption, read-only uses `muted`, disabled is 45%. A `CodeField` wraps
//! `nsh_base::editor()` with min/max rows, the code font and the
//! editor-scope colors.

use nsh_base::input;
use nsh_core::prelude::*;

use super::ControlSize;
use crate::motion;
use crate::tokens;
use crate::typography::{CODE_FONT_STACK, type_caption, type_code_caption};

/// A [`text_field`]'s size and states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFieldOptions {
    /// The size (default [`ControlSize::Md`]; prompts use `lg`).
    pub size: ControlSize,
    /// A bad value: `destructive` outline and caption.
    pub invalid: bool,
    /// Selectable but not editable: `muted` fill.
    pub readonly: bool,
    /// No typing; 45% opacity.
    pub disabled: bool,
    /// Draw in the code font at caption size (paths, scopes, flags).
    pub mono: bool,
    /// What screen readers call the field (usually the FormRow's label).
    pub accessibility_label: Option<&'static str>,
}

impl TextFieldOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `size`.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Invalid or not.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Read-only or not.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Disabled or not.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// In the code font or not.
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// With an accessible name.
    pub fn accessibility_label(mut self, label: &'static str) -> Self {
        self.accessibility_label = Some(label);
        self
    }
}

/// Whether a text field is a single line ([`text_field`]) or several
/// ([`text_area`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFieldVariant {
    /// One line at the control height.
    #[default]
    SingleLine,
    /// At least three rows (80pt), growing with the text.
    MultiLine,
}

/// A text field. `value` is controlled and `on_change` receives every
/// edit; `error` (shown under the field) states why an invalid value is
/// rejected.
pub fn text_field(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    on_change: impl Fn(&str) + 'static,
    options: TextFieldOptions,
    error: Option<SharedString>,
) -> impl IntoElement {
    field(
        placeholder,
        value,
        on_change,
        options,
        error,
        TextFieldVariant::SingleLine,
    )
}

/// A three-row text area with the same chrome as [`text_field`].
pub fn text_area(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    on_change: impl Fn(&str) + 'static,
    options: TextFieldOptions,
    error: Option<SharedString>,
) -> impl IntoElement {
    field(
        placeholder,
        value,
        on_change,
        options,
        error,
        TextFieldVariant::MultiLine,
    )
}

fn field(
    placeholder: impl IntoReactive<SharedString> + 'static,
    value: impl IntoReactive<SharedString> + 'static,
    on_change: impl Fn(&str) + 'static,
    options: TextFieldOptions,
    error: Option<SharedString>,
    variant: TextFieldVariant,
) -> impl IntoElement {
    let size = options.size;
    let multiline = variant == TextFieldVariant::MultiLine;
    let text_size = if options.mono {
        tokens::type_caption()
    } else {
        size.text_size()
    };
    let line_height = if options.mono || size == ControlSize::Xs {
        tokens::leading_caption()
    } else {
        tokens::leading_body()
    };
    let value = value.into_reactive();
    let placeholder = placeholder.into_reactive();
    let mut input_element = input()
        .value(move || value.get().to_string())
        .on_change(move |text: &str| on_change(text))
        .placeholder(move || placeholder.get().to_string())
        .placeholder_color(move || color(tokens::muted_foreground()))
        .caret_color(move || color(tokens::primary()))
        .selection_color(move || color(tokens::selection()))
        .readonly(options.readonly)
        .disabled(options.disabled)
        .multiline(multiline)
        .w_full()
        .px(px(8.0))
        .text_size(px(text_size))
        .line_height(px(line_height))
        .text_color(tokens::foreground)
        .bg(move || {
            if options.readonly {
                tokens::muted()
            } else {
                tokens::field()
            }
        })
        .border_1()
        .border_color(move || {
            if options.invalid {
                tokens::destructive()
            } else {
                tokens::input()
            }
        })
        .rounded(px(tokens::radius()))
        .focus(|s| {
            s.outline(px(2.0))
                .outline_color(tokens::ring)
                .outline_offset(px(1.0))
        });
    input_element = if multiline {
        input_element.min_h(px(80.0))
    } else {
        input_element.h(px(size.height()))
    };
    if options.mono {
        input_element = input_element.font_family(CODE_FONT_STACK);
    }
    let mut root = view()
        .flex_col()
        .gap(px(4.0))
        .w_full()
        .child(input_element);
    if let Some(label) = options.accessibility_label {
        root = root.label(label);
    }
    if let Some(error) = error {
        root = root.child(
            type_caption()
                .apply(text(error))
                .text_color(tokens::destructive),
        );
    }
    let root = disabled_look!(root.when(options.disabled, |v| {
        v.opacity(motion::OPACITY_DISABLED)
    }));
    focus_ring!(root)
}

/// A [`code_field`]'s chrome and editor settings.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeFieldOptions {
    /// The language for highlighting and indentation (`"shell"`, `"json"`).
    pub language: SharedString,
    /// The rows it starts at (default 2).
    pub min_rows: u32,
    /// The rows it grows to before scrolling (default 8).
    pub max_rows: u32,
    /// The line-number gutter (default on).
    pub line_numbers: bool,
    /// Selectable but not editable.
    pub readonly: bool,
    /// A bad value: `destructive` outline.
    pub invalid: bool,
    /// What screen readers call the field.
    pub accessibility_label: Option<&'static str>,
    /// Shown while the text is empty.
    pub placeholder: Option<SharedString>,
}

impl Default for CodeFieldOptions {
    fn default() -> Self {
        Self {
            language: SharedString::from("plaintext"),
            min_rows: 2,
            max_rows: 8,
            line_numbers: true,
            readonly: false,
            invalid: false,
            accessibility_label: None,
            placeholder: None,
        }
    }
}

impl CodeFieldOptions {
    /// A field for `language`.
    pub fn new(language: impl Into<SharedString>) -> Self {
        Self {
            language: language.into(),
            ..Self::default()
        }
    }

    /// The rows it starts at.
    pub fn min_rows(mut self, rows: u32) -> Self {
        self.min_rows = rows;
        self
    }

    /// The rows it grows to.
    pub fn max_rows(mut self, rows: u32) -> Self {
        self.max_rows = rows;
        self
    }

    /// The line-number gutter or not.
    pub fn line_numbers(mut self, line_numbers: bool) -> Self {
        self.line_numbers = line_numbers;
        self
    }

    /// Read-only or not.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Invalid or not.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// With an accessible name.
    pub fn accessibility_label(mut self, label: &'static str) -> Self {
        self.accessibility_label = Some(label);
        self
    }

    /// With a placeholder.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }
}

/// A small code editor (Run Command, Template, regex, PATH, JSON): the
/// editor grows from `min_rows` to `max_rows` and scrolls past that, in
/// `editor-bg` with the code font and `radius` 8.
pub fn code_field(
    value: impl IntoReactive<Option<SharedString>> + 'static,
    on_change: impl Fn(&SharedString) + 'static,
    options: CodeFieldOptions,
) -> impl IntoElement {
    let value = value.into_reactive();
    let mut editor = nsh_base::editor()
        .value(move || value.get())
        .on_change(move |text: &SharedString| on_change(text))
        .language(options.language.clone())
        .min_rows(options.min_rows)
        .max_rows(options.max_rows)
        .line_numbers(options.line_numbers)
        .read_only(options.readonly)
        .font_family(CODE_FONT_STACK)
        .font_size(px(tokens::editor_font_size()))
        .line_height(px(tokens::editor_line_height()))
        .soft_wrap(true)
        .w_full()
        .bg(tokens::editor_bg)
        .rounded(px(tokens::radius()))
        .border_1()
        .border_color(move || {
            if options.invalid {
                tokens::destructive()
            } else {
                tokens::input()
            }
        });
    if let Some(label) = options.accessibility_label {
        editor = editor.accessibility_label(label);
    }
    if let Some(placeholder) = options.placeholder {
        editor = editor.placeholder(placeholder);
    }
    let root = view().flex_col().w_full().overflow_hidden().child(editor);
    focus_ring!(root)
}

/// The caption-size code font for inline paths and chips.
pub fn code_caption(label: impl IntoReactive<SharedString> + 'static) -> impl IntoElement {
    type_code_caption().apply(
        text(label)
            .text_color(tokens::editor_fg)
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(tokens::radius_xs()))
            .bg(tokens::muted),
    )
}
