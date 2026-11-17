//! The language definition model: [`LanguageDef`] and the small types in it.
//!
//! A definition is plain data (serde + TOML). Built-in definitions are
//! embedded from `defs/*.toml`; user definitions are TOML files in the user's
//! language directory with the same schema. Run/compile behaviour is in
//! [`crate::RunSpec`], diagnostics in [`DiagnosticDef`], templates in
//! [`crate::TemplateDef`], documentation sources in [`DocSource`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::LanguageError;
use crate::run::RunSpec;
use crate::template::TemplateDef;
use crate::vars::percent_encode;

fn yes() -> bool {
    true
}

fn four() -> u8 {
    4
}

fn is_yes(value: &bool) -> bool {
    *value
}

fn is_four(value: &u8) -> bool {
    *value == 4
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A language: everything the app knows about editing and running one kind of
/// source file. Mirrors CodeRunner's `Language.plist` with a declarative run specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageDef {
    /// Stable slug, unique in a registry: `python`, `javascript-node`. Used in
    /// settings, documents and file names. `[a-z0-9]` and `-` only.
    pub id: String,
    /// CodeRunner's language uuid, kept for `.crLanguage` import/export and
    /// for documents that remember a uuid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Display name, also the menu title: `JavaScript (Node.js)`.
    pub name: String,
    /// Menu group when it differs from what the name implies (`JavaScript`
    /// for `JavaScript (Node.js)`). See [`LanguageDef::group_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Variant label inside the group (`Node.js`). See
    /// [`LanguageDef::variant_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Shown in language menus and considered by detection (CodeRunner's
    /// per-language checkbox).
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    /// File extensions without the dot, lowercase. The first is the one new
    /// files get.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Extensions for which this language wins when several languages list
    /// the same one (`h`, `js`, `ts`, `cs`). An extension claimed by one
    /// language only needs no entry here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_for: Vec<String>,
    /// Exact file names (`Makefile`, `.zshrc`, `Rakefile`), case-sensitive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filenames: Vec<String>,
    /// Interpreter names for `#!` lines (`python`, `node`, `bash`). An entry
    /// also matches the entry followed by digits and dots (`python3.12`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shebangs: Vec<String>,
    /// Editor mode names in modelines (`# vim: ft=python`, `-*- mode: python
    /// -*-`) that select this language, lowercase.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modelines: Vec<String>,
    /// Regular expressions whose matches in a text make it look like this
    /// language; used by "New from Clipboard". Each distinct match scores one
    /// point; see [`crate::detect`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_patterns: Vec<String>,
    /// The name to give the NSH editor's `language()` prop (verified against
    /// `nsh-base`'s built-in language table, see [`crate::NSH_BUILTIN_EDITOR_LANGUAGES`]).
    pub editor_syntax: String,
    /// A file extension or name the syntect syntax set (two-face) resolves, for
    /// languages NSH does not know by name (AppleScript, LaTeX). Defaults to
    /// `editor_syntax`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax_hint: Option<String>,
    /// TextMate scope of the syntax (`source.python`), CodeRunner's `Grammar`.
    #[serde(default = "default_grammar")]
    pub grammar: String,
    /// Comment markers for Un/Comment Lines.
    #[serde(default, skip_serializing_if = "CommentMarkers::is_empty")]
    pub comments: CommentMarkers,
    /// Indentation width used when `custom_tab_mode` is true (else the global
    /// default applies).
    #[serde(default = "four", skip_serializing_if = "is_four")]
    pub tab_width: u8,
    /// Spaces instead of tabs when `custom_tab_mode` is true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub soft_tabs: bool,
    /// Whether `tab_width`/`soft_tabs` override the global defaults.
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom_tab_mode: bool,
    /// How the language is compiled and run.
    #[serde(default, skip_serializing_if = "RunSpec::is_empty")]
    pub run: RunSpec,
    /// Console behaviour.
    #[serde(default, skip_serializing_if = "ConsoleSpec::is_default")]
    pub console: ConsoleSpec,
    /// Console I/O encoding as an `NSStringEncoding` code; 0 = the global
    /// default (UTF-8). See [`crate::encoding_name`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output_encoding: i64,
    /// Debugger bundle name (`lldb`, `pdb`, `node`, `jdb`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debugger: Option<String>,
    /// How to install the toolchain when it is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer: Option<InstallerHint>,
    /// The language needs a toolchain that macOS does not ship.
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_external_components: bool,
    /// Messages shown when the compile or run command exits with a status
    /// (the key is the status as a string; only `"127"` is used in practice).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub errors: BTreeMap<String, String>,
    /// Linters and error sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticDef>,
    /// Also use the built-in diagnostics (invalid syntax, conflict markers);
    /// the inverse of CodeRunner's `DiagnosticsDontUseBuiltins`.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub builtin_diagnostics: bool,
    /// New-file templates.
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "templates")]
    pub templates: Vec<TemplateDef>,
    /// Documentation sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docs: Vec<DocSource>,
    /// Format revision (CodeRunner's `LanguageBundleVersion`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub bundle_version: u32,
    /// `true` for definitions shipped with the app, set by the registry when
    /// it loads them; it is never written to user files.
    #[serde(default, skip_serializing)]
    pub builtin: bool,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn default_grammar() -> String {
    "text.plain".to_owned()
}

