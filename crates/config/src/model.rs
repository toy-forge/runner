//! The typed preference tree.
//!
//! Every struct is `#[serde(default)]`: a missing key takes its default, so a
//! `settings.toml` only has to hold what the user changed. The TOML layout is
//! `[pane.category]` tables with snake_case keys; the dotted path of a key is
//! its preference id (see [`crate::PrefSchema`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Default theme names: ScratchRunner's bundled pair (`scratch-theme`'s
/// `DEFAULT_LIGHT` / `DEFAULT_DARK`). CodeRunner's `CodeRunner` and
/// `CodeRunner Light` are mapped onto them by the import.
pub mod defaults {
    /// Theme used when "all appearances" share one theme (`ColorTheme`).
    pub const THEME: &str = "ScratchRunner Dark";
    /// Theme for the light appearance (`ColorThemeLight`).
    pub const THEME_LIGHT: &str = "ScratchRunner Light";
    /// Theme for the dark appearance (`ColorThemeDark`).
    pub const THEME_DARK: &str = "ScratchRunner Dark";
    /// Editor font family. CodeRunner sets this in code (the binary mentions
    /// `Menlo`).
    pub const FONT_FAMILY: &str = "Menlo";
    /// Editor font size in points (set in code by CodeRunner; unverified).
    pub const FONT_SIZE: f64 = 12.0;
    /// Name of the key-binding set used by default.
    pub const KEY_BINDING_SET: &str = "Default";
    /// Name of the always-active set of global compound commands.
    pub const GLOBAL_KEY_BINDING_SET: &str = "_Global_";
    /// The shell used to run code.
    pub const SHELL: &str = "/bin/bash";
    /// `PATH` used when it is not determined automatically.
    pub const PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    /// Where untitled documents live, with the `$(APPLICATION_SUPPORT)`
    /// placeholder of [`crate::Paths::expand_dir_template`].
    pub const AUTOSAVE_DIRECTORY: &str = "$(APPLICATION_SUPPORT)/ScratchRunner/Unsaved";
    /// The user agent of the documentation web view (an iPhone Safari).
    pub const DOCS_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1";
    /// The id of the preferred global documentation source (Dash, disabled).
    pub const PREFERRED_GLOBAL_DOCS: &str = "02787B2F-25D7-48D7-802D-14C9226EEE3A";
    /// The default invisible-characters string: escapes for tab, space,
    /// newline and carriage return, then a no-break space.
    pub const INVISIBLE_CHARACTERS: &str = "\\t \\n\\r\u{a0}";
    /// Default `TERM_PROGRAM` shell variable.
    pub const TERM_PROGRAM: &str = "ScratchRunner";
}

/// All preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Preferences {
    /// Preferences > General.
    pub general: General,
    /// Preferences > Text Editing.
    pub text_editing: TextEditing,
    /// Language-related defaults (Preferences > Languages; the per-language
    /// settings live in the language bundles).
    pub languages: Languages,
    /// Preferences > Themes.
    pub themes: Themes,
    /// Preferences > Key Bindings.
    pub key_bindings: KeyBindings,
    /// Preferences > Shell.
    pub shell: Shell,
    /// Preferences > Advanced.
    pub advanced: Advanced,
    /// Debugger view state and options (no pane).
    pub debugger: Debugger,
    /// File > Print options (no pane).
    pub printing: Printing,
    /// Window and view state kept as preferences (no pane).
    pub ui: UiState,
    /// Hidden preferences (no pane).
    pub misc: Misc,
}

// ---------------------------------------------------------------- General

/// Preferences > General.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct General {
    /// `Documents` category.
    pub documents: Documents,
    /// `Tabs` category.
    pub tabs: Tabs,
    /// `Console` category.
    pub console: ConsoleDefaults,
    /// `File Navigator` category.
    pub file_navigator: FileNavigator,
    /// `Notifications` category.
    pub notifications: Notifications,
    /// `Updates` category (not offered by ScratchRunner; kept for fidelity).
    pub updates: Updates,
    /// File extensions the user added in the File Type Associations sheet
    /// (`AddedFileTypeAssociationExtensions`).
    pub added_file_type_association_extensions: Vec<String>,
    /// File types the File Type Associations sheet checks
    /// (`allowedFileTypes`): `public.plain-text`, `public.source-code`, and
    /// any added extension. `None` — the default — means the two built-in
    /// types; `Some([])` really checks nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_file_types: Option<Vec<String>>,
}

/// The types the File Type Associations sheet checks before the user has
/// ever touched it.
pub const DEFAULT_ALLOWED_FILE_TYPES: [&str; 2] = ["public.plain-text", "public.source-code"];

/// General > Documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Documents {
    /// Enable Auto Save and Versions (`Autosave`).
    pub autosave: bool,
    /// Show all filename extensions (`AppleShowAllExtensions`).
    pub show_all_extensions: bool,
    /// Ask to save untitled documents when quitting (`AskToSaveWhenQuitting`).
    pub ask_to_save_when_quitting: bool,
    /// Use templates for new files (`OpenPresetsEnabled`).
    pub use_templates_for_new_files: bool,
    /// Allow filename suggestions from macOS (`AllowSuggestedFilenames`).
    pub allow_suggested_filenames: bool,
    /// Default File Encoding (`FileEncoding`): an encoding label from
    /// [`crate::FILE_ENCODINGS`], such as `utf-8`.
    pub file_encoding: String,
}

