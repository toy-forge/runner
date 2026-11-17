//! Variable tree model, node ids and the three sort modes.
//!
//! Variables arrive as a tree.  Every node has a stable [`VariableId`]-a path
//! of child indices such as `0.2.1`-so the UI can ask for exactly one node to
//! be expanded without knowing the debugger's own expression syntax.  The
//! optional `expr` field carries the debugger expression needed to evaluate or
//! assign the variable (`p['x']`, `it.pos.a`).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::model::FrameId;

/// What role a variable plays in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    /// A function argument (`ARG:` in LLDB, pdb's `args`).
    Parameter,
    /// A local variable.
    Variable,
    /// A file-static variable.
    Static,
    /// A global variable.
    Global,
    /// A function return value.
    ReturnValue,
    /// A child of an aggregate.
    Member,
    /// A kind we could not classify.
    Other,
}

impl VariableKind {
    /// Parse the spelling used by helpers and adapter parsers.
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "parameter" | "arg" | "argument" => VariableKind::Parameter,
            "variable" | "local" => VariableKind::Variable,
            "static" => VariableKind::Static,
            "global" => VariableKind::Global,
            "return_value" | "return" => VariableKind::ReturnValue,
            "member" | "field" => VariableKind::Member,
            _ => VariableKind::Other,
        }
    }
}

/// Stable id of a variable node: the indices from the roots to the node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VariableId(pub Vec<u32>);

impl VariableId {
    /// Id of a root node.
    pub fn root(index: usize) -> Self {
        Self(vec![index as u32])
    }

    /// Extend an id with a child index.
    pub fn child(&self, index: usize) -> Self {
        let mut path = self.0.clone();
        path.push(index as u32);
        Self(path)
    }

    /// Raw path.
    pub fn path(&self) -> &[u32] {
        &self.0
    }

    /// Depth below the roots (a root has depth 0).
    pub fn depth(&self) -> usize {
        self.0.len().saturating_sub(1)
    }
}

impl fmt::Display for VariableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, part) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

impl FromStr for VariableId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("empty variable id".into());
        }
        let mut path = Vec::new();
        for part in s.split('.') {
            let index: u32 = part
                .parse()
                .map_err(|_| format!("invalid variable id component `{part}`"))?;
            path.push(index);
        }
        Ok(Self(path))
    }
}

/// One variable (or aggregate member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    /// Stable tree id, assigned by [`VariableTree::assign_ids`].
    pub id: VariableId,
    /// Name as shown in the outline (for children: `x`, `[0]`, `*argv`).
    pub name: String,
    /// Display name when it differs from `name`.
    pub display_name: String,
    /// Type name when the debugger reports one.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Role.
    pub kind: VariableKind,
    /// Leaf value (a string as printed by the debugger / helper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Short description, e.g. a container summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Raw value (address, id, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_value: Option<String>,
    /// Debugger expression that evaluates to this variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// Children fetched with this query.
    #[serde(default)]
    pub children: Vec<Variable>,
    /// There are more children, fetchable with `ExpandVariable`.
    #[serde(default)]
    pub expandable: bool,
    /// The helper capped the child list.
    #[serde(default)]
    pub truncated: bool,
}

impl Variable {
    /// Create a leaf variable.
    pub fn leaf(name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        Self {
            id: VariableId(vec![0]),
            display_name: name.clone(),
            name,
            type_name: None,
            kind: VariableKind::Variable,
            value: Some(value),
            description: None,
            raw_value: None,
            expr: None,
            children: Vec::new(),
            expandable: false,
            truncated: false,
        }
    }

    /// True when the node has or can fetch children.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty() || self.expandable
    }

    /// Find a node by id.
    pub fn find(&self, id: &VariableId) -> Option<&Variable> {
        let mut node = self;
        for component in id.path() {
            node = node.children.get(*component as usize)?;
        }
        Some(node)
    }

    /// Find a mutable node by id.
    pub fn find_mut(&mut self, id: &VariableId) -> Option<&mut Variable> {
        let mut node = self;
        for component in id.path() {
            node = node.children.get_mut(*component as usize)?;
        }
        Some(node)
    }
}

/// A frame's variable roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableTree {
    /// Root variables.
    pub roots: Vec<Variable>,
    /// Frame these variables belong to.
    pub frame: FrameId,
    /// Some children were capped and can be fetched lazily.
    pub truncated: bool,
}

impl VariableTree {
    /// Build a tree and assign ids.
    pub fn new(frame: FrameId, roots: Vec<Variable>) -> Self {
        let mut tree = Self {
            roots,
            frame,
            truncated: false,
        };
        tree.assign_ids();
        tree.truncated = tree.roots.iter().any(|r| r.truncated);
        tree
    }

