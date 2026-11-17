//! Completion from words already in the document. Always available, no server, no configuration.

use std::collections::HashMap;

use crate::fallback::keywords::LanguageTables;
use crate::text::{is_word_char, word_before};
use crate::types::{CompletionItem, CompletionKind, CompletionSource, Position};

/// Completions from the words of `text`, ranked by prefix match and how often each word occurs.
///
/// The word being typed at `position` is excluded. `language` selects the language's extra word
/// characters (`$` in shell, `-` in CSS, ...). Words shorter than two characters and pure numbers
/// are skipped. `limit` caps the result.
pub fn word_completions(
    text: &str,
    position: Position,
    language: &str,
    limit: usize,
) -> Vec<CompletionItem> {
    let extra = LanguageTables::builtin().extra_word_chars(language);
    let (_, typed) = word_before(text, position, extra);
    let typed_lower = typed.to_ascii_lowercase();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut start = None;
    for (offset, ch) in text.char_indices() {
        if is_word_char(ch, extra) {
            start.get_or_insert(offset);
        } else if let Some(word_start) = start.take() {
            count_word(&text[word_start..offset], &typed_lower, &mut counts);
        }
    }
    if let Some(word_start) = start {
        count_word(&text[word_start..], &typed_lower, &mut counts);
    }
    let mut words: Vec<(&str, usize)> = counts.into_iter().collect();
    words.sort_by(|(left_word, left_count), (right_word, right_count)| {
        let left_prefix = left_word.to_ascii_lowercase().starts_with(&typed_lower);
        let right_prefix = right_word.to_ascii_lowercase().starts_with(&typed_lower);
        right_prefix
            .cmp(&left_prefix)
            .then_with(|| right_count.cmp(left_count))
            .then_with(|| left_word.cmp(right_word))
    });
    words
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(rank, (word, count))| {
            let mut item = CompletionItem::simple(
                word.to_owned(),
                CompletionKind::Text,
                CompletionSource::Word,
            );
            item.detail = (count > 1).then(|| format!("{count} uses"));
            item.sort_text = Some(format!("{rank:04}"));
            item
        })
        .collect()
}

fn count_word<'a>(word: &'a str, typed_lower: &str, counts: &mut HashMap<&'a str, usize>) {
    if word.chars().count() < 2 || word.chars().all(|ch| ch.is_ascii_digit()) {
        return;
    }
    if !typed_lower.is_empty() && word.to_ascii_lowercase() == typed_lower {
        return;
    }
    *counts.entry(word).or_insert(0) += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_prefix_matches_and_counts() {
        let text = "value = value_of(other)\nvalue_of_thing = value\n";
        let items = word_completions(text, Position::new(2, 1), "python", 10);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels[0], "value");
        assert!(labels.contains(&"value_of"));
        assert!(labels.contains(&"other"));
        assert!(!labels.contains(&"val")); // the word being typed
        assert_eq!(items[0].source, CompletionSource::Word);
        assert_eq!(items[0].detail.as_deref(), Some("2 uses"));
    }

    #[test]
    fn excludes_numbers_short_words_and_the_typed_word() {
        let text = "a bb 123 x bb cc";
        let items = word_completions(text, Position::new(0, 0), "python", 10);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["bb", "cc"]);
    }

    #[test]
    fn language_word_characters() {
        let text = "echo $HOME\necho $HOSTNAME\n";
        let items = word_completions(text, Position::new(1, 5), "shell", 10);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"$HOME"));
        assert!(labels.contains(&"$HOSTNAME"));
        let items = word_completions(text, Position::new(1, 5), "python", 10);
        assert!(items.iter().all(|item| !item.label.starts_with('$')));
    }

    #[test]
    fn limit_and_empty_text() {
        assert!(word_completions("", Position::new(0, 0), "python", 10).is_empty());
        let items = word_completions("aa bb cc dd", Position::new(0, 0), "python", 2);
        assert_eq!(items.len(), 2);
        let items = word_completions("aa bb cc dd", Position::new(0, 0), "python", 0);
        assert!(items.is_empty());
    }

    #[test]
    fn unicode_words() {
        let text = "café = café_au_lait";
        let items = word_completions(text, Position::new(0, 2), "python", 10);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"café"));
        assert!(labels.contains(&"café_au_lait"));
    }
}