impl Default for Documents {
    fn default() -> Self {
        Self {
            autosave: true,
            show_all_extensions: true,
            ask_to_save_when_quitting: false,
            use_templates_for_new_files: true,
            allow_suggested_filenames: false,
            file_encoding: "utf-8".to_owned(),
        }
    }
}

/// Where a new tab is inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InsertNewTabs {
    /// After existing tabs.
    #[default]
    AfterExisting,
    /// Before existing tabs.
    BeforeExisting,
    /// After the selected tab.
    AfterSelected,
}

/// General > Tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tabs {
    /// Prefer opening new documents in tabs (`PreferTabs`).
    pub prefer_tabs: bool,
    /// Hide tab bar by default (`TabBarHidden`).
    pub tab_bar_hidden: bool,
    /// Insert New Tabs (`InsertNewTabs`).
    pub insert_new_tabs: InsertNewTabs,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            prefer_tabs: true,
            tab_bar_hidden: false,
            insert_new_tabs: InsertNewTabs::AfterExisting,
        }
    }
}

/// Where the console sits relative to the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConsolePosition {
    /// Below the editor.
    #[default]
    Bottom,
    /// To the right of the editor.
    Right,
}

/// General > Console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsoleDefaults {
    /// Console Position (`ConsolePosition`).
    pub position: ConsolePosition,
    /// Hide console automatically (`HideConsoleAutomatically`).
    pub hide_automatically: bool,
}

impl Default for ConsoleDefaults {
    fn default() -> Self {
        Self {
            position: ConsolePosition::Bottom,
            hide_automatically: true,
        }
    }
}

/// General > File Navigator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FileNavigator {
    /// Show hidden files and folders (`ShowHiddenFiles`).
    pub show_hidden_files: bool,
    /// Sort folders on top (`SortFoldersOnTop`).
    pub sort_folders_on_top: bool,
}

/// General > Notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// Show notifications while the app is inactive
    /// (`UserNotificationsEnabled`).
    pub enabled: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// General > Updates (Sparkle in CodeRunner; ScratchRunner has no updater).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    /// Check for updates automatically (`SUEnableAutomaticChecks`).
    pub check_automatically: bool,
    /// Automatically download updates when available (`SUAutomaticallyUpdate`).
    pub automatically_download: bool,
    /// Send anonymous system profile information (`SUSendProfileInfo`).
    pub send_profile_info: bool,
    /// Allow automatic updates to beta versions (`CheckForBetaUpdates`).
    pub check_for_beta_updates: bool,
    /// Seconds between automatic checks (`SUScheduledCheckInterval`).
    pub scheduled_check_interval: u32,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            check_automatically: true,
            automatically_download: false,
            send_profile_info: true,
            check_for_beta_updates: false,
            scheduled_check_interval: 172_800,
        }
    }
}

// ----------------------------------------------------------- Text Editing

/// Preferences > Text Editing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TextEditing {
    /// `Appearance` category.
    pub appearance: Appearance,
    /// `Indentation` category.
    pub indentation: Indentation,
    /// `Behavior` category.
    pub behavior: Behavior,
    /// `Completion` category.
    pub completion: Completion,
    /// `Diagnostics` category.
    pub diagnostics: DiagnosticsPrefs,
}

/// Text Editing > Appearance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// The editor font.
    pub font: FontPrefs,
    /// Syntax highlighting (`SyntaxHighlightingEnabled`).
    pub syntax_highlighting: bool,
    /// Line Spacing, a multiple of the font's line height (`LineHeight`).
    pub line_height: f64,
    /// Show navigational markers in scroller (`ScrollMarkersEnabled`).
    pub scroll_markers: bool,
    /// Keep markers visible when scroller is hidden
    /// (`ScrollMarkersAlwaysVisible`).
    pub scroll_markers_always_visible: bool,
    /// Wrap lines (`WrapLinesEnabled`).
    pub wrap_lines: bool,
    /// Allow scrolling past end of document (`AllowScrollPastEnd`).
    pub allow_scroll_past_end: bool,
    /// The page guide.
    pub page_guide: PageGuide,
    /// Invisible characters.
    pub invisibles: Invisibles,
    /// Insertion Point.
    pub insertion_point: InsertionPoint,
    /// Alternate Insertion Point (used by key-binding modes such as Vim's).
    pub alternate_insertion_point: InsertionPoint,
    /// Highlight current line (`HighlightCurrentLine`).
    pub highlight_current_line: bool,
    /// Line numbers.
    pub line_numbers: LineNumbers,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font: FontPrefs::default(),
            syntax_highlighting: true,
            line_height: 1.2,
            scroll_markers: true,
            scroll_markers_always_visible: true,
            wrap_lines: true,
            allow_scroll_past_end: true,
            page_guide: PageGuide::default(),
            invisibles: Invisibles::default(),
            insertion_point: InsertionPoint::default(),
            alternate_insertion_point: InsertionPoint {
                style: InsertionPointStyle::Block,
                ..InsertionPoint::default()
            },
            highlight_current_line: true,
            line_numbers: LineNumbers::default(),
        }
    }
}

impl Appearance {
    /// The height of an editor line in points: the font size times the
    /// line spacing, never less than the font size.
    pub fn line_height_points(&self) -> f64 {
        let spacing = if self.line_height.is_finite() { self.line_height.max(1.0) } else { 1.0 };
        self.font.size * spacing
    }
}

