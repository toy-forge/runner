//! The static preference table behind [`crate::PrefSchema`].
//!
//! Labels, help texts and option titles are CodeRunner 4.6's, verbatim (docs/
//! research/03-preferences.md). Ids follow the struct tree of
//! [`crate::Preferences`]; `cr_key` is the CodeRunner defaults key.

use crate::schema::{
    ConstValue as V, ControlKind, Gate, KeyValueTableSpec, Layout, Pane, PopupOption as O,
    PrefSpec,
};

// ---------------------------------------------------------------- builders

impl PrefSpec {
    const fn new(id: &'static str, cr: &'static str, label: Option<&'static str>, control: ControlKind, default: V) -> Self {
        let mut spec = PrefSpec::BASE;
        spec.id = id;
        spec.label = label;
        spec.control = control;
        spec.default = default;
        spec.cr_key = if cr.is_empty() { None } else { Some(cr) };
        spec
    }

    const fn help(mut self, text: &'static str) -> Self {
        self.help = Some(text);
        self
    }

    /// Nests under `parent` without enabling or disabling it.
    const fn nested(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Nests under `parent`; enabled while the parent boolean is on.
    const fn gated(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self.gate = Gate::WhenOn;
        self
    }

    /// Nests under `parent`; enabled while the parent boolean is off.
    const fn gated_off(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self.gate = Gate::WhenOff;
        self
    }

    const fn layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }

    const fn restart(mut self) -> Self {
        self.requires_restart = true;
        self
    }

    const fn editor(mut self) -> Self {
        self.per_editor = true;
        self
    }

    const fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }

    const fn no_ui(mut self) -> Self {
        self.in_ui = false;
        self
    }

    const fn no_import(mut self) -> Self {
        self.import = false;
        self
    }
}

const fn checkbox(id: &'static str, cr: &'static str, label: &'static str, default: bool) -> PrefSpec {
    PrefSpec::new(id, cr, Some(label), ControlKind::Checkbox, V::Bool(default))
}

const fn hidden_bool(id: &'static str, cr: &'static str, default: bool) -> PrefSpec {
    PrefSpec::new(id, cr, None, ControlKind::Checkbox, V::Bool(default)).no_ui()
}

const fn popup(
    id: &'static str,
    cr: &'static str,
    label: Option<&'static str>,
    options: &'static [O],
    default: V,
) -> PrefSpec {
    PrefSpec::new(id, cr, label, ControlKind::Popup(options), default)
}

const fn radio(id: &'static str, cr: &'static str, options: &'static [O], default: V) -> PrefSpec {
    PrefSpec::new(id, cr, None, ControlKind::Radio(options), default)
}

const fn dynamic_popup(
    id: &'static str,
    cr: &'static str,
    label: Option<&'static str>,
    source: &'static str,
    default: &'static str,
) -> PrefSpec {
    PrefSpec::new(id, cr, label, ControlKind::DynamicPopup { source }, V::Str(default))
}

const fn number_f(
    id: &'static str,
    cr: &'static str,
    label: Option<&'static str>,
    default: f64,
    min: f64,
    max: f64,
    step: f64,
) -> PrefSpec {
    PrefSpec::new(
        id,
        cr,
        label,
        ControlKind::Number { min, max, step, integer: false },
        V::Float(default),
    )
}

const fn number_i(
    id: &'static str,
    cr: &'static str,
    label: Option<&'static str>,
    default: i64,
    min: i64,
    max: i64,
) -> PrefSpec {
    PrefSpec::new(
        id,
        cr,
        label,
        ControlKind::Number { min: min as f64, max: max as f64, step: 1.0, integer: true },
        V::Int(default),
    )
}

const fn text(id: &'static str, cr: &'static str, label: Option<&'static str>, default: &'static str) -> PrefSpec {
    PrefSpec::new(
        id,
        cr,
        label,
        ControlKind::Text { multiline: false, placeholder: None },
        V::Str(default),
    )
}

const fn custom(id: &'static str, cr: &'static str, kind: &'static str, default: V) -> PrefSpec {
    PrefSpec::new(id, cr, None, ControlKind::Custom(kind), default)
}

const fn button(id: &'static str, label: &'static str, action: &'static str) -> PrefSpec {
    PrefSpec::new(id, "", Some(label), ControlKind::Button { action }, V::Bool(false))
}

/// Stamps the pane and category on a group of specs.
const fn group<const N: usize>(
    pane: Pane,
    category: Option<&'static str>,
    mut specs: [PrefSpec; N],
) -> [PrefSpec; N] {
    let mut i = 0;
    while i < N {
        specs[i].pane = pane;
        specs[i].category = category;
        i += 1;
    }
    specs
}

// ------------------------------------------------------------ option lists

static OPT_INSERT_NEW_TABS: &[O] = &[
    O::with_cr(V::Str("after_existing"), "After existing tabs", V::Int(0)),
    O::with_cr(V::Str("before_existing"), "Before existing tabs", V::Int(1)),
    O::with_cr(V::Str("after_selected"), "After the selected tab", V::Int(2)),
];

static OPT_CONSOLE_POSITION: &[O] = &[
    O::with_cr(V::Str("bottom"), "Bottom", V::Int(0)),
    O::with_cr(V::Str("right"), "Right", V::Int(1)),
];

static OPT_SOFT_TABS: &[O] = &[
    O::new(V::Bool(false), "Tabs"),
    O::new(V::Bool(true), "Spaces"),
];

