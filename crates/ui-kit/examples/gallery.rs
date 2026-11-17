//! The component gallery of DESIGN.md 7.30: every primitive of the kit on one
//! surface, in light and dark.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p scratch-ui-kit --example gallery
//! ```
//!
//! Environment:
//! * `GALLERY_APPEARANCE=system|light|dark` opens in that appearance
//!   (the in-window control switches it at runtime).
//! * `GALLERY_PAGE=0..3` opens on Foundation, Controls, Chrome or Icons.
//!
//! The example reads the app's tokens from `ui/theme.json` (DEISGN.md 3) and
//! serves the crate's SVG icons through `AppOptions::asset_base`: `cargo run`
//! does not register a Rust crate's assets, so the example points the
//! platform at a directory of links to `crates/ui-kit/assets`.

use std::path::Path;

use nsh_base::{SelectItem, Theme};
use nsh_core::element::Tokens;
use nsh_core::element::desktop::MenuItem;
use nsh_core::geometry::Axis;
use nsh_core::platform::Appearance;
use nsh_core::prelude::*;
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;
use scratch_ui_kit::typography::TypeStyle;

/// The 34 language definitions of `crates/language/defs`, in id order.
const LANGUAGES: [&str; 34] = [
    "applescript",
    "c",
    "cpp",
    "csharp-dotnet",
    "csharp-mono",
    "csharp-script",
    "css",
    "go",
    "haskell",
    "html",
    "java",
    "javascript-browser",
    "javascript-jquery",
    "javascript-node",
    "json",
    "kotlin",
    "latex",
    "lua",
    "markdown",
    "objective-c",
    "objective-cpp",
    "perl",
    "php",
    "plain-text",
    "python",
    "ruby",
    "rust",
    "shell",
    "swift",
    "toml",
    "typescript-browser",
    "typescript-node",
    "xml",
    "yaml",
];

/// The gallery's initial page from `GALLERY_PAGE` (0 when unset or wrong).
fn initial_page() -> usize {
    std::env::var("GALLERY_PAGE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|page| *page < 4)
        .unwrap_or(0)
}

/// The page's initial scroll offset from `GALLERY_SCROLL`, in points.
fn initial_scroll() -> f32 {
    std::env::var("GALLERY_SCROLL")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|offset| offset.is_finite() && *offset >= 0.0)
        .unwrap_or(0.0)
}

/// The initial appearance and the segmented control's index for it.
fn initial_appearance() -> (Appearance, usize) {
    match std::env::var("GALLERY_APPEARANCE").as_deref() {
        Ok("light") => (Appearance::Light, 1),
        Ok("dark") => (Appearance::Dark, 2),
        _ => (Appearance::System, 0),
    }
}

/// The app's design tokens from `ui/theme.json` (DEISGN.md 3); the default
/// theme with a loud diagnostic when the file cannot be read or parsed.
fn theme_tokens() -> Tokens {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/theme.json");
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("gallery: {}: {error}", path.display());
            return Theme::default().into_tokens();
        }
    };
    match Theme::from_json_lenient(&json) {
        Ok((theme, ignored)) => {
            for item in ignored {
                eprintln!("gallery: theme value ignored: {item}");
            }
            theme.into_tokens()
        }
        Err(error) => {
            eprintln!("gallery: {}: {error}", path.display());
            Theme::default().into_tokens()
        }
    }
}