/// The editor font. CodeRunner archives an `NSFont` in the `Font` key; here
/// the family (or PostScript name) and size are plain values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontPrefs {
    /// Font family or PostScript name (`Font`).
    pub family: String,
    /// Size in points (`Font`).
    pub size: f64,
    /// Ligatures (`FontLigatures`).
    pub ligatures: bool,
    /// Antialiasing (`FontAntialiasing`).
    pub antialiasing: bool,
    /// Smoothing (`FontSmoothing`).
    pub smoothing: bool,
}

impl Default for FontPrefs {
    fn default() -> Self {
        Self {
            family: defaults::FONT_FAMILY.to_owned(),
            size: defaults::FONT_SIZE,
            ligatures: false,
            antialiasing: true,
            smoothing: true,
        }
    }
}

/// Show page guide at column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageGuide {
    /// Show page guide at column (`ShowPageGuide`).
    pub show: bool,
    /// The column (`PageGuide`).
    pub column: u32,
}

impl Default for PageGuide {
    fn default() -> Self {
        Self {
            show: false,
            column: 80,
        }
    }
}

/// When invisible characters are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InvisiblesMode {
    /// Everywhere (`0`).
    Everywhere,
    /// At beginning and end of lines (`1`).
    BeginningAndEnd,
    /// Nonconforming indentation (`2`).
    NonconformingIndentation,
    /// In selected text (`4`).
    Selection,
    /// In selected text and nonconforming indentation (`6`).
    #[default]
    SelectionAndNonconformingIndentation,
}

/// Show invisible characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Invisibles {
    /// Show invisible characters (`ShowInvisibles`).
    pub show: bool,
    /// The characters, as backslash escapes (`InvisibleCharacters`).
    pub characters: String,
    /// When they are drawn (`InvisiblesMode`).
    pub mode: InvisiblesMode,
}

impl Default for Invisibles {
    fn default() -> Self {
        Self {
            show: true,
            characters: defaults::INVISIBLE_CHARACTERS.to_owned(),
            mode: InvisiblesMode::default(),
        }
    }
}

/// The shape of the insertion point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InsertionPointStyle {
    /// A vertical bar.
    #[default]
    Bar,
    /// A block.
    Block,
    /// An underline.
    Underline,
}

/// One insertion-point configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InsertionPoint {
    /// The shape (`InsertionPointStyle[Alternate]`).
    pub style: InsertionPointStyle,
    /// Blink (`BlinkInsertionPoint[Alternate]`).
    pub blink: bool,
    /// Animate the blink (`AnimateInsertionPoint[Alternate]`).
    pub animate: bool,
    /// Visible with selection (`InsertionPointVisibleWithSelection[Alternate]`).
    pub visible_with_selection: bool,
}

impl Default for InsertionPoint {
    fn default() -> Self {
        Self {
            style: InsertionPointStyle::Bar,
            blink: true,
            animate: true,
            visible_with_selection: true,
        }
    }
}

/// Line numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineNumbers {
    /// Show line numbers (`LineNumbersEnabled`).
    pub enabled: bool,
    /// Relative to selection (`RelativeLineNumbers`).
    pub relative: bool,
    /// Draw separator at left margin (`DrawLineNumberSeparator`).
    pub draw_separator: bool,
}

impl Default for LineNumbers {
    fn default() -> Self {
        Self {
            enabled: true,
            relative: false,
            draw_separator: true,
        }
    }
}

/// Text Editing > Indentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Indentation {
    /// Indent Using: `false` tabs, `true` spaces (`DefaultTabModeSoftTabs`).
    pub soft_tabs: bool,
    /// Width of an indent level (`DefaultTabModeWidth`).
    pub tab_width: u32,
    /// Indent wrapped lines (`IndentWrappedLines`).
    pub indent_wrapped_lines: bool,
    /// Indent levels added to wrapped lines (`IndentWrappedLinesLevel`).
    pub indent_wrapped_lines_level: f64,
    /// Treat consecutive spaces indentation as single unit
    /// (`SpacesIndentationSingleUnit`).
    pub spaces_indentation_single_unit: bool,
    /// Auto-Indent while typing (`AutoIndentWhileTyping`).
    pub auto_indent_while_typing: bool,
    /// Auto-Indent pasted code (`AutoIndentOnPaste`).
    pub auto_indent_on_paste: bool,
}

impl Default for Indentation {
    fn default() -> Self {
        Self {
            soft_tabs: false,
            tab_width: 4,
            indent_wrapped_lines: true,
            indent_wrapped_lines_level: 1.0,
            spaces_indentation_single_unit: true,
            auto_indent_while_typing: true,
            auto_indent_on_paste: true,
        }
    }
}

