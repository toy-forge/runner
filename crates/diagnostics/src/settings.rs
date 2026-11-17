//! Diagnostics preferences and per-document diagnostics data (what the
//! Preferences > Text Editing > Diagnostics pane and the Diagnostics menu
//! edit). Everything is serde data so the app can persist it anywhere (the
//! per-document part replaces CodeRunner's `CodeRunner:Diagnostics` xattr).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use scratch_language::{DiagnosticDef, DiagnosticKind, LanguageDef, UpdateMode};

use crate::ignore::RuleList;

fn yes() -> bool {
    true
}

fn default_delay() -> f64 {
    1.5
}

fn default_extra_clang() -> String {
    "-Wno-unused-command-line-argument".to_owned()
}

/// Global diagnostics preferences. Field names follow CodeRunner's keys
/// (`DiagnosticsWhileTyping`, `DiagnosticsDelay`, ...); defaults are its
/// defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsPrefs {
    /// `DiagnosticsWhileTyping`: lint while typing.
    pub while_typing: bool,
    /// `DiagnosticsDelay`: seconds of idle time before an on-edit lint.
    pub delay_secs: f64,
    /// `DiagnosticsInvalidSyntax`: run the built-in invalid syntax checker.
    pub invalid_syntax: bool,
    /// `DiagnosticsInvalidSyntaxLimitToOne`: only the first syntax error.
    pub invalid_syntax_limit_to_one: bool,
    /// `DiagnosticsFromConsole`: import errors from the console output.
    pub from_console: bool,
    /// `DiagnosticsAnimate`.
    pub animate: bool,
    /// `DiagnosticsHighlightLine`.
    pub highlight_line: bool,
    /// `DiagnosticsUnderline`.
    pub underline: bool,
    /// `SymbolListIncludesEditorAnnotations`.
    pub include_in_symbol_list: bool,
    /// `DiagnosticsIconOnly`: icon only instead of full messages.
    pub icon_only: bool,
    /// `IgnoredDiagnostics`: global ignore rules.
    pub ignored: RuleList,
    /// `AutoFixDiagnostics`: messages fixed automatically on run.
    pub auto_fix: RuleList,
    /// `AutomaticClangFlags`.
    pub automatic_clang_flags: bool,
    /// `ClangFlagsFromUser`.
    pub clang_flags_from_user: bool,
    /// `ExtraClangFlags`.
    pub extra_clang_flags: String,
    /// `ExtraSwiftFlags`.
    pub extra_swift_flags: String,
    /// "Disable Diagnostics Here > All Diagnostics".
    pub disabled_all: bool,
    /// "Disable ... Language": language ids.
    pub disabled_languages: BTreeSet<String>,
    /// "Disable ... From diagnostic": `language-id/diagnostic-id`.
    pub disabled_diagnostics: BTreeSet<String>,
    /// Per-diagnostic update mode chosen in the Diagnostics menu or the
    /// language preferences: `language-id/diagnostic-id` -> bits (1 edit,
    /// 2 run, 4 manual).
    pub update_overrides: BTreeMap<String, u8>,
    /// Console diagnostics set to "Console Highlight Only"
    /// (`language-id/diagnostic-id`).
    pub console_highlight_only: BTreeSet<String>,
}

impl Default for DiagnosticsPrefs {
    fn default() -> Self {
        Self {
            while_typing: yes(),
            delay_secs: default_delay(),
            invalid_syntax: yes(),
            invalid_syntax_limit_to_one: yes(),
            from_console: yes(),
            animate: yes(),
            highlight_line: yes(),
            underline: yes(),
            include_in_symbol_list: yes(),
            icon_only: false,
            ignored: RuleList::default(),
            auto_fix: RuleList::default(),
            automatic_clang_flags: yes(),
            clang_flags_from_user: yes(),
            extra_clang_flags: default_extra_clang(),
            extra_swift_flags: String::new(),
            disabled_all: false,
            disabled_languages: BTreeSet::new(),
            disabled_diagnostics: BTreeSet::new(),
            update_overrides: BTreeMap::new(),
            console_highlight_only: BTreeSet::new(),
        }
    }
}

fn key(language: &str, diagnostic: &str) -> String {
    format!("{language}/{diagnostic}")
}

