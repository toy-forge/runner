//! Native `About ScratchRunner` dialog window and `ScratchRunner Help` (⌘?)
//! multi-topic documentation window.
//!
//! Motion inventory: uses only `scratch_ui_kit::motion` press/wash recipes on
//! selectable topic rows; all colors, radii, typography and spacing read from
//! `scratch_ui_kit::tokens`.

use nsh_core::Size;
use nsh_core::desktop::{WindowAttributes, WindowKind};
use nsh_core::element::desktop::{OpenChange, open_window, window};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

const ABOUT_WINDOW_ID: &str = "about";
const HELP_WINDOW_ID: &str = "help";

#[derive(Clone, Default, PartialEq)]
struct AboutOpen(bool);

#[derive(Clone, Default, PartialEq)]
struct HelpState {
    open: bool,
    topic: usize,
    query: String,
}

struct HelpSection {
    heading: &'static str,
    body: &'static str,
    shortcuts: &'static [(&'static str, &'static str)],
}

struct HelpTopic {
    title: &'static str,
    summary: &'static str,
    sections: &'static [HelpSection],
}

const TOPICS: &[HelpTopic] = &[
    HelpTopic {
        title: "Getting Started",
        summary: "ScratchRunner opens instantly into a ready-to-run scratchpad without creating a project.",
        sections: &[
            HelpSection {
                heading: "Scratchpad Workflow",
                body: "Open a new tab, choose your language from the toolbar capsule or Language switcher, write code, and press Run. Unsaved documents are preserved across launches via crash-safe session recovery.",
                shortcuts: &[("New Tab", "mod+t"), ("Run Document", "mod+r"), ("Quick Open", "mod+shift+o")],
            },
            HelpSection {
                heading: "Workspace Panes",
                body: "Toggle the File Navigator sidebar, bottom or right Console pane, and WebKit Documentation sidebar cleanly from the toolbar or View menu.",
                shortcuts: &[("File Navigator", "cmd+shift+h"), ("Console", "mod+shift+c"), ("Documentation", "mod+shift+i")],
            },
        ],
    },
    HelpTopic {
        title: "Running & Compiling",
        summary: "Run scripts, single-file programs, or multi-step compiled languages with full stdin/stdout streaming.",
        sections: &[
            HelpSection {
                heading: "Run Modes",
                body: "Long-press or right-click the Run button for Run, Run & Debug, Run Without Debugging, Run Selection, or Compile Only. Large console output (>128 KiB) prompts with Keep Tail, Save to File, or Clear & Continue.",
                shortcuts: &[("Run", "mod+r"), ("Compile / Diagnostics", "mod+b"), ("Stop", "mod+.")],
            },
            HelpSection {
                heading: "Output Formats",
                body: "Plain text ANSI streams, interactive REPL input with Up/Down history, or Web/Markdown preview rendered via WKWebView with live reload and JavaScript Inspector.",
                shortcuts: &[("Clear Console", "mod+k")],
            },
        ],
    },
    HelpTopic {
        title: "Run Settings & Arguments",
        summary: "Configure per-document command-line arguments, stdin input, compiler flags, and environment variables.",
        sections: &[
            HelpSection {
                heading: "Document Overrides",
                body: "Open Edit Run Settings… to supply Arguments, Input (stdin) sent when execution starts, Compiler Flags appended during compilation, or working-directory and script overrides.",
                shortcuts: &[("Run Settings", "mod+shift+r")],
            },
            HelpSection {
                heading: "Defaults & Built-in Variables",
                body: "Click the Help (?) button inside Run Settings to review variables such as $filename, $compiler, and CR_ENCODING, or choose Make Default to persist flags for new documents in the same language.",
                shortcuts: &[],
            },
        ],
    },
    HelpTopic {
        title: "Interactive Debugging",
        summary: "Step through Python, Node.js, C/C++/Rust/Go (LLDB/GDB), PHP (Xdebug), Java, Perl, or Bash.",
        sections: &[
            HelpSection {
                heading: "Breakpoints & Conditions",
                body: "Click in the editor gutter to set breakpoints, or right-click a gutter mark to add a Condition expression, Ignore Count, Log Message / Sound / Action, and Auto-Continue.",
                shortcuts: &[("Toggle Breakpoint", "mod+\\"), ("Run & Debug", "mod+alt+r")],
            },
            HelpSection {
                heading: "Stepping & Inspection",
                body: "When paused, use the split Stack | Variables debugger panes, evaluate expressions live in the Debugger Console prompt, or hover identifiers in the editor for instant value popovers.",
                shortcuts: &[("Pause / Continue", "ctrl+1"), ("Step Over", "ctrl+2"), ("Step Into", "ctrl+3"), ("Step Out", "ctrl+4")],
            },
        ],
    },
    HelpTopic {
        title: "Language Servers & Diagnostics",
        summary: "Completions, Go to Definition, Rename, Signature Help, Code Actions, Formatting, and Linters.",
        sections: &[
            HelpSection {
                heading: "Semantic Navigation",
                body: "Jump to declarations across workspace files, inspect parameter signatures, rename symbols safely, apply quick fixes via the lightbulb menu, and inspect running LSP servers from the status bar.",
                shortcuts: &[("Go to Definition", "f12"), ("Signature Help", "cmd+shift+space"), ("Rename Symbol", "f2"), ("Format Document", "ctrl+shift+i")],
            },
            HelpSection {
                heading: "On-Edit & Tool Diagnostics",
                body: "Configure external linters, regular-expression or script parsers, line highlighting, and automatic on-edit checks in Text Editing > Diagnostics or per language.",
                shortcuts: &[("Run Diagnostics / Compile", "mod+b")],
            },
        ],
    },
    HelpTopic {
        title: "Documentation Lookup",
        summary: "Quick WebKit-rendered API lookup for the active symbol or configured developer documentation sites.",
        sections: &[
            HelpSection {
                heading: "Quick Lookup & Sources",
                body: "Look up the identifier at the caret in the embedded Documentation browser. Switch global and per-language doc sets from the source strip, bookmark pages, and jump to headings via the Table of Contents footer.",
                shortcuts: &[("Look Up Current Symbol", "mod+i"), ("Toggle Documentation View", "mod+shift+i")],
            },
        ],
    },
    HelpTopic {
        title: "Find & Project Replace",
        summary: "In-file regex search/replace with saved patterns and multi-file folder search with Undo journal.",
        sections: &[
            HelpSection {
                heading: "In-File Find Bar",
                body: "Supports regular expressions, Insert Pattern helpers, Recent Searches history, Saved Patterns, Current Search as Regular Expression, Replace in Selection, and multi-caret occurrence selection.",
                shortcuts: &[("Find", "mod+f"), ("Find and Replace", "mod+alt+f"), ("Select Next Occurrence", "mod+e"), ("Select All Occurrences", "cmd+ctrl+e")],
            },
            HelpSection {
                heading: "Project Search & Undo",
                body: "Switch the left sidebar to Search to search folders or custom path lists with Whole Word and Regex modes, preview per-file matches, replace across disk with confirmation, and Undo Replacements cleanly.",
                shortcuts: &[],
            },
        ],
    },
    HelpTopic {
        title: "Themes & Key Bindings",
        summary: "Customize TextMate color themes and record menu, command, or Vim/compound key bindings.",
        sections: &[
            HelpSection {
                heading: "Color Themes & Printing",
                body: "Choose from built-in light and dark palettes including ScratchRunner, Xcode, GitHub, One Dark, Solarized, Tomorrow, Dracula, and Nord, or import/edit .tmTheme and .crTheme files.",
                shortcuts: &[("Preferences", "mod+,"), ("Print Source", "mod+p")],
            },
            HelpSection {
                heading: "Interactive Key Recorder & Compounds",
                body: "Click any shortcut pill in Key Bindings to record a keystroke live with AppKit capture (Esc cancels, Backspace clears). Author JSON compound commands or load .crBindings sets.",
                shortcuts: &[],
            },
        ],
    },
    HelpTopic {
        title: "Shell Environment",
        summary: "Control PATH discovery, login-shell flags, and environment variables passed to every run.",
        sections: &[
            HelpSection {
                heading: "PATH & Variables",
                body: "By default ScratchRunner captures your login shell PATH and exposes a Refresh PATH button in Preferences > Shell. Override PATH or add custom variables when isolating toolchains.",
                shortcuts: &[],
            },
        ],
    },
];