static OPT_TAB_WIDTH: &[O] = &[
    O::new(V::Int(2), "2"),
    O::new(V::Int(3), "3"),
    O::new(V::Int(4), "4"),
    O::new(V::Int(6), "6"),
    O::new(V::Int(8), "8"),
];

static OPT_INVISIBLES_MODE: &[O] = &[
    O::with_cr(V::Str("everywhere"), "Everywhere", V::Int(0)),
    O::with_cr(V::Str("beginning_and_end"), "At beginning and end of lines", V::Int(1)),
    O::with_cr(V::Str("nonconforming_indentation"), "Nonconforming indentation", V::Int(2)),
    O::SEPARATOR,
    O::with_cr(V::Str("selection"), "In selected text", V::Int(4)),
    O::with_cr(
        V::Str("selection_and_nonconforming_indentation"),
        "In selected text + nonconforming indentation",
        V::Int(6),
    ),
];

static OPT_INSERTION_STYLE: &[O] = &[
    O::new(V::Str("bar"), "Bar"),
    O::new(V::Str("block"), "Block"),
    O::new(V::Str("underline"), "Underline"),
];

static OPT_ICON_ONLY: &[O] = &[
    O::new(V::Bool(false), "Full diagnostic messages"),
    O::new(V::Bool(true), "Icon only"),
];

static OPT_THEME_RADIO: &[O] = &[
    O::new(V::Bool(false), "Use the chosen theme for all appearances"),
    O::new(V::Bool(true), "Use separate themes for different appearances:"),
];

static OPT_PATH_RADIO: &[O] = &[
    O::new(V::Bool(true), "Automatically determine PATH environment variable"),
    O::new(V::Bool(false), "Use custom PATH:"),
];

static OPT_SORT_MODE: &[O] = &[
    O::with_cr(V::Str("name"), "Name", V::Int(0)),
    O::with_cr(V::Str("natural"), "Natural Order", V::Int(1)),
    O::with_cr(V::Str("reverse_natural"), "Reverse Natural Order", V::Int(2)),
];

/// The file encodings of the Default File Encoding popup: the stored label,
/// the menu title, and CodeRunner's `NSStringEncoding` value.
pub static FILE_ENCODINGS: &[O] = &[
    O::with_cr(V::Str("utf-8"), "Unicode (UTF-8)", V::Int(4)),
    O::with_cr(V::Str("utf-16"), "Unicode (UTF-16)", V::Int(10)),
    O::with_cr(V::Str("utf-16be"), "Unicode (UTF-16 Big-Endian)", V::Int(0x9000_0100)),
    O::with_cr(V::Str("utf-16le"), "Unicode (UTF-16 Little-Endian)", V::Int(0x9400_0100)),
    O::with_cr(V::Str("utf-32"), "Unicode (UTF-32)", V::Int(0x8c00_0100)),
    O::with_cr(V::Str("utf-32be"), "Unicode (UTF-32 Big-Endian)", V::Int(0x9800_0100)),
    O::with_cr(V::Str("utf-32le"), "Unicode (UTF-32 Little-Endian)", V::Int(0x9c00_0100)),
    O::with_cr(V::Str("us-ascii"), "Western (ASCII)", V::Int(1)),
    O::with_cr(V::Str("iso-8859-1"), "Western (ISO Latin 1)", V::Int(5)),
    O::with_cr(V::Str("macintosh"), "Western (Mac OS Roman)", V::Int(30)),
    O::with_cr(V::Str("windows-1252"), "Western (Windows Latin 1)", V::Int(12)),
    O::with_cr(V::Str("iso-8859-2"), "Central European (ISO Latin 2)", V::Int(9)),
    O::with_cr(V::Str("windows-1250"), "Central European (Windows Latin 2)", V::Int(15)),
    O::with_cr(V::Str("windows-1251"), "Cyrillic (Windows)", V::Int(11)),
    O::with_cr(V::Str("koi8-r"), "Cyrillic (KOI8-R)", V::Int(0x8000_0a02)),
    O::with_cr(V::Str("windows-1253"), "Greek (Windows)", V::Int(13)),
    O::with_cr(V::Str("windows-1254"), "Turkish (Windows Latin 5)", V::Int(14)),
    O::with_cr(V::Str("euc-jp"), "Japanese (EUC)", V::Int(3)),
    O::with_cr(V::Str("shift_jis"), "Japanese (Shift JIS)", V::Int(8)),
    O::with_cr(V::Str("iso-2022-jp"), "Japanese (ISO 2022-JP)", V::Int(21)),
    O::with_cr(V::Str("gb18030"), "Chinese (GB 18030)", V::Int(0x8000_0632)),
    O::with_cr(V::Str("big5"), "Chinese (Big 5)", V::Int(0x8000_0a03)),
    O::with_cr(V::Str("euc-kr"), "Korean (EUC)", V::Int(0x8000_0940)),
];

static ENVIRONMENT_TABLE: KeyValueTableSpec = KeyValueTableSpec {
    key_title: "Variable",
    value_title: "Value",
    add_tooltip: "Add a shell variable",
    remove_tooltip: "Delete the selected shell variable",
    reset_tooltip: "Reset all shell variables",
    reset_title: "Reset",
};

// ------------------------------------------------------------------ General

