//! New-file templates.
//!
//! A language has any number of [`TemplateDef`] items. Items that share a
//! `name` form one multi-file template (`Class` = `.h` + `.cpp`); see
//! [`group_templates`]. [`render_template`] turns the text of one item into
//! the document's initial content: placeholders are substituted, indentation
//! follows the user's tab preferences and the `<##>` caret marker is removed
//! and reported as an offset.
//!
//! # Placeholders
//!
//! Each is wrapped in three underscores on both sides:
//!
//! | Placeholder | Value |
//! |--|--|
//! | `___FILENAME___` | `MyFile.java` |
//! | `___FILEBASENAME___` | `MyFile` |
//! | `___FILEEXTENSION___` | `java` |
//! | `___FILEIDENTIFIER___` | the base name with non-word characters removed |
//! | `___DATE___` | the creation date |
//! | `___USERNAME___` | the login name |
//! | `___FULLUSERNAME___` | the user's full name |
//!
//! `<##>` marks the caret. Xcode style `<#name#>` placeholders are left in the
//! text and reported by [`find_placeholders`].

use serde::{Deserialize, Serialize};

use crate::model::IndentStyle;

/// The caret marker.
pub const CARET_MARKER: &str = "<##>";

/// The document a very first launch opens (a Shell Script containing this
/// text, caret at the marker).
pub const FIRST_LAUNCH_TEXT: &str = "#!/bin/bash\necho \"Welcome to ScratchRunner!\"<##>";

/// The language id of [`FIRST_LAUNCH_TEXT`].
pub const FIRST_LAUNCH_LANGUAGE: &str = "shell";

fn is_false(value: &bool) -> bool {
    !*value
}

/// One template item of a language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateDef {
    /// Template name; items with the same name form a multi-file template.
    pub name: String,
    /// File extension without the dot.
    pub extension: String,
    /// The text, with tabs as indentation units, `<##>` caret marker and
    /// `___PLACEHOLDERS___`.
    pub text: String,
    /// Used by *New File* in this language. At most one per language; not
    /// combinable with `name_on_creation`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    /// Ask for a name and a location when creating it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub name_on_creation: bool,
}

/// All items of one template name (one file per item).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateGroup<'a> {
    /// The shared name.
    pub name: &'a str,
    /// The items, in definition order.
    pub files: Vec<&'a TemplateDef>,
    /// Whether it is the language's default template.
    pub default: bool,
    /// Whether the user is asked for a name on creation.
    pub name_on_creation: bool,
}

/// Groups template items by name, keeping the order of first appearance.
pub fn group_templates(templates: &[TemplateDef]) -> Vec<TemplateGroup<'_>> {
    let mut groups: Vec<TemplateGroup<'_>> = Vec::new();
    for template in templates {
        match groups.iter_mut().find(|g| g.name == template.name) {
            Some(group) => {
                group.files.push(template);
                group.default |= template.default;
                group.name_on_creation |= template.name_on_creation;
            }
            None => groups.push(TemplateGroup {
                name: &template.name,
                files: vec![template],
                default: template.default,
                name_on_creation: template.name_on_creation,
            }),
        }
    }
    groups
}

/// The values placeholders are replaced with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateContext {
    /// The new file's name with extension (`Untitled.java`).
    pub file_name: String,
    /// The creation date as it should appear (`11/16/26`).
    pub date: String,
    /// The login name.
    pub user_name: String,
    /// The user's full name.
    pub full_user_name: String,
    /// The user's indentation preference: leading tabs of the template are
    /// converted when it asks for spaces.
    pub indent: IndentStyle,
}

impl TemplateContext {
    /// A context for `file_name` with today's date (local, `M/D/YY`, like
    /// Xcode's `___DATE___`) and the current user (`USER`/`LOGNAME`; the full
    /// name from `id -F` on macOS, falling back to the login name).
    pub fn for_file(file_name: &str, indent: IndentStyle) -> Self {
        let user_name = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_default();
        let full_user_name = full_name().unwrap_or_else(|| user_name.clone());
        Self {
            file_name: file_name.to_owned(),
            date: chrono::Local::now().format("%-m/%-d/%y").to_string(),
            user_name,
            full_user_name,
            indent,
        }
    }