impl DiagnosticsPrefs {
    /// The edit debounce delay.
    pub fn delay(&self) -> Duration {
        Duration::from_secs_f64(self.delay_secs.clamp(0.0, 60.0))
    }

    /// The update mode of a diagnostic after user overrides.
    pub fn update_mode(&self, language: &str, def: &DiagnosticDef) -> UpdateMode {
        match self.update_overrides.get(&key(language, &def.id)) {
            Some(bits) => UpdateMode::from_bits(i64::from(*bits)),
            None => def.update,
        }
    }

    /// Stores an update mode override (`None` removes it).
    pub fn set_update_mode(&mut self, language: &str, diagnostic: &str, mode: Option<UpdateMode>) {
        let k = key(language, diagnostic);
        match mode {
            Some(m) => {
                self.update_overrides
                    .insert(k, u8::try_from(m.bits()).unwrap_or(0));
            }
            None => {
                self.update_overrides.remove(&k);
            }
        }
    }

    /// Whether a console diagnostic only highlights (no editor annotations).
    pub fn console_highlight_only(&self, language: &str, def: &DiagnosticDef) -> bool {
        def.kind == DiagnosticKind::ConsoleHighlight
            || self
                .console_highlight_only
                .contains(&key(language, &def.id))
    }

    /// Applies one "Disable Diagnostics Here" choice.
    pub fn disable(&mut self, scope: &DisableScope) {
        match scope {
            DisableScope::Document => {}
            DisableScope::Diagnostic { language, id } => {
                self.disabled_diagnostics.insert(key(language, id));
            }
            DisableScope::Language(language) => {
                self.disabled_languages.insert(language.clone());
            }
            DisableScope::All => self.disabled_all = true,
        }
    }

    /// Undoes [`DiagnosticsPrefs::disable`] ("Enable Diagnostics For ...").
    pub fn enable(&mut self, scope: &DisableScope) {
        match scope {
            DisableScope::Document => {}
            DisableScope::Diagnostic { language, id } => {
                self.disabled_diagnostics.remove(&key(language, id));
            }
            DisableScope::Language(language) => {
                self.disabled_languages.remove(language);
            }
            DisableScope::All => self.disabled_all = false,
        }
    }

    /// Whether the diagnostic `diagnostic` of `language` runs for a
    /// document with `doc` data.
    pub fn is_enabled(&self, language: &str, diagnostic: &str, doc: &DocumentDiagnosticsData) -> bool {
        !(self.disabled_all
            || doc.disabled
            || self.disabled_languages.contains(language)
            || self.disabled_diagnostics.contains(&key(language, diagnostic))
            || doc.disabled_diagnostics.contains(&key(language, diagnostic)))
    }
}

/// The scopes of the "Disable Diagnostics Here" submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisableScope {
    /// This document (store in [`DocumentDiagnosticsData::disabled`]).
    Document,
    /// One diagnostic of a language ("From \"Clang\"").
    Diagnostic {
        /// Language id.
        language: String,
        /// Diagnostic id.
        id: String,
    },
    /// All diagnostics of a language.
    Language(String),
    /// Everything.
    All,
}

/// Diagnostics state saved with a document (the ignored messages and
/// disabled diagnostics of that document).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentDiagnosticsData {
    /// "Ignored Messages For Document...".
    pub ignored: RuleList,
    /// All diagnostics disabled for this document.
    pub disabled: bool,
    /// `language-id/diagnostic-id` disabled for this document.
    pub disabled_diagnostics: BTreeSet<String>,
}

impl DocumentDiagnosticsData {
    /// Disables one diagnostic for this document.
    pub fn disable_diagnostic(&mut self, language: &str, id: &str) {
        self.disabled_diagnostics.insert(key(language, id));
    }

    /// Re-enables one diagnostic for this document.
    pub fn enable_diagnostic(&mut self, language: &str, id: &str) {
        self.disabled_diagnostics.remove(&key(language, id));
    }

    /// Whether there is anything worth saving.
    pub fn is_empty(&self) -> bool {
        self.ignored.is_empty() && !self.disabled && self.disabled_diagnostics.is_empty()
    }
}