/// Opens the `About ScratchRunner` dialog window.
pub fn open_about() {
    if global::<AboutOpen>().with_untracked(|a| a.0) {
        let _ = open_window(window().id(ABOUT_WINDOW_ID));
    } else {
        global::<AboutOpen>().set(AboutOpen(true));
    }
}

/// Opens the `ScratchRunner Help` documentation window.
pub fn open_help() {
    if global::<HelpState>().with_untracked(|h| h.open) {
        let _ = open_window(window().id(HELP_WINDOW_ID));
    } else {
        global::<HelpState>().update(|h| h.open = true);
    }
}

/// Mounts both secondary windows declaratively at the app root.
pub fn overlay() -> impl IntoElement {
    view()
        .child(about_window())
        .child(help_window())
}

fn about_window() -> impl IntoElement {
    let mut attributes = WindowAttributes::default();
    attributes.resizable = Some(false);
    let version = env!("CARGO_PKG_VERSION");
    window()
        .id(ABOUT_WINDOW_ID)
        .kind(WindowKind::Dialog)
        .title("About ScratchRunner")
        .default_size(Size::new(px(360.0), px(270.0)))
        .attributes(attributes)
        .open(|| global::<AboutOpen>().get().0)
        .on_open_change(|change: &OpenChange| {
            if !change.open {
                global::<AboutOpen>().set(AboutOpen(false));
            }
        })
        .content(move || {
            view()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .bg(tokens::background)
                .text_color(tokens::foreground)
                .child(glyph(IconName::AppMark, IconSize::Xl, Tone::Primary))
                .child(type_title().apply(text("ScratchRunner")))
                .child(
                    type_caption()
                        .apply(text(format!("Version {version} (Build 1)")).text_color(tokens::muted_foreground)),
                )
                .child(
                    type_body()
                        .apply(
                            text("Native Multi-Language Scratchpad & Debugger")
                                .text_center()
                                .text_color(tokens::muted_foreground),
                        ),
                )
                .child(
                    type_caption()
                        .apply(text("Copyright © 2026 ScratchRunner Contributors").text_color(tokens::muted_foreground)),
                )
        })
}

