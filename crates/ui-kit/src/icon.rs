//! Icons and glyph tiles, DESIGN.md section 6.
//!
//! Every icon is one SVG of `assets/icons/` (copied here from the app's
//! `assets/icons/`), drawn by NSH's `svg()` element in `currentColor`, so a
//! glyph follows the text color of its control and its hover, selected and
//! disabled states. [`IconName`] has one variant per file: a file without a
//! variant (or a variant without a file) fails the build through `asset!`.
//!
//! Language tiles need no files: [`GlyphTile`] is the 16x16 rounded square
//! with a one- or two-letter monogram and a stable tint per language
//! ([`language_tile`]).
//!
//! The crate's assets must be registered with the app's engine
//! (`registry.assets(scratch_ui_kit::ASSETS)`), and an app run from `cargo
//! run` needs `AppOptions::asset_base` pointing at a directory that has
//! `@scratch-ui-kit/icons/...` (see the README).

use nsh_core::{EllipsizeMode, element::ImageAsset, prelude::*};

use crate::tokens;

macro_rules! icon_names {
    ($(
        $(#[doc = $doc:literal])*
        $variant:ident => $path:literal
    ),* $(,)?) => {
        /// Every icon of DESIGN.md 6.3: one variant per SVG file.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $(
                $(#[doc = $doc])*
                #[doc = concat!("File `", $path, "`.")]
                $variant,
            )*
        }

        impl IconName {
            /// Every icon, in the order of DESIGN.md 6.3.
            pub const ALL: &'static [IconName] = &[$(Self::$variant),*];

            /// The file's path under the crate's `assets/` directory.
            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => $path,)*
                }
            }

            /// The file as an image asset, which the engine serves.
            pub fn asset(self) -> ImageAsset {
                match self {
                    $(Self::$variant => asset!($path),)*
                }
            }
        }
    };
}

