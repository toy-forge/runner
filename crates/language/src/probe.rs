//! Pure source probes used by declarative build specs.
//!
//! CodeRunner ships small helper binaries (`autoinclude`, `parsejava`) that
//! its compile scripts call. This module is the clean-room equivalent, as
//! plain functions over source text and a directory:
//!
//! * [`local_sources`]: the files to compile together for a C-family file
//!   (the file plus the implementation files of the local headers it includes,
//!   recursively);
//! * [`framework_args`]: `-framework X` arguments for `#import <X/X.h>`;
//! * [`java_package`], [`java_main_class`], [`java_source_root`];
//! * [`swift_sources`]: `main.swift` compiles every `*.swift` of its directory.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::PlanError;

/// The C-family dialect a file is in; decides which sibling files count as
/// implementations of an included header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFamily {
    /// C: `x.h` -> `x.c`.
    C,
    /// C++: `x.h` -> `x.cpp`, `x.cc`, `x.cxx`, `x.cp`, `x.c++`.
    Cpp,
    /// Objective-C: `x.h` -> `x.m`, `x.c`.
    ObjC,
    /// Objective-C++: `x.h` -> `x.mm`, `x.cpp`, `x.m`, `x.cc`, `x.cxx`, `x.c`.
    ObjCpp,
}

impl SourceFamily {
    /// Extensions of implementation files, in order of preference.
    pub fn implementation_extensions(self) -> &'static [&'static str] {
        match self {
            Self::C => &["c"],
            Self::Cpp => &["cpp", "cc", "cxx", "cp", "c++"],
            Self::ObjC => &["m", "c"],
            Self::ObjCpp => &["mm", "cpp", "m", "cc", "cxx", "c"],
        }
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("built-in probe regex is valid"))
}

/// Removes `//` and `/* */` comments (not aware of string literals; good
/// enough for scanning declarations).
pub fn strip_c_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = '\0';
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && next == '/' {
                        break;
                    }
                    prev = next;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The package a Java source declares (`package a.b;`), empty for the default
/// package.
pub fn java_package(source: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let clean = strip_c_comments(source);
    regex(&RE, r"(?m)^\s*package\s+([\w.]+)\s*;")
        .captures(&clean)
        .map(|c| c[1].to_owned())
        .unwrap_or_default()
}

/// The fully qualified class of the `main` method of a Java source, `None`
/// when there is no `main`. The class is the nearest class-like declaration
/// before the method.
///
/// ```
/// use scratch_language::java_main_class;
/// let src = "package demo;\nclass Helper {}\npublic class App {\n public static void main(String[] args) {}\n}";
/// assert_eq!(java_main_class(src).as_deref(), Some("demo.App"));
/// ```
pub fn java_main_class(source: &str) -> Option<String> {
    static CLASS: OnceLock<Regex> = OnceLock::new();
    static MAIN: OnceLock<Regex> = OnceLock::new();
    let clean = strip_c_comments(source);
    let main = regex(
        &MAIN,
        r"\bstatic\s+(?:final\s+)?void\s+main\s*\(\s*(?:final\s+)?String(?:\s*(?:\[\s*\]|\.\.\.)\s*[\w$]+|\s+[\w$]+\s*\[\s*\])\s*\)",
    )
    .find(&clean)?;
    let class = regex(
        &CLASS,
        r"\b(?:class|interface|enum|record)\s+([A-Za-z_$][\w$]*)",
    )
    .captures_iter(&clean[..main.start()])
    .last()?[1]
        .to_owned();
    let package = java_package(source);
    Some(if package.is_empty() {
        class
    } else {
        format!("{package}.{class}")
    })
}

/// The root a Java file must be compiled from: `dir` minus the package's
/// directories when `dir` ends with them, else `dir` itself.
///
/// ```
/// use std::path::Path;
/// use scratch_language::java_source_root;
/// assert_eq!(java_source_root(Path::new("/p/src/com/x"), "com.x"), Path::new("/p/src"));
/// assert_eq!(java_source_root(Path::new("/p/other"), "com.x"), Path::new("/p/other"));
/// ```
pub fn java_source_root(dir: &Path, package: &str) -> PathBuf {
    if package.is_empty() {
        return dir.to_path_buf();
    }
    let parts: Vec<&str> = package.split('.').collect();
    let comps: Vec<Component<'_>> = dir.components().collect();
    if comps.len() > parts.len() {
        let tail = &comps[comps.len() - parts.len()..];
        let matches = tail
            .iter()
            .zip(&parts)
            .all(|(c, p)| c.as_os_str().to_str() == Some(*p));
        if matches {
            return comps[..comps.len() - parts.len()].iter().collect();
        }
    }
    dir.to_path_buf()
}

/// The path of the Java file relative to its source root:
/// `com/x/App.java` for package `com.x` (when the directory matches), else just
/// the file name.
pub fn java_source_relative(dir: &Path, file_name: &str, package: &str) -> String {
    let root = java_source_root(dir, package);
    match dir.strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => {
            rel.join(file_name).to_string_lossy().into_owned()
        }
        _ => file_name.to_owned(),
    }
}

