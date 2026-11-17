//! The Preferences window (Cmd-,): General, Text Editing, Shell, Languages,
//! Keyboard and Themes, read from and written to `settings.toml`.
//!
//! [`settings`] is the live source of the preferences. Views read its fields
//! (named as in [`scratch_config::Preferences`]) inside bindings, so a change
//! applies to every open editor at once; [`save`] writes the file. The window
//! edits one field at a time and saves after each change. The Languages pane
//! edits the language registry instead, which keeps its own files under
//! `Languages/`.
//!
//! The window is a native dialog window (`window()`, id `prefs`): a toolbar of
//! icon-over-label pane buttons across the top — as CodeRunner's `NSToolbar`
//! and macOS Settings do — the chosen pane below it, and the window title
//! `Preferences – <Pane>`. Form panes are a centered fixed-width column of
//! right-aligned labels and aligned controls separated by hairlines;
//! Languages, Keyboard and Themes are master/detail splits. Controls come
//! from `scratch-ui-kit`; the selection lists (pane toolbar, languages,
//! themes) are `nsh_base` tabs drawn with the kit's tokens and motion, as the
//! kit has no list row yet.
//!
//! Motion inventory (DESIGN.md 9.3; with reduced motion every line is instant):
//!
//! | Change | Motion |
//! |---|---|
//! | Window opens or closes | The system's window animation |
//! | Pane, language or theme chosen | The pane button press-washes; the pane cross-fades (`MotionPreset::FadeIn`) |
//! | List row hovered | Wash fades in, `motion-instant`, ease-out (`motion::state_wash`) |
//! | List row pressed | Scales to 0.97, `motion-fast`, soft settle (`motion::press`) |
//! | Kit controls (buttons, checkboxes, popups, fields, steppers) | The kit's own recipes |
//! | Pane shown | Cross-fades in (`MotionPreset::FadeIn`, ease-out) |
//! | Shell variable added or removed | None: a short table, the row appears in place |
//! | Language edit rejected | None: the message appears under the fields at once |
//! | Loading the language list | None: the registry loads in well under a second |

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::{SelectItem, tab, tabs};
use nsh_core::Size;
use nsh_core::desktop::{WindowAttributes, WindowKind};
use nsh_core::element::desktop::{MenuItem, OpenChange, open_window, popup_menu, window};
use scratch_config::{
    ConsolePosition, FILE_ENCODINGS, InsertNewTabs, InsertionPointStyle, InvisiblesMode, Loaded, Paths, Preferences, default_environment,
};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::state::{self, display, with};

/// Width of the label column of a form row.
const LABEL_W: f32 = 150.0;
/// How long a text field waits after the last keystroke before applying.
const TYPING_PAUSE: Duration = Duration::from_millis(700);
const WINDOW_ID: &str = "prefs";

// ------------------------------------------------------------ settings file

thread_local! { static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) }; }

/// Reads `settings.toml` into [`settings`]. Called once at start, after the
/// data directory is known. A file that cannot be read or holds invalid
/// values never stops the app: those values keep their defaults, and the
/// returned notice (also put in the status bar) says what was ignored.
pub fn load(paths: &Paths) -> Option<String> {
    let path = paths.settings();
    let (prefs, notice) = read(&path);
    FILE.with_borrow_mut(|file| *file = Some(path));
    settings().set(prefs);
    if let Some(notice) = &notice { state::notice(notice.clone()); }
    // For screenshots and manual checks: open the window at launch on a pane.
    // "Languages:1:c" also picks the language-detail sub-tab and language.
    if let Some(pane) = std::env::var_os("SCRATCHRUNNER_OPEN_PREFERENCES") {
        let pane = pane.to_string_lossy();
        let mut parts = pane.split(':');
        let category = parts.next().unwrap_or("");
        if let Some(index) = parts.next() {
            if index == "fta" {
                global::<FileTypesSheet>().set(FileTypesSheet(true));
            } else if let Ok(index) = index.parse::<usize>() {
                match Category::from_identifier(category) {
                    Category::TextEditing => global::<TextEditingSubTab>().set(TextEditingSubTab(index)),
                    Category::Keyboard => global::<BindingsTab>().set(BindingsTab(index)),
                    _ => global::<LanguageSubTab>().set(LanguageSubTab(index)),
                }
            }
        }
        if let Some(language) = parts.next() {
            let language = language.strip_suffix('+').unwrap_or(language);
            global::<SelectedLanguage>().set(SelectedLanguage(language.to_owned()));
            if parts.next() == Some("expand") || pane.ends_with('+') {
                global::<DiagExpanded>().update(|e| { e.0.insert(format!("{language}:0")); });
            }
        }
        show(Category::from_identifier(category));
    }
    notice
}

/// The preferences in `path` and a one-line notice of what was ignored.
fn read(path: &std::path::Path) -> (Preferences, Option<String>) {
    let loaded = Loaded::load(path);
    let notice = match loaded.warnings.as_slice() {
        [] => None,
        [one] => Some(format!("Settings: {}", one.message)),
        [first, rest @ ..] => Some(format!("Settings: {} (and {} more problems; those values use their defaults)", first.message, rest.len())),
    };
    (loaded.prefs, notice)
}

/// The live preferences. Views read them; the window writes them.
pub fn settings() -> Signal<Preferences> { global::<Preferences>() }

/// Writes the live preferences to `settings.toml` (atomically; comments and
/// unknown keys in the file survive). Call after changing `settings()`.
pub fn save() {
    let Some(path) = FILE.with_borrow(Clone::clone) else { return };
    if let Err(error) = settings().with_untracked(|prefs| prefs.save(&path)) {
        state::notice(format!("Preferences could not be saved: {error}"));
    }
}

/// Changes the preferences and saves them.
pub(super) fn write(change: impl FnOnce(&mut Preferences)) {
    settings().update(change);
    save();
}

/// A field of the preferences, narrowed so its readers run only when it changes.
fn bound<T: Clone + PartialEq + 'static>(get: fn(&Preferences) -> T) -> Memo<T> {
    computed(move || settings().with(get))
}

// ------------------------------------------------------- typed text edits

/// Text typed into a field and not applied yet, by field key, with the
/// generation of its last keystroke.
type Pending = Vec<(String, u64, Box<dyn FnOnce()>)>;

thread_local! {
    static PENDING: RefCell<Pending> = const { RefCell::new(Vec::new()) };
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Holds `apply` for the field `key` until typing pauses (or the pane or
/// window goes away), replacing what the field held before.
pub(crate) fn defer(key: String, apply: impl FnOnce() + 'static) {
    let generation = GENERATION.with(|g| { g.set(g.get() + 1); g.get() });
    PENDING.with_borrow_mut(|pending| {
        pending.retain(|(k, _, _)| *k != key);
        pending.push((key.clone(), generation, Box::new(apply)));
    });
    set_timeout(TYPING_PAUSE, move || apply_pending(Some((&key, generation))));
}

/// Applies the held text of one field (when it was not typed into since
/// `generation`), or of every field.
fn apply_pending(only: Option<(&str, u64)>) {
    let taken: Vec<Box<dyn FnOnce()>> = PENDING.with_borrow_mut(|pending| {
        let (take, keep): (Pending, Pending) = std::mem::take(pending).into_iter()
            .partition(|(key, generation, _)| only.is_none_or(|(k, g)| key == k && *generation == g));
        *pending = keep;
        take.into_iter().map(|(_, _, apply)| apply).collect()
    });
    for apply in taken { apply(); }
}

/// Everything typed and not yet applied, applied now.
fn flush() {
    apply_pending(None);
    commit_language_draft();
}

// ------------------------------------------------------------------ window

/// A category of the window, in list order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Category {
    #[default]
    General,
    TextEditing,
    Shell,
    Languages,
    Keyboard,
    Themes,
    Advanced,
}

impl Category {
    const ALL: [Category; 7] = [Self::General, Self::TextEditing, Self::Shell, Self::Languages, Self::Keyboard, Self::Themes, Self::Advanced];

    fn title(self) -> &'static str {
        match self {
            Self::General => "General", Self::TextEditing => "Text Editing", Self::Shell => "Shell",
            Self::Languages => "Languages", Self::Keyboard => "Keyboard", Self::Themes => "Themes", Self::Advanced => "Advanced",
        }
    }

    /// The value kept in `ui.selected_preferences_pane` (CodeRunner's pane ids).
    fn identifier(self) -> &'static str {
        match self {
            Self::General => "General", Self::TextEditing => "TextEditing", Self::Shell => "Shell",
            Self::Languages => "Languages", Self::Keyboard => "KeyBindings", Self::Themes => "Themes", Self::Advanced => "Advanced",
        }
    }

    /// The toolbar icon of the pane (the `Pref*` artwork where it exists).
    fn icon_name(self) -> IconName {
        match self {
            Category::General => IconName::PrefGeneral,
            Category::TextEditing => IconName::PrefText,
            Category::Shell => IconName::Terminal,
            Category::Languages => IconName::PrefLanguages,
            Category::Keyboard => IconName::Keyboard,
            Category::Themes => IconName::PrefThemes,
            Category::Advanced => IconName::Wrench,
        }
    }

    fn from_identifier(identifier: &str) -> Self {
        Self::ALL.into_iter().find(|c| c.identifier() == identifier).unwrap_or_default()
    }
}

#[derive(Clone, Default, PartialEq)]
struct PrefsWindow { open: bool, pane: Category }

fn ui() -> Signal<PrefsWindow> { global::<PrefsWindow>() }

/// Opens the Preferences window on the pane it showed last.
pub fn open() {
    let pane = settings().with_untracked(|prefs| Category::from_identifier(&prefs.ui.selected_preferences_pane));
    show(pane);
}

/// Opens the Preferences window on `pane` (`Edit Languages…` opens Languages).
pub fn open_pane(pane: Category) { show(pane); }

/// Opens Preferences on Text Editing with its Diagnostics segment selected
/// (Diagnostics > Diagnostics Preferences…).
pub fn open_text_editing_diagnostics() {
    global::<TextEditingSubTab>().set(TextEditingSubTab(4));
    show(Category::TextEditing);
}

/// Opens Preferences > Languages with `language` selected on its
/// Diagnostics segment (Diagnostics > "Customize ……").
pub fn open_language_diagnostics(language: &str) {
    global::<SelectedLanguage>().set(SelectedLanguage(language.to_owned()));
    global::<LanguageSubTab>().set(LanguageSubTab(3));
    show(Category::Languages);
}

fn show(pane: Category) {
    if ui().with_untracked(|w| w.open) {
        choose(pane);
        // Already open: bring it to the front.
        let _ = open_window(window().id(WINDOW_ID));
    } else {
        ui().set(PrefsWindow { open: true, pane });
    }
}

fn choose(pane: Category) {
    flush();
    ui().update(|w| w.pane = pane);
    write(|prefs| prefs.ui.selected_preferences_pane = pane.identifier().to_owned());
}

/// The window, mounted once at the root.
pub fn overlay() -> impl IntoElement {
    let mut attributes = WindowAttributes::default();
    attributes.min_size = Some(Some(Size::new(px(700.0), px(500.0))));
    attributes.resizable = Some(true);
    window()
        .id(WINDOW_ID)
        .kind(WindowKind::Dialog)
        .title(move || format!("Preferences \u{2013} {}", ui().with(|w| w.pane.title())))
        .default_size(Size::new(px(780.0), px(620.0)))
        .attributes(attributes)
        .open(move || ui().with(|w| w.open))
        .on_open_change(|change: &OpenChange| {
            if !change.open {
                flush();
                ui().update(|w| w.open = false);
            }
        })
        .content(content)
}

fn content() -> impl IntoElement {
    let pane = computed(move || ui().with(|w| w.pane));
    view().flex_1().flex_col().min_h(px(0.0)).bg(tokens::background).text_color(tokens::foreground)
        .child(prefs_toolbar(pane))
        .child(view().flex_1().min_w(px(0.0)).min_h(px(0.0))
            .child(dynamic(move || { let pane = pane.get(); untracked(|| pane_view(pane)).into_element() })))
        .child(dynamic(move || {
            if global::<FileTypesSheet>().get().0 { file_types_sheet().into_element() } else { view().into_element() }
        }))
}

/// The window's top bar: one icon-over-label button per pane, centered as
/// CodeRunner's `NSToolbar` and macOS Settings show their panes. The window
/// title already names the pane, so the bar carries no heading.
fn prefs_toolbar(pane: Memo<Category>) -> impl IntoElement {
    view().w_full().flex_shrink_0().border_b_1().border_color(tokens::border).bg(tokens::sidebar)
        .child(view().label("Preference panes").child(
            tabs().flex_row().items_end().justify_center().gap(px(4.0)).px_4().py(px(5.0))
            .children(Category::ALL.into_iter().map(move |category| {
                let selected = computed(move || pane.get() == category);
                pane_tab(category, selected)
            }))))
}

/// A pane button: the pane's icon (`IconSize::Xl`, the preferences-strip
/// size) over its name. The selected pane's cell washes and its icon tints
/// with the primary accent, like the system's toolbar selection.
fn pane_tab(category: Category, selected: Memo<bool>) -> nsh_base::TabElement {
    let name = category.icon_name();
    let cell = tab().selected(selected).on_press(move || choose(category))
        .w(px(78.0)).pt(px(4.0)).pb(px(3.0)).flex_col().items_center().gap(px(3.0)).rounded(px(tokens::radius()))
        .accessibility_label(category.title())
        .bg(move || if selected.get() { tokens::accent() } else { tokens::sidebar() })
        .hover(|s| s.bg(tokens::control_hover))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)))
        .child(dynamic(move || glyph(name, IconSize::Xl, if selected.get() { Tone::Primary } else { Tone::Default }).into_element()))
        .child(text(category.title()).number_of_lines(1)
            .text_size(px(tokens::type_caption()))
            .text_color(move || if selected.get() { tokens::primary() } else { tokens::foreground() }));
    motion::press(cell)
}

/// A row of a vertical selection list (categories, languages): the list's
/// one indicator marks the selected row.
fn list_tab(selected: Memo<bool>, on_press: impl Fn() + 'static) -> nsh_base::TabElement {
    let row = tab().selected(selected).on_press(on_press)
        .flex_row().items_center().justify_between().gap_2().h(px(tokens::row_md())).px_2().rounded(px(tokens::radius()))
        .hover(|s| s.bg(tokens::control_hover))
        .active(|s| motion::press_scale(s, false))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(tokens::ring).outline_offset(px(1.0)));
    motion::press(motion::state_wash(row))
}

fn pane_view(pane: Category) -> View {
    let fade = motion().entering(MotionPreset::FadeIn);
    let body = match pane {
        Category::General => form(general()).into_element(),
        Category::TextEditing => text_editing().into_element(),
        Category::Shell => form(shell()).into_element(),
        Category::Languages => languages().into_element(),
        Category::Keyboard => keyboard().into_element(),
        Category::Themes => themes().into_element(),
        Category::Advanced => form(advanced()).into_element(),
    };
    view().flex_1().min_h(px(0.0)).bind_motion(&fade).child(body)
}

// ------------------------------------------------------------ form pieces

/// The form column width: CodeRunner's panes lay controls out on a ~600pt
/// centered column (a ~150pt right-aligned label spine plus controls).
const FORM_W: f32 = 600.0;

/// The sections of a form, a hairline between each pair.
fn with_separators(sections: Vec<AnyElement>) -> Vec<AnyElement> {
    let mut children = Vec::with_capacity(sections.len() * 2);
    for (index, item) in sections.into_iter().enumerate() {
        if index > 0 { children.push(separator(SeparatorOrientation::Horizontal).into_element()); }
        children.push(item);
    }
    children
}

/// A scrolling column of sections on a centered fixed-width column.
fn form(sections: Vec<AnyElement>) -> impl IntoElement {
    scroll().flex_1().min_h(px(0.0))
        .child(view().w_full().flex_col().items_center()
            .child(view().w(px(FORM_W)).pt_5().pb_6().gap_5().children(with_separators(sections))))
}

/// A bordered list box for the editable lists of the detail panes
/// (templates, doc resources, diagnostics).
fn card(rows: Vec<AnyElement>) -> View {
    view().w_full().gap_3().p_4().rounded(px(tokens::radius_lg())).border_1().border_color(tokens::border)
        .bg(tokens::card).text_color(tokens::card_foreground).children(rows)
}

/// A titled group of form rows: the section header over plainly laid-out
/// rows — no card, the hairlines between sections do the grouping.
fn section(title: &'static str, rows: Vec<AnyElement>) -> AnyElement {
    view().gap_2()
        .child(section_header(title, SectionHeaderVariant::Form, SectionHeaderOptions::new()))
        .child(view().w_full().gap(px(8.0)).children(rows))
        .into_element()
}

/// A labelled row.
fn row(label: &'static str, control: impl IntoElement) -> AnyElement {
    form_row(label, control, FormRowOptions::new().label_width(LABEL_W)).into_element()
}

/// A labelled row with a caption under its control.
fn row_with_caption(label: &'static str, control: impl IntoElement, caption: &'static str) -> AnyElement {
    form_row(label, control, FormRowOptions::new().label_width(LABEL_W).caption(caption)).into_element()
}

/// A checkbox bound to a boolean preference, in the control column.
fn check(label: &'static str, get: fn(&Preferences) -> bool, set: fn(&mut Preferences, bool)) -> AnyElement {
    row("", check_box(label, get, set))
}

fn check_box(label: &'static str, get: fn(&Preferences) -> bool, set: fn(&mut Preferences, bool)) -> impl IntoElement {
    checkbox(label, bound(get), CheckboxOptions::new(), move |next| write(move |prefs| set(prefs, next)))
}

