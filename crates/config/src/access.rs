//! Reading, writing, validating and diffing preferences by id.

use thiserror::Error;
use toml::{Table, Value};

use crate::model::Preferences;
use crate::schema::{ControlKind, PrefId, PrefSchema, PrefSpec};

/// Why a preference could not be read or changed.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PrefError {
    /// No preference has this id.
    #[error("unknown preference `{0}`")]
    Unknown(String),
    /// The id names an action button, not a value.
    #[error("`{0}` is an action, not a preference")]
    NotAValue(String),
    /// The value has the wrong type.
    #[error("`{id}` expects {expected}, got {got}")]
    WrongType {
        /// The preference.
        id: String,
        /// The type that was expected.
        expected: &'static str,
        /// The type that was given.
        got: &'static str,
    },
    /// A number outside its range.
    #[error("`{id}` must be between {min} and {max}")]
    OutOfRange {
        /// The preference.
        id: String,
        /// The smallest allowed value.
        min: f64,
        /// The largest allowed value.
        max: f64,
    },
    /// A value that is not one of the options.
    #[error("`{id}`: {value} is not one of the options")]
    NotAnOption {
        /// The preference.
        id: String,
        /// The rejected value.
        value: String,
    },
    /// Any other rejection.
    #[error("`{id}`: {message}")]
    Invalid {
        /// The preference.
        id: String,
        /// What is wrong.
        message: String,
    },
}

pub(crate) fn type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "a string",
        Value::Integer(_) => "an integer",
        Value::Float(_) => "a number",
        Value::Boolean(_) => "a boolean",
        Value::Datetime(_) => "a datetime",
        Value::Array(_) => "an array",
        Value::Table(_) => "a table",
    }
}

/// Equality that treats `12` and `12.0` as the same number.
pub(crate) fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer(i), Value::Float(f)) | (Value::Float(f), Value::Integer(i)) => {
            (*i as f64) == *f
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| values_equal(a, b))
        }
        (Value::Table(x), Value::Table(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| values_equal(v, w)))
        }
        _ => a == b,
    }
}

/// The value at a dotted path.
pub(crate) fn lookup<'a>(table: &'a Table, id: &str) -> Option<&'a Value> {
    let mut parts = id.split('.');
    let mut current = table.get(parts.next()?)?;
    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

/// Sets the value at a dotted path, creating tables on the way.
pub(crate) fn insert(table: &mut Table, id: &str, value: Value) {
    match id.split_once('.') {
        None => {
            table.insert(id.to_owned(), value);
        }
        Some((head, rest)) => {
            let child = table
                .entry(head.to_owned())
                .or_insert_with(|| Value::Table(Table::new()));
            if !child.is_table() {
                *child = Value::Table(Table::new());
            }
            if let Value::Table(inner) = child {
                insert(inner, rest, value);
            }
        }
    }
}

/// Checks `value` against the spec and returns it normalised (an integer
/// given for a float preference becomes a float and the reverse for whole
/// numbers).
pub(crate) fn validate(spec: &PrefSpec, value: Value) -> Result<Value, PrefError> {
    let id = spec.id.to_owned();
    let wrong = |expected: &'static str, got: &Value| PrefError::WrongType {
        id: id.clone(),
        expected,
        got: type_name(got),
    };
    match spec.control {
        ControlKind::Button { .. } => Err(PrefError::NotAValue(id)),
        ControlKind::Checkbox => match value {
            Value::Boolean(_) => Ok(value),
            other => Err(wrong("a boolean", &other)),
        },
        ControlKind::Radio(options) | ControlKind::Popup(options) => {
            let found = options
                .iter()
                .filter(|o| !o.separator)
                .any(|o| values_equal(&o.value.to_value(), &value));
            if found {
                Ok(value)
            } else {
                Err(PrefError::NotAnOption {
                    id,
                    value: value.to_string(),
                })
            }
        }
        ControlKind::DynamicPopup { .. } | ControlKind::Text { .. } | ControlKind::EditButton { .. } | ControlKind::Path { .. } => {
            match value {
                Value::String(_) => Ok(value),
                other => Err(wrong("a string", &other)),
            }
        }
        ControlKind::Font { .. } => match value {
            Value::String(ref s) if !s.trim().is_empty() => Ok(value),
            Value::String(_) => Err(PrefError::Invalid {
                id,
                message: "the font name is empty".to_owned(),
            }),
            other => Err(wrong("a string", &other)),
        },
        ControlKind::Number { min, max, integer, .. } => {
            let number = match &value {
                Value::Integer(i) => *i as f64,
                Value::Float(f) => *f,
                other => return Err(wrong("a number", other)),
            };
            if number.is_nan() || number < min || number > max {
                return Err(PrefError::OutOfRange { id, min, max });
            }
            if integer {
                if number.fract() != 0.0 {
                    return Err(wrong("an integer", &value));
                }
                Ok(Value::Integer(number as i64))
            } else {
                Ok(Value::Float(number))
            }
        }
        ControlKind::KeyValueTable(_) => match &value {
            Value::Table(t) if t.values().all(Value::is_str) => Ok(value),
            Value::Table(_) => Err(PrefError::Invalid {
                id,
                message: "every value must be a string".to_owned(),
            }),
            other => Err(wrong("a table", other)),
        },
        ControlKind::Color | ControlKind::Custom(_) => {
            let default = spec.default_value();
            if std::mem::discriminant(&default) == std::mem::discriminant(&value) {
                Ok(value)
            } else {
                Err(wrong(type_name(&default), &value))
            }
        }
    }
}