    /// A fully explicit context (tests, previews).
    pub fn new(
        file_name: &str,
        date: &str,
        user_name: &str,
        full_user_name: &str,
        indent: IndentStyle,
    ) -> Self {
        Self {
            file_name: file_name.to_owned(),
            date: date.to_owned(),
            user_name: user_name.to_owned(),
            full_user_name: full_user_name.to_owned(),
            indent,
        }
    }
}

fn full_name() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-F")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// A rendered template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTemplate {
    /// The document text (caret marker removed, placeholders substituted).
    pub text: String,
    /// Byte offset of the caret (the first `<##>`), `None` when the template
    /// has no marker.
    pub caret_offset: Option<usize>,
    /// The caret as (line, column), both from zero, columns in characters;
    /// `None` without a marker.
    pub caret: Option<(u32, u32)>,
}

impl RenderedTemplate {
    /// The caret position, or the start of the document without a marker.
    pub fn caret_or_start(&self) -> (u32, u32) {
        self.caret.unwrap_or((0, 0))
    }

    /// The caret byte offset, or the end of the text without a marker (where a
    /// new document's caret belongs when nothing marks it).
    pub fn caret_offset_or_end(&self) -> usize {
        self.caret_offset.unwrap_or(self.text.len())
    }
}

/// Renders template text: substitutes placeholders, converts leading tabs to
/// the indentation preference, removes `<##>` and reports where the first one
/// was.
///
/// ```
/// use scratch_language::{render_template, TemplateContext, IndentStyle};
///
/// let ctx = TemplateContext::new("Main.go", "1/2/26", "me", "Me", IndentStyle::default());
/// let out = render_template("package main\n\nfunc main() {\n\t<##>\n}", &ctx);
/// assert_eq!(out.text, "package main\n\nfunc main() {\n\t\n}");
/// assert_eq!(out.caret, Some((3, 1)));
/// ```
pub fn render_template(template: &str, ctx: &TemplateContext) -> RenderedTemplate {
    let (base, ext) = split_file_name(&ctx.file_name);
    let identifier: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let mut text = template.to_owned();
    // Longer names first: none is a substring of another, but the order keeps
    // the header-guard idiom `____FILEBASENAME_______FILEEXTENSION____` working.
    for (placeholder, value) in [
        ("___FILEIDENTIFIER___", identifier.as_str()),
        ("___FILEBASENAME___", base),
        ("___FILEEXTENSION___", ext),
        ("___FILENAME___", ctx.file_name.as_str()),
        ("___FULLUSERNAME___", ctx.full_user_name.as_str()),
        ("___USERNAME___", ctx.user_name.as_str()),
        ("___DATE___", ctx.date.as_str()),
    ] {
        text = text.replace(placeholder, value);
    }
    if ctx.indent.soft_tabs {
        text = tabs_to_spaces(&text, ctx.indent.width);
    }
    match text.find(CARET_MARKER) {
        Some(offset) => {
            let before = &text[..offset];
            let line = before.matches('\n').count() as u32;
            let column = before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count() as u32;
            let text = text.replace(CARET_MARKER, "");
            RenderedTemplate {
                text,
                caret_offset: Some(offset),
                caret: Some((line, column)),
            }
        }
        None => RenderedTemplate {
            text,
            caret_offset: None,
            caret: None,
        },
    }
}

fn split_file_name(file_name: &str) -> (&str, &str) {
    match file_name.rfind('.') {
        Some(dot) if dot > 0 => (&file_name[..dot], &file_name[dot + 1..]),
        _ => (file_name, ""),
    }
}