impl LanguageDef {
    /// A new, empty definition (what "Create a new language" starts with):
    /// no run command, grammar `text.plain`, tabs of width 4.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            uuid: None,
            name: name.into(),
            group: None,
            variant: None,
            enabled: true,
            extensions: Vec::new(),
            default_for: Vec::new(),
            filenames: Vec::new(),
            shebangs: Vec::new(),
            modelines: Vec::new(),
            content_patterns: Vec::new(),
            editor_syntax: "Plain Text".to_owned(),
            syntax_hint: None,
            grammar: default_grammar(),
            comments: CommentMarkers::default(),
            tab_width: 4,
            soft_tabs: false,
            custom_tab_mode: false,
            run: RunSpec::default(),
            console: ConsoleSpec::default(),
            output_encoding: 0,
            debugger: None,
            installer: None,
            requires_external_components: false,
            errors: BTreeMap::new(),
            diagnostics: Vec::new(),
            builtin_diagnostics: true,
            templates: Vec::new(),
            docs: Vec::new(),
            bundle_version: 0,
            builtin: false,
        }
    }

    /// Parses a definition from TOML.
    pub fn from_toml_str(text: &str) -> Result<Self, LanguageError> {
        Self::from_toml_with_origin(text, None)
    }

    pub(crate) fn from_toml_with_origin(
        text: &str,
        origin: Option<&str>,
    ) -> Result<Self, LanguageError> {
        toml::from_str(text).map_err(|e| LanguageError::Parse {
            origin: origin.map(str::to_owned),
            message: e.to_string(),
        })
    }

    /// Serializes the definition as TOML (the export format; user overrides
    /// are stored the same way).
    pub fn to_toml_string(&self) -> Result<String, LanguageError> {
        toml::to_string_pretty(self).map_err(|e| LanguageError::Serialize(e.to_string()))
    }

    /// The menu group: the explicit `group`, else the part of the name before
    /// ` (Variant)` or a trailing version number, else the whole name.
    pub fn group_name(&self) -> &str {
        match &self.group {
            Some(group) => group,
            None => split_group_variant(&self.name).0,
        }
    }

    /// The variant label: the explicit `variant`, else what is in parentheses
    /// (or the trailing version) of the name.
    pub fn variant_name(&self) -> Option<&str> {
        match &self.variant {
            Some(variant) => Some(variant),
            None => split_group_variant(&self.name).1,
        }
    }

    /// The extension new files of this language get (`py`), if any.
    pub fn default_extension(&self) -> Option<&str> {
        self.extensions.first().map(String::as_str)
    }

    /// Whether a program can be run for this language (a run command or a
    /// declarative run exists, or an in-process renderer does).
    pub fn is_runnable(&self) -> bool {
        !self.run.command.trim().is_empty() || self.console.renderer.is_some()
    }

    /// Whether the language has a compile phase (`Compile` / ⌘B semantics).
    pub fn has_compile_step(&self) -> bool {
        !matches!(self.run.compile, crate::run::CompileSpec::None)
    }

    /// Plans a run of this language (see [`RunSpec::plan`]); the run command
    /// and flags default to the language's.
    ///
    /// # Errors
    ///
    /// [`crate::PlanError`] when a template fails or a requirement is not met.
    pub fn plan_run(
        &self,
        ctx: &crate::run::RunContext,
    ) -> Result<crate::run::RunPlan, crate::error::PlanError> {
        let mut ctx = ctx.clone();
        if !ctx.vars.is_set("encoding_name") {
            ctx.vars = ctx.vars.with_encoding(self.output_encoding);
        }
        self.run.plan(&ctx)
    }

    /// The syntect hint: `syntax_hint`, else `editor_syntax`.
    pub fn syntax_hint(&self) -> &str {
        self.syntax_hint.as_deref().unwrap_or(&self.editor_syntax)
    }

    /// Whether NSH's editor knows `editor_syntax` by itself. When false the app
    /// must register an editor language first ([`LanguageDef::editor_registration`]).
    pub fn editor_syntax_is_builtin(&self) -> bool {
        is_nsh_builtin_editor_language(&self.editor_syntax)
    }

    /// The data to register with NSH (`register_language(&EditorLanguage::new(..))`)
    /// when [`LanguageDef::editor_syntax_is_builtin`] is false; `None` otherwise.
    pub fn editor_registration(&self) -> Option<EditorRegistration> {
        if self.editor_syntax_is_builtin() {
            return None;
        }
        Some(EditorRegistration {
            name: self.editor_syntax.clone(),
            aliases: vec![self.id.clone()],
            extensions: self.extensions.clone(),
            file_names: self.filenames.clone(),
            line_comment: self.comments.line.clone(),
            block_comment: self.comments.block.clone(),
            tab_size: self.tab_width,
            syntax_hint: self.syntax_hint().to_owned(),
        })
    }

    /// The effective indentation given the global defaults: the language's own
    /// when `custom_tab_mode` is on, else `default`.
    pub fn effective_indent(&self, default: IndentStyle) -> IndentStyle {
        if self.custom_tab_mode {
            IndentStyle {
                soft_tabs: self.soft_tabs,
                width: self.tab_width,
            }
        } else {
            default
        }
    }

    /// The message to show when the compile phase or the run command exits
    /// with `status`: the language's `errors` entry, and for 127 ("command not
    /// found") the installer hint.
    pub fn exit_status_message(&self, status: i32) -> Option<String> {
        if let Some(message) = self.errors.get(&status.to_string()) {
            return Some(message.clone());
        }
        if status == 127 {
            return self.install_hint();
        }
        None
    }

    /// "To run your code, you need to have the Go compiler installed. You can
    /// get it at <http://golang.org>", when the language has an installer hint.
    pub fn install_hint(&self) -> Option<String> {
        let installer = self.installer.as_ref()?;
        Some(format!(
            "To run your code, you need to have {} installed. You can get it at {}",
            installer.name, installer.url
        ))
    }

    /// Checks the definition for problems (see [`Issue`]). Built-ins are
    /// tested to have none; the registry refuses to load user definitions with
    /// errors.
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut warnings = Vec::new();
        let mut error = |message: String| issues.push(Issue::error(message));
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || self.id.starts_with('-')
        {
            error(format!("id `{}` must be a slug of [a-z0-9-]", self.id));
        }
        if self.name.trim().is_empty() {
            error("name is empty".to_owned());
        }
        for ext in &self.extensions {
            if ext.is_empty()
                || ext.starts_with('.')
                || ext.chars().any(|c| c.is_uppercase() || c.is_whitespace())
            {
                error(format!("extension `{ext}` must be lowercase without a dot"));
            }
        }
        for ext in &self.default_for {
            if !self.extensions.contains(ext) {
                error(format!("default_for `{ext}` is not in extensions"));
            }
        }
        if self.tab_width == 0 || self.tab_width > 16 {
            error(format!("tab_width {} is out of range", self.tab_width));
        }
        if self.editor_syntax.trim().is_empty() {
            error("editor_syntax is empty".to_owned());
        }
        let defaults = self.templates.iter().filter(|t| t.default).count();
        if defaults > 1 {
            error("more than one default template".to_owned());
        }
        for template in &self.templates {
            if template.default && template.name_on_creation {
                error(format!(
                    "template `{}` cannot be both default and name-on-creation",
                    template.name
                ));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for diagnostic in &self.diagnostics {
            if !seen.insert(diagnostic.id.as_str()) {
                error(format!("duplicate diagnostic id `{}`", diagnostic.id));
            }
        }
        for message in self.run.validate() {
            error(message);
        }
        let defined = self.run.defined_variables();
        let mut unknown: Vec<String> = self.run.referenced_variables().into_iter().collect();
        for diagnostic in &self.diagnostics {
            if let Some(command) = &diagnostic.command {
                unknown.extend(crate::expand::referenced_variables(command));
            }
        }
        unknown.retain(|name| {
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && !crate::vars::KNOWN_VARIABLES.contains(&name.as_str())
                && !defined.contains(name)
        });
        unknown.sort();
        unknown.dedup();
        for name in unknown {
            warnings.push(Issue::warning(format!("`${name}` is not a known variable")));
        }
        for entry in &self.content_patterns {
            let (_, pattern) = crate::detect::parse_content_pattern(entry);
            if let Err(e) = regex::Regex::new(pattern) {
                error(format!("content pattern `{pattern}`: {e}"));
            }
        }
        for diagnostic in &self.diagnostics {
            if let Some(pattern) = &diagnostic.regex
                && let Err(e) = regex::Regex::new(pattern)
            {
                // CodeRunner regexes may use features the `regex` crate lacks
                // (duplicate group names, look-around): only a warning.
                warnings.push(Issue::warning(format!(
                    "diagnostic `{}` regex is not supported by the `regex` crate: {e}",
                    diagnostic.id
                )));
            }
        }
        issues.extend(warnings);
        issues
    }
}

