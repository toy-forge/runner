//! The virtual workspace: where unsaved documents become real files for language servers.
//!
//! Language servers are file-based: rust-analyzer refuses a file outside a Cargo package, gopls
//! needs a module, and most servers only return useful answers for files they opened. Unsaved
//! ScratchRunner documents therefore get a stable directory under
//! `~/Library/Caches/ScratchRunner/workspaces/<id>/` (or the cache directory the app chooses),
//! and the document's text is written there on every change. Servers can then read and index it.
//!
//! Single-file projects get the scaffolding the server needs: a `Cargo.toml` for rust-analyzer,
//! a `go.mod` for gopls, a `tsconfig.json` for typescript-language-server. Scaffolding files are
//! only created when missing; a user's own files are never overwritten.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{LspError, Result};
use crate::uri::path_to_uri;

/// Project files a virtual workspace can create for a single file, chosen per server in the
/// catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scaffold {
    /// Nothing; the server works with a lone file.
    #[default]
    None,
    /// A Cargo package (rust-analyzer).
    Cargo,
    /// A Go module (gopls).
    Go,
    /// A `tsconfig.json` (typescript-language-server).
    #[serde(alias = "tsconfig", alias = "typescript")]
    TsConfig,
}

/// One document materialised in the virtual workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualFile {
    /// The directory that serves as the server's workspace root.
    pub root: PathBuf,
    /// The document's file inside it.
    pub path: PathBuf,
    /// The document's `file://` URI.
    pub uri: String,
}

/// Manages the cache directories of unsaved documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualWorkspace {
    base: PathBuf,
}

impl VirtualWorkspace {
    /// A workspace whose documents live under `base`. A relative base is resolved against the
    /// process's current directory, so URIs are always absolute.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        let base = if base.is_absolute() {
            base
        } else {
            std::env::current_dir()
                .map(|current| current.join(&base))
                .unwrap_or(base)
        };
        Self { base }
    }

    /// The app's default cache: `~/Library/Caches/ScratchRunner/workspaces`.
    pub fn default_base() -> PathBuf {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Library/Caches/ScratchRunner/workspaces")
    }

    /// The base directory (for display and tests).
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// The stable directory of the document identified by `id`.
    pub fn workspace_dir(&self, id: &str) -> PathBuf {
        self.base.join(directory_name(id))
    }

    /// Writes `text` as the document `id` and returns its path and URI. `relative_path` is the
    /// path inside the workspace (`src/main.rs`); when absent, `main.<extension>` is used.
    /// `scaffold` files are created when they are missing.
    pub fn prepare(
        &self,
        id: &str,
        extension: Option<&str>,
        relative_path: Option<&str>,
        scaffold: Scaffold,
        text: &str,
    ) -> Result<VirtualFile> {
        let root = self.workspace_dir(id);
        fs::create_dir_all(&root)?;
        let relative = sanitize_relative_path(relative_path)
            .unwrap_or_else(|| format!("main.{}", sanitize_extension(extension)));
        let path = root.join(&relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, text)?;
        scaffold_files(&root, scaffold)?;
        Ok(VirtualFile {
            uri: path_to_uri(&path),
            root,
            path,
        })
    }

    /// Removes a document's directory, ignoring errors (cache cleanup is best effort).
    pub fn remove(&self, id: &str) {
        let _ = fs::remove_dir_all(self.workspace_dir(id));
    }
}

/// A short stable directory name for a document identifier. The hash keeps ids that sanitise to
/// the same text apart; the readable prefix makes the cache inspectable.
fn directory_name(id: &str) -> String {
    let mut readable: String = id
        .chars()
        .take(48)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    while readable.contains("..") {
        readable = readable.replace("..", "_");
    }
    let readable = readable
        .trim_matches(|ch| ch == '.' || ch == '_')
        .to_owned();
    let mut readable = readable;
    if readable.is_empty() {
        readable.push_str("document");
    }
    format!("{readable}-{:016x}", fnv1a(id))
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Accepts only plain relative paths (`a/b.txt`), never absolute paths or `..` segments.
fn sanitize_relative_path(path: Option<&str>) -> Option<String> {
    let path = Path::new(path?);
    if path.is_absolute() {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if !part.is_empty() {
                    parts.push(part.into_owned());
                }
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn sanitize_extension(extension: Option<&str>) -> String {
    let extension = extension.unwrap_or("txt").trim_start_matches('.');
    if !extension.is_empty()
        && extension.len() <= 16
        && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        extension.to_owned()
    } else {
        "txt".to_owned()
    }
}

fn scaffold_files(root: &Path, scaffold: Scaffold) -> Result<()> {
    match scaffold {
        Scaffold::None => {}
        Scaffold::Cargo => {
            let manifest = root.join("Cargo.toml");
            if !manifest.exists() {
                let name = format!("scratch_{:016x}", fnv1a(&root.to_string_lossy()));
                write_atomic(
                    &manifest,
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"
                    ),
                )?;
            }
        }
        Scaffold::Go => {
            let module = root.join("go.mod");
            if !module.exists() {
                let name = format!("scratchrunner/{:016x}", fnv1a(&root.to_string_lossy()));
                write_atomic(&module, &format!("module {name}\n\ngo 1.21\n"))?;
            }
        }
        Scaffold::TsConfig => {
            let config = root.join("tsconfig.json");
            if !config.exists() {
                write_atomic(&config, TSCONFIG)?;
            }
        }
    }
    Ok(())
}

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["**/*.ts", "**/*.tsx", "**/*.js", "**/*.jsx"]
}
"#;

