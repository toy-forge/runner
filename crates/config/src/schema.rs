//! The preference schema: a static, data-driven description of every
//! preference, from which the Preferences window is generated.
//!
//! [`PrefSchema::all`] lists the entries in display order. Entries of a pane
//! are grouped by [`PrefSpec::category`] (a segmented control or a heading),
//! then [`PrefSpec::section`] (a labelled sub-group). Entries with a
//! [`PrefSpec::parent`] are laid out indented under the parent and, when
//! [`PrefSpec::gate`] says so, disabled while the parent is off. Controls whose
//! [`Layout`] is not [`Layout::Normal`] are drawn inside or beside the parent
//! instead of on a row of their own. Entries with [`PrefSpec::in_ui`] false
//! have no control (hidden preferences).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::PrefValue;
use crate::schema_table::GROUPS;

/// A preference id: the dotted TOML path of the key, such as
/// `text_editing.indentation.tab_width`.
pub type PrefId = &'static str;

/// Title of the alert shown after changing a [`PrefSpec::requires_restart`]
/// preference.
pub const RESTART_ALERT_TITLE: &str = "App restart required before this change takes effect.";
/// Informative text of the restart alert.
pub const RESTART_ALERT_MESSAGE: &str =
    "Your new setting will take effect the next time ScratchRunner is launched.";

/// A pane of the Preferences window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    /// General.
    General,
    /// Text Editing.
    TextEditing,
    /// Languages (per-language settings live in the language bundles).
    Languages,
    /// Themes.
    Themes,
    /// Key Bindings.
    KeyBindings,
    /// Shell.
    Shell,
    /// Advanced.
    Advanced,
    /// Preferences without a pane (state and hidden options).
    Hidden,
}

impl Pane {
    /// The seven panes in toolbar order.
    pub const ALL: [Pane; 7] = [
        Pane::General,
        Pane::TextEditing,
        Pane::Languages,
        Pane::Themes,
        Pane::KeyBindings,
        Pane::Shell,
        Pane::Advanced,
    ];

    /// The toolbar label.
    pub const fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::TextEditing => "Text Editing",
            Pane::Languages => "Languages",
            Pane::Themes => "Themes",
            Pane::KeyBindings => "Key Bindings",
            Pane::Shell => "Shell",
            Pane::Advanced => "Advanced",
            Pane::Hidden => "Hidden",
        }
    }

    /// The stable identifier stored in `ui.selected_preferences_pane`.
    pub const fn identifier(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::TextEditing => "TextEditing",
            Pane::Languages => "Languages",
            Pane::Themes => "Themes",
            Pane::KeyBindings => "KeyBindings",
            Pane::Shell => "Shell",
            Pane::Advanced => "Advanced",
            Pane::Hidden => "Hidden",
        }
    }

    /// The pane with this [`Pane::identifier`].
    pub fn from_identifier(identifier: &str) -> Option<Pane> {
        Pane::ALL
            .into_iter()
            .find(|pane| pane.identifier() == identifier)
    }

    /// The window title while this pane is shown: `Preferences – <Pane>`.
    pub fn window_title(self) -> String {
        format!("Preferences \u{2013} {}", self.title())
    }

    /// An SF Symbol to stand in for the toolbar icon.
    pub const fn icon_symbol(self) -> &'static str {
        match self {
            Pane::General => "gearshape",
            Pane::TextEditing => "square.and.pencil",
            Pane::Languages => "curlybraces.square",
            Pane::Themes => "paintpalette",
            Pane::KeyBindings => "command.square",
            Pane::Shell => "terminal",
            Pane::Advanced => "gearshape.2",
            Pane::Hidden => "questionmark",
        }
    }
}

/// A constant value usable in a `static` table: defaults and popup options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstValue {
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string.
    Str(&'static str),
    /// A list of strings.
    StrList(&'static [&'static str]),
    /// A list of integers.
    IntList(&'static [i64]),
    /// Any other value, as the text of a TOML value (`{ a = 1 }`, `[ ... ]`).
    Toml(&'static str),
}

impl ConstValue {
    /// The value as a [`PrefValue`].
    pub fn to_value(&self) -> PrefValue {
        match *self {
            ConstValue::Bool(b) => PrefValue::Boolean(b),
            ConstValue::Int(i) => PrefValue::Integer(i),
            ConstValue::Float(f) => PrefValue::Float(f),
            ConstValue::Str(s) => PrefValue::String(s.to_owned()),
            ConstValue::StrList(items) => PrefValue::Array(
                items
                    .iter()
                    .map(|s| PrefValue::String((*s).to_owned()))
                    .collect(),
            ),
            ConstValue::IntList(items) => {
                PrefValue::Array(items.iter().map(|i| PrefValue::Integer(*i)).collect())
            }
            ConstValue::Toml(text) => parse_value(text).unwrap_or_else(|| {
                tracing::error!("invalid TOML in a preference default: {text}");
                PrefValue::Table(Default::default())
            }),
        }
    }
}

/// Parses the text of one TOML value.
pub(crate) fn parse_value(text: &str) -> Option<PrefValue> {
    let table: toml::Table = toml::from_str(&format!("v = {text}")).ok()?;
    table.get("v").cloned()
}

/// One entry of a popup or radio group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupOption {
    /// The value stored in the preference.
    pub value: ConstValue,
    /// The menu title.
    pub title: &'static str,
    /// The value CodeRunner stores for this option, when it differs from
    /// [`PopupOption::value`] (CodeRunner stores enums as integers).
    pub cr: Option<ConstValue>,
    /// A separator line instead of a choice; the other fields are unused.
    pub separator: bool,
}