/// An asset base that resolves `asset:/@scratch-ui-kit/icons/...` to this
/// crate's `assets/icons/...` (the layout `asset!` publishes).
#[cfg(target_os = "macos")]
fn asset_base() -> nsh_platform_macos::AssetBase {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let staging = std::env::temp_dir().join("scratch-ui-kit-gallery-assets");
    let scoped = staging.join("@scratch-ui-kit");
    let _ = std::fs::remove_file(&scoped);
    if !scoped.exists() {
        let _ = std::fs::create_dir_all(&staging);
        let _ = std::os::unix::fs::symlink(&assets, &scoped);
    }
    nsh_platform_macos::AssetBase::Directory(staging)
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use nsh_core::desktop::{MainWindowOptions, WindowAttributes, Workspaces};
    use nsh_core::prelude::*;
    use nsh_platform_macos as platform;

    let mut attributes = WindowAttributes::default();
    attributes.workspaces = Some(Workspaces::All);
    let mut main_window = MainWindowOptions::default();
    main_window.default_size = Some(size(px(1200.0), px(880.0)));
    main_window.attributes = attributes;

    let (appearance, _) = initial_appearance();
    let root = platform::Root::of::<Gallery>()
        .tokens(theme_tokens())
        .appearance(appearance)
        .app_id("com.tbxark.scratchrunner.ui-kit-gallery")
        .main_window(main_window);
    platform::run_app(
        platform::AppSource::Rust(root),
        platform::AppOptions {
            title: "Scratch UI Kit Gallery".to_owned(),
            app_id: "com.tbxark.scratchrunner.ui-kit-gallery".to_owned(),
            size: (1200, 880),
            asset_base: Some(asset_base()),
            ..Default::default()
        },
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The gallery runs on macOS only.");
}

/// The gallery root: a fixed header (title, appearance, pages) over the
/// chosen page's scroll view.
#[component]
fn Gallery() -> impl IntoElement {
    let (_, appearance_index) = initial_appearance();
    let appearance_index = signal(appearance_index);
    let page = signal(initial_page());

    let name = signal(SharedString::from("ScratchRunner"));
    let search = signal(SharedString::from("TODO"));
    let notes = signal(SharedString::from("Run the active document.\n"));
    let code = signal(Some(SharedString::from(
        "fn main() {\n    println!(\"Hello, Scratch!\");\n}",
    )));
    let url = signal(SharedString::from("example.com"));
    let checked = signal(true);
    let mixed = signal(false);
    let choice = signal(0usize);
    let on = signal(true);
    let number = signal(12.0f64);
    let accent = signal(Some(hsl(210.0, 88.0, 54.0)));
    let encoding = signal(Some(SharedString::from("UTF-8")));
    let segment = signal(1usize);
    let initial_offset = initial_scroll();

    view()
        .size_full()
        .flex_col()
        .bg(tokens::window)
        .text_color(tokens::foreground)
        .child(
            view()
                .w_full()
                .px(px(28.0))
                .py(px(18.0))
                .flex_row()
                .items_end()
                .justify_between()
                .gap(px(24.0))
                .bg(tokens::toolbar)
                .border_b_1()
                .border_color(tokens::toolbar_border)
                .child(
                    view()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            type_display()
                                .apply(text("Scratch UI Kit"))
                                .text_color(tokens::foreground),
                        )
                        .child(type_caption().apply(text(
                            "DESIGN.md sections 2-10 on nsh_base. Tokens from ui/theme.json; both appearances.",
                        )).text_color(tokens::muted_foreground)),
                )
                .child(
                    view()
                        .flex_col()
                        .items_end()
                        .gap(px(8.0))
                        .child(
                            row().child(segmented(
                                "Gallery page",
                                [
                                    Segment::new("Foundation"),
                                    Segment::new("Controls"),
                                    Segment::new("Chrome"),
                                    Segment::new("Icons"),
                                ],
                                move || page.get(),
                                move |index| page.set(index),
                                SegmentedOptions::new(),
                            ))
                            .child(segmented(
                                "Appearance",
                                [
                                    Segment::new("System"),
                                    Segment::new("Light"),
                                    Segment::new("Dark"),
                                ],
                                move || appearance_index.get(),
                                move |index| {
                                    appearance_index.set(index);
                                    set_color_scheme(match index {
                                        1 => Appearance::Light,
                                        2 => Appearance::Dark,
                                        _ => Appearance::System,
                                    });
                                },
                                SegmentedOptions::new().size(ControlSize::Sm),
                            )),
                        ),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .content_offset(point(px(0.0), px(initial_offset)))
                .child(
                view()
                    .w_full()
                    .flex_col()
                    .items_center()
                    .p(px(24.0))
                    .child(
                        view()
                            .w(px(1040.0))
                            .flex_col()
                            .gap(px(24.0))
                            .child(when(move || page.get() == 0, foundation_page))
                            .child(when(move || page.get() == 1, move || controls_page(
                                name,
                                search,
                                notes,
                                code,
                                url,
                                checked,
                                mixed,
                                choice,
                                on,
                                number,
                                accent,
                                segment,
                            )))
                            .child(when(move || page.get() == 2, move || chrome_page(
                                name,
                                number,
                                encoding,
                                on,
                            )))
                            .child(when(move || page.get() == 3, icons_page)),
                    ),
            ),
        )
}

/// Foundation: typography, palette, metrics, motion and marks.
fn foundation_page() -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(24.0))
        .child(section(
            "Typography",
            "The five interface steps and the code font of section 4; captions use muted-foreground.",
            stack()
                .child(type_line(
                    "display · 22 / 28 semibold",
                    "ScratchRunner runs your code",
                    type_display(),
                ))
                .child(type_line(
                    "title · 17 / 24 semibold",
                    "Run Settings",
                    type_title(),
                ))
                .child(type_line(
                    "subhead · 15 / 20 medium",
                    "Editor",
                    type_subhead().medium(),
                ))
                .child(type_line(
                    "body · 13 / 18 regular",
                    "Line numbers, soft wrap and font size apply to every document.",
                    type_body(),
                ))
                .child(type_line(
                    "caption · 12 / 16 regular",
                    "Updated just now",
                    type_caption(),
                ))
                .child(
                    view()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            type_mono()
                                .apply(text("fn main() { println!(\"Hello, Scratch!\"); }"))
                                .text_color(tokens::foreground),
                        )
                        .child(label("mono · 13 / 18 regular, SF Mono, Menlo, monospace")),
                ),
        ))
        .child(section(
            "Color tokens",
            "One accessor per theme token; a missing token reads loud magenta, never a panic.",
            stack()
                .child(
                    row()
                        .child(swatch("window", tokens::window()))
                        .child(swatch("toolbar", tokens::toolbar()))
                        .child(swatch("background", tokens::background()))
                        .child(swatch("field", tokens::field()))
                        .child(swatch("control", tokens::control()))
                        .child(swatch("muted", tokens::muted()))
                        .child(swatch("accent", tokens::accent()))
                        .child(swatch("border", tokens::border())),
                )
                .child(
                    row()
                        .child(swatch("primary", tokens::primary()))
                        .child(swatch("success", tokens::success()))
                        .child(swatch("warning", tokens::warning()))
                        .child(swatch("info", tokens::info()))
                        .child(swatch("destructive", tokens::destructive()))
                        .child(swatch("ring", tokens::ring()))
                        .child(swatch("selection", tokens::selection()))
                        .child(swatch("link", tokens::link())),
                )
                .child(
                    row()
                        .child(swatch("chart-1", tokens::chart_1()))
                        .child(swatch("chart-2", tokens::chart_2()))
                        .child(swatch("chart-3", tokens::chart_3()))
                        .child(swatch("chart-4", tokens::chart_4()))
                        .child(swatch("chart-5", tokens::chart_5()))
                        .child(swatch("diff added", tokens::diff_added()))
                        .child(swatch("diff changed", tokens::diff_changed()))
                        .child(swatch("diff removed", tokens::diff_removed())),
                ),
        ))
        .child(section(
            "Metrics and motion",
            "Numbers are points except motion-*, in milliseconds; the kit reads every one from the theme.",
            stack()
                .child(
                    row()
                        .child(value_line("radius", format!("{} pt", tokens::radius())))
                        .child(value_line("ctl-md", format!("{} pt", tokens::ctl_md())))
                        .child(value_line("row-md", format!("{} pt", tokens::row_md())))
                        .child(value_line("type-body", format!("{} pt", tokens::type_body())))
                        .child(value_line("icon-md", format!("{} pt", tokens::icon_md())))
                        .child(value_line("hairline", format!("{} pt", tokens::hairline()))),
                )
                .child(
                    row()
                        .child(value_line("instant", format!("{} ms", motion::instant())))
                        .child(value_line("fast", format!("{} ms", motion::fast())))
                        .child(value_line("base", format!("{} ms", motion::base())))
                        .child(value_line("slow", format!("{} ms", motion::slow())))
                        .child(value_line("slower", format!("{} ms", motion::slower())))
                        .child(value_line(
                            "press scale",
                            format!("{:.2} / {:.2}", motion::PRESS_SCALE, motion::PRESS_SCALE_ICON),
                        ))
                        .child(value_line(
                            "reduced motion",
                            if motion::reduced() { "on" } else { "off" }.to_owned(),
                        )),
                ),
        ))
        .child(section(
            "Kbd, badges and separators",
            "Section 7.15, 7.16 and 7.26: keycaps, status marks and the 1pt hairline.",
            stack()
                .child(row().child(kbd("⌘")).child(kbd("⇧⌘P")).child(kbd_for("mod+,")).child(kbd("Esc")))
                .child(
                    row()
                        .child(badge("Neutral", BadgeOptions::new()))
                        .child(badge("12", BadgeOptions::new().variant(BadgeVariant::Count)))
                        .child(badge("Primary", BadgeOptions::new().variant(BadgeVariant::Primary)))
                        .child(badge(
                            "Succeeded",
                            BadgeOptions::new()
                                .variant(BadgeVariant::Success)
                                .icon(IconName::Check),
                        ))
                        .child(badge(
                            "Warning",
                            BadgeOptions::new()
                                .variant(BadgeVariant::Warning)
                                .icon(IconName::WarningTriangle),
                        ))
                        .child(badge(
                            "Failed",
                            BadgeOptions::new()
                                .variant(BadgeVariant::Danger)
                                .icon(IconName::ErrorCircle),
                        )),
                )
                .child(
                    row()
                        .child(label("Horizontal"))
                        .child(view().w(px(280.0)).child(separator(
                            SeparatorOrientation::Horizontal,
                        )))
                        .child(label("Vertical"))
                        .child(separator(SeparatorOrientation::Vertical))
                        .child(label("between capsule members")),
                ),
        ))
}