/// The files to compile together for a C-family `main_file` in `dir`: the file
/// first, then the implementation files of the local headers it includes
/// (`#include "x.h"` / `#import "x.h"`), found recursively. Implementation
/// files that define `main` are skipped. Names are relative to `dir`.
///
/// # Errors
///
/// [`PlanError::Probe`] when `main_file` cannot be read.
pub fn local_sources(
    dir: &Path,
    main_file: &str,
    family: SourceFamily,
) -> Result<Vec<String>, PlanError> {
    static INCLUDE: OnceLock<Regex> = OnceLock::new();
    static MAIN_FN: OnceLock<Regex> = OnceLock::new();
    let include = regex(&INCLUDE, r#"(?m)^\s*#\s*(?:include|import)\s+"([^"]+)""#);
    let main_fn = regex(&MAIN_FN, r"\bmain\s*\(");

    let mut result = vec![main_file.to_owned()];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(main_file.to_owned());
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(main_file.to_owned());

    while let Some(current) = queue.pop_front() {
        let path = dir.join(&current);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if current == main_file => {
                return Err(PlanError::Probe {
                    path,
                    message: e.to_string(),
                });
            }
            Err(_) => continue,
        };
        let text = strip_c_comments(&text);
        let base = Path::new(&current)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        for caps in include.captures_iter(&text) {
            let header = normalize(&base.join(&caps[1]));
            let header_name = header.to_string_lossy().into_owned();
            if seen.insert(header_name.clone()) && dir.join(&header).is_file() {
                queue.push_back(header_name.clone());
            }
            // An included implementation file is part of the including file.
            let ext = header
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if family.implementation_extensions().contains(&ext) {
                continue;
            }
            for impl_ext in family.implementation_extensions() {
                let candidate = header.with_extension(impl_ext);
                let candidate_name = candidate.to_string_lossy().into_owned();
                if seen.contains(&candidate_name) {
                    break;
                }
                let full = dir.join(&candidate);
                if full.is_file() {
                    seen.insert(candidate_name.clone());
                    let defines_main = std::fs::read_to_string(&full)
                        .map(|t| main_fn.is_match(&strip_c_comments(&t)))
                        .unwrap_or(false);
                    if !defines_main {
                        result.push(candidate_name.clone());
                        queue.push_back(candidate_name);
                    }
                    break;
                }
            }
        }
    }
    Ok(result)
}

/// Collapses `.` and `..` components without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// `-framework X` arguments for the Apple frameworks the given files import:
/// `#import <X/Y.h>` / `#include <X/Y.h>` with a capitalised `X`, and
/// `@import X;`. Lower-case first components (`sys/`, `dispatch/`) are system
/// headers of libSystem and need no flag.
pub fn framework_args(dir: &Path, files: &[String]) -> Vec<String> {
    static ANGLE: OnceLock<Regex> = OnceLock::new();
    static MODULE: OnceLock<Regex> = OnceLock::new();
    let angle = regex(
        &ANGLE,
        r"(?m)^\s*#\s*(?:import|include)\s*<([A-Z][A-Za-z0-9_]*)/",
    );
    let module = regex(&MODULE, r"(?m)^\s*@import\s+([A-Za-z][A-Za-z0-9_]*)");
    let mut frameworks: Vec<String> = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
            continue;
        };
        let text = strip_c_comments(&text);
        for caps in angle
            .captures_iter(&text)
            .chain(module.captures_iter(&text))
        {
            let name = caps[1].to_owned();
            if !frameworks.contains(&name) {
                frameworks.push(name);
            }
        }
    }
    frameworks
        .into_iter()
        .flat_map(|name| ["-framework".to_owned(), name])
        .collect()
}

/// The Swift files to compile: all `*.swift` files of `dir` (sorted, with
/// `main.swift` first) when `file_name` is `main.swift`, else just the file.
///
/// # Errors
///
/// [`PlanError::Probe`] when the directory cannot be listed.
pub fn swift_sources(dir: &Path, file_name: &str) -> Result<Vec<String>, PlanError> {
    if file_name != "main.swift" {
        return Ok(vec![file_name.to_owned()]);
    }
    let entries = std::fs::read_dir(dir).map_err(|e| PlanError::Probe {
        path: dir.to_path_buf(),
        message: e.to_string(),
    })?;
    let mut others: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.ends_with(".swift") && name != "main.swift" && !name.starts_with('.'))
        .collect();
    others.sort();
    let mut all = vec!["main.swift".to_owned()];
    all.extend(others);
    Ok(all)
}

