//! Stable identities for documents.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A document's identity. It is assigned when the document is created and
/// kept for its life, including across restarts; it names the draft and the
/// unsaved-store manifest of the document and ties session tabs to documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(uuid::Uuid);

impl DocumentId {
    /// A new, random identity.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// The identity of a nil UUID. Only useful as a placeholder in tests.
    pub fn nil() -> Self {
        Self(uuid::Uuid::nil())
    }
}

impl Default for DocumentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for DocumentId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        uuid::Uuid::parse_str(text).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_and_parses_back() {
        let id = DocumentId::new();
        assert_eq!(id.to_string().parse::<DocumentId>().unwrap(), id);
        assert!("not-a-uuid".parse::<DocumentId>().is_err());
    }

    #[test]
    fn serializes_as_a_plain_string() {
        let id = DocumentId::nil();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"00000000-0000-0000-0000-000000000000\""
        );
    }
}
