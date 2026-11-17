//! Importing CodeRunner `.crLanguage` bundles.
//!
//! A `.crLanguage` is a directory with `Language.plist` and optional
//! `Scripts/compile.sh`, `Docs/<uuid>/{script.js,style.css}` and
//! `Diagnostics/<uuid>/parser`. [`from_cr_language_plist`] converts the plist
//! alone; [`import_cr_language_bundle`] also reads the scripts: a language with
//! `CompilerScript = true` becomes [`CompileSpec::Shell`] holding the script
//! text (the CodeRunner protocol, see [`crate::protocol`]). Export is our own
//! TOML ([`LanguageDef::to_toml_string`]).
//!
//! Keys that have no equivalent are dropped and reported in
//! [`Imported::warnings`]; unknown diagnostic types are skipped.

use std::collections::BTreeMap;
use std::path::Path;

use plist::{Dictionary, Value};

use crate::error::LanguageError;
use crate::grammar::grammar_info;
use crate::model::{
    ConsolePosition, ConsoleSpec, ConsoleType, DiagnosticDef, DiagnosticKind, DiagnosticOptions,
    DocSource, DocType, InstallerHint, LanguageDef, OutputMode, UpdateMode,
};
use crate::run::{CompileSpec, ShellSpec};
use crate::template::TemplateDef;

/// The result of an import.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The converted language (`builtin` is false; the id is a slug of the
    /// name, to be made unique by the registry).
    pub def: LanguageDef,
    /// Things that could not be converted.
    pub warnings: Vec<String>,
}

/// Turns a display name into an id slug: lowercase ASCII letters and digits,
/// other runs of characters become one `-`; `++` reads as `pp` and `#` as
/// `sharp`.
///
/// ```
/// use scratch_language::slugify;
/// assert_eq!(slugify("JavaScript (zx.mjs)"), "javascript-zx-mjs");
/// assert_eq!(slugify("C++"), "cpp");
/// assert_eq!(slugify("C# (Mono)"), "csharp-mono");
/// ```
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.replace("++", "pp").replace('#', "sharp").chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

/// Converts the bytes of a CodeRunner `Language.plist` (XML or binary).
/// `CompilerScript = true` yields an empty [`CompileSpec::Shell`] and a
/// warning; use [`import_cr_language_bundle`] to get the script too.
///
/// # Errors
///
/// [`LanguageError::Plist`] when the bytes are not a plist dictionary.
pub fn from_cr_language_plist(bytes: &[u8]) -> Result<Imported, LanguageError> {
    convert(bytes, &BundleFiles::default())
}

/// Imports a CodeRunner language from a path: a `.crLanguage` directory
/// ([`import_cr_language_bundle`]) or a bare `Language.plist` file
/// ([`from_cr_language_plist`]).
///
/// # Errors
///
/// As the two functions it calls.
pub fn import_cr_language_path(path: &Path) -> Result<Imported, LanguageError> {
    if path.is_dir() {
        import_cr_language_bundle(path)
    } else {
        let bytes = std::fs::read(path).map_err(|e| LanguageError::io("read", path, e))?;
        from_cr_language_plist(&bytes)
    }
}

/// Imports a `.crLanguage` directory.
///
/// # Errors
///
/// [`LanguageError::Io`] when `Language.plist` cannot be read,
/// [`LanguageError::Plist`] when it is not valid.
pub fn import_cr_language_bundle(dir: &Path) -> Result<Imported, LanguageError> {
    let plist_path = dir.join("Language.plist");
    let bytes =
        std::fs::read(&plist_path).map_err(|e| LanguageError::io("read", &plist_path, e))?;
    let mut files = BundleFiles::default();
    let script_path = dir.join("Scripts").join("compile.sh");
    files.compile_script = std::fs::read_to_string(script_path).ok();
    files.read_docs_and_parsers(dir);
    if let Ok(entries) = std::fs::read_dir(dir.join("Scripts")) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != "compile.sh" && !name.starts_with('.') {
                files.helper_scripts.push(name);
            }
        }
        files.helper_scripts.sort();
    }
    convert(&bytes, &files)
}

#[derive(Default)]
struct BundleFiles {
    compile_script: Option<String>,
    doc_js: BTreeMap<String, String>,
    doc_css: BTreeMap<String, String>,
    parsers: BTreeMap<String, String>,
    helper_scripts: Vec<String>,
}