impl PopupOption {
    /// A choice whose stored value is the same in CodeRunner.
    pub const fn new(value: ConstValue, title: &'static str) -> Self {
        Self {
            value,
            title,
            cr: None,
            separator: false,
        }
    }

    /// A choice stored differently in CodeRunner.
    pub const fn with_cr(value: ConstValue, title: &'static str, cr: ConstValue) -> Self {
        Self {
            value,
            title,
            cr: Some(cr),
            separator: false,
        }
    }

    /// A separator line.
    pub const SEPARATOR: PopupOption = PopupOption {
        value: ConstValue::Bool(false),
        title: "-",
        cr: None,
        separator: true,
    };
}

/// Describes a key/value table control (the Shell Variables table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyValueTableSpec {
    /// Title of the key column.
    pub key_title: &'static str,
    /// Title of the value column.
    pub value_title: &'static str,
    /// Tooltip of the `+` button.
    pub add_tooltip: &'static str,
    /// Tooltip of the `-` button.
    pub remove_tooltip: &'static str,
    /// Tooltip of the reset button.
    pub reset_tooltip: &'static str,
    /// Title of the reset button.
    pub reset_title: &'static str,
}

/// Which control edits a preference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlKind {
    /// A checkbox (switch).
    Checkbox,
    /// Radio buttons, one per option (two options for a boolean).
    Radio(&'static [PopupOption]),
    /// A popup button with fixed options.
    Popup(&'static [PopupOption]),
    /// A popup whose options come from the app at run time; `source` names
    /// them: `themes.light`, `themes.dark`, `key_binding_sets`,
    /// `global_docs`.
    DynamicPopup {
        /// The option source.
        source: &'static str,
    },
    /// A numeric text field with a stepper.
    Number {
        /// Smallest allowed value.
        min: f64,
        /// Largest allowed value.
        max: f64,
        /// Stepper increment.
        step: f64,
        /// Only whole numbers are allowed.
        integer: bool,
    },
    /// A text field.
    Text {
        /// A multi-line editor.
        multiline: bool,
        /// Placeholder text of the empty field.
        placeholder: Option<&'static str>,
    },
    /// A font chooser: the family, with the size in the preference
    /// `size_id`.
    Font {
        /// The id of the size preference.
        size_id: PrefId,
    },
    /// A color well (reserved; no preference uses it yet).
    Color,
    /// A file or folder path field.
    Path {
        /// A folder rather than a file.
        directory: bool,
    },
    /// A push button that only triggers `action`; it holds no value.
    Button {
        /// The action id the UI dispatches.
        action: &'static str,
    },
    /// A push button that opens an editor for this text value; `action` names
    /// the sheet.
    EditButton {
        /// The action id the UI dispatches.
        action: &'static str,
    },
    /// An editable table of string pairs.
    KeyValueTable(&'static KeyValueTableSpec),
    /// A control the schema cannot describe; `kind` names the custom editor
    /// (`docs_table`, `theme_list`) or is empty for hidden preferences.
    Custom(&'static str),
}

/// How a child control is enabled by its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Always enabled.
    None,
    /// Enabled while the parent boolean is on.
    WhenOn,
    /// Enabled while the parent boolean is off.
    WhenOff,
}

/// Where a control sits relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// On a row of its own (indented when it has a parent).
    Normal,
    /// The control replaces the `%@` in its own label (`Delay %@ seconds`).
    InlineInOwnLabel,
    /// The control replaces the `%@` in the parent's label.
    InlineInParentLabel,
    /// The control follows the parent's control on the same row.
    SameRowAsParent,
}

/// The description of one preference or action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefSpec {
    /// The id; for a preference, the dotted path in `settings.toml`. For a
    /// pure button, a path under `<pane>.action`.
    pub id: PrefId,
    /// The pane.
    pub pane: Pane,
    /// The category (a heading or segment), `None` for entries outside any.
    pub category: Option<&'static str>,
    /// The sub-group inside the category.
    pub section: Option<&'static str>,
    /// The label, `None` for controls without one.
    pub label: Option<&'static str>,
    /// Tooltip or informative text, from CodeRunner verbatim.
    pub help: Option<&'static str>,
    /// The control.
    pub control: ControlKind,
    /// The default value (for a pure button, an unused `Bool(false)`).
    pub default: ConstValue,
    /// The parent entry this one is nested under.
    pub parent: Option<PrefId>,
    /// How the parent enables this entry.
    pub gate: Gate,
    /// Placement relative to the parent.
    pub layout: Layout,
    /// A change only takes effect after a restart
    /// ([`RESTART_ALERT_TITLE`]).
    pub requires_restart: bool,
    /// Shown in the Preferences window.
    pub in_ui: bool,
    /// The key of the same preference in CodeRunner's defaults.
    pub cr_key: Option<&'static str>,
    /// Whether [`crate::import_from_cr_defaults`] copies the CodeRunner value.
    pub import: bool,
    /// Mirrored into every open editor, so a change applies live.
    pub per_editor: bool,
}