/// Splits a language name into `(group, variant)`: `JavaScript (Node.js)` ->
/// (`JavaScript`, `Node.js`); `Python 3.2` -> (`Python`, `3.2`); anything else
/// -> (name, None). These are CodeRunner's two grouping patterns.
pub fn split_group_variant(name: &str) -> (&str, Option<&str>) {
    let name = name.trim();
    if let Some(open) = name.rfind(" (")
        && name.ends_with(')')
        && open > 0
    {
        let group = name[..open].trim_end();
        let variant = &name[open + 2..name.len() - 1];
        if !group.is_empty() {
            return (group, Some(variant));
        }
    }
    if let Some(space) = name.rfind(' ') {
        let tail = &name[space + 1..];
        if !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
            && tail.chars().any(|c| c.is_ascii_digit())
            && space > 0
        {
            return (name[..space].trim_end(), Some(tail));
        }
    }
    (name, None)
}

/// A validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// How bad it is.
    pub severity: Severity,
    /// What is wrong.
    pub message: String,
}

impl Issue {
    pub(crate) fn error(message: String) -> Self {
        Self {
            severity: Severity::Error,
            message,
        }
    }

    pub(crate) fn warning(message: String) -> Self {
        Self {
            severity: Severity::Warning,
            message,
        }
    }
}

