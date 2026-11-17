//! The syntax tree of a compound command and its JSON parser / printer.

use std::sync::Arc;

use serde_json::Value;

use super::{CompoundError, lenient};

/// An argument of a command call.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// A string.
    Text(String),
    /// A number.
    Number(f64),
    /// A list of arguments (for example `["{", "}"]`).
    List(Vec<Arg>),
    /// A command list (arguments of `and:`, `or:`, `negate:`).
    Commands(Arc<CommandList>),
}

impl Arg {
    /// The text of a [`Arg::Text`].
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Arg::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The number of a [`Arg::Number`] (or of a numeric [`Arg::Text`]).
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Arg::Number(n) => Some(*n),
            Arg::Text(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
}

/// One command with its arguments: `"insertText:", "x"` is `Call { name: "insertText:", args: [Text("x")] }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The command name, including colons.
    pub name: String,
    /// One argument per colon in the name.
    pub args: Vec<Arg>,
}

/// One element of a command list.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A command call.
    Call(Call),
    /// A nested command list (its own failure scope).
    List(Arc<CommandList>),
    /// An object inside a list: wait for a keystroke and run the matching entry.
    Wait(Arc<KeyTable>),
}

/// A command list: runs its items in order; a failing test aborts it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandList {
    /// The items.
    pub items: Vec<Item>,
}

/// Key equivalent strings mapped to command lists (a JSON object).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyTable {
    /// The entries in source order.
    pub entries: Vec<KeyEntry>,
}

/// One entry of a [`KeyTable`].
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEntry {
    /// The key equivalent (`"j"`, `"\r"`, `"any"`) or a custom name for `callCompoundCommand:`.
    pub key: String,
    /// What to run.
    pub list: Arc<CommandList>,
}

/// The root of a compound command.
#[derive(Debug, Clone, PartialEq)]
pub enum Root {
    /// A command list.
    List(Arc<CommandList>),
    /// Entry points by key equivalent or name.
    Entries(Arc<KeyTable>),
}

/// A parsed compound command.
///
/// ```
/// use scratch_commands::compound::CompoundCommand;
/// let c = CompoundCommand::parse(r#"[ "testNotEditable", "insertText:", "x", ]"#).unwrap();
/// assert!(c.entry(None).is_some());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CompoundCommand {
    /// The root.
    pub root: Root,
}

/// Commands whose list argument is a command list rather than data.
fn takes_command_list(name: &str) -> bool {
    matches!(name, "and:" | "or:" | "negate:")
}

impl CompoundCommand {
    /// Parses the source text (JSON with comments and trailing commas).
    pub fn parse(source: &str) -> Result<CompoundCommand, CompoundError> {
        CompoundCommand::from_json(&lenient::parse(source)?)
    }

    /// Builds a compound command from parsed JSON (a list or an object).
    pub fn from_json(v: &Value) -> Result<CompoundCommand, CompoundError> {
        match v {
            Value::Array(a) => Ok(CompoundCommand { root: Root::List(Arc::new(parse_list(a, "$")?)) }),
            Value::Object(o) => Ok(CompoundCommand { root: Root::Entries(Arc::new(parse_table(o, "$")?)) }),
            _ => Err(CompoundError::BadRoot),
        }
    }

    /// The command list to run: the root list when `entry` is `None`, or the entry of a
    /// key table by key equivalent / name.
    pub fn entry(&self, entry: Option<&str>) -> Option<Arc<CommandList>> {
        match (&self.root, entry) {
            (Root::List(l), None) => Some(l.clone()),
            (Root::Entries(t), Some(k)) => t.entries.iter().find(|e| e.key == k).map(|e| e.list.clone()),
            _ => None,
        }
    }

    /// The entries of a key-table root (empty for a list root).
    pub fn entries(&self) -> &[KeyEntry] {
        match &self.root {
            Root::Entries(t) => &t.entries,
            Root::List(_) => &[],
        }
    }