/// Controls: buttons, icon buttons, capsules, segments, fields and toggles.
#[allow(clippy::too_many_arguments)]
fn controls_page(
    name: Signal<SharedString>,
    search: Signal<SharedString>,
    notes: Signal<SharedString>,
    code: Signal<Option<SharedString>>,
    url: Signal<SharedString>,
    checked: Signal<bool>,
    mixed: Signal<bool>,
    choice: Signal<usize>,
    on: Signal<bool>,
    number: Signal<f64>,
    accent: Signal<Option<Hsla>>,
    segment: Signal<usize>,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(24.0))
        .child(section(
            "Buttons",
            "Six variants, four sizes, leading icons, loading (70%) and disabled (45%) states, 0.97 press scale.",
            stack()
                .child(
                    row()
                        .child(button("Primary", ButtonOptions::new().variant(ButtonVariant::Primary), || {}))
                        .child(button("Default", ButtonOptions::new(), || {}))
                        .child(button("Secondary", ButtonOptions::new().variant(ButtonVariant::Secondary), || {}))
                        .child(button("Ghost", ButtonOptions::new().variant(ButtonVariant::Ghost), || {}))
                        .child(button(
                            "Delete",
                            ButtonOptions::new()
                                .variant(ButtonVariant::Destructive)
                                .icon(IconName::Trash),
                            || {},
                        ))
                        .child(button("Documentation", ButtonOptions::new().variant(ButtonVariant::Link), || {})),
                )
                .child(
                    row()
                        .child(button("Run", ButtonOptions::new().variant(ButtonVariant::Primary).icon(IconName::Run), || {}))
                        .child(button("Loading", ButtonOptions::new().loading(true), || {}))
                        .child(button("Disabled", ButtonOptions::new().disabled(true), || {}))
                        .child(button("Disabled primary", ButtonOptions::new().variant(ButtonVariant::Primary).disabled(true), || {})),
                )
                .child(
                    row()
                        .child(cell("xs", button("Xs", ButtonOptions::new().size(ControlSize::Xs), || {})))
                        .child(cell("sm", button("Sm", ButtonOptions::new().size(ControlSize::Sm), || {})))
                        .child(cell("md", button("Md", ButtonOptions::new(), || {})))
                        .child(cell("lg", button("Lg", ButtonOptions::new().size(ControlSize::Lg), || {}))),
                ),
        ))
        .child(section(
            "Icon buttons and capsules",
            "Square targets with tooltips and accessible labels; capsule groups share one silhouette and 1pt dividers.",
            stack()
                .child(
                    row()
                        .child(icon_button(IconName::Run, "Run", IconButtonOptions::new(), || {}))
                        .child(icon_button(
                            IconName::Bookmark,
                            "Bookmark",
                            IconButtonOptions::new().selected(true),
                            || {},
                        ))
                        .child(icon_button(
                            IconName::Gear,
                            "Settings",
                            IconButtonOptions::new()
                                .variant(IconButtonVariant::Filled)
                                .shortcut("⌘,"),
                            || {},
                        ))
                        .child(icon_button(IconName::Trash, "Delete", IconButtonOptions::new().disabled(true), || {}))
                        .child(icon_button(
                            IconName::SidebarLeft,
                            "Toggle sidebar",
                            IconButtonOptions::new().size(ControlSize::Sm),
                            || {},
                        )),
                )
                .child(
                    row()
                        .child(capsule_group(
                            "Run controls",
                            CapsuleGroupOptions::new(),
                            vec![
                                icon_button(
                                    IconName::Run,
                                    "Run",
                                    IconButtonOptions::new().variant(IconButtonVariant::Capsule),
                                    || {},
                                )
                                .into_element(),
                                icon_button(
                                    IconName::Pause,
                                    "Pause",
                                    IconButtonOptions::new().variant(IconButtonVariant::Capsule),
                                    || {},
                                )
                                .into_element(),
                                icon_button(
                                    IconName::Stop,
                                    "Stop",
                                    IconButtonOptions::new().variant(IconButtonVariant::Capsule),
                                    || {},
                                )
                                .into_element(),
                                icon_button(
                                    IconName::Eraser,
                                    "Clear console",
                                    IconButtonOptions::new().variant(IconButtonVariant::Capsule),
                                    || {},
                                )
                                .into_element(),
                            ],
                        ))
                        .child(icon_button(IconName::Plus, "New document", IconButtonOptions::new(), || {})),
                )
                .child(
                    row()
                        .child(segmented(
                            "Editor pane",
                            [
                                Segment::new("Editor").icon(IconName::FileCode),
                                Segment::new("Split").icon(IconName::SidebarRight),
                                Segment::new("Preview").icon(IconName::Eye),
                            ],
                            move || segment.get(),
                            move |index| segment.set(index),
                            SegmentedOptions::new(),
                        ))
                        .child(segmented(
                            "Density",
                            [Segment::new("Compact"), Segment::new("Comfortable")],
                            || 0,
                            |_| {},
                            SegmentedOptions::new().size(ControlSize::Sm).equal(true),
                        ))
                        .child(segmented(
                            "Disabled",
                            [Segment::new("A"), Segment::new("B")],
                            || 1,
                            |_| {},
                            SegmentedOptions::new().disabled(true),
                        )),
                ),
        ))
        .child(section(
            "Search and text fields",
            "Capsule search with leading glyph, count and clear button; text areas and the code editor wrap nsh_base.",
            stack()
                .child(
                    view()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            row()
                                .child(
                                    view().w(px(300.0)).child(search_field(
                                        "Search documents",
                                        search,
                                        Some(SharedString::from("2 matches")),
                                        move |_| {},
                                        move || search.set(SharedString::default()),
                                        SearchFieldOptions::new(),
                                    )),
                                )
                                .child(
                                    view().w(px(260.0)).child(search_field(
                                        "Filter console",
                                        SharedString::from("error"),
                                        Option::<SharedString>::None,
                                        move |_| {},
                                        || {},
                                        SearchFieldOptions::new()
                                            .leading(IconName::Filter)
                                            .invalid(true),
                                    )),
                                )
                                .child(
                                    view().w(px(240.0)).child(search_field(
                                        "Disabled",
                                        SharedString::default(),
                                        Option::<SharedString>::None,
                                        move |_| {},
                                        || {},
                                        SearchFieldOptions::new().disabled(true),
                                    )),
                                ),
                        ),
                )
                .child(
                    row()
                        .child(
                            view().w(px(280.0)).child(text_field(
                                "Document name",
                                name,
                                move |text| name.set(SharedString::from(text.to_owned())),
                                TextFieldOptions::new(),
                                None,
                            )),
                        )
                        .child(
                            view().w(px(280.0)).child(text_field(
                                "https://example.com",
                                url,
                                move |text| url.set(SharedString::from(text.to_owned())),
                                TextFieldOptions::new().invalid(true),
                                Some(SharedString::from("Enter a URL with a scheme.")),
                            )),
                        )
                        .child(
                            view().w(px(240.0)).child(text_field(
                                "Read-only path",
                                "/usr/local/bin",
                                |_| {},
                                TextFieldOptions::new().readonly(true).mono(true),
                                None,
                            )),
                        )
                        .child(
                            view().w(px(240.0)).child(text_field(
                                "Disabled",
                                "Not editable",
                                |_| {},
                                TextFieldOptions::new().disabled(true),
                                None,
                            )),
                        ),
                )
                .child(
                    view().w(px(600.0)).child(text_area(
                        "Notes",
                        notes,
                        move |text| notes.set(SharedString::from(text.to_owned())),
                        TextFieldOptions::new(),
                        None,
                    )),
                )
                .child(
                    view().w(px(600.0)).child(code_field(
                        code,
                        move |text| code.set(Some(text.clone())),
                        CodeFieldOptions::new("rust")
                            .min_rows(3)
                            .max_rows(8)
                            .accessibility_label("Run command")
                            .placeholder("The command to run"),
                    )),
                ),
        ))
        .child(section(
            "Checkboxes, radios and switches",
            "16pt indicators, a 36x20 switch track and a 16pt thumb: sections 7.7 and 7.8.",
            stack()
                .child(
                    row()
                        .child(checkbox("Show line numbers", checked, CheckboxOptions::new(), move |value| checked.set(value)))
                        .child(checkbox("Soft wrap", false, CheckboxOptions::new(), |_| {}))
                        .child(checkbox("Mixed", mixed, CheckboxOptions::new().indeterminate(true), move |value| mixed.set(value)))
                        .child(checkbox("Disabled", true, CheckboxOptions::new().disabled(true), |_| {})),
                )
                .child(
                    radio_group(
                        "Case sensitivity",
                        [
                            radio("Sensitive", move || choice.get() == 0, false, move || choice.set(0)).into_element(),
                            radio("Insensitive", move || choice.get() == 1, false, move || choice.set(1)).into_element(),
                            radio("Disabled", false, true, || {}).into_element(),
                        ],
                    ),
                )
                .child(
                    row()
                        .child(switch("Auto-save on run", on, false, move |value| on.set(value)))
                        .child(switch("Disabled", true, true, |_| {})),
                ),
        ))
        .child(section(
            "Numbers and colors",
            "Number fields with a 16pt stepper; color wells open the system picker.",
            stack()
                .child(
                    row()
                        .child(cell("number_field", number_field(
                            "Font size",
                            number,
                            NumberFieldOptions::new().range(8.0, 72.0).suffix("pt"),
                            move |value| number.set(value),
                        )))
                        .child(cell("stepper", stepper(
                            "Tab width",
                            number,
                            NumberFieldOptions::new().range(1.0, 16.0).width(64.0),
                            move |value| number.set(value),
                        )))
                        .child(cell("disabled", number_field(
                            "Increment",
                            number,
                            NumberFieldOptions::new().disabled(true),
                            |_| {},
                        ))),
                )
                .child(
                    row()
                        .child(color_well(
                            "Accent",
                            accent,
                            move |value| accent.set(Some(value)),
                            ColorWellOptions::new(),
                        ))
                        .child(
                            view()
                                .flex_col()
                                .items_center()
                                .gap(px(6.0))
                                .child(color_well(
                                    "Rule",
                                    accent,
                                    |_| {},
                                    ColorWellOptions::new().size(ColorWellSize::Chip),
                                ))
                                .child(label("chip")),
                        )
                        .child(color_well(
                            "Background",
                            Option::<Hsla>::None,
                            |_| {},
                            ColorWellOptions::new(),
                        )),
                ),
        ))
}