/// How serious an [`Issue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The definition must not be used.
    Error,
    /// Usable, but something may not work.
    Warning,
}

/// Comment markers (for Un/Comment Lines).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentMarkers {
    /// Line comment marker: `//`, `#`, `--`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Block comment `[open, close]`: `["/*", "*/"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<(String, String)>,
}

impl CommentMarkers {
    /// Whether no marker is set.
    pub fn is_empty(&self) -> bool {
        self.line.is_none() && self.block.is_none()
    }
}

/// Indentation style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentStyle {
    /// Spaces instead of tabs.
    pub soft_tabs: bool,
    /// Width of one level (tab stop / number of spaces).
    pub width: u8,
}

impl Default for IndentStyle {
    /// CodeRunner's defaults: tabs, width 4.
    fn default() -> Self {
        Self {
            soft_tabs: false,
            width: 4,
        }
    }
}

/// Data to register an editor language with NSH
/// (`EditorLanguage::new(name).aliases(..).extensions(..).line_comment(..)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRegistration {
    /// The editor language name (`editor_syntax`).
    pub name: String,
    /// Aliases (the language id).
    pub aliases: Vec<String>,
    /// Extensions without the dot.
    pub extensions: Vec<String>,
    /// Whole file names.
    pub file_names: Vec<String>,
    /// Line comment marker.
    pub line_comment: Option<String>,
    /// Block comment markers.
    pub block_comment: Option<(String, String)>,
    /// Indent width.
    pub tab_size: u8,
    /// The syntect syntax hint (`.syntax(hint)`).
    pub syntax_hint: String,
}

/// The languages NSH's editor knows by name (from `nsh-base/src/editor/language.rs`
/// at the pinned revision). Names match case-insensitively, also by alias.
pub const NSH_BUILTIN_EDITOR_LANGUAGES: &[&str] = &[
    "Plain Text",
    "Rust",
    "JavaScript",
    "JSX",
    "TypeScript",
    "TSX",
    "JSON",
    "Python",
    "Go",
    "C",
    "C++",
    "C#",
    "Java",
    "Kotlin",
    "Swift",
    "Objective-C",
    "Objective-C++",
    "Ruby",
    "PHP",
    "Lua",
    "Shell",
    "PowerShell",
    "SQL",
    "HTML",
    "XML",
    "Vue",
    "Svelte",
    "CSS",
    "SCSS",
    "Sass",
    "Less",
    "Markdown",
    "YAML",
    "TOML",
    "Dockerfile",
    "Makefile",
    "Dart",
    "Scala",
    "Elixir",
    "Haskell",
    "Zig",
    "GraphQL",
    "Diff",
    "Perl",
    "R",
    "Julia",
    "Erlang",
    "Clojure",
    "OCaml",
    "F#",
    "Nix",
    "Fish",
    "Protocol Buffers",
    "Terraform",
    "INI",
    "CMake",
];