/// One choice of a popup.
#[derive(Clone)]
struct Opt<T> { value: T, title: &'static str, available: bool }

fn opt<T>(value: T, title: &'static str) -> Opt<T> { Opt { value, title, available: true } }

/// A popup bound to a preference.
fn choice<T: Clone + PartialEq + 'static>(label: &'static str, options: Vec<Opt<T>>, get: fn(&Preferences) -> T, set: fn(&mut Preferences, T)) -> impl IntoElement {
    let options: Rc<[Opt<T>]> = options.into();
    // The kit's trigger shows the chosen value, so an item's value is its
    // title (unique within a popup).
    let items = options.iter().map(|o| SelectItem::new(o.title, o.title).disabled(!o.available)).collect();
    let current = { let options = options.clone(); computed(move || { let value = settings().with(get); options.iter().find(|o| o.value == value).map(|o| o.title) }) };
    select_popup(label, move || current.get().map(SharedString::from), "—", items, SelectPopupOptions::new(), move |next| {
        let picked = next.and_then(|title| options.iter().find(|o| o.title == title.as_ref()).map(|o| o.value.clone()));
        if let Some(value) = picked { write(move |prefs| set(prefs, value)); }
    })
}

/// The editor font family: the installed fixed-pitch families, via
/// `NSFontManager` (`native::fixed_pitch_families`). The family list
/// arrives on a main-thread warm after startup, so the popup repopulates
/// when `FontRevision` bumps; the shown name is the resolved family (a
/// persisted family that is not installed falls back to the default
/// monospace, as the editor does). The "Select…" button beside it opens
/// the system font panel for anything beyond the list.
fn font_family_popup() -> impl IntoElement {
    dynamic(move || {
        global::<state::FontRevision>().get();
        let items: Vec<SelectItem> = super::native::fixed_pitch_families()
            .iter()
            .map(|family| SelectItem::new(family.clone(), family.clone()))
            .collect();
        let family = settings().with(|prefs| {
            super::native::font_family_or_default(&prefs.text_editing.appearance.font.family, scratch_config::defaults::FONT_FAMILY)
        });
        select_popup("Editor font family", move || Some(SharedString::from(family.clone())), "—", items, SelectPopupOptions::new(), move |next| {
            if let Some(family) = next {
                write(move |prefs| prefs.text_editing.appearance.font.family = family.to_string());
            }
        }).into_element()
    })
}

/// A number field bound to a preference. Values outside `min..=max` (a
/// number still being typed) are not applied.
fn number(label: &'static str, options: NumberFieldOptions, get: fn(&Preferences) -> f64, set: fn(&mut Preferences, f64)) -> impl IntoElement {
    let (min, max) = (options.min.unwrap_or(f64::MIN), options.max.unwrap_or(f64::MAX));
    number_field(label, bound(get), options, move |next| {
        if (min..=max).contains(&next) { write(move |prefs| set(prefs, next)); }
    })
}

/// A text field that applies its text once typing pauses, or when the pane
/// or window goes away (not per keystroke: a half-typed font name or path
/// is never applied). `key` names the field among the pending edits.
fn field(key: String, placeholder: &'static str, initial: String, options: TextFieldOptions, apply: impl Fn(String) + 'static) -> impl IntoElement {
    let apply = Rc::new(apply);
    let draft = signal(initial);
    text_field(placeholder, move || SharedString::from(draft.get()), move |value: &str| {
        draft.set(value.to_owned());
        let (apply, value) = (apply.clone(), value.to_owned());
        defer(key.clone(), move || apply(value));
    }, options, None)
}

/// A field bound to a text preference.
fn text_pref(label: &'static str, placeholder: &'static str, options: TextFieldOptions, get: fn(&Preferences) -> String, set: fn(&mut Preferences, String)) -> impl IntoElement {
    field(label.to_owned(), placeholder, settings().with_untracked(get), options.accessibility_label(label), move |value| {
        if settings().with_untracked(get) != value { write(move |prefs| set(prefs, value)); }
    })
}

// ------------------------------------------------------------------ panes

fn general() -> Vec<AnyElement> {
    let encodings = FILE_ENCODINGS.iter().filter(|o| !o.separator).filter_map(|o| match o.value {
        scratch_config::ConstValue::Str(value) => Some(opt(value.to_owned(), o.title)),
        _ => None,
    }).collect();
    vec![
        section("Documents", vec![
            check("Enable Auto Save and Versions", |p| p.general.documents.autosave, |p, v| p.general.documents.autosave = v),
            check("Show all filename extensions", |p| p.general.documents.show_all_extensions, |p, v| p.general.documents.show_all_extensions = v),
            check("Ask to save untitled documents when quitting", |p| p.general.documents.ask_to_save_when_quitting, |p, v| p.general.documents.ask_to_save_when_quitting = v),
            check("Use templates for new files", |p| p.general.documents.use_templates_for_new_files, |p, v| p.general.documents.use_templates_for_new_files = v),
            check("Allow filename suggestions from macOS", |p| p.general.documents.allow_suggested_filenames, |p, v| p.general.documents.allow_suggested_filenames = v),
            row("Default File Encoding:", choice("Default File Encoding", encodings, |p| p.general.documents.file_encoding.clone(), |p, v| p.general.documents.file_encoding = v)),
            row("", view().child(button("Edit File Type Associations…", ButtonOptions::new().variant(ButtonVariant::Default),
                || global::<FileTypesSheet>().set(FileTypesSheet(true))))),
        ]),
        section("Tabs", vec![
            check("Prefer opening new documents in tabs", |p| p.general.tabs.prefer_tabs, |p, v| p.general.tabs.prefer_tabs = v),
            row("Insert New Tabs:", choice("Insert New Tabs", vec![
                opt(InsertNewTabs::AfterExisting, "After existing tabs"),
                opt(InsertNewTabs::BeforeExisting, "Before existing tabs"),
                opt(InsertNewTabs::AfterSelected, "After the selected tab"),
            ], |p| p.general.tabs.insert_new_tabs, |p, v| p.general.tabs.insert_new_tabs = v)),
        ]),
        section("Console", vec![
            row("Console Position:", choice("Console Position", vec![opt(ConsolePosition::Bottom, "Bottom"), opt(ConsolePosition::Right, "Right")],
                |p| p.general.console.position, |p, v| p.general.console.position = v)),
            check("Hide console automatically", |p| p.general.console.hide_automatically, |p, v| p.general.console.hide_automatically = v),
        ]),
        section("File Navigator", vec![
            check("Show hidden files and folders", |p| p.general.file_navigator.show_hidden_files, |p, v| p.general.file_navigator.show_hidden_files = v),
            check("Sort folders on top", |p| p.general.file_navigator.sort_folders_on_top, |p, v| p.general.file_navigator.sort_folders_on_top = v),
        ]),
    ]
}

// ------------------------------------------------------- file type sheet

/// Whether the Edit File Type Associations sheet is open (a dialog inside
/// the Preferences window).
#[derive(Clone, Copy, Default, PartialEq)]
struct FileTypesSheet(bool);

/// The rows of the sheet: the two built-in types then the user's added
/// extensions (DESIGN 8.13: `Plain Text`, `Source Code`, added extensions).
fn file_type_rows(added: &[String]) -> Vec<(String, String)> {
    [("public.plain-text".to_owned(), "Plain Text".to_owned()),
     ("public.source-code".to_owned(), "Source Code".to_owned())]
        .into_iter()
        .chain(added.iter().map(|ext| (ext.clone(), format!(".{ext}"))))
        .collect()
}

fn file_types_sheet() -> impl IntoElement {
    let (added, checked) = settings().with_untracked(|prefs| (
        prefs.general.added_file_type_association_extensions.clone(),
        prefs.general.allowed_file_types.clone()
            .unwrap_or_else(|| scratch_config::DEFAULT_ALLOWED_FILE_TYPES.iter().map(|s| s.to_string()).collect())
            .into_iter().collect::<std::collections::BTreeSet<_>>(),
    ));
    let added = signal(added);
    let checked = signal(checked);
    let prompting = signal(false);
    let typed = signal(String::new());

    let close = || global::<FileTypesSheet>().set(FileTypesSheet(false));
    let commit = move || {
        write(|prefs| {
            prefs.general.allowed_file_types = Some(checked.get_untracked().into_iter().collect());
            prefs.general.added_file_type_association_extensions = added.get_untracked();
        });
        close();
    };

    let enter = |from: f32| MotionSpec::new(KeyframeTiming::new(motion::slow()).easing(motion::EASE_ENTER)).values(MotionChannel::Opacity, [from, 1.0]);
    let scrim_motion = motion().entering(enter(0.0));
    let sheet_motion = motion().entering(enter(0.0)
        .values(MotionChannel::TranslateY, [-motion::DISTANCE_SHEET, 0.0])
        .values(MotionChannel::Scale, [motion::POPUP_SCALE, 1.0]));

    let rows = computed(move || file_type_rows(&added.get()));
    let row_list = scroll().flex_shrink(1.0).min_h(px(0.0)).max_h(px(300.0))
        .rounded(px(tokens::radius())).border_1().border_color(tokens::border)
        .child(view().p_1().gap(px(1.0)).child(each(move || rows.get(), |row: &(String, String)| row.0.clone(), move |row, _| {
            let (id, title) = row.get_untracked();
            let checked = checked;
            let on = { let id = id.clone(); computed(move || checked.with(|set| set.contains(&id))) };
            view().flex_row().items_center().gap_2().h(px(tokens::row_md())).px_2()
                .child(checkbox(title, move || on.get(), CheckboxOptions::default(), {
                    let id = id.clone();
                    move |next: bool| checked.update(|set| { if next { set.insert(id.clone()); } else { set.remove(&id); } })
                }))
        })));

    let add_extensions = move || {
        let parsed: Vec<String> = typed.get_untracked().split(|c: char| c == ',' || c.is_whitespace())
            .map(|part| part.trim().trim_start_matches('.')).filter(|part| !part.is_empty())
            .map(str::to_lowercase).collect();
        if parsed.is_empty() { return; }
        added.update(|list| for ext in &parsed { if !list.contains(ext) { list.push(ext.clone()); } });
        checked.update(|set| for ext in parsed { set.insert(ext); });
        typed.set(String::new());
        prompting.set(false);
    };

    nsh_base::dialog()
        .animation(ModalAnimation::None)
        .label("File Type Associations")
        .on_cancel(close)
        .flex_1()
        .child(view().absolute().inset_0().bg(tokens::scrim).bind_motion(&scrim_motion))
        .child(view().flex_1().items_center().pt(px(44.0)).pb(px(12.0))
            .child(view().bind_motion(&sheet_motion)
                .w(px(520.0)).min_h(px(0.0)).flex_shrink(1.0)
                .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
                .shadow_lg().shadow_color(tokens::shadow())
                .bg(tokens::background)
                .child(view().px(px(20.0)).pt(px(20.0)).pb(px(12.0)).gap(px(2.0))
                    .child(type_subhead().semibold().apply(text("File Type Associations")).text_color(tokens::foreground))
                    .child(type_body().apply(text("Select the file types you would like ScratchRunner to open by default."))
                        .text_color(tokens::muted_foreground)))
                .child(view().px(px(20.0)).child(row_list))
                .child(view().px(px(20.0)).pt(px(8.0)).flex_row().gap(px(8.0))
                    .child(button("Select All", ButtonOptions::new().variant(ButtonVariant::Default),
                        move || checked.set(file_type_rows(&added.get_untracked()).into_iter().map(|row| row.0).collect())))
                    .child(button("Deselect All", ButtonOptions::new().variant(ButtonVariant::Default),
                        move || checked.set(std::collections::BTreeSet::new())))
                    .child(view().flex_1())
                    .child(button("Add File Extensions…", ButtonOptions::new().variant(ButtonVariant::Default), move || prompting.set(true))))
                .child(when(move || prompting.get(), move || view().px(px(20.0)).pt(px(8.0)).flex_row().items_center().gap(px(8.0))
                    .child(view().flex_1().child(text_field("sh, ps1, xyz", move || SharedString::from(typed.get()), {
                        let typed = typed;
                        move |text: &str| typed.set(text.to_owned())
                    }, TextFieldOptions::new().mono(true).accessibility_label("File extensions"), None)))
                    .child(button("Add", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), add_extensions))))
                .child(when(move || prompting.get(), || view().px(px(20.0)).pt(px(4.0))
                    .child(type_caption().apply(text("Enter file extensions, comma separated.")).text_color(tokens::muted_foreground))))
                .child(view().flex_row().justify_end().gap(px(8.0)).px(px(20.0)).pt(px(16.0)).pb(px(20.0))
                    .child(button("Cancel", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Lg), close))
                    .child(button("Set as Default", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Lg), commit)))))
}

/// The selected Text Editing category (DESIGN 8.13.1: `Appearance |
/// Indentation | Behavior | Completion | Diagnostics`).
#[derive(Clone, Copy, Default, PartialEq)]
struct TextEditingSubTab(usize);

fn text_editing() -> impl IntoElement {
    let tab = global::<TextEditingSubTab>();
    view().flex_1().min_h(px(0.0)).flex_col().gap_1()
        .child(view().flex_row().justify_center().pt_3().pb_1()
            .child(segmented("Text Editing category",
                ["Appearance", "Indentation", "Behavior", "Completion", "Diagnostics"].into_iter().map(Segment::new),
                move || tab.get().0,
                |index| global::<TextEditingSubTab>().set(TextEditingSubTab(index)),
                SegmentedOptions::new())))
        .child(scroll().flex_1().min_h(px(0.0)).child(dynamic(move || {
            (match tab.get().0 {
                1 => text_indentation,
                2 => text_behavior,
                3 => text_completion,
                4 => text_diagnostics,
                _ => text_appearance,
            })()
        })))
}

/// One category's rows: the same centered column `form` produces.
fn text_sections(sections: Vec<AnyElement>) -> AnyElement {
    view().w_full().flex_col().items_center()
        .child(view().w(px(FORM_W)).pt_3().pb_6().gap_5().children(with_separators(sections)))
        .into_element()
}

fn text_appearance() -> AnyElement {
    text_sections(vec![
        section("Font", vec![
            row("Font:", view().flex_row().items_center().gap_2()
                .child(view().w(px(180.0)).child(font_family_popup()))
                .child(number("Font size", NumberFieldOptions::new().range(6.0, 72.0).suffix("pt"), |p| p.text_editing.appearance.font.size, |p, v| p.text_editing.appearance.font.size = v))
                .child(button("Select…", ButtonOptions::new().variant(ButtonVariant::Default), super::edit::show_fonts))),
            row("Line Spacing:", number("Line spacing", NumberFieldOptions::new().range(1.0, 3.0).step(0.05), |p| p.text_editing.appearance.line_height, |p, v| p.text_editing.appearance.line_height = v)),
        ]),
        section("Appearance", vec![
            check("Syntax highlighting", |p| p.text_editing.appearance.syntax_highlighting, |p, v| p.text_editing.appearance.syntax_highlighting = v),
            check("Wrap lines", |p| p.text_editing.appearance.wrap_lines, |p, v| p.text_editing.appearance.wrap_lines = v),
            check("Allow scrolling past end of document", |p| p.text_editing.appearance.allow_scroll_past_end, |p, v| p.text_editing.appearance.allow_scroll_past_end = v),
            check("Highlight current line", |p| p.text_editing.appearance.highlight_current_line, |p, v| p.text_editing.appearance.highlight_current_line = v),
            row("Page Guide:", view().flex_row().items_center().gap_2()
                .child(check_box("Show", |p| p.text_editing.appearance.page_guide.show, |p, v| p.text_editing.appearance.page_guide.show = v))
                .child(type_body().apply(text("at column")))
                .child(number("Page guide column", NumberFieldOptions::new().range(1.0, 400.0), |p| p.text_editing.appearance.page_guide.column as f64, |p, v| p.text_editing.appearance.page_guide.column = v as u32))),
        ]),
        section("Scroller", vec![
            check("Show navigational markers in the scroller", |p| p.text_editing.appearance.scroll_markers, |p, v| p.text_editing.appearance.scroll_markers = v),
            check("Keep markers visible when the scroller is hidden", |p| p.text_editing.appearance.scroll_markers_always_visible, |p, v| p.text_editing.appearance.scroll_markers_always_visible = v),
        ]),
        section("Line Numbers", vec![
            check("Show line numbers", |p| p.text_editing.appearance.line_numbers.enabled, |p, v| p.text_editing.appearance.line_numbers.enabled = v),
            check("Line numbers relative to the selection", |p| p.text_editing.appearance.line_numbers.relative, |p, v| p.text_editing.appearance.line_numbers.relative = v),
            check("Draw a separator at the left margin", |p| p.text_editing.appearance.line_numbers.draw_separator, |p, v| p.text_editing.appearance.line_numbers.draw_separator = v),
        ]),
        section("Invisible Characters", vec![
            check("Show invisible characters", |p| p.text_editing.appearance.invisibles.show, |p, v| p.text_editing.appearance.invisibles.show = v),
            row("Show:", choice("Invisible characters mode", vec![
                opt(InvisiblesMode::Everywhere, "Everywhere"),
                opt(InvisiblesMode::BeginningAndEnd, "At beginning and end of lines"),
                opt(InvisiblesMode::NonconformingIndentation, "Nonconforming indentation"),
                opt(InvisiblesMode::Selection, "In selected text"),
                opt(InvisiblesMode::SelectionAndNonconformingIndentation, "In selected text and nonconforming indentation"),
            ], |p| p.text_editing.appearance.invisibles.mode, |p, v| p.text_editing.appearance.invisibles.mode = v)),
        ]),
        section("Insertion Point", vec![
            row("Insertion Point:", choice("Insertion Point", vec![
                opt(InsertionPointStyle::Bar, "Bar"),
                // The editor draws a bar caret only (DESIGN.md G8).
                Opt { value: InsertionPointStyle::Block, title: "Block", available: false },
                Opt { value: InsertionPointStyle::Underline, title: "Underline", available: false },
            ], |p| p.text_editing.appearance.insertion_point.style, |p, v| p.text_editing.appearance.insertion_point.style = v)),
            check("Blink the insertion point", |p| p.text_editing.appearance.insertion_point.blink, |p, v| p.text_editing.appearance.insertion_point.blink = v),
            check("Animate the blink", |p| p.text_editing.appearance.insertion_point.animate, |p, v| p.text_editing.appearance.insertion_point.animate = v),
            check("Show the insertion point over selected text", |p| p.text_editing.appearance.insertion_point.visible_with_selection, |p, v| p.text_editing.appearance.insertion_point.visible_with_selection = v),
        ]),
    ])
}

fn text_indentation() -> AnyElement {
    text_sections(vec![
        section("Indentation", vec![
            row_with_caption("Indent Using:", view().flex_row().items_center().gap_2()
                .child(choice("Indent using", vec![opt(false, "Tabs"), opt(true, "Spaces")],
                    |p| p.text_editing.indentation.soft_tabs, |p, v| p.text_editing.indentation.soft_tabs = v))
                .child(choice("Indent width", vec![opt(2, "2"), opt(3, "3"), opt(4, "4"), opt(6, "6"), opt(8, "8")],
                    |p| p.text_editing.indentation.tab_width, |p, v| p.text_editing.indentation.tab_width = v)),
                "Indentation can be customized on a per-language basis under the Languages tab."),
            row("Wrapped Lines:", view().flex_row().items_center().gap_2()
                .child(check_box("Indent", |p| p.text_editing.indentation.indent_wrapped_lines, |p, v| p.text_editing.indentation.indent_wrapped_lines = v))
                .child(type_body().apply(text("by")))
                .child(number("Wrapped line indent levels", NumberFieldOptions::new().range(0.0, 8.0).step(0.5), |p| p.text_editing.indentation.indent_wrapped_lines_level, |p, v| p.text_editing.indentation.indent_wrapped_lines_level = v))
                .child(type_body().apply(text("level(s)")))),
            check("Treat consecutive spaces as a single unit", |p| p.text_editing.indentation.spaces_indentation_single_unit, |p, v| p.text_editing.indentation.spaces_indentation_single_unit = v),
            check("Auto-Indent while typing", |p| p.text_editing.indentation.auto_indent_while_typing, |p, v| p.text_editing.indentation.auto_indent_while_typing = v),
            check("Auto-Indent pasted code", |p| p.text_editing.indentation.auto_indent_on_paste, |p, v| p.text_editing.indentation.auto_indent_on_paste = v),
        ]),
    ])
}

fn text_behavior() -> AnyElement {
    let pair = |label: &'static str, get: fn(&Preferences) -> bool, set: fn(&mut Preferences, bool)| {
        view().w(px(56.0)).child(check_box(label, get, set)).into_element()
    };
    text_sections(vec![
        section("Character Pairs", vec![
            check("Automatically close character pairs", |p| p.text_editing.behavior.character_pairs.close_enabled, |p, v| p.text_editing.behavior.character_pairs.close_enabled = v),
            row("Close:", view().flex_row().items_center().gap_1()
                .child(pair("{", |p| p.text_editing.behavior.character_pairs.close_curly_brackets, |p, v| p.text_editing.behavior.character_pairs.close_curly_brackets = v))
                .child(pair("(", |p| p.text_editing.behavior.character_pairs.close_parens, |p, v| p.text_editing.behavior.character_pairs.close_parens = v))
                .child(pair("[", |p| p.text_editing.behavior.character_pairs.close_square_brackets, |p, v| p.text_editing.behavior.character_pairs.close_square_brackets = v))
                .child(pair("<", |p| p.text_editing.behavior.character_pairs.close_less_than, |p, v| p.text_editing.behavior.character_pairs.close_less_than = v))
                .child(pair("\"", |p| p.text_editing.behavior.character_pairs.close_double_quote, |p, v| p.text_editing.behavior.character_pairs.close_double_quote = v))
                .child(pair("'", |p| p.text_editing.behavior.character_pairs.close_single_quote, |p, v| p.text_editing.behavior.character_pairs.close_single_quote = v))
                .child(pair("`", |p| p.text_editing.behavior.character_pairs.close_backtick, |p, v| p.text_editing.behavior.character_pairs.close_backtick = v))),
            check("Jump past the ending character when its key is pressed", |p| p.text_editing.behavior.character_pairs.jump_past_enabled, |p, v| p.text_editing.behavior.character_pairs.jump_past_enabled = v),
            check("Automatically highlight balancing characters", |p| p.text_editing.behavior.character_pairs.highlight_closing_brackets, |p, v| p.text_editing.behavior.character_pairs.highlight_closing_brackets = v),
        ]),
        section("Selection", vec![
            check("Double-click also selects characters used in identifiers", |p| p.text_editing.behavior.word_selection_includes_variable_chars, |p, v| p.text_editing.behavior.word_selection_includes_variable_chars = v),
            row("Kill Ring Size:", number("Kill ring size", NumberFieldOptions::new().range(1.0, 64.0), |p| p.text_editing.behavior.kill_ring_size as f64, |p, v| p.text_editing.behavior.kill_ring_size = v as u32)),
        ]),
        section("Code Tokenizers", vec![
            check("Enable code tokenizers", |p| p.text_editing.behavior.tokenizers.enabled, |p, v| p.text_editing.behavior.tokenizers.enabled = v),
            check("Underline tokens at the insertion point", |p| p.text_editing.behavior.tokenizers.highlight_tokens, |p, v| p.text_editing.behavior.tokenizers.highlight_tokens = v),
        ]),
        section("Spelling", vec![
            check("Check spelling while typing", |p| p.text_editing.behavior.spelling.check_while_typing, |p, v| p.text_editing.behavior.spelling.check_while_typing = v),
            check("Check grammar with spelling", |p| p.text_editing.behavior.spelling.check_grammar, |p, v| p.text_editing.behavior.spelling.check_grammar = v),
            check("Correct spelling automatically", |p| p.text_editing.behavior.spelling.correct_automatically, |p, v| p.text_editing.behavior.spelling.correct_automatically = v),
        ]),
    ])
}

fn text_completion() -> AnyElement {
    text_sections(vec![
        section("Completion", vec![
            check("Suggest completions while typing", |p| p.text_editing.completion.enabled, |p, v| p.text_editing.completion.enabled = v),
            row("Suggest After:", view().flex_row().items_center().gap_2()
                .child(number("Characters before completing", NumberFieldOptions::new().range(1.0, 12.0), |p| p.text_editing.completion.word_length as f64, |p, v| p.text_editing.completion.word_length = v as u32))
                .child(type_body().apply(text("character(s)")))),
            check("Use word-completions for unsupported languages", |p| p.text_editing.completion.word_completions, |p, v| p.text_editing.completion.word_completions = v),
            check("Enable short string completions", |p| p.text_editing.completion.short_string_completions, |p, v| p.text_editing.completion.short_string_completions = v),
            check("Automatically insert import statements", |p| p.text_editing.completion.automatically_insert_imports, |p, v| p.text_editing.completion.automatically_insert_imports = v),
            check("Animate the completion panel", |p| p.text_editing.completion.animate_panel, |p, v| p.text_editing.completion.animate_panel = v),
        ]),
        section("Matching", vec![
            check("Fuzzy search", |p| p.text_editing.completion.fuzzy, |p, v| p.text_editing.completion.fuzzy = v),
            check("Ignore with only low-quality results", |p| p.text_editing.completion.fuzzy_limit, |p, v| p.text_editing.completion.fuzzy_limit = v),
            check("Force correct character order in matches", |p| p.text_editing.completion.fuzzy_enforce_character_order, |p, v| p.text_editing.completion.fuzzy_enforce_character_order = v),
            check("Prioritize previously chosen completions", |p| p.text_editing.completion.hits_enabled, |p, v| p.text_editing.completion.hits_enabled = v),
        ]),
    ])
}