impl Preferences {
    /// The preferences as a TOML table (every key, defaults included).
    pub fn to_table(&self) -> Table {
        match Value::try_from(self) {
            Ok(Value::Table(table)) => table,
            _ => Table::new(),
        }
    }

    /// Builds preferences from a table; missing keys take their defaults.
    ///
    /// # Errors
    ///
    /// The deserialisation error when a value has the wrong shape. Use
    /// [`Loaded::parse`](crate::Loaded::parse) for tolerant loading.
    pub fn from_table(table: Table) -> Result<Self, toml::de::Error> {
        Value::Table(table).try_into()
    }

    /// The value of a preference, by full id or by a bare key that is unique
    /// (`tab_width`). `None` for unknown ids and action buttons.
    ///
    /// This serialises the tree; read typed fields directly in hot paths.
    pub fn get(&self, id: &str) -> Option<Value> {
        let spec = PrefSchema::resolve(id)?;
        if !spec.is_value() {
            return None;
        }
        lookup(&self.to_table(), spec.id).cloned()
    }

    /// Changes a preference after validating the value against its schema
    /// entry (type, range, options). On error nothing changes.
    ///
    /// # Errors
    ///
    /// [`PrefError`] describing why the value was rejected.
    pub fn set(&mut self, id: &str, value: Value) -> Result<(), PrefError> {
        let spec = PrefSchema::resolve(id).ok_or_else(|| PrefError::Unknown(id.to_owned()))?;
        let value = validate(spec, value)?;
        let mut table = self.to_table();
        insert(&mut table, spec.id, value);
        let updated = Self::from_table(table).map_err(|e| PrefError::Invalid {
            id: spec.id.to_owned(),
            message: e.message().to_owned(),
        })?;
        *self = updated;
        Ok(())
    }

    /// Resets one preference to its default.
    ///
    /// # Errors
    ///
    /// [`PrefError::Unknown`] or [`PrefError::NotAValue`].
    pub fn reset(&mut self, id: &str) -> Result<(), PrefError> {
        let spec = PrefSchema::resolve(id).ok_or_else(|| PrefError::Unknown(id.to_owned()))?;
        self.set(spec.id, spec.default_value())
    }

    /// Whether a preference has its default value.
    pub fn is_default(&self, id: &str) -> bool {
        let Some(spec) = PrefSchema::resolve(id) else {
            return false;
        };
        self.get(spec.id)
            .is_some_and(|v| values_equal(&v, &spec.default_value()))
    }

    /// The ids of the preferences whose values differ, in schema order.
    pub fn diff(old: &Preferences, new: &Preferences) -> Vec<PrefId> {
        let (a, b) = (old.to_table(), new.to_table());
        PrefSchema::values()
            .filter(|spec| match (lookup(&a, spec.id), lookup(&b, spec.id)) {
                (Some(x), Some(y)) => !values_equal(x, y),
                (None, None) => false,
                _ => true,
            })
            .map(|spec| spec.id)
            .collect()
    }
}
