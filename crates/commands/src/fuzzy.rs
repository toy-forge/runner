//! A small fuzzy matcher for command search (QuickNav's `mainMenu` provider).
//!
//! [`fuzzy_match`] returns a score and the matched character positions so the UI can
//! highlight them. Matching is a subsequence match; the score rewards contiguous runs,
//! word/delimiter boundaries, a match at the start and an exact prefix, and penalises gaps.
//! Lower-case queries match case-insensitively; a query with an upper-case character is
//! case-sensitive ("smart case").
//!
//! ```
//! use scratch_commands::fuzzy::fuzzy_match;
//! let m = fuzzy_match("fs", "File Save").unwrap();
//! assert_eq!(m.positions, [0, 5]);
//! ```

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better.
    pub score: i32,
    /// Character indices (not bytes) of the matched query characters in the haystack.
    pub positions: Vec<usize>,
}

fn is_boundary(prev: Option<char>, c: char) -> bool {
    match prev {
        None => true,
        Some(p) => !p.is_alphanumeric() && p != '_' || p.is_lowercase() && c.is_uppercase(),
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Matches `query` against `text`. An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, text: &str) -> Option<FuzzyMatch> {
    let hay: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() {
        return Some(FuzzyMatch { score: 0, positions: Vec::new() });
    }
    if needle.len() > hay.len() {
        return None;
    }
    let sensitive = needle.iter().any(|c| c.is_uppercase());
    let same = |a: char, b: char| if sensitive { a == b } else { a.eq_ignore_ascii_case(&b) };
    const NONE: i32 = i32::MIN / 4;
    let n = hay.len();
    let m = needle.len();
    // best[i][j]: best score matching needle[..=i] with needle[i] at hay[j].
    let mut best = vec![vec![NONE; n]; m];
    let mut prev = vec![vec![usize::MAX; n]; m];
    for (j, slot) in best[0].iter_mut().enumerate() {
        if same(hay[j], needle[0]) {
            let mut score = 10;
            if is_boundary(None, hay[j]) {
                score += 20;
            }
            if j == 0 {
                score += 15;
            } else {
                score -= (j as i32).min(10);
            }
            *slot = score;
        }
    }
    for i in 1..m {
        let (prior, rest) = best.split_at_mut(i);
        let previous = &prior[i - 1];
        for (j, slot) in rest[0].iter_mut().enumerate().skip(i) {
            if !same(hay[j], needle[i]) {
                continue;
            }
            let mut best_here = NONE;
            let mut from = usize::MAX;
            for (k, &before) in previous.iter().enumerate().take(j).skip(i - 1) {
                if before == NONE {
                    continue;
                }
                let mut score = before + 10;
                if j == k + 1 {
                    score += 15;
                } else {
                    score -= ((j - k - 1) as i32).min(10) * 2;
                }
                if is_boundary(Some(hay[j - 1]), hay[j]) {
                    score += 20;
                }
                if score > best_here {
                    best_here = score;
                    from = k;
                }
            }
            *slot = best_here;
            prev[i][j] = from;
        }
    }
    let (mut end, mut score) = (usize::MAX, NONE);
    for (j, &candidate) in best[m - 1].iter().enumerate() {
        if candidate > score {
            score = candidate;
            end = j;
        }
    }
    if end == usize::MAX {
        return None;
    }
    if m == n {
        score += 30;
    } else if end + 1 == m {
        score += 15; // matched a prefix of the text
    }
    // Prefer shorter candidates when scores otherwise tie.
    score -= (char_len(text).saturating_sub(m) / 8) as i32;
    let mut positions = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = j;
        j = prev[i][j];
    }
    Some(FuzzyMatch { score, positions })
}

/// Matches every text, best score first (ties keep input order).
pub fn fuzzy_filter<'a>(query: &str, texts: impl IntoIterator<Item = &'a str>) -> Vec<(&'a str, FuzzyMatch)> {
    let mut out: Vec<(&str, FuzzyMatch)> = texts.into_iter().filter_map(|t| fuzzy_match(query, t).map(|m| (t, m))).collect();
    out.sort_by_key(|(_, m)| std::cmp::Reverse(m.score));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let m = fuzzy_match("", "Anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
    }

    #[test]
    fn subsequence_and_positions() {
        let m = fuzzy_match("fs", "File Save").unwrap();
        assert_eq!(m.positions, [0, 5]);
        let m = fuzzy_match("sv", "File Save").unwrap();
        assert_eq!(m.positions, [5, 7]);
        assert!(fuzzy_match("xyz", "File Save").is_none());
        assert!(fuzzy_match("efil", "File").is_none(), "order matters");
    }

    #[test]
    fn contiguous_and_boundary_matches_rank_higher() {
        let compact = fuzzy_match("save", "Save As").unwrap();
        let scattered = fuzzy_match("save", "Search All Values Elsewhere").unwrap();
        assert!(compact.score > scattered.score, "{} vs {}", compact.score, scattered.score);
        let boundary = fuzzy_match("gs", "Find Go Save").unwrap();
        assert_eq!(boundary.positions, [5, 8]);
    }

    #[test]
    fn smart_case() {
        assert!(fuzzy_match("save", "Save").is_some());
        assert!(fuzzy_match("Save", "Save").is_some());
        assert!(fuzzy_match("SAVE", "Save").is_none());
    }

    #[test]
    fn first_position_wins_in_filter_order() {
        let items = ["Open Recent", "Open Quickly", "Close Window"];
        let out = fuzzy_filter("open", items);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(t, _)| t.starts_with("Open")));
    }

    #[test]
    fn unicode_positions_are_chars() {
        let m = fuzzy_match("é", "xé").unwrap();
        assert_eq!(m.positions, [1]);
    }
}