static GENERAL_DOCUMENTS: &[PrefSpec] = &group(Pane::General, Some("Documents"), [
    checkbox("general.documents.autosave", "Autosave", "Enable Auto Save and Versions", true),
    checkbox("general.documents.show_all_extensions", "AppleShowAllExtensions", "Show all filename extensions", true),
    checkbox("general.documents.ask_to_save_when_quitting", "AskToSaveWhenQuitting", "Ask to save untitled documents when quitting", false).restart(),
    checkbox("general.documents.use_templates_for_new_files", "OpenPresetsEnabled", "Use templates for new files", true),
    checkbox("general.documents.allow_suggested_filenames", "AllowSuggestedFilenames", "Allow filename suggestions from macOS", false).restart(),
]);

static GENERAL_TABS: &[PrefSpec] = &group(Pane::General, Some("Tabs"), [
    checkbox("general.tabs.prefer_tabs", "PreferTabs", "Prefer opening new documents in tabs", true),
    checkbox("general.tabs.tab_bar_hidden", "TabBarHidden", "Hide tab bar by default", false),
    popup("general.tabs.insert_new_tabs", "InsertNewTabs", Some("Insert New Tabs"), OPT_INSERT_NEW_TABS, V::Str("after_existing")),
]);

static GENERAL_CONSOLE: &[PrefSpec] = &group(Pane::General, Some("Console"), [
    popup("general.console.position", "ConsolePosition", Some("Console Position"), OPT_CONSOLE_POSITION, V::Str("bottom")),
    checkbox("general.console.hide_automatically", "HideConsoleAutomatically", "Hide console automatically", true),
]);

static GENERAL_FILE_NAVIGATOR: &[PrefSpec] = &group(Pane::General, Some("File Navigator"), [
    checkbox("general.file_navigator.show_hidden_files", "ShowHiddenFiles", "Show hidden files and folders", false),
    checkbox("general.file_navigator.sort_folders_on_top", "SortFoldersOnTop", "Sort folders on top", false),
]);

static GENERAL_NOTIFICATIONS: &[PrefSpec] = &group(Pane::General, Some("Notifications"), [
    checkbox("general.notifications.enabled", "UserNotificationsEnabled", "Show notifications while the app is inactive", true),
]);

static GENERAL_OTHER: &[PrefSpec] = &group(Pane::General, None, [
    popup("general.documents.file_encoding", "FileEncoding", Some("Default File Encoding"), FILE_ENCODINGS, V::Str("utf-8")),
    button("general.action.edit_file_type_associations", "Edit File Type Associations...", "edit_file_type_associations"),
    custom("general.added_file_type_association_extensions", "AddedFileTypeAssociationExtensions", "", V::StrList(&[])).no_ui(),
]);

// CodeRunner updates itself with Sparkle; ScratchRunner has no updater, so
// these keep their place in the model but have no control.
static GENERAL_UPDATES: &[PrefSpec] = &group(Pane::General, Some("Updates"), [
    checkbox("general.updates.check_automatically", "SUEnableAutomaticChecks", "Check for updates automatically", true).no_ui().no_import(),
    checkbox("general.updates.automatically_download", "SUAutomaticallyUpdate", "Automatically download updates when available", false).gated("general.updates.check_automatically").no_ui().no_import(),
    button("general.action.check_for_updates", "Check For Updates Now...", "check_for_updates").no_ui(),
    checkbox("general.updates.send_profile_info", "SUSendProfileInfo", "Send anonymous system profile information", true).no_ui().no_import(),
    checkbox("general.updates.check_for_beta_updates", "CheckForBetaUpdates", "Allow automatic updates to beta versions", false).help("If disabled, you can still find beta versions by doing a manual check for updates.").no_ui().no_import(),
    number_i("general.updates.scheduled_check_interval", "SUScheduledCheckInterval", None, 172_800, 0, 31_536_000).no_ui().no_import(),
]);

// ------------------------------------------------------------ Text Editing

