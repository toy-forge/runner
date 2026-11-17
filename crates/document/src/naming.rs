//! File names: `Untitled`, `Untitled 2`, `report copy`, `report copy 2`.
//!
//! All functions are pure; they take an `is_taken` predicate so the caller
//! decides what "exists" means (a directory listing, the names of open
//! documents, or both). [`dir_has`] is the predicate for a directory.

use std::path::Path;

/// The base name of untitled documents.
pub const UNTITLED: &str = "Untitled";

/// `Untitled.py` for `n == 1`, else `Untitled 2.py`. An empty extension gives
/// a name without dot.
pub fn untitled_file_name(extension: &str, n: u32) -> String {
    let stem = if n <= 1 {
        UNTITLED.to_owned()
    } else {
        format!("{UNTITLED} {n}")
    };
    with_extension(&stem, extension)
}

/// The first of `Untitled.ext`, `Untitled 2.ext`, ... that is not taken.
pub fn next_untitled_name(extension: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let extension = extension.trim_start_matches('.');
    (1..)
        .map(|n| untitled_file_name(extension, n))
        .find(|name| !is_taken(name))
        .unwrap_or_else(|| untitled_file_name(extension, u32::MAX))
}

/// Parses an untitled name: `Untitled 3.py` is `(3, "py")`, `Untitled.py` is
/// `(1, "py")`; anything else is `None`.
pub fn parse_untitled(name: &str) -> Option<(u32, String)> {
    let (stem, ext) = split_extension(name);
    let number = if stem == UNTITLED {
        1
    } else {
        stem.strip_prefix(UNTITLED)?
            .strip_prefix(' ')?
            .parse::<u32>()
            .ok()
            .filter(|n| *n >= 2)?
    };
    Some((number, ext.to_owned()))
}

/// The name of an untitled file moved to another language: same number, new
/// extension, or the next free number when that name is taken.
pub fn retarget_untitled_name(
    current: &str,
    new_extension: &str,
    is_taken: impl Fn(&str) -> bool,
) -> String {
    let new_extension = new_extension.trim_start_matches('.');
    let number = parse_untitled(current).map_or(1, |(n, _)| n);
    let same = untitled_file_name(new_extension, number);
    if same == current || !is_taken(&same) {
        return same;
    }
    (number + 1..)
        .map(|n| untitled_file_name(new_extension, n))
        .find(|name| !is_taken(name))
        .unwrap_or(same)
}

/// Splits `name` into stem and extension at the last dot. Names that start
/// with a dot and have no other (`.zshrc`) have no extension.
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    }
}

fn with_extension(stem: &str, extension: &str) -> String {
    let extension = extension.trim_start_matches('.');
    if extension.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{extension}")
    }
}

/// Strips a trailing ` copy` or ` copy N` from a stem.
fn copy_root(stem: &str) -> &str {
    if let Some(root) = stem.strip_suffix(" copy") {
        return root;
    }
    if let Some((root, number)) = stem.rsplit_once(' ')
        && number.chars().all(|c| c.is_ascii_digit())
        && !number.is_empty()
        && let Some(root) = root.strip_suffix(" copy")
    {
        return root;
    }
    stem
}

/// The name of a duplicate: `a.py` gives `a copy.py`, then `a copy 2.py`,
/// `a copy 3.py`. Duplicating a copy numbers the next one (`a copy.py` gives
/// `a copy 2.py`, not `a copy copy.py`).
pub fn duplicate_name(file_name: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = split_extension(file_name);
    let root = copy_root(stem);
    let first = with_extension(&format!("{root} copy"), ext);
    if !is_taken(&first) {
        return first;
    }
    (2..)
        .map(|n| with_extension(&format!("{root} copy {n}"), ext))
        .find(|name| !is_taken(name))
        .unwrap_or(first)
}