impl BundleFiles {
    fn read_docs_and_parsers(&mut self, dir: &Path) {
        let each = |sub: &str| -> Vec<(String, std::path::PathBuf)> {
            std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .collect()
        };
        for (uuid, path) in each("Docs") {
            if let Ok(js) = std::fs::read_to_string(path.join("script.js")) {
                self.doc_js.insert(uuid.to_ascii_lowercase(), js);
            }
            if let Ok(css) = std::fs::read_to_string(path.join("style.css")) {
                self.doc_css.insert(uuid.to_ascii_lowercase(), css);
            }
        }
        for (uuid, path) in each("Diagnostics") {
            if let Ok(script) = std::fs::read_to_string(path.join("parser")) {
                self.parsers.insert(uuid.to_ascii_lowercase(), script);
            }
        }
    }
}

fn convert(bytes: &[u8], files: &BundleFiles) -> Result<Imported, LanguageError> {
    let value = Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| LanguageError::Plist(e.to_string()))?;
    let Value::Dictionary(dict) = value else {
        return Err(LanguageError::Plist(
            "the root is not a dictionary".to_owned(),
        ));
    };
    let mut warnings = Vec::new();

    let name = str_of(&dict, "Name").unwrap_or_else(|| "Untitled".to_owned());
    let mut id = slugify(&name);
    if id.is_empty() {
        id = "language".to_owned();
    }
    let mut def = LanguageDef::new(id, name);
    def.uuid = str_of(&dict, "uuid");
    def.enabled = bool_of(&dict, "Enabled").unwrap_or(true);
    def.extensions = array_of(&dict, "FileExtensions")
        .iter()
        .filter_map(|v| v.as_string())
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    def.grammar = str_of(&dict, "Grammar").unwrap_or_else(|| "text.plain".to_owned());
    match grammar_info(&def.grammar) {
        Some(info) => {
            def.editor_syntax = info.editor_syntax.to_owned();
            def.syntax_hint = info.syntax_hint.map(str::to_owned);
            def.comments = info.comments();
        }
        None => {
            if def.grammar != "text.plain" {
                warnings.push(format!(
                    "grammar `{}` has no known editor syntax; the language is edited as plain text",
                    def.grammar
                ));
            }
            def.syntax_hint = def.extensions.first().cloned();
        }
    }
    def.tab_width = int_of(&dict, "TabWidth")
        .and_then(|w| u8::try_from(w).ok())
        .filter(|w| (1..=16).contains(w))
        .unwrap_or(4);
    def.soft_tabs = bool_of(&dict, "SoftTabs").unwrap_or(false);
    def.custom_tab_mode = bool_of(&dict, "CustomTabMode").unwrap_or(false);
    def.output_encoding = int_of(&dict, "OutputEncoding").unwrap_or(0);
    def.debugger = str_of(&dict, "Debugger").filter(|s| !s.is_empty());
    def.requires_external_components =
        bool_of(&dict, "RequiresExternalComponents").unwrap_or(false);
    def.bundle_version = int_of(&dict, "LanguageBundleVersion")
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    def.builtin_diagnostics = !bool_of(&dict, "DiagnosticsDontUseBuiltins").unwrap_or(false);

    def.run.command = str_of(&dict, "RunCommand").unwrap_or_default();
    def.run.flags = str_of(&dict, "CompilerFlags").unwrap_or_default();
    if bool_of(&dict, "CompilerScript").unwrap_or(false) {
        let script = match &files.compile_script {
            Some(script) => script.clone(),
            None => {
                warnings.push(
                    "the language uses a compile script but Scripts/compile.sh was not available"
                        .to_owned(),
                );
                String::new()
            }
        };
        def.run.compile = CompileSpec::Shell(ShellSpec { script });
    }
    if !files.helper_scripts.is_empty() {
        warnings.push(format!(
            "helper scripts in Scripts/ are not imported ($scripts_dir files): {}",
            files.helper_scripts.join(", ")
        ));
    }

    let console_type = int_of(&dict, "ConsoleType").unwrap_or(0);
    def.console = ConsoleSpec {
        kind: ConsoleType::from_cr(console_type).unwrap_or_else(|| {
            warnings.push(format!("unknown ConsoleType {console_type}"));
            ConsoleType::Terminal
        }),
        switchable: bool_of(&dict, "ConsoleTypeSwitchable").unwrap_or(false),
        live: bool_of(&dict, "LiveConsole").unwrap_or(false),
        position: match int_of(&dict, "ConsolePosition") {
            Some(1) => Some(ConsolePosition::Bottom),
            Some(2) => Some(ConsolePosition::Right),
            _ => None,
        },
        renderer: None,
    };

    let installer: Vec<String> = array_of(&dict, "Installer")
        .iter()
        .filter_map(|v| v.as_string().map(str::to_owned))
        .collect();
    if let [executable, name, url] = installer.as_slice() {
        def.installer = Some(InstallerHint {
            executable: executable.clone(),
            name: name.clone(),
            url: url.clone(),
        });
    }
    if let Some(Value::Dictionary(errors)) = dict.get("Errors") {
        for (status, message) in errors {
            if let Some(message) = message.as_string() {
                def.errors.insert(status.clone(), message.to_owned());
            }
        }
    }

    for item in array_of(&dict, "Templates") {
        let Value::Dictionary(t) = item else { continue };
        def.templates.push(TemplateDef {
            name: str_of(t, "Name").unwrap_or_default(),
            extension: str_of(t, "Extension").unwrap_or_default(),
            text: str_of(t, "Template").unwrap_or_default(),
            default: bool_of(t, "Default").unwrap_or(false),
            name_on_creation: bool_of(t, "NameOnCreation").unwrap_or(false),
        });
    }

    for item in array_of(&dict, "Docs") {
        let Value::Dictionary(d) = item else { continue };
        let uuid = str_of(d, "uuid").map(|u| u.to_ascii_lowercase());
        let kind_code = int_of(d, "Type").unwrap_or(0);
        def.docs.push(DocSource {
            name: str_of(d, "Name").unwrap_or_default(),
            kind: DocType::from_cr(kind_code).unwrap_or_else(|| {
                warnings.push(format!("unknown documentation type {kind_code}"));
                DocType::Search
            }),
            url: str_of(d, "URL").unwrap_or_default(),
            post: str_of(d, "POST").filter(|p| !p.is_empty()),
            enabled: bool_of(d, "Enabled").unwrap_or(true),
            scope: str_of(d, "Scope").filter(|s| !s.is_empty()),
            css: uuid.as_ref().and_then(|u| files.doc_css.get(u)).cloned(),
            js: uuid.as_ref().and_then(|u| files.doc_js.get(u)).cloned(),
        });
    }

    for item in array_of(&dict, "Diagnostics") {
        let Value::Dictionary(d) = item else { continue };
        let name = str_of(d, "Name").unwrap_or_default();
        let type_code = int_of(d, "Type").unwrap_or(1);
        let Some(kind) = DiagnosticKind::from_cr(type_code) else {
            warnings.push(format!(
                "diagnostic `{name}` has unknown type {type_code} and was skipped"
            ));
            continue;
        };
        let mode_code = int_of(d, "OutputMode").unwrap_or(0);
        let output_mode = OutputMode::from_cr(mode_code).unwrap_or_else(|| {
            warnings.push(format!(
                "diagnostic `{name}`: unknown output mode {mode_code}"
            ));
            OutputMode::Unix
        });
        let script_uuid = str_of(d, "OutputScriptUUID").filter(|s| !s.is_empty());
        let output_script = script_uuid.map(|uuid| {
            files
                .parsers
                .get(&uuid.to_ascii_lowercase())
                .cloned()
                .unwrap_or(uuid)
        });
        let base = slugify(&name);
        let base = if base.is_empty() {
            "diagnostic".to_owned()
        } else {
            base
        };
        let mut id = base.clone();
        let mut n = 2;
        while def.diagnostics.iter().any(|existing| existing.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        def.diagnostics.push(DiagnosticDef {
            id,
            uuid: str_of(d, "uuid"),
            name,
            kind,
            update: UpdateMode::from_bits(int_of(d, "UpdateMode").unwrap_or(0)),
            options: DiagnosticOptions::from_bits(int_of(d, "Options").unwrap_or(0)),
            output_mode,
            regex: str_of(d, "OutputRegex").filter(|r| !r.is_empty()),
            output_script,
            command: str_of(d, "Command").filter(|c| !c.is_empty()),
            flags: str_of(d, "CompilerFlags").filter(|f| !f.is_empty()),
            key_equivalent: str_of(d, "KeyEquivalent").filter(|k| !k.is_empty()),
        });
    }

    const KNOWN: &[&str] = &[
        "Name",
        "uuid",
        "Enabled",
        "FileExtensions",
        "Grammar",
        "RunCommand",
        "CompilerScript",
        "CompilerFlags",
        "ConsoleType",
        "ConsoleTypeSwitchable",
        "LiveConsole",
        "ConsolePosition",
        "OutputEncoding",
        "TabWidth",
        "SoftTabs",
        "CustomTabMode",
        "Debugger",
        "RequiresExternalComponents",
        "Installer",
        "Errors",
        "Templates",
        "Diagnostics",
        "DiagnosticsDontUseBuiltins",
        "Docs",
        "LanguageBundleVersion",
        "LastUsed",
        "UseCount",
    ];
    for key in dict.keys() {
        if !KNOWN.contains(&key.as_str()) {
            warnings.push(format!("unknown key `{key}` was ignored"));
        }
    }
    Ok(Imported { def, warnings })
}