/// Whether NSH's editor has a built-in language with this name.
pub fn is_nsh_builtin_editor_language(name: &str) -> bool {
    NSH_BUILTIN_EDITOR_LANGUAGES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name.trim()))
}

/// How the run output is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleType {
    /// A text console on a pty.
    #[default]
    Terminal,
    /// The program's stdout is a complete HTML document shown in a web view.
    Web,
    /// The program's stdout is an HTML fragment (`<script>`, `<style>`)
    /// injected into a host page.
    WebResource,
    /// The last output line is the path of a PDF to display.
    Pdf,
}

impl ConsoleType {
    /// CodeRunner's integer code (0 terminal, 1 web, 2 web resource, 3 pdf).
    pub fn to_cr(self) -> i64 {
        match self {
            Self::Terminal => 0,
            Self::Web => 1,
            Self::WebResource => 2,
            Self::Pdf => 3,
        }
    }

    /// Converts CodeRunner's integer code.
    pub fn from_cr(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Terminal,
            1 => Self::Web,
            2 => Self::WebResource,
            3 => Self::Pdf,
            _ => return None,
        })
    }

    /// The label of CodeRunner's "Console Type" popup.
    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Web => "Web",
            Self::WebResource => "Web Resource",
            Self::Pdf => "PDF",
        }
    }
}

/// Where the console sits for this language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolePosition {
    /// Below the editor.
    Bottom,
    /// Right of the editor.
    Right,
}

/// Console behaviour of a language.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSpec {
    /// How output is shown.
    #[serde(default, rename = "type")]
    pub kind: ConsoleType,
    /// The user may toggle between the terminal and the web view (Markdown, PHP).
    #[serde(default, skip_serializing_if = "is_false")]
    pub switchable: bool,
    /// Re-run automatically while typing and re-render.
    #[serde(default, skip_serializing_if = "is_false")]
    pub live: bool,
    /// Console position override; `None` = the global preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<ConsolePosition>,
    /// Name of an in-process renderer that replaces the run command when the
    /// run command is empty (`markdown`: render the document to HTML).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
}

impl ConsoleSpec {
    /// Whether every field has its default (so it is left out of TOML).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// How to get a missing toolchain (CodeRunner's `Installer` triple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerHint {
    /// The executable or package id (`go`, `python3`).
    pub executable: String,
    /// Human name used in messages (`the Go compiler`).
    pub name: String,
    /// Where to get it.
    pub url: String,
}

/// What a diagnostic entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    /// Run the language's compile phase with diagnostics flags (type 0).
    CompileScript,
    /// Run the `command` (type 1).
    Command,
    /// Parse the run's console output (type 2).
    ConsoleText,
    /// Only highlight/link the console output, no annotations (type 3).
    ConsoleHighlight,
    /// Built-in clang front end (type 100).
    Clang,
    /// Built-in: errors from the grammar (type 101).
    InvalidSyntax,
    /// Built-in: regex validity (type 102).
    RegexCheck,
    /// Built-in: version control conflict markers (type 105).
    Conflict,
}

impl DiagnosticKind {
    /// CodeRunner's integer code.
    pub fn to_cr(self) -> i64 {
        match self {
            Self::CompileScript => 0,
            Self::Command => 1,
            Self::ConsoleText => 2,
            Self::ConsoleHighlight => 3,
            Self::Clang => 100,
            Self::InvalidSyntax => 101,
            Self::RegexCheck => 102,
            Self::Conflict => 105,
        }
    }

    /// Converts CodeRunner's integer code (103/104 are unknown).
    pub fn from_cr(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::CompileScript,
            1 => Self::Command,
            2 => Self::ConsoleText,
            3 => Self::ConsoleHighlight,
            100 => Self::Clang,
            101 => Self::InvalidSyntax,
            102 => Self::RegexCheck,
            105 => Self::Conflict,
            _ => return None,
        })
    }

    /// Whether the entry is a built-in ("Built-in: ..." in CodeRunner).
    pub fn is_builtin(self) -> bool {
        self.to_cr() >= 100
    }
}