static TEXT_APPEARANCE: &[PrefSpec] = &group(Pane::TextEditing, Some("Appearance"), [
    PrefSpec::new("text_editing.appearance.font.family", "Font", Some("Font"), ControlKind::Font { size_id: "text_editing.appearance.font.size" }, V::Str("Menlo")).editor(),
    button("text_editing.action.choose_font", "Choose Font...", "order_front_font_panel").nested("text_editing.appearance.font.family"),
    number_f("text_editing.appearance.font.size", "", None, 12.0, 1.0, 400.0, 1.0).no_ui().editor(),
    checkbox("text_editing.appearance.font.ligatures", "FontLigatures", "Ligatures", false).nested("text_editing.appearance.font.family").editor(),
    checkbox("text_editing.appearance.font.antialiasing", "FontAntialiasing", "Antialiasing", true).nested("text_editing.appearance.font.family").editor(),
    checkbox("text_editing.appearance.font.smoothing", "FontSmoothing", "Smoothing", true).nested("text_editing.appearance.font.family").editor(),
    checkbox("text_editing.appearance.syntax_highlighting", "SyntaxHighlightingEnabled", "Syntax highlighting", true).editor(),
    number_f("text_editing.appearance.line_height", "LineHeight", Some("Line Spacing"), 1.2, 0.0, 100.0, 0.05).editor(),
    checkbox("text_editing.appearance.scroll_markers", "ScrollMarkersEnabled", "Show navigational markers in scroller", true).editor(),
    checkbox("text_editing.appearance.scroll_markers_always_visible", "ScrollMarkersAlwaysVisible", "Keep markers visible when scroller is hidden", true).gated("text_editing.appearance.scroll_markers"),
    checkbox("text_editing.appearance.wrap_lines", "WrapLinesEnabled", "Wrap lines", true).editor(),
    checkbox("text_editing.appearance.allow_scroll_past_end", "AllowScrollPastEnd", "Allow scrolling past end of document", true).editor(),
    checkbox("text_editing.appearance.page_guide.show", "ShowPageGuide", "Show page guide at column", false).editor(),
    number_i("text_editing.appearance.page_guide.column", "PageGuide", None, 80, 1, 1000).gated("text_editing.appearance.page_guide.show").layout(Layout::SameRowAsParent).editor(),
    checkbox("text_editing.appearance.invisibles.show", "ShowInvisibles", "Show invisible characters: %@", true).section("Invisibles").editor(),
    text("text_editing.appearance.invisibles.characters", "InvisibleCharacters", None, "\\t \\n\\r\u{a0}").gated("text_editing.appearance.invisibles.show").layout(Layout::InlineInParentLabel).editor(),
    popup("text_editing.appearance.invisibles.mode", "InvisiblesMode", None, OPT_INVISIBLES_MODE, V::Str("selection_and_nonconforming_indentation")).gated("text_editing.appearance.invisibles.show").layout(Layout::SameRowAsParent).section("Invisibles").editor(),
    popup("text_editing.appearance.insertion_point.style", "InsertionPointStyle", Some("Insertion Point"), OPT_INSERTION_STYLE, V::Str("bar")).section("Insertion Point").editor(),
    checkbox("text_editing.appearance.insertion_point.blink", "BlinkInsertionPoint", "Blink", true).nested("text_editing.appearance.insertion_point.style").section("Insertion Point").editor(),
    checkbox("text_editing.appearance.insertion_point.animate", "AnimateInsertionPoint", "Animate", true).gated("text_editing.appearance.insertion_point.blink").section("Insertion Point").editor(),
    checkbox("text_editing.appearance.insertion_point.visible_with_selection", "InsertionPointVisibleWithSelection", "Visible with selection", true).nested("text_editing.appearance.insertion_point.style").section("Insertion Point").editor(),
    popup("text_editing.appearance.alternate_insertion_point.style", "InsertionPointStyleAlternate", Some("Alternate Insertion Point"), OPT_INSERTION_STYLE, V::Str("block")).section("Insertion Point").editor(),
    checkbox("text_editing.appearance.alternate_insertion_point.blink", "BlinkInsertionPointAlternate", "Blink", true).nested("text_editing.appearance.alternate_insertion_point.style").section("Insertion Point").editor(),
    checkbox("text_editing.appearance.alternate_insertion_point.animate", "AnimateInsertionPointAlternate", "Animate", true).gated("text_editing.appearance.alternate_insertion_point.blink").section("Insertion Point").editor(),
    checkbox("text_editing.appearance.alternate_insertion_point.visible_with_selection", "InsertionPointVisibleWithSelectionAlternate", "Visible with selection", true).nested("text_editing.appearance.alternate_insertion_point.style").section("Insertion Point").editor(),
    checkbox("text_editing.appearance.highlight_current_line", "HighlightCurrentLine", "Highlight current line", true).editor(),
    checkbox("text_editing.appearance.line_numbers.enabled", "LineNumbersEnabled", "Show line numbers", true).editor(),
    checkbox("text_editing.appearance.line_numbers.relative", "RelativeLineNumbers", "Relative to selection", false).gated("text_editing.appearance.line_numbers.enabled").editor(),
    checkbox("text_editing.appearance.line_numbers.draw_separator", "DrawLineNumberSeparator", "Draw separator at left margin", true).editor(),
]);

static TEXT_INDENTATION: &[PrefSpec] = &group(Pane::TextEditing, Some("Indentation"), [
    popup("text_editing.indentation.soft_tabs", "DefaultTabModeSoftTabs", Some("Indent Using"), OPT_SOFT_TABS, V::Bool(false)).help("Indentation can be customized on a per-language basis under the Languages tab."),
    popup("text_editing.indentation.tab_width", "DefaultTabModeWidth", None, OPT_TAB_WIDTH, V::Int(4)).nested("text_editing.indentation.soft_tabs").layout(Layout::SameRowAsParent),
    checkbox("text_editing.indentation.indent_wrapped_lines", "IndentWrappedLines", "Indent wrapped lines %@ indent levels", true).editor(),
    number_f("text_editing.indentation.indent_wrapped_lines_level", "IndentWrappedLinesLevel", None, 1.0, -1_000_000.0, 1_000_000.0, 0.5).gated("text_editing.indentation.indent_wrapped_lines").layout(Layout::InlineInParentLabel).editor(),
    checkbox("text_editing.indentation.spaces_indentation_single_unit", "SpacesIndentationSingleUnit", "Treat consecutive spaces indentation as single unit", true).editor(),
    checkbox("text_editing.indentation.auto_indent_while_typing", "AutoIndentWhileTyping", "Auto-Indent while typing", true).section("Auto-Indent").editor(),
    checkbox("text_editing.indentation.auto_indent_on_paste", "AutoIndentOnPaste", "Auto-Indent pasted code", true).section("Auto-Indent").editor(),
]);