/// Chrome: popups, headers, form rows, feedback, tooltip and split handle.
fn chrome_page(
    name: Signal<SharedString>,
    number: Signal<f64>,
    encoding: Signal<Option<SharedString>>,
    on: Signal<bool>,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(24.0))
        .child(section(
            "Popup buttons and selects",
            "Section 7.4: the system menu anchored under its trigger, or a searchable select for long lists.",
            stack()
                .child(
                    row()
                        .child(popup_button(
                            "Language",
                            "Rust",
                            language_menu(),
                            PopupButtonOptions::new(),
                            |_| {},
                        ))
                        .child(popup_button(
                            "Language",
                            "Rust",
                            language_menu(),
                            PopupButtonOptions::new().variant(PopupButtonVariant::Capsule),
                            |_| {},
                        ))
                        .child(popup_button(
                            "Encoding",
                            "UTF-8",
                            language_menu(),
                            PopupButtonOptions::new().variant(PopupButtonVariant::Bar),
                            |_| {},
                        ))
                        .child(popup_button(
                            "Disabled",
                            "Rust",
                            language_menu(),
                            PopupButtonOptions::new().disabled(true),
                            |_| {},
                        )),
                )
                .child(
                    row()
                        .child(select_popup(
                            "Encoding",
                            encoding,
                            "Choose an encoding",
                            vec![
                                SelectItem::new("utf-8", "UTF-8"),
                                SelectItem::new("utf-16", "UTF-16"),
                                SelectItem::new("ascii", "ASCII"),
                                SelectItem::new("latin-1", "Latin-1"),
                            ],
                            SelectPopupOptions::new().searchable(true),
                            move |value| encoding.set(value),
                        ))
                        .child(select_popup(
                            "Popup style",
                            Some(SharedString::from("field")),
                            "Choose a style",
                            vec![
                                SelectItem::new("field", "Field"),
                                SelectItem::new("capsule", "Capsule"),
                                SelectItem::new("bar", "Bar"),
                            ],
                            SelectPopupOptions::new(),
                            |_| {},
                        )),
                ),
        ))
        .child(section(
            "Section headers",
            "Three variants: form (a hairline), list (a 24pt band with actions) and pane.",
            stack()
                .child(section_header(
                    "Form header",
                    SectionHeaderVariant::Form,
                    SectionHeaderOptions::new(),
                ))
                .child(section_header_with_actions(
                    "List header",
                    SectionHeaderVariant::List,
                    [
                        icon_button(IconName::Plus, "Add", IconButtonOptions::new().size(ControlSize::Xs), || {})
                            .into_element(),
                        icon_button(
                            IconName::Gear,
                            "Settings",
                            IconButtonOptions::new().size(ControlSize::Xs),
                            || {},
                        )
                        .into_element(),
                    ],
                    SectionHeaderOptions::new(),
                ))
                .child(section_header(
                    "Pane header",
                    SectionHeaderVariant::Pane,
                    SectionHeaderOptions::new(),
                )),
        ))
        .child(section(
            "Form rows",
            "A 200pt right-aligned label column (120 in the languages pane), a flexible control, optional caption and help.",
            view()
                .flex_col()
                .gap(px(12.0))
                .child(form_row(
                    "Document name",
                    text_field(
                        "Untitled",
                        name,
                        move |text| name.set(SharedString::from(text.to_owned())),
                        TextFieldOptions::new(),
                        None,
                    ),
                    FormRowOptions::new(),
                ))
                .child(form_row_with_caption(
                    "Font size",
                    number_field(
                        "Font size",
                        number,
                        NumberFieldOptions::new().range(8.0, 72.0).suffix("pt"),
                        move |value| number.set(value),
                    ),
                    "Points; applies to every document.",
                    FormRowOptions::new(),
                ))
                .child(form_row_with_help(
                    "Line height",
                    number_field(
                        "Line height",
                        number,
                        NumberFieldOptions::new().range(1.0, 2.0).step(0.05),
                        move |value| number.set(value),
                    ),
                    || {},
                    FormRowOptions::new(),
                ))
                .child(form_row(
                    "",
                    switch("Show invisible characters", on, false, move |value| on.set(value)),
                    FormRowOptions::new(),
                )),
        ))
        .child(section(
            "Feedback and split handles",
            "A spinner for progress cues, a 2/4pt progress bar, tooltips after 500 ms, and the 9pt resize handle.",
            stack()
                .child(
                    row()
                        .child(cell("xs", spinner(IconSize::Xs, Tone::Muted, "Loading")))
                        .child(cell("sm", spinner(IconSize::Sm, Tone::Default, "Loading")))
                        .child(cell("md", spinner(IconSize::Md, Tone::Primary, "Loading")))
                        .child(cell("lg", spinner(IconSize::Lg, Tone::Muted, "Loading"))),
                )
                .child(
                    row()
                        .child(view().w(px(320.0)).child(progress_bar(35.0f32, ProgressOptions::new())))
                        .child(view().w(px(320.0)).child(progress_bar(
                            0.0f32,
                            ProgressOptions::new().indeterminate(true).label("Running"),
                        )))
                        .child(view().w(px(320.0)).child(progress_bar(
                            70.0f32,
                            ProgressOptions::new().height(4.0),
                        ))),
                )
                .child(
                    row()
                        .child(
                            view()
                                .child(button("Hover for a tooltip", ButtonOptions::new(), || {}))
                                .tooltip(tooltip("A hint after half a second", Some("⌘K"))),
                        )
                        .child(label("Tooltip: popover fill, radius-sm, E2, caption")),
                )
                .child(
                    view()
                        .relative()
                        .w(px(480.0))
                        .h(px(72.0))
                        .rounded(px(tokens::radius()))
                        .bg(tokens::background)
                        .border_1()
                        .border_color(tokens::border)
                        .child(split_handle(Axis::Horizontal, "Resize gallery pane", false))
                        .child(
                            view()
                                .absolute()
                                .left(px(0.0))
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .w(px(220.0))
                                .p(px(10.0))
                                .child(label("Drag the right edge")),
                        ),
                ),
        ))
}