/// When a diagnostic runs (CodeRunner's `UpdateMode` bitmask: 1 edit, 2 run,
/// 4 manual).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMode {
    /// While editing (debounced; needs the "diagnostics while typing" pref).
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_edit: bool,
    /// Before/after a run.
    #[serde(default, skip_serializing_if = "is_false")]
    pub on_run: bool,
    /// Only when invoked by the user (with the entry's key equivalent).
    #[serde(default, skip_serializing_if = "is_false")]
    pub manual: bool,
}

impl UpdateMode {
    /// Decodes CodeRunner's bitmask.
    pub fn from_bits(bits: i64) -> Self {
        Self {
            on_edit: bits & 1 != 0,
            on_run: bits & 2 != 0,
            manual: bits & 4 != 0,
        }
    }

    /// Encodes CodeRunner's bitmask.
    pub fn bits(self) -> i64 {
        i64::from(self.on_edit) | i64::from(self.on_run) << 1 | i64::from(self.manual) << 2
    }
}

/// Diagnostic options (CodeRunner's `Options` bitmask: 1 custom flags, 2 add to
/// existing flags, 4 cancel run on new errors, 8 on warnings, 16 on notices).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticOptions {
    /// Use this entry's `flags` for the compile phase ("Use custom compile flags").
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom_flags: bool,
    /// Append `flags` to the language's flags instead of replacing them (only
    /// meaningful with `custom_flags`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub add_to_existing: bool,
    /// Cancel the run if new errors are produced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_on_errors: bool,
    /// Cancel the run if new warnings are produced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_on_warnings: bool,
    /// Cancel the run if new notices are produced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_on_notices: bool,
}

impl DiagnosticOptions {
    /// Decodes CodeRunner's bitmask.
    pub fn from_bits(bits: i64) -> Self {
        Self {
            custom_flags: bits & 1 != 0,
            add_to_existing: bits & 2 != 0,
            cancel_on_errors: bits & 4 != 0,
            cancel_on_warnings: bits & 8 != 0,
            cancel_on_notices: bits & 16 != 0,
        }
    }

    /// Encodes CodeRunner's bitmask.
    pub fn bits(self) -> i64 {
        i64::from(self.custom_flags)
            | i64::from(self.add_to_existing) << 1
            | i64::from(self.cancel_on_errors) << 2
            | i64::from(self.cancel_on_warnings) << 3
            | i64::from(self.cancel_on_notices) << 4
    }
}

/// How a diagnostic's output is interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// `file:line:col: message` (GCC/Unix style); built-in parser.
    #[default]
    Unix,
    /// A regular expression with named groups: `file`, `line`, `column`,
    /// `message`, `messageID`, `error`, `warning`, `note`, `zeroLine`,
    /// `zeroColumn`, `endLine`, `endColumn`, `offset`, `endOffset`, `length`,
    /// `code`.
    Regex,
    /// A custom script reads the output on stdin and prints JSON.
    CustomScript,
    /// CheckStyle XML.
    CheckStyle,
}

impl OutputMode {
    /// CodeRunner's integer code.
    pub fn to_cr(self) -> i64 {
        match self {
            Self::Unix => 0,
            Self::Regex => 1,
            Self::CustomScript => 2,
            Self::CheckStyle => 3,
        }
    }

    /// Converts CodeRunner's integer code.
    pub fn from_cr(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Unix,
            1 => Self::Regex,
            2 => Self::CustomScript,
            3 => Self::CheckStyle,
            _ => return None,
        })
    }
}

/// A linter / error source of a language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticDef {
    /// Slug, unique within the language (`console-text`, `syntax`).
    pub id: String,
    /// CodeRunner's uuid (kept for import/export).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Display name (`Python Syntax`).
    pub name: String,
    /// What the entry does.
    pub kind: DiagnosticKind,
    /// When it runs.
    #[serde(default)]
    pub update: UpdateMode,
    /// Options.
    #[serde(default)]
    pub options: DiagnosticOptions,
    /// How the output is parsed.
    #[serde(default)]
    pub output_mode: OutputMode,
    /// The regex for [`OutputMode::Regex`] (named groups, see [`OutputMode::Regex`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// For [`OutputMode::CustomScript`]: the script text (or its uuid when
    /// imported without the script file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_script: Option<String>,
    /// The command of [`DiagnosticKind::Command`], a run-command template
    /// (`$filename`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Compile flags used when `options.custom_flags` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Shortcut to run the entry manually (CodeRunner key-equivalent string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_equivalent: Option<String>,
}

