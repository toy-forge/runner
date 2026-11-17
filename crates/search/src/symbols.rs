//! Heuristic symbol extraction for the symbol popup and QuickNav.
//!
//! CodeRunner builds its symbol list from TextMate `GrammarSupport`
//! `symbolList` entries (`kind`, `scope`, `greedy`). This crate has no
//! grammar engine, so symbols are extracted with a small per-language table
//! of regular expressions over the source lines, plus a nesting tracker
//! (brace depth for C-like languages, indentation for Python/Ruby/shell,
//! heading depth for Markdown, command rank for LaTeX).
//!
//! Extraction is **line based and heuristic**: it finds the constructs a
//! symbol navigator needs (functions, methods, classes, structs, enums,
//! protocols, traits, extensions, type aliases, macros, HTML ids, CSS
//! selectors, Markdown headings and `MARK` / `#pragma mark` sections) with
//! their 0-based line, UTF-16 column, nesting level and kind. It never
//! parses strings/comments fully; brace tracking understands C-style line
//! and block comments and simple string literals.
//!
//! The 29 bundled CodeRunner languages map onto 24 extractors (three C#
//! bundles share [`LanguageId::CSharp`], three JavaScript bundles share
//! [`LanguageId::JavaScript`], two TypeScript bundles share
//! [`LanguageId::TypeScript`], Objective-C and Objective-C++ are distinct,
//! and AppleScript/plain text have their own).
//!
//! ```
//! use scratch_search::symbols::{extract_symbols, LanguageId, SymbolKind};
//!
//! let symbols = extract_symbols("struct A {\n  fn run(&self) {}\n}\n", LanguageId::Rust);
//! assert_eq!(symbols[0].kind, SymbolKind::Struct);
//! assert_eq!(symbols[1].kind, SymbolKind::Method);
//! assert_eq!(symbols[1].level, 1);
//! ```

use std::path::Path;
use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};

use crate::text::byte_to_utf16;

/// What a symbol is, mirroring CodeRunner's `SymbolKindImage_*` set plus the
/// extra kinds the bundled languages expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Free function.
    Function,
    /// Method / member function.
    Method,
    /// Class.
    Class,
    /// Struct.
    Struct,
    /// Enum.
    Enum,
    /// Protocol (Objective-C / Swift).
    Protocol,
    /// Trait (Rust).
    Trait,
    /// Swift extension / Objective-C category.
    Extension,
    /// Interface (Java, C#, TypeScript, Go, Kotlin).
    Interface,
    /// Type alias / typedef.
    TypeAlias,
    /// Macro (`#define`, `macro_rules!`).
    Macro,
    /// `MARK` / `#pragma mark` / TeX section.
    Section,
    /// Markdown heading.
    Heading,
    /// CSS selector.
    Selector,
    /// HTML `id`.
    Id,
    /// Module / package / namespace (Rust/JS uses [`Namespace`](Self::Namespace)).
    Module,
    /// Namespace.
    Namespace,
    /// AppleScript handler.
    Handler,
    /// Property (Objective-C `@property`, AppleScript property).
    Property,
    /// Variable / constant.
    Variable,
    /// Constant.
    Constant,
}

impl SymbolKind {
    /// Human-readable label for the symbol popup and QuickNav subtitles.
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Function => "Function",
            SymbolKind::Method => "Method",
            SymbolKind::Class => "Class",
            SymbolKind::Struct => "Struct",
            SymbolKind::Enum => "Enum",
            SymbolKind::Protocol => "Protocol",
            SymbolKind::Trait => "Trait",
            SymbolKind::Extension => "Extension",
            SymbolKind::Interface => "Interface",
            SymbolKind::TypeAlias => "Type Alias",
            SymbolKind::Macro => "Macro",
            SymbolKind::Section => "Section",
            SymbolKind::Heading => "Heading",
            SymbolKind::Selector => "Selector",
            SymbolKind::Id => "ID",
            SymbolKind::Module => "Module",
            SymbolKind::Namespace => "Namespace",
            SymbolKind::Handler => "Handler",
            SymbolKind::Property => "Property",
            SymbolKind::Variable => "Variable",
            SymbolKind::Constant => "Constant",
        }
    }
}

/// One extracted symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// Display name (without punctuation like `def`/`func`).
    pub name: String,
    /// What it is.
    pub kind: SymbolKind,
    /// 0-based line.
    pub line: u32,
    /// UTF-16 column of the name.
    pub column: u32,
    /// Nesting level (0 = top level).
    pub level: u32,
}

/// Options of [`extract_symbols_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolOptions {
    /// Tab width used to turn indentation into nesting levels.
    pub tab_width: u32,
}

impl Default for SymbolOptions {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}

/// A language with a symbol extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LanguageId {
    /// Plain text: no symbols.
    PlainText,
    /// AppleScript (`source.applescript`).
    AppleScript,
    /// C (`source.c`).
    C,
    /// C++ (`source.c++`).
    Cpp,
    /// C# - all three bundled variants (`source.cs`).
    CSharp,
    /// CSS (`source.css`).
    Css,
    /// Go (`source.go`).
    Go,
    /// Haskell (`source.haskell`).
    Haskell,
    /// HTML (`text.html.basic`).
    Html,
    /// Java (`source.java`).
    Java,
    /// JavaScript - browser, jQuery and Node.js bundles (`source.js*`).
    JavaScript,
    /// Kotlin (`source.Kotlin`).
    Kotlin,
    /// LaTeX (`text.tex.latex`).
    Latex,
    /// Lua (`source.lua`).
    Lua,
    /// Markdown (`text.html.markdown`).
    Markdown,
    /// Objective-C (`source.objc`).
    ObjectiveC,
    /// Objective-C++ (`source.objc++`).
    ObjectiveCpp,
    /// Perl (`source.perl`).
    Perl,
    /// PHP (`text.html.php`).
    Php,
    /// Python (`source.python`).
    Python,
    /// Ruby (`source.ruby`).
    Ruby,
    /// Rust (`source.rust`).
    Rust,
    /// Shell script (`source.shell`).
    Shell,
    /// Swift (`source.swift`).
    Swift,
    /// TypeScript - browser and Node.js bundles (`source.ts`).
    TypeScript,
}

/// Every language with an extractor.
pub const SUPPORTED_LANGUAGES: &[LanguageId] = &[
    LanguageId::PlainText,
    LanguageId::AppleScript,
    LanguageId::C,
    LanguageId::Cpp,
    LanguageId::CSharp,
    LanguageId::Css,
    LanguageId::Go,
    LanguageId::Haskell,
    LanguageId::Html,
    LanguageId::Java,
    LanguageId::JavaScript,
    LanguageId::Kotlin,
    LanguageId::Latex,
    LanguageId::Lua,
    LanguageId::Markdown,
    LanguageId::ObjectiveC,
    LanguageId::ObjectiveCpp,
    LanguageId::Perl,
    LanguageId::Php,
    LanguageId::Python,
    LanguageId::Ruby,
    LanguageId::Rust,
    LanguageId::Shell,
    LanguageId::Swift,
    LanguageId::TypeScript,
];

