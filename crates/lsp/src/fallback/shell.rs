//! Shell completion: pure helpers around bash's `compgen`, plus a small built-in command list.
//!
//! The process call is the caller's: [`compgen_argv`] builds what to run, and
//! [`parse_compgen_output`] turns its stdout into completion items. That keeps this module pure
//! and testable, while the app can use its own process facilities.

use crate::types::{CompletionItem, CompletionKind, CompletionSource};

/// What to ask bash's `compgen` for (`compgen -A <flag>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellCompletionKind {
    /// Command names on `PATH`.
    Command,
    /// Shell functions.
    Function,
    /// Aliases.
    Alias,
    /// Shell builtins.
    Builtin,
    /// Variables (without `$`).
    Variable,
    /// Shell keywords.
    Keyword,
    /// Signal names.
    Signal,
    /// Files.
    File,
    /// Directories.
    Directory,
    /// Users.
    User,
    /// Host names.
    Hostname,
}

impl ShellCompletionKind {
    /// The `compgen -A` argument.
    pub fn compgen_flag(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Function => "function",
            Self::Alias => "alias",
            Self::Builtin => "builtin",
            Self::Variable => "variable",
            Self::Keyword => "keyword",
            Self::Signal => "signal",
            Self::File => "file",
            Self::Directory => "directory",
            Self::User => "user",
            Self::Hostname => "hostname",
        }
    }

    /// The completion-list kind.
    pub fn completion_kind(self) -> CompletionKind {
        match self {
            Self::Command | Self::Builtin | Self::Alias | Self::Function => {
                CompletionKind::Function
            }
            Self::Variable => CompletionKind::Variable,
            Self::Keyword => CompletionKind::Keyword,
            Self::Signal | Self::User | Self::Hostname => CompletionKind::Value,
            Self::File => CompletionKind::File,
            Self::Directory => CompletionKind::Folder,
        }
    }
}

/// The script to give `bash -c` to list entries of `kind` starting with `prefix`.
pub fn compgen_command(kind: ShellCompletionKind, prefix: &str) -> String {
    format!(
        "compgen -A {} -- {}",
        kind.compgen_flag(),
        shell_quote(prefix)
    )
}

/// The argv to run: `bash --noprofile --norc -c <script>`.
pub fn compgen_argv(kind: ShellCompletionKind, prefix: &str) -> Vec<String> {
    [
        "bash",
        "--noprofile",
        "--norc",
        "-c",
        &compgen_command(kind, prefix),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Quotes one word for bash with single quotes.
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Turns `compgen`'s stdout into completion items, dropping empty and duplicate lines.
pub fn parse_compgen_output(output: &str, kind: ShellCompletionKind) -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| seen.insert(*line))
        .map(|line| {
            let mut item = CompletionItem::simple(
                line.to_owned(),
                kind.completion_kind(),
                CompletionSource::Shell,
            );
            item.insert_text = Some(line.to_owned());
            item.detail = Some(format!("shell {}", kind.compgen_flag()));
            item
        })
        .collect()
}

/// Bash's builtins and keywords, for completion without running anything.
pub fn shell_builtins() -> &'static [&'static str] {
    &[
        "alias",
        "bg",
        "bind",
        "break",
        "builtin",
        "caller",
        "case",
        "cd",
        "command",
        "compgen",
        "complete",
        "continue",
        "declare",
        "dirs",
        "disown",
        "do",
        "done",
        "echo",
        "elif",
        "else",
        "enable",
        "esac",
        "eval",
        "exec",
        "exit",
        "export",
        "false",
        "fc",
        "fg",
        "fi",
        "for",
        "function",
        "getopts",
        "hash",
        "help",
        "history",
        "if",
        "in",
        "jobs",
        "kill",
        "let",
        "local",
        "logout",
        "mapfile",
        "popd",
        "printf",
        "pushd",
        "pwd",
        "read",
        "readarray",
        "readonly",
        "return",
        "select",
        "set",
        "shift",
        "shopt",
        "source",
        "suspend",
        "test",
        "then",
        "time",
        "times",
        "trap",
        "true",
        "type",
        "typeset",
        "ulimit",
        "umask",
        "unalias",
        "unset",
        "until",
        "wait",
        "while",
    ]
}

/// Commands common enough to suggest from the table.
pub fn common_commands() -> &'static [&'static str] {
    &[
        "awk", "brew", "cargo", "cat", "chmod", "chown", "clang", "clear", "cp", "curl", "diff",
        "docker", "find", "git", "go", "grep", "head", "kill", "less", "ln", "ls", "make", "man",
        "mkdir", "mv", "node", "npm", "open", "ps", "python3", "rm", "rsync", "rustc", "scp",
        "sed", "sort", "ssh", "swift", "tail", "tar", "touch", "uniq", "unzip", "wc", "which",
        "xargs", "zip",
    ]
}

/// Static completions for a shell word, useful before (or without) running `compgen`.
pub fn shell_completions(prefix: &str) -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    shell_builtins()
        .iter()
        .chain(common_commands())
        .filter(|name| name.starts_with(prefix))
        .filter(|name| seen.insert(**name))
        .map(|name| {
            let mut item = CompletionItem::simple(
                (*name).to_owned(),
                CompletionKind::Function,
                CompletionSource::Shell,
            );
            item.sort_text = Some(name.to_string());
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compgen_command_quotes_the_prefix() {
        assert_eq!(
            compgen_command(ShellCompletionKind::Command, "git ch"),
            "compgen -A command -- 'git ch'"
        );
        assert_eq!(
            compgen_command(ShellCompletionKind::Variable, "it's"),
            r"compgen -A variable -- 'it'\''s'"
        );
        assert_eq!(
            compgen_argv(ShellCompletionKind::Function, "git"),
            [
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                "compgen -A function -- 'git'"
            ]
        );
        assert_eq!(
            ShellCompletionKind::Directory.completion_kind(),
            CompletionKind::Folder
        );
    }

    #[test]
    fn parses_output_without_duplicates() {
        let items = parse_compgen_output("git\ngit\n  gh  \n\n", ShellCompletionKind::Command);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "git");
        assert_eq!(items[0].source, CompletionSource::Shell);
        assert_eq!(items[0].insert_text.as_deref(), Some("git"));
        assert_eq!(items[1].label, "gh");
        assert!(parse_compgen_output("", ShellCompletionKind::File).is_empty());
    }

    #[test]
    fn compgen_output_can_be_run_when_bash_is_available() {
        let argv = compgen_argv(ShellCompletionKind::Command, "gi");
        let output = match std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
        {
            Ok(output) if output.status.success() => output,
            _ => {
                eprintln!("skipped: bash is not available for compgen");
                return;
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let items = parse_compgen_output(&stdout, ShellCompletionKind::Command);
        assert!(
            items.iter().any(|item| item.label.starts_with("gi")),
            "{:?}",
            items.iter().map(|item| &item.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn static_lists_filter_by_prefix() {
        let items = shell_completions("gi");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "git");
        assert!(shell_completions("zzz").is_empty());
        assert!(shell_builtins().contains(&"export"));
        assert!(
            shell_completions("export")
                .iter()
                .any(|item| item.label == "export")
        );
    }
}