/// Turns arbitrary text into an identifier: non-alphanumeric characters become
/// `_`, a leading digit gets a `_` prefix, empty becomes `main`.
///
/// ```
/// use scratch_language::sanitize_identifier;
/// assert_eq!(sanitize_identifier("my file-1"), "my_file_1");
/// assert_eq!(sanitize_identifier("1st"), "_1st");
/// ```
pub fn sanitize_identifier(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        return "main".to_owned();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn java_package_and_main() {
        let src = "// package no;\npackage com.acme.tools;\nimport x.Y;\nclass Util {}\npublic class Tool {\n  public static void main(final String... argv) {}\n}\n";
        assert_eq!(java_package(src), "com.acme.tools");
        assert_eq!(java_main_class(src).as_deref(), Some("com.acme.tools.Tool"));
        assert_eq!(java_main_class("class A { void f() {} }"), None);
        assert_eq!(
            java_main_class("class A { public static void main(String args[]) {} }").as_deref(),
            Some("A")
        );
        assert_eq!(
            java_main_class("/* class Fake { */ class Real { static void main(String[] a){} }")
                .as_deref(),
            Some("Real")
        );
        assert_eq!(java_package("class A {}"), "");
    }

    #[test]
    fn java_roots() {
        assert_eq!(
            java_source_root(Path::new("/p/src/com/acme"), "com.acme"),
            Path::new("/p/src")
        );
        assert_eq!(
            java_source_root(Path::new("/p/src"), "com.acme"),
            Path::new("/p/src")
        );
        assert_eq!(
            java_source_root(Path::new("/p/src"), ""),
            Path::new("/p/src")
        );
        assert_eq!(
            java_source_relative(Path::new("/p/src/com/acme"), "A.java", "com.acme"),
            "com/acme/A.java"
        );
        assert_eq!(
            java_source_relative(Path::new("/p/other"), "A.java", "com.acme"),
            "A.java"
        );
    }

    #[test]
    fn local_sources_follow_includes_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(
            d,
            "main.c",
            "#include <stdio.h>\n#include \"util.h\"\nint main(void){return 0;}\n",
        );
        write(d, "util.h", "#include \"deep.h\"\nint util(void);\n");
        write(
            d,
            "util.c",
            "#include \"util.h\"\nint util(void){return 1;}\n",
        );
        write(d, "deep.h", "int deep(void);\n");
        write(d, "deep.c", "int deep(void){return 2;}\n");
        write(d, "unrelated.c", "int other(void){return 3;}\n");
        let got = local_sources(d, "main.c", SourceFamily::C).unwrap();
        assert_eq!(got, ["main.c", "util.c", "deep.c"]);
    }

    #[test]
    fn local_sources_skip_files_with_main_and_missing_impls() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(
            d,
            "a.cpp",
            "#include \"b.h\"\n#include \"c.h\"\nint main(){}\n",
        );
        write(d, "b.h", "");
        write(d, "b.cpp", "int main() { return 1; }\n"); // would clash with a.cpp's main
        write(d, "c.h", "");
        let got = local_sources(d, "a.cpp", SourceFamily::Cpp).unwrap();
        assert_eq!(got, ["a.cpp"]);
    }

    #[test]
    fn local_sources_ignore_comments_and_system_includes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(
            d,
            "a.c",
            "// #include \"x.h\"\n/* #include \"y.h\" */\n#include <z.h>\nint main(){}\n",
        );
        write(d, "x.c", "");
        write(d, "x.h", "");
        write(d, "y.c", "");
        write(d, "y.h", "");
        assert_eq!(local_sources(d, "a.c", SourceFamily::C).unwrap(), ["a.c"]);
    }

    #[test]
    fn local_sources_subdirectories_and_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "a.c", "#include \"lib/m.h\"\nint main(){}\n");
        write(d, "lib/m.h", "#include \"../n.h\"\n");
        write(d, "lib/m.c", "");
        write(d, "n.h", "");
        write(d, "n.c", "");
        let mut got = local_sources(d, "a.c", SourceFamily::C).unwrap();
        got.sort();
        assert_eq!(got, ["a.c", "lib/m.c", "n.c"]);
    }

    #[test]
    fn local_sources_missing_main_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            local_sources(tmp.path(), "nope.c", SourceFamily::C),
            Err(PlanError::Probe { .. })
        ));
    }

    #[test]
    fn frameworks_from_imports() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "a.m",
            "#import <Foundation/Foundation.h>\n#import <AppKit/AppKit.h>\n#include <sys/types.h>\n#import <dispatch/dispatch.h>\n// #import <Nope/Nope.h>\n@import CoreGraphics;\n#import <Foundation/NSString.h>\n",
        );
        assert_eq!(
            framework_args(tmp.path(), &["a.m".into()]),
            [
                "-framework",
                "Foundation",
                "-framework",
                "AppKit",
                "-framework",
                "CoreGraphics"
            ]
        );
    }

    #[test]
    fn swift_main_compiles_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        for f in [
            "main.swift",
            "b.swift",
            "a.swift",
            "notes.txt",
            ".hidden.swift",
        ] {
            write(tmp.path(), f, "");
        }
        assert_eq!(
            swift_sources(tmp.path(), "main.swift").unwrap(),
            ["main.swift", "a.swift", "b.swift"]
        );
        assert_eq!(swift_sources(tmp.path(), "b.swift").unwrap(), ["b.swift"]);
    }

    #[test]
    fn identifiers() {
        assert_eq!(sanitize_identifier("my file-1"), "my_file_1");
        assert_eq!(sanitize_identifier(""), "main");
        assert_eq!(sanitize_identifier("9"), "_9");
    }
}