/// The diagnostics of `language` including the built-in ones CodeRunner adds
/// unless `builtin_diagnostics` is off: version control conflicts for every
/// language, invalid syntax where a checker exists (`json`, `toml`, `xml`),
/// and the regex check for regex documents. Entries the language already
/// lists (same kind) are not duplicated.
pub fn effective_diagnostics(language: &LanguageDef, prefs: &DiagnosticsPrefs) -> Vec<DiagnosticDef> {
    let mut list = language.diagnostics.clone();
    if !language.builtin_diagnostics {
        return list;
    }
    let has = |list: &[DiagnosticDef], kind: DiagnosticKind| list.iter().any(|d| d.kind == kind);
    let builtin = |id: &str, name: &str, kind, update| DiagnosticDef {
        id: id.to_owned(),
        uuid: None,
        name: name.to_owned(),
        kind,
        update,
        options: scratch_language::DiagnosticOptions::default(),
        output_mode: scratch_language::OutputMode::Unix,
        regex: None,
        output_script: None,
        command: None,
        flags: None,
        key_equivalent: None,
    };
    let edit_run = UpdateMode {
        on_edit: true,
        on_run: true,
        manual: false,
    };
    if !has(&list, DiagnosticKind::Conflict) {
        list.push(builtin(
            "builtin-conflict",
            "Version Control Conflicts",
            DiagnosticKind::Conflict,
            edit_run,
        ));
    }
    if prefs.invalid_syntax
        && !has(&list, DiagnosticKind::InvalidSyntax)
        && crate::builtin::has_syntax_checker(language)
    {
        list.push(builtin(
            "builtin-invalid-syntax",
            "Invalid Syntax",
            DiagnosticKind::InvalidSyntax,
            edit_run,
        ));
    }
    if !has(&list, DiagnosticKind::RegexCheck) && crate::builtin::is_regex_language(language) {
        list.push(builtin(
            "builtin-regex",
            "Regex Invalid",
            DiagnosticKind::RegexCheck,
            edit_run,
        ));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_coderunner() {
        let p = DiagnosticsPrefs::default();
        assert!(p.while_typing && p.invalid_syntax && p.from_console);
        assert_eq!(p.delay(), Duration::from_millis(1500));
        assert_eq!(p.extra_clang_flags, "-Wno-unused-command-line-argument");
        let json = serde_json::to_string(&p).unwrap();
        let back: DiagnosticsPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        let partial: DiagnosticsPrefs = serde_json::from_str(r#"{"delay_secs": 3.0}"#).unwrap();
        assert!(partial.while_typing);
        assert_eq!(partial.delay(), Duration::from_secs(3));
    }

    #[test]
    fn enablement_scopes() {
        let mut prefs = DiagnosticsPrefs::default();
        let mut doc = DocumentDiagnosticsData::default();
        assert!(prefs.is_enabled("c", "clang", &doc));
        prefs.disable(&DisableScope::Diagnostic {
            language: "c".into(),
            id: "clang".into(),
        });
        assert!(!prefs.is_enabled("c", "clang", &doc));
        assert!(prefs.is_enabled("cpp", "clang", &doc));
        prefs.enable(&DisableScope::Diagnostic {
            language: "c".into(),
            id: "clang".into(),
        });
        doc.disable_diagnostic("c", "clang");
        assert!(!prefs.is_enabled("c", "clang", &doc));
        assert!(!doc.is_empty());
        doc.enable_diagnostic("c", "clang");
        prefs.disable(&DisableScope::Language("c".into()));
        assert!(!prefs.is_enabled("c", "clang", &doc));
        prefs.disable(&DisableScope::All);
        assert!(!prefs.is_enabled("go", "x", &doc));
    }

    #[test]
    fn update_override() {
        let reg = scratch_language::LanguageRegistry::builtin();
        let c = reg.get("c").unwrap();
        let clang = &c.diagnostics[0];
        let mut prefs = DiagnosticsPrefs::default();
        assert!(prefs.update_mode("c", clang).on_edit);
        prefs.set_update_mode("c", &clang.id, Some(UpdateMode { on_edit: false, on_run: false, manual: true }));
        let m = prefs.update_mode("c", clang);
        assert!(m.manual && !m.on_edit);
        prefs.set_update_mode("c", &clang.id, None);
        assert!(prefs.update_mode("c", clang).on_edit);
    }
}
