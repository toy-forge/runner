//! The language definitions embedded in the binary (`defs/*.toml`).
//!
//! 29 languages mirror the ones CodeRunner 4.6 bundles (C, C++, Objective-C,
//! Swift, Rust, Go, Java, Kotlin, three C#, Haskell, three JavaScript, two
//! TypeScript, Python, Ruby, Perl, PHP, Lua, Shell, AppleScript, HTML, CSS,
//! Markdown, LaTeX), plus Plain Text and four editor-only languages (JSON,
//! YAML, XML, TOML) that have no run command.

use crate::error::LanguageError;
use crate::model::LanguageDef;

macro_rules! def {
    ($id:literal) => {
        ($id, include_str!(concat!("../defs/", $id, ".toml")))
    };
}

/// `(id, TOML text)` of every built-in definition, in menu order (Plain Text
/// first, then alphabetical).
pub const BUILTIN_SOURCES: &[(&str, &str)] = &[
    def!("plain-text"),
    def!("applescript"),
    def!("c"),
    def!("csharp-dotnet"),
    def!("csharp-script"),
    def!("csharp-mono"),
    def!("cpp"),
    def!("css"),
    def!("go"),
    def!("haskell"),
    def!("html"),
    def!("java"),
    def!("javascript-browser"),
    def!("javascript-jquery"),
    def!("javascript-node"),
    def!("json"),
    def!("kotlin"),
    def!("latex"),
    def!("lua"),
    def!("markdown"),
    def!("objective-c"),
    def!("objective-cpp"),
    def!("perl"),
    def!("php"),
    def!("python"),
    def!("ruby"),
    def!("rust"),
    def!("shell"),
    def!("swift"),
    def!("toml"),
    def!("typescript-browser"),
    def!("typescript-node"),
    def!("xml"),
    def!("yaml"),
];

/// Parses every built-in definition. Each result carries `builtin = true`.
/// A definition that fails to parse is returned as an error in its slot (the
/// tests make sure none does).
pub fn parse_builtins() -> Vec<Result<LanguageDef, LanguageError>> {
    BUILTIN_SOURCES
        .iter()
        .map(|(id, text)| {
            LanguageDef::from_toml_with_origin(text, Some(&format!("built-in {id}"))).map(
                |mut def| {
                    def.builtin = true;
                    def
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses_and_matches_its_file_name() {
        for ((id, _), parsed) in BUILTIN_SOURCES.iter().zip(parse_builtins()) {
            let def = parsed.unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(&def.id, id);
            assert!(def.builtin);
        }
    }

    #[test]
    fn there_are_29_bundled_languages_plus_extras() {
        let defs: Vec<_> = parse_builtins().into_iter().map(Result::unwrap).collect();
        let with_uuid = defs.iter().filter(|d| d.uuid.is_some()).count();
        assert_eq!(
            with_uuid, 29,
            "the 29 CodeRunner languages carry their uuid"
        );
        assert_eq!(defs.len(), 34);
    }
}
