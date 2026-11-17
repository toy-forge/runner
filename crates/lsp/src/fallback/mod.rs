//! Completion that needs no language server: words already in the document, authored keyword and
//! snippet tables, file paths, and shell `compgen` output.
//!
//! The UI merges these with server completions; [`rank_completions`] is the shared ranker, and
//! [`fallback_completions`] is the one-call convenience that applies all the text-only providers
//! to a document position.

pub mod keywords;
pub mod paths;
pub mod shell;
pub mod words;

use std::path::Path;

use crate::text::word_before;
use crate::types::{CompletionItem, CompletionSource, Position};

pub use keywords::{LanguageTable, LanguageTables, SnippetTemplate, TableKind};
pub use paths::{looks_like_path, path_completions, path_fragment};
pub use shell::{
    ShellCompletionKind, common_commands, compgen_argv, compgen_command, parse_compgen_output,
    shell_builtins, shell_completions,
};
pub use words::word_completions;

/// A fuzzy-match score for `needle` in `haystack`: `None` when the characters are not all present
/// in order; higher is better. Prefix matches score highest, then contiguous matches, then
/// matches that start early.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = haystack.to_lowercase().chars().collect();
    if needle.len() > haystack.len() {
        return None;
    }
    let mut positions = Vec::with_capacity(needle.len());
    let mut wanted = 0;
    for (position, ch) in haystack.iter().enumerate() {
        if wanted < needle.len() && *ch == needle[wanted] {
            positions.push(position);
            wanted += 1;
        }
    }
    if wanted < needle.len() {
        return None;
    }
    let mut score = 10 * needle.len() as i32;
    if positions.first() == Some(&0) {
        score += 40;
        if needle.len() == haystack.len() {
            score += 10;
        }
    }
    if positions.windows(2).all(|pair| pair[1] == pair[0] + 1) {
        score += 20;
    }
    score -= positions.first().copied().unwrap_or(0) as i32;
    score -= haystack.len().saturating_sub(needle.len()) as i32;
    Some(score)
}

fn source_rank(source: CompletionSource) -> i32 {
    match source {
        CompletionSource::Server => 4,
        CompletionSource::Static => 3,
        CompletionSource::Shell | CompletionSource::Path => 2,
        CompletionSource::Word => 1,
    }
}

/// Ranks and de-duplicates completion items for what the user typed.
///
/// Server answers come before static tables, which come before shell/path guesses and words in
/// the document; within a source, fuzzy match quality decides. Items that match the same text are
/// merged, keeping the higher-ranked source. When `typed` is empty the existing `sort_text`
/// decides inside each source.
pub fn rank_completions(items: Vec<CompletionItem>, typed: &str) -> Vec<CompletionItem> {
    let mut merged: Vec<CompletionItem> = Vec::with_capacity(items.len());
    let mut index_by_text: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(items.len());
    for item in items {
        let key = item.match_text().to_lowercase();
        if let Some(&index) = index_by_text.get(&key) {
            if source_rank(item.source) > source_rank(merged[index].source) {
                merged[index] = item;
            }
            continue;
        }
        index_by_text.insert(key, merged.len());
        merged.push(item);
    }
    merged.sort_by(|left, right| {
        let left_score = source_rank(left.source) * 1000
            + fuzzy_score(typed, left.match_text()).unwrap_or(-1000);
        let right_score = source_rank(right.source) * 1000
            + fuzzy_score(typed, right.match_text()).unwrap_or(-1000);
        right_score
            .cmp(&left_score)
            .then_with(|| left.sort_key().cmp(right.sort_key()))
            .then_with(|| left.label.cmp(&right.label))
    });
    merged
}

/// Every text-only provider applied at `position`: authored keywords/snippets, words in the
/// document, and (only when the caret is on something path-like) file paths.
pub fn fallback_completions(
    text: &str,
    position: Position,
    language: &str,
    base_dir: &Path,
) -> Vec<CompletionItem> {
    let tables = LanguageTables::builtin();
    let extra = tables.extra_word_chars(language);
    let (_, typed) = word_before(text, position, extra);
    let mut items = tables.complete(language, "", 300);
    items.extend(word_completions(text, position, language, 100));
    if path_fragment(text, position).is_some_and(|(_, fragment)| looks_like_path(&fragment)) {
        items.extend(path_completions(text, position, base_dir));
    }
    let mut ranked = rank_completions(items, &typed);
    ranked.truncate(200);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CompletionKind;

    #[test]
    fn fuzzy_matches_in_order() {
        assert!(fuzzy_score("", "anything").is_some());
        assert!(fuzzy_score("abc", "abc") > fuzzy_score("abc", "axbxc"));
        assert!(fuzzy_score("abc", "axbxc").is_some());
        assert_eq!(fuzzy_score("cba", "abc"), None);
        assert_eq!(fuzzy_score("abcd", "abc"), None);
        assert!(fuzzy_score("PY", "python").unwrap() > 0);
        assert_eq!(fuzzy_score("x", "abc"), None);
    }

    #[test]
    fn ranking_prefers_servers_then_static_then_words() {
        let word = CompletionItem::simple("print", CompletionKind::Text, CompletionSource::Word);
        let keyword =
            CompletionItem::simple("print", CompletionKind::Keyword, CompletionSource::Static);
        let server =
            CompletionItem::simple("print", CompletionKind::Function, CompletionSource::Server);
        let ranked = rank_completions(vec![word, keyword.clone(), server.clone()], "pri");
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].source, CompletionSource::Server);

        let other = CompletionItem::simple("printer", CompletionKind::Text, CompletionSource::Word);
        let ranked = rank_completions(vec![other, keyword], "pri");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].label, "print"); // static beats word
        let ranked = rank_completions(
            vec![
                CompletionItem::simple("zebra", CompletionKind::Text, CompletionSource::Word),
                CompletionItem::simple("zest", CompletionKind::Text, CompletionSource::Word),
            ],
            "ze",
        );
        assert_eq!(ranked[0].label, "zest");
    }

    #[test]
    fn aggregate_includes_words_keywords_and_only_path_like_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.txt"), "").unwrap();
        let text = "def report():\n    ret";
        let items = fallback_completions(text, Position::new(1, 7), "python", dir.path());
        assert!(items.iter().any(|item| item.label == "return"));
        assert!(items.iter().any(|item| item.label == "report"));
        // `ret` is not path-like, so report.txt is not offered.
        assert!(!items.iter().any(|item| item.label == "report.txt"));

        let text = "open(./re";
        let items = fallback_completions(text, Position::new(0, 10), "python", dir.path());
        assert!(items.iter().any(|item| item.label == "report.txt"));
    }
}