impl DiagnosticDef {
    /// The compile flags this diagnostic run uses, given the language's
    /// current flags: custom flags replace (or, with `add_to_existing`, are
    /// appended to) them.
    pub fn effective_flags(&self, language_flags: &str) -> String {
        match (&self.flags, self.options.custom_flags) {
            (Some(own), true) if self.options.add_to_existing => {
                format!("{language_flags} {own}").trim().to_owned()
            }
            (Some(own), true) => own.clone(),
            _ => language_flags.to_owned(),
        }
    }
}

/// What a documentation entry does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocType {
    /// `{query}` is substituted into the URL.
    #[default]
    Search,
    /// A fixed page.
    Bookmark,
    /// Search, then follow the first result ("I'm feeling lucky").
    TextFind,
}

impl DocType {
    /// CodeRunner's integer code (0 search, 1 bookmark, 2 text find).
    pub fn to_cr(self) -> i64 {
        match self {
            Self::Search => 0,
            Self::Bookmark => 1,
            Self::TextFind => 2,
        }
    }

    /// Converts CodeRunner's integer code.
    pub fn from_cr(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Search,
            1 => Self::Bookmark,
            2 => Self::TextFind,
            _ => return None,
        })
    }
}

/// A documentation source of a language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocSource {
    /// Display name (`Mozilla MDN`).
    pub name: String,
    /// How it is used.
    #[serde(default, rename = "type")]
    pub kind: DocType,
    /// URL template. `{query}` is replaced by the percent-encoded search
    /// text and `{lang}` by the language name.
    pub url: String,
    /// POST body template (same placeholders); `None` = GET.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    /// Shown in the docs panel.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    /// Only for text in this scope selector (`source.php`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// CSS injected into pages of this source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// JavaScript injected into pages of this source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,
}

impl DocSource {
    /// The URL for looking up `query` (percent-encoded) for language `lang`.
    pub fn url_for(&self, query: &str, lang: &str) -> String {
        fill(&self.url, query, lang)
    }

    /// The POST body for `query`, when the source uses POST.
    pub fn post_body_for(&self, query: &str, lang: &str) -> Option<String> {
        self.post.as_deref().map(|body| fill(body, query, lang))
    }
}

