//! File URIs, as the protocol spells them.
//!
//! Servers exchange `file://` URIs whose paths are percent-encoded UTF-8. The editor works with
//! filesystem paths, so both directions are needed. Other schemes (untitled, in-memory) are
//! rejected: this client materialises unsaved documents in a virtual workspace instead, so every
//! document it knows about has a real file.

use std::path::{Path, PathBuf};

use crate::error::{LspError, Result};

/// The URI for a filesystem path.
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut uri = String::with_capacity(text.len() + 7);
    uri.push_str("file://");
    if !text.starts_with('/') {
        // A relative path: prefixing `/` makes it valid; the manager only passes absolute paths.
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// The path of a `file:` URI.
///
/// Percent escapes are decoded as UTF-8; `file://localhost/path` is accepted. Anything that is
/// not a local file URI is an error, so a server cannot make the editor write through an
/// unexpected scheme.
pub fn uri_to_path(uri: &str) -> Result<PathBuf> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return Err(LspError::Io(format!("not a file URI: {uri}")));
    };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if !rest.starts_with('/') {
        return Err(LspError::Io(format!(
            "file URI has no absolute path: {uri}"
        )));
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input
                .next()
                .ok_or_else(|| LspError::Io(format!("truncated percent escape in URI: {uri}")))?;
            let low = input
                .next()
                .ok_or_else(|| LspError::Io(format!("truncated percent escape in URI: {uri}")))?;
            let high = (high as char)
                .to_digit(16)
                .ok_or_else(|| LspError::Io(format!("bad percent escape in URI: {uri}")))?;
            let low = (low as char)
                .to_digit(16)
                .ok_or_else(|| LspError::Io(format!("bad percent escape in URI: {uri}")))?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    if bytes.contains(&0) {
        return Err(LspError::Io(format!("URI contains a NUL byte: {uri}")));
    }
    let path =
        String::from_utf8(bytes).map_err(|_| LspError::Io(format!("URI is not UTF-8: {uri}")))?;
    Ok(PathBuf::from(path))
}

/// The scheme of a URI (`file`, `untitled`, ...), if it has one.
pub fn scheme(uri: &str) -> Option<&str> {
    let colon = uri.find(':')?;
    let scheme = &uri[..colon];
    (!scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

/// Whether `uri` names a local file.
pub fn is_file_uri(uri: &str) -> bool {
    uri.starts_with("file://")
}

/// The last path component of a URI, percent-decoded (`main.py`).
pub fn file_name(uri: &str) -> Option<String> {
    let path = uri_to_path(uri).ok()?;
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_spaces_and_unicode() {
        for path in [
            Path::new("/tmp/a b/c.py"),
            Path::new("/tmp/日本語/файл.rs"),
            Path::new("/tmp/100%/x#y?.txt"),
            Path::new("/"),
            Path::new("/tmp/emoji😀.md"),
        ] {
            let uri = path_to_uri(path);
            assert!(uri.starts_with("file:///"), "{uri}");
            assert_eq!(uri_to_path(&uri).unwrap(), path);
        }
        assert_eq!(path_to_uri(Path::new("/tmp/a b")), "file:///tmp/a%20b");
        assert_eq!(
            path_to_uri(Path::new("/tmp/日本")),
            "file:///tmp/%E6%97%A5%E6%9C%AC"
        );
    }

    #[test]
    fn accepts_localhost_and_rejects_other_schemes() {
        assert_eq!(
            uri_to_path("file://localhost/tmp/x").unwrap(),
            PathBuf::from("/tmp/x")
        );
        assert!(uri_to_path("untitled:Untitled-1").is_err());
        assert!(uri_to_path("file://relative/x").is_err());
        assert!(uri_to_path("file:///tmp/%zz").is_err());
        assert!(uri_to_path("file:///tmp/%e6").is_err());
    }

    #[test]
    fn identifies_schemes_and_names() {
        assert_eq!(scheme("file:///tmp/x"), Some("file"));
        assert_eq!(scheme("untitled:U1"), Some("untitled"));
        assert_eq!(scheme("no-colon"), None);
        assert!(is_file_uri("file:///x"));
        assert!(!is_file_uri("http://x"));
        assert_eq!(file_name("file:///tmp/a%20b.py").as_deref(), Some("a b.py"));
    }
}