fn text_diagnostics() -> AnyElement {
    text_sections(vec![
        section("Diagnostics", vec![
            check("Generate diagnostics while typing", |p| p.text_editing.diagnostics.while_typing, |p, v| p.text_editing.diagnostics.while_typing = v),
            row("After:", view().flex_row().items_center().gap_2()
                .child(number("Diagnostics delay in seconds", NumberFieldOptions::new().range(0.0, 10.0).step(0.1), |p| p.text_editing.diagnostics.delay, |p, v| p.text_editing.diagnostics.delay = v))
                .child(type_body().apply(text("second(s)")))),
            check("Show errors derived from syntax definitions", |p| p.text_editing.diagnostics.invalid_syntax, |p, v| p.text_editing.diagnostics.invalid_syntax = v),
            check("Only show the first syntax error in a document", |p| p.text_editing.diagnostics.invalid_syntax_limit_to_one, |p, v| p.text_editing.diagnostics.invalid_syntax_limit_to_one = v),
            check("Include errors from the console", |p| p.text_editing.diagnostics.from_console, |p, v| p.text_editing.diagnostics.from_console = v),
        ]),
        section("Display", vec![
            check("Animate diagnostic messages", |p| p.text_editing.diagnostics.animate, |p, v| p.text_editing.diagnostics.animate = v),
            check("Highlight lines with diagnostics", |p| p.text_editing.diagnostics.highlight_line, |p, v| p.text_editing.diagnostics.highlight_line = v),
            check("Underline text associated with diagnostics", |p| p.text_editing.diagnostics.underline, |p, v| p.text_editing.diagnostics.underline = v),
            check("Include diagnostics in the symbol navigator", |p| p.text_editing.diagnostics.symbol_list_includes_annotations, |p, v| p.text_editing.diagnostics.symbol_list_includes_annotations = v),
            check("Show the icon only instead of the full message", |p| p.text_editing.diagnostics.icon_only, |p, v| p.text_editing.diagnostics.icon_only = v),
        ]),
        section("Handling", vec![
            row("Automatically Fix:", text_area("Regular expressions, one per line",
                settings().with_untracked(|p| SharedString::from(p.text_editing.diagnostics.auto_fix_diagnostics.clone())),
                |next: &str| write(move |p| p.text_editing.diagnostics.auto_fix_diagnostics = next.to_owned()),
                TextFieldOptions::new().mono(true).accessibility_label("Auto-fix diagnostics"), None).into_element()),
            row("Ignore:", text_area("Regular expressions, one per line",
                settings().with_untracked(|p| SharedString::from(p.text_editing.diagnostics.ignored_diagnostics.clone())),
                |next: &str| write(move |p| p.text_editing.diagnostics.ignored_diagnostics = next.to_owned()),
                TextFieldOptions::new().mono(true).accessibility_label("Ignored diagnostics"), None).into_element()),
        ]),
    ])
}

fn shell() -> Vec<AnyElement> {
    let automatic = bound(|p| p.shell.automatically_determine_path);
    vec![
        section("PATH", vec![
            check("Automatically determine PATH environment variable", |p| p.shell.automatically_determine_path, |p, v| p.shell.automatically_determine_path = v),
            // The kit's field takes `disabled` once: made again when it flips.
            row("Custom PATH:", dynamic(move || {
                let disabled = automatic.get();
                untracked(|| text_pref("Custom PATH", "/usr/local/bin:/usr/bin:/bin", TextFieldOptions::new().mono(true).disabled(disabled),
                    |p| p.shell.path.clone(), |p, v| p.shell.path = v.trim().to_owned())).into_element()
            })),
        ]),
        section("Shell", vec![
            row_with_caption("Shell:", view().w(px(260.0)).child(text_pref("Shell", "/bin/bash", TextFieldOptions::new(), |p| p.shell.shell.clone(), |p, v| {
                let v = v.trim();
                if !v.is_empty() { p.shell.shell = v.to_owned(); }
            })), "Using a shell other than bash is not recommended and may cause undesirable behavior."),
            row_with_caption("Arguments:", view().w(px(260.0)).child(text_pref("Shell arguments", "none", TextFieldOptions::new().mono(true),
                |p| p.shell.arguments.join(" "), |p, v| p.shell.arguments = split_arguments(&v))),
                "Passed to the shell before the run command, separated by spaces."),
            form_row("", checkbox("Invoke shell in login mode when running code", bound(|p| p.shell.run_as_login_shell), CheckboxOptions::new(),
                move |next| write(move |p| p.shell.run_as_login_shell = next)),
                FormRowOptions::new().label_width(LABEL_W).caption("When invoked in login mode, the shell executes startup scripts such as ~/.profile.")).into_element(),
        ]),
        section("Shell Variables", vec![environment()]),
    ]
}

fn environment() -> AnyElement {
    let rows = computed(|| settings().with(|p| p.shell.environment.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()));
    let head = |label: &'static str| type_caption().medium().apply(text(label).text_color(tokens::muted_foreground));
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    view().gap_2()
        .child(view().flex_row().gap_2().child(view().w(px(180.0)).child(head("Variable"))).child(view().flex_1().child(head("Value"))))
        .child(each(rows, |(key, _): &(String, String)| key.clone(), move |row, _| {
            let (key, value) = row.get_untracked();
            let (renamed, edited, removed) = (key.clone(), key.clone(), key.clone());
            view().flex_row().items_center().gap_2()
                .child(view().w(px(180.0)).child(field(format!("env-name:{key}"), "NAME", key.clone(), TextFieldOptions::new().mono(true).accessibility_label("Variable"),
                    move |next| rename_variable(&renamed, next.trim()))))
                .child(view().flex_1().child(field(format!("env-value:{key}"), "value", value, TextFieldOptions::new().mono(true).accessibility_label("Value"), move |next| {
                    let key = edited.clone();
                    if settings().with_untracked(|p| p.shell.environment.get(&key) != Some(&next)) { write(move |p| { p.shell.environment.insert(key, next); }); }
                })))
                .child(icon_button(IconName::Minus, "Delete the shell variable", strip, move || {
                    let key = removed.clone();
                    write(move |p| { p.shell.environment.remove(&key); });
                }))
        }))
        .child(when(move || rows.with(Vec::is_empty), || type_body().apply(text("No Items").text_color(tokens::muted_foreground))))
        .child(view().flex_row().items_center().gap_2()
            .child(icon_button(IconName::Plus, "Add a shell variable", strip, || {
                flush();
                write(|p| { let name = unique_variable(&p.shell.environment); p.shell.environment.insert(name, String::new()); });
            }))
            .child(button("Reset", ButtonOptions::new().size(ControlSize::Sm), || {
                PENDING.with_borrow_mut(|pending| pending.retain(|(key, _, _)| !key.starts_with("env-")));
                write(|p| p.shell.environment = default_environment());
            })))
        .into_element()
}

fn rename_variable(old: &str, new: &str) {
    if new == old { return; }
    if new.is_empty() || settings().with_untracked(|p| p.shell.environment.contains_key(new)) {
        state::notice(if new.is_empty() { "A shell variable needs a name.".to_owned() } else { format!("The shell variable {new} already exists.") });
        return;
    }
    let (old, new) = (old.to_owned(), new.to_owned());
    write(move |p| {
        if let Some(value) = p.shell.environment.remove(&old) { p.shell.environment.insert(new, value); }
    });
}

/// `VARIABLE`, or `VARIABLE_2`, ... when taken.
fn unique_variable(environment: &std::collections::BTreeMap<String, String>) -> String {
    (1..).map(|n| if n == 1 { "VARIABLE".to_owned() } else { format!("VARIABLE_{n}") })
        .find(|name| !environment.contains_key(name)).expect("a free name")
}

/// Shell arguments typed as one line, split at whitespace.
fn split_arguments(line: &str) -> Vec<String> { line.split_whitespace().map(str::to_owned).collect() }

// ---------------------------------------------------------------- themes

/// The theme the editor on the right shows.
#[derive(Clone, Default, PartialEq)]
struct SelectedTheme(String);

/// The element row whose scope the field below the table edits.
#[derive(Clone, Copy, Default, PartialEq)]
struct SelectedThemeItem(usize);

/// "Show all" is checked: default element names the theme does not define
/// are listed too and created on first edit (CodeRunner's checkbox).
#[derive(Clone, Copy, Default, PartialEq)]
struct ShowAllThemeItems(bool);

/// "Rename…" was chosen: the theme editor shows a name field.
#[derive(Clone, Copy, Default, PartialEq)]
struct RenamingTheme(bool);

/// CodeRunner's default element names and scopes (`Default.tmTheme` +
/// `DefaultThemeItems.plist`), listed when "Show all" is on.
const DEFAULT_ELEMENTS: &[(&str, &str)] = &[
    ("Comment", "comment"),
    ("String", "string"),
    ("Number", "constant.numeric"),
    ("Built-in constant", "constant.language"),
    ("User-defined constant", "constant.character constant.other"),
    ("Variable", "variable"),
    ("Keyword", "keyword"),
    ("Storage", "storage"),
    ("Class name", "entity.name.class"),
    ("Inherited class", "entity.other.inherited-class"),
    ("Function name", "entity.name.function"),
    ("Function argument", "variable.parameter"),
    ("Tag name", "entity.name.tag"),
    ("Tag attribute", "entity.other.attribute-name"),
    ("Library function", "support.function"),
    ("Library constant", "support.constant"),
    ("Library class/type", "support.class"),
    ("Library variable", "support.other.variable"),
    ("Invalid", "invalid"),
];

fn themes() -> impl IntoElement {
    let revision = global::<super::theme::ThemesRevision>();
    let selected = global::<SelectedTheme>();
    let rows = computed(move || { revision.get(); super::theme::with_library(|library| library.entries()) });
    if selected.with_untracked(|s| s.0.is_empty()) {
        let name = settings().with_untracked(|prefs| prefs.themes.resolve(dark()).to_owned());
        selected.set(SelectedTheme(name));
    }
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    view().flex_1().min_h(px(0.0)).flex_row().gap_3().px_4().pb_4().pt_3()
        .child(view().w(px(200.0)).min_h(px(0.0)).gap_2()
            .child(view().flex_1().min_h(px(0.0)).rounded(px(tokens::radius_lg())).border_1().border_color(tokens::border).bg(tokens::card)
                .child(scroll().flex_1().min_h(px(0.0)).child(view().label("Color Themes").child(
                    tabs().flex_col().gap(px(1.0)).p_1()
                    .indicator_style(|s| s.left(px(0.0)).right(px(0.0)).rounded(px(tokens::radius())).bg(tokens::selection))
                    .child(each(move || rows.get(), |entry: &scratch_theme::ThemeEntry| entry.name.clone(), move |row, _| {
                        let entry = row.get_untracked();
                        let name = entry.name.clone();
                        let is_selected = { let name = name.clone(); computed(move || selected.with(|s| s.0 == name)) };
                        let pick = name.clone();
                        list_tab(is_selected, move || {
                            flush();
                            global::<RenamingTheme>().set(RenamingTheme(false));
                            if !settings().with_untracked(|p| p.themes.use_separate_color_themes) {
                                apply_theme(&pick);
                            }
                            selected.set(SelectedTheme(pick.clone()));
                            global::<SelectedThemeItem>().set(SelectedThemeItem(0));
                        })
                        .child(type_body().apply(text(move || row.with(|e| e.name.clone())).number_of_lines(1).flex_1().min_w(px(0.0))
                            .text_color(move || if is_selected.get() { tokens::selection_foreground() } else { tokens::foreground() })))
                        .child(text(if entry.is_dark { "\u{263e}" } else { "\u{263c}" })
                            .text_color(move || if is_selected.get() { tokens::selection_foreground() } else { tokens::muted_foreground() }))
                    }))))))
            .child(view().flex_row().items_center().gap_2()
                .child(icon_button(IconName::Plus, "Create a new theme", strip, || {
                    let items = vec![
                        MenuItem::new("New Light Theme").on_select(|| new_theme(false)),
                        MenuItem::new("New Dark Theme").on_select(|| new_theme(true)),
                    ];
                    spawn(async move { let _ = popup_menu(items).await; });
                }))
                .child(icon_button(IconName::Minus, "Delete the selected theme", strip, || {
                    let name = global::<SelectedTheme>().get_untracked().0;
                    if name.is_empty() { return; }
                    let result = super::theme::mutate_library(|library| library.delete(&name));
                    match result {
                        Ok(()) => global::<SelectedTheme>().set(SelectedTheme(String::new())),
                        Err(error) => state::notice(format!("Not deleted: {error}")),
                    }
                }))
                .child(icon_button(IconName::EllipsisCircle, "More theme actions…", strip, || {
                    spawn(async move { let _ = popup_menu(theme_action_items()).await; });
                }))
            .child(view().flex_col().gap_1()
                .child(radio("Use the chosen theme for all appearances", move || settings().with(|p| !p.themes.use_separate_color_themes), false, || {
                    write(|p| p.themes.use_separate_color_themes = false);
                }))
                .child(radio("Use different themes for light and dark appearances", move || settings().with(|p| p.themes.use_separate_color_themes), false, || {
                    write(|p| p.themes.use_separate_color_themes = true);
                }))
                .child(dynamic(move || {
                    let separate = settings().with(|p| p.themes.use_separate_color_themes);
                    untracked(|| if separate {
                        view().flex_col().gap_1().pl(px(20.0))
                            .child(theme_slot_row("Light:", false))
                            .child(theme_slot_row("Dark:", true))
                            .into_element()
                    } else {
                        view().into_element()
                    }).into_element()
                }))
                .child(check_box("Match window appearance to theme", |p| p.themes.match_appearance_to_theme, |p, v| p.themes.match_appearance_to_theme = v))))
        .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
            let name = global::<SelectedTheme>().with(|s| s.0.clone());
            global::<super::theme::ThemesRevision>().get();
            untracked(|| theme_editor(name)).into_element()
        }))))
}

/// Creates a user theme, applies it per the appearance radios, selects it.
fn new_theme(dark: bool) {
    let made = super::theme::mutate_library(|library| {
        let name = library.unique_name(if dark { "Untitled Dark" } else { "Untitled Light" });
        library.new_theme(&name, dark).map(|arc| arc.name.clone())
    });
    match made {
        Ok(name) => {
            apply_theme(&name);
            global::<SelectedTheme>().set(SelectedTheme(name));
        }
        Err(error) => state::notice(format!("Not created: {error}")),
    }
}

/// Puts `name` into the preference slot the appearance radios choose.
fn apply_theme(name: &str) {
    write(|p| {
        if p.themes.use_separate_color_themes {
            if dark() { p.themes.color_theme_dark = name.to_owned(); } else { p.themes.color_theme_light = name.to_owned(); }
        } else {
            p.themes.color_theme = name.to_owned();
        }
    });
}

/// A `Light:`/`Dark:` row of the separate-appearances choice (CodeRunner's
/// two popups, disabled unless the second radio is on — shown only then).
fn theme_slot_row(label: &'static str, dark_slot: bool) -> impl IntoElement {
    let current = settings().with_untracked(|p| if dark_slot { p.themes.color_theme_dark.clone() } else { p.themes.color_theme_light.clone() });
    let names = super::theme::with_library(|library| library.entries().into_iter().map(|e| e.name).collect::<Vec<_>>());
    let items = names.into_iter().map(|name| {
        let checked = name == current;
        MenuItem::new(&name).checked(checked).on_select(move || {
            write(|p| if dark_slot { p.themes.color_theme_dark = name.clone(); } else { p.themes.color_theme_light = name.clone(); });
        })
    }).collect();
    let title = if current.is_empty() { "None".to_owned() } else { current };
    view().flex_row().items_center().gap_2()
        .child(type_body().apply(text(label)))
        .child(popup_button("Choose the theme for this appearance", title, items, PopupButtonOptions::new(), |_open: bool| {}))
}

/// The theme list's gear menu (CodeRunner: Duplicate, Rename, Invert,
/// Delete…, Import…, Export…, Get More Themes, Reset…, Reset All…,
/// Show in Finder).
fn theme_action_items() -> Vec<MenuItem> {
    let name = global::<SelectedTheme>().with_untracked(|s| s.0.clone());
    let none = name.is_empty();
    let has_file = super::theme::with_library(|library| library.user_path(&name).is_some());
    vec![
        MenuItem::new("Duplicate").disabled(none).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            let copy = super::theme::mutate_library(|library| {
                library.duplicate(&name, &library.unique_name(&format!("{name} Copy"))).map(|arc| arc.name.clone())
            });
            match copy {
                Ok(name) => global::<SelectedTheme>().set(SelectedTheme(name)),
                Err(error) => state::notice(format!("Not duplicated: {error}")),
            }
        }),
        MenuItem::new("Rename…").disabled(none).on_select(|| global::<RenamingTheme>().set(RenamingTheme(true))),
        MenuItem::new("Invert").disabled(none).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            let made = super::theme::mutate_library(|library| {
                library.invert(&name, &library.unique_name(&format!("{name} Inverted"))).map(|arc| arc.name.clone())
            });
            match made {
                Ok(name) => global::<SelectedTheme>().set(SelectedTheme(name)),
                Err(error) => state::notice(format!("Not inverted: {error}")),
            }
        }),
        MenuItem::new("Delete…").disabled(none).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            if name.is_empty() { return; }
            spawn(async move {
                let response = nsh_base::alert_dialog()
                    .title(format!("Are you sure you want to delete the theme “{name}”?"))
                    .description("The theme will be moved to the Trash.")
                    .destructive("delete", "Delete")
                    .cancel("cancel", "Cancel")
                    .show().await;
                if let Ok(response) = response && response.action.as_deref() == Some("delete") {
                    let result = super::theme::mutate_library(|library| library.delete(&name));
                    match result {
                        Ok(()) => global::<SelectedTheme>().set(SelectedTheme(String::new())),
                        Err(error) => state::notice(format!("Not deleted: {error}")),
                    }
                }
            });
        }),
        MenuItem::separator(),
        MenuItem::new("Import…").on_select(|| {
            spawn(async move {
                let dialog = nsh_core::file_dialog::OpenDialog::files()
                    .title("Import a theme").button_label("Import")
                    .filter(nsh_core::file_dialog::FileFilter::new("Themes", ["tmTheme", "crTheme", "plist"]));
                if let Ok(Some(paths)) = pick_paths(dialog).await {
                    for path in paths {
                        match super::theme::mutate_library(|library| library.import(&path).map(|arc| arc.name.clone())) {
                            Ok(name) => global::<SelectedTheme>().set(SelectedTheme(name)),
                            Err(error) => state::notice(format!("Not imported: {error}")),
                        }
                    }
                }
            });
        }),
        MenuItem::new("Export…").disabled(none).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            if name.is_empty() { return; }
            spawn(async move {
                let dialog = nsh_core::file_dialog::SaveDialog::new()
                    .title("Export the theme").button_label("Export").file_name(format!("{name}.tmTheme"))
                    .filter(nsh_core::file_dialog::FileFilter::new("TextMate Theme", ["tmTheme"]));
                if let Ok(Some(path)) = pick_save_path(dialog).await {
                    match super::theme::with_library(|library| library.export(&name, &path)) {
                        Ok(path) => state::notice(format!("Exported to {}.", path.display())),
                        Err(error) => state::notice(format!("Not exported: {error}")),
                    }
                }
            });
        }),
        MenuItem::new("Get More Themes").on_select(|| {
            spawn(async move { let _ = open_url("https://google.com/search?q=textmate+themes").await; });
        }),
        MenuItem::separator(),
        MenuItem::new("Reset…").disabled(none || !has_file).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            let result = super::theme::mutate_library(|library| library.delete(&name));
            match result {
                Ok(()) => state::notice(format!("{name} was reset to the default.")),
                Err(error) => state::notice(format!("Not reset: {error}")),
            }
        }),
        MenuItem::new("Reset All…").on_select(|| {
            spawn(async move {
                let response = nsh_base::alert_dialog()
                    .title("Are you sure you want to reset all themes to the defaults?")
                    .description("This action cannot be undone.")
                    .destructive("reset", "Reset All")
                    .cancel("cancel", "Cancel")
                    .show().await;
                if let Ok(response) = response && response.action.as_deref() == Some("reset") {
                    let result = super::theme::mutate_library(|library| library.reset_all());
                    match result {
                        Ok(count) => state::notice(format!("{count} theme(s) reset.")),
                        Err(error) => state::notice(format!("Not reset: {error}")),
                    }
                }
            });
        }),
        MenuItem::new("Show in Finder").disabled(none || !has_file).on_select(|| {
            let name = global::<SelectedTheme>().get_untracked().0;
            let path = super::theme::with_library(|library| library.user_path(&name));
            match path {
                Some(path) => { let _ = std::process::Command::new("open").arg("-R").arg(path).spawn(); }
                None => state::notice("A built-in theme has no file to show.".to_owned()),
            }
        }),
    ]
}

/// The editor of the selected theme: the six global wells, the element
/// table and the scope row (DESIGN 8.13.3).
fn theme_editor(name: String) -> View {
    let theme = super::theme::with_library(|library| library.get(&name).map(|arc| (*arc).clone()));
    let Some(theme) = theme else {
        return view().p_4().child(type_body().apply(text("Select a theme.").text_color(tokens::muted_foreground)));
    };
    let separate = settings().with_untracked(|p| p.themes.use_separate_color_themes);
    let slot = if separate { if dark() { "Dark" } else { "Light" } } else { "all appearances" };
    let wells: Vec<AnyElement> = [
        ("Foreground", theme.global.foreground, well_set(&name, |g, c| g.foreground = Some(c))),
        ("Background", theme.global.background, well_set(&name, |g, c| g.background = Some(c))),
        ("Selection", theme.global.selection, well_set(&name, |g, c| g.selection = Some(c))),
        ("Current Line", theme.global.line_highlight, well_set(&name, |g, c| g.line_highlight = Some(c))),
        ("Cursor", theme.global.caret, well_set(&name, |g, c| g.caret = Some(c))),
        ("Invisibles", theme.global.invisibles, well_set(&name, |g, c| g.invisibles = Some(c))),
    ].into_iter().map(|(label, rgba, set)| {
        color_well(label, move || rgba.map(rgba_to_hsla), set, ColorWellOptions::new()).into_element()
    }).collect();
    let mut rows: Vec<AnyElement> = theme.items.iter().enumerate().map(|(index, item)| {
        let style = item.font_style.unwrap_or_default();
        let title = item.name.clone().unwrap_or_else(|| if item.scope.is_empty() { "Unnamed".to_owned() } else { item.scope.clone() });
        element_row(&name, index, title, item.foreground, item.background, style)
    }).collect();
    if global::<ShowAllThemeItems>().get_untracked().0 {
        for (title, scope) in DEFAULT_ELEMENTS {
            if !theme.items.iter().any(|item| item.name.as_deref() == Some(*title)) {
                rows.push(virtual_element_row(&name, title, scope));
            }
        }
    }
    let item_index = global::<SelectedThemeItem>().get_untracked().0;
    let scope = theme.items.get(item_index).map(|i| i.scope.clone()).unwrap_or_default();
    let scope_id = name.clone();
    let assign_id = name.clone();
    view().flex_1().min_h(px(0.0)).gap_3()
        .child(view().gap_2()
            .child(dynamic({
                let name = name.clone();
                move || {
                    if global::<RenamingTheme>().get().0 {
                        let value = name.clone();
                        untracked(|| view().flex_row().items_center().gap_2()
                            .child(type_body().apply(text("Name:")))
                            .child(view().w(px(220.0)).child(field("theme-rename".to_owned(), "Theme name", value,
                                TextFieldOptions::new().accessibility_label("Theme name"), move |next| {
                                    let next = next.trim().to_owned();
                                    global::<RenamingTheme>().set(RenamingTheme(false));
                                    if next.is_empty() { return; }
                                    let old = global::<SelectedTheme>().get_untracked().0;
                                    match super::theme::mutate_library(|library| library.rename(&old, &next).map(|arc| arc.name.clone())) {
                                        Ok(new_name) => global::<SelectedTheme>().set(SelectedTheme(new_name)),
                                        Err(error) => state::notice(format!("Not renamed: {error}")),
                                    }
                                }))).into_element()).into_element()
                    } else {
                        let name = name.clone();
                        untracked(|| section_header(name, SectionHeaderVariant::Form, SectionHeaderOptions::new()).into_element()).into_element()
                    }
                }
            }))
            .child(view().flex_row().items_center().justify_between().gap_4()
                .child(view().flex_row().gap_3().children(wells))
                .child(button(if separate { format!("Apply to {slot}") } else { "Apply".to_owned() }, ButtonOptions::new().size(ControlSize::Sm), move || {
                    apply_theme(&assign_id);
                }))))
        .child(view().flex_1().min_h(px(0.0)).rounded(px(tokens::radius_lg())).border_1().border_color(tokens::border).bg(tokens::card)
            .child(scroll().flex_1().min_h(px(0.0)).child(view().flex_col().p_1().children(rows))))
        .child(view().flex_row().items_center().gap_2()
            .child(icon_button(IconName::Plus, "Add a theme element", IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm), {
                let name = name.clone();
                move || mutate_theme(&name, |theme| theme.items.push(scratch_theme::ThemeItem::new(
                    "New Element", "comment", theme.global.foreground.unwrap_or_default(), None)))
            }))
            .child(type_caption().apply(text("Scope:")).child(field(format!("theme-scope:{name}"), "source.python comment", scope,
                TextFieldOptions::new().mono(true).accessibility_label("Scope selector"), move |next| {
                    let index = item_index;
                    mutate_theme(&scope_id, move |theme| if let Some(item) = theme.items.get_mut(index) { item.scope = next.trim().to_owned(); })
                })))
            .child(type_caption().apply(text("Use \u{2303}\u{2318}P in the editor to show the active scope").text_color(tokens::muted_foreground)))
            .child(view().flex_1())
            .child(checkbox("Show all", global::<ShowAllThemeItems>().get_untracked().0, CheckboxOptions::new(),
                |on| global::<ShowAllThemeItems>().set(ShowAllThemeItems(on)))))
}