impl LanguageId {
    /// Canonical short id (`"rust"`, `"cpp"`, ...).
    pub fn id(self) -> &'static str {
        match self {
            LanguageId::PlainText => "text",
            LanguageId::AppleScript => "applescript",
            LanguageId::C => "c",
            LanguageId::Cpp => "cpp",
            LanguageId::CSharp => "csharp",
            LanguageId::Css => "css",
            LanguageId::Go => "go",
            LanguageId::Haskell => "haskell",
            LanguageId::Html => "html",
            LanguageId::Java => "java",
            LanguageId::JavaScript => "javascript",
            LanguageId::Kotlin => "kotlin",
            LanguageId::Latex => "latex",
            LanguageId::Lua => "lua",
            LanguageId::Markdown => "markdown",
            LanguageId::ObjectiveC => "objc",
            LanguageId::ObjectiveCpp => "objcpp",
            LanguageId::Perl => "perl",
            LanguageId::Php => "php",
            LanguageId::Python => "python",
            LanguageId::Ruby => "ruby",
            LanguageId::Rust => "rust",
            LanguageId::Shell => "shell",
            LanguageId::Swift => "swift",
            LanguageId::TypeScript => "typescript",
        }
    }

    /// Display name (`"C#"`, `"JavaScript"`, ...).
    pub fn name(self) -> &'static str {
        match self {
            LanguageId::PlainText => "Text",
            LanguageId::AppleScript => "AppleScript",
            LanguageId::C => "C",
            LanguageId::Cpp => "C++",
            LanguageId::CSharp => "C#",
            LanguageId::Css => "CSS",
            LanguageId::Go => "Go",
            LanguageId::Haskell => "Haskell",
            LanguageId::Html => "HTML",
            LanguageId::Java => "Java",
            LanguageId::JavaScript => "JavaScript",
            LanguageId::Kotlin => "Kotlin",
            LanguageId::Latex => "LaTeX",
            LanguageId::Lua => "Lua",
            LanguageId::Markdown => "Markdown",
            LanguageId::ObjectiveC => "Objective-C",
            LanguageId::ObjectiveCpp => "Objective-C++",
            LanguageId::Perl => "Perl",
            LanguageId::Php => "PHP",
            LanguageId::Python => "Python",
            LanguageId::Ruby => "Ruby",
            LanguageId::Rust => "Rust",
            LanguageId::Shell => "Shell Script",
            LanguageId::Swift => "Swift",
            LanguageId::TypeScript => "TypeScript",
        }
    }

    /// Resolves a language id, bundle id, scope name, extension or display
    /// name (`"rust"`, `"source.rust"`, `".rs"`, `"Rust"`, `"c#"`, `"js"`).
    pub fn from_id(id: &str) -> Option<Self> {
        let mut s = id.trim().to_ascii_lowercase();
        if let Some(rest) = s.strip_prefix('.') {
            s = rest.to_string();
        }
        if let Some(rest) = s
            .strip_prefix("source.")
            .or_else(|| s.strip_prefix("text."))
            .or_else(|| s.strip_prefix("source-"))
        {
            s = rest.to_string();
        }
        Some(match s.as_str() {
            "text" | "plain" | "plaintext" | "txt" | "text.plain" => LanguageId::PlainText,
            "applescript" | "scpt" | "applescript.applescript" => LanguageId::AppleScript,
            "c" | "h" | "source.c" => LanguageId::C,
            "cpp" | "c++" | "cc" | "cxx" | "cplusplus" | "hpp" | "hh" | "h++" | "cp" => {
                LanguageId::Cpp
            }
            "c#" | "csharp" | "cs" | "csx" | "dotnet" | "csharp.net" => LanguageId::CSharp,
            "css" => LanguageId::Css,
            "go" | "golang" => LanguageId::Go,
            "haskell" | "hs" => LanguageId::Haskell,
            "html" | "htm" | "xhtml" | "shtml" | "html.basic" => LanguageId::Html,
            "html.php" => LanguageId::Php,
            "java" => LanguageId::Java,
            "javascript" | "js" | "node" | "nodejs" | "js.node" | "js.jquery" | "jsx" | "mjs"
            | "cjs" => LanguageId::JavaScript,
            "kotlin" | "kt" | "kts" => LanguageId::Kotlin,
            "latex" | "tex" | "tex.latex" => LanguageId::Latex,
            "lua" => LanguageId::Lua,
            "markdown" | "md" | "html.markdown" | "gfm" => LanguageId::Markdown,
            "objective-c" | "objectivec" | "objc" | "obj-c" | "m" => LanguageId::ObjectiveC,
            "objective-c++" | "objectivec++" | "objc++" | "obj-c++" | "objcpp" | "mm" => {
                LanguageId::ObjectiveCpp
            }
            "perl" | "pl" | "pm" => LanguageId::Perl,
            "php" => LanguageId::Php,
            "python" | "py" | "python3" => LanguageId::Python,
            "ruby" | "rb" | "erb" => LanguageId::Ruby,
            "rust" | "rs" => LanguageId::Rust,
            "shell" | "sh" | "bash" | "zsh" | "shell-script" | "shellscript" => LanguageId::Shell,
            "swift" => LanguageId::Swift,
            "typescript" | "ts" | "tsx" | "ts.node" | "typescriptreact" => LanguageId::TypeScript,
            _ => return None,
        })
    }

    /// File extensions (without dots) of the bundled language bundles.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            LanguageId::PlainText => &["txt"],
            LanguageId::AppleScript => &["applescript", "scpt"],
            LanguageId::C => &["c", "h"],
            LanguageId::Cpp => &["cpp", "cc", "cp", "cxx", "c++", "h", "hh", "hpp", "h++"],
            LanguageId::CSharp => &["cs", "csx"],
            LanguageId::Css => &["css"],
            LanguageId::Go => &["go"],
            LanguageId::Haskell => &["hs"],
            LanguageId::Html => &["html", "htm", "shtml", "xhtml", "tmpl", "tpl"],
            LanguageId::Java => &["java"],
            LanguageId::JavaScript => &["js"],
            LanguageId::Kotlin => &["kt", "kts"],
            LanguageId::Latex => &["tex"],
            LanguageId::Lua => &["lua"],
            LanguageId::Markdown => &["md", "markdown"],
            LanguageId::ObjectiveC => &["m", "h"],
            LanguageId::ObjectiveCpp => &["mm", "h"],
            LanguageId::Perl => &["pl"],
            LanguageId::Php => &["php"],
            LanguageId::Python => &["py"],
            LanguageId::Ruby => &["rb"],
            LanguageId::Rust => &["rs"],
            LanguageId::Shell => &["sh"],
            LanguageId::Swift => &["swift"],
            LanguageId::TypeScript => &["ts"],
        }
    }

    /// Language for a file extension (`"rs"` / `".rs"`).
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        SUPPORTED_LANGUAGES
            .iter()
            .copied()
            .find(|l| l.extensions().contains(&ext.as_str()))
    }
}

/// Detects a language from a path's extension (`PlainText` when unknown).
pub fn detect_language(path: &Path) -> LanguageId {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(LanguageId::from_extension)
        .unwrap_or(LanguageId::PlainText)
}

/// Extracts symbols with default [`SymbolOptions`].
pub fn extract_symbols(text: &str, language: LanguageId) -> Vec<Symbol> {
    extract_symbols_with(text, language, &SymbolOptions::default())
}