    /// An empty tree.
    pub fn empty(frame: FrameId) -> Self {
        Self {
            roots: Vec::new(),
            frame,
            truncated: false,
        }
    }

    /// Assign ids from the current child indices.
    pub fn assign_ids(&mut self) {
        fn walk(node: &mut Variable, id: VariableId) {
            node.id = id.clone();
            for (index, child) in node.children.iter_mut().enumerate() {
                walk(child, id.child(index));
            }
        }
        for (index, root) in self.roots.iter_mut().enumerate() {
            walk(root, VariableId::root(index));
        }
    }

    /// Find a node by id.
    pub fn find(&self, id: &VariableId) -> Option<&Variable> {
        let (first, rest) = id.path().split_first()?;
        let root = self.roots.get(*first as usize)?;
        if rest.is_empty() {
            Some(root)
        } else {
            root.find(&VariableId(rest.to_vec()))
        }
    }

    /// Sort all levels with `mode`.
    pub fn sort(&mut self, mode: SortMode) {
        sort_variables(&mut self.roots, mode);
    }

    /// Replace the children of `parent` with an expanded subtree.
    ///
    /// The subtree roots become the children of the parent and get fresh ids
    /// on the next [`Self::assign_ids`].  Returns false when the parent is
    /// unknown.
    pub fn replace_children(&mut self, parent: &VariableId, children: Vec<Variable>) -> bool {
        let Some(node) = self.find_mut(parent) else {
            return false;
        };
        node.expandable = false;
        node.truncated = children.iter().any(|c| c.truncated);
        node.children = children;
        self.assign_ids();
        true
    }

    fn find_mut(&mut self, id: &VariableId) -> Option<&mut Variable> {
        let (first, rest) = id.path().split_first()?;
        let root = self.roots.get_mut(*first as usize)?;
        if rest.is_empty() {
            Some(root)
        } else {
            root.find_mut(&VariableId(rest.to_vec()))
        }
    }

    /// True when there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

/// Variables outline order.  The numeric values are CodeRunner's
/// `DebuggerVariablesSortMode` (research 10, G37).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SortMode {
    /// 0: lexicographic by name.
    Name,
    /// 1: natural (numeric-aware) ascending; the default.
    #[default]
    Natural,
    /// 2: natural descending.
    ReverseNatural,
}

impl SortMode {
    /// The preference value stored by the UI.
    pub fn code(self) -> u8 {
        match self {
            SortMode::Name => 0,
            SortMode::Natural => 1,
            SortMode::ReverseNatural => 2,
        }
    }

    /// Parse a stored preference value.
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => SortMode::Name,
            2 => SortMode::ReverseNatural,
            _ => SortMode::Natural,
        }
    }

    /// Menu title.
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Name => "Name",
            SortMode::Natural => "Natural Order",
            SortMode::ReverseNatural => "Reverse Natural Order",
        }
    }
}

/// Sort roots and every nested child list.
pub fn sort_variables(variables: &mut [Variable], mode: SortMode) {
    let cmp: fn(&Variable, &Variable) -> Ordering = match mode {
        SortMode::Name => compare_name,
        SortMode::Natural => |a, b| natural_cmp(&a.display_name, &b.display_name),
        SortMode::ReverseNatural => {
            |a, b| natural_cmp(&b.display_name, &a.display_name)
        }
    };
    variables.sort_by(cmp);
    for variable in variables {
        sort_variables(&mut variable.children, mode);
    }
}

/// Clamp long `value`/`description` strings, marking affected nodes as
/// truncated.  `max_chars == 0` disables clamping.  Raw values are preserved.
pub fn truncate_values(variables: &mut [Variable], max_chars: usize) {
    if max_chars == 0 {
        return;
    }
    for variable in variables {
        for field in [
            &mut variable.value,
            &mut variable.description,
        ] {
            let Some(text) = field else { continue };
            if text.chars().count() <= max_chars {
                continue;
            }
            let keep = max_chars.saturating_sub(1);
            *text = text.chars().take(keep).collect::<String>() + "\u{2026}";
            variable.truncated = true;
        }
        truncate_values(&mut variable.children, max_chars);
    }
}

fn compare_name(a: &Variable, b: &Variable) -> Ordering {
    a.display_name
        .to_ascii_lowercase()
        .cmp(&b.display_name.to_ascii_lowercase())
        .then_with(|| a.display_name.cmp(&b.display_name))
}