/// A theme's `color_well` `on_change`, saving `rgba` into its global settings.
fn well_set(name: &str, apply: fn(&mut scratch_theme::GlobalSettings, scratch_theme::Rgba)) -> impl Fn(Hsla) + 'static {
    let name = name.to_owned();
    move |hsla| mutate_theme(&name, move |theme| apply(&mut theme.global, hsla_to_rgba(hsla)))
}

/// One element row: name, FG and BG wells, B / I / U toggles, a `radio` that
/// points the scope field at it and a delete button (DESIGN 8.13.3's table).
fn element_row(theme: &str, index: usize, title: String, foreground: Option<scratch_theme::Rgba>,
    background: Option<scratch_theme::Rgba>, style: scratch_theme::FontStyle) -> AnyElement {
    let small = ColorWellOptions::new().size(ColorWellSize::Chip);
    let selected = { computed(move || global::<SelectedThemeItem>().with(|s| s.0 == index)) };
    let (fg, bg, removed) = (theme.to_owned(), theme.to_owned(), theme.to_owned());
    let font = |flag: fn(&mut scratch_theme::FontStyle, bool)| {
        let theme = theme.to_owned();
        move |on: bool| {
        let theme = theme.clone();
        mutate_theme(&theme, move |t| if let Some(item) = t.items.get_mut(index) {
            let mut style = item.font_style.unwrap_or_default();
            flag(&mut style, on);
            item.font_style = if style == scratch_theme::FontStyle::PLAIN { None } else { Some(style) };
        })
        }
    };
    view().flex_row().items_center().gap_2().px_2().py_1()
        .bg(move || if selected.get() { tokens::muted() } else { tokens::card() }).rounded(px(tokens::radius()))
        .child(radio("", selected, false, move || global::<SelectedThemeItem>().set(SelectedThemeItem(index))))
        .child(view().w(px(150.0)).child(type_body().apply(text(title).number_of_lines(1)
            .text_color(foreground.map(|c| nsh_core::element::AdaptiveColor::from(rgba_to_hsla(c))).unwrap_or_else(tokens::foreground)))))
        .child(color_well("Element foreground", move || foreground.map(rgba_to_hsla), move |c| mutate_theme(&fg, move |t| if let Some(i) = t.items.get_mut(index) { i.foreground = Some(hsla_to_rgba(c)); }), small))
        .child(color_well("Element background", move || background.map(rgba_to_hsla), move |c| mutate_theme(&bg, move |t| if let Some(i) = t.items.get_mut(index) { i.background = Some(hsla_to_rgba(c)); }), small))
        .child(checkbox("B", style.bold, CheckboxOptions::new(), font(|s, on| s.bold = on)))
        .child(checkbox("I", style.italic, CheckboxOptions::new(), font(|s, on| s.italic = on)))
        .child(checkbox("U", style.underline, CheckboxOptions::new(), font(|s, on| s.underline = on)))
        .child(view().flex_1())
        .child(icon_button(IconName::Minus, "Delete the element", IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm),
            move || mutate_theme(&removed, move |t| { t.items.remove(index); })))
        .into_element()
}

/// A "Show all" row for a default element the theme does not define yet:
/// empty wells and toggles; the first edit creates the item (CodeRunner's
/// `Default.tmTheme`/`DefaultThemeItems.plist` rows).
fn virtual_element_row(theme: &str, title: &str, scope: &str) -> AnyElement {
    let small = ColorWellOptions::new().size(ColorWellSize::Chip);
    let well = |apply: fn(&mut scratch_theme::ThemeItem, scratch_theme::Rgba)| {
        let (theme, title, scope) = (theme.to_owned(), title.to_owned(), scope.to_owned());
        move |hsla: Hsla| mutate_item(&theme, &title, &scope, move |i| apply(i, hsla_to_rgba(hsla)))
    };
    let flag = |apply: fn(&mut scratch_theme::FontStyle)| {
        let (theme, title, scope) = (theme.to_owned(), title.to_owned(), scope.to_owned());
        move |_: bool| mutate_item(&theme, &title, &scope, move |i| {
            let mut style = i.font_style.unwrap_or_default();
            apply(&mut style);
            i.font_style = Some(style);
        })
    };
    view().flex_row().items_center().gap_2().px_2().py_1()
        .child(view().w(px(18.0))) // aligns with the selection radio of defined rows
        .child(view().w(px(150.0)).child(type_body().apply(text(title.to_owned()).number_of_lines(1)
            .text_color(tokens::muted_foreground))))
        .child(color_well("Element foreground", || None, well(|i, c| i.foreground = Some(c)), small))
        .child(color_well("Element background", || None, well(|i, c| i.background = Some(c)), small))
        .child(checkbox("B", false, CheckboxOptions::new(), flag(|s| s.bold = true)))
        .child(checkbox("I", false, CheckboxOptions::new(), flag(|s| s.italic = true)))
        .child(checkbox("U", false, CheckboxOptions::new(), flag(|s| s.underline = true)))
        .child(view().flex_1())
        .into_element()
}

/// Mutates the element named `element`, creating it first (with `scope`)
/// when the theme does not define it — the "Show all" rows' write path.
fn mutate_item(theme: &str, element: &str, scope: &str, apply: impl FnOnce(&mut scratch_theme::ThemeItem) + 'static) {
    let (element, scope) = (element.to_owned(), scope.to_owned());
    mutate_theme(theme, move |t| {
        if !t.items.iter().any(|i| i.name.as_deref() == Some(element.as_str())) {
            let mut item = scratch_theme::ThemeItem::new(&element, &scope,
                t.global.foreground.unwrap_or_default(), None);
            item.foreground = None;
            t.items.push(item);
        }
        if let Some(item) = t.items.iter_mut().find(|i| i.name.as_deref() == Some(element.as_str())) {
            apply(item);
        }
    });
}

/// Saves a mutation of the theme `name` back to the library.
fn mutate_theme(name: &str, f: impl FnOnce(&mut scratch_theme::Theme)) {
    let result = super::theme::mutate_library(|library| {
        let Some(arc) = library.get(name) else { return Err(format!("No theme `{name}`.")) };
        let mut theme = (*arc).clone();
        f(&mut theme);
        library.save(theme).map(|_| ()).map_err(|error| format!("Not saved: {error}"))
    });
    if let Err(error) = result { state::notice(error); }
}

fn rgba_to_hsla(c: scratch_theme::Rgba) -> Hsla {
    Hsla::from(nsh_core::Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: c.a as f32 / 255.0 })
}

fn hsla_to_rgba(c: Hsla) -> scratch_theme::Rgba {
    let rgb = nsh_core::Rgba::from(c);
    scratch_theme::Rgba { r: (rgb.r * 255.0).round() as u8, g: (rgb.g * 255.0).round() as u8, b: (rgb.b * 255.0).round() as u8, a: (rgb.a * 255.0).round() as u8 }
}

// --------------------------------------------------------------- advanced

fn advanced() -> Vec<AnyElement> {
    vec![
        section("Documents & Editor", vec![
            check("Use columns in selection label", |p| p.advanced.selection_label_uses_columns, |p, v| p.advanced.selection_label_uses_columns = v),
            check("Automatically resolve relative file URLs in webpages", |p| p.advanced.resolve_relative_file_urls, |p, v| p.advanced.resolve_relative_file_urls = v),
            check("Close new unedited document when opening another", |p| p.advanced.replace_untitled_document, |p, v| p.advanced.replace_untitled_document = v),
            check("Write compiler output files to temporary directory", |p| p.advanced.write_compiler_output_to_temp_dir, |p, v| p.advanced.write_compiler_output_to_temp_dir = v),
            check("Group similar languages in menus", |p| p.advanced.group_languages, |p, v| p.advanced.group_languages = v),
            check("Use custom file icons", |p| p.advanced.custom_icons_enabled, |p, v| p.advanced.custom_icons_enabled = v),
            check("Enable three-finger tap in editor", |p| p.advanced.three_finger_lookup_enabled, |p, v| p.advanced.three_finger_lookup_enabled = v),
        ]),
        section("Clang completion and diagnostics", vec![
            check("Automatically determine appropriate system compile flags", |p| p.advanced.clang.automatic_flags, |p, v| p.advanced.clang.automatic_flags = v),
            check("Apply user-defined compile flags", |p| p.advanced.clang.flags_from_user, |p, v| p.advanced.clang.flags_from_user = v),
            row("Extra Clang Flags:", view().w(px(220.0)).child(text_pref("Extra Clang flags", "none", TextFieldOptions::new().mono(true),
                |p| p.advanced.clang.extra_flags.clone(), |p, v| p.advanced.clang.extra_flags = v.trim().to_owned()))),
            row("Extra Swift Flags:", view().w(px(220.0)).child(text_pref("Extra Swift flags", "none", TextFieldOptions::new().mono(true),
                |p| p.advanced.extra_swift_flags.clone(), |p, v| p.advanced.extra_swift_flags = v.trim().to_owned()))),
        ]),
        section("Debugging", vec![
            check("Hide debugger termination message", |p| p.advanced.debugger_hide_termination_message, |p, v| p.advanced.debugger_hide_termination_message = v),
            check("Center current location during stepping", |p| p.advanced.debugging.center_during_stepping, |p, v| p.advanced.debugging.center_during_stepping = v),
        ]),
    ]
}

// --------------------------------------------------------------- keyboard

/// The pane's selected category (DESIGN 8.13.5: `Menu Items | Commands |
/// Compound Commands | Global Compound Commands`).
#[derive(Clone, Copy, Default, PartialEq)]
struct BindingsTab(usize);

/// The selected row's key: `bound:<id>` for a menu item, `bind:<identifier>`
/// for a set binding.
#[derive(Clone, Default, PartialEq)]
struct SelectedBinding(String);

/// The row whose recorder is armed (keyed like [`SelectedBinding`]).
#[derive(Clone, Default, PartialEq)]
struct Recording(String);

/// The bottom bar's filter text.
#[derive(Clone, Default, PartialEq)]
struct BindingFilter(String);

/// The set row shows its rename field.
#[derive(Clone, Default, PartialEq)]
struct RenamingSet(bool);

fn keyboard() -> impl IntoElement {
    let tab = global::<BindingsTab>();
    let revision = global::<super::bindings::BindingsRevision>();
    let rows = computed(move || {
        revision.get();
        let filter = global::<BindingFilter>().with(|f| f.0.to_lowercase());
        let tab = tab.get().0;
        binding_rows(tab, &filter)
    });
    view().flex_1().min_h(px(0.0)).flex_col().gap_2().px_4().pb_3().pt_3()
        // The set controls get their own row; the category segments sit
        // left-aligned under it, flush with the table's edge.
        .child(set_row())
        .child(segmented("Key bindings category",
            ["Menu Items", "Commands", "Compound Commands", "Global Compound Commands"].into_iter().map(Segment::new),
            move || tab.get().0,
            |index| global::<BindingsTab>().set(BindingsTab(index)),
            SegmentedOptions::new().size(ControlSize::Sm)))
        .child(view().flex_1().min_h(px(0.0)).flex_row().gap_3()
            .child(bindings_table(rows))
            .child(view().w(px(260.0)).min_h(px(0.0)).flex_col().pt_1().child(dynamic(move || {
                revision.get();
                binding_detail()
            }))))
        .child(bottom_bar())
}

/// The bindings master table: a `Command | Menu | Shortcut` header over
/// single-line rows (one `row_md` each — no two-line rows, no truncation
/// of the title into a second line; the menu/command name is its own
/// column).
fn bindings_table(rows: Memo<Vec<BindingRow>>) -> impl IntoElement {
    let second_head = move || match global::<BindingsTab>().get().0 { 0 => "Menu", 1 => "Command", _ => "" };
    view().flex_1().min_w(px(0.0)).min_h(px(0.0)).flex_col()
        .rounded(px(tokens::radius_lg())).border_1().border_color(tokens::border).bg(tokens::card)
        .child(view().flex_row().items_center().gap_2().h(px(26.0)).px_2().border_b_1().border_color(tokens::border)
            .child(view().w(px(20.0)))
            .child(view().flex_1().min_w(px(0.0)).child(type_caption().apply(text("Command").text_color(tokens::muted_foreground))))
            .child(view().w(px(84.0)).flex_shrink_0().child(type_caption().apply(text(second_head).text_color(tokens::muted_foreground))))
            .child(view().w(px(112.0)).flex_shrink_0().flex_row().justify_end().child(type_caption().apply(text("Shortcut").text_color(tokens::muted_foreground)))))
        .child(scroll().flex_1().min_h(px(0.0)).child(view().flex_col().p_1()
            .child(each(move || rows.get(), |row: &BindingRow| row.key.clone(), move |row, _| binding_row(row.get_untracked())))))
}

/// One row of the bindings table.
#[derive(Clone, PartialEq)]
struct BindingRow {
    /// `bound:<id>` or `bind:<identifier>`.
    key: String,
    /// The title shown.
    title: String,
    /// The group label of a menu-item row ("File"), or the command name of a
    /// command binding, or `""` for a compound row.
    subtitle: String,
    /// The row belongs to the global set.
    global: bool,
    /// The `COMMANDS` command of a menu-item row.
    bound: Option<&'static super::bindings::Bound>,
    /// The set binding of a command/compound row.
    binding: Option<scratch_commands::bindings::Binding>,
    /// The checkbox's state (effective-key semantics for menu rows).
    enabled: bool,
    /// The key's glyph text (`""` = unbound).
    keys: String,
    /// A binding overrides the default (a Reset button is shown).
    overridden: bool,
}

/// The rows of a category, filtered by the bottom bar's text.
fn binding_rows(tab: usize, filter: &str) -> Vec<BindingRow> {
    let matches = |title: &str, extra: &str| {
        filter.is_empty()
            || title.to_lowercase().contains(filter)
            || extra.to_lowercase().contains(filter)
    };
    let mut rows = Vec::new();
    match tab {
        0 => {
            let set = super::bindings::active_set();
            for bound in super::bindings::COMMANDS {
                if !matches(bound.title, bound.menu) { continue; }
                rows.push(BindingRow {
                    key: format!("bound:{}", bound.id),
                    title: bound.title.to_owned(),
                    subtitle: bound.menu.to_owned(),
                    global: false,
                    bound: Some(bound),
                    binding: None,
                    enabled: super::bindings::key_enabled(&set, bound),
                    keys: super::bindings::display_of(&set, bound),
                    overridden: super::bindings::binding_of(&set, bound).is_some(),
                });
            }
        }
        1 => {
            for b in super::bindings::active_set().bindings.iter().filter(|b| !b.is_compound()) {
                let command = b.command.clone().unwrap_or_default();
                let title = b.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| command.clone());
                if !matches(&title, &command) { continue; }
                let shortcut = b.shortcut().ok().flatten().map(|s| s.display()).unwrap_or_default();
                rows.push(BindingRow {
                    key: format!("bind:{}", b.identifier),
                    title,
                    subtitle: command,
                    global: false,
                    bound: None,
                    binding: Some(b.clone()),
                    enabled: b.enabled,
                    keys: shortcut,
                    overridden: false,
                });
            }
        }
        _ => {
            let (set, global) = if tab == 3 { (super::bindings::global_set(), true) } else { (super::bindings::active_set(), false) };
            for b in set.bindings.iter().filter(|b| b.is_compound()) {
                let title = b.display_name();
                if !matches(&title, "") { continue; }
                let shortcut = b.shortcut().ok().flatten().map(|s| s.display()).unwrap_or_default();
                let invalid = matches!(b.parse_compound(), Some(Err(_)));
                rows.push(BindingRow {
                    key: format!("bind:{}", b.identifier),
                    title: if invalid { format!("{title} ⚠") } else { title },
                    subtitle: String::new(),
                    global,
                    bound: None,
                    binding: Some(b.clone()),
                    enabled: b.enabled,
                    keys: shortcut,
                    overridden: false,
                });
            }
        }
    }
    rows
}

/// A table row: the enabled checkbox, the title, the reset ghost, and the
/// shortcut recorder capsule.
fn binding_row(row: BindingRow) -> AnyElement {
    let selected = { let key = row.key.clone(); computed(move || global::<SelectedBinding>().with(|s| s.0 == key)) };
    let armed = { let key = row.key.clone(); computed(move || global::<Recording>().with(|r| r.0 == key)) };
    let pick = {
        let key = row.key.clone();
        move || global::<SelectedBinding>().set(SelectedBinding(key.clone()))
    };
    let toggle = {
        let row = row.clone();
        move |on: bool| {
            if let Some(bound) = row.bound {
                super::bindings::set_key_enabled(bound, on);
            } else if let Some(b) = &row.binding {
                super::bindings::mutate_binding(&b.identifier, row.global, move |b| b.enabled = on);
            }
        }
    };
    let (title_color, subtitle) = (row.title.clone(), row.subtitle.clone());
    let capsule = view().w(px(112.0)).h(px(20.0)).px_2().flex_row().items_center().justify_center()
        .rounded(px(tokens::radius())).border_1()
        .border_color(move || if armed.get() { tokens::ring() } else { tokens::input() })
        .bg(move || if armed.get() { tokens::muted() } else { tokens::card() })
        .label(format!("Shortcut for {}", row.title))
        .child({
            let keys = row.keys.clone();
            text(move || if armed.get() { "Type shortcut…".to_owned() } else if keys.is_empty() { "None".to_owned() } else { keys.clone() })
                .text_size(px(tokens::type_caption()))
                .text_color(move || {
                    if armed.get() { tokens::selection_foreground() }
                    else if row.keys.is_empty() { tokens::muted_foreground() }
                    else { tokens::foreground() }
                })
        });
    let reset = {
        let bound = row.bound;
        move || if let Some(bound) = bound { super::bindings::reset(bound); }
    };
    view().flex_row().items_center().gap_2().px_2().h(px(tokens::row_md())).rounded(px(tokens::radius()))
        .bg(move || if selected.get() { tokens::muted() } else { tokens::card() })
        .hover(|s| s.bg(move || if selected.get() { tokens::muted() } else { tokens::control_hover() }))
        .on_press(pick)
        .child(checkbox("", row.enabled, CheckboxOptions::new(), toggle))
        .child(type_body().apply(text(title_color).number_of_lines(1).flex_1().min_w(px(0.0))))
        .child(view().w(px(84.0)).flex_shrink_0().child(type_caption().apply(text(subtitle).number_of_lines(1).text_color(tokens::muted_foreground))))
        .child(when(move || row.overridden, move || button("Reset", ButtonOptions::new().size(ControlSize::Sm), reset)))
        .child(capsule)
        .into_element()
}

/// The detail column's label width (the pane is ~260pt wide).
const DETAIL_LABEL_W: f32 = 70.0;

/// The right-hand detail: the compound-command JSON editor for compound
/// rows, a command-name field for command rows, a summary for menu-item
/// rows, else the empty state with the category's counts.
fn binding_detail() -> AnyElement {
    let key = global::<SelectedBinding>().with(|s| s.0.clone());
    if let Some(identifier) = key.strip_prefix("bind:") {
        let tab = global::<BindingsTab>().get_untracked().0;
        let global_row = tab == 3;
        let set = if global_row { super::bindings::global_set() } else { super::bindings::active_set() };
        if let Some(b) = set.bindings.iter().find(|b| b.identifier == identifier) {
            if b.is_compound() {
                return compound_editor(b.clone(), global_row);
            }
            return command_detail(b.clone());
        }
    }
    if let Some(bound_id) = key.strip_prefix("bound:")
        && let Some(bound) = super::bindings::COMMANDS.iter().find(|b| b.id == bound_id) {
            return bound_detail(bound);
        }
    bindings_empty_detail()
}