icon_names! {
    /// The application mark.
    AppMark => "icons/app-mark.svg",
    /// A downward arrow.
    ArrowDown => "icons/arrow-down.svg",
    /// An upward arrow.
    ArrowUp => "icons/arrow-up.svg",
    /// An arrow leaving to the upper right (open externally).
    ArrowUpRight => "icons/arrow-up-right.svg",
    /// An open book (documentation).
    BookOpen => "icons/book-open.svg",
    /// A bookmark.
    Bookmark => "icons/bookmark.svg",
    /// Braces (variables, languages).
    Braces => "icons/braces.svg",
    /// A breakpoint tag.
    Breakpoint => "icons/breakpoint.svg",
    /// A bug.
    Bug => "icons/bug.svg",
    /// A check mark.
    Check => "icons/check.svg",
    /// A downward chevron.
    ChevronDown => "icons/chevron-down.svg",
    /// A leftward chevron.
    ChevronLeft => "icons/chevron-left.svg",
    /// A rightward chevron.
    ChevronRight => "icons/chevron-right.svg",
    /// An upward chevron.
    ChevronUp => "icons/chevron-up.svg",
    /// Two leftward chevrons.
    ChevronsLeft => "icons/chevrons-left.svg",
    /// Two rightward chevrons.
    ChevronsRight => "icons/chevrons-right.svg",
    /// Up and down chevrons (a popup indicator).
    ChevronsUpDown => "icons/chevrons-up-down.svg",
    /// A clipboard.
    Clipboard => "icons/clipboard.svg",
    /// A close cross.
    Close => "icons/close.svg",
    /// A filled circle with a cross (clear).
    CloseCircle => "icons/close-circle.svg",
    /// The command key.
    Command => "icons/command.svg",
    /// A console docked to the bottom.
    ConsoleBottom => "icons/console-bottom.svg",
    /// A console docked to the right.
    ConsoleRight => "icons/console-right.svg",
    /// Two overlapping pages (copy).
    Copy => "icons/copy.svg",
    /// The debugger's execution arrow.
    DebugArrow => "icons/debug-arrow.svg",
    /// Continue execution.
    DebugContinue => "icons/debug-continue.svg",
    /// A stack of frames.
    DebugStack => "icons/debug-stack.svg",
    /// Step into.
    DebugStepIn => "icons/debug-step-in.svg",
    /// Step out.
    DebugStepOut => "icons/debug-step-out.svg",
    /// Step over.
    DebugStepOver => "icons/debug-step-over.svg",
    /// A filled dot.
    Dot => "icons/dot.svg",
    /// A drag handle.
    DragHandle => "icons/drag-handle.svg",
    /// A horizontal ellipsis.
    Ellipsis => "icons/ellipsis.svg",
    /// An ellipsis in a circle.
    EllipsisCircle => "icons/ellipsis-circle.svg",
    /// An eraser.
    Eraser => "icons/eraser.svg",
    /// A filled circle with an exclamation mark (error).
    ErrorCircle => "icons/error-circle.svg",
    /// An open eye.
    Eye => "icons/eye.svg",
    /// A crossed-out eye.
    EyeOff => "icons/eye-off.svg",
    /// A plain file.
    File => "icons/file.svg",
    /// A binary file.
    FileBinary => "icons/file-binary.svg",
    /// A source file.
    FileCode => "icons/file-code.svg",
    /// An image file.
    FileImage => "icons/file-image.svg",
    /// A file with a plus (new file).
    FilePlus => "icons/file-plus.svg",
    /// A text file.
    FileText => "icons/file-text.svg",
    /// A funnel (filter).
    Filter => "icons/filter.svg",
    /// A folder.
    Folder => "icons/folder.svg",
    /// An open folder.
    FolderOpen => "icons/folder-open.svg",
    /// A folder with a plus (new folder).
    FolderPlus => "icons/folder-plus.svg",
    /// A gear (settings).
    Gear => "icons/gear.svg",
    /// A globe.
    Globe => "icons/globe.svg",
    /// A folder with an arrow (go to folder).
    GoToFolder => "icons/go-to-folder.svg",
    /// A circle with a question mark (help).
    HelpCircle => "icons/help-circle.svg",
    /// A circle with an i (information).
    InfoCircle => "icons/info-circle.svg",
    /// A keyboard.
    Keyboard => "icons/keyboard.svg",
    /// A chain link.
    Link => "icons/link.svg",
    /// A padlock.
    Lock => "icons/lock.svg",
    /// A minus.
    Minus => "icons/minus.svg",
    /// A moon (dark appearance).
    Moon => "icons/moon.svg",
    /// A pause mark.
    Pause => "icons/pause.svg",
    /// A pause mark in a circle.
    PauseCircle => "icons/pause-circle.svg",
    /// A pencil.
    Pencil => "icons/pencil.svg",
    /// A plus.
    Plus => "icons/plus.svg",
    /// The General preferences pane.
    PrefGeneral => "icons/pref-general.svg",
    /// The Languages preferences pane.
    PrefLanguages => "icons/pref-languages.svg",
    /// The Text Editing preferences pane.
    PrefText => "icons/pref-text.svg",
    /// The Themes preferences pane.
    PrefThemes => "icons/pref-themes.svg",
    /// A record circle.
    Record => "icons/record.svg",
    /// A circular refresh arrow.
    Refresh => "icons/refresh.svg",
    /// A filled play triangle (run).
    Run => "icons/run.svg",
    /// A magnifying glass (search).
    Search => "icons/search.svg",
    /// A sidebar on the left.
    SidebarLeft => "icons/sidebar-left.svg",
    /// A sidebar on the right.
    SidebarRight => "icons/sidebar-right.svg",
    /// A spinner arc.
    Spinner => "icons/spinner.svg",
    /// A filled stop square.
    Stop => "icons/stop.svg",
    /// A stop mark in a circle.
    StopCircle => "icons/stop-circle.svg",
    /// A filled circle with a check (success).
    SuccessCircle => "icons/success-circle.svg",
    /// A sun (light appearance).
    Sun => "icons/sun.svg",
    /// A terminal.
    Terminal => "icons/terminal.svg",
    /// A trash can.
    Trash => "icons/trash.svg",
    /// A warning triangle.
    WarningTriangle => "icons/warning-triangle.svg",
    /// The Web Inspector in its own window.
    WebInspectorWindow => "icons/web-inspector-window.svg",
    /// A crossed-out Wi-Fi mark.
    WifiOff => "icons/wifi-off.svg",
    /// A wrench (run settings).
    Wrench => "icons/wrench.svg",
    /// A magnifying glass with a plus (zoom in).
    ZoomIn => "icons/zoom-in.svg",
    /// A magnifying glass with a minus (zoom out).
    ZoomOut => "icons/zoom-out.svg",
}