/// Icons: the 85 SVG files of section 6.3 and the language tiles of 6.2.
fn icons_page() -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(24.0))
        .child(section(
            "Icons",
            "85 SVGs, 16x16, currentColor; glyph() takes a typed IconName and a tone.",
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(10.0))
                .children(IconName::ALL.iter().map(|icon| icon_cell(*icon).into_element())),
        ))
        .child(section(
            "Language tiles",
            "The 34 language definitions: a 1-3 character monogram on a stable 16% tint (glyph-1..10).",
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .children(LANGUAGES.iter().map(|id| language_cell(id).into_element())),
        ))
}

/// One gallery section: a titled, outlined card on the page.
fn section(
    title: &'static str,
    subtitle: &'static str,
    body: impl IntoElement,
) -> View {
    view()
        .w_full()
        .flex_col()
        .gap(px(14.0))
        .p(px(20.0))
        .rounded(px(tokens::radius_lg()))
        .bg(tokens::background)
        .border_1()
        .border_color(tokens::border)
        .child(section_header(
            title,
            SectionHeaderVariant::Pane,
            SectionHeaderOptions::new(),
        ))
        .child(label(subtitle))
        .child(body)
}

/// A wrapping row of controls.
fn row() -> View {
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(12.0))
}

/// A vertical stack that keeps a section's rows apart.
fn stack() -> View {
    view().flex_col().gap(px(12.0))
}