/// The detail of a menu-item binding: what it fires, its menu, its key and
/// a reset when the binding overrides the default.
fn bound_detail(bound: &'static super::bindings::Bound) -> AnyElement {
    let set = super::bindings::active_set();
    let keys = super::bindings::display_of(&set, bound);
    let overridden = super::bindings::binding_of(&set, bound).is_some();
    let enabled = super::bindings::key_enabled(&set, bound);
    let detail_row = |label: &'static str, control: AnyElement| form_row(label, control, FormRowOptions::new().label_width(DETAIL_LABEL_W)).into_element();
    view().gap_3().pt_1()
        .child(detail_row("Command:", type_body().apply(text(bound.title)).into_element()))
        .child(detail_row("Menu:", type_body().apply(text(bound.menu)).into_element()))
        .child(detail_row("Shortcut:", type_body().apply(
            text(if keys.is_empty() { "none".to_owned() } else { keys })).into_element()))
        .child(detail_row("", checkbox("Enabled", enabled, CheckboxOptions::new(), move |on| super::bindings::set_key_enabled(bound, on)).into_element()))
        .child(when(move || overridden, move || button("Restore Default", ButtonOptions::new().size(ControlSize::Sm),
            move || super::bindings::reset(bound))))
        .child(type_caption().apply(text("Click the shortcut cell in the table to record a new key.")
            .text_color(tokens::muted_foreground)))
        .into_element()
}

/// The detail column with nothing selected: the keyboard glyph, the
/// category's counts and how to record — not a bare placeholder.
fn bindings_empty_detail() -> AnyElement {
    let tab = global::<BindingsTab>().get_untracked().0;
    let all = binding_rows(tab, "");
    let total = all.len();
    let enabled = all.iter().filter(|row| row.enabled).count();
    let with_keys = all.iter().filter(|row| !row.keys.is_empty()).count();
    view().flex_1().min_h(px(0.0)).items_center().justify_center().gap_2().p_3()
        .child(glyph(IconName::Keyboard, IconSize::Xl, Tone::Muted))
        .child(type_body().apply(text("Select a binding to view or edit it.").text_color(tokens::foreground)))
        .child(type_caption().apply(text(format!("{total} bindings · {enabled} enabled · {with_keys} with shortcuts"))
            .text_color(tokens::muted_foreground)))
        .child(type_caption().apply(text("Click a shortcut cell to record a key; press ⌫ while recording to clear it.")
            .text_center().max_w(px(210.0)).text_color(tokens::muted_foreground)))
        .into_element()
}

/// The JSON editor of a compound-command binding (DESIGN 8.13.5's right pane),
/// with the invalid-JSON notice below it.
fn compound_editor(b: scratch_commands::bindings::Binding, global_row: bool) -> AnyElement {
    let identifier = b.identifier.clone();
    let code = b.compound_code.clone().unwrap_or_default();
    let invalid = b.parse_compound().and_then(|r| r.err().map(|e| e.to_string()));
    let (name_for_field, name_for_code) = (identifier.clone(), identifier.clone());
    view().flex_1().min_h(px(0.0)).flex_col().gap_2()
        .child(view().flex_row().items_center().gap_2()
            .child(type_body().apply(text("Name:")))
            .child(view().flex_1().child(field(format!("compound-name:{identifier}"), "Command name",
                b.title.clone().unwrap_or_default(), TextFieldOptions::new().accessibility_label("Compound command name"),
                move |next| super::bindings::mutate_binding(&name_for_field, global_row, move |b| b.title = Some(next)))))
        )
        .child(view().flex_1().min_h(px(0.0)).child(code_field(
            Some(SharedString::from(code)),
            move |next: &SharedString| super::bindings::mutate_binding(&name_for_code, global_row, move |b| b.compound_code = Some(next.to_string())),
            CodeFieldOptions::new("json").min_rows(8).max_rows(30).invalid(invalid.is_some()))))
        .child({
            let error = invalid.unwrap_or_default();
            when({
                let error = error.clone();
                move || !error.is_empty()
            }, move || type_caption().apply(text(error.clone()).text_color(tokens::destructive)))
        })
        .child(type_caption().apply(text("A JSON array of editor commands, or a key table mapping keys to arrays.")
            .text_color(tokens::muted_foreground)))
        .into_element()
}

/// The command name field of a command binding (the Commands category).
fn command_detail(b: scratch_commands::bindings::Binding) -> AnyElement {
    let identifier = b.identifier.clone();
    let (id_title, id_command) = (identifier.clone(), identifier.clone());
    view().gap_3()
        .child(row("Title:", field(format!("binding-title:{identifier}"), "Title",
            b.title.clone().unwrap_or_default(), TextFieldOptions::new().accessibility_label("Binding title"),
            move |next| super::bindings::mutate_binding(&id_title, false, move |b| b.title = Some(next)))))
        .child(row_with_caption("Command:", field(format!("binding-command:{identifier}"), "selectTab3",
            b.command.clone().unwrap_or_default(), TextFieldOptions::new().mono(true).accessibility_label("Command"),
            move |next| super::bindings::mutate_binding(&id_command, false, move |b| b.command = Some(next.trim().to_owned()))),
            "An editor command name (insertNewlineBelow, selectTab3) or a MainMenu/… menu path."))
        .into_element()
}

/// The pane's top row: the set popup and its action menu (DESIGN 8.13.5).
fn set_row() -> impl IntoElement {
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    view().flex_row().items_center().gap_2()
        .child(type_body().apply(text("Key Bindings Set:")))
        .child(view().w(px(160.0)).child(dynamic(|| {
            global::<super::bindings::BindingsRevision>().get();
            let items: Vec<SelectItem> = super::bindings::sets().iter().map(|s| SelectItem::new(s.title.clone(), s.title.clone())).collect();
            untracked(|| select_popup("Key bindings set", move || Some(SharedString::from(super::bindings::selected_set_name())), "Default",
                items, SelectPopupOptions::new(), |next| {
                    if let Some(name) = next { super::bindings::select_set(&name); }
                })).into_element()
        })))
        .child(when(move || global::<RenamingSet>().with(|r| r.0), || {
            let current = super::bindings::selected_set_name();
            field("set-rename".to_owned(), "Set name", current, TextFieldOptions::new().accessibility_label("Rename the set"),
                move |next| {
                    super::bindings::rename_set(&super::bindings::selected_set_name(), &next);
                    global::<RenamingSet>().set(RenamingSet(false));
                }).into_element()
        }))
        .child(view().flex_1())
        .child(icon_button(IconName::EllipsisCircle, "Key binding set actions…", strip, set_actions))
}

/// The `⋯` menu of the set row.
fn set_actions() {
    let name = super::bindings::selected_set_name();
    let read_only = super::bindings::sets().iter().find(|s| s.title == name).is_some_and(|s| s.read_only);
    let items = vec![
        MenuItem::new("Duplicate").on_select({
            let name = name.clone();
            move || { super::bindings::duplicate_set(&name); }
        }),
        MenuItem::new("Rename…").disabled(read_only).on_select(|| global::<RenamingSet>().set(RenamingSet(true))),
        MenuItem::new("Delete").disabled(read_only).on_select({
            let name = name.clone();
            move || super::bindings::delete_set(&name)
        }),
        MenuItem::separator(),
        MenuItem::new("Import…").on_select(|| {
            spawn(async move {
                let dialog = nsh_core::file_dialog::OpenDialog::files()
                    .filter(nsh_core::file_dialog::FileFilter::new("Key Bindings", ["crBindings", "json", "toml", "dict"]));
                if let Ok(Some(paths)) = pick_paths(dialog).await
                    && let Some(path) = paths.into_iter().next() { super::bindings::import(path); }
            });
        }),
        MenuItem::new("Export…").on_select({
            let name = name.clone();
            move || {
                let name = name.clone();
                spawn(async move {
                    let dialog = nsh_core::file_dialog::SaveDialog::new().file_name(format!("{name}.crBindings"));
                    if let Ok(Some(path)) = pick_save_path(dialog).await { super::bindings::export(&name, path); }
                });
            }
        }),
        MenuItem::separator(),
        MenuItem::new("Show in Finder").on_select(super::bindings::show_in_finder),
    ];
    spawn(async move { let _ = popup_menu(items).await; });
}

/// The bottom bar: the filter field, add/remove, the invalid-JSON notice and
/// the help button (DESIGN 8.13.5).
fn bottom_bar() -> impl IntoElement {
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    let invalid = computed(|| {
        global::<super::bindings::BindingsRevision>().get();
        let tab = global::<BindingsTab>().get_untracked().0;
        if tab < 2 { return false; }
        let set = if tab == 3 { super::bindings::global_set() } else { super::bindings::active_set() };
        set.bindings.iter().filter(|b| b.is_compound() && b.enabled).any(|b| matches!(b.parse_compound(), Some(Err(_))))
    });
    view().flex_row().items_center().gap_2()
        .child(view().w(px(200.0)).child(field("binding-filter".to_owned(), "Filter", String::new(),
            TextFieldOptions::new().accessibility_label("Filter bindings"),
            |next| global::<BindingFilter>().set(BindingFilter(next)))))
        .child(icon_button(IconName::Plus, "Add a binding", strip, || {
            let items = vec![
                MenuItem::new("New Command Binding").on_select(|| {
                    let id = super::bindings::add_command();
                    global::<BindingsTab>().set(BindingsTab(1));
                    global::<SelectedBinding>().set(SelectedBinding(format!("bind:{id}")));
                }),
                MenuItem::new("New Compound Command").on_select(|| {
                    let global_row = global::<BindingsTab>().get_untracked().0 == 3;
                    let id = super::bindings::add_compound(global_row);
                    if !global_row { global::<BindingsTab>().set(BindingsTab(2)); }
                    global::<SelectedBinding>().set(SelectedBinding(format!("bind:{id}")));
                }),
            ];
            spawn(async move { let _ = popup_menu(items).await; });
        }))
        .child(icon_button(IconName::Minus, "Delete the selected binding", strip, || {
            let key = global::<SelectedBinding>().get_untracked().0;
            if let Some(identifier) = key.strip_prefix("bind:") {
                let global_row = global::<BindingsTab>().get_untracked().0 == 3;
                super::bindings::delete_binding(identifier, global_row);
                global::<SelectedBinding>().set(SelectedBinding(String::new()));
            }
        }))
        .child(checkbox("Enable compound commands",
            settings().with_untracked(|p| p.key_bindings.compound_commands_enabled),
            CheckboxOptions::new(),
            |on| {
                write(|p| p.key_bindings.compound_commands_enabled = on);
                super::bindings::bump();
            }))
        .child(when(move || invalid.get(), || badge("Invalid JSON", BadgeOptions::new().variant(BadgeVariant::Danger))))
        .child(when(
            || global::<super::compound::KeyMapIssues>().with(|i| !i.0.is_empty()),
            || view().flex_1().min_w(px(0.0)).child(type_caption().apply(text(
                global::<super::compound::KeyMapIssues>().with(|i| i.0.first().cloned().unwrap_or_default()),
            ).number_of_lines(1).text_color(tokens::warning))),
        ))
        .child(view().flex_1())
        .child(icon_button(IconName::HelpCircle, "Key bindings help", strip, super::chrome::toggle_docs))
}

// -------------------------------------------------------------- languages

#[derive(Clone, PartialEq)]
struct LanguageRow { id: String, name: String, modified: bool, enabled: bool }

/// Bumped after the registry changes, so the list reads it again.
#[derive(Clone, Copy, Default, PartialEq)]
struct LanguagesRevision(u64);

/// Bumped by Reset, so the detail is made again with the restored values.
#[derive(Clone, Copy, Default, PartialEq)]
struct LanguageReset(u64);

#[derive(Clone, Default, PartialEq)]
struct SelectedLanguage(String);

/// Typed but not yet applied edits of the selected language.
#[derive(Clone, Default, PartialEq)]
struct LanguageDraft { id: String, command: Option<String>, extensions: Option<String> }

/// Why the last edit of a language was refused.
#[derive(Clone, Default, PartialEq)]
struct LanguageMessage(Option<String>);

/// "Rename…" was chosen for the selected language: the Settings tab shows
/// its name field until a name is committed (the Keyboard pane's pattern).
#[derive(Clone, Copy, Default, PartialEq)]
struct RenamingLanguage(bool);

fn language_rows() -> Option<Vec<LanguageRow>> {
    let mut rows = with(|workspace| workspace.registry.as_ref().map(|registry| registry.all().iter()
        .map(|def| LanguageRow { id: def.id.clone(), name: def.name.clone(), modified: registry.is_overridden(&def.id), enabled: def.enabled })
        .collect::<Vec<_>>()))?;
    rows.sort_by_key(|row| row.name.to_lowercase());
    Some(rows)
}

fn languages() -> impl IntoElement {
    let revision = global::<LanguagesRevision>();
    let selected = global::<SelectedLanguage>();
    let loading = computed(|| display().with(|view| view.loading));
    let rows = computed(move || { revision.get(); loading.get(); language_rows() });
    // Select the active document's language, or the first row once the registry
    // has loaded — the registry may still be loading at mount, so this runs in
    // an effect rather than once here.
    effect(move || {
        if !selected.with_untracked(|s| s.0.is_empty()) { return; }
        let active = with(|w| w.active.and_then(|id| w.doc(id)).map(|doc| doc.document.language.clone()));
        if let Some(id) = active.or_else(|| rows.get().and_then(|rows| rows.first().map(|row| row.id.clone()))) { selected.set(SelectedLanguage(id)); }
    });
    let muted = |words: &'static str| type_body().apply(text(words).p_3().text_color(tokens::muted_foreground));
    view().flex_1().min_h(px(0.0)).flex_row().gap_3().px_4().pb_4().pt_3()
        .child(view().w(px(200.0)).min_h(px(0.0)).flex_col().gap_2()
            .child(view().flex_1().min_h(px(0.0)).rounded(px(tokens::radius_lg())).border_1().border_color(tokens::border).bg(tokens::card)
            .child(when(move || rows.with(Option::is_none), move || muted("Loading languages…")))
            .child(when(move || rows.with(|rows| rows.as_ref().is_some_and(Vec::is_empty)), move || muted("No Items")))
            .child(scroll().flex_1().min_h(px(0.0)).child(view().label("Languages").child(
                tabs().flex_col().gap(px(1.0)).p_1()
                .indicator_style(|s| s.left(px(0.0)).right(px(0.0)).rounded(px(tokens::radius())).bg(tokens::selection))
                .child(each(move || rows.get().unwrap_or_default(), |row: &LanguageRow| row.id.clone(), move |row, _| {
                    let id = row.get_untracked().id;
                    let check_id = id.clone();
                    let is_selected = { let id = id.clone(); computed(move || selected.with(|s| s.0 == id)) };
                    list_tab(is_selected, move || {
                        flush();
                        global::<LanguageMessage>().set(LanguageMessage(None));
                        global::<RenamingLanguage>().set(RenamingLanguage(false));
                        selected.set(SelectedLanguage(id.clone()));
                    })
                    .child(type_body().apply(text(move || row.with(|r| r.name.clone())).number_of_lines(1).flex_1().min_w(px(0.0))
                        .text_color(move || if is_selected.get() { tokens::selection_foreground() } else { tokens::foreground() })))
                    .child(when(move || row.with(|r| r.modified), || badge("Modified", BadgeOptions::default())))
                    .child(checkbox("", row.get_untracked().enabled, CheckboxOptions::new(),
                        move |on| set_language_enabled(&check_id, on)))
                }))))))
            .child(language_buttons()))
        .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
            let id = selected.with(|s| s.0.clone());
            global::<LanguageReset>().get();
            loading.get();
            untracked(|| language_detail(id)).into_element()
        })))
}

/// The `+` / `−` / actions row under the language list (CodeRunner's
/// `addLanguage:`/`deleteLanguage:` plus its pull-down menu).
fn language_buttons() -> impl IntoElement {
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    view().flex_row().items_center().gap_2()
        .child(icon_button(IconName::Plus, "Add a language", strip, add_language))
        .child(icon_button(IconName::Minus, "Delete the selected language", strip, || {
            let id = global::<SelectedLanguage>().with_untracked(|s| s.0.clone());
            delete_language(&id);
        }))
        .child(icon_button(IconName::EllipsisCircle, "Language actions", strip, || {
            spawn(async move { let _ = popup_menu(language_action_items()).await; });
        }))
}

/// The actions pull-down of the language list (CodeRunner: Duplicate,
/// Rename, Delete…, Import…, Export…, Reset…, Reset All…, Show in Finder).
fn language_action_items() -> Vec<MenuItem> {
    let id = global::<SelectedLanguage>().with_untracked(|s| s.0.clone());
    let none = id.is_empty();
    let (builtin, overridden, is_default) = with(|w| w.registry.as_ref().map(|r|
        (r.is_builtin(&id), r.is_overridden(&id), r.default_language().id == id)).unwrap_or((false, false, false)));
    let act = |id: &str, f: fn(&str)| { let id = id.to_owned(); move || f(&id) };
    vec![
        MenuItem::new("Duplicate").disabled(none).on_select(act(&id, duplicate_language)),
        MenuItem::new("Rename…").disabled(none).on_select(|| {
            global::<LanguageSubTab>().set(LanguageSubTab(0));
            global::<RenamingLanguage>().set(RenamingLanguage(true));
        }),
        MenuItem::new("Delete…").disabled(none || builtin).on_select(act(&id, confirm_delete_language)),
        MenuItem::separator(),
        MenuItem::new("Import…").on_select(import_language),
        MenuItem::new("Export…").disabled(none).on_select(act(&id, export_language)),
        MenuItem::separator(),
        MenuItem::new("Reset…").disabled(none || !overridden).on_select(act(&id, confirm_reset_language)),
        MenuItem::new("Reset All…").on_select(reset_all_languages),
        MenuItem::new("Show in Finder").disabled(none).on_select(act(&id, show_language_in_finder)),
        MenuItem::separator(),
        MenuItem::new("Use Selected as Default").disabled(none || is_default).on_select(move || {
            match with_registry(|registry| registry.set_default_language(&id)) {
                Ok(Ok(())) => global::<LanguagesRevision>().update(|r| r.0 += 1),
                Ok(Err(error)) => state::notice(format!("{error}")),
                Err(error) => state::notice(error),
            }
        }),
    ]
}

/// Runs `f` on the registry, pointing it at the user's `Languages/` dir first.
fn with_registry<R>(f: impl FnOnce(&mut scratch_language::LanguageRegistry) -> R) -> Result<R, String> {
    with(|workspace| {
        let dir = workspace.paths.as_ref().map(Paths::languages);
        let registry = workspace.registry.as_mut().ok_or("The languages are still loading.")?;
        if registry.user_dir().is_none() && let Some(dir) = dir { registry.set_user_dir(dir); }
        Ok(f(registry))
    })
}

/// A registry call's `Ok(Err)` and registry-not-ready `Err` both become a
/// notice; `Ok(Ok)` bumps the list revision.
fn registry_done(result: Result<Result<(), scratch_language::LanguageError>, String>) {
    match result {
        Ok(Ok(())) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageReset>().update(|r| r.0 += 1);
        }
        Ok(Err(error)) => state::notice(format!("{error}")),
        Err(error) => state::notice(error),
    }
}

/// The Enabled checkbox of a list row (CodeRunner's `Enabled` column).
fn set_language_enabled(id: &str, enabled: bool) {
    registry_done(with_registry(|registry| registry.set_enabled(id, enabled)));
}

/// The registry's `save` of a new or imported def: bump + select it.
fn save_new_language(def: scratch_language::LanguageDef) {
    let id = def.id.clone();
    let name = def.name.clone();
    match with_registry(|registry| registry.save(def)) {
        Ok(Ok(())) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageReset>().update(|r| r.0 += 1);
            global::<SelectedLanguage>().set(SelectedLanguage(id));
            state::notice(format!("{name} was added."));
        }
        Ok(Err(error)) => state::notice(format!("Not saved: {error}")),
        Err(error) => state::notice(error),
    }
}

/// A display name no language has yet: `base`, `base 2`, `base 3`, …
fn unique_language_name(base: &str) -> String {
    with(|w| w.registry.as_ref().map(|registry| {
        let mut name = base.to_owned();
        let mut i = 1;
        while registry.all().iter().any(|d| d.name == name) { i += 1; name = format!("{base} {i}"); }
        name
    })).unwrap_or_else(|| base.to_owned())
}

/// `+`: a fresh language as CodeRunner seeds one (`Default.crLanguage`:
/// plain text, no run command, enabled).
fn add_language() {
    let name = unique_language_name("Untitled");
    save_new_language(scratch_language::LanguageDef::new(scratch_language::slugify(&name), name));
}

/// `−` / `Delete…`: confirms, then deletes (built-ins refuse in the registry).
fn confirm_delete_language(id: &str) {
    if id.is_empty() { return; }
    let name = language_def(id).map(|d| d.name).unwrap_or_else(|| id.to_owned());
    let id = id.to_owned();
    spawn(async move {
        let response = nsh_base::alert_dialog()
            .title(format!("Are you sure you want to delete the language “{name}”?"))
            .description("The language will be moved to the Trash.")
            .destructive("delete", "Delete")
            .cancel("cancel", "Cancel")
            .show().await;
        if let Ok(response) = response && response.action.as_deref() == Some("delete") {
            delete_language(&id);
        }
    });
}

fn delete_language(id: &str) {
    match with_registry(|registry| registry.delete(id)) {
        Ok(Ok(())) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageReset>().update(|r| r.0 += 1);
            global::<SelectedLanguage>().set(SelectedLanguage(String::new()));
        }
        Ok(Err(error)) => state::notice(format!("Not deleted: {error}")),
        Err(error) => state::notice(error),
    }
}

/// `Duplicate`: a copy named "<name> Copy" (numbered if taken), selected.
fn duplicate_language(id: &str) {
    let base = language_def(id).map(|d| format!("{} Copy", d.name)).unwrap_or_else(|| "Copy".to_owned());
    let name = unique_language_name(&base);
    match with_registry(|registry| registry.duplicate(id, &name)) {
        Ok(Ok(new_id)) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageReset>().update(|r| r.0 += 1);
            global::<SelectedLanguage>().set(SelectedLanguage(new_id));
        }
        Ok(Err(error)) => state::notice(format!("Not duplicated: {error}")),
        Err(error) => state::notice(error),
    }
}