/// Compare two strings treating digit runs as numbers ("x2" < "x10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a_bytes, b_bytes) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a_bytes.len() && j < b_bytes.len() {
        let ca = a[i..].chars().next().unwrap_or('\0');
        let cb = b[j..].chars().next().unwrap_or('\0');
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let start_a = i;
            while i < a_bytes.len() && a_bytes[i].is_ascii_digit() {
                i += 1;
            }
            let start_b = j;
            while j < b_bytes.len() && b_bytes[j].is_ascii_digit() {
                j += 1;
            }
            let digits_a = a[start_a..i].trim_start_matches('0');
            let digits_b = b[start_b..j].trim_start_matches('0');
            let cmp = digits_a
                .len()
                .cmp(&digits_b.len())
                .then_with(|| digits_a.cmp(digits_b));
            if cmp != Ordering::Equal {
                return cmp;
            }
            let raw = a[start_a..i].len().cmp(&b[start_b..j].len());
            if raw != Ordering::Equal {
                return raw;
            }
        } else {
            let lower_a = ca.to_ascii_lowercase();
            let lower_b = cb.to_ascii_lowercase();
            if lower_a != lower_b {
                return lower_a.cmp(&lower_b);
            }
            i += ca.len_utf8();
            j += cb.len_utf8();
        }
    }
    (a_bytes.len() - i).cmp(&(b_bytes.len() - j))
}

/// One watch expression's value at a stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchValue {
    /// Expression that was evaluated.
    pub expression: String,
    /// Result text, when evaluation succeeded.
    pub value: Option<String>,
    /// Error text, when it failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> Variable {
        Variable::leaf(name, "1")
    }

    #[test]
    fn natural_order_compares_number_runs() {
        assert_eq!(natural_cmp("x2", "x10"), Ordering::Less);
        assert_eq!(natural_cmp("x010", "x10"), Ordering::Greater);
        assert_eq!(natural_cmp("a1b2", "a1b10"), Ordering::Less);
        assert_eq!(natural_cmp("Alpha", "alpha"), Ordering::Equal);
        assert_eq!(natural_cmp("b", "a10"), Ordering::Greater);
    }

    #[test]
    fn sorts_every_level() {
        let mut tree = VariableTree::new(
            FrameId::main(),
            vec![var("z10"), var("z2"), var("a")],
        );
        tree.roots[0].children = vec![var("c10"), var("c2")];
        tree.assign_ids();
        tree.sort(SortMode::Natural);
        let names: Vec<_> = tree.roots.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["a", "z2", "z10"]);
        let child_names: Vec<_> = tree.roots[2].children.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(child_names, ["c2", "c10"]);
        tree.sort(SortMode::ReverseNatural);
        let names: Vec<_> = tree.roots.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["z10", "z2", "a"]);
    }

    #[test]
    fn ids_are_stable_paths_and_can_replace_children() {
        let mut tree = VariableTree::new(FrameId::main(), vec![var("a"), var("b")]);
        tree.roots[1].children = vec![var("x"), var("y")];
        tree.assign_ids();
        assert_eq!(tree.roots[1].children[0].id.to_string(), "1.0");
        assert_eq!(tree.find(&"1.1".parse().unwrap()).unwrap().name, "y");
        assert!(tree.replace_children(&"1".parse().unwrap(), vec![var("z")]));
        assert_eq!(tree.roots[1].children[0].id.to_string(), "1.0");
        assert_eq!(tree.roots[1].children[0].name, "z");
    }

    #[test]
    fn truncation_marks_nodes_and_spares_raw_values() {
        let mut parent = Variable::leaf("big", "abcdefghij");
        parent.raw_value = Some("abcdefghij".into());
        parent.children = vec![Variable::leaf("child", "abcdefghij")];
        let mut roots = vec![parent, Variable::leaf("small", "abc")];
        truncate_values(&mut roots, 5);
        assert_eq!(roots[0].value.as_deref(), Some("abcd\u{2026}"));
        assert!(roots[0].truncated);
        assert_eq!(roots[0].raw_value.as_deref(), Some("abcdefghij"));
        assert_eq!(roots[0].children[0].value.as_deref(), Some("abcd\u{2026}"));
        assert!(roots[0].children[0].truncated);
        assert_eq!(roots[1].value.as_deref(), Some("abc"));
        assert!(!roots[1].truncated);
    }

    #[test]
    fn kind_parsing() {
        assert_eq!(VariableKind::parse("ARG"), VariableKind::Parameter);
        assert_eq!(VariableKind::parse("global"), VariableKind::Global);
        assert_eq!(VariableKind::parse("weird"), VariableKind::Other);
    }

    #[test]
    fn sort_mode_codes_match_the_preference() {
        assert_eq!(SortMode::Name.code(), 0);
        assert_eq!(SortMode::Natural.code(), 1);
        assert_eq!(SortMode::ReverseNatural.code(), 2);
        assert_eq!(SortMode::from_code(2), SortMode::ReverseNatural);
    }
}
