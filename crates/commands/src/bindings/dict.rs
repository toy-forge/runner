//! Import of macOS `KeyBindings.dict` files.
//!
//! The AppKit standard file and most user files are dictionaries mapping an NS key equivalent
//! (`"^w"`, `"@~e"`, `"@\u{F700}"`) to a selector (`"deleteWordBackward:"`) or to an array of
//! selectors (the first one that responds wins). The file may be an old-style ASCII property
//! list, an XML plist or a binary plist.

use std::io::Cursor;

use plist::Value;

use super::{Binding, BindingError, BindingSet, plist_io, stable_identifier};
use crate::shortcut::Shortcut;

/// Result of importing a `KeyBindings.dict`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictImport {
    /// The imported set (`macOS Key Bindings`, editable).
    pub set: BindingSet,
    /// Entries that were skipped, with the reason.
    pub warnings: Vec<String>,
}

fn parse_value(bytes: &[u8]) -> Result<Value, BindingError> {
    if bytes.starts_with(b"bplist") {
        return Value::from_reader(Cursor::new(bytes)).map_err(|e| BindingError::Dict(e.to_string()));
    }
    let text = String::from_utf8_lossy(bytes);
    if text.trim_start().starts_with('<') {
        let clean = plist_io::sanitize(&text);
        return Value::from_reader_xml(Cursor::new(clean.as_bytes())).map_err(|e| BindingError::Dict(e.to_string()));
    }
    Value::from_reader_ascii(Cursor::new(text.as_bytes())).map_err(|e| BindingError::Dict(e.to_string()))
}

/// Converts a selector (`deleteWordBackward:`) to a command name (`deleteWordBackward`).
/// Selectors that take arguments cannot be bound directly.
fn selector_command(selector: &str) -> Result<String, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("empty selector".into());
    }
    if selector.matches(':').count() > 1 {
        return Err(format!("selector {selector:?} takes arguments"));
    }
    let name = selector.strip_suffix(':').unwrap_or(selector);
    if name.is_empty() {
        return Err(format!("selector {selector:?} has an empty name"));
    }
    Ok(name.to_owned())
}

/// Imports a `KeyBindings.dict` from its bytes.
pub fn import_key_bindings_dict(bytes: &[u8]) -> Result<DictImport, BindingError> {
    let value = parse_value(bytes)?;
    let Value::Dictionary(dict) = value else {
        return Err(BindingError::Dict("the root must be a dictionary".into()));
    };
    let mut bindings = Vec::new();
    let mut warnings = Vec::new();
    for (key, value) in dict.iter() {
        let key = plist_io::restore(key);
        if let Err(e) = Shortcut::parse_crbindings(&key) {
            warnings.push(format!("{key:?}: invalid key equivalent: {e}"));
            continue;
        }
        let selectors: Vec<&str> = match value {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => {
                let mut v = Vec::new();
                for item in a {
                    match item {
                        Value::String(s) => v.push(s.as_str()),
                        other => warnings.push(format!("{key:?}: unsupported array element {other:?}")),
                    }
                }
                v
            }
            other => {
                warnings.push(format!("{key:?}: unsupported value {other:?} (nested dictionaries are not imported)"));
                continue;
            }
        };
        let Some((first, rest)) = selectors.split_first() else {
            warnings.push(format!("{key:?}: empty selector list"));
            continue;
        };
        if !rest.is_empty() {
            warnings.push(format!("{key:?}: only {first:?} is imported; {} alternative selector(s) ignored", rest.len()));
        }
        match selector_command(first) {
            Ok(command) => {
                let identifier = stable_identifier("macos-dict", &format!("{key}|{command}"));
                bindings.push(Binding {
                    identifier,
                    key_equivalent: Some(key),
                    title: None,
                    command: Some(command),
                    compound_code: None,
                    enabled: true,
                });
            }
            Err(e) => warnings.push(format!("{key:?}: {e}")),
        }
    }
    let set = BindingSet { title: "macOS Key Bindings".to_owned(), read_only: false, bindings };
    Ok(DictImport { set, warnings })
}

/// Imports a `KeyBindings.dict` from text (ASCII/XML plists; use the byte form for binary).
pub fn import_key_bindings_dict_str(text: &str) -> Result<DictImport, BindingError> {
    import_key_bindings_dict(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_ascii_dict() {
        let text = r#"{
            "^w" = "deleteWordBackward:";
            "@~e" = ("moveToBeginningOfLine:", "moveToBeginningOfParagraph:");
            "@\UF700" = "moveUp:";
            "no-key" = "beep:";
        }"#;
        let import = import_key_bindings_dict_str(text).unwrap();
        assert_eq!(import.set.bindings.len(), 3);
        let command_of = |key: &str| {
            import.set.bindings.iter().find(|b| b.key_equivalent.as_deref() == Some(key)).and_then(|b| b.command.clone())
        };
        assert_eq!(command_of("^w").as_deref(), Some("deleteWordBackward"));
        assert_eq!(command_of("@~e").as_deref(), Some("moveToBeginningOfLine"));
        assert!(import.set.bindings.iter().any(|b| b.key_equivalent.as_deref() == Some("@\u{F700}")));
        assert_eq!(import.warnings.len(), 2, "{:?}", import.warnings);
    }

    #[test]
    fn imports_xml_dict() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>^a</key><string>moveToBeginningOfParagraph:</string>
</dict></plist>"#;
        let import = import_key_bindings_dict_str(text).unwrap();
        assert_eq!(import.set.bindings.len(), 1);
        assert_eq!(import.set.bindings[0].command.as_deref(), Some("moveToBeginningOfParagraph"));
    }

    #[test]
    fn system_standard_key_binding_dict_imports() {
        let path = "/System/Library/Frameworks/AppKit.framework/Resources/StandardKeyBinding.dict";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIP: {path} is not present");
            return;
        };
        let import = import_key_bindings_dict(&bytes).expect("the system dict imports");
        assert!(import.set.bindings.len() > 100, "only {} bindings", import.set.bindings.len());
    }
}