/// `Rename…`'s field commits here: a new display name for `id`.
fn rename_language(id: &str, next: &str) {
    let next = next.trim();
    if next.is_empty() { return; }
    mutate_language(id, move |def| def.name = next.to_owned());
}

/// `Import…`: `.crLanguage` bundles / `Language.plist` / our `.toml` files.
fn import_language() {
    spawn(async move {
        let dialog = nsh_core::file_dialog::OpenDialog::files_and_folders()
            .title("Import a language")
            .button_label("Import")
            .filter(nsh_core::file_dialog::FileFilter::new("Languages", ["crLanguage", "plist", "toml"]));
        if let Ok(Some(paths)) = pick_paths(dialog).await {
            for path in paths { import_language_path(&path); }
        }
    });
}

/// Imports one path: `.toml` reads our format, anything else goes through
/// the `.crLanguage` importer. An id or name already taken gets a suffix.
fn import_language_path(path: &std::path::Path) {
    let outcome = if path.extension().is_some_and(|e| e == "toml") {
        std::fs::read_to_string(path).ok()
            .and_then(|text| scratch_language::LanguageDef::from_toml_str(&text).ok())
            .map(|def| (def, Vec::new()))
    } else {
        scratch_language::import_cr_language_path(path).ok().map(|i| (i.def, i.warnings))
    };
    let Some((mut def, warnings)) = outcome else {
        state::notice(format!("Not imported: {} is not a language file.", path.display()));
        return;
    };
    let result = with_registry(|registry| {
        while registry.get(&def.id).is_some() { def.id = format!("{}-imported", def.id); }
        if registry.by_name(&def.name).is_some() { def.name = unique_language_name(&def.name); }
        registry.save(def.clone()).map(|()| def.name.clone())
    });
    match result {
        Ok(Ok(name)) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageReset>().update(|r| r.0 += 1);
            for warning in warnings { state::notice(format!("{name}: {warning}")); }
            global::<SelectedLanguage>().set(SelectedLanguage(String::new()));
        }
        Ok(Err(error)) => state::notice(format!("Not imported: {error}")),
        Err(error) => state::notice(error),
    }
}

/// `Export…`: writes the language's `.toml` (the format `Languages/` loads;
/// CodeRunner exports `.crLanguage`, listed as an intentional difference).
fn export_language(id: &str) {
    let Some(def) = language_def(id) else { return; };
    let text = match def.to_toml_string() {
        Ok(text) => text,
        Err(error) => { state::notice(format!("Not exported: {error}")); return; }
    };
    let file_name = format!("{}.toml", def.id);
    spawn(async move {
        let dialog = nsh_core::file_dialog::SaveDialog::new()
            .title("Export the language").button_label("Export").file_name(file_name)
            .filter(nsh_core::file_dialog::FileFilter::new("ScratchRunner Language", ["toml"]));
        if let Ok(Some(path)) = pick_save_path(dialog).await
            && let Err(error) = std::fs::write(&path, text) {
                state::notice(format!("Not exported: {error}"));
            }
    });
}

/// `Reset…`: confirms, then hands back to the bundled defaults.
fn confirm_reset_language(id: &str) {
    let name = language_def(id).map(|d| d.name).unwrap_or_else(|| id.to_owned());
    let id = id.to_owned();
    spawn(async move {
        let response = nsh_base::alert_dialog()
            .title(format!("Are you sure you want to reset the language “{name}” to the default?"))
            .description("The settings for this language will be reset to the default.")
            .destructive("reset", "Reset")
            .cancel("cancel", "Cancel")
            .show().await;
        if let Ok(response) = response && response.action.as_deref() == Some("reset") {
            reset_language(&id);
        }
    });
}

/// `Reset All…`: confirms, then drops every user language file.
fn reset_all_languages() {
    spawn(async move {
        let response = nsh_base::alert_dialog()
            .title("Are you sure you want to reset all languages to the defaults?")
            .description("This action cannot be undone.")
            .destructive("reset", "Reset All")
            .cancel("cancel", "Cancel")
            .show().await;
        if let Ok(response) = response && response.action.as_deref() == Some("reset") {
            registry_done(with_registry(|registry| registry.reset_all()));
        }
    });
}

/// `Show in Finder`: the language's user file; a bundled, unmodified
/// language has none.
fn show_language_in_finder(id: &str) {
    let path = with(|w| w.registry.as_ref()
        .and_then(|r| r.user_dir().map(|d| d.join(format!("{id}.toml")))));
    match path {
        Some(path) if path.exists() => { let _ = std::process::Command::new("open").arg("-R").arg(path).spawn(); }
        _ => state::notice("A built-in language with no changes has no file to show.".to_owned()),
    }
}

/// The selected sub-tab of a language's detail (DESIGN 8.13.2:
/// Settings | Templates | Docs | Diagnostics).
#[derive(Clone, Copy, Default, PartialEq)]
struct LanguageSubTab(usize);

/// The template whose body the editor below the list edits.
#[derive(Clone, Copy, Default, PartialEq)]
struct SelectedTemplate(usize);

fn language_detail(id: String) -> View {
    let known = with(|workspace| workspace.registry.as_ref().is_some_and(|registry| registry.get(&id).is_some()));
    if !known {
        return view().p_4().child(type_body().apply(text("Select a language.").text_color(tokens::muted_foreground)));
    }
    let tab = global::<LanguageSubTab>();
    let detail = {
        let id = id.clone();
        dynamic(move || {
            global::<LanguagesRevision>().get();
            global::<LanguageReset>().get();
            match tab.get().0 {
                1 => language_templates(&id),
                2 => language_docs(&id),
                3 => language_diagnostics(&id),
                _ => language_settings(&id),
            }
        })
    };
    view().flex_1().min_h(px(0.0)).gap_3()
        .child(view().flex_row().justify_center()
            .child(segmented("Language detail", ["Settings", "Templates", "Docs", "Diagnostics"].into_iter().map(Segment::new),
                move || tab.get().0,
                |index| global::<LanguageSubTab>().set(LanguageSubTab(index)),
                SegmentedOptions::new())))
        .child(scroll().flex_1().min_h(px(0.0)).child(view().pb_2().child(detail)))
}

fn select_language_message() -> AnyElement {
    view().p_4().child(type_body().apply(text("Select a language.").text_color(tokens::muted_foreground))).into_element()
}

/// The current definition of a language (call inside a `LanguagesRevision`
/// subscription).
fn language_def(id: &str) -> Option<scratch_language::LanguageDef> {
    with(|workspace| workspace.registry.as_ref().and_then(|registry| registry.get(id).cloned()))
}

/// Mutates a language and saves it back to the registry.
fn mutate_language(id: &str, f: impl FnOnce(&mut scratch_language::LanguageDef)) {
    let result = with(|workspace| {
        let dir = workspace.paths.as_ref().map(Paths::languages);
        let registry = workspace.registry.as_mut().ok_or("The languages are still loading.")?;
        if registry.user_dir().is_none() && let Some(dir) = dir { registry.set_user_dir(dir); }
        let mut def = registry.get(id).ok_or_else(|| format!("No language `{id}`."))?.clone();
        let before = def.clone();
        f(&mut def);
        if def == before { return Ok(false); }
        registry.save(def).map(|()| true).map_err(|error| format!("Not saved: {error}"))
    });
    match result {
        Ok(true) => {
            global::<LanguagesRevision>().update(|r| r.0 += 1);
            global::<LanguageMessage>().set(LanguageMessage(None));
        }
        Ok(false) => {}
        Err(error) => global::<LanguageMessage>().set(LanguageMessage(Some(error))),
    }
}

fn language_settings(id: &str) -> AnyElement {
    let found = with(|workspace| workspace.registry.as_ref().and_then(|registry| registry.get(id).map(|def| (def.clone(), registry.pristine(id).cloned()))));
    let Some((def, pristine)) = found else { return select_language_message() };
    let revision = global::<LanguagesRevision>();
    let overridden = { let id = id.to_owned(); computed(move || { revision.get(); with(|w| w.registry.as_ref().is_some_and(|r| r.is_overridden(&id))) }) };
    let edit = |id: String, apply: fn(&mut LanguageDraft, String)| move |value: String| {
        global::<LanguageDraft>().update(|d| { if d.id != id { *d = LanguageDraft { id: id.clone(), ..LanguageDraft::default() }; } apply(d, value); });
        commit_language_draft();
    };
    let default_command = pristine.as_ref().filter(|p| p.run.command != def.run.command)
        .map(|p| format!("Default: {}", if p.run.command.is_empty() { "none" } else { &p.run.command }));
    let compile = if def.has_compile_step() { "Yes: a compile script runs first; its last output line is $compiler." } else { "No" };
    let command_caption = SharedString::from(default_command.map_or_else(
        || "$filename is the source file; $compiler is the compile script's output.".to_owned(),
        |default| format!("$filename is the source file; $compiler is the compile script's output. {default}.")));
    let reset_id = id.to_owned();
    let debugger_label = debugger_name(def.debugger.as_deref()).to_owned();
    let rows: Vec<AnyElement> = vec![
        when(move || global::<RenamingLanguage>().get().0, {
            let (id, name) = (id.to_owned(), def.name.clone());
            move || form_row("Name:", view().w(px(220.0)).child(field(format!("lang-rename:{id}"), "Name", name.clone(),
                TextFieldOptions::new().accessibility_label("Language name"), {
                    let id = id.clone();
                    move |next| { rename_language(&id, &next); global::<RenamingLanguage>().set(RenamingLanguage(false)); }
                })),
                FormRowOptions::new().label_width(120.0)).into_element()
        }).into_element(),
        form_row("Compile Script:", type_body().apply(text(compile)), FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("Compile Flags:", view().w(px(220.0)).child(field(format!("lang-flags:{id}"), "-Wall -O2", def.run.flags.clone(),
            TextFieldOptions::new().mono(true).accessibility_label("Compile flags"), {
                let id = id.to_owned();
                move |next| mutate_language(&id, move |d| d.run.flags = next.trim().to_owned())
            })),
            FormRowOptions::new().label_width(120.0).caption("Passed to the compile script; used only when one runs.")).into_element(),
        form_row("Run Command:", field(format!("lang-command:{id}"), "$filename", def.run.command.clone(),
            TextFieldOptions::new().mono(true).accessibility_label("Run command"), edit(id.to_owned(), |d, v| d.command = Some(v))),
            FormRowOptions::new().label_width(120.0).caption(command_caption)).into_element(),
        form_row("I/O Encoding:", view().w(px(180.0)).child(popup_button("Choose the console I/O encoding", encoding_title(def.output_encoding),
            encoding_items(id, def.output_encoding), PopupButtonOptions::new(), |_open: bool| {})),
            FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("File Extensions:", view().w(px(220.0)).child(field(format!("lang-extensions:{id}"), "h, m", def.extensions.join(", "),
            TextFieldOptions::new().accessibility_label("File extensions"), edit(id.to_owned(), |d, v| d.extensions = Some(v)))),
            FormRowOptions::new().label_width(120.0).caption("Comma separated; the first is the default for new files.")).into_element(),
        form_row("Syntax Mode:", view().w(px(180.0)).child(popup_button("Choose the syntax mode", def.editor_syntax.clone(),
            syntax_items(id, &def.editor_syntax), PopupButtonOptions::new(), |_open: bool| {})),
            FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("Console:", view().w(px(180.0)).child(popup_button("Choose the console type", def.console.kind.label(),
            console_items(id, &def.console), PopupButtonOptions::new(), |_open: bool| {})),
            FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("Debugger:", view().w(px(180.0)).child(popup_button("Choose the debugger for this language", debugger_label,
            debugger_items(id, def.debugger.clone()), PopupButtonOptions::new(), |_open: bool| {})),
            FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("Indentation:", view().w(px(180.0)).child(popup_button("Choose the indentation for this language", indent_title(&def),
            indent_items(id, &def), PopupButtonOptions::new(), |_open: bool| {})),
            FormRowOptions::new().label_width(120.0)).into_element(),
        form_row("", type_body().apply(text(move || global::<LanguageMessage>().with(|m| m.0.clone().unwrap_or_default())).text_color(tokens::destructive)),
            FormRowOptions::new().label_width(120.0)).into_element(),
        // The kit's button takes `disabled` once: made again when it flips.
        form_row("", dynamic(move || {
            let enabled = overridden.get();
            let id = reset_id.clone();
            untracked(|| button("Reset to Default", ButtonOptions::new().size(ControlSize::Sm).disabled(!enabled), move || reset_language(&id))).into_element()
        }), FormRowOptions::new().label_width(120.0)).into_element(),
    ];
    view().gap_2()
        .child(section_header(def.name.clone(), SectionHeaderVariant::Form, SectionHeaderOptions::new()))
        .child(card(rows))
        .into_element()
}

/// An adapter id's menu label (None reads "None").
fn debugger_name(id: Option<&str>) -> &'static str {
    match id {
        None => "None",
        Some("lldb") => "LLDB",
        Some("pdb") => "Python (pdb)",
        Some("node") => "Node.js (inspect)",
        Some("jdb") => "Java (jdb)",
        Some("bashdb") => "Bash (bashdb)",
        Some("perl") => "Perl (perl -d)",
        Some("phpdbg") => "PHP (phpdbg)",
        Some(_) => "Custom",
    }
}

/// The Debugger popup's items: None plus the built-in adapters.
fn debugger_items(id: &str, current: Option<String>) -> Vec<MenuItem> {
    const ADAPTERS: [&str; 7] = ["lldb", "pdb", "node", "jdb", "bashdb", "perl", "phpdbg"];
    let mut items = vec![MenuItem::new("None").checked(current.is_none()).on_select({
        let id = id.to_owned();
        move || mutate_language(&id, |def| def.debugger = None)
    })];
    for adapter in ADAPTERS {
        items.push(MenuItem::new(debugger_name(Some(adapter))).checked(current.as_deref() == Some(adapter)).on_select({
            let id = id.to_owned();
            move || mutate_language(&id, move |def| def.debugger = Some(adapter.to_owned()))
        }));
    }
    items
}

/// The "I/O Encoding" popup's title ("Automatic (UTF-8)" for code 0).
fn encoding_title(code: i64) -> String {
    if code == 0 { "Automatic (UTF-8)".to_owned() } else { scratch_language::encoding_name(code).to_owned() }
}

/// The "I/O Encoding" popup (CodeRunner lists every NSStringEncoding; the
/// common ones cover the field's purpose).
fn encoding_items(id: &str, current: i64) -> Vec<MenuItem> {
    let set = |code: i64| { let id = id.to_owned(); move || mutate_language(&id, move |d| d.output_encoding = code) };
    let mut items = vec![MenuItem::new("Automatic (UTF-8)").checked(current == 0).on_select(set(0))];
    for (name, code) in [("UTF-8", 4), ("US-ASCII", 1), ("UTF-16", 10), ("ISO-8859-1", 5), ("Windows-1252", 12), ("Macintosh", 30)] {
        items.push(MenuItem::new(name).checked(current == code).on_select(set(code)));
    }
    items
}

/// The "Syntax Mode" popup: one item per editor grammar any language uses
/// (CodeRunner lists the imported grammars; ours are registered with the
/// languages themselves), labelled by that language's name.
fn syntax_items(id: &str, current: &str) -> Vec<MenuItem> {
    let mut grammars: Vec<(String, String)> = with(|w| w.registry.as_ref().map(|registry| {
        let mut seen = std::collections::BTreeMap::new();
        for def in registry.all() {
            seen.entry(def.editor_syntax.clone()).or_insert_with(|| def.name.clone());
        }
        seen.into_iter().collect()
    })).unwrap_or_default();
    grammars.sort_by_key(|a| a.1.to_lowercase());
    grammars.into_iter().map(|(syntax, name)| {
        let id = id.to_owned();
        MenuItem::new(if syntax == "text.plain" { "Plain Text".to_owned() } else { name })
            .checked(syntax == current)
            .on_select(move || mutate_language(&id, {
                let syntax = syntax.clone();
                move |d| d.editor_syntax = syntax
            }))
    }).collect::<Vec<_>>()
}

/// The "Console" popup: the four console types, then the position override
/// and the switchable/live flags, as CodeRunner's one menu does.
fn console_items(id: &str, spec: &scratch_language::ConsoleSpec) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = [
        (scratch_language::ConsoleType::Terminal, "Terminal"),
        (scratch_language::ConsoleType::Web, "Web"),
        (scratch_language::ConsoleType::WebResource, "Web Resource"),
        (scratch_language::ConsoleType::Pdf, "PDF"),
    ].into_iter().map(|(kind, label)| {
        let id = id.to_owned();
        MenuItem::new(label).checked(spec.kind == kind)
            .on_select(move || mutate_language(&id, move |d| d.console.kind = kind))
    }).collect();
    items.push(MenuItem::separator());
    items.push(MenuItem::new("Same as Default").checked(spec.position.is_none()).on_select({
        let id = id.to_owned();
        move || mutate_language(&id, move |d| d.console.position = None)
    }));
    for (position, label) in [(scratch_language::ConsolePosition::Bottom, "Console Position: Bottom"),
        (scratch_language::ConsolePosition::Right, "Console Position: Right")] {
        let id = id.to_owned();
        items.push(MenuItem::new(label).checked(spec.position == Some(position))
            .on_select(move || mutate_language(&id, move |d| d.console.position = Some(position))));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new("Allow Toggling of Console Types").checked(spec.switchable).on_select({
        let id = id.to_owned();
        let on = !spec.switchable;
        move || mutate_language(&id, move |d| d.console.switchable = on)
    }));
    items.push(MenuItem::new("Live Updating").checked(spec.live).on_select({
        let id = id.to_owned();
        let on = !spec.live;
        move || mutate_language(&id, move |d| d.console.live = on)
    }));
    items
}

/// The "Indentation" popup's title ("Default", "Tabs, N wide", "N spaces").
fn indent_title(def: &scratch_language::LanguageDef) -> String {
    if !def.custom_tab_mode { "Default".to_owned() }
    else if def.soft_tabs { format!("{} space{}", def.tab_width, if def.tab_width == 1 { "" } else { "s" }) }
    else { format!("Tabs, {} wide", def.tab_width) }
}

/// The "Indentation" popup: Default, then Tabs and Spaces at each of
/// CodeRunner's widths (2, 3, 4, 6, 8).
fn indent_items(id: &str, def: &scratch_language::LanguageDef) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::new("Default").checked(!def.custom_tab_mode).on_select({
        let id = id.to_owned();
        move || mutate_language(&id, move |d| d.custom_tab_mode = false)
    })];
    for soft in [false, true] {
        items.push(MenuItem::separator());
        for width in [2u8, 3, 4, 6, 8] {
            let checked = def.custom_tab_mode && def.soft_tabs == soft && def.tab_width == width;
            let label = if soft { format!("{width} spaces") } else { format!("Tabs, {width} wide") };
            let id = id.to_owned();
            items.push(MenuItem::new(label).checked(checked).on_select(
                move || mutate_language(&id, move |d| { d.custom_tab_mode = true; d.soft_tabs = soft; d.tab_width = width; })));
        }
    }
    items
}

// ------------------------------------------------- languages / templates

fn language_templates(id: &str) -> AnyElement {
    let Some(def) = language_def(id) else { return select_language_message() };
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    let selected = global::<SelectedTemplate>();
    let count = def.templates.len();
    if count > 0 && selected.with_untracked(|s| s.0 >= count) { selected.set(SelectedTemplate(count - 1)); }
    let rows: Vec<AnyElement> = def.templates.iter().enumerate().map(|(index, template)| {
        let (rename, rextend, removed, toggle_default, toggle_ask) =
            (id.to_owned(), id.to_owned(), id.to_owned(), id.to_owned(), id.to_owned());
        let is_selected = { computed(move || selected.with(|s| s.0 == index)) };
        let row = view().flex_col().gap_1()
            .child(view().flex_row().items_center().gap_2()
                .child(radio("", is_selected, false, move || {
                    global::<SelectedTemplate>().set(SelectedTemplate(index));
                }))
                .child(view().flex_1().min_w(px(0.0)).child(field(format!("tpl-name:{id}:{index}"), "Name", template.name.clone(),
                    TextFieldOptions::new().accessibility_label("Template name"), move |next| {
                        mutate_language(&rename, move |def| if let Some(t) = def.templates.get_mut(index) { t.name = next.trim().to_owned(); })
                    })))
                .child(view().w(px(56.0)).child(field(format!("tpl-ext:{id}:{index}"), "ext", template.extension.clone(),
                    TextFieldOptions::new().accessibility_label("Template extension"), move |next| {
                        mutate_language(&rextend, move |def| if let Some(t) = def.templates.get_mut(index) { t.extension = next.trim().trim_start_matches('.').to_owned(); })
                    })))
                .child(icon_button(IconName::Minus, "Delete the template", strip, move || {
                    mutate_language(&removed, move |def| { def.templates.remove(index); })
                })))
            .child(view().flex_row().items_center().gap_3().pl(px(24.0))
                .child(checkbox("Default", template.default, CheckboxOptions::new(), move |on| {
                    mutate_language(&toggle_default, move |def| {
                        for (i, t) in def.templates.iter_mut().enumerate() { t.default = on && i == index; }
                    })
                }))
                .child(checkbox("Ask for name", template.name_on_creation, CheckboxOptions::new(), move |on| {
                    mutate_language(&toggle_ask, move |def| if let Some(t) = def.templates.get_mut(index) { t.name_on_creation = on; })
                })));
        row.bg(move || if is_selected.get() { tokens::muted() } else { tokens::card() })
            .rounded(px(tokens::radius())).px_2().py_1()
            .into_element()
    }).collect();
    let body = def.templates.get(selected.get_untracked().0.min(count.saturating_sub(1))).map(|t| t.text.clone());
    let body_lang = def.grammar.clone();
    let body_id = id.to_owned();
    let body_index = selected.get_untracked().0.min(count.saturating_sub(1));
    view().gap_2()
        .child(view().flex_row().items_center().justify_between()
            .child(view().flex_row().items_center().gap_2()
                .child(icon_button(IconName::Plus, "Create a new template", strip, {
                    let id = id.to_owned();
                    move || mutate_language(&id, |def| def.templates.push(scratch_language::TemplateDef {
                        name: "New Template".to_owned(), extension: def.extensions.first().cloned().unwrap_or_default(),
                        text: String::new(), default: false, name_on_creation: false,
                    }))
                }))
                .child(type_caption().apply(text(format!("{} template(s)", count)).text_color(tokens::muted_foreground)))))
        .child(type_caption().apply(text("Use <##> to represent text insertion point.").text_color(tokens::muted_foreground)))
        .child(card(rows))
        .child(when(count == 0, || type_body().apply(text("No templates.").p_3().text_color(tokens::muted_foreground))))
        .child(when(count > 0, move || {
            let text = body.clone().unwrap_or_default();
            let lang = body_lang.clone();
            let id = body_id.clone();
            form_row("Template:", view().flex_1().child(code_field(
                move || Some(SharedString::from(text.clone())),
                move |next: &SharedString| mutate_language(&id, move |def| if let Some(t) = def.templates.get_mut(body_index) { t.text = next.to_string(); }),
                CodeFieldOptions::new(if lang.is_empty() { "plain".to_owned() } else { lang.clone() }).min_rows(6).line_numbers(true))),
                FormRowOptions::new().label_width(120.0)).into_element()
        }))
        .into_element()
}

// ----------------------------------------------------- languages / docs

/// The CSS/JS editor sheet of a Docs row: `(language, doc index, field)`.
#[derive(Clone, Default, PartialEq)]
struct DocEditSheet(Option<(String, usize, &'static str)>);

fn language_docs(id: &str) -> AnyElement {
    let Some(def) = language_def(id) else { return select_language_message() };
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    let count = def.docs.len();
    let rows: Vec<AnyElement> = def.docs.iter().enumerate().map(|(index, source)| {
        let lang = id.to_owned();
        let kind = source.kind;
        let header = view().flex_row().items_center().gap_2()
            .child(checkbox("", source.enabled, CheckboxOptions::new(), {
                let lang = lang.clone();
                move |on| mutate_language(&lang, move |def| if let Some(d) = def.docs.get_mut(index) { d.enabled = on; })
            }))
            .child(view().w(px(130.0)).child(field(format!("doc-name:{lang}:{index}"), "Name", source.name.clone(),
                TextFieldOptions::new().accessibility_label("Documentation name"), {
                    let lang = lang.clone();
                    move |next| mutate_language(&lang, move |def| if let Some(d) = def.docs.get_mut(index) { d.name = next.trim().to_owned(); })
                })))
            .child(view().w(px(110.0)).child(popup_button("Documentation type", doc_type_name(kind), doc_type_items(&lang, index, kind),
                PopupButtonOptions::new(), |_open: bool| {})))
            .child(view().flex_1().min_w(px(0.0)))
            .child(button("CSS", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), {
                let lang = lang.clone();
                move || global::<DocEditSheet>().set(DocEditSheet(Some((lang.clone(), index, "css"))))
            }))
            .child(button("JS", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), {
                let lang = lang.clone();
                move || global::<DocEditSheet>().set(DocEditSheet(Some((lang.clone(), index, "js"))))
            }))
            .child(icon_button(IconName::Minus, "Delete the documentation entry", strip, move || {
                let lang = lang.clone();
                spawn(async move {
                    let response = nsh_base::alert_dialog()
                        .title("Are you sure you want to delete the documentation entry?")
                        .destructive("delete", "Delete")
                        .cancel("cancel", "Cancel")
                        .show().await;
                    if let Ok(response) = response && response.action.as_deref() == Some("delete") {
                        mutate_language(&lang, move |def| { def.docs.remove(index); });
                    }
                });
            }));
        view().flex_col().gap_1().px_2().py_1()
            .child(header)
            .child(field(format!("doc-url:{id}:{index}"), "https://… (use {query})", source.url.clone(),
                TextFieldOptions::new().mono(true).accessibility_label("Documentation URL"), {
                    let lang = id.to_owned();
                    move |next| mutate_language(&lang, move |def| if let Some(d) = def.docs.get_mut(index) { d.url = next.trim().to_owned(); })
                }))
            .child(field(format!("doc-post:{id}:{index}"), "POST body (empty = GET)", source.post.clone().unwrap_or_default(),
                TextFieldOptions::new().mono(true).accessibility_label("Documentation POST body"), {
                    let lang = id.to_owned();
                    move |next| mutate_language(&lang, move |def| if let Some(d) = def.docs.get_mut(index) {
                        d.post = if next.trim().is_empty() { None } else { Some(next) };
                    })
                }))
            .into_element()
    }).collect();

    // The global-docs section, rebuilt when `DocsRevision` bumps.
    let global_section = dynamic(move || {
        global::<DocsRevision>().get();
        let global_rows: Vec<AnyElement> = settings().with_untracked(|p| p.languages.docs.global_entries.clone())
            .iter().enumerate().map(|(index, source)| global_doc_row(index, source, strip)).collect();
        view().gap_2()
            .child(type_caption().apply(text("Global entries (available for all languages):")).pt_2().text_color(tokens::muted_foreground))
            .child(view().flex_row().items_center().gap_2()
                .child(icon_button(IconName::Plus, "Add a global documentation resource", strip, || {
                    let mut serial = 0u64; global::<DocSerial>().update(|s| { s.0 += 1; serial = s.0 });
                    write(move |p| p.languages.docs.global_entries.push(scratch_config::DocSource {
                        id: format!("user-{serial:x}"),
                        name: "Docs".to_owned(), kind: scratch_config::DocKind::Search,
                        url: "https://www.google.com/search?q={query}".to_owned(), post: String::new(), enabled: true,
                    }));
                    global::<DocsRevision>().update(|r| r.0 += 1);
                })))
            .child(card(global_rows))
            .child(view().flex_row().items_center().gap_2().pt_1()
                .child(check("When invoked from the editor, always use:", |p| p.languages.docs.use_preferred_global_docs,
                    |p, v| p.languages.docs.use_preferred_global_docs = v))
                .child(view().w(px(200.0)).child(preferred_docs_popup())))
            .into_element()
    });

    view().gap_2()
        .child(type_caption().apply(text("Below you can add online documentation resources for this language. Use {query} in the URL to represent the search string.")
            .text_color(tokens::muted_foreground)))
        .child(view().flex_row().items_center().gap_2()
            .child(icon_button(IconName::Plus, "Add a documentation resource", strip, {
                let id = id.to_owned();
                move || mutate_language(&id, |def| def.docs.push(scratch_language::DocSource {
                    name: "Docs".to_owned(), kind: scratch_language::DocType::Search,
                    url: "https://www.google.com/search?q={query}".to_owned(), post: None, css: None, js: None, enabled: true, scope: None,
                }))
            }))
            .child(type_caption().apply(text(format!("{count} resource(s)")).text_color(tokens::muted_foreground))))
        .child(card(rows))
        .child(when(count == 0, || type_body().apply(text("No Items").p_3().text_color(tokens::muted_foreground))))
        .child(global_section)
        .child(dynamic(move || match global::<DocEditSheet>().get().0 {
            Some((lang, index, field)) => doc_css_js_sheet(lang, index, field).into_element(),
            None => view().into_element(),
        }))
        .into_element()
}

/// A bump so the global docs table redraws after a prefs change.
#[derive(Clone, Copy, Default, PartialEq)]
struct DocsRevision(u64);

/// A serial for fresh global doc ids.
#[derive(Clone, Copy, Default, PartialEq)]
struct DocSerial(u64);

/// One row of the global-docs table (Enabled, Name, Type, URL, POST, −).
fn global_doc_row(index: usize, source: &scratch_config::DocSource, strip: IconButtonOptions) -> AnyElement {
    let kind = source.kind;
    let id = source.id.clone();
    view().flex_col().gap_1().px_2().py_1()
        .child(view().flex_row().items_center().gap_2()
            .child(checkbox("", source.enabled, CheckboxOptions::new(), move |on| {
                write(move |p| if let Some(d) = p.languages.docs.global_entries.get_mut(index) { d.enabled = on; });
                global::<DocsRevision>().update(|r| r.0 += 1);
            }))
            .child(view().w(px(130.0)).child(field(format!("gdoc-name:{id}"), "Name", source.name.clone(),
                TextFieldOptions::new().accessibility_label("Documentation name"), move |next| {
                    write(move |p| if let Some(d) = p.languages.docs.global_entries.get_mut(index) { d.name = next.trim().to_owned(); });
                    global::<DocsRevision>().update(|r| r.0 += 1);
                })))
            .child(view().w(px(110.0)).child(popup_button("Documentation type", doc_kind_name(kind),
                [scratch_config::DocKind::Search, scratch_config::DocKind::Bookmark, scratch_config::DocKind::TextFind].into_iter().map(|kind| {
                    MenuItem::new(doc_kind_name(kind)).checked(kind == source.kind).on_select(move || {
                        write(move |p| if let Some(d) = p.languages.docs.global_entries.get_mut(index) { d.kind = kind; });
                        global::<DocsRevision>().update(|r| r.0 += 1);
                    })
                }).collect(), PopupButtonOptions::new(), |_open: bool| {})))
            .child(view().flex_1().min_w(px(0.0)))
            .child(icon_button(IconName::Minus, "Delete the documentation entry", strip, move || {
                spawn(async move {
                    let response = nsh_base::alert_dialog()
                        .title("Are you sure you want to delete the documentation entry?")
                        .destructive("delete", "Delete")
                        .cancel("cancel", "Cancel")
                        .show().await;
                    if let Ok(response) = response && response.action.as_deref() == Some("delete") {
                        write(move |p| { p.languages.docs.global_entries.remove(index); });
                        global::<DocsRevision>().update(|r| r.0 += 1);
                    }
                });
            })))
        .child(field(format!("gdoc-url:{id}"), "https://… (use {query})", source.url.clone(),
            TextFieldOptions::new().mono(true).accessibility_label("Documentation URL"), move |next| {
                write(move |p| if let Some(d) = p.languages.docs.global_entries.get_mut(index) { d.url = next.trim().to_owned(); });
                global::<DocsRevision>().update(|r| r.0 += 1);
            }))
        .child(field(format!("gdoc-post:{id}"), "POST body (empty = GET)", source.post.clone(),
            TextFieldOptions::new().mono(true).accessibility_label("Documentation POST body"), move |next| {
                write(move |p| if let Some(d) = p.languages.docs.global_entries.get_mut(index) { d.post = next.trim().to_owned(); });
                global::<DocsRevision>().update(|r| r.0 += 1);
            }))
        .into_element()
}

/// The "always use" popup: the global entries by name.
fn preferred_docs_popup() -> impl IntoElement {
    global::<DocsRevision>().get();
    let entries = settings().with_untracked(|p| p.languages.docs.global_entries.clone());
    let preferred = settings().with_untracked(|p| p.languages.docs.preferred_global_docs_id.clone());
    let title = entries.iter().find(|e| e.id == preferred).map(|e| e.name.clone()).unwrap_or_else(|| "Dash".to_owned());
    let items = entries.iter().map(|e| {
        let id = e.id.clone();
        MenuItem::new(e.name.clone()).checked(e.id == preferred).on_select(move || {
            let id = id.clone();
            write(move |p| p.languages.docs.preferred_global_docs_id = id.clone());
        })
    }).collect();
    popup_button("Preferred documentation source", title, items, PopupButtonOptions::new(), |_open: bool| {})
}

/// The CSS/JS editor sheet of a Docs row.
fn doc_css_js_sheet(lang: String, index: usize, field: &'static str) -> impl IntoElement {
    let title = if field == "css" { "Custom CSS" } else { "Custom JavaScript" };
    let language = lang.clone();
    let close = || global::<DocEditSheet>().set(DocEditSheet(None));
    nsh_base::dialog()
        .label(title)
        .on_cancel(close)
        .flex_1()
        .child(view().absolute().inset_0().bg(tokens::scrim))
        .child(view().flex_1().items_center().pt(px(44.0)).pb(px(12.0))
            .child(view().w(px(560.0)).min_h(px(0.0)).flex_shrink(1.0)
                .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
                .shadow_lg().shadow_color(tokens::shadow())
                .bg(tokens::background)
                .child(view().px(px(20.0)).pt(px(20.0)).pb(px(12.0))
                    .child(type_subhead().semibold().apply(text(title)).text_color(tokens::foreground)))
                .child(view().px(px(20.0)).pb(px(8.0)).child(code_field(
                    move || Some(SharedString::from(language_def(&language)
                        .and_then(|d| d.docs.get(index).and_then(|d| if field == "css" { d.css.clone() } else { d.js.clone() }))
                        .unwrap_or_default())),
                    {
                        let key = format!("doc-{field}:{lang}:{index}");
                        move |text: &SharedString| {
                            let next = text.to_string();
                            let lang = lang.clone();
                            defer(key.clone(), move || mutate_language(&lang, move |def| if let Some(d) = def.docs.get_mut(index) {
                                if field == "css" { d.css = Some(next.clone()) } else { d.js = Some(next.clone()) }
                            }));
                        }
                    },
                    CodeFieldOptions::new(if field == "css" { "CSS" } else { "JavaScript" }).min_rows(6).max_rows(14)
                        .accessibility_label(title))))
                .child(view().flex_row().justify_end().px(px(20.0)).pt(px(8.0)).pb(px(20.0))
                    .child(button("Done", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Lg), close)))))
}

