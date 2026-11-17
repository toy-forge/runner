//! Our own JSON and TOML binding-set formats, and file loading by extension.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Binding, BindingError, BindingSet, stable_identifier};

/// A binding as written in JSON/TOML. `identifier` may be omitted; a stable one is derived.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    identifier: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "key",
        alias = "keyEquivalent"
    )]
    key_equivalent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "compound",
        alias = "compoundCommandCode"
    )]
    compound: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileSet {
    title: String,
    #[serde(default, skip_serializing_if = "is_false", alias = "readOnly")]
    read_only: bool,
    #[serde(default, alias = "keyBindings")]
    bindings: Vec<FileBinding>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn slug(title: &str, index: usize, b: &Binding) -> String {
    let name = b.title.as_deref().or(b.command.as_deref()).unwrap_or_default();
    format!("{title}|{index}|{name}|{}", b.key_equivalent.as_deref().unwrap_or_default())
}

fn from_file_set(set: FileSet) -> BindingSet {
    let title_for_ids = set.title.clone();
    let bindings = set
        .bindings
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let mut binding = Binding {
                identifier: b.identifier,
                key_equivalent: b.key_equivalent,
                title: b.title,
                command: b.command,
                compound_code: b.compound,
                enabled: b.enabled,
            };
            if binding.identifier.is_empty() {
                binding.identifier = stable_identifier("file", &slug(&title_for_ids, i, &binding));
            }
            binding
        })
        .collect();
    BindingSet { title: set.title, read_only: set.read_only, bindings }
}

fn to_file_set(set: &BindingSet) -> FileSet {
    FileSet {
        title: set.title.clone(),
        read_only: set.read_only,
        bindings: set
            .bindings
            .iter()
            .map(|b| FileBinding {
                identifier: b.identifier.clone(),
                key_equivalent: b.key_equivalent.clone(),
                title: b.title.clone(),
                command: b.command.clone(),
                compound: b.compound_code.clone(),
                enabled: b.enabled,
            })
            .collect(),
    }
}

/// Reads our JSON format.
pub(super) fn from_json(text: &str) -> Result<BindingSet, BindingError> {
    let set: FileSet = serde_json::from_str(text).map_err(|e| BindingError::Json(e.to_string()))?;
    Ok(from_file_set(set))
}

/// Writes our JSON format (pretty-printed).
pub(super) fn to_json(set: &BindingSet) -> Result<String, BindingError> {
    serde_json::to_string_pretty(&to_file_set(set)).map_err(|e| BindingError::Json(e.to_string()))
}

/// Reads our TOML format.
pub(super) fn from_toml(text: &str) -> Result<BindingSet, BindingError> {
    let set: FileSet = toml::from_str(text).map_err(|e| BindingError::Toml(e.to_string()))?;
    Ok(from_file_set(set))
}

/// Writes our TOML format.
pub(super) fn to_toml(set: &BindingSet) -> Result<String, BindingError> {
    toml::to_string_pretty(&to_file_set(set)).map_err(|e| BindingError::Toml(e.to_string()))
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// Loads a set from a file, choosing the format by extension.
pub(super) fn load_file(path: &Path) -> Result<BindingSet, BindingError> {
    match extension(path).as_str() {
        "crbindings" => {
            let bytes = std::fs::read(path).map_err(|e| BindingError::Io(e.to_string()))?;
            BindingSet::from_plist_bytes(&bytes)
        }
        "json" => {
            let text = std::fs::read_to_string(path).map_err(|e| BindingError::Io(e.to_string()))?;
            from_json(&text)
        }
        "toml" => {
            let text = std::fs::read_to_string(path).map_err(|e| BindingError::Io(e.to_string()))?;
            from_toml(&text)
        }
        other => Err(BindingError::UnsupportedExtension(other.to_owned())),
    }
}

/// Saves a set to a file, choosing the format by extension.
pub(super) fn save_file(set: &BindingSet, path: &Path) -> Result<(), BindingError> {
    match extension(path).as_str() {
        "crbindings" => {
            let bytes = set.to_plist_bytes()?;
            std::fs::write(path, bytes).map_err(|e| BindingError::Io(e.to_string()))
        }
        "json" => {
            let text = set.to_json()?;
            std::fs::write(path, text).map_err(|e| BindingError::Io(e.to_string()))
        }
        "toml" => {
            let text = set.to_toml()?;
            std::fs::write(path, text).map_err(|e| BindingError::Io(e.to_string()))
        }
        other => Err(BindingError::UnsupportedExtension(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BindingSet {
        BindingSet {
            title: "Mine".into(),
            read_only: false,
            bindings: vec![
                Binding::command(Some("Save It"), "@s", "MainMenu/File/Save"),
                Binding::compound("Helper", "", r#"["beep"]"#),
            ],
        }
    }

    #[test]
    fn json_roundtrip() {
        let set = sample();
        let text = to_json(&set).unwrap();
        assert!(text.contains("\"title\": \"Mine\""));
        assert_eq!(from_json(&text).unwrap(), set);
    }

    #[test]
    fn toml_roundtrip() {
        let set = sample();
        let text = to_toml(&set).unwrap();
        assert_eq!(from_toml(&text).unwrap(), set);
    }

    #[test]
    fn missing_identifiers_become_stable() {
        let a = from_json(r#"{"title":"X","bindings":[{"key":"@s","command":"MainMenu/File/Save"}]}"#).unwrap();
        let b = from_json(r#"{"title":"X","bindings":[{"key":"@s","command":"MainMenu/File/Save"}]}"#).unwrap();
        assert_eq!(a.bindings[0].identifier, b.bindings[0].identifier);
        assert!(!a.bindings[0].identifier.is_empty());
    }
}