    /// The compound command as JSON (what `compoundCommandArray` / `compoundCommandDictionary` cache).
    pub fn to_json(&self) -> Value {
        match &self.root {
            Root::List(l) => list_json(l),
            Root::Entries(t) => table_json(t),
        }
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

fn parse_list(a: &[Value], path: &str) -> Result<CommandList, CompoundError> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < a.len() {
        let here = format!("{path}[{i}]");
        match &a[i] {
            Value::String(name) => {
                let n = name.matches(':').count();
                let mut args = Vec::with_capacity(n);
                for k in 0..n {
                    let Some(v) = a.get(i + 1 + k) else {
                        return Err(CompoundError::MissingArgument { command: name.clone(), index: k + 1, expected: n });
                    };
                    args.push(parse_arg(v, name, &format!("{path}[{}]", i + 1 + k))?);
                }
                i += n;
                items.push(Item::Call(Call { name: name.clone(), args }));
            }
            Value::Array(sub) => items.push(Item::List(Arc::new(parse_list(sub, &here)?))),
            Value::Object(o) => items.push(Item::Wait(Arc::new(parse_table(o, &here)?))),
            other => {
                return Err(CompoundError::Structure { path: here, expected: "a command name, list or object", found: kind(other) });
            }
        }
        i += 1;
    }
    Ok(CommandList { items })
}

fn parse_arg(v: &Value, command: &str, path: &str) -> Result<Arg, CompoundError> {
    match v {
        Value::String(s) => Ok(Arg::Text(s.clone())),
        Value::Number(n) => Ok(Arg::Number(n.as_f64().unwrap_or(0.0))),
        Value::Bool(b) => Ok(Arg::Number(if *b { 1.0 } else { 0.0 })),
        Value::Array(a) if takes_command_list(command) => Ok(Arg::Commands(Arc::new(parse_list(a, path)?))),
        Value::Array(a) => a.iter().enumerate().map(|(i, v)| parse_arg(v, command, &format!("{path}[{i}]"))).collect::<Result<Vec<_>, _>>().map(Arg::List),
        other => Err(CompoundError::Structure { path: path.to_owned(), expected: "a string, number or list argument", found: kind(other) }),
    }
}

fn parse_table(o: &serde_json::Map<String, Value>, path: &str) -> Result<KeyTable, CompoundError> {
    let mut entries = Vec::with_capacity(o.len());
    for (k, v) in o {
        let here = format!("{path}.{k}");
        match v {
            Value::Array(a) => entries.push(KeyEntry { key: k.clone(), list: Arc::new(parse_list(a, &here)?) }),
            other => return Err(CompoundError::Structure { path: here, expected: "a command list", found: kind(other) }),
        }
    }
    Ok(KeyTable { entries })
}

fn arg_json(a: &Arg) -> Value {
    match a {
        Arg::Text(s) => Value::String(s.clone()),
        Arg::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => Value::from(*n as i64),
        Arg::Number(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
        Arg::List(l) => Value::Array(l.iter().map(arg_json).collect()),
        Arg::Commands(c) => list_json(c),
    }
}

fn list_json(l: &CommandList) -> Value {
    let mut out = Vec::new();
    for item in &l.items {
        match item {
            Item::Call(c) => {
                out.push(Value::String(c.name.clone()));
                out.extend(c.args.iter().map(arg_json));
            }
            Item::List(sub) => out.push(list_json(sub)),
            Item::Wait(t) => out.push(table_json(t)),
        }
    }
    Value::Array(out)
}

fn table_json(t: &KeyTable) -> Value {
    Value::Object(t.entries.iter().map(|e| (e.key.clone(), list_json(&e.list))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_follow_colons() {
        let c = CompoundCommand::parse(r#"["moveLeft", "insertText:", "hello", "surroundSelectionWithString:endString:", "(", ")", "beep"]"#).unwrap();
        let l = c.entry(None).unwrap();
        assert_eq!(l.items.len(), 4);
        match &l.items[2] {
            Item::Call(call) => {
                assert_eq!(call.name, "surroundSelectionWithString:endString:");
                assert_eq!(call.args, [Arg::Text("(".into()), Arg::Text(")".into())]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_argument_is_an_error() {
        let e = CompoundCommand::parse(r#"["insertText:"]"#).unwrap_err();
        assert!(matches!(e, CompoundError::MissingArgument { .. }), "{e}");
    }

    #[test]
    fn nested_lists_objects_and_list_args() {
        let c = CompoundCommand::parse(r#"["testSucceedingText:", ["{", "}"], ["a"], {"x": ["b"], "any": []}, "negate:", ["testHasMultipleSelections"]]"#).unwrap();
        let l = c.entry(None).unwrap();
        assert_eq!(l.items.len(), 4);
        assert!(matches!(&l.items[0], Item::Call(Call { args, .. }) if matches!(&args[0], Arg::List(v) if v.len() == 2)));
        assert!(matches!(&l.items[1], Item::List(_)));
        assert!(matches!(&l.items[2], Item::Wait(t) if t.entries.len() == 2));
        assert!(matches!(&l.items[3], Item::Call(Call { args, .. }) if matches!(&args[0], Arg::Commands(_))));
    }

    #[test]
    fn dictionary_root() {
        let c = CompoundCommand::parse(r#"{"{": ["a"], "\r": ["b"]}"#).unwrap();
        assert!(c.entry(None).is_none());
        assert!(c.entry(Some("\r")).is_some());
        assert_eq!(c.entries().len(), 2);
        assert_eq!(c.entries()[0].key, "{");
    }

    #[test]
    fn bad_roots_and_items() {
        assert!(matches!(CompoundCommand::parse("3"), Err(CompoundError::BadRoot)));
        assert!(matches!(CompoundCommand::parse("[1]"), Err(CompoundError::Structure { .. })));
        assert!(matches!(CompoundCommand::parse(r#"{"a": 1}"#), Err(CompoundError::Structure { .. })));
        assert!(matches!(CompoundCommand::parse("["), Err(CompoundError::Json { .. })));
    }

    #[test]
    fn json_roundtrip() {
        let src = r#"["a", "setNumber:withName:", 3, "n", ["b", {"x": ["c"]}], "testPrecedingText:", ["p", "q"]]"#;
        let c = CompoundCommand::parse(src).unwrap();
        let again = CompoundCommand::from_json(&c.to_json()).unwrap();
        assert_eq!(c, again);
        assert_eq!(c.to_json(), serde_json::from_str::<Value>(src).unwrap());
    }
}