/// The first of `name`, `stem 2.ext`, `stem 3.ext`, ... that is not taken
/// (New File / New Folder in the file navigator).
pub fn numbered_name(name: &str, is_taken: impl Fn(&str) -> bool) -> String {
    if !is_taken(name) {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    (2..)
        .map(|n| with_extension(&format!("{stem} {n}"), ext))
        .find(|candidate| !is_taken(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// Why a file name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// Empty or only whitespace.
    #[error("the name is empty")]
    Empty,
    /// Contains `/` or `:` (macOS file names cannot hold them; `:` is shown as
    /// `/` by Finder).
    #[error("the name contains \"/\" or \":\"")]
    InvalidCharacter,
    /// `.` or `..`.
    #[error("the name is reserved")]
    Reserved,
    /// Longer than 255 bytes.
    #[error("the name is too long")]
    TooLong,
}

/// Checks a name typed in a rename field or save panel.
///
/// # Errors
///
/// See [`NameError`].
pub fn validate_file_name(name: &str) -> Result<(), NameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err(NameError::Empty)
    } else if trimmed == "." || trimmed == ".." {
        Err(NameError::Reserved)
    } else if trimmed.contains(['/', ':', '\0']) {
        Err(NameError::InvalidCharacter)
    } else if trimmed.len() > 255 {
        Err(NameError::TooLong)
    } else {
        Ok(())
    }
}

/// A predicate for [`next_untitled_name`] and friends: `name` exists in `dir`.
pub fn dir_has(dir: &Path) -> impl Fn(&str) -> bool + '_ {
    move |name| dir.join(name).symlink_metadata().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn taken(names: &[&str]) -> impl Fn(&str) -> bool {
        let set: HashSet<String> = names.iter().map(|s| (*s).to_owned()).collect();
        move |name| set.contains(name)
    }

    #[test]
    fn untitled_names_count_up() {
        assert_eq!(untitled_file_name("py", 1), "Untitled.py");
        assert_eq!(untitled_file_name("py", 2), "Untitled 2.py");
        assert_eq!(untitled_file_name("", 3), "Untitled 3");
        assert_eq!(next_untitled_name("py", taken(&[])), "Untitled.py");
        assert_eq!(
            next_untitled_name(".py", taken(&["Untitled.py"])),
            "Untitled 2.py"
        );
        assert_eq!(
            next_untitled_name(
                "py",
                taken(&["Untitled.py", "Untitled 2.py", "Untitled 4.py"])
            ),
            "Untitled 3.py"
        );
        assert_eq!(
            next_untitled_name("c", taken(&["Untitled.py"])),
            "Untitled.c"
        );
    }

    #[test]
    fn parses_untitled_names() {
        assert_eq!(parse_untitled("Untitled.py"), Some((1, "py".into())));
        assert_eq!(parse_untitled("Untitled 12.c"), Some((12, "c".into())));
        assert_eq!(parse_untitled("Untitled"), Some((1, String::new())));
        assert_eq!(parse_untitled("Untitled 1.c"), None);
        assert_eq!(parse_untitled("Untitled2.c"), None);
        assert_eq!(parse_untitled("report.py"), None);
    }

    #[test]
    fn retargets_untitled_names_for_a_new_language() {
        assert_eq!(
            retarget_untitled_name("Untitled 2.py", "rb", taken(&[])),
            "Untitled 2.rb"
        );
        assert_eq!(
            retarget_untitled_name("Untitled.py", "rb", taken(&["Untitled.rb"])),
            "Untitled 2.rb"
        );
        assert_eq!(
            retarget_untitled_name("Untitled.py", "py", taken(&["Untitled.py"])),
            "Untitled.py"
        );
        assert_eq!(
            retarget_untitled_name("weird.txt", "rb", taken(&[])),
            "Untitled.rb"
        );
    }

    #[test]
    fn splits_extensions_sanely() {
        assert_eq!(split_extension("a.py"), ("a", "py"));
        assert_eq!(split_extension("archive.tar.gz"), ("archive.tar", "gz"));
        assert_eq!(split_extension(".zshrc"), (".zshrc", ""));
        assert_eq!(split_extension("Makefile"), ("Makefile", ""));
        assert_eq!(split_extension("trailing."), ("trailing.", ""));
    }

    #[test]
    fn duplicate_names_follow_the_finder() {
        assert_eq!(duplicate_name("a.py", taken(&[])), "a copy.py");
        assert_eq!(duplicate_name("a.py", taken(&["a copy.py"])), "a copy 2.py");
        assert_eq!(
            duplicate_name("a.py", taken(&["a copy.py", "a copy 2.py"])),
            "a copy 3.py"
        );
        assert_eq!(
            duplicate_name("a copy.py", taken(&["a copy.py"])),
            "a copy 2.py"
        );
        assert_eq!(
            duplicate_name("a copy 2.py", taken(&["a copy 2.py", "a copy.py"])),
            "a copy 3.py"
        );
        assert_eq!(duplicate_name("Makefile", taken(&[])), "Makefile copy");
        assert_eq!(
            duplicate_name("notes 2.txt", taken(&[])),
            "notes 2 copy.txt"
        );
    }

    #[test]
    fn numbered_names_for_new_files() {
        assert_eq!(
            numbered_name("untitled folder", taken(&[])),
            "untitled folder"
        );
        assert_eq!(
            numbered_name("untitled folder", taken(&["untitled folder"])),
            "untitled folder 2"
        );
        assert_eq!(numbered_name("a.py", taken(&["a.py", "a 2.py"])), "a 3.py");
    }

    #[test]
    fn validates_names() {
        assert_eq!(validate_file_name("ok.py"), Ok(()));
        assert_eq!(validate_file_name("  "), Err(NameError::Empty));
        assert_eq!(validate_file_name("a/b"), Err(NameError::InvalidCharacter));
        assert_eq!(validate_file_name("a:b"), Err(NameError::InvalidCharacter));
        assert_eq!(validate_file_name(".."), Err(NameError::Reserved));
        assert_eq!(
            validate_file_name(&"x".repeat(300)),
            Err(NameError::TooLong)
        );
    }

    #[test]
    fn dir_predicate_sees_files_and_dangling_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::os::unix::fs::symlink("nowhere", dir.path().join("link")).unwrap();
        let has = dir_has(dir.path());
        assert!(has("a.txt") && has("link") && !has("b.txt"));
    }
}