/// A glyph's color role (DESIGN.md 6.1): state is a tone, never a file
/// swap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    /// `foreground`.
    #[default]
    Default,
    /// `muted-foreground`.
    Muted,
    /// `primary`.
    Primary,
    /// `success`.
    Success,
    /// `warning`.
    Warning,
    /// `destructive`.
    Danger,
    /// `info`.
    Info,
    /// `primary-foreground`.
    OnPrimary,
    /// `selection-foreground`.
    OnSelection,
    /// `foreground` at 45% opacity.
    Disabled,
}

impl Tone {
    /// The icon color for this role.
    pub fn color(self) -> AdaptiveColor {
        match self {
            Tone::Default => tokens::foreground(),
            Tone::Muted => tokens::muted_foreground(),
            Tone::Primary => tokens::primary(),
            Tone::Success => tokens::success(),
            Tone::Warning => tokens::warning(),
            Tone::Danger => tokens::destructive(),
            Tone::Info => tokens::info(),
            Tone::OnPrimary => tokens::primary_foreground(),
            Tone::OnSelection => tokens::selection_foreground(),
            Tone::Disabled => tokens::foreground().opacity(crate::motion::OPACITY_DISABLED),
        }
    }
}

/// The icon sizes of DESIGN.md's scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconSize {
    /// 12: an inline glyph, disclosure or badge icon.
    Xs,
    /// 14: a small button, tree row or tab icon.
    Sm,
    /// 16: the toolbar and default button (the 1:1 grid).
    #[default]
    Md,
    /// 20: a prompt or alert glyph.
    Lg,
    /// 24: the preferences strip.
    Xl,
    /// 32: an empty state.
    Xxl,
}

impl IconSize {
    /// The size in points, read from the theme token.
    pub fn points(self) -> f32 {
        match self {
            IconSize::Xs => tokens::icon_xs(),
            IconSize::Sm => tokens::icon_sm(),
            IconSize::Md => tokens::icon_md(),
            IconSize::Lg => tokens::icon_lg(),
            IconSize::Xl => tokens::icon_xl(),
            IconSize::Xxl => tokens::icon_2xl(),
        }
    }
}

/// An icon drawn at `size` in `tone`, a square of `currentColor`.
pub fn glyph(name: IconName, size: IconSize, tone: Tone) -> Svg {
    glyph_at(name, size.points(), tone)
}

/// An icon drawn at `points` (12, 14, 16, 20, 24 or 32 is the designed
/// scale) in `tone`.
pub fn glyph_at(name: IconName, points: f32, tone: Tone) -> Svg {
    svg(name.asset())
        .size(px(points))
        .text_color(move || tone.color())
}

/// The ten tile colors of DESIGN.md 6.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlyphTone {
    /// `glyph-1` (blue).
    One,
    /// `glyph-2` (teal).
    Two,
    /// `glyph-3` (green).
    Three,
    /// `glyph-4` (orange).
    Four,
    /// `glyph-5` (red).
    Five,
    /// `glyph-6` (pink).
    Six,
    /// `glyph-7` (purple).
    Seven,
    /// `glyph-8` (indigo).
    Eight,
    /// `glyph-9` (yellow).
    Nine,
    /// `glyph-10` (gray).
    Ten,
}

impl GlyphTone {
    /// The tone for a stable hash of `id`: `hash(id) mod 10 + 1`, so a
    /// language always has the same tile color.
    pub fn for_id(id: &str) -> Self {
        match fnv1a(id) % 10 {
            0 => GlyphTone::One,
            1 => GlyphTone::Two,
            2 => GlyphTone::Three,
            3 => GlyphTone::Four,
            4 => GlyphTone::Five,
            5 => GlyphTone::Six,
            6 => GlyphTone::Seven,
            7 => GlyphTone::Eight,
            8 => GlyphTone::Nine,
            _ => GlyphTone::Ten,
        }
    }

    /// The tile color.
    pub fn color(self) -> AdaptiveColor {
        match self {
            GlyphTone::One => tokens::glyph_1(),
            GlyphTone::Two => tokens::glyph_2(),
            GlyphTone::Three => tokens::glyph_3(),
            GlyphTone::Four => tokens::glyph_4(),
            GlyphTone::Five => tokens::glyph_5(),
            GlyphTone::Six => tokens::glyph_6(),
            GlyphTone::Seven => tokens::glyph_7(),
            GlyphTone::Eight => tokens::glyph_8(),
            GlyphTone::Nine => tokens::glyph_9(),
            GlyphTone::Ten => tokens::glyph_10(),
        }
    }
}