fn fill(template: &str, query: &str, lang: &str) -> String {
    template
        .replace("{query}", &percent_encode(query))
        .replace("{lang}", &percent_encode(lang))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_and_variant_follow_coderunner_patterns() {
        assert_eq!(
            split_group_variant("JavaScript (Node.js)"),
            ("JavaScript", Some("Node.js"))
        );
        assert_eq!(
            split_group_variant("C# (.NET Script)"),
            ("C#", Some(".NET Script"))
        );
        assert_eq!(split_group_variant("Python 3.2"), ("Python", Some("3.2")));
        assert_eq!(
            split_group_variant("Objective-C++"),
            ("Objective-C++", None)
        );
        assert_eq!(split_group_variant("C"), ("C", None));
        assert_eq!(split_group_variant("(odd)"), ("(odd)", None));
    }

    #[test]
    fn group_prefers_explicit_field() {
        let mut def = LanguageDef::new("x", "Thing (Fast)");
        assert_eq!(def.group_name(), "Thing");
        assert_eq!(def.variant_name(), Some("Fast"));
        def.group = Some("Stuff".into());
        def.variant = Some("Quick".into());
        assert_eq!(def.group_name(), "Stuff");
        assert_eq!(def.variant_name(), Some("Quick"));
    }

    #[test]
    fn bit_masks_round_trip() {
        for bits in 0..8 {
            assert_eq!(UpdateMode::from_bits(bits).bits(), bits);
        }
        for bits in 0..32 {
            assert_eq!(DiagnosticOptions::from_bits(bits).bits(), bits);
        }
        let swift = DiagnosticOptions::from_bits(7);
        assert!(swift.custom_flags && swift.add_to_existing && swift.cancel_on_errors);
        assert!(!swift.cancel_on_warnings);
        let mode = UpdateMode::from_bits(3);
        assert!(mode.on_edit && mode.on_run && !mode.manual);
    }

    #[test]
    fn cr_codes_round_trip() {
        for code in 0..4 {
            assert_eq!(ConsoleType::from_cr(code).unwrap().to_cr(), code);
            assert_eq!(OutputMode::from_cr(code).unwrap().to_cr(), code);
        }
        for code in [0, 1, 2, 3, 100, 101, 102, 105] {
            assert_eq!(DiagnosticKind::from_cr(code).unwrap().to_cr(), code);
        }
        assert_eq!(DiagnosticKind::from_cr(103), None);
        assert_eq!(DocType::from_cr(2), Some(DocType::TextFind));
        assert_eq!(ConsoleType::from_cr(9), None);
    }

    #[test]
    fn effective_flags_replace_or_append() {
        let mut d = DiagnosticDef {
            id: "x".into(),
            uuid: None,
            name: "x".into(),
            kind: DiagnosticKind::CompileScript,
            update: UpdateMode::default(),
            options: DiagnosticOptions::default(),
            output_mode: OutputMode::Unix,
            regex: None,
            output_script: None,
            command: None,
            flags: Some("-typecheck".into()),
            key_equivalent: None,
        };
        assert_eq!(
            d.effective_flags("-O"),
            "-O",
            "flags ignored without the option"
        );
        d.options.custom_flags = true;
        assert_eq!(d.effective_flags("-O"), "-typecheck");
        d.options.add_to_existing = true;
        assert_eq!(d.effective_flags("-O"), "-O -typecheck");
        assert_eq!(d.effective_flags(""), "-typecheck");
    }

    #[test]
    fn doc_url_substitution_encodes() {
        let doc = DocSource {
            name: "Google".into(),
            kind: DocType::Search,
            url: "https://google.com/search?q={query}+{lang}".into(),
            post: Some("q={query}".into()),
            enabled: true,
            scope: None,
            css: None,
            js: None,
        };
        assert_eq!(
            doc.url_for("a b&c", "C++"),
            "https://google.com/search?q=a%20b%26c+C%2B%2B"
        );
        assert_eq!(doc.post_body_for("x y", "Go").as_deref(), Some("q=x%20y"));
    }

    #[test]
    fn exit_status_messages() {
        let mut def = LanguageDef::new("go", "Go");
        assert_eq!(def.exit_status_message(127), None);
        def.installer = Some(InstallerHint {
            executable: "go".into(),
            name: "the Go compiler".into(),
            url: "https://go.dev".into(),
        });
        assert!(
            def.exit_status_message(127)
                .unwrap()
                .contains("the Go compiler")
        );
        def.errors.insert("127".into(), "custom".into());
        assert_eq!(def.exit_status_message(127).as_deref(), Some("custom"));
        assert_eq!(def.exit_status_message(1), None);
    }

    #[test]
    fn validate_catches_bad_definitions() {
        let mut def = LanguageDef::new("Bad Id", "");
        def.extensions = vec![".py".into(), "PY".into()];
        def.default_for = vec!["zz".into()];
        let messages: Vec<_> = def.validate().into_iter().map(|i| i.message).collect();
        assert!(messages.iter().any(|m| m.contains("slug")));
        assert!(messages.iter().any(|m| m.contains("name is empty")));
        assert!(messages.iter().any(|m| m.contains("`.py`")));
        assert!(messages.iter().any(|m| m.contains("`PY`")));
        assert!(messages.iter().any(|m| m.contains("default_for")));
        assert!(LanguageDef::new("ok", "Ok").validate().is_empty());
    }

    #[test]
    fn editor_registration_only_for_unknown_syntaxes() {
        let mut def = LanguageDef::new("applescript", "AppleScript");
        def.editor_syntax = "AppleScript".into();
        def.syntax_hint = Some("applescript".into());
        def.extensions = vec!["applescript".into()];
        def.comments.line = Some("#".into());
        let reg = def.editor_registration().unwrap();
        assert_eq!(reg.name, "AppleScript");
        assert_eq!(reg.syntax_hint, "applescript");
        assert_eq!(reg.line_comment.as_deref(), Some("#"));
        def.editor_syntax = "Python".into();
        assert!(def.editor_registration().is_none());
        assert!(is_nsh_builtin_editor_language("c++"));
        assert!(!is_nsh_builtin_editor_language("LaTeX"));
    }

    #[test]
    fn effective_indent_uses_custom_only_when_asked() {
        let mut def = LanguageDef::new("hs", "Haskell");
        def.soft_tabs = true;
        def.tab_width = 2;
        let global = IndentStyle::default();
        assert_eq!(def.effective_indent(global), global);
        def.custom_tab_mode = true;
        assert_eq!(
            def.effective_indent(global),
            IndentStyle {
                soft_tabs: true,
                width: 2
            }
        );
    }
}