impl PrefSpec {
    /// A spec with neutral defaults; the constructors below fill it in.
    pub(crate) const BASE: PrefSpec = PrefSpec {
        id: "",
        pane: Pane::Hidden,
        category: None,
        section: None,
        label: None,
        help: None,
        control: ControlKind::Checkbox,
        default: ConstValue::Bool(false),
        parent: None,
        gate: Gate::None,
        layout: Layout::Normal,
        requires_restart: false,
        in_ui: true,
        cr_key: None,
        import: true,
        per_editor: false,
    };

    /// A preference is a value stored in `settings.toml`; a button is not.
    pub fn is_value(&self) -> bool {
        !matches!(self.control, ControlKind::Button { .. })
    }

    /// The default as a [`PrefValue`].
    pub fn default_value(&self) -> PrefValue {
        self.default.to_value()
    }

    /// The last dotted segment of the id.
    pub fn key(&self) -> &'static str {
        self.id.rsplit('.').next().unwrap_or(self.id)
    }

    /// The options of a popup or radio group.
    pub fn options(&self) -> &'static [PopupOption] {
        match self.control {
            ControlKind::Popup(options) | ControlKind::Radio(options) => options,
            _ => &[],
        }
    }

    /// The label with the `%@` placeholder removed, for clients that do not
    /// support inline controls.
    pub fn label_without_placeholder(&self) -> Option<String> {
        self.label
            .map(|label| label.replace(" %@", "").replace("%@", ""))
    }
}

/// The registry of all preference specs.
#[derive(Debug, Clone, Copy)]
pub struct PrefSchema;

struct Index {
    by_id: HashMap<&'static str, &'static PrefSpec>,
    by_cr_key: HashMap<&'static str, &'static PrefSpec>,
    by_bare: HashMap<&'static str, Vec<&'static PrefSpec>>,
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = Index {
            by_id: HashMap::new(),
            by_cr_key: HashMap::new(),
            by_bare: HashMap::new(),
        };
        for spec in PrefSchema::all() {
            index.by_id.insert(spec.id, spec);
            if let Some(key) = spec.cr_key {
                index.by_cr_key.entry(key).or_insert(spec);
            }
            if spec.is_value() {
                index.by_bare.entry(spec.key()).or_default().push(spec);
            }
        }
        index
    })
}

impl PrefSchema {
    /// Every entry, in display order (pane, then category, then plist order).
    pub fn all() -> impl Iterator<Item = &'static PrefSpec> {
        GROUPS.iter().flat_map(|group| group.iter())
    }

    /// The preferences (not the pure buttons), in display order.
    pub fn values() -> impl Iterator<Item = &'static PrefSpec> {
        Self::all().filter(|spec| spec.is_value())
    }

    /// The entry with exactly this id.
    pub fn get(id: &str) -> Option<&'static PrefSpec> {
        index().by_id.get(id).copied()
    }

    /// The entry for an id, or for a bare key such as `tab_width` when that
    /// key is unique among all preferences.
    pub fn resolve(id_or_key: &str) -> Option<&'static PrefSpec> {
        if let Some(spec) = Self::get(id_or_key) {
            return Some(spec);
        }
        match index().by_bare.get(id_or_key)?.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The entry whose CodeRunner key is `key`. The Font family entry owns
    /// `Font`.
    pub fn by_cr_key(key: &str) -> Option<&'static PrefSpec> {
        index().by_cr_key.get(key).copied()
    }

    /// The entries of a pane, in display order.
    pub fn pane(pane: Pane) -> impl Iterator<Item = &'static PrefSpec> {
        Self::all().filter(move |spec| spec.pane == pane)
    }

    /// The entries of a pane that have a control.
    pub fn visible(pane: Pane) -> impl Iterator<Item = &'static PrefSpec> {
        Self::pane(pane).filter(|spec| spec.in_ui)
    }

    /// The distinct categories of a pane in display order; `None` is the
    /// uncategorised entries.
    pub fn categories(pane: Pane) -> Vec<Option<&'static str>> {
        let mut categories = Vec::new();
        for spec in Self::visible(pane) {
            if !categories.contains(&spec.category) {
                categories.push(spec.category);
            }
        }
        categories
    }

    /// The entries of one category of a pane, in display order.
    pub fn category(
        pane: Pane,
        category: Option<&str>,
    ) -> impl Iterator<Item = &'static PrefSpec> {
        let category = category.map(str::to_owned);
        Self::visible(pane).filter(move |spec| spec.category == category.as_deref())
    }

    /// The entries nested directly under `parent`.
    pub fn children(parent: &str) -> impl Iterator<Item = &'static PrefSpec> {
        let parent = parent.to_owned();
        Self::all().filter(move |spec| spec.parent == Some(parent.as_str()))
    }
}