/// Muted caption text.
fn label(text_value: &'static str) -> impl IntoElement {
    type_caption()
        .apply(text(text_value))
        .text_color(tokens::muted_foreground)
}

/// A control above its caption, for size and state comparisons.
fn cell(caption: &'static str, element: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .child(element)
        .child(label(caption))
}

/// A type sample over the step it demonstrates.
fn type_line(caption: &'static str, sample: &'static str, style: TypeStyle) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(2.0))
        .child(
            style
                .apply(text(sample))
                .text_color(tokens::foreground),
        )
        .child(label(caption))
}

/// A color swatch with its token name.
fn swatch(caption: &'static str, color: AdaptiveColor) -> impl IntoElement {
    view()
        .w(px(84.0))
        .flex_col()
        .gap(px(4.0))
        .child(
            view()
                .w_full()
                .h(px(32.0))
                .rounded(px(tokens::radius_sm()))
                .bg(move || color)
                .border_1()
                .border_color(tokens::border),
        )
        .child(label(caption))
}

/// A token name on the left and its value on the right.
fn value_line(name: &'static str, value: String) -> impl IntoElement {
    view()
        .w(px(156.0))
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(label(name))
        .child(
            type_mono()
                .apply(text(SharedString::from(value)))
                .text_color(tokens::foreground),
        )
}

