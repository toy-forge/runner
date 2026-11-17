//! Finder-like name ordering.

use std::cmp::Ordering;

use super::entry::{Entry, EntryRef};

/// Case-insensitive "natural" comparison: digit runs compare by value
/// (`file2` < `file10`), like `localizedStandardCompare:`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let (lx, ly) = (
                    x.to_lowercase().next().unwrap_or(x),
                    y.to_lowercase().next().unwrap_or(y),
                );
                if lx != ly {
                    return lx.cmp(&ly);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// How the navigator orders and filters siblings (View Options in the context menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub struct ViewOptions {
    /// "Show Hidden Files" (CodeRunner pref `ShowHiddenFiles`, default off).
    pub show_hidden: bool,
    /// "Sort Folders on Top" (CodeRunner pref `SortFoldersOnTop`, default off).
    pub folders_first: bool,
}


impl ViewOptions {
    /// Orders two siblings.
    pub fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        if self.folders_first && a.is_dir() != b.is_dir() {
            return if a.is_dir() {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        natural_cmp(&a.name, &b.name)
    }

    /// Sorts `entries` in place.
    pub fn sort(&self, entries: &mut [EntryRef]) {
        entries.sort_by(|a, b| self.compare(a, b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_compare_by_value_and_case_is_ignored() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("File2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("a", "B"), Ordering::Less);
        assert_eq!(natural_cmp("x007", "x7"), "x007".cmp("x7"));
        assert_eq!(natural_cmp("img01", "img1"), "img01".cmp("img1"));
    }
}