/// Text Editing > Behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    /// Character pairs.
    pub character_pairs: CharacterPairs,
    /// Double-click on word also selects relevant special chars
    /// (`IncludeVariableCharsInWordSelectionGranularity`).
    pub word_selection_includes_variable_chars: bool,
    /// Code tokenizers.
    pub tokenizers: Tokenizers,
    /// Kill-Ring Size (`NSTextKillRingSize`).
    pub kill_ring_size: u32,
    /// Spelling.
    pub spelling: Spelling,
    /// Modifier masks (`NSEvent` flags) that start multiple-selection mouse
    /// gestures (`MultipleSelectionsEventMasks`). Hidden.
    pub multiple_selections_event_masks: Vec<i64>,
    /// `NonContiguousLayout`. Hidden.
    pub non_contiguous_layout: bool,
    /// `BreakLinesAtAnyCharacter`. Hidden.
    pub break_lines_at_any_character: bool,
    /// Comment characters chosen by the user per scope, such as
    /// `{"text.plain": {"line": "//"}}` (`CommentCharactersByScope`). Hidden.
    pub comment_characters_by_scope: BTreeMap<String, BTreeMap<String, String>>,
    /// Patterns saved in the find bar (`TextFinderUserPatterns`). Hidden.
    pub text_finder_user_patterns: Vec<String>,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            character_pairs: CharacterPairs::default(),
            word_selection_includes_variable_chars: true,
            tokenizers: Tokenizers::default(),
            kill_ring_size: 7,
            spelling: Spelling::default(),
            multiple_selections_event_masks: vec![393_216, 1_048_576, 524_288],
            non_contiguous_layout: true,
            break_lines_at_any_character: false,
            comment_characters_by_scope: BTreeMap::new(),
            text_finder_user_patterns: Vec::new(),
        }
    }
}

/// Automatic closing of, and jumping past, character pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterPairs {
    /// Automatically close character pairs (`CloseBracketsEnabled`).
    pub close_enabled: bool,
    /// Close `{` (`CloseCurlyBrackets`).
    pub close_curly_brackets: bool,
    /// Close `(` (`CloseParens`).
    pub close_parens: bool,
    /// Close `[` (`CloseSquareBrackets`).
    pub close_square_brackets: bool,
    /// Close `<` (`CloseLessThan`).
    pub close_less_than: bool,
    /// Close `"` (`CloseDoubleQuote`).
    pub close_double_quote: bool,
    /// Close `'` (`CloseSingleQuote`).
    pub close_single_quote: bool,
    /// Close `` ` `` (`CloseBacktick`).
    pub close_backtick: bool,
    /// Jump past ending character when matching key pressed
    /// (`JumpPastBracketsEnabled`).
    pub jump_past_enabled: bool,
    /// Jump past `}` (`JumpPastCurlyBrackets`).
    pub jump_past_curly_brackets: bool,
    /// Jump past `)` (`JumpPastParens`).
    pub jump_past_parens: bool,
    /// Jump past `]` (`JumpPastSquareBrackets`).
    pub jump_past_square_brackets: bool,
    /// Jump past `>` (`JumpPastGreaterThan`).
    pub jump_past_greater_than: bool,
    /// Jump past `"` (`JumpPastDoubleQuote`).
    pub jump_past_double_quote: bool,
    /// Jump past `'` (`JumpPastSingleQuote`).
    pub jump_past_single_quote: bool,
    /// Jump past `` ` `` (`JumpPastBacktick`).
    pub jump_past_backtick: bool,
    /// Automatically highlight balancing characters
    /// (`HighlightClosingBrackets`).
    pub highlight_closing_brackets: bool,
}

impl Default for CharacterPairs {
    fn default() -> Self {
        Self {
            close_enabled: true,
            close_curly_brackets: true,
            close_parens: true,
            close_square_brackets: true,
            close_less_than: true,
            close_double_quote: true,
            close_single_quote: true,
            close_backtick: true,
            jump_past_enabled: true,
            jump_past_curly_brackets: true,
            jump_past_parens: true,
            jump_past_square_brackets: true,
            jump_past_greater_than: true,
            jump_past_double_quote: true,
            jump_past_single_quote: true,
            jump_past_backtick: true,
            highlight_closing_brackets: true,
        }
    }
}

/// Code tokenizers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tokenizers {
    /// Enable code tokenizers (`TokenizersEnabled`).
    pub enabled: bool,
    /// Clang (`ClangTokenizerEnabled`).
    pub clang: bool,
    /// PHP (`PHPTokenizerEnabled`).
    pub php: bool,
    /// Python (`PythonTokenizerEnabled`).
    pub python: bool,
    /// Shell (`ShellTokenizerEnabled`).
    pub shell: bool,
    /// Swift (`SwiftTokenizerEnabled`).
    pub swift: bool,
    /// Generic Source Code (`GenericSourceTokenizerEnabled`).
    pub generic_source: bool,
    /// Underline tokens at insertion point (`HighlightTokens`).
    pub highlight_tokens: bool,
}

impl Default for Tokenizers {
    fn default() -> Self {
        Self {
            enabled: true,
            clang: true,
            php: true,
            python: true,
            shell: true,
            swift: true,
            generic_source: true,
            highlight_tokens: true,
        }
    }
}

/// Spell checking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spelling {
    /// Check spelling while typing (`CheckSpellingWhileTyping`).
    pub check_while_typing: bool,
    /// Check grammar with spelling (`CheckGrammarWithSpelling`).
    pub check_grammar: bool,
    /// Correct spelling automatically (`CorrectSpellingAutomatically`).
    pub correct_automatically: bool,
}

impl Default for Spelling {
    fn default() -> Self {
        Self {
            check_while_typing: true,
            check_grammar: false,
            correct_automatically: false,
        }
    }
}

