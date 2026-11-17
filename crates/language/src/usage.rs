//! Usage bookkeeping kept outside the language definitions.
//!
//! CodeRunner stores `LastUsed` and `UseCount` inside each language's plist and
//! uses them to order the language menus. Here they live in a separate small
//! map ([`LanguageUsage`]) that the app persists wherever it likes (it is
//! `Serialize`/`Deserialize`), so language files stay pure definitions.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Use statistics of one language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEntry {
    /// How many times the language was used (a document set to it or run).
    #[serde(default)]
    pub use_count: u32,
    /// When it was last used, seconds since the Unix epoch.
    #[serde(default)]
    pub last_used: i64,
}

/// Use statistics by language id.
///
/// ```
/// use scratch_language::LanguageUsage;
///
/// let mut usage = LanguageUsage::default();
/// usage.record_at("python", 100);
/// usage.record_at("python", 200);
/// usage.record_at("go", 300);
/// assert_eq!(usage.get("python").use_count, 2);
/// assert_eq!(usage.most_recent("go", "python"), std::cmp::Ordering::Greater);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LanguageUsage {
    entries: BTreeMap<String, UsageEntry>,
}

impl LanguageUsage {
    /// The statistics of `id` (zeros when never used).
    pub fn get(&self, id: &str) -> UsageEntry {
        self.entries.get(id).copied().unwrap_or_default()
    }

    /// Records a use of `id` now.
    pub fn record(&mut self, id: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        self.record_at(id, now);
    }

    /// Records a use of `id` at `unix_seconds` (the time never goes backwards
    /// for an id).
    pub fn record_at(&mut self, id: &str, unix_seconds: i64) {
        let entry = self.entries.entry(id.to_owned()).or_default();
        entry.use_count = entry.use_count.saturating_add(1);
        entry.last_used = entry.last_used.max(unix_seconds);
    }

    /// Forgets `id` (a deleted language).
    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    /// Compares two ids by recency: `Greater` when `a` was used more recently
    /// than `b`; ties fall back to the use count.
    pub fn most_recent(&self, a: &str, b: &str) -> std::cmp::Ordering {
        let (a, b) = (self.get(a), self.get(b));
        a.last_used
            .cmp(&b.last_used)
            .then(a.use_count.cmp(&b.use_count))
    }

    /// Compares two ids by use count: `Greater` when `a` was used more; ties
    /// fall back to recency.
    pub fn most_used(&self, a: &str, b: &str) -> std::cmp::Ordering {
        let (a, b) = (self.get(a), self.get(b));
        a.use_count
            .cmp(&b.use_count)
            .then(a.last_used.cmp(&b.last_used))
    }

    /// The id used most recently among `ids`, if any was ever used.
    pub fn last_used_of<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
        ids.into_iter()
            .filter(|id| self.get(id).use_count > 0)
            .max_by(|a, b| self.most_recent(a, b))
    }

    /// Serializes as TOML (`python = { use_count = 3, last_used = 1700000000 }`).
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }

    /// Parses the TOML form.
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    #[test]
    fn counts_and_recency() {
        let mut u = LanguageUsage::default();
        assert_eq!(u.get("x"), UsageEntry::default());
        u.record_at("a", 10);
        u.record_at("a", 5);
        u.record_at("b", 20);
        assert_eq!(
            u.get("a"),
            UsageEntry {
                use_count: 2,
                last_used: 10
            }
        );
        assert_eq!(u.most_recent("b", "a"), Ordering::Greater);
        assert_eq!(u.most_used("a", "b"), Ordering::Greater);
        assert_eq!(u.last_used_of(["a", "b", "never"]), Some("b"));
        assert_eq!(u.last_used_of(["never"]), None);
        u.remove("b");
        assert_eq!(u.get("b").use_count, 0);
    }

    #[test]
    fn toml_round_trip() {
        let mut u = LanguageUsage::default();
        u.record_at("python", 1700000000);
        let text = u.to_toml_string().unwrap();
        assert_eq!(LanguageUsage::from_toml_str(&text).unwrap(), u);
        assert!(LanguageUsage::from_toml_str("").unwrap().is_empty());
    }

    #[test]
    fn record_uses_the_clock() {
        let mut u = LanguageUsage::default();
        u.record("x");
        assert!(u.get("x").last_used > 1_600_000_000);
    }
}