/// Extracts symbols from `text`.
pub fn extract_symbols_with(
    text: &str,
    language: LanguageId,
    options: &SymbolOptions,
) -> Vec<Symbol> {
    let rules = compiled_rules(language);
    if rules.is_empty() {
        return Vec::new();
    }
    let tab_width = options.tab_width.max(1);
    let nesting = nesting_of(language);
    let mut brace = BraceTracker::default();
    let mut indent_stack: Vec<u32> = vec![0];
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut in_fence = false;

    for (line_index, raw_line) in text.split('\n').enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim_start();

        if language == LanguageId::Markdown
            && (trimmed.starts_with("```") || trimmed.starts_with("~~~"))
        {
            in_fence = !in_fence;
            continue;
        }

        if nesting == Nesting::Indent && !in_fence && !line.trim().is_empty() {
            let units = leading_indent_units(line, tab_width);
            while indent_stack.len() > 1 && units < indent_stack.last().copied().unwrap_or(0) {
                indent_stack.pop();
            }
            let top = indent_stack.last().copied().unwrap_or(0);
            if units > top {
                indent_stack.push(units);
            }
        }
        let structure_level = match nesting {
            Nesting::Braces => brace.depth,
            Nesting::Indent => indent_stack.len().saturating_sub(1) as u32,
            Nesting::None => 0,
        };

        if !in_fence {
            for rule in rules {
                if rule.multi {
                    for caps in rule.re.captures_iter(line) {
                        push_capture(
                            &mut symbols,
                            rule,
                            &caps,
                            line,
                            line_index as u32,
                            structure_level,
                        );
                    }
                } else if let Some(caps) = rule.re.captures(line) {
                    push_capture(
                        &mut symbols,
                        rule,
                        &caps,
                        line,
                        line_index as u32,
                        structure_level,
                    );
                }
            }
        }

        if nesting == Nesting::Braces {
            brace.scan(line, language);
        }
    }

    symbols.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then(a.column.cmp(&b.column))
            .then(a.name.cmp(&b.name))
    });
    symbols.dedup_by(|a, b| a.line == b.line && a.name == b.name);
    symbols
}

fn push_capture(
    symbols: &mut Vec<Symbol>,
    rule: &CompiledRule,
    caps: &Captures<'_>,
    line: &str,
    line_index: u32,
    structure_level: u32,
) {
    let Some(m) = caps.get(rule.name_group) else {
        return;
    };
    let name = m.as_str().trim();
    if name.is_empty() || name.chars().count() > 256 || reserved_name(name) {
        return;
    }
    if !line.is_char_boundary(m.start()) {
        return;
    }
    let column = byte_to_utf16(line, m.start());
    let level = match rule.level {
        LevelRule::Structure => structure_level,
        LevelRule::Heading(group) => caps
            .get(group)
            .map(|g| g.as_str().chars().count().saturating_sub(1) as u32)
            .unwrap_or(structure_level),
        LevelRule::Fixed(level) => level,
        LevelRule::Tex(group) => caps
            .get(group)
            .map(|g| tex_level(g.as_str()))
            .unwrap_or(structure_level),
    };
    let kind = if rule.method_if_nested && level > 0 && rule.kind == SymbolKind::Function {
        SymbolKind::Method
    } else {
        rule.kind
    };
    symbols.push(Symbol {
        name: name.to_string(),
        kind,
        line: line_index,
        column: column as u32,
        level,
    });
}

fn tex_level(command: &str) -> u32 {
    match command {
        "part" | "chapter" => 0,
        "section" => 1,
        "subsection" => 2,
        "subsubsection" => 3,
        "paragraph" => 4,
        "subparagraph" => 5,
        _ => 0,
    }
}

fn reserved_name(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "return"
            | "else"
            | "do"
            | "try"
            | "finally"
            | "func"
            | "function"
            | "var"
            | "let"
            | "class"
            | "struct"
            | "enum"
            | "protocol"
            | "extension"
            | "import"
            | "export"
            | "new"
            | "delete"
            | "typeof"
            | "sizeof"
            | "case"
            | "default"
            | "guard"
            | "defer"
            | "self"
            | "super"
            | "true"
            | "false"
            | "nil"
            | "null"
            | "undefined"
    )
}

fn leading_indent_units(line: &str, tab_width: u32) -> u32 {
    let mut units = 0u32;
    for c in line.chars() {
        match c {
            ' ' => units += 1,
            '\t' => units += tab_width,
            _ => break,
        }
    }
    units
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nesting {
    Braces,
    Indent,
    None,
}

fn nesting_of(language: LanguageId) -> Nesting {
    match language {
        LanguageId::C
        | LanguageId::Cpp
        | LanguageId::CSharp
        | LanguageId::Css
        | LanguageId::Go
        | LanguageId::Java
        | LanguageId::JavaScript
        | LanguageId::Kotlin
        | LanguageId::ObjectiveC
        | LanguageId::ObjectiveCpp
        | LanguageId::Perl
        | LanguageId::Php
        | LanguageId::Rust
        | LanguageId::Swift
        | LanguageId::TypeScript => Nesting::Braces,
        LanguageId::AppleScript
        | LanguageId::Haskell
        | LanguageId::Html
        | LanguageId::Lua
        | LanguageId::Python
        | LanguageId::Ruby
        | LanguageId::Shell => Nesting::Indent,
        LanguageId::Latex | LanguageId::Markdown | LanguageId::PlainText => Nesting::None,
    }
}

/// Minimal C-style brace tracker: understands `//`, `/* */`, `"..."`,
/// `'...'` and backtick strings. Raw strings and nested block comments are
/// not special-cased (heuristic).
#[derive(Default)]
struct BraceTracker {
    depth: u32,
    in_block_comment: bool,
}

impl BraceTracker {
    fn scan(&mut self, line: &str, language: LanguageId) {
        let hash_comments = matches!(language, LanguageId::Php | LanguageId::Perl);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if self.in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    self.in_block_comment = false;
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'/') => break,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    self.in_block_comment = true;
                }
                '#' if hash_comments => break,
                '"' | '\'' | '`' => {
                    let quote = c;
                    let mut escaped = false;
                    for s in chars.by_ref() {
                        if escaped {
                            escaped = false;
                        } else if s == '\\' {
                            escaped = true;
                        } else if s == quote {
                            break;
                        }
                    }
                }
                '{' => self.depth = self.depth.saturating_add(1),
                '}' => self.depth = self.depth.saturating_sub(1),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LevelRule {
    Structure,
    Heading(usize),
    Fixed(u32),
    Tex(usize),
}

#[derive(Debug, Clone, Copy)]
struct RuleSpec {
    kind: SymbolKind,
    pattern: &'static str,
    name_group: usize,
    level: LevelRule,
    multi: bool,
    method_if_nested: bool,
}

const fn r(kind: SymbolKind, pattern: &'static str, name_group: usize) -> RuleSpec {
    RuleSpec {
        kind,
        pattern,
        name_group,
        level: LevelRule::Structure,
        multi: false,
        method_if_nested: false,
    }
}

const fn rf(
    kind: SymbolKind,
    pattern: &'static str,
    name_group: usize,
    level: LevelRule,
) -> RuleSpec {
    RuleSpec {
        kind,
        pattern,
        name_group,
        level,
        multi: false,
        method_if_nested: false,
    }
}

const fn rm(
    kind: SymbolKind,
    pattern: &'static str,
    name_group: usize,
    method_if_nested: bool,
) -> RuleSpec {
    RuleSpec {
        kind,
        pattern,
        name_group,
        level: LevelRule::Structure,
        multi: false,
        method_if_nested,
    }
}

const fn rmulti(kind: SymbolKind, pattern: &'static str, name_group: usize) -> RuleSpec {
    RuleSpec {
        kind,
        pattern,
        name_group,
        level: LevelRule::Structure,
        multi: true,
        method_if_nested: false,
    }
}

struct CompiledRule {
    kind: SymbolKind,
    re: Regex,
    name_group: usize,
    level: LevelRule,
    multi: bool,
    method_if_nested: bool,
}

fn compiled_rules(language: LanguageId) -> &'static [CompiledRule] {
    static ALL: OnceLock<Vec<(LanguageId, Vec<CompiledRule>)>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let mut all = Vec::new();
        for &lang in SUPPORTED_LANGUAGES {
            let mut specs: Vec<RuleSpec> = base_rules(lang).to_vec();
            specs.extend_from_slice(section_rules(lang));
            let compiled = specs
                .iter()
                .filter_map(|spec| {
                    Regex::new(spec.pattern).ok().map(|re| CompiledRule {
                        kind: spec.kind,
                        re,
                        name_group: spec.name_group,
                        level: spec.level,
                        multi: spec.multi,
                        method_if_nested: spec.method_if_nested,
                    })
                })
                .collect();
            all.push((lang, compiled));
        }
        all
    });
    all.iter()
        .find(|(lang, _)| *lang == language)
        .map(|(_, rules)| rules.as_slice())
        .unwrap_or(&[])
}

