//! The server catalogue and keyword tables must agree with the language definitions in
//! `crates/language`: every language has a server-free fallback, and no server names a language
//! that does not exist.

use scratch_language::LanguageRegistry;
use scratch_lsp::{LanguageTables, ServerCatalog};

#[test]
fn every_builtin_language_has_a_static_table_and_extension() {
    let registry = LanguageRegistry::builtin();
    let tables = LanguageTables::builtin();
    for language in registry.all() {
        let table = tables
            .get(&language.id)
            .unwrap_or_else(|| panic!("no keyword table for {}", language.id));
        assert!(
            table
                .extensions
                .iter()
                .any(|extension| language.extensions.contains(extension)),
            "{} shares no extension with its table ({:?} vs {:?})",
            language.id,
            language.extensions,
            table.extensions
        );
        assert_eq!(
            tables.extension(&language.id),
            language.extensions.first().map(String::as_str),
            "{} extension",
            language.id
        );
    }
}

#[test]
fn every_server_serves_a_real_language() {
    let registry = LanguageRegistry::builtin();
    let catalog = ServerCatalog::builtin();
    for definition in catalog.definitions() {
        for language in definition.languages.keys() {
            assert!(
                registry.get(language).is_some(),
                "server {} names unknown language {language}",
                definition.id
            );
            assert!(
                definition.virtual_path(language).is_some(),
                "server {} has no virtual path for {language}",
                definition.id
            );
        }
    }
    // The languages the app advertises as server-backed in its status bar.
    for language in [
        "python",
        "rust",
        "go",
        "javascript-node",
        "typescript-node",
        "shell",
        "swift",
        "c",
        "cpp",
        "objective-c",
        "java",
        "php",
        "ruby",
        "lua",
        "kotlin",
        "html",
        "css",
        "json",
        "yaml",
        "toml",
        "xml",
        "markdown",
        "latex",
        "haskell",
    ] {
        assert!(
            catalog.server_for_language(language).is_some(),
            "no server for {language}"
        );
    }
}