static TEXT_BEHAVIOR: &[PrefSpec] = &group(Pane::TextEditing, Some("Behavior"), [
    checkbox("text_editing.behavior.character_pairs.close_enabled", "CloseBracketsEnabled", "Automatically close character pairs", true).section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_curly_brackets", "CloseCurlyBrackets", "{", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_parens", "CloseParens", "(", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_square_brackets", "CloseSquareBrackets", "[", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_less_than", "CloseLessThan", "<", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_double_quote", "CloseDoubleQuote", "\"", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_single_quote", "CloseSingleQuote", "'", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.close_backtick", "CloseBacktick", "`", true).gated("text_editing.behavior.character_pairs.close_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_enabled", "JumpPastBracketsEnabled", "Jump past ending character when matching key pressed", true).section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_curly_brackets", "JumpPastCurlyBrackets", "}", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_parens", "JumpPastParens", ")", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_square_brackets", "JumpPastSquareBrackets", "]", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_greater_than", "JumpPastGreaterThan", ">", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_double_quote", "JumpPastDoubleQuote", "\"", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_single_quote", "JumpPastSingleQuote", "'", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.jump_past_backtick", "JumpPastBacktick", "`", true).gated("text_editing.behavior.character_pairs.jump_past_enabled").section("Character Pairs").editor(),
    checkbox("text_editing.behavior.character_pairs.highlight_closing_brackets", "HighlightClosingBrackets", "Automatically highlight balancing characters", true).section("Character Pairs").editor(),
    checkbox("text_editing.behavior.word_selection_includes_variable_chars", "IncludeVariableCharsInWordSelectionGranularity", "Double-click on word also selects relevant special chars", true).section("Selection").editor(),
    checkbox("text_editing.behavior.tokenizers.enabled", "TokenizersEnabled", "Enable code tokenizers", true).section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.clang", "ClangTokenizerEnabled", "Clang", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.php", "PHPTokenizerEnabled", "PHP", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.python", "PythonTokenizerEnabled", "Python", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.shell", "ShellTokenizerEnabled", "Shell", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.swift", "SwiftTokenizerEnabled", "Swift", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.generic_source", "GenericSourceTokenizerEnabled", "Generic Source Code", true).gated("text_editing.behavior.tokenizers.enabled").section("Tokenizers").editor(),
    checkbox("text_editing.behavior.tokenizers.highlight_tokens", "HighlightTokens", "Underline tokens at insertion point", true).section("Tokenizers").editor(),
    number_i("text_editing.behavior.kill_ring_size", "NSTextKillRingSize", Some("Kill-Ring Size"), 7, 0, 1000).help("The kill-ring is similar to the pasteboard. Text that is \"killed\" (default key binding ^K) is added to the buffer and can be inserted again later (\"yanked\", default key binding ^Y).").editor(),
    checkbox("text_editing.behavior.spelling.check_while_typing", "CheckSpellingWhileTyping", "Check spelling while typing", true).section("Spelling").editor(),
    checkbox("text_editing.behavior.spelling.check_grammar", "CheckGrammarWithSpelling", "Check grammar with spelling", false).gated("text_editing.behavior.spelling.check_while_typing").section("Spelling").editor(),
    checkbox("text_editing.behavior.spelling.correct_automatically", "CorrectSpellingAutomatically", "Correct spelling automatically", false).gated("text_editing.behavior.spelling.check_while_typing").section("Spelling").editor(),
    custom("text_editing.behavior.multiple_selections_event_masks", "MultipleSelectionsEventMasks", "", V::IntList(&[393_216, 1_048_576, 524_288])).no_ui().editor(),
    hidden_bool("text_editing.behavior.non_contiguous_layout", "NonContiguousLayout", true),
    hidden_bool("text_editing.behavior.break_lines_at_any_character", "BreakLinesAtAnyCharacter", false).editor(),
    custom("text_editing.behavior.comment_characters_by_scope", "CommentCharactersByScope", "", V::Toml("{}")).no_ui(),
    custom("text_editing.behavior.text_finder_user_patterns", "TextFinderUserPatterns", "", V::StrList(&[])).no_ui(),
]);

static TEXT_COMPLETION: &[PrefSpec] = &group(Pane::TextEditing, Some("Completion"), [
    checkbox("text_editing.completion.enabled", "CompletionsEnabled", "Suggest completions while typing", true),
    number_i("text_editing.completion.word_length", "CodeCompletionWordLength", Some("Display completions after typing %@ characters"), 2, 0, 100).gated("text_editing.completion.enabled").layout(Layout::InlineInOwnLabel),
    checkbox("text_editing.completion.fuzzy_limit", "CodeCompletionFuzzyLimit", "Ignore with only low-quality results", true).gated("text_editing.completion.enabled"),
    checkbox("text_editing.completion.fuzzy", "CodeCompletionFuzzy", "Fuzzy search", true),
    checkbox("text_editing.completion.fuzzy_enforce_character_order", "CodeCompletionFuzzyEnforceCharacterOrder", "Force correct character order in matches", false).gated("text_editing.completion.fuzzy"),
    checkbox("text_editing.completion.hits_enabled", "CodeCompletionHitsEnabled", "Prioritize previously chosen completions", true),
    checkbox("text_editing.completion.word_completions", "WordCompletionsEnabled", "Use word-completions for unsupported languages", true),
    checkbox("text_editing.completion.short_string_completions", "ShortStringCompletionsEnabled", "Enable short string completions", true),
    checkbox("text_editing.completion.automatically_insert_imports", "AutomaticallyInsertImportStatements", "Automatically insert import statements", true),
    checkbox("text_editing.completion.animate_panel", "AnimateCompletionPanel", "Animate completion panel", true),
]);