/// Text Editing > Completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Completion {
    /// Suggest completions while typing (`CompletionsEnabled`).
    pub enabled: bool,
    /// Display completions after typing N characters
    /// (`CodeCompletionWordLength`).
    pub word_length: u32,
    /// Ignore with only low-quality results (`CodeCompletionFuzzyLimit`).
    pub fuzzy_limit: bool,
    /// Fuzzy search (`CodeCompletionFuzzy`).
    pub fuzzy: bool,
    /// Force correct character order in matches
    /// (`CodeCompletionFuzzyEnforceCharacterOrder`).
    pub fuzzy_enforce_character_order: bool,
    /// Prioritize previously chosen completions (`CodeCompletionHitsEnabled`).
    pub hits_enabled: bool,
    /// Use word-completions for unsupported languages
    /// (`WordCompletionsEnabled`).
    pub word_completions: bool,
    /// Enable short string completions (`ShortStringCompletionsEnabled`).
    pub short_string_completions: bool,
    /// Automatically insert import statements
    /// (`AutomaticallyInsertImportStatements`).
    pub automatically_insert_imports: bool,
    /// Animate completion panel (`AnimateCompletionPanel`).
    pub animate_panel: bool,
}

impl Default for Completion {
    fn default() -> Self {
        Self {
            enabled: true,
            word_length: 2,
            fuzzy_limit: true,
            fuzzy: true,
            fuzzy_enforce_character_order: false,
            hits_enabled: true,
            word_completions: true,
            short_string_completions: true,
            automatically_insert_imports: true,
            animate_panel: true,
        }
    }
}

/// Text Editing > Diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsPrefs {
    /// Generate diagnostics while typing (`DiagnosticsWhileTyping`).
    pub while_typing: bool,
    /// Delay in seconds (`DiagnosticsDelay`).
    pub delay: f64,
    /// Show errors derived from syntax definitions (`DiagnosticsInvalidSyntax`).
    pub invalid_syntax: bool,
    /// Only show first syntax error in document
    /// (`DiagnosticsInvalidSyntaxLimitToOne`).
    pub invalid_syntax_limit_to_one: bool,
    /// Include errors from the console in the editor (`DiagnosticsFromConsole`).
    pub from_console: bool,
    /// Animate diagnostic messages (`DiagnosticsAnimate`).
    pub animate: bool,
    /// Highlight lines with diagnostics (`DiagnosticsHighlightLine`).
    pub highlight_line: bool,
    /// Underline text associated with diagnostics (`DiagnosticsUnderline`).
    pub underline: bool,
    /// Include diagnostics in symbol navigator
    /// (`SymbolListIncludesEditorAnnotations`).
    pub symbol_list_includes_annotations: bool,
    /// Show: `false` full messages, `true` icon only (`DiagnosticsIconOnly`).
    pub icon_only: bool,
    /// Automatically applied fixes: regular expressions, one per line
    /// (`AutoFixDiagnostics`).
    pub auto_fix_diagnostics: String,
    /// Globally ignored diagnostic messages: regular expressions, one per line
    /// (`IgnoredDiagnostics`).
    pub ignored_diagnostics: String,
    /// Per-diagnostic update-mode overrides from the Diagnostics menu:
    /// `language-id/diagnostic-id` -> bits (1 edit, 2 run, 4 manual)
    /// (`CRUpdateModes`).
    pub update_overrides: BTreeMap<String, i64>,
    /// Diagnostics disabled for every document
    /// (`language-id/diagnostic-id`); `language-id/console` is the console
    /// text source.
    pub disabled_diagnostics: BTreeSet<String>,
    /// Console diagnostics set to "Console Highlight Only"
    /// (`language-id/diagnostic-id`).
    pub console_highlight_only: BTreeSet<String>,
}

impl Default for DiagnosticsPrefs {
    fn default() -> Self {
        Self {
            while_typing: true,
            delay: 1.5,
            invalid_syntax: true,
            invalid_syntax_limit_to_one: true,
            from_console: true,
            animate: true,
            highlight_line: true,
            underline: true,
            symbol_list_includes_annotations: true,
            icon_only: false,
            auto_fix_diagnostics: String::new(),
            ignored_diagnostics: String::new(),
            update_overrides: BTreeMap::new(),
            disabled_diagnostics: BTreeSet::new(),
            console_highlight_only: BTreeSet::new(),
        }
    }
}

// -------------------------------------------------------------- Languages

/// Language-related defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Languages {
    /// Id of the language of the last new document (`CurrentLanguageUUID`).
    /// Empty before the first launch has chosen one.
    pub current_language: String,
    /// Version of the language bundles last installed
    /// (`LanguagesBundleVersion`).
    pub bundle_version: u32,
    /// Documentation sources.
    pub docs: DocsPrefs,
}

/// How a documentation source is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    /// A URL with `{query}` replaced by the search string (`0`).
    #[default]
    Search,
    /// A fixed page (`1`).
    Bookmark,
    /// A find-in-page lookup (`2`).
    TextFind,
}

/// A global documentation source (`GlobalDocs` entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DocSource {
    /// Stable id (`uuid`).
    pub id: String,
    /// Display name (`Name`).
    pub name: String,
    /// How it is used (`Type`).
    pub kind: DocKind,
    /// The URL; `{query}` and `{lang}` are replaced (`URL`).
    pub url: String,
    /// POST body, empty for none (`POST`).
    pub post: String,
    /// Whether the source is offered (`Enabled`).
    pub enabled: bool,
}