fn doc_kind_name(kind: scratch_config::DocKind) -> &'static str {
    match kind {
        scratch_config::DocKind::Search => "Search",
        scratch_config::DocKind::Bookmark => "Bookmark",
        scratch_config::DocKind::TextFind => "Text Find",
    }
}

fn doc_type_name(kind: scratch_language::DocType) -> &'static str {
    match kind {
        scratch_language::DocType::Search => "Search",
        scratch_language::DocType::Bookmark => "Bookmark",
        scratch_language::DocType::TextFind => "Text Find",
    }
}

fn doc_type_items(id: &str, index: usize, current: scratch_language::DocType) -> Vec<MenuItem> {
    use scratch_language::DocType;
    [DocType::Search, DocType::Bookmark, DocType::TextFind].into_iter().map(|kind| {
        let id = id.to_owned();
        MenuItem::new(doc_type_name(kind)).checked(kind == current).on_select(move || {
            mutate_language(&id, move |def| if let Some(d) = def.docs.get_mut(index) { d.kind = kind; })
        })
    }).collect()
}

// ---------------------------------------------- languages / diagnostics

// ------------------------------------------------------ diagnostics rows

/// The diagnostic rows opened by their disclosure, keyed `"{lang}:{index}"`.
#[derive(Clone, Default, PartialEq)]
struct DiagExpanded(std::collections::BTreeSet<String>);

/// Which diagnostics list sheet is open (Auto-Fixes / Ignored).
#[derive(Clone, Copy, Default, PartialEq)]
enum DiagSheet { #[default] None, AutoFix, Ignored }

fn language_diagnostics(id: &str) -> AnyElement {
    let Some(def) = language_def(id) else { return select_language_message() };
    let strip = IconButtonOptions::new().variant(IconButtonVariant::Filled).size(ControlSize::Sm);
    let builtin = { let id = id.to_owned(); move |on: bool| mutate_language(&id, move |def| def.builtin_diagnostics = on) };
    let count = def.diagnostics.len();
    let rows: Vec<AnyElement> = def.diagnostics.iter().enumerate().map(|(index, diagnostic)| diagnostic_row(id, index, diagnostic)).collect();
    view().gap_2()
        .child(checkbox("Automatically add built-in diagnostics", def.builtin_diagnostics, CheckboxOptions::new(), builtin))
        .child(view().flex_row().items_center().gap_2()
            .child(icon_button(IconName::Plus, "Add a diagnostic", strip, {
                let id = id.to_owned();
                move || {
                    mutate_language(&id, |def| {
                        let slug = unique_slug(&def.diagnostics.iter().map(|d| d.id.clone()).collect::<Vec<_>>(), "my-diagnostic");
                        def.diagnostics.push(scratch_language::DiagnosticDef {
                            id: slug, uuid: None, name: "My Diagnostic".to_owned(), kind: scratch_language::DiagnosticKind::Command,
                            update: scratch_language::UpdateMode::default(), options: Default::default(), output_mode: Default::default(),
                            regex: None, output_script: None, command: None, flags: None, key_equivalent: None,
                        });
                    });
                    let index = language_def(&id).map(|def| def.diagnostics.len().saturating_sub(1)).unwrap_or(0);
                    global::<DiagExpanded>().update(|e| { e.0.insert(format!("{id}:{index}")); });
                }
            }))
            .child(type_caption().apply(text(format!("{count} diagnostic(s)")).text_color(tokens::muted_foreground)))
            .child(view().flex_1())
            .child(button("Options…", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), || {
                spawn(async move {
                    let _ = popup_menu(diag_options_items()).await;
                });
            }))
            .child(button("Auto-Fixes…", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), || {
                global::<DiagSheet>().set(DiagSheet::AutoFix);
            }))
            .child(button("Ignored…", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm), || {
                global::<DiagSheet>().set(DiagSheet::Ignored);
            })))
        .child(card(rows))
        .child(when(count == 0, || type_body().apply(text("No Items").p_3().text_color(tokens::muted_foreground))))
        .child(dynamic(move || match global::<DiagSheet>().get() {
            DiagSheet::None => view().into_element(),
            which => diag_list_sheet(which).into_element(),
        }))
        .into_element()
}

/// The `Options…` menu of the diagnostics tab: the global toggles.
type BoolPrefRow = (&'static str, fn(&Preferences) -> bool, fn(&mut Preferences, bool));

fn diag_options_items() -> Vec<MenuItem> {
    let items: [BoolPrefRow; 4] = [
        ("Generate diagnostics while typing", |p| p.text_editing.diagnostics.while_typing,
         |p, v| p.text_editing.diagnostics.while_typing = v),
        ("Include errors from the console", |p| p.text_editing.diagnostics.from_console,
         |p, v| p.text_editing.diagnostics.from_console = v),
        ("Highlight lines with diagnostics", |p| p.text_editing.diagnostics.highlight_line,
         |p, v| p.text_editing.diagnostics.highlight_line = v),
        ("Underline text associated with diagnostics", |p| p.text_editing.diagnostics.underline,
         |p, v| p.text_editing.diagnostics.underline = v),
    ];
    items
    .into_iter().map(|(title, get, set)| {
        MenuItem::new(title).checked(settings().with_untracked(get)).on_select(move || write(|p| {
            let next = !get(p);
            set(p, next);
        }))
    }).collect()
}

/// The Auto-Fixes / Ignored list sheet: a text area over the prefs field.
fn diag_list_sheet(which: DiagSheet) -> impl IntoElement {
    type TextPrefRow = (&'static str, fn(&Preferences) -> String, fn(&mut Preferences, String), &'static str);
    let (title, get, set, hint): TextPrefRow = match which {
        DiagSheet::AutoFix => ("Automatically Applied Fixes",
            |p| p.text_editing.diagnostics.auto_fix_diagnostics.clone(),
            |p, v| p.text_editing.diagnostics.auto_fix_diagnostics = v,
            "Regular expressions, one per line"),
        _ => ("Globally Ignored Diagnostic Messages",
            |p| p.text_editing.diagnostics.ignored_diagnostics.clone(),
            |p, v| p.text_editing.diagnostics.ignored_diagnostics = v,
            "Regular expressions, one per line"),
    };
    let close = || global::<DiagSheet>().set(DiagSheet::None);
    nsh_base::dialog()
        .label(title)
        .on_cancel(close)
        .flex_1()
        .child(view().absolute().inset_0().bg(tokens::scrim))
        .child(view().flex_1().items_center().pt(px(44.0)).pb(px(12.0))
            .child(view().w(px(560.0)).min_h(px(0.0)).flex_shrink(1.0)
                .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
                .shadow_lg().shadow_color(tokens::shadow())
                .bg(tokens::background)
                .child(view().px(px(20.0)).pt(px(20.0)).pb(px(12.0))
                    .child(type_subhead().semibold().apply(text(title)).text_color(tokens::foreground)))
                .child(view().px(px(20.0)).pb(px(8.0)).h(px(160.0)).child(text_area(hint,
                    settings().with_untracked(|p| SharedString::from(get(p))),
                    move |next: &str| write(move |p| set(p, next.to_owned())),
                    TextFieldOptions::new().mono(true).accessibility_label(title), None)))
                .child(view().flex_row().justify_end().px(px(20.0)).pt(px(8.0)).pb(px(20.0))
                    .child(button("Done", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Lg), close)))))
}

/// One diagnostic: a collapsible card row — header with the name, the
/// update-mode popup and a row menu; the detail fields below.
fn diagnostic_row(lang: &str, index: usize, diagnostic: &scratch_language::DiagnosticDef) -> AnyElement {
    let key = format!("{lang}:{index}");
    let is_open = { let key = key.clone(); computed(move || global::<DiagExpanded>().with(|e| e.0.contains(&key))) };

    let name_field = {
        let lang = lang.to_owned();
        view().flex_grow(1.0).child(field(format!("diag-name:{lang}:{index}"), "My Diagnostic", diagnostic.name.clone(),
            TextFieldOptions::new().accessibility_label("Diagnostic name"), move |next| {
                mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.name = next.trim().to_owned(); })
            }))
    };

    let disclosure = {
        let key = key.clone();
        dynamic(move || {
            let icon = if is_open.get() { IconName::ChevronDown } else { IconName::ChevronRight };
            let key = key.clone();
            icon_button(icon, "Show the diagnostic's details", IconButtonOptions::new().variant(IconButtonVariant::Ghost).size(ControlSize::Sm),
                move || global::<DiagExpanded>().update(|e| { if !e.0.remove(&key) { e.0.insert(key.clone()); } }))
                .into_element()
        })
    };

    let actions = {
        let (lang, name) = (lang.to_owned(), diagnostic.name.clone());
        icon_button(IconName::EllipsisCircle, "Diagnostic actions",
            IconButtonOptions::new().variant(IconButtonVariant::Ghost).size(ControlSize::Sm), move || {
                let (lang, name) = (lang.clone(), name.clone());
                spawn(async move {
                    let _ = popup_menu(vec![
                        MenuItem::new("Delete Diagnostic…").on_select(move || {
                            let (lang, name) = (lang.clone(), name.clone());
                            spawn(async move {
                                let response = nsh_base::alert_dialog()
                                    .title(format!("Are you sure you want to delete the diagnostic “{name}”?"))
                                    .description("The associated files will be moved to the Trash.")
                                    .destructive("delete", "Delete")
                                    .cancel("cancel", "Cancel")
                                    .show().await;
                                if let Ok(response) = response && response.action.as_deref() == Some("delete") {
                                    mutate_language(&lang, move |def| { def.diagnostics.remove(index); });
                                }
                            });
                        }),
                    ]).await;
                });
            })
    };

    let header = view().flex_row().items_center().gap_2().px_2().py_1()
        .child(disclosure)
        .child(name_field)
        .child(popup_button("Diagnostic update mode", diag_update_title(diagnostic), diag_update_items(lang, index, diagnostic),
            PopupButtonOptions::new(), |_open: bool| {}))
        .child(actions);

    let body = {
        let (lang, diagnostic) = (lang.to_owned(), diagnostic.clone());
        when(move || is_open.get(), move || diagnostic_body(&lang, index, &diagnostic))
    };

    view().child(header).child(body).into_element()
}