fn section_rules(language: LanguageId) -> &'static [RuleSpec] {
    match language {
        LanguageId::C | LanguageId::Cpp | LanguageId::ObjectiveC | LanguageId::ObjectiveCpp => {
            &C_SECTIONS
        }
        LanguageId::CSharp
        | LanguageId::Go
        | LanguageId::Java
        | LanguageId::JavaScript
        | LanguageId::Kotlin
        | LanguageId::Rust
        | LanguageId::Swift
        | LanguageId::TypeScript => &SLASH_SECTIONS,
        LanguageId::Php
        | LanguageId::Perl
        | LanguageId::Python
        | LanguageId::Ruby
        | LanguageId::Shell => &HASH_SECTIONS,
        LanguageId::AppleScript | LanguageId::Haskell | LanguageId::Lua => &DASH_SECTIONS,
        _ => &[],
    }
}

static C_SECTIONS: [RuleSpec; 2] = [
    rf(
        SymbolKind::Section,
        r"^[ \t]*//[ \t]*(?:MARK|mark)[ \t]*:?[ \t]*-*[ \t]*(.+?)[ \t]*$",
        1,
        LevelRule::Fixed(0),
    ),
    rf(
        SymbolKind::Section,
        r"^[ \t]*#pragma[ \t]+mark[ \t]*-*[ \t]*(.+?)[ \t]*$",
        1,
        LevelRule::Fixed(0),
    ),
];

static SLASH_SECTIONS: [RuleSpec; 1] = [rf(
    SymbolKind::Section,
    r"^[ \t]*//[ \t]*(?:MARK|mark)[ \t]*:?[ \t]*-*[ \t]*(.+?)[ \t]*$",
    1,
    LevelRule::Fixed(0),
)];

static HASH_SECTIONS: [RuleSpec; 1] = [rf(
    SymbolKind::Section,
    r"^[ \t]*#[ \t]*(?:MARK|mark)[ \t]*:?[ \t]*-*[ \t]*(.+?)[ \t]*$",
    1,
    LevelRule::Fixed(0),
)];

static DASH_SECTIONS: [RuleSpec; 1] = [rf(
    SymbolKind::Section,
    r"^[ \t]*--[ \t]*(?:MARK|mark)[ \t]*:?[ \t]*-*[ \t]*(.+?)[ \t]*$",
    1,
    LevelRule::Fixed(0),
)];

fn base_rules(language: LanguageId) -> &'static [RuleSpec] {
    match language {
        LanguageId::PlainText => &[],
        LanguageId::C => &C_RULES,
        LanguageId::Cpp => &CPP_RULES,
        LanguageId::ObjectiveC => &OBJC_RULES,
        LanguageId::ObjectiveCpp => &OBJCPP_RULES,
        LanguageId::CSharp => &CSHARP_RULES,
        LanguageId::Css => &CSS_RULES,
        LanguageId::Go => &GO_RULES,
        LanguageId::Haskell => &HASKELL_RULES,
        LanguageId::Html => &HTML_RULES,
        LanguageId::Java => &JAVA_RULES,
        LanguageId::JavaScript | LanguageId::TypeScript => &JS_RULES,
        LanguageId::Kotlin => &KOTLIN_RULES,
        LanguageId::Latex => &LATEX_RULES,
        LanguageId::Lua => &LUA_RULES,
        LanguageId::Markdown => &MARKDOWN_RULES,
        LanguageId::Perl => &PERL_RULES,
        LanguageId::Php => &PHP_RULES,
        LanguageId::Python => &PYTHON_RULES,
        LanguageId::Ruby => &RUBY_RULES,
        LanguageId::Rust => &RUST_RULES,
        LanguageId::Shell => &SHELL_RULES,
        LanguageId::Swift => &SWIFT_RULES,
        LanguageId::AppleScript => &APPLESCRIPT_RULES,
    }
}