fn str_of(dict: &Dictionary, key: &str) -> Option<String> {
    match dict.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

fn int_of(dict: &Dictionary, key: &str) -> Option<i64> {
    match dict.get(key)? {
        Value::Integer(i) => i.as_signed(),
        Value::Real(r) => Some(*r as i64),
        Value::Boolean(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// CodeRunner stores booleans as booleans, integers or `"YES"` strings.
fn bool_of(dict: &Dictionary, key: &str) -> Option<bool> {
    match dict.get(key)? {
        Value::Boolean(b) => Some(*b),
        Value::Integer(i) => Some(i.as_signed().unwrap_or(0) != 0),
        Value::String(s) => Some(matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "1"
        )),
        _ => None,
    }
}

fn array_of<'a>(dict: &'a Dictionary, key: &str) -> &'a [Value] {
    match dict.get(key) {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Name</key><string>My Lang (Fast)</string>
<key>uuid</key><string>AAAAAAAA-0000-0000-0000-000000000001</string>
<key>Enabled</key><string>YES</string>
<key>FileExtensions</key><array><string>.ML</string><string>mlx</string></array>
<key>Grammar</key><string>source.python</string>
<key>RunCommand</key><string>ml $filename</string>
<key>CompilerFlags</key><string>-O2</string>
<key>CompilerScript</key><true/>
<key>ConsoleType</key><integer>3</integer>
<key>TabWidth</key><integer>2</integer>
<key>SoftTabs</key><integer>1</integer>
<key>CustomTabMode</key><true/>
<key>Installer</key><array><string>ml</string><string>ML</string><string>https://ml.example</string></array>
<key>Errors</key><dict><key>127</key><string>no ml</string></dict>
<key>Templates</key><array><dict>
  <key>Name</key><string>Main</string><key>Extension</key><string>ml</string>
  <key>Template</key><string>main &lt;##&gt;</string><key>Default</key><integer>1</integer>
  <key>NameOnCreation</key><false/></dict></array>
<key>Diagnostics</key><array>
 <dict><key>Name</key><string>Lint</string><key>Type</key><integer>1</integer>
  <key>UpdateMode</key><integer>5</integer><key>Options</key><integer>6</integer>
  <key>OutputMode</key><integer>1</integer><key>OutputRegex</key><string>^(?&lt;file&gt;.*):(?&lt;line&gt;\d+)$</string>
  <key>Command</key><string>ml -c $filename</string><key>OutputScriptUUID</key><string></string></dict>
 <dict><key>Name</key><string>Lint</string><key>Type</key><integer>103</integer></dict>
 <dict><key>Name</key><string>Lint</string><key>Type</key><integer>100</integer></dict>
</array>
<key>Docs</key><array><dict><key>Name</key><string>Docs</string><key>URL</key><string>https://d/?q={query}</string>
  <key>Type</key><integer>2</integer><key>POST</key><string></string><key>Enabled</key><true/></dict></array>
<key>Surprise</key><string>x</string>
</dict></plist>"#;

    #[test]
    fn converts_all_fields() {
        let imported = from_cr_language_plist(SAMPLE.as_bytes()).unwrap();
        let d = &imported.def;
        assert_eq!(d.id, "my-lang-fast");
        assert_eq!(d.name, "My Lang (Fast)");
        assert_eq!(d.group_name(), "My Lang");
        assert_eq!(
            d.uuid.as_deref(),
            Some("AAAAAAAA-0000-0000-0000-000000000001")
        );
        assert!(d.enabled && !d.builtin);
        assert_eq!(d.extensions, ["ml", "mlx"]);
        assert_eq!(d.editor_syntax, "Python");
        assert_eq!(d.comments.line.as_deref(), Some("#"));
        assert_eq!(d.run.command, "ml $filename");
        assert_eq!(d.run.flags, "-O2");
        assert!(matches!(&d.run.compile, CompileSpec::Shell(s) if s.script.is_empty()));
        assert_eq!(d.console.kind, ConsoleType::Pdf);
        assert_eq!(
            (d.tab_width, d.soft_tabs, d.custom_tab_mode),
            (2, true, true)
        );
        assert_eq!(d.installer.as_ref().unwrap().url, "https://ml.example");
        assert_eq!(d.errors.get("127").map(String::as_str), Some("no ml"));
        assert_eq!(d.templates.len(), 1);
        assert!(d.templates[0].default && !d.templates[0].name_on_creation);
        assert_eq!(d.docs[0].kind, DocType::TextFind);
        assert!(d.docs[0].post.is_none());

        assert_eq!(d.diagnostics.len(), 2, "type 103 is skipped");
        assert_eq!(d.diagnostics[0].id, "lint");
        assert_eq!(d.diagnostics[1].id, "lint-2");
        assert_eq!(d.diagnostics[0].kind, DiagnosticKind::Command);
        assert!(
            d.diagnostics[0].update.on_edit
                && !d.diagnostics[0].update.on_run
                && d.diagnostics[0].update.manual
        );
        assert!(
            d.diagnostics[0].options.add_to_existing && d.diagnostics[0].options.cancel_on_errors
        );
        assert_eq!(d.diagnostics[0].output_mode, OutputMode::Regex);
        assert_eq!(d.diagnostics[1].kind, DiagnosticKind::Clang);

        assert!(imported.warnings.iter().any(|w| w.contains("compile.sh")));
        assert!(imported.warnings.iter().any(|w| w.contains("type 103")));
        assert!(imported.warnings.iter().any(|w| w.contains("Surprise")));
        // the only problem is the compile script that a plist alone cannot carry
        let errors: Vec<_> = d
            .validate()
            .into_iter()
            .filter(|i| i.severity == crate::model::Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].message.contains("empty script"));
    }

    #[test]
    fn imported_language_survives_a_toml_round_trip() {
        let imported = from_cr_language_plist(SAMPLE.as_bytes()).unwrap();
        let text = imported.def.to_toml_string().unwrap();
        assert_eq!(LanguageDef::from_toml_str(&text).unwrap(), imported.def);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            from_cr_language_plist(b"not a plist"),
            Err(LanguageError::Plist(_))
        ));
        assert!(matches!(
            from_cr_language_plist(
                b"<?xml version=\"1.0\"?><plist version=\"1.0\"><array/></plist>"
            ),
            Err(LanguageError::Plist(_))
        ));
    }

    #[test]
    fn bundle_import_reads_scripts_docs_and_parsers() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("My.crLanguage");
        std::fs::create_dir_all(bundle.join("Scripts")).unwrap();
        std::fs::create_dir_all(bundle.join("Docs/D0C-1")).unwrap();
        std::fs::create_dir_all(bundle.join("Diagnostics/PARSER-1")).unwrap();
        let plist = r#"<?xml version="1.0"?><plist version="1.0"><dict>
