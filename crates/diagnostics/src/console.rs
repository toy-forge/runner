//! Console error import: turn the output of a run (compiler errors, Python
//! tracebacks, rustc/go/clang/javac/node formats) into editor annotations
//! ("Include errors from the console in the editor") and into console
//! spans for colouring and linking.
//!
//! ```
//! use scratch_diagnostics::console::{import_console, import_into};
//! use scratch_diagnostics::{AnnotationSet, DocumentContext, DiagnosticsPrefs};
//!
//! let output = "Traceback (most recent call last):\n  File \"/work/a.py\", line 2, in <module>\n    1/0\nZeroDivisionError: division by zero\n";
//! let import = import_console(output, None);
//! assert_eq!(import.diagnostics.len(), 1);
//! assert!(!import.highlights.is_empty());
//!
//! let source = "x = 1\n1/0\n";
//! let path = std::path::Path::new("/work/a.py");
//! let ctx = DocumentContext::new(source).path(path);
//! let mut set = AnnotationSet::new();
//! import_into(&mut set, &ctx, &import, &DiagnosticsPrefs::default());
//! assert_eq!(set.counts().errors, 1);
//! ```

use scratch_language::{DiagnosticKind, LanguageDef, OutputMode};

use crate::annotation::{AnnotationSet, ChangeReport, DocumentContext, resolve_all};
use crate::model::{Diagnostic, MatchedLocation, Origin};
use crate::parse::{self, ParseOptions};
use crate::settings::DiagnosticsPrefs;
use crate::text::strip_ansi;

/// The `source` id console annotations carry in an [`AnnotationSet`].
pub const CONSOLE_SOURCE: &str = "console";

/// The result of importing console output.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleImport {
    /// The text everything refers to: the output without ANSI escapes. All
    /// `matched` ranges are byte ranges in it.
    pub text: String,
    /// Diagnostics found, in output order, all with
    /// [`Origin::Console`] and source [`CONSOLE_SOURCE`].
    pub diagnostics: Vec<Diagnostic>,
    /// Spans of `text` to colour and link: every `matched` range of the
    /// diagnostics and their children, plus the matches of highlight-only
    /// entries, sorted by position. A link of `Some("")` means "link to the
    /// diagnostic" (use [`crate::AnnotationId`] of the placed annotation).
    pub highlights: Vec<MatchedLocation>,
}

fn collect(d: &Diagnostic, out: &mut Vec<MatchedLocation>) {
    out.extend(d.matched.iter().cloned());
    for c in &d.children {
        collect(c, out);
    }
}

fn stamp(d: &mut Diagnostic) {
    d.source = Some(CONSOLE_SOURCE.to_owned());
    d.origin = Origin::Console;
    for c in &mut d.children {
        stamp(c);
    }
}

/// Parses console output. The language's console entries
/// (`ConsoleText`/`ConsoleHighlight` with a unix, regex or CheckStyle mode;
/// custom-script entries need the runner) are applied first, then every
/// built-in format ([`parse::parse_auto`]); duplicates are removed.
pub fn import_console(output: &str, language: Option<&LanguageDef>) -> ConsoleImport {
    let text = strip_ansi(output);
    let opts = ParseOptions::console(language.map(|l| l.id.as_str()));
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut highlight_only: Vec<MatchedLocation> = Vec::new();
    if let Some(lang) = language {
        for entry in &lang.diagnostics {
            if !matches!(entry.kind, DiagnosticKind::ConsoleText | DiagnosticKind::ConsoleHighlight)
                || entry.output_mode == OutputMode::CustomScript
            {
                continue;
            }
            let Ok(found) = parse::interpret(&text, entry.output_mode, entry.regex.as_deref(), &opts) else {
                continue;
            };
            if entry.kind == DiagnosticKind::ConsoleHighlight {
                for d in &found {
                    collect(d, &mut highlight_only);
                }
            } else {
                diagnostics.extend(found);
            }
        }
    }
    diagnostics.extend(parse::parse_auto(&text, &opts));
    let mut diagnostics = parse::dedupe(diagnostics);
    for d in &mut diagnostics {
        stamp(d);
    }
    let mut highlights = highlight_only;
    for d in &diagnostics {
        collect(d, &mut highlights);
    }
    highlights.sort_by_key(|m| (m.range.start, m.range.end));
    highlights.dedup_by(|a, b| a.range == b.range);
    ConsoleImport {
        text,
        diagnostics,
        highlights,
    }
}

/// Places the imported diagnostics that are about this document into `set`
/// (replacing the previous console annotations). With the preference
/// `from_console` off the console annotations are removed instead.
pub fn import_into(
    set: &mut AnnotationSet,
    ctx: &DocumentContext<'_>,
    import: &ConsoleImport,
    prefs: &DiagnosticsPrefs,
) -> ChangeReport {
    if !prefs.from_console {
        set.clear_origin(Origin::Console);
        return ChangeReport::default();
    }
    let (here, _foreign) = resolve_all(&import.diagnostics, ctx);
    set.replace_source(CONSOLE_SOURCE, Origin::Console, here)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ansi_is_stripped_before_parsing() {
        let out = "\u{1b}[1mmain.c:3:5: \u{1b}[31merror:\u{1b}[0m use of undeclared identifier 'x'\n";
        let import = import_console(out, None);
        assert_eq!(import.diagnostics.len(), 1);
        assert_eq!(import.diagnostics[0].message, "use of undeclared identifier 'x'");
        assert!(import.text.starts_with("main.c:3:5: error:"));
        let m = &import.highlights[0];
        assert_eq!(&import.text[m.range.clone()], "main.c:3:5: error: use of undeclared identifier 'x'");
    }

    #[test]
    fn only_this_documents_errors_become_annotations() {
        let out = "/work/a.c:2:3: error: bad\n/work/other.c:9:1: warning: elsewhere\n";
        let import = import_console(out, None);
        assert_eq!(import.diagnostics.len(), 2);
        let text = "int x;\n  y;\n";
        let path = Path::new("/work/a.c");
        let ctx = DocumentContext::new(text).path(path);
        let mut set = AnnotationSet::new();
        let r = import_into(&mut set, &ctx, &import, &DiagnosticsPrefs::default());
        assert_eq!(r.new.errors, 1);
        assert_eq!(set.counts().total(), 1);
        let a = set.visible()[0];
        assert_eq!(a.origin, Origin::Console);
        assert_eq!(&text[a.range.clone().unwrap()], "y");
        // The pref off removes them.
        let prefs = DiagnosticsPrefs { from_console: false, ..DiagnosticsPrefs::default() };
        import_into(&mut set, &ctx, &import, &prefs);
        assert_eq!(set.counts().total(), 0);
    }

    #[test]
    fn language_entries_add_highlight_only_spans() {
        let reg = scratch_language::LanguageRegistry::builtin();
        let python = reg.get("python").unwrap();
        let out = "Traceback (most recent call last):\n  File \"/w/a.py\", line 3, in <module>\n    f()\nNameError: name 'f' is not defined\n";
        let import = import_console(out, Some(python));
        assert_eq!(import.diagnostics.len(), 1);
        // The frame line is highlighted by the language's highlight-only entry
        // and by the traceback parser (deduplicated by range).
        let frame = out.find("  File").unwrap();
        assert!(import.highlights.iter().any(|h| h.range.start <= frame + 2 && h.range.end > frame + 10));
        let mut sorted = import.highlights.clone();
        sorted.dedup_by(|a, b| a.range == b.range);
        assert_eq!(sorted.len(), import.highlights.len());
    }
}