static TEXT_DIAGNOSTICS: &[PrefSpec] = &group(Pane::TextEditing, Some("Diagnostics"), [
    checkbox("text_editing.diagnostics.while_typing", "DiagnosticsWhileTyping", "Generate diagnostics while typing", true).section("Behavior"),
    number_f("text_editing.diagnostics.delay", "DiagnosticsDelay", Some("Delay %@ seconds"), 1.5, 0.0, 3600.0, 0.5).gated("text_editing.diagnostics.while_typing").layout(Layout::InlineInOwnLabel).section("Behavior"),
    checkbox("text_editing.diagnostics.invalid_syntax", "DiagnosticsInvalidSyntax", "Show errors derived from syntax definitions", true).section("Behavior").editor(),
    checkbox("text_editing.diagnostics.invalid_syntax_limit_to_one", "DiagnosticsInvalidSyntaxLimitToOne", "Only show first syntax error in document", true).gated("text_editing.diagnostics.invalid_syntax").section("Behavior").editor(),
    checkbox("text_editing.diagnostics.from_console", "DiagnosticsFromConsole", "Include errors from the console in the editor", true).section("Behavior"),
    checkbox("text_editing.diagnostics.animate", "DiagnosticsAnimate", "Animate diagnostic messages", true).section("Appearance").editor(),
    checkbox("text_editing.diagnostics.highlight_line", "DiagnosticsHighlightLine", "Highlight lines with diagnostics", true).section("Appearance").editor(),
    checkbox("text_editing.diagnostics.underline", "DiagnosticsUnderline", "Underline text associated with diagnostics", true).section("Appearance").editor(),
    checkbox("text_editing.diagnostics.symbol_list_includes_annotations", "SymbolListIncludesEditorAnnotations", "Include diagnostics in symbol navigator", true).section("Appearance").editor(),
    popup("text_editing.diagnostics.icon_only", "DiagnosticsIconOnly", Some("Show"), OPT_ICON_ONLY, V::Bool(false)).section("Appearance").editor(),
    PrefSpec::new("text_editing.diagnostics.auto_fix_diagnostics", "AutoFixDiagnostics", Some("Configure..."), ControlKind::EditButton { action: "edit_automatic_fixes" }, V::Str("")).section("Auto-Fixes"),
    PrefSpec::new("text_editing.diagnostics.ignored_diagnostics", "IgnoredDiagnostics", Some("Configure..."), ControlKind::EditButton { action: "edit_ignored_diagnostics" }, V::Str("")).section("Ignored Messages"),
    custom("text_editing.diagnostics.update_overrides", "CRUpdateModes", "", V::Toml("{}")).no_ui().no_import(),
    custom("text_editing.diagnostics.disabled_diagnostics", "DisabledDiagnostics", "", V::Toml("[]")).no_ui().no_import(),
    custom("text_editing.diagnostics.console_highlight_only", "ConsoleHighlightOnly", "", V::Toml("[]")).no_ui().no_import(),
]);

// --------------------------------------------------------------- Languages

static LANGUAGES: &[PrefSpec] = &group(Pane::Languages, Some("Docs"), [
    custom("languages.docs.global_entries", "GlobalDocs", "docs_table", V::Toml(DEFAULT_GLOBAL_DOCS))
        .help("Below you can add online documentation resources for this language. Use {query} in the URL to represent the search string."),
    checkbox("languages.docs.use_preferred_global_docs", "UsePreferredGlobalDocsUUID", "When invoked from the editor, always use:", false),
    dynamic_popup("languages.docs.preferred_global_docs_id", "PreferredGlobalDocsUUID", None, "global_docs", "02787B2F-25D7-48D7-802D-14C9226EEE3A").gated("languages.docs.use_preferred_global_docs").layout(Layout::SameRowAsParent),
    custom("languages.docs.state", "DocsState", "", V::Toml(DEFAULT_DOCS_STATE)).no_ui(),
    hidden_bool("languages.docs.autolookup_enabled", "AutolookupEnabled", false),
    hidden_bool("languages.docs.suppress_404_alert", "Suppress404Alert", false),
    text("languages.current_language", "CurrentLanguageUUID", None, "").no_ui().no_import(),
    number_i("languages.bundle_version", "LanguagesBundleVersion", None, 0, 0, 4_294_967_295).no_ui().no_import(),
]);

const DEFAULT_GLOBAL_DOCS: &str = r#"[
  { id = "DEE58E80-8F40-4CC3-8CF4-A6CB3839F18C", name = "Google", kind = "search", url = "https://www.google.com/search?q={query}+{lang}#rso", post = "", enabled = true },
  { id = "65FBFE31-7A8E-48BD-8CE3-49164CB25EB3", name = "Stack Overflow", kind = "search", url = "https://stackoverflow.com/search?q=%5B{lang}%5D+{query}", post = "", enabled = true },
  { id = "8FEA0EBE-873A-4991-8E6A-6D494D9DB050", name = "ChatGPT", kind = "bookmark", url = "https://chatgpt.com", post = "", enabled = true },
  { id = "507F15C9-03D5-4DF2-96E3-FF19DDD50168", name = "Claude", kind = "bookmark", url = "https://claude.ai", post = "", enabled = true },
  { id = "02787B2F-25D7-48D7-802D-14C9226EEE3A", name = "Dash", kind = "search", url = "dash://{lang}:{query}", post = "", enabled = false },
]"#;