static C_RULES: [RuleSpec; 9] = [
    r(
        SymbolKind::Macro,
        r"^[ \t]*#[ \t]*define[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:typedef[ \t]+)?struct[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:typedef[ \t]+)?class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:typedef[ \t]+)?union[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*enum(?:[ \t]+class)?[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Namespace,
        r"^[ \t]*namespace[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Method,
        r"^[ \t]*(?:[A-Za-z_][A-Za-z0-9_:<>,\*& \t]*::)([A-Za-z_]\w*)[ \t]*\((?:[^{;]*\{.*|[^;]*)$",
        1,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*(?:[A-Za-z_][A-Za-z0-9_:<>,\*& \t]*[ \t\*&])([A-Za-z_]\w*)[ \t]*\((?:[^{;]*\{.*|[^;]*)$",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*typedef\b.*?\b([A-Za-z_]\w*)[ \t]*;",
        1,
    ),
];

static CPP_RULES: [RuleSpec; 9] = C_RULES;

static OBJC_RULES: [RuleSpec; 10] = [
    r(
        SymbolKind::Macro,
        r"^[ \t]*#[ \t]*define[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*@interface[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*@implementation[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Protocol,
        r"^[ \t]*@protocol[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Method,
        r"^[ \t]*[-+][ \t]*\([^)]*\)[ \t]*([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Property,
        r"^[ \t]*@property\b.*?\b([A-Za-z_]\w*)[ \t]*;",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:typedef[ \t]+)?struct[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*enum(?:[ \t]+class)?[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*(?:[A-Za-z_][A-Za-z0-9_:<>,\*& \t]*[ \t\*&])([A-Za-z_]\w*)[ \t]*\((?:[^{;]*\{.*|[^;]*)$",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*typedef\b.*?\b([A-Za-z_]\w*)[ \t]*;",
        1,
    ),
];

static OBJCPP_RULES: [RuleSpec; 10] = OBJC_RULES;

static CSHARP_RULES: [RuleSpec; 8] = [
    r(SymbolKind::Namespace, r"^[ \t]*namespace[ \t]+([\w.]+)", 1),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:\[[^\]]*\][ \t]*)*(?:(?:public|private|protected|internal|static|sealed|abstract|partial|readonly|new)\s+)*class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:(?:public|private|protected|internal|static|readonly|partial|ref)\s+)*struct[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*(?:(?:public|private|protected|internal|partial)\s+)*interface[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*(?:(?:public|private|protected|internal)\s+)*enum[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:(?:public|private|protected|internal|static|sealed|abstract|partial|readonly)\s+)*record[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    rm(
        SymbolKind::Method,
        r"^[ \t]*(?:\[[^\]]*\][ \t]*)*(?:(?:public|private|protected|internal|static|virtual|override|sealed|abstract|async|extern|unsafe|partial|new|readonly)\s+)+[\w<>\[\],\.\?]+[ \t]+([A-Za-z_]\w*)[ \t]*\(",
        1,
        true,
    ),
    rm(
        SymbolKind::Method,
        r"^[ \t]*(?:public|private|protected|internal)[ \t]+([A-Z][A-Za-z0-9_]*)[ \t]*\(",
        1,
        true,
    ),
];

static CSS_RULES: [RuleSpec; 1] = [r(
    SymbolKind::Selector,
    r"^[ \t]*([^@{}/ \t][^{};]*?)[ \t]*\{",
    1,
)];

static GO_RULES: [RuleSpec; 6] = [
    rm(
        SymbolKind::Method,
        r"^[ \t]*func[ \t]*\([^)]*\)[ \t]*([A-Za-z_]\w*)[ \t]*\(",
        1,
        false,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*func[ \t]+([A-Za-z_]\w*)[ \t]*\(",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*type[ \t]+([A-Za-z_]\w*)[ \t]+struct\b",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*type[ \t]+([A-Za-z_]\w*)[ \t]+interface\b",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*type[ \t]+([A-Za-z_]\w*)\b",
        1,
    ),
    r(
        SymbolKind::Constant,
        r"^[ \t]*(?:const|var)[ \t]+([A-Za-z_]\w*)",
        1,
    ),
];

static HASKELL_RULES: [RuleSpec; 6] = [
    r(SymbolKind::Module, r"^module[ \t]+([\w.]+)", 1),
    r(
        SymbolKind::Struct,
        r"^(?:data|newtype)[ \t]+([A-Z][\w']*)",
        1,
    ),
    r(SymbolKind::TypeAlias, r"^type[ \t]+([A-Z][\w']*)", 1),
    r(
        SymbolKind::Class,
        r"^class[ \t]+(?:\([^)]*\)[ \t]*=>[ \t]*)?([A-Z][\w']*)",
        1,
    ),
    r(SymbolKind::Function, r"^([a-z_][\w']*)[ \t]*::", 1),
    r(SymbolKind::Function, r"^([a-z_][\w']*)[ \t]+.*=", 1),
];

static HTML_RULES: [RuleSpec; 2] = [
    rmulti(SymbolKind::Id, r#"\bid[ \t]*=[ \t]*["']([^"']+)["']"#, 1),
    rf(
        SymbolKind::Heading,
        r"<h([1-6])[^>]*>([^<]+)",
        2,
        LevelRule::Heading(1),
    ),
];

static JAVA_RULES: [RuleSpec; 5] = [
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|static|final|abstract|sealed|non-sealed|strictfp)\s+)*class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|static|abstract|sealed|non-sealed|strictfp)\s+)*interface[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|static|final|strictfp)\s+)*enum[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|static|final|strictfp)\s+)*record[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    rm(
        SymbolKind::Method,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|static|final|abstract|synchronized|native|default|strictfp)\s+)+[\w<>\[\],\.\?]+[ \t]+([A-Za-z_]\w*)[ \t]*\(",
        1,
        true,
    ),
];

static JS_RULES: [RuleSpec; 8] = [
    r(
        SymbolKind::Function,
        r"^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:async[ \t]+)?function[ \t]+([A-Za-z_$][\w$]*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:abstract[ \t]+)?class[ \t]+([A-Za-z_$][\w$]*)",
        1,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+([A-Za-z_$][\w$]*)[ \t]*=[ \t]*(?:async[ \t]*)?(?:function\b|\([^)]*\)[ \t]*=>|[A-Za-z_$][\w$]*[ \t]*=>)",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?interface[ \t]+([A-Za-z_$][\w$]*)",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*(?:export[ \t]+)?type[ \t]+([A-Za-z_$][\w$]*)[ \t]*=",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*(?:export[ \t]+)?(?:const[ \t]+)?enum[ \t]+([A-Za-z_$][\w$]*)",
        1,
    ),
    r(
        SymbolKind::Namespace,
        r"^[ \t]*(?:export[ \t]+)?namespace[ \t]+([A-Za-z_$][\w$]*)",
        1,
    ),
    rm(
        SymbolKind::Method,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|protected[ \t]+|static[ \t]+|async[ \t]+|get[ \t]+|set[ \t]+|\*[ \t]*)*(?:#)?([A-Za-z_$][\w$]*)[ \t]*\([^)]*\)[ \t]*(?::[^{;]+)?[ \t]*\{",
        1,
        true,
    ),
];

static KOTLIN_RULES: [RuleSpec; 5] = [
    rm(
        SymbolKind::Function,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|internal|open|final|override|abstract|suspend|inline|operator|infix|tailrec|external|data|enum|sealed|annotation|companion|const|lateinit|vararg|noinline|crossinline|reified)\s+)*fun[ \t]+(?:<[^>]+>[ \t]*)?(?:[\w.]+\.)?([A-Za-z_]\w*)[ \t]*\(",
        1,
        true,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:@\w+[ \t]*)*(?:(?:public|private|protected|internal|open|final|abstract|sealed|data|enum|value|annotation|inner|inline)\s+)*class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:(?:public|private|protected|internal|open|final|abstract|sealed|data|companion|inner|inline)\s+)*object[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*(?:(?:public|private|protected|internal|sealed|fun|value)\s+)*interface[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*(?:public|private|internal|protected)?[ \t]*typealias[ \t]+([A-Za-z_]\w*)",
        1,
    ),
];

static LATEX_RULES: [RuleSpec; 1] = [rf(
    SymbolKind::Section,
    r"^[ \t]*\\(part|chapter|section|subsection|subsubsection|paragraph|subparagraph)\*?[ \t]*\{([^}]*)\}",
    2,
    LevelRule::Tex(1),
)];

static LUA_RULES: [RuleSpec; 2] = [
    r(
        SymbolKind::Function,
        r"^[ \t]*(?:local[ \t]+)?function[ \t]+([A-Za-z_][\w.:]*)",
        1,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*([A-Za-z_][\w.]*)[ \t]*=[ \t]*function\b",
        1,
    ),
];

static MARKDOWN_RULES: [RuleSpec; 1] = [rf(
    SymbolKind::Heading,
    r"^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$",
    2,
    LevelRule::Heading(1),
)];

static PERL_RULES: [RuleSpec; 3] = [
    r(SymbolKind::Function, r"^[ \t]*sub[ \t]+([A-Za-z_]\w*)", 1),
    r(SymbolKind::Module, r"^[ \t]*package[ \t]+([\w:]+)", 1),
    r(
        SymbolKind::Constant,
        r"^[ \t]*use[ \t]+constant[ \t]+([A-Za-z_]\w*)",
        1,
    ),
];

static PHP_RULES: [RuleSpec; 5] = [
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:abstract[ \t]+|final[ \t]+|readonly[ \t]+)*class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Interface,
        r"^[ \t]*(?:abstract[ \t]+|final[ \t]+)?interface[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(SymbolKind::Trait, r"^[ \t]*trait[ \t]+([A-Za-z_]\w*)", 1),
    r(SymbolKind::Enum, r"^[ \t]*enum[ \t]+([A-Za-z_]\w*)", 1),
    rm(
        SymbolKind::Function,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|protected[ \t]+|static[ \t]+|final[ \t]+|abstract[ \t]+)*function[ \t]+&?[ \t]*([A-Za-z_]\w*)[ \t]*\(",
        1,
        true,
    ),
];

static PYTHON_RULES: [RuleSpec; 2] = [
    rm(
        SymbolKind::Function,
        r"^[ \t]*(?:async[ \t]+)?def[ \t]+([A-Za-z_]\w*)[ \t]*\(",
        1,
        true,
    ),
    r(SymbolKind::Class, r"^[ \t]*class[ \t]+([A-Za-z_]\w*)", 1),
];

static RUBY_RULES: [RuleSpec; 3] = [
    rm(
        SymbolKind::Function,
        r"^[ \t]*def[ \t]+(?:self\.)?([A-Za-z_]\w*[?!=]?)",
        1,
        true,
    ),
    r(SymbolKind::Class, r"^[ \t]*class[ \t]+([\w:]+)", 1),
    r(SymbolKind::Module, r"^[ \t]*module[ \t]+([\w:]+)", 1),
];