/// Per-source docs view state (`DocsState` entry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DocsState {
    /// Recolour the page to the editor theme (`Transparency`).
    pub transparency: bool,
    /// A search redirects to the page being looked up
    /// (`expectsRedirectFromSearchResults`).
    pub expects_redirect_from_search_results: bool,
    /// Persisted zoom, a fraction where `1.0` is 100% (`ZoomLevel`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom_level: Option<f64>,
}

/// Documentation sources and their state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocsPrefs {
    /// When invoked from the editor, always use the preferred source
    /// (`UsePreferredGlobalDocsUUID`).
    pub use_preferred_global_docs: bool,
    /// Id of the preferred global source (`PreferredGlobalDocsUUID`).
    pub preferred_global_docs_id: String,
    /// Global entries, available for all languages (`GlobalDocs`).
    pub global_entries: Vec<DocSource>,
    /// View state by source id (`DocsState`).
    pub state: BTreeMap<String, DocsState>,
    /// Automatic lookups (`AutolookupEnabled`).
    pub autolookup_enabled: bool,
    /// Do not ask about a 404 lookup page (`Suppress404Alert`).
    pub suppress_404_alert: bool,
}

impl Default for DocsPrefs {
    fn default() -> Self {
        Self {
            use_preferred_global_docs: false,
            preferred_global_docs_id: defaults::PREFERRED_GLOBAL_DOCS.to_owned(),
            global_entries: default_global_docs(),
            state: default_docs_state(),
            autolookup_enabled: false,
            suppress_404_alert: false,
        }
    }
}

fn doc(id: &str, name: &str, kind: DocKind, url: &str, enabled: bool) -> DocSource {
    DocSource {
        id: id.to_owned(),
        name: name.to_owned(),
        kind,
        url: url.to_owned(),
        post: String::new(),
        enabled,
    }
}

/// The five global documentation sources of a fresh install.
pub fn default_global_docs() -> Vec<DocSource> {
    vec![
        doc(
            "DEE58E80-8F40-4CC3-8CF4-A6CB3839F18C",
            "Google",
            DocKind::Search,
            "https://www.google.com/search?q={query}+{lang}#rso",
            true,
        ),
        doc(
            "65FBFE31-7A8E-48BD-8CE3-49164CB25EB3",
            "Stack Overflow",
            DocKind::Search,
            "https://stackoverflow.com/search?q=%5B{lang}%5D+{query}",
            true,
        ),
        doc(
            "8FEA0EBE-873A-4991-8E6A-6D494D9DB050",
            "ChatGPT",
            DocKind::Bookmark,
            "https://chatgpt.com",
            true,
        ),
        doc(
            "507F15C9-03D5-4DF2-96E3-FF19DDD50168",
            "Claude",
            DocKind::Bookmark,
            "https://claude.ai",
            true,
        ),
        doc(
            defaults::PREFERRED_GLOBAL_DOCS,
            "Dash",
            DocKind::Search,
            "dash://{lang}:{query}",
            false,
        ),
    ]
}

fn default_docs_state() -> BTreeMap<String, DocsState> {
    [
        "FF667E76-BF73-4C18-AD70-EABAC0861D92",
        "69B84E29-B4B4-423E-B816-7E6818E845F3",
        "A8506643-4CED-4DAD-8691-34283E1C9A59",
        "6984DD13-CCF5-43D7-B6CF-993EE0235A86",
    ]
    .into_iter()
    .map(|id| {
        (
            id.to_owned(),
            DocsState {
                transparency: true,
                ..DocsState::default()
            },
        )
    })
    .collect()
}

// ----------------------------------------------------------------- Themes

/// Preferences > Themes: which theme is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Themes {
    /// Use separate themes for different appearances (`UseSeparateColorThemes`).
    pub use_separate_color_themes: bool,
    /// The theme for all appearances when [`Themes::use_separate_color_themes`]
    /// is off (`ColorTheme`).
    pub color_theme: String,
    /// The theme for the light appearance (`ColorThemeLight`).
    pub color_theme_light: String,
    /// The theme for the dark appearance (`ColorThemeDark`).
    pub color_theme_dark: String,
    /// Window chrome follows the theme's light or dark nature
    /// (`MatchAppearanceToTheme`).
    pub match_appearance_to_theme: bool,
}

impl Default for Themes {
    fn default() -> Self {
        Self {
            use_separate_color_themes: true,
            color_theme: defaults::THEME.to_owned(),
            color_theme_light: defaults::THEME_LIGHT.to_owned(),
            color_theme_dark: defaults::THEME_DARK.to_owned(),
            match_appearance_to_theme: false,
        }
    }
}

impl Themes {
    /// The name of the theme to use for the system appearance.
    pub fn resolve(&self, dark: bool) -> &str {
        match (self.use_separate_color_themes, dark) {
            (false, _) => &self.color_theme,
            (true, true) => &self.color_theme_dark,
            (true, false) => &self.color_theme_light,
        }
    }
}

// ----------------------------------------------------------- Key Bindings

/// Preferences > Key Bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    /// Name of the active key-binding set (`KeyBindingSet`).
    pub set: String,
    /// Compound commands are active (`CompoundCommandsEnabled`).
    pub compound_commands_enabled: bool,
    /// Per-command key equivalents the user recorded (`command id` →
    /// CodeRunner `@~^$` text; `""` disables the command's key).
    pub overrides: BTreeMap<String, String>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            set: defaults::KEY_BINDING_SET.to_owned(),
            compound_commands_enabled: true,
            overrides: BTreeMap::new(),
        }
    }
}