const DEFAULT_DOCS_STATE: &str = r#"{
  "FF667E76-BF73-4C18-AD70-EABAC0861D92" = { transparency = true, expects_redirect_from_search_results = false },
  "69B84E29-B4B4-423E-B816-7E6818E845F3" = { transparency = true, expects_redirect_from_search_results = false },
  "A8506643-4CED-4DAD-8691-34283E1C9A59" = { transparency = true, expects_redirect_from_search_results = false },
  "6984DD13-CCF5-43D7-B6CF-993EE0235A86" = { transparency = true, expects_redirect_from_search_results = false },
}"#;

// ------------------------------------------------------------------ Themes

static THEMES: &[PrefSpec] = &group(Pane::Themes, None, [
    radio("themes.use_separate_color_themes", "UseSeparateColorThemes", OPT_THEME_RADIO, V::Bool(true)),
    custom("themes.color_theme", "ColorTheme", "theme_list", V::Str("ScratchRunner Dark")),
    dynamic_popup("themes.color_theme_light", "ColorThemeLight", Some("Light:"), "themes.light", "ScratchRunner Light").gated("themes.use_separate_color_themes"),
    dynamic_popup("themes.color_theme_dark", "ColorThemeDark", Some("Dark:"), "themes.dark", "ScratchRunner Dark").gated("themes.use_separate_color_themes"),
    hidden_bool("themes.match_appearance_to_theme", "MatchAppearanceToTheme", false),
]);

// ------------------------------------------------------------ Key Bindings

static KEY_BINDINGS: &[PrefSpec] = &group(Pane::KeyBindings, None, [
    dynamic_popup("key_bindings.set", "KeyBindingSet", Some("Key Bindings Set:"), "key_binding_sets", "Default"),
    hidden_bool("key_bindings.compound_commands_enabled", "CompoundCommandsEnabled", true),
]);

// ------------------------------------------------------------------- Shell

static SHELL: &[PrefSpec] = &group(Pane::Shell, None, [
    radio("shell.automatically_determine_path", "AutomaticallyDeterminePATH", OPT_PATH_RADIO, V::Bool(true)),
    button("shell.action.refresh_path", "Refresh PATH", "refresh_path_variable").gated("shell.automatically_determine_path").layout(Layout::SameRowAsParent).help("Refresh PATH"),
    PrefSpec::new("shell.path", "PATH", None, ControlKind::Text { multiline: true, placeholder: None }, V::Str("/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")).gated_off("shell.automatically_determine_path"),
    checkbox("shell.run_as_login_shell", "RunAsLoginShell", "Invoke shell in login mode when running code", false).help("When invoked in login mode, the shell executes startup scripts such as ~/.profile."),
    PrefSpec::new("shell.shell", "Shell", Some("Shell:"), ControlKind::Text { multiline: false, placeholder: Some("/path/to/shell") }, V::Str("/bin/bash")).help("Using a shell other than bash is not recommended and may cause undesirable behavior."),
    PrefSpec::new("shell.environment", "Environment", Some("Shell Variables:"), ControlKind::KeyValueTable(&ENVIRONMENT_TABLE), V::Toml(DEFAULT_ENVIRONMENT)),
    custom("shell.arguments", "ShellArguments", "", V::StrList(&[])).no_ui(),
    custom("shell.compile_arguments", "CompileShellArguments", "", V::StrList(&[])).no_ui(),
]);

const DEFAULT_ENVIRONMENT: &str =
    r#"{ TERM = "dumb", TERM_PROGRAM = "ScratchRunner", LC_CTYPE = "UTF-8" }"#;

// ---------------------------------------------------------------- Advanced

static ADVANCED_GENERAL: &[PrefSpec] = &group(Pane::Advanced, None, [
    checkbox("advanced.selection_label_uses_columns", "SelectionLabelUsesColumns", "Use columns in selection label", true),
    checkbox("advanced.resolve_relative_file_urls", "ResolveRelativeFileURLs", "Automatically resolve relative file URLs in webpages", true),
    checkbox("advanced.debugger_hide_termination_message", "DebuggerHideTerminationMessage", "Hide debugger termination message", true),
    checkbox("advanced.replace_untitled_document", "ReplaceUntitledDocument", "Close new unedited document when opening another", true),
    checkbox("advanced.write_compiler_output_to_temp_dir", "WriteCompilerOutputFilesToTemporaryDirectory", "Write compiler output files to temporary directory", true),
    checkbox("advanced.group_languages", "GroupLanguages", "Group similar languages in menus", true),
    checkbox("advanced.custom_icons_enabled", "CustomIconsEnabled", "Use custom file icons", true),
    checkbox("advanced.three_finger_lookup_enabled", "ThreeFingerLookupEnabled", "Enable three-finger tap in editor", true).editor(),
    text("advanced.extra_swift_flags", "ExtraSwiftFlags", Some("Extra Swift Flags"), "-sdk macosx"),
    text("advanced.docs_user_agent", "DocsUserAgent", None, "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1").no_ui(),
    PrefSpec::new("advanced.autosave_documents_directory", "AutosaveDocumentsDirectory", None, ControlKind::Path { directory: true }, V::Str("$(APPLICATION_SUPPORT)/ScratchRunner/Unsaved")).no_ui().no_import(),
]);

static ADVANCED_CLANG: &[PrefSpec] = &group(Pane::Advanced, Some("Clang completion and diagnostics"), [
    checkbox("advanced.clang.automatic_flags", "AutomaticClangFlags", "Automatically determine appropriate system compile flags", true),
    checkbox("advanced.clang.flags_from_user", "ClangFlagsFromUser", "Apply user-defined compile flags", true),
    text("advanced.clang.extra_flags", "ExtraClangFlags", Some("Extra Clang Flags"), "-Wno-unused-command-line-argument"),
]);

