//! Reading and writing CodeRunner `.crBindings` XML property lists.
//!
//! Shipped CodeRunner files contain raw control characters (Escape, Backspace) inside
//! `<string>` elements, which strict XML parsers reject. We substitute them with private-use
//! placeholders before parsing and restore them afterwards. The parsed
//! `compoundCommandArray` / `compoundCommandDictionary` caches are ignored on read and
//! regenerated on write (the source of truth is `compoundCommandCode`).

use std::io::Cursor;

use plist::{Dictionary, Value};

use super::{Binding, BindingError, BindingSet};
use crate::compound::{CompoundCommand, Root};

/// First placeholder code point for an XML-invalid C0 control character.
const CONTROL_BASE: u32 = 0xE000;

/// Replaces XML 1.0-invalid control characters with private-use placeholders.
pub(super) fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            let n = c as u32;
            if n < 0x20 && !matches!(c, '\t' | '\n' | '\r') { char::from_u32(CONTROL_BASE + n).unwrap_or('\u{FFFD}') } else { c }
        })
        .collect()
}

/// Restores placeholders written by [`sanitize`].
pub(super) fn restore(text: &str) -> String {
    text.chars()
        .map(|c| {
            let n = c as u32;
            if (CONTROL_BASE..CONTROL_BASE + 0x20).contains(&n) {
                char::from_u32(n - CONTROL_BASE).unwrap_or('\u{FFFD}')
            } else {
                c
            }
        })
        .collect()
}

fn string_field(dict: &Dictionary, key: &str) -> Result<Option<String>, BindingError> {
    match dict.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(restore(s))),
        Some(_) => Err(BindingError::Format(format!("{key:?} must be a string"))),
    }
}

fn bool_field(dict: &Dictionary, key: &str, default: bool) -> Result<bool, BindingError> {
    match dict.get(key) {
        None => Ok(default),
        Some(Value::Boolean(b)) => Ok(*b),
        Some(_) => Err(BindingError::Format(format!("{key:?} must be a boolean"))),
    }
}

/// Parses a `.crBindings` property list.
pub(super) fn read(bytes: &[u8]) -> Result<BindingSet, BindingError> {
    let value = if bytes.starts_with(b"bplist") {
        Value::from_reader(Cursor::new(bytes)).map_err(|e| BindingError::Plist(e.to_string()))?
    } else {
        let text = String::from_utf8_lossy(bytes);
        let clean = sanitize(&text);
        if clean.trim_start().starts_with('<') {
            Value::from_reader_xml(Cursor::new(clean.as_bytes()))
        } else {
            Value::from_reader(Cursor::new(clean.as_bytes()))
        }
        .map_err(|e| BindingError::Plist(e.to_string()))?
    };
    let Value::Dictionary(root) = value else {
        return Err(BindingError::Format("the root must be a dictionary".into()));
    };
    let title = string_field(&root, "title")?.unwrap_or_else(|| "Untitled".to_owned());
    let read_only = bool_field(&root, "readOnly", false)?;
    let Some(Value::Array(items)) = root.get("keyBindings") else {
        return Err(BindingError::Format("\"keyBindings\" must be an array".into()));
    };
    let mut bindings = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Value::Dictionary(d) = item else {
            return Err(BindingError::Format(format!("keyBindings[{i}] must be a dictionary")));
        };
        // `compoundCommandArray` / `compoundCommandDictionary` are parsed caches: ignored.
        let identifier = string_field(d, "identifier")?.unwrap_or_default();
        let key_equivalent = string_field(d, "keyEquivalent")?;
        let title = string_field(d, "title")?;
        let command = string_field(d, "command")?;
        let compound_code = string_field(d, "compoundCommandCode")?;
        bindings.push(Binding { identifier, key_equivalent, title, command, compound_code, enabled: true });
    }
    Ok(BindingSet { title, read_only, bindings })
}

fn to_plist(value: &serde_json::Value) -> Option<Value> {
    plist::to_value(value).ok()
}

fn caches(code: &str) -> Option<(Option<Value>, Option<Value>)> {
    let command = CompoundCommand::parse(code).ok()?;
    let value = to_plist(&command.to_json())?;
    match &command.root {
        Root::List(_) => Some((Some(value), None)),
        Root::Entries(_) => Some((None, Some(value))),
    }
}

fn binding_value(b: &Binding) -> Result<Value, BindingError> {
    let mut d = Dictionary::new();
    d.insert("identifier".to_owned(), Value::String(b.identifier.clone()));
    if let Some(k) = &b.key_equivalent {
        d.insert("keyEquivalent".to_owned(), Value::String(k.clone()));
    }
    if let Some(t) = &b.title {
        d.insert("title".to_owned(), Value::String(t.clone()));
    }
    if let Some(c) = &b.command {
        d.insert("command".to_owned(), Value::String(c.clone()));
    }
    if let Some(code) = &b.compound_code {
        d.insert("compoundCommandCode".to_owned(), Value::String(code.clone()));
        if let Some((array, dict)) = caches(code) {
            if let Some(a) = array {
                d.insert("compoundCommandArray".to_owned(), a);
            }
            if let Some(o) = dict {
                d.insert("compoundCommandDictionary".to_owned(), o);
            }
        }
    }
    Ok(Value::Dictionary(d))
}

/// Serialises a set as a `.crBindings` XML property list (CodeRunner-compatible).
pub(super) fn write(set: &BindingSet) -> Result<Vec<u8>, BindingError> {
    let mut root = Dictionary::new();
    let mut items = Vec::with_capacity(set.bindings.len());
    for b in &set.bindings {
        items.push(binding_value(b)?);
    }
    root.insert("keyBindings".to_owned(), Value::Array(items));
    root.insert("readOnly".to_owned(), Value::Boolean(set.read_only));
    root.insert("title".to_owned(), Value::String(set.title.clone()));
    let mut out = Vec::new();
    Value::Dictionary(root).to_writer_xml(&mut out).map_err(|e| BindingError::Plist(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_roundtrip() {
        let s = "a\u{1b}b\u{8}c\u{7f}d";
        assert_eq!(restore(&sanitize(s)), s);
    }

    #[test]
    fn control_character_in_xml_is_read() {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
             <key>title</key><string>T</string>\n<key>readOnly</key><true/>\n\
             <key>keyBindings</key><array><dict>\n<key>identifier</key><string>X</string>\n\
             <key>keyEquivalent</key><string>^{}</string>\n</dict></array>\n</dict></plist>",
            '\u{1b}'
        );
        let set = read(xml.as_bytes()).expect("parses");
        assert_eq!(set.bindings[0].key_equivalent.as_deref(), Some("^\u{1b}"));
    }
}