fn tabs_to_spaces(text: &str, width: u8) -> String {
    let unit = " ".repeat(usize::from(width.max(1)));
    text.split('\n')
        .map(|line| {
            let tabs = line.chars().take_while(|c| *c == '\t').count();
            if tabs == 0 {
                line.to_owned()
            } else {
                format!("{}{}", unit.repeat(tabs), &line[tabs..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte ranges of Xcode-style `<#placeholder#>` tokens in `text`.
pub fn find_placeholders(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(start) = text[from..].find("<#") {
        let start = from + start;
        if text[start..].starts_with(CARET_MARKER) {
            from = start + CARET_MARKER.len();
            continue;
        }
        match text[start + 2..].find("#>") {
            Some(end) => {
                let end = start + 2 + end + 2;
                out.push(start..end);
                from = end;
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(file: &str, indent: IndentStyle) -> TemplateContext {
        TemplateContext::new(file, "1/2/26", "tb", "Tom B", indent)
    }

    #[test]
    fn caret_marker_is_removed_and_located() {
        let out = render_template(
            "def main():\n\t<##>\n",
            &ctx("a.py", IndentStyle::default()),
        );
        assert_eq!(out.text, "def main():\n\t\n");
        assert_eq!(out.caret, Some((1, 1)));
        assert_eq!(out.caret_offset, Some(13));
    }

    #[test]
    fn no_marker_means_no_caret() {
        let out = render_template("fn main() {}\n", &ctx("a.rs", IndentStyle::default()));
        assert_eq!(out.caret, None);
        assert_eq!(out.caret_or_start(), (0, 0));
        assert_eq!(out.caret_offset_or_end(), 13);
    }

    #[test]
    fn only_first_marker_counts_but_all_are_removed() {
        let out = render_template("<##>a<##>", &ctx("a", IndentStyle::default()));
        assert_eq!(out.text, "a");
        assert_eq!(out.caret_offset, Some(0));
    }

    #[test]
    fn columns_count_characters() {
        let out = render_template("é好<##>", &ctx("a", IndentStyle::default()));
        assert_eq!(out.caret, Some((0, 2)));
        assert_eq!(out.caret_offset, Some(5));
    }

    #[test]
    fn placeholders_are_substituted() {
        let t = "// ___FILENAME___\n// ___FILEBASENAME___ ___FILEEXTENSION___ ___FILEIDENTIFIER___\n// by ___FULLUSERNAME___ (___USERNAME___) on ___DATE___";
        let out = render_template(t, &ctx("My-File.java", IndentStyle::default()));
        assert_eq!(
            out.text,
            "// My-File.java\n// My-File java MyFile\n// by Tom B (tb) on 1/2/26"
        );
    }

    #[test]
    fn header_guard_idiom() {
        let t = "#ifndef ____FILEBASENAME_______FILEEXTENSION____\n#define ____FILEBASENAME_______FILEEXTENSION____\n#endif";
        let out = render_template(t, &ctx("Foo.h", IndentStyle::default()));
        assert_eq!(out.text, "#ifndef _Foo_h_\n#define _Foo_h_\n#endif");
    }

    #[test]
    fn soft_tabs_convert_leading_tabs_only() {
        let indent = IndentStyle {
            soft_tabs: true,
            width: 2,
        };
        let out = render_template("a\n\tb\t c\n\t\td", &ctx("a", indent));
        assert_eq!(out.text, "a\n  b\t c\n    d");
    }

    #[test]
    fn grouping_keeps_order_and_merges_flags() {
        let item = |name: &str, ext: &str, default: bool, noc: bool| TemplateDef {
            name: name.into(),
            extension: ext.into(),
            text: String::new(),
            default,
            name_on_creation: noc,
        };
        let templates = vec![
            item("Main File", "cpp", true, false),
            item("Class", "h", false, true),
            item("Class", "cpp", false, true),
        ];
        let groups = group_templates(&templates);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Main File");
        assert!(groups[0].default);
        assert_eq!(groups[1].files.len(), 2);
        assert!(groups[1].name_on_creation);
    }

    #[test]
    fn xcode_placeholders_are_found() {
        let text = "a <#class#> b <##> <#x##y#>";
        let found: Vec<_> = find_placeholders(text)
            .into_iter()
            .map(|r| &text[r])
            .collect();
        assert_eq!(found, ["<#class#>", "<#x##y#>"]);
    }

    #[test]
    fn first_launch_text_has_a_caret() {
        let out = render_template(FIRST_LAUNCH_TEXT, &ctx("x.sh", IndentStyle::default()));
        assert_eq!(out.text, "#!/bin/bash\necho \"Welcome to ScratchRunner!\"");
        assert_eq!(out.caret, Some((1, 32)));
    }
}