/// Writes a file through a temporary name in the same directory, so a server reading it never
/// sees a half-written document and a crash cannot truncate it.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| LspError::Io(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".scratchrunner-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_updates_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = VirtualWorkspace::new(dir.path().join("ws"));
        let first = workspace
            .prepare(
                "scratch:1",
                Some("py"),
                Some("pkg/main.py"),
                Scaffold::None,
                "x = 1\n",
            )
            .unwrap();
        assert_eq!(
            first.path,
            workspace.workspace_dir("scratch:1").join("pkg/main.py")
        );
        assert_eq!(fs::read_to_string(&first.path).unwrap(), "x = 1\n");
        assert!(first.uri.starts_with("file://"));

        let second = workspace
            .prepare(
                "scratch:1",
                Some("py"),
                Some("pkg/main.py"),
                Scaffold::None,
                "x = 2\n",
            )
            .unwrap();
        assert_eq!(first.path, second.path);
        assert_eq!(fs::read_to_string(&second.path).unwrap(), "x = 2\n");
    }

    #[test]
    fn directory_names_are_readable_stable_and_safe() {
        let workspace = VirtualWorkspace::new("/tmp/ws");
        let one = workspace.workspace_dir("untitled:Untitled-1");
        assert_eq!(one, workspace.workspace_dir("untitled:Untitled-1"));
        assert!(
            one.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("untitled_Untitled-1-")
        );
        let other = workspace.workspace_dir("untitled/Untitled-1");
        assert_ne!(one, other, "different ids must not share a directory");
        assert!(
            !workspace
                .workspace_dir("../../etc/passwd")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("..")
        );
        assert!(
            workspace
                .workspace_dir("..")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("document-")
        );
    }

    #[test]
    fn refuses_parent_and_absolute_relative_paths() {
        assert_eq!(sanitize_relative_path(Some("../evil.py")), None);
        assert_eq!(sanitize_relative_path(Some("/etc/passwd")), None);
        assert_eq!(sanitize_relative_path(Some("a/../../b")), None);
        assert_eq!(
            sanitize_relative_path(Some("src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            sanitize_relative_path(Some("./a//b")).as_deref(),
            Some("a/b")
        );
        assert_eq!(sanitize_relative_path(None), None);
    }

    #[test]
    fn creates_cargo_scaffolding_once() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = VirtualWorkspace::new(dir.path());
        let file = workspace
            .prepare(
                "rust-doc",
                Some("rs"),
                Some("src/main.rs"),
                Scaffold::Cargo,
                "fn main() {}\n",
            )
            .unwrap();
        let manifest = file.root.join("Cargo.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        assert!(text.contains("[package]"));
        assert!(text.contains("edition = \"2021\""));

        fs::write(&manifest, "# mine\n").unwrap();
        workspace
            .prepare(
                "rust-doc",
                Some("rs"),
                Some("src/main.rs"),
                Scaffold::Cargo,
                "fn main() {}\n",
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&manifest).unwrap(), "# mine\n");
    }

    #[test]
    fn creates_go_and_ts_scaffolding() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = VirtualWorkspace::new(dir.path());
        let go = workspace
            .prepare(
                "go-doc",
                Some("go"),
                Some("main.go"),
                Scaffold::Go,
                "package main\n",
            )
            .unwrap();
        assert!(
            fs::read_to_string(go.root.join("go.mod"))
                .unwrap()
                .starts_with("module scratchrunner/")
        );
        let ts = workspace
            .prepare(
                "ts-doc",
                Some("ts"),
                Some("main.ts"),
                Scaffold::TsConfig,
                "export {}\n",
            )
            .unwrap();
        let config = fs::read_to_string(ts.root.join("tsconfig.json")).unwrap();
        assert!(config.contains("\"module\": \"ESNext\""));
    }

    #[test]
    fn falls_back_to_a_safe_extension() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = VirtualWorkspace::new(dir.path());
        let file = workspace
            .prepare("no-path", Some("../../x"), None, Scaffold::None, "text")
            .unwrap();
        assert_eq!(file.path.file_name().unwrap(), "main.txt");
        let file = workspace
            .prepare("no-ext", None, None, Scaffold::None, "text")
            .unwrap();
        assert_eq!(file.path.file_name().unwrap(), "main.txt");
    }

    #[test]
    fn remove_deletes_only_that_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = VirtualWorkspace::new(dir.path());
        let one = workspace
            .prepare("a", Some("txt"), None, Scaffold::None, "a")
            .unwrap();
        let two = workspace
            .prepare("b", Some("txt"), None, Scaffold::None, "b")
            .unwrap();
        workspace.remove("a");
        assert!(!one.path.exists());
        assert!(two.path.exists());
    }
}