/// One icon over its variant name, split into words ("WebInspectorWindow" ->
/// "Web Inspector Window") so a long name wraps between words.
fn icon_cell(icon: IconName) -> impl IntoElement {
    let mut name = String::new();
    let mut previous_lower = false;
    for character in format!("{icon:?}").chars() {
        if previous_lower && character.is_uppercase() {
            name.push(' ');
        }
        name.push(character);
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
    }
    // Two names are wider than a 108pt cell and would wrap the caption row.
    let name = match icon {
        IconName::ChevronsUpDown => "Chevrons U/D".to_owned(),
        IconName::WebInspectorWindow => "Web Inspector".to_owned(),
        _ => name,
    };
    view()
        .w(px(108.0))
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .py(px(6.0))
        .child(glyph(icon, IconSize::Md, Tone::Default))
        .child(
            type_caption()
                .apply(text(SharedString::from(name)))
                .text_color(tokens::muted_foreground),
        )
}

/// One language tile and its id.
fn language_cell(id: &'static str) -> impl IntoElement {
    view()
        .w(px(150.0))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(language_glyph_tile(id))
        .child(label(id))
}

/// A native menu for the popup demonstrations.
fn language_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Rust"),
        MenuItem::new("Swift"),
        MenuItem::new("Python"),
        MenuItem::new("JavaScript"),
        MenuItem::separator(),
        MenuItem::new("Other…"),
    ]
}
