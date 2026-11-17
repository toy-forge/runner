//! File-path completion inside strings and after `#include "` / `import "`.
//!
//! The functions are pure: the caller supplies the document text, the caret and the directory
//! relative paths are resolved against (the document's directory).

use std::path::{Path, PathBuf};

use crate::text::{LineIndex, column_to_byte, utf16_len};
use crate::types::{CompletionItem, CompletionKind, CompletionSource, Position, Range, TextEdit};

/// The path fragment immediately before `position`: its range and text. `None` when the caret is
/// not after something that looks like a path.
pub fn path_fragment(text: &str, position: Position) -> Option<(Range, String)> {
    let index = LineIndex::new(text);
    let (line_start, line_end) = index.line_range(text, position.line as usize)?;
    let line = &text[line_start..line_end];
    let caret = column_to_byte(line, position.character);
    let before = &line[..caret];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| !is_path_char(*ch))
        .map_or(0, |(offset, ch)| offset + ch.len_utf8());
    let fragment = &before[start..];
    if fragment.is_empty() {
        return None;
    }
    Some((
        Range {
            start: Position::new(position.line, utf16_len(&line[..start]) as u32),
            end: Position::new(position.line, utf16_len(&line[..caret]) as u32),
        },
        fragment.to_owned(),
    ))
}

/// Whether a fragment is worth completing as a path: it has a separator, starts at the home
/// directory, or explicitly starts with `.`/`..`.
pub fn looks_like_path(fragment: &str) -> bool {
    fragment.contains('/') || fragment.starts_with('~') || fragment.starts_with('.')
}

fn is_path_char(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(
            ch,
            '"' | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | ','
                | ';'
                | ':'
                | '='
                | '|'
                | '&'
                | '*'
                | '?'
                | '!'
        )
}

/// Lists the files and directories matching the path fragment before `position`. `base_dir` is
/// the directory relative paths are resolved against (usually the document's directory).
///
/// Directories get a trailing `/` and sort first; hidden entries appear only when the typed name
/// starts with `.`. The text edit replaces the fragment, so the UI can apply it as-is.
pub fn path_completions(text: &str, position: Position, base_dir: &Path) -> Vec<CompletionItem> {
    let Some((range, fragment)) = path_fragment(text, position) else {
        return Vec::new();
    };
    let (display_dir, name_prefix) = match fragment.rsplit_once('/') {
        Some((directory, name)) => (format!("{directory}/"), name.to_owned()),
        None => (String::new(), fragment.clone()),
    };
    let (search_dir, insert_dir) = if let Some(rest) = display_dir.strip_prefix('~') {
        let Some(home) = home_directory() else {
            return Vec::new();
        };
        (home.join(rest.trim_start_matches('/')), display_dir.clone())
    } else if display_dir.starts_with('/') {
        (PathBuf::from(&display_dir), display_dir.clone())
    } else {
        (base_dir.join(&display_dir), display_dir.clone())
    };
    // A bare `~` means "list the home directory".
    let (search_dir, insert_dir) = if fragment == "~" {
        let Some(home) = home_directory() else {
            return Vec::new();
        };
        (home, "~/".to_owned())
    } else {
        (search_dir, insert_dir)
    };
    let Ok(entries) = std::fs::read_dir(&search_dir) else {
        return Vec::new();
    };
    let show_hidden = name_prefix.starts_with('.');
    let prefix_lower = name_prefix.to_lowercase();
    let mut matches: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            if !name.to_lowercase().starts_with(&prefix_lower) {
                return None;
            }
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    matches.sort_by(|(left_name, left_dir), (right_name, right_dir)| {
        right_dir
            .cmp(left_dir)
            .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
    });
    matches
        .into_iter()
        .take(100)
        .map(|(name, is_dir)| {
            let mut new_text = format!("{insert_dir}{name}");
            if is_dir {
                new_text.push('/');
            }
            let mut item = CompletionItem::simple(
                if is_dir { format!("{name}/") } else { name },
                if is_dir {
                    CompletionKind::Folder
                } else {
                    CompletionKind::File
                },
                CompletionSource::Path,
            );
            item.insert_text = Some(new_text.clone());
            item.text_edit = Some(TextEdit { range, new_text });
            item.sort_text = Some(format!("{}{}", u8::from(!is_dir), item.label));
            item
        })
        .collect()
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_fragment_and_ignores_quotes() {
        let text = "open(\"src/ma";
        let (range, fragment) = path_fragment(text, Position::new(0, 12)).unwrap();
        assert_eq!(fragment, "src/ma");
        assert_eq!(range.start, Position::new(0, 6));
        let (_, fragment) = path_fragment("x = na", Position::new(0, 6)).unwrap();
        assert_eq!(fragment, "na");
        assert!(path_fragment("   ", Position::new(0, 3)).is_none());
        assert!(!looks_like_path("na"));
        assert!(looks_like_path("./x"));
        assert!(looks_like_path("~/x"));
    }

    #[test]
    fn lists_directories_first_and_adds_slashes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("samples")).unwrap();
        std::fs::write(dir.path().join("main.py"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let text = "open(src/";
        let items = path_completions(text, Position::new(0, 9), dir.path());
        assert!(items.is_empty(), "only `src` is inside src, hidden ignored");

        let text = "open(s";
        let items = path_completions(text, Position::new(0, 6), dir.path());
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["samples/", "src/"]);
        assert_eq!(items[0].text_edit.as_ref().unwrap().new_text, "samples/");
        assert_eq!(items[0].kind, Some(CompletionKind::Folder));

        let items = path_completions("open(m", Position::new(0, 6), dir.path());
        assert_eq!(items[0].label, "main.py");
        assert_eq!(items[0].text_edit.as_ref().unwrap().new_text, "main.py");
        assert_eq!(items[0].kind, Some(CompletionKind::File));

        let items = path_completions("open(./", Position::new(0, 7), dir.path());
        assert!(items.iter().any(|item| item.label == "main.py"));
        assert!(items.iter().any(|item| item.label == "samples/"));
        let items = path_completions("open(./.", Position::new(0, 8), dir.path());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, ".hidden");
    }

    #[test]
    fn absolute_and_parent_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file.txt"), "").unwrap();
        let text = "open(sub/";
        let items = path_completions(text, Position::new(0, 9), dir.path());
        assert_eq!(items[0].label, "file.txt");
        assert_eq!(
            items[0].text_edit.as_ref().unwrap().new_text,
            "sub/file.txt"
        );

        let text = "open(../";
        let items = path_completions(text, Position::new(0, 8), &dir.path().join("sub"));
        assert!(items.iter().any(|item| item.label == "sub/"));

        let absolute = format!("open({}/su", dir.path().display());
        let position = Position::new(0, absolute.len() as u32);
        let items = path_completions(&absolute, position, Path::new("/"));
        assert_eq!(items[0].label, "sub/");
        assert_eq!(
            items[0].text_edit.as_ref().unwrap().new_text,
            format!("{}/sub/", dir.path().display())
        );
    }

    #[test]
    fn bare_fragments_complete_when_the_caller_asks_for_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("name.txt"), "").unwrap();
        // `looks_like_path` is the hint the UI uses to decide whether to ask at all.
        assert!(!looks_like_path("na"));
        let items = path_completions("value = na", Position::new(0, 10), dir.path());
        assert_eq!(items[0].label, "name.txt");
        let items = path_completions("value = ./na", Position::new(0, 12), dir.path());
        assert_eq!(items[0].label, "name.txt");
    }
}