// ------------------------------------------------------------------ Shell

/// Preferences > Shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shell {
    /// Automatically determine the `PATH` environment variable
    /// (`AutomaticallyDeterminePATH`); otherwise [`Shell::path`] is used.
    pub automatically_determine_path: bool,
    /// The custom `PATH` (`PATH`).
    pub path: String,
    /// Invoke the shell in login mode when running code (`RunAsLoginShell`).
    pub run_as_login_shell: bool,
    /// The shell (`Shell`).
    pub shell: String,
    /// Shell variables (`Environment`).
    pub environment: BTreeMap<String, String>,
    /// Extra shell arguments (`ShellArguments`). Hidden.
    pub arguments: Vec<String>,
    /// Extra shell arguments for the compile step (`CompileShellArguments`).
    /// Hidden.
    pub compile_arguments: Vec<String>,
}

/// The shell variables of a fresh install: `TERM`, `TERM_PROGRAM`, `LC_CTYPE`.
pub fn default_environment() -> BTreeMap<String, String> {
    [
        ("TERM", "dumb"),
        ("TERM_PROGRAM", defaults::TERM_PROGRAM),
        ("LC_CTYPE", "UTF-8"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            automatically_determine_path: true,
            path: defaults::PATH.to_owned(),
            run_as_login_shell: false,
            shell: defaults::SHELL.to_owned(),
            environment: default_environment(),
            arguments: Vec::new(),
            compile_arguments: Vec::new(),
        }
    }
}

// --------------------------------------------------------------- Advanced

/// Preferences > Advanced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    /// Use columns in selection label (`SelectionLabelUsesColumns`).
    pub selection_label_uses_columns: bool,
    /// Automatically resolve relative file URLs in webpages
    /// (`ResolveRelativeFileURLs`).
    pub resolve_relative_file_urls: bool,
    /// Hide debugger termination message (`DebuggerHideTerminationMessage`).
    pub debugger_hide_termination_message: bool,
    /// Close new unedited document when opening another
    /// (`ReplaceUntitledDocument`).
    pub replace_untitled_document: bool,
    /// Write compiler output files to temporary directory
    /// (`WriteCompilerOutputFilesToTemporaryDirectory`).
    pub write_compiler_output_to_temp_dir: bool,
    /// Group similar languages in menus (`GroupLanguages`).
    pub group_languages: bool,
    /// Use custom file icons (`CustomIconsEnabled`).
    pub custom_icons_enabled: bool,
    /// Enable three-finger tap in editor (`ThreeFingerLookupEnabled`).
    pub three_finger_lookup_enabled: bool,
    /// Clang completion and diagnostics.
    pub clang: ClangPrefs,
    /// Extra Swift Flags (`ExtraSwiftFlags`).
    pub extra_swift_flags: String,
    /// Debugging.
    pub debugging: DebuggingPrefs,
    /// User agent of the documentation web view (`DocsUserAgent`). Hidden.
    pub docs_user_agent: String,
    /// Where untitled documents are kept (`AutosaveDocumentsDirectory`).
    /// Hidden; supports `$(APPLICATION_SUPPORT)`.
    pub autosave_documents_directory: String,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            selection_label_uses_columns: true,
            resolve_relative_file_urls: true,
            debugger_hide_termination_message: true,
            replace_untitled_document: true,
            write_compiler_output_to_temp_dir: true,
            group_languages: true,
            custom_icons_enabled: true,
            three_finger_lookup_enabled: true,
            clang: ClangPrefs::default(),
            extra_swift_flags: "-sdk macosx".to_owned(),
            debugging: DebuggingPrefs::default(),
            docs_user_agent: defaults::DOCS_USER_AGENT.to_owned(),
            autosave_documents_directory: defaults::AUTOSAVE_DIRECTORY.to_owned(),
        }
    }
}

/// Advanced > Clang completion and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClangPrefs {
    /// Automatically determine appropriate system compile flags
    /// (`AutomaticClangFlags`).
    pub automatic_flags: bool,
    /// Apply user-defined compile flags (`ClangFlagsFromUser`).
    pub flags_from_user: bool,
    /// Extra Clang Flags (`ExtraClangFlags`).
    pub extra_flags: String,
}

impl Default for ClangPrefs {
    fn default() -> Self {
        Self {
            automatic_flags: true,
            flags_from_user: true,
            extra_flags: "-Wno-unused-command-line-argument".to_owned(),
        }
    }
}

/// Advanced > Debugging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DebuggingPrefs {
    /// Center current location during stepping (`CenterDuringStepping`).
    pub center_during_stepping: bool,
}

// --------------------------------------------------------------- Debugger

/// How the debugger variables view is sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VariablesSortMode {
    /// By name (`0`).
    Name,
    /// In the order the debugger reports them (`1`).
    #[default]
    Natural,
    /// Reverse of the natural order (`2`).
    ReverseNatural,
}

/// Debugger view state and options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Debugger {
    /// Variables sort mode (`DebuggerVariablesSortMode`).
    pub variables_sort_mode: VariablesSortMode,
    /// Show variable types, by debugger name (`DebuggerVariablesDisplayTypes`).
    pub variables_display_types: BTreeMap<String, bool>,
    /// Show all variables, by debugger name (`DebuggerVariablesShowAll`).
    pub variables_show_all: BTreeMap<String, bool>,
    /// Show variable values on hover (`HoverVariables`).
    pub hover_variables: bool,
    /// Hide the debugger view when the run ends
    /// (`AutomaticallyHideDebuggerView`).
    pub hide_view_automatically: bool,
    /// Log every debugger command (`VerboseDebugger`).
    pub verbose: bool,
}