/// The part of the caption step a two-letter monogram takes in a 16pt tile
/// (`As`, `C#`).
const TWO_CHAR_SCALE: f32 = 0.875;

/// The part of the caption step a three-letter monogram takes in a 16pt
/// tile (`C++`).
const THREE_CHAR_SCALE: f32 = 0.66;

/// FNV-1a, 32 bit: stable across builds and platforms.
fn fnv1a(text: &str) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The monogram and tint of a language's [`GlyphTile`]: the abbreviations
/// of the 34 language definitions in `crates/language/defs`, and a stable
/// tone from the id. An unknown language is `?` in the gray tile.
pub fn language_tile(language_id: &str) -> (&'static str, GlyphTone) {
    let text = match language_id {
        "applescript" => "As",
        "c" => "C",
        "cpp" => "C++",
        "csharp-dotnet" | "csharp-mono" | "csharp-script" => "C#",
        "css" => "Cs",
        "go" => "Go",
        "haskell" => "Hs",
        "html" => "Ht",
        "java" => "Jv",
        "javascript-browser" | "javascript-jquery" | "javascript-node" => "JS",
        "json" => "{}",
        "kotlin" => "Kt",
        "latex" => "Tx",
        "lua" => "Lu",
        "markdown" => "Md",
        "objective-c" => "OC",
        "objective-cpp" => "O+",
        "perl" => "Pl",
        "php" => "Ph",
        "plain-text" => "Tx",
        "python" => "Py",
        "ruby" => "Rb",
        "rust" => "Rs",
        "shell" => "Sh",
        "swift" => "Sw",
        "toml" => "Tm",
        "typescript-browser" | "typescript-node" => "TS",
        "xml" => "Xm",
        "yaml" => "Ym",
        _ => "?",
    };
    let tone = match language_id {
        "" => GlyphTone::Ten,
        _ => GlyphTone::for_id(language_id),
    };
    (text, tone)
}

/// The 16x16 rounded tile of DESIGN.md 6.2: `text` (1 to 3 characters) in
/// the tile color on the same color at 16% opacity.
pub fn glyph_tile(label: impl IntoReactive<SharedString> + 'static, tone: GlyphTone) -> impl IntoElement {
    let label = label.into_reactive();
    let shown = label.clone();
    let sized = label.clone();
    view()
        .size(px(16.0))
        .items_center()
        .justify_center()
        .rounded(px(tokens::radius_xs()))
        .bg(move || tone.color().opacity(0.16))
        .child(
            text(move || shown.get())
                // The tile's full width, so a scaled monogram is measured
                // against 16pt rather than its unscaled intrinsic width.
                .w(px(16.0))
                .text_center()
                .text_size(move || {
                    // A 16pt tile fits one caption-sized letter; two and
                    // three (C++, C#) scale down from the same caption step.
                    px(match sized.get().chars().count() {
                        0..=1 => tokens::type_caption(),
                        2 => tokens::type_caption() * TWO_CHAR_SCALE,
                        _ => tokens::type_caption() * THREE_CHAR_SCALE,
                    })
                })
                .line_height(px(tokens::leading_caption()))
                .number_of_lines(1u32)
                // A tight monogram must not become an ellipsis in the tile.
                .text_overflow(EllipsizeMode::Clip)
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || tone.color()),
        )
}

/// The language's [`GlyphTile`], from [`language_tile`].
pub fn language_glyph_tile(language_id: &str) -> impl IntoElement {
    let (text, tone) = language_tile(language_id);
    glyph_tile(text, tone)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_design_icon_has_a_variant() {
        assert_eq!(IconName::ALL.len(), 85);
        for icon in IconName::ALL {
            assert!(icon.path().starts_with("icons/"));
            assert!(icon.path().ends_with(".svg"));
        }
    }

    #[test]
    fn every_language_has_a_monogram() {
        for id in LANGUAGES {
            let (text, _) = language_tile(id);
            assert!(!text.is_empty(), "{id}");
            assert!((1..=3).contains(&text.chars().count()), "{id}: {text}");
        }
    }

    #[test]
    fn a_language_tint_is_stable() {
        for id in LANGUAGES {
            assert_eq!(language_tile(id), language_tile(id));
        }
        assert_eq!(language_tile("unknown-language").0, "?");
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a("python"), 0x0fce_ff97);
    }
}