fn help_window() -> impl IntoElement {
    let mut attributes = WindowAttributes::default();
    attributes.min_size = Some(Some(Size::new(px(640.0), px(420.0))));
    attributes.resizable = Some(true);
    window()
        .id(HELP_WINDOW_ID)
        .kind(WindowKind::Normal)
        .title("ScratchRunner Help")
        .default_size(Size::new(px(840.0), px(580.0)))
        .attributes(attributes)
        .open(|| global::<HelpState>().with(|h| h.open))
        .on_open_change(|change: &OpenChange| {
            if !change.open {
                global::<HelpState>().update(|h| h.open = false);
            }
        })
        .content(help_content)
}

fn help_content() -> impl IntoElement {
    let state = global::<HelpState>();
    let filtered = computed(move || {
        let query = state.with(|s| s.query.trim().to_lowercase());
        TOPICS
            .iter()
            .enumerate()
            .filter(|(_, topic)| {
                query.is_empty()
                    || topic.title.to_lowercase().contains(&query)
                    || topic.summary.to_lowercase().contains(&query)
                    || topic.sections.iter().any(|sec| {
                        sec.heading.to_lowercase().contains(&query)
                            || sec.body.to_lowercase().contains(&query)
                    })
            })
            .map(|(idx, _)| idx)
            .collect::<Vec<_>>()
    });
    let active_topic = computed(move || {
        let selected = state.with(|s| s.topic);
        let indices = filtered.get();
        if indices.contains(&selected) {
            selected
        } else {
            indices.first().copied().unwrap_or(0)
        }
    });

    view()
        .flex_1()
        .flex_row()
        .min_h(px(0.0))
        .bg(tokens::background)
        .text_color(tokens::foreground)
        .child(
            view()
                .w(px(256.0))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .border_r_1()
                .border_color(tokens::border)
                .bg(tokens::sidebar)
                .child(
                    view().p_3().border_b_1().border_color(tokens::border).child(search_field(
                        "Filter help topics",
                        move || SharedString::from(state.with(|s| s.query.clone())),
                        None::<SharedString>,
                        |next: &str| global::<HelpState>().update(|s| s.query = next.to_owned()),
                        || global::<HelpState>().update(|s| s.query.clear()),
                        SearchFieldOptions::new().size(ControlSize::Sm),
                    )),
                )
                .child(
                    scroll().flex_1().min_h(px(0.0)).child(
                        view().flex_col().p_2().gap_1().child(each(
                            move || filtered.get(),
                            |idx| *idx,
                            move |row, _| {
                                let idx = row.get_untracked();
                                let topic = &TOPICS[idx];
                                let selected = computed(move || active_topic.get() == idx);
                                let item = view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .h(px(tokens::row_md()))
                                    .rounded(px(tokens::radius()))
                                    .cursor_pointer()
                                    .role(AccessibilityRole::Button)
                                    .bg(move || if selected.get() { tokens::accent() } else { tokens::sidebar() })
                                    .hover(|s| s.bg(tokens::control_hover))
                                    .on_press(move || global::<HelpState>().update(|s| s.topic = idx))
                                    .child(glyph(IconName::BookOpen, IconSize::Sm, Tone::Muted))
                                    .child(
                                        type_body().apply(
                                            text(topic.title)
                                                .number_of_lines(1)
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .text_color(move || {
                                                    if selected.get() {
                                                        tokens::primary()
                                                    } else {
                                                        tokens::foreground()
                                                    }
                                                }),
                                        ),
                                    );
                                motion::press(item)
                            },
                        )),
                    ),
                ),
        )
        .child(
            view().flex_1().min_w(px(0.0)).min_h(px(0.0)).child(scroll().flex_1().min_h(px(0.0)).child(dynamic(
                move || {
                    let indices = filtered.get();
                    if indices.is_empty() {
                        return view()
                            .flex_1()
                            .p_6()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(glyph(IconName::HelpCircle, IconSize::Xl, Tone::Muted))
                            .child(type_body().apply(text("No matching help topics.").text_color(tokens::muted_foreground)))
                            .into_element();
                    }
                    let topic = &TOPICS[active_topic.get()];
                    view()
                        .flex_col()
                        .gap_4()
                        .p_6()
                        .child(
                            view()
                                .flex_col()
                                .gap_1()
                                .pb_3()
                                .border_b_1()
                                .border_color(tokens::border)
                                .child(type_title().apply(text(topic.title)))
                                .child(type_body().apply(text(topic.summary).text_color(tokens::muted_foreground))),
                        )
                        .children(topic.sections.iter().map(|sec| {
                            view()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .rounded(px(tokens::radius_lg()))
                                .border_1()
                                .border_color(tokens::border)
                                .bg(tokens::card)
                                .child(
                                    type_subhead()
                                        .semibold()
                                        .apply(text(sec.heading).text_color(tokens::foreground)),
                                )
                                .child(type_body().apply(text(sec.body).text_color(tokens::foreground)))
                                .child(when(
                                    {
                                        let has = !sec.shortcuts.is_empty();
                                        move || has
                                    },
                                    || {
                                        view()
                                            .flex_col()
                                            .gap_1()
                                            .pt_2()
                                            .border_t_1()
                                            .border_color(tokens::border)
                                            .children(sec.shortcuts.iter().map(|(label, stroke)| {
                                                view()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .py(px(2.0))
                                                    .child(
                                                        type_caption()
                                                            .apply(text(*label).text_color(tokens::muted_foreground)),
                                                    )
                                                    .child(kbd_for(stroke))
                                            }))
                                    },
                                ))
                                .into_element()
                        }))
                        .into_element()
                },
            ))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_topics_cover_core_workflows() {
        assert!(TOPICS.len() >= 9);
        assert!(TOPICS.iter().all(|t| !t.title.is_empty() && !t.sections.is_empty()));
    }
}