<key>Name</key><string>My</string><key>CompilerScript</key><true/>
<key>Docs</key><array><dict><key>Name</key><string>D</string><key>URL</key><string>u</string><key>uuid</key><string>D0C-1</string></dict></array>
<key>Diagnostics</key><array><dict><key>Name</key><string>P</string><key>Type</key><integer>2</integer>
 <key>OutputMode</key><integer>2</integer><key>OutputScriptUUID</key><string>PARSER-1</string></dict></array>
</dict></plist>"#;
        std::fs::write(bundle.join("Language.plist"), plist).unwrap();
        std::fs::write(bundle.join("Scripts/compile.sh"), "echo hi\n").unwrap();
        std::fs::write(bundle.join("Scripts/helper.pl"), "").unwrap();
        std::fs::write(bundle.join("Docs/D0C-1/style.css"), "body{}").unwrap();
        std::fs::write(bundle.join("Docs/D0C-1/script.js"), "1").unwrap();
        std::fs::write(bundle.join("Diagnostics/PARSER-1/parser"), "#!/bin/sh\n").unwrap();
        let imported = import_cr_language_bundle(&bundle).unwrap();
        let d = imported.def;
        assert!(matches!(&d.run.compile, CompileSpec::Shell(s) if s.script == "echo hi\n"));
        assert_eq!(d.docs[0].css.as_deref(), Some("body{}"));
        assert_eq!(d.docs[0].js.as_deref(), Some("1"));
        assert_eq!(
            d.diagnostics[0].output_script.as_deref(),
            Some("#!/bin/sh\n")
        );
        assert!(imported.warnings.iter().any(|w| w.contains("helper.pl")));
        assert!(!imported.warnings.iter().any(|w| w.contains("compile.sh")));
    }

    #[test]
    fn bundle_without_plist_is_an_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            import_cr_language_bundle(tmp.path()),
            Err(LanguageError::Io { .. })
        ));
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("C# (.NET Script)"), "csharp-net-script");
        assert_eq!(slugify("  --  "), "");
        assert_eq!(slugify("Python 3"), "python-3");
    }
}