/// The update-mode popup's title (CodeRunner titles the bar popup with the
/// mode: `Run Diagnostic`, `On Edit`, `On Run`, `Manually`).
fn diag_update_title(d: &scratch_language::DiagnosticDef) -> String {
    let mut parts = Vec::new();
    if d.update.on_edit { parts.push("On Edit"); }
    if d.update.on_run { parts.push("On Run"); }
    if d.update.manual { parts.push("Manually"); }
    if parts.is_empty() { "Run Diagnostic".to_owned() } else { parts.join(" + ") }
}

fn diag_update_items(lang: &str, index: usize, d: &scratch_language::DiagnosticDef) -> Vec<MenuItem> {
    let set = |lang: &str, index: usize, change: fn(&mut scratch_language::DiagnosticDef)| {
        let lang = lang.to_owned();
        move || mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { change(d); })
    };
    let lang_owned = lang.to_owned();
    vec![
        MenuItem::new("Run Diagnostic").on_select(move || diagnostic_run_now(lang_owned.clone(), index)),
        MenuItem::separator(),
        MenuItem::new("On Edit").checked(d.update.on_edit).on_select(set(lang, index, |d| d.update.on_edit = !d.update.on_edit)),
        MenuItem::new("On Run").checked(d.update.on_run).on_select(set(lang, index, |d| d.update.on_run = !d.update.on_run)),
        MenuItem::new("Manually").checked(d.update.manual).on_select(set(lang, index, |d| d.update.manual = !d.update.manual)),
        MenuItem::separator(),
        MenuItem::new("Cancel Run If New Errors Produced").checked(d.options.cancel_on_errors).on_select(set(lang, index, |d| d.options.cancel_on_errors = !d.options.cancel_on_errors)),
        MenuItem::new("Cancel Run If New Warnings Produced").checked(d.options.cancel_on_warnings).on_select(set(lang, index, |d| d.options.cancel_on_warnings = !d.options.cancel_on_warnings)),
        MenuItem::new("Cancel Run If New Notices Produced").checked(d.options.cancel_on_notices).on_select(set(lang, index, |d| d.options.cancel_on_notices = !d.options.cancel_on_notices)),
    ]
}

/// The expanded body (DESIGN 8.13.2: generate-using popup, kind fields,
/// output-mode popup, regex/script editors, key recorder, test run).
fn diagnostic_body(lang: &str, index: usize, d: &scratch_language::DiagnosticDef) -> AnyElement {
    use scratch_language::{DiagnosticKind as K, OutputMode as O};
    let lab = |title: &'static str, control: AnyElement| view().flex_row().items_center().gap_2().px_2().py_1()
        .child(view().w(px(170.0)).child(type_body().apply(text(title)).text_color(tokens::muted_foreground)))
        .child(control).into_element();

    let mut rows: Vec<AnyElement> = Vec::new();

    // Generate diagnostics using:
    let kinds = [K::Command, K::CompileScript, K::ConsoleText, K::ConsoleHighlight];
    let kind_items = kinds.into_iter().map(|kind| {
        let lang = lang.to_owned();
        MenuItem::new(diag_kind_name(kind)).checked(kind == d.kind).on_select(move || {
            mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.kind = kind; })
        })
    }).collect();
    rows.push(lab("Generate diagnostics using:", view().w(px(230.0))
        .child(popup_button("Diagnostic kind", diag_kind_name(d.kind), kind_items, PopupButtonOptions::new(), |_open: bool| {})).into_element()));

    // Kind fields.
    match d.kind {
        K::CompileScript => {
            let lang = lang.to_owned();
            rows.push(lab("", view().flex_row().items_center().gap_4()
                .child(checkbox("Use custom compile flags:", d.options.custom_flags, CheckboxOptions::default(), {
                    let lang = lang.clone();
                    move |on: bool| mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.options.custom_flags = on; })
                }))
                .child(checkbox("Add to existing flags", d.options.add_to_existing, CheckboxOptions::default(), {
                    let lang = lang.clone();
                    move |on: bool| mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.options.add_to_existing = on; })
                })).into_element()));
            if d.options.custom_flags {
                let lang = lang.to_owned();
                rows.push(lab("Compile Flags:", view().w(px(280.0)).child(field(format!("diag-flags:{lang}:{index}"), "-Wall",
                    d.flags.clone().unwrap_or_default(), TextFieldOptions::new().mono(true).accessibility_label("Diagnostic compile flags"), move |next| {
                        mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.flags = Some(next.trim().to_owned()); })
                    })).into_element()));
            }
        }
        K::Command => {
            let lang = lang.to_owned();
            let lang_for_value = lang.clone();
            rows.push(lab("Command:", view().flex_1().min_w(px(0.0)).child(code_field(
                move || Some(SharedString::from(language_def(&lang_for_value).and_then(|d| d.diagnostics.get(index).and_then(|d| d.command.clone())).unwrap_or_default())),
                {
                    let lang = lang.clone();
                    let key = format!("diag-cmd:{lang}:{index}");
                    move |text: &SharedString| {
                        let next = text.to_string();
                        let lang = lang.clone();
                        defer(key.clone(), move || mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.command = Some(next.clone()); }));
                    }
                }, CodeFieldOptions::new("shell").min_rows(1).max_rows(4).accessibility_label("Diagnostic command")
                    .placeholder("flake8 $filename".to_owned()))).into_element()));
        }
        _ => {}
    }

    // Mode to interpret output:
    let modes = [(O::Unix, "Text (Unix/GCC style messages)"), (O::Regex, "Regular Expression"), (O::CustomScript, "Custom Script"), (O::CheckStyle, "CheckStyle")];
    let mode_items = modes.into_iter().map(|(mode, title)| {
        let lang = lang.to_owned();
        MenuItem::new(title).checked(mode == d.output_mode).on_select(move || {
            mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.output_mode = mode; })
        })
    }).collect();
    let mode_title = modes.iter().find(|(mode, _)| *mode == d.output_mode).map(|(_, title)| *title).unwrap_or("Text (Unix/GCC style messages)");
    rows.push(lab("Mode to interpret output:", view().w(px(230.0))
        .child(popup_button("Diagnostic output mode", mode_title, mode_items, PopupButtonOptions::new(), |_open: bool| {})).into_element()));

    match d.output_mode {
        O::Regex => {
            let lang = lang.to_owned();
            let lang_for_value = lang.clone();
            rows.push(view().px_2().py_1().gap_1()
                .child(code_field(
                    move || Some(SharedString::from(language_def(&lang_for_value).and_then(|d| d.diagnostics.get(index).and_then(|d| d.regex.clone()))
                        .unwrap_or_else(|| r"^(?<file>[^\s].*?):(?<line>\d+):(?<column>\d+):? (?<message>.*)$".to_owned()))),
                    {
                        let lang = lang.clone();
                        let key = format!("diag-regex:{lang}:{index}");
                        move |text: &SharedString| {
                            let next = text.to_string();
                            let lang = lang.clone();
                            defer(key.clone(), move || mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.regex = Some(next.clone()); }));
                        }
                    }, CodeFieldOptions::new("regex").min_rows(1).max_rows(4).accessibility_label("Diagnostic regular expression")))
                .child(type_caption().apply(text("Possible capture groups: file, line, column, message, messageID, error, warning, note, zeroLine, zeroColumn, endLine, endColumn, offset, endOffset, length, code"))
                    .text_color(tokens::muted_foreground))
                .into_element());
        }
        O::CustomScript => {
            let lang = lang.to_owned();
            let lang_for_value = lang.clone();
            rows.push(view().px_2().py_1().gap_1()
                .child(type_caption().apply(text("The script reads the output on stdin and prints JSON:")).text_color(tokens::muted_foreground))
                .child(code_field(
                    move || Some(SharedString::from(language_def(&lang_for_value).and_then(|d| d.diagnostics.get(index).and_then(|d| d.output_script.clone())).unwrap_or_default())),
                    {
                        let lang = lang.clone();
                        let key = format!("diag-script:{lang}:{index}");
                        move |text: &SharedString| {
                            let next = text.to_string();
                            let lang = lang.clone();
                            defer(key.clone(), move || mutate_language(&lang, move |def| if let Some(d) = def.diagnostics.get_mut(index) { d.output_script = Some(next.clone()); }));
                        }
                    }, CodeFieldOptions::new("shell").min_rows(3).max_rows(10).accessibility_label("Diagnostic output script")))
                .into_element());
        }
        _ => {}
    }

    // Keyboard shortcut recorder.
    rows.push(lab("Keyboard shortcut:", diag_key_recorder(lang, index, d).into_element()));

    // Test run.
    let lang_owned = lang.to_owned();
    rows.push(view().px_2().py_1().pb_2().flex_row().justify_end()
        .child(button("Perform Test Run…", ButtonOptions::new().variant(ButtonVariant::Default), move || diagnostic_test_run(&lang_owned, index)))
        .into_element());

    view().pb_1().gap_0().children(rows).into_element()
}

/// The diagnostic's `Keyboard shortcut:` recorder — a smaller take on the
/// bindings pane's capsule; writes `key_equivalent` in crbindings format.
fn diag_key_recorder(lang: &str, index: usize, d: &scratch_language::DiagnosticDef) -> impl IntoElement {
    let key = format!("diag-key:{lang}:{index}");
    let armed = { let key = key.clone(); computed(move || global::<Recording>().with(|r| r.0 == key)) };
    let label = d.key_equivalent.as_deref()
        .and_then(|text| scratch_commands::Stroke::parse_crbindings(text).ok())
        .map(|stroke| stroke.display())
        .unwrap_or_default();
    view().min_w(px(140.0)).h(px(22.0)).px_2().flex_row().items_center().justify_center()
        .rounded(px(tokens::radius())).border_1()
        .border_color(move || if armed.get() { tokens::ring() } else { tokens::input() })
        .bg(move || if armed.get() { tokens::muted() } else { tokens::card() })
        .label("Keyboard shortcut for the diagnostic")
        .child({
            let empty = label.is_empty();
            text(move || if armed.get() { "Type shortcut…".to_owned() } else if empty { "None".to_owned() } else { label.clone() })
                .text_size(px(tokens::type_caption()))
                .text_color(move || if armed.get() { tokens::selection_foreground() } else if empty { tokens::muted_foreground() } else { tokens::foreground() })
        })
}

/// Runs the diagnostic once on the active document ("Run Diagnostic" in the
/// update-mode popup).
fn diagnostic_run_now(lang: String, index: usize) {
    let Some(def) = language_def(&lang) else { return };
    let Some(diagnostic) = def.diagnostics.get(index).cloned() else { return };
    let Some((text, file, saved, flags)) = with(|workspace| workspace.active.and_then(|id| workspace.doc(id)).map(|doc| {
        let text = state::editor_api(doc.document.id).map(|api| api.value()).unwrap_or_default();
        let file = state::saved_path(&doc.document);
        (text, file, !doc.document.dirty, doc.document.run.compiler_flags.clone())
    })) else { state::notice("Open a document to run the diagnostic."); return; };
    run_diagnostic_job(Arc::new(def), diagnostic, text, file, saved, flags.unwrap_or_default());
}

/// "Perform Test Run…": pick a document, run the diagnostic on it, and
/// report in an alert how many diagnostics it produced.
fn diagnostic_test_run(lang: &str, index: usize) {
    let Some(def) = language_def(lang) else { return };
    let Some(diagnostic) = def.diagnostics.get(index).cloned() else { return };
    let language = Arc::new(def.clone());
    let extensions = def.extensions.clone();
    Owner::detached().run(|| spawn(async move {
        let dialog = nsh_core::file_dialog::OpenDialog::files()
            .filter(nsh_core::file_dialog::FileFilter::new(format!("{} documents", def.name), extensions));
        match pick_paths(dialog).await {
            Ok(Some(paths)) => if let Some(path) = paths.into_iter().next() {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                run_diagnostic_job(language, diagnostic, text, Some(path), true, def.run.flags.clone());
            },
            Ok(None) => {},
            Err(error) => state::notice(format!("The test run could not pick a document: {error}")),
        }
    }));
}

/// Runs `diagnostic` on `text` (a worker thread) and reports the count and
/// raw output in an alert.
fn run_diagnostic_job(language: Arc<scratch_language::LanguageDef>, diagnostic: scratch_language::DiagnosticDef,
    text: String, file: Option<PathBuf>, saved: bool, flags: String)
{
    let name = diagnostic.name.clone();
    Owner::detached().run(|| spawn(async move {
        let mut job = scratch_diagnostics::runner::DiagnosticJob::new(language, diagnostic, text.clone()).saved(saved).language_flags(flags);
        if let Some(file) = file.clone() { job = job.file(file); }
        let result = blocking(move || scratch_diagnostics::runner::run_job(&job, &scratch_diagnostics::exec::CancelToken::new())).await;
        let (title, description) = match result {
            Ok(Ok(output)) => {
                let (ours, _theirs) = output.annotations(&text, file.as_deref());
                let mut description = format!("Output from the diagnostic run:\n\n{}", output.raw_output.trim());
                if description.len() > 1500 { description.truncate(1500); description.push('…'); }
                (format!("The diagnostics test run produced {} diagnostic(s).", ours.len()), description)
            }
            Ok(Err(error)) => ("The diagnostics test run failed.".to_owned(), error.to_string()),
            Err(error) => ("The diagnostics test run failed.".to_owned(), error.to_string()),
        };
        let _ = nsh_base::alert_dialog()
            .title(format!("{name}: {title}"))
            .description(description)
            .action("ok", "OK")
            .show().await;
    }));
}

fn diag_kind_name(kind: scratch_language::DiagnosticKind) -> &'static str {
    use scratch_language::DiagnosticKind as K;
    match kind {
        K::CompileScript => "Compile Script", K::Command => "Command", K::ConsoleText => "Console Text",
        K::ConsoleHighlight => "Console Text (Highlight Only)", K::Clang => "Clang", K::InvalidSyntax => "Invalid Syntax",
        K::RegexCheck => "Regex Check", K::Conflict => "Conflicts",
    }
}

/// `base`, or `base-2`, ... when taken.
fn unique_slug(taken: &[String], base: &str) -> String {
    (1..).map(|n| if n == 1 { base.to_owned() } else { format!("{base}-{n}") })
        .find(|slug| !taken.iter().any(|t| t == slug)).expect("a free slug")
}

/// Applies the typed run command and extensions of a language, if any.
fn commit_language_draft() {
    let draft = global::<LanguageDraft>();
    let LanguageDraft { id, command, extensions } = draft.get_untracked();
    if id.is_empty() || (command.is_none() && extensions.is_none()) { return; }
    draft.set(LanguageDraft::default());
    let message = match edit_language(&id, command, extensions) {
        Ok(changed) => { if changed { global::<LanguagesRevision>().update(|r| r.0 += 1); } None }
        Err(error) => Some(error),
    };
    global::<LanguageMessage>().set(LanguageMessage(message));
}

/// Saves a language with a new run command and extensions. `Ok(false)` when
/// nothing changed.
fn edit_language(id: &str, command: Option<String>, extensions: Option<String>) -> Result<bool, String> {
    with(|workspace| {
        let dir = workspace.paths.as_ref().map(Paths::languages);
        let registry = workspace.registry.as_mut().ok_or("The languages are still loading.")?;
        if registry.user_dir().is_none() && let Some(dir) = dir { registry.set_user_dir(dir); }
        let current = registry.get(id).ok_or_else(|| format!("No language `{id}`."))?;
        let mut def = current.clone();
        if let Some(command) = command { def.run.command = command.trim().to_owned(); }
        if let Some(extensions) = extensions {
            def.extensions = parse_extensions(&extensions);
            if def.extensions.is_empty() && !current.extensions.is_empty() { return Err("Enter at least one file extension.".to_owned()); }
        }
        if &def == current { return Ok(false); }
        registry.save(def).map(|()| true).map_err(|error| format!("Not saved: {error}"))
    })
}

fn reset_language(id: &str) {
    PENDING.with_borrow_mut(|pending| pending.retain(|(key, _, _)| !key.starts_with("lang-")));
    global::<LanguageDraft>().set(LanguageDraft::default());
    let result = with(|workspace| workspace.registry.as_mut().map(|registry| registry.reset(id)));
    let message = match result {
        Some(Ok(())) => None,
        Some(Err(error)) => Some(format!("Not reset: {error}")),
        None => Some("The languages are still loading.".to_owned()),
    };
    global::<LanguageMessage>().set(LanguageMessage(message));
    global::<LanguagesRevision>().update(|r| r.0 += 1);
    global::<LanguageReset>().update(|r| r.0 += 1);
}

/// `"py, .pyw  PYI"` to `["py", "pyw", "PYI"]`: separated by commas or
/// spaces, a leading dot dropped, duplicates removed.
fn parse_extensions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split([',', ' ', '\t']) {
        let part = part.trim().trim_start_matches('.');
        if !part.is_empty() && !out.iter().any(|e| e == part) { out.push(part.to_owned()); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_split_and_cleaned() {
        assert_eq!(parse_extensions("py, .pyw  pyi,py"), vec!["py", "pyw", "pyi"]);
        assert!(parse_extensions(" , ").is_empty());
    }

    #[test]
    fn arguments_split_at_whitespace() {
        assert_eq!(split_arguments("  -e   -u "), vec!["-e", "-u"]);
        assert!(split_arguments("").is_empty());
    }

    #[test]
    fn new_variables_get_a_free_name() {
        let mut env = default_environment();
        assert_eq!(unique_variable(&env), "VARIABLE");
        env.insert("VARIABLE".into(), String::new());
        assert_eq!(unique_variable(&env), "VARIABLE_2");
    }

    #[test]
    fn categories_keep_coderunner_pane_ids() {
        for category in Category::ALL { assert_eq!(Category::from_identifier(category.identifier()), category); }
        assert_eq!(Category::from_identifier("NoSuchPane"), Category::General);
    }

    #[test]
    fn an_invalid_file_gives_defaults_and_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(read(&path), (Preferences::default(), None), "a missing file is not a problem");
        std::fs::write(&path, "[text_editing.indentation]\ntab_width = 5\nsoft_tabs = true\n").unwrap();
        let (prefs, notice) = read(&path);
        assert!(prefs.text_editing.indentation.soft_tabs, "valid values are kept");
        assert_eq!(prefs.text_editing.indentation.tab_width, 4, "the invalid one falls back");
        assert!(notice.unwrap().starts_with("Settings: "));
        std::fs::write(&path, "this is = = not toml").unwrap();
        let (prefs, notice) = read(&path);
        assert_eq!(prefs, Preferences::default());
        assert!(notice.unwrap().contains("Using defaults"));
    }

    #[test]
    fn pending_edits_apply_latest_only_and_once() {
        use std::cell::Cell;
        thread_local! { static SEEN: Cell<u32> = const { Cell::new(0) }; }
        let push = |key: &str, n: u32| {
            let generation = GENERATION.with(|g| { g.set(g.get() + 1); g.get() });
            PENDING.with_borrow_mut(|p| { p.retain(|(k, _, _)| k != key); p.push((key.to_owned(), generation, Box::new(move || SEEN.set(n)))); });
            generation
        };
        let first = push("font", 1);
        let second = push("font", 2);
        apply_pending(Some(("font", first)));
        assert_eq!(SEEN.get(), 0, "a stale timer applies nothing");
        apply_pending(Some(("font", second)));
        assert_eq!(SEEN.get(), 2);
        SEEN.set(0);
        apply_pending(None);
        assert_eq!(SEEN.get(), 0, "applied once");
    }

    #[test]
    fn bound_keys_are_keystrokes() {
        for bound in crate::workbench::bindings::COMMANDS {
            if bound.keys.is_empty() { continue; }
            let parsed = nsh_core::keymap::Keystroke::parse_with(bound.keys, true).unwrap_or_else(|e| panic!("{}: {e}", bound.id));
            assert!(parsed.reserved(nsh_core::element::Os::MacOs).is_none(), "{}", bound.id);
        }
    }

    #[test]
    fn indentation_title_reads_like_coderunner() {
        let mut def = scratch_language::LanguageDef::new("x", "X");
        assert_eq!(indent_title(&def), "Default");
        def.custom_tab_mode = true;
        def.soft_tabs = false;
        def.tab_width = 4;
        assert_eq!(indent_title(&def), "Tabs, 4 wide");
        def.soft_tabs = true;
        assert_eq!(indent_title(&def), "4 spaces");
    }

    #[test]
    fn encoding_zero_is_automatic() {
        assert_eq!(encoding_title(0), "Automatic (UTF-8)");
        assert_eq!(encoding_title(4), "UTF-8");
    }

    #[test]
    fn default_elements_cover_coderunner_names() {
        assert_eq!(DEFAULT_ELEMENTS.len(), 19);
        assert!(DEFAULT_ELEMENTS.iter().all(|(name, scope)| !name.is_empty() && !scope.is_empty()));
        assert_eq!(DEFAULT_ELEMENTS[0].0, "Comment");
    }

    #[test]
    fn bound_ids_paths_and_keys_are_unique() {
        let commands = crate::workbench::bindings::COMMANDS;
        for (i, a) in commands.iter().enumerate() {
            assert!(commands[i + 1..].iter().all(|b| b.id != a.id), "{} is listed twice", a.id);
            assert!(commands[i + 1..].iter().all(|b| b.path != a.path), "{} is listed twice", a.path);
            if !a.keys.is_empty() {
                assert!(commands[i + 1..].iter().all(|b| b.keys != a.keys || b.keys.is_empty()), "{} is listed twice", a.keys);
            }
        }
    }
}
