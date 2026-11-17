//! Missing tool handling: which tool is missing, how to get it, and whether
//! the user was already asked (CodeRunner's `DiagnosticsInstallationInfo`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A diagnostics tool that could not be run because it is not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingTool {
    /// The program that was not found (`shellcheck`).
    pub program: String,
    /// The language's message for a missing toolchain, when it has one
    /// (`Could not find Lua on your system...`).
    pub message: Option<String>,
    /// Homebrew formula that provides the program, when known.
    pub homebrew_formula: Option<&'static str>,
}

impl MissingTool {
    /// Builds the description for `program`.
    pub fn new(program: &str, message: Option<String>) -> Self {
        let name = program.rsplit('/').next().unwrap_or(program);
        Self {
            program: name.to_owned(),
            message,
            homebrew_formula: homebrew_formula(name),
        }
    }

    /// The command that installs it, e.g. `brew install shellcheck`.
    pub fn install_command(&self) -> Option<String> {
        self.homebrew_formula.map(|f| format!("brew install {f}"))
    }
}

/// The Homebrew formula that provides a tool, for the tools diagnostics
/// commonly use. `None` for tools macOS or Xcode already provides or that
/// are unknown.
pub fn homebrew_formula(program: &str) -> Option<&'static str> {
    Some(match program {
        "shellcheck" => "shellcheck",
        "node" | "npm" | "npx" => "node",
        "tsc" => "typescript",
        "eslint" => "eslint",
        "python3" | "python" | "pip3" => "python",
        "php" => "php",
        "ruby" => "ruby",
        "perl" => "perl",
        "lua" | "luac" => "lua",
        "go" | "gofmt" => "go",
        "rustc" | "cargo" => "rust",
        "java" | "javac" => "openjdk",
        "dotnet" => "dotnet",
        "kotlinc" | "kotlin" => "kotlin",
        "ghc" | "runghc" => "ghc",
        "mono" | "mcs" | "csc" => "mono",
        "clang" | "swiftc" | "xcrun" | "osacompile" | "bash" => return None,
        _ => return None,
    })
}

/// What happened with the install prompt of one program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationInfo {
    /// The user was asked already.
    pub prompted: bool,
    /// Outcome (`installed`, `declined`, ...), free text for the app.
    pub status: String,
}

/// Per-program record of install prompts: persist it as the
/// `DiagnosticsInstallationInfo` preference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstallPrompts {
    by_program: BTreeMap<String, InstallationInfo>,
}

impl InstallPrompts {
    /// Whether the user should be offered to install `tool` (not asked
    /// before and a formula is known).
    pub fn should_prompt(&self, tool: &MissingTool) -> bool {
        tool.homebrew_formula.is_some()
            && !self
                .by_program
                .get(&tool.program)
                .is_some_and(|i| i.prompted)
    }

    /// Records that the user was asked.
    pub fn mark_prompted(&mut self, program: &str, status: &str) {
        self.by_program.insert(
            program.to_owned(),
            InstallationInfo {
                prompted: true,
                status: status.to_owned(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_once() {
        let tool = MissingTool::new("/usr/local/bin/shellcheck", None);
        assert_eq!(tool.program, "shellcheck");
        assert_eq!(tool.install_command().as_deref(), Some("brew install shellcheck"));
        let mut prompts = InstallPrompts::default();
        assert!(prompts.should_prompt(&tool));
        prompts.mark_prompted("shellcheck", "declined");
        assert!(!prompts.should_prompt(&tool));
        assert!(!prompts.should_prompt(&MissingTool::new("osacompile", None)));
        let json = serde_json::to_string(&prompts).unwrap();
        assert!(json.contains("declined"));
    }
}