static ADVANCED_DEBUGGING: &[PrefSpec] = &group(Pane::Advanced, Some("Debugging"), [
    checkbox("advanced.debugging.center_during_stepping", "CenterDuringStepping", "Center current location during stepping", false),
]);

// ------------------------------------------------- Hidden (no pane at all)

static HIDDEN_DEBUGGER: &[PrefSpec] = &group(Pane::Hidden, None, [
    PrefSpec::new("debugger.variables_sort_mode", "DebuggerVariablesSortMode", Some("Sort By"), ControlKind::Popup(OPT_SORT_MODE), V::Str("natural")).no_ui(),
    custom("debugger.variables_display_types", "DebuggerVariablesDisplayTypes", "", V::Toml("{ pdb = false, xdebug = true, gdb = true, lldb = true }")).no_ui(),
    custom("debugger.variables_show_all", "DebuggerVariablesShowAll", "", V::Toml("{}")).no_ui(),
    hidden_bool("debugger.hover_variables", "HoverVariables", true),
    hidden_bool("debugger.hide_view_automatically", "AutomaticallyHideDebuggerView", true),
    hidden_bool("debugger.verbose", "VerboseDebugger", false),
]);

static HIDDEN_PRINTING: &[PrefSpec] = &group(Pane::Hidden, None, [
    hidden_bool("printing.with_colors", "PrintWithColors", true),
    hidden_bool("printing.with_line_numbers", "PrintWithLineNumbers", true),
    number_i("printing.font_size", "PrintFontSize", None, 8, 1, 200).no_ui(),
    number_f("printing.line_height", "PrintLineHeight", None, 1.2, 0.5, 10.0, 0.05).no_ui(),
    number_i("printing.top_margin", "PrintTopMargin", None, 40, 0, 1000).no_ui(),
    number_i("printing.bottom_margin", "PrintBottomMargin", None, 40, 0, 1000).no_ui(),
    number_i("printing.left_margin", "PrintLeftMargin", None, 40, 0, 1000).no_ui(),
    number_i("printing.right_margin", "PrintRightMargin", None, 40, 0, 1000).no_ui(),
]);

static HIDDEN_UI: &[PrefSpec] = &group(Pane::Hidden, None, [
    hidden_bool("ui.sidebar_visible", "SidebarVisible", true),
    hidden_bool("ui.titlebar_hidden", "TitlebarHidden", false),
    PrefSpec::new("ui.file_view_directory", "FileViewDirectory", None, ControlKind::Path { directory: true }, V::Str("~/")).no_ui(),
    text("ui.go_to_folder_path", "GoToFolderPath", None, "").no_ui(),
    text("ui.search_excluded_directories", "SearchExcludedDirectories", None, "").no_ui(),
    hidden_bool("ui.pdf_view_auto_scales", "PDFViewAutoScales", true),
    number_f("ui.pdf_view_scale_factor", "PDFViewScaleFactor", None, 1.0, 0.05, 20.0, 0.1).no_ui(),
    text("ui.selected_preferences_pane", "NKPreferencesSelectedIdentifier", None, "General").no_ui().no_import(),
    hidden_bool("ui.overlay_scrollers_fallback_for_accessory_views", "NSOverlayScrollersFallBackForAccessoryViews", false),
    hidden_bool("ui.web_inspector.developer_extras", "WebKitDeveloperExtras", true),
    hidden_bool("ui.web_inspector.starts_attached", "__WebInspectorPageGroupLevel1__.WebKit2InspectorStartsAttached", true),
    number_i("ui.web_inspector.attachment_side", "__WebInspectorPageGroupLevel1__.WebKit2InspectorAttachmentSide", None, 1, 0, 2).no_ui(),
]);

static HIDDEN_MISC: &[PrefSpec] = &group(Pane::Hidden, None, [
    custom("misc.suppressed_alerts", "SuppressedAlerts", "", V::Toml("{}")).no_ui().no_import(),
    number_i("misc.launches", "launches", None, 0, 0, 4_294_967_295).no_ui().no_import(),
    text("misc.last_launch_date", "LastLaunchDate", None, "").no_ui().no_import(),
    hidden_bool("misc.set_as_default_editor", "SetAsDefaultEditor", false),
    hidden_bool("misc.set_as_default_editor_suppressed", "SetAsDefaultEditorSuppressed", false),
    custom("misc.diagnostics_installation_info", "DiagnosticsInstallationInfo", "", V::Toml("{}")).no_ui().no_import(),
]);

/// All groups in display order.
pub(crate) static GROUPS: &[&[PrefSpec]] = &[
    GENERAL_DOCUMENTS,
    GENERAL_TABS,
    GENERAL_CONSOLE,
    GENERAL_FILE_NAVIGATOR,
    GENERAL_NOTIFICATIONS,
    GENERAL_OTHER,
    GENERAL_UPDATES,
    TEXT_APPEARANCE,
    TEXT_INDENTATION,
    TEXT_BEHAVIOR,
    TEXT_COMPLETION,
    TEXT_DIAGNOSTICS,
    LANGUAGES,
    THEMES,
    KEY_BINDINGS,
    SHELL,
    ADVANCED_GENERAL,
    ADVANCED_CLANG,
    ADVANCED_DEBUGGING,
    HIDDEN_DEBUGGER,
    HIDDEN_PRINTING,
    HIDDEN_UI,
    HIDDEN_MISC,
];