static RUST_RULES: [RuleSpec; 9] = [
    rm(
        SymbolKind::Function,
        r#"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:const[ \t]+)?(?:async[ \t]+)?(?:unsafe[ \t]+)?(?:extern[ \t]+"[^"]*"[ \t]+)?fn[ \t]+([A-Za-z_]\w*)"#,
        1,
        true,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?struct[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?enum[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Trait,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:unsafe[ \t]+)?trait[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?union[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?type[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Module,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Macro,
        r"^[ \t]*macro_rules![ \t]*([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Constant,
        r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:const|static)[ \t]+(?:mut[ \t]+)?([A-Za-z_]\w*)[ \t]*:",
        1,
    ),
];

static SHELL_RULES: [RuleSpec; 2] = [
    r(
        SymbolKind::Function,
        r"^[ \t]*function[ \t]+([A-Za-z_][\w-]*)",
        1,
    ),
    r(
        SymbolKind::Function,
        r"^[ \t]*([A-Za-z_][\w-]*)[ \t]*\([ \t]*\)",
        1,
    ),
];

static SWIFT_RULES: [RuleSpec; 8] = [
    rm(
        SymbolKind::Function,
        r"^[ \t]*(?:@\w+(?:\([^)]*\))?[ \t]+)*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+|open[ \t]+|package[ \t]+|static[ \t]+|class[ \t]+|final[ \t]+|override[ \t]+|mutating[ \t]+|nonmutating[ \t]+|convenience[ \t]+|required[ \t]+|lazy[ \t]+|weak[ \t]+|unowned[ \t]+|dynamic[ \t]+|indirect[ \t]+)*func[ \t]+([A-Za-z_]\w*)",
        1,
        true,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+|open[ \t]+|final[ \t]+)*class[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Struct,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+|final[ \t]+)*struct[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Enum,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+|indirect[ \t]+)*enum[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Protocol,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+)*protocol[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Extension,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+)*extension[ \t]+([\w.]+)",
        1,
    ),
    r(
        SymbolKind::TypeAlias,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+)*typealias[ \t]+([A-Za-z_]\w*)",
        1,
    ),
    r(
        SymbolKind::Class,
        r"^[ \t]*(?:public[ \t]+|private[ \t]+|fileprivate[ \t]+|internal[ \t]+|final[ \t]+)*actor[ \t]+([A-Za-z_]\w*)",
        1,
    ),
];

static APPLESCRIPT_RULES: [RuleSpec; 4] = [
    r(SymbolKind::Handler, r"^[ \t]*on[ \t]+([A-Za-z_]\w*)", 1),
    r(SymbolKind::Handler, r"^[ \t]*to[ \t]+([A-Za-z_]\w*)", 1),
    r(SymbolKind::Class, r"^[ \t]*script[ \t]+([A-Za-z_]\w*)", 1),
    r(
        SymbolKind::Property,
        r"^[ \t]*property[ \t]+([A-Za-z_]\w*)",
        1,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<(&str, SymbolKind, u32)> {
        symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.level))
            .collect()
    }

    #[test]
    fn all_rule_patterns_compile() {
        for &lang in SUPPORTED_LANGUAGES {
            for spec in base_rules(lang).iter().chain(section_rules(lang).iter()) {
                assert!(
                    Regex::new(spec.pattern).is_ok(),
                    "{} rule /{}/ does not compile",
                    lang.id(),
                    spec.pattern
                );
            }
        }
        // The compiled table must contain every rule (no silent drops).
        for &lang in SUPPORTED_LANGUAGES {
            let expected = base_rules(lang).len() + section_rules(lang).len();
            assert_eq!(compiled_rules(lang).len(), expected, "{}", lang.id());
            assert!(
                !lang.extensions().is_empty(),
                "{} has no extensions",
                lang.id()
            );
            if lang != LanguageId::PlainText {
                assert!(!base_rules(lang).is_empty(), "{} has no rules", lang.id());
            }
        }
    }

    #[test]
    fn language_ids_and_extensions() {
        assert_eq!(LanguageId::from_id("source.rust"), Some(LanguageId::Rust));
        assert_eq!(LanguageId::from_id("Rust"), Some(LanguageId::Rust));
        assert_eq!(LanguageId::from_id(".rs"), Some(LanguageId::Rust));
        assert_eq!(LanguageId::from_id("c#"), Some(LanguageId::CSharp));
        assert_eq!(LanguageId::from_id("C# (.NET)"), None);
        assert_eq!(LanguageId::from_id("source.cs"), Some(LanguageId::CSharp));
        assert_eq!(LanguageId::from_id("js.node"), Some(LanguageId::JavaScript));
        assert_eq!(LanguageId::from_id("text.html.php"), Some(LanguageId::Php));
        assert_eq!(
            LanguageId::from_id("objective-c++"),
            Some(LanguageId::ObjectiveCpp)
        );
        assert_eq!(LanguageId::from_id("banana"), None);
        assert_eq!(LanguageId::from_extension("rs"), Some(LanguageId::Rust));
        assert_eq!(
            LanguageId::from_extension(".mm"),
            Some(LanguageId::ObjectiveCpp)
        );
        assert_eq!(
            detect_language(Path::new("/x/y/main.swift")),
            LanguageId::Swift
        );
        assert_eq!(
            detect_language(Path::new("/x/y/main.unknown")),
            LanguageId::PlainText
        );
    }

    #[test]
    fn rust_symbols_and_nesting() {
        let text = "\
// MARK: - Basics
pub struct Point { x: i32 }
pub enum Shape { Circle }
pub trait Area { fn area(&self) -> f64; }
pub type Id = u64;
mod inner {}
macro_rules! my_macro { () => {} }

impl Area for Point {
    pub async fn area(&self) -> f64 {
        fn local() {}
        1.0
    }
}
";
        let symbols = extract_symbols(text, LanguageId::Rust);
        let n = names(&symbols);
        assert!(n.contains(&("Basics", SymbolKind::Section, 0)));
        assert!(n.contains(&("Point", SymbolKind::Struct, 0)));
        assert!(n.contains(&("Shape", SymbolKind::Enum, 0)));
        assert!(n.contains(&("Area", SymbolKind::Trait, 0)));
        assert!(n.contains(&("Id", SymbolKind::TypeAlias, 0)));
        assert!(n.contains(&("inner", SymbolKind::Module, 0)));
        assert!(n.contains(&("my_macro", SymbolKind::Macro, 0)));
        assert!(n.contains(&("area", SymbolKind::Method, 1)));
        assert!(n.contains(&("local", SymbolKind::Method, 2)));
        assert_eq!(symbols.iter().filter(|s| s.name == "Area").count(), 1);
    }

    #[test]
    fn python_symbols_indent_and_methods() {
        let text = "\
# MARK: Helpers
import os

def top(a):
    return a

class Greeter:
    def hello(self):
        def inner():
            pass
        return inner
";
        let symbols = extract_symbols(text, LanguageId::Python);
        let n = names(&symbols);
        assert!(n.contains(&("Helpers", SymbolKind::Section, 0)));
        assert!(n.contains(&("top", SymbolKind::Function, 0)));
        assert!(n.contains(&("Greeter", SymbolKind::Class, 0)));
        assert!(n.contains(&("hello", SymbolKind::Method, 1)));
        assert!(n.contains(&("inner", SymbolKind::Method, 2)));
    }

    #[test]
    fn c_family_symbols() {
        let text = "\
#include <stdio.h>
#define MAX 10
typedef struct Point { int x; } Point;
enum Color { RED };
static int add(int a, int b) {
    return a + b;
}
// MARK: - API
#pragma mark Interface
void API::run(void) {
}
";
        let symbols = extract_symbols(text, LanguageId::Cpp);
        let n = names(&symbols);
        assert!(n.contains(&("MAX", SymbolKind::Macro, 0)));
        assert!(n.contains(&("Point", SymbolKind::Struct, 0)));
        assert!(n.contains(&("Color", SymbolKind::Enum, 0)));
        assert!(n.contains(&("add", SymbolKind::Function, 0)));
        assert!(n.contains(&("API", SymbolKind::Section, 0)));
        assert!(n.contains(&("Interface", SymbolKind::Section, 0)));
        assert!(n.contains(&("run", SymbolKind::Method, 0)));

        // The C extractor is the same table, and Objective-C++ combines the C
        // and Objective-C tables.
        let c = extract_symbols("int main(void) { return 0; }\n", LanguageId::C);
        assert!(names(&c).contains(&("main", SymbolKind::Function, 0)));
        let objcpp = extract_symbols("@interface Mixed\n@end\n", LanguageId::ObjectiveCpp);
        assert!(names(&objcpp).contains(&("Mixed", SymbolKind::Class, 0)));
    }

    #[test]
    fn objective_c_symbols() {
        let text = "\
#import <Foundation/Foundation.h>

@interface Person : NSObject
@property (nonatomic, copy) NSString *name;
- (void)greet;
+ (instancetype)person;
@end

@implementation Person
- (void)greet {
}
@end

@protocol Greeting <NSObject>
@end
";
        let symbols = extract_symbols(text, LanguageId::ObjectiveC);
        let n = names(&symbols);
        assert!(n.contains(&("Person", SymbolKind::Class, 0)));
        assert!(n.contains(&("name", SymbolKind::Property, 0)));
        assert!(n.contains(&("greet", SymbolKind::Method, 0)));
        assert!(n.contains(&("person", SymbolKind::Method, 0)));
        assert!(n.contains(&("Greeting", SymbolKind::Protocol, 0)));
    }

    #[test]
    fn csharp_and_java_symbols() {
        let cs = "\
namespace Demo {
    public sealed class Service {
        public async Task<int> RunAsync(int x) {
            return x;
        }
        public Service() {
        }
    }
    public record Pair(int A, int B);
    public interface IThing { }
    public enum Mode { A }
}
";
        let symbols = extract_symbols(cs, LanguageId::CSharp);
        let n = names(&symbols);
        assert!(n.contains(&("Demo", SymbolKind::Namespace, 0)));
        assert!(n.contains(&("Service", SymbolKind::Class, 1)));
        assert!(n.contains(&("RunAsync", SymbolKind::Method, 2)));
        assert!(n.contains(&("Pair", SymbolKind::Struct, 1)));
        assert!(n.contains(&("IThing", SymbolKind::Interface, 1)));
        assert!(n.contains(&("Mode", SymbolKind::Enum, 1)));

        let java = "\
package demo;
public class Main {
    public static void main(String[] args) {
        System.out.println(1);
    }
    interface Callback { void run(); }
    enum Kind { A }
    record Point(int x, int y) {}
}
";
        let symbols = extract_symbols(java, LanguageId::Java);
        let n = names(&symbols);
        assert!(n.contains(&("Main", SymbolKind::Class, 0)));
        assert!(n.contains(&("main", SymbolKind::Method, 1)));
        assert!(n.contains(&("Callback", SymbolKind::Interface, 1)));
        assert!(n.contains(&("Kind", SymbolKind::Enum, 1)));
        assert!(n.contains(&("Point", SymbolKind::Struct, 1)));
    }

    #[test]
    fn go_symbols() {
        let text = "\
package main

const Answer = 42

func main() {
}

func (s *Server) Start(port int) error {
    return nil
}

type Server struct {
    port int
}

type Handler interface {
    Serve()
}

type Alias = Server
";
        let symbols = extract_symbols(text, LanguageId::Go);
        let n = names(&symbols);
        assert!(n.contains(&("Answer", SymbolKind::Constant, 0)));
        assert!(n.contains(&("main", SymbolKind::Function, 0)));
        assert!(n.contains(&("Start", SymbolKind::Method, 0)));
        assert!(n.contains(&("Server", SymbolKind::Struct, 0)));
        assert!(n.contains(&("Handler", SymbolKind::Interface, 0)));
        assert!(n.contains(&("Alias", SymbolKind::TypeAlias, 0)));
    }

    #[test]
    fn javascript_and_typescript_symbols() {
        let js = "\
export function greet(name) {}
export default class App {
    constructor() {}
    async load(id) {
        const helper = (x) => x;
        if (id) {
            return helper(id);
        }
    }
}
export const run = async () => {};
";
        let symbols = extract_symbols(js, LanguageId::JavaScript);
        let n = names(&symbols);
        assert!(n.contains(&("greet", SymbolKind::Function, 0)));
        assert!(n.contains(&("App", SymbolKind::Class, 0)));
        assert!(n.contains(&("constructor", SymbolKind::Method, 1)));
        assert!(n.contains(&("load", SymbolKind::Method, 1)));
        assert!(n.contains(&("run", SymbolKind::Function, 0)));
        assert!(!n.iter().any(|(name, ..)| *name == "if"));

        let ts = "\
export interface User { name: string }
export type Id = string;
export enum Mode { A }
export namespace Api {}
export class Store {
    get(id: Id): User {
        return { name: id };
    }
}
";
        let symbols = extract_symbols(ts, LanguageId::TypeScript);
        let n = names(&symbols);
        assert!(n.contains(&("User", SymbolKind::Interface, 0)));
        assert!(n.contains(&("Id", SymbolKind::TypeAlias, 0)));
        assert!(n.contains(&("Mode", SymbolKind::Enum, 0)));
        assert!(n.contains(&("Api", SymbolKind::Namespace, 0)));
        assert!(n.contains(&("Store", SymbolKind::Class, 0)));
        assert!(n.contains(&("get", SymbolKind::Method, 1)));
    }

    #[test]
    fn kotlin_symbols() {
        let text = "\
package demo

data class User(val name: String)

fun top() {}

object Singleton {}

interface Repo {
    fun load()
}

typealias Rows = List<String>

class Service {
    suspend fun fetch(id: Int): String = \"\"
}
";
        let symbols = extract_symbols(text, LanguageId::Kotlin);
        let n = names(&symbols);
        assert!(n.contains(&("User", SymbolKind::Class, 0)));
        assert!(n.contains(&("top", SymbolKind::Function, 0)));
        assert!(n.contains(&("Singleton", SymbolKind::Class, 0)));
        assert!(n.contains(&("Repo", SymbolKind::Interface, 0)));
        assert!(n.contains(&("load", SymbolKind::Method, 1)));
        assert!(n.contains(&("Rows", SymbolKind::TypeAlias, 0)));
        assert!(n.contains(&("fetch", SymbolKind::Method, 1)));
    }

    #[test]
    fn swift_symbols() {
        let text = "\
import Foundation

struct Point {
    func distance() -> Double { 0 }
}

protocol Shape {
    func area() -> Double
}

extension Point: Shape {
    func area() -> Double { 0 }
}

public class Renderer {
    private func draw() {}
}
typealias Distance = Double
enum Mode { case a }
";
        let symbols = extract_symbols(text, LanguageId::Swift);
        let n = names(&symbols);
        assert!(n.contains(&("Point", SymbolKind::Struct, 0)));
        assert!(n.contains(&("distance", SymbolKind::Method, 1)));
        assert!(n.contains(&("Shape", SymbolKind::Protocol, 0)));
        assert!(n.contains(&("area", SymbolKind::Method, 1)));
        assert!(n.contains(&("Point", SymbolKind::Extension, 0)));
        assert!(n.contains(&("Renderer", SymbolKind::Class, 0)));
        assert!(n.contains(&("draw", SymbolKind::Method, 1)));
        assert!(n.contains(&("Distance", SymbolKind::TypeAlias, 0)));
        assert!(n.contains(&("Mode", SymbolKind::Enum, 0)));
    }

    #[test]
    fn markdown_headings_and_fences() {
        let text = "\
# Title
Intro

## Section
```rust
# not a heading
```
### Sub
Setext
------
";
        let symbols = extract_symbols(text, LanguageId::Markdown);
        let n = names(&symbols);
        assert_eq!(n[0], ("Title", SymbolKind::Heading, 0));
        assert_eq!(n[1], ("Section", SymbolKind::Heading, 1));
        assert_eq!(n[2], ("Sub", SymbolKind::Heading, 2));
        assert_eq!(n.len(), 3);
    }

    #[test]
    fn latex_sections() {
        let text = "\
\\documentclass{article}
\\begin{document}
\\chapter{Intro}
\\section{Methods}
\\subsection{Details}
\\subsubsection{More}
\\paragraph{Note}
\\end{document}
";
        let symbols = extract_symbols(text, LanguageId::Latex);
        let n = names(&symbols);
        assert_eq!(n[0], ("Intro", SymbolKind::Section, 0));
        assert_eq!(n[1], ("Methods", SymbolKind::Section, 1));
        assert_eq!(n[2], ("Details", SymbolKind::Section, 2));
        assert_eq!(n[3], ("More", SymbolKind::Section, 3));
        assert_eq!(n[4], ("Note", SymbolKind::Section, 4));
    }

    #[test]
    fn lua_perl_ruby_shell_symbols() {
        let lua = "\
local function helper()
end
function M.run()
end
M.extra = function() end
";
        let symbols = extract_symbols(lua, LanguageId::Lua);
        let n = names(&symbols);
        assert!(n.contains(&("helper", SymbolKind::Function, 0)));
        assert!(n.contains(&("M.run", SymbolKind::Function, 0)));
        assert!(n.contains(&("M.extra", SymbolKind::Function, 0)));

        let perl = "package Demo::Thing;\nsub run {\n}\nuse constant PI => 3.14;\n";
        let symbols = extract_symbols(perl, LanguageId::Perl);
        let n = names(&symbols);
        assert!(n.contains(&("Demo::Thing", SymbolKind::Module, 0)));
        assert!(n.contains(&("run", SymbolKind::Function, 0)));
        assert!(n.contains(&("PI", SymbolKind::Constant, 0)));

        let ruby = "module Outer\n  class Inner\n    def call(x)\n    end\n  end\nend\n";
        let symbols = extract_symbols(ruby, LanguageId::Ruby);
        let n = names(&symbols);
        assert!(n.contains(&("Outer", SymbolKind::Module, 0)));
        assert!(n.contains(&("Inner", SymbolKind::Class, 1)));
        assert!(n.contains(&("call", SymbolKind::Method, 2)));

        let sh = "function deploy {\n  echo hi\n}\nbuild() {\n  echo x\n}\n";
        let symbols = extract_symbols(sh, LanguageId::Shell);
        let n = names(&symbols);
        assert!(n.contains(&("deploy", SymbolKind::Function, 0)));
        assert!(n.contains(&("build", SymbolKind::Function, 0)));
    }

    #[test]
    fn php_css_html_haskell_applescript_symbols() {
        let php = "\
<?php
namespace Demo;
abstract class Base {}
interface Runner {}
trait Loggable {}
enum Suit {}
class App extends Base {
    public function run(): void {}
}
";
        let symbols = extract_symbols(php, LanguageId::Php);
        let n = names(&symbols);
        assert!(n.contains(&("Base", SymbolKind::Class, 0)));
        assert!(n.contains(&("Runner", SymbolKind::Interface, 0)));
        assert!(n.contains(&("Loggable", SymbolKind::Trait, 0)));
        assert!(n.contains(&("Suit", SymbolKind::Enum, 0)));
        assert!(n.contains(&("App", SymbolKind::Class, 0)));
        assert!(n.contains(&("run", SymbolKind::Method, 1)));

        let css = "body {\n  color: red;\n}\n.card > .title, #id {\n}\n@media screen {\n}\n";
        let symbols = extract_symbols(css, LanguageId::Css);
        let n = names(&symbols);
        assert!(n.contains(&("body", SymbolKind::Selector, 0)));
        assert!(n.contains(&(".card > .title, #id", SymbolKind::Selector, 0)));
        assert!(!n.iter().any(|(name, ..)| *name == "@media screen"));

        let html = "<h1>Welcome</h1>\n<div id=\"main\"></div>\n<span id='side'></span>\n";
        let symbols = extract_symbols(html, LanguageId::Html);
        let n = names(&symbols);
        assert!(n.contains(&("Welcome", SymbolKind::Heading, 0)));
        assert!(n.contains(&("main", SymbolKind::Id, 0)));
        assert!(n.contains(&("side", SymbolKind::Id, 0)));

        let hs = "\
module Demo where
data Shape = Circle
type Name = String
class Showable a where
greet :: String -> String
greet x = x
";
        let symbols = extract_symbols(hs, LanguageId::Haskell);
        let n = names(&symbols);
        assert!(n.contains(&("Demo", SymbolKind::Module, 0)));
        assert!(n.contains(&("Shape", SymbolKind::Struct, 0)));
        assert!(n.contains(&("Name", SymbolKind::TypeAlias, 0)));
        assert!(n.contains(&("Showable", SymbolKind::Class, 0)));
        assert!(n.contains(&("greet", SymbolKind::Function, 0)));

        let scpt = "on run\nend run\non greet(name)\nend greet\nscript Helper\nend script\nproperty count : 0\n";
        let symbols = extract_symbols(scpt, LanguageId::AppleScript);
        let n = names(&symbols);
        assert!(n.contains(&("run", SymbolKind::Handler, 0)));
        assert!(n.contains(&("greet", SymbolKind::Handler, 0)));
        assert!(n.contains(&("Helper", SymbolKind::Class, 0)));
        assert!(n.contains(&("count", SymbolKind::Property, 0)));
    }

    #[test]
    fn unicode_names_and_columns() {
        let text = "struct Größe {\n  fn größe() {}\n}\n";
        let symbols = extract_symbols(text, LanguageId::Rust);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Größe");
        assert_eq!(symbols[1].name, "größe");
        assert_eq!(symbols[1].line, 1);
        // "  fn " is 5 UTF-16 units
        assert_eq!(symbols[1].column, 5);
    }

    #[test]
    fn crlf_lines_and_plain_text() {
        let text = "struct A {\r\n  fn b() {}\r\n}\r\n";
        let symbols = extract_symbols(text, LanguageId::Rust);
        assert_eq!(symbols[0].line, 0);
        assert_eq!(symbols[1].line, 1);
        assert_eq!(symbols[0].name, "A");
        assert!(extract_symbols("fn a() {}", LanguageId::PlainText).is_empty());
    }

    #[test]
    fn big_file_is_fast_enough() {
        let mut text = String::new();
        for i in 0..5_000 {
            text.push_str(&format!("fn function_{i}(x: i32) -> i32 {{ x }}\n"));
        }
        let start = std::time::Instant::now();
        let symbols = extract_symbols(&text, LanguageId::Rust);
        assert_eq!(symbols.len(), 5_000);
        assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    }

    #[test]
    fn symbol_kind_labels() {
        assert_eq!(SymbolKind::Method.label(), "Method");
        assert_eq!(SymbolKind::TypeAlias.label(), "Type Alias");
        assert_eq!(LanguageId::CSharp.name(), "C#");
    }
}