impl Default for Debugger {
    fn default() -> Self {
        Self {
            variables_sort_mode: VariablesSortMode::Natural,
            variables_display_types: [("pdb", false), ("xdebug", true), ("gdb", true), ("lldb", true)]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            variables_show_all: BTreeMap::new(),
            hover_variables: true,
            hide_view_automatically: true,
            verbose: false,
        }
    }
}

// --------------------------------------------------------------- Printing

/// File > Print options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Printing {
    /// Print with colors (`PrintWithColors`).
    pub with_colors: bool,
    /// Print with line numbers (`PrintWithLineNumbers`).
    pub with_line_numbers: bool,
    /// Font size in points (`PrintFontSize`).
    pub font_size: u32,
    /// Line height multiple (`PrintLineHeight`).
    pub line_height: f64,
    /// Top margin in points (`PrintTopMargin`).
    pub top_margin: u32,
    /// Bottom margin in points (`PrintBottomMargin`).
    pub bottom_margin: u32,
    /// Left margin in points (`PrintLeftMargin`).
    pub left_margin: u32,
    /// Right margin in points (`PrintRightMargin`).
    pub right_margin: u32,
}

impl Default for Printing {
    fn default() -> Self {
        Self {
            with_colors: true,
            with_line_numbers: true,
            font_size: 8,
            line_height: 1.2,
            top_margin: 40,
            bottom_margin: 40,
            left_margin: 40,
            right_margin: 40,
        }
    }
}

// ---------------------------------------------------------------- UI state

/// Window and view state kept in the preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The file navigator sidebar is shown (`SidebarVisible`).
    pub sidebar_visible: bool,
    /// Hide the window title text (`TitlebarHidden`).
    pub titlebar_hidden: bool,
    /// Root of the file navigator (`FileViewDirectory`); may start with `~`.
    pub file_view_directory: String,
    /// The last path typed into Go to Folder (`GoToFolderPath`).
    pub go_to_folder_path: String,
    /// Directories excluded from searches, comma separated
    /// (`SearchExcludedDirectories`).
    pub search_excluded_directories: String,
    /// The PDF console scales to fit (`PDFViewAutoScales`).
    pub pdf_view_auto_scales: bool,
    /// The PDF console scale factor (`PDFViewScaleFactor`).
    pub pdf_view_scale_factor: f64,
    /// The pane the Preferences window opens on
    /// (`NKPreferencesSelectedIdentifier`), a [`crate::Pane::identifier`].
    pub selected_preferences_pane: String,
    /// Prevent legacy scrollers when using a resize indicator
    /// (`NSOverlayScrollersFallBackForAccessoryViews`).
    pub overlay_scrollers_fallback_for_accessory_views: bool,
    /// The web console's inspector.
    pub web_inspector: WebInspector,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            titlebar_hidden: false,
            file_view_directory: "~/".to_owned(),
            go_to_folder_path: String::new(),
            search_excluded_directories: String::new(),
            pdf_view_auto_scales: true,
            pdf_view_scale_factor: 1.0,
            selected_preferences_pane: "General".to_owned(),
            overlay_scrollers_fallback_for_accessory_views: false,
            web_inspector: WebInspector::default(),
        }
    }
}

/// Web console inspector options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebInspector {
    /// Developer extras are enabled (`WebKitDeveloperExtras`).
    pub developer_extras: bool,
    /// The inspector starts attached (`WebKit2InspectorStartsAttached`).
    pub starts_attached: bool,
    /// Attachment side: `0` bottom, `1` right, `2` left
    /// (`WebKit2InspectorAttachmentSide`).
    pub attachment_side: u32,
}

impl Default for WebInspector {
    fn default() -> Self {
        Self {
            developer_extras: true,
            starts_attached: true,
            attachment_side: 1,
        }
    }
}

// ------------------------------------------------------------------- Misc

/// Hidden preferences without a pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Misc {
    /// Alerts the user chose to suppress, by alert token (`SuppressedAlerts`).
    pub suppressed_alerts: BTreeMap<String, bool>,
    /// Number of launches (`launches`); `0` means the first launch.
    pub launches: u32,
    /// RFC 3339 time of the last launch (`LastLaunchDate`), empty if never.
    pub last_launch_date: String,
    /// The user chose to set the app as the default editor
    /// (`SetAsDefaultEditor`).
    pub set_as_default_editor: bool,
    /// Do not ask about being the default editor again
    /// (`SetAsDefaultEditorSuppressed`).
    pub set_as_default_editor_suppressed: bool,
    /// Prompts about installing a missing diagnostics tool, by diagnostic id
    /// (`DiagnosticsInstallationInfo`).
    pub diagnostics_installation_info: BTreeMap<String, InstallInfo>,
}

/// What happened when a diagnostic's tool was found missing
/// (`DiagnosticsInstallationInfo` entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstallInfo {
    /// The exit status that revealed the missing tool (`status`, usually 127).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    /// The user was already asked to install it (`prompted`).
    pub prompted: bool,
}
