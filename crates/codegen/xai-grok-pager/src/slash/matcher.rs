//! Nucleo-based fuzzy matcher for slash command and argument suggestions.
//!
//! Thin wrapper around nucleo's `MultiPattern` that provides ranked results and highlight index extraction.

use nucleo::{
    Config, Matcher, Utf32String,
    pattern::{CaseMatching, MultiPattern, Normalization},
};

/// Fuzzy matcher backed by nucleo.
/// Maintains internal state (pattern and matcher) between calls for efficiency.
/// Not thread-safe; intended for single-threaded use within `SlashController`.
#[derive(Debug)]
pub struct FuzzyMatcher {
    pattern: MultiPattern,
    matcher: Matcher,
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        Self {
            pattern: MultiPattern::new(1),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// Rank items by fuzzy match score. Returns `(index, score)` pairs sorted by descending score, then ascending key
    /// text. At most `limit` results are returned. When `query` is empty, returns the first `limit` items with score 0
    /// (insertion order).
    pub fn rank<T, F>(
        &mut self,
        items: &[T],
        query: &str,
        limit: usize,
        mut key_fn: F,
    ) -> Vec<(usize, u32)>
    where
        F: FnMut(&T) -> &str,
    {
        if limit == 0 || items.is_empty() {
            return Vec::new();
        }

        let trimmed = query.trim();
        if trimmed.is_empty() {
            let capped = items.len().min(limit);
            return (0..capped).map(|idx| (idx, 0)).collect();
        }

        self.pattern
            .reparse(0, trimmed, CaseMatching::Smart, Normalization::Smart, false);

        let mut hits: Vec<(usize, u32, String)> = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let text = key_fn(item);
            if text.is_empty() {
                continue;
            }
            let matcher_text = Utf32String::from(text);
            if let Some(score) = self
                .pattern
                .score(std::slice::from_ref(&matcher_text), &mut self.matcher)
            {
                hits.push((idx, score, text.to_owned()));
            }
        }

        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        if hits.len() > limit {
            hits.truncate(limit);
        }
        hits.into_iter()
            .map(|(idx, score, _)| (idx, score))
            .collect()
    }

    /// Rank model selector items with Pi TUI's fuzzy-search algorithm.
    ///
    /// Pi treats whitespace and `/` as independent tokens and rewards exact,
    /// contiguous, early, and word-boundary matches. This intentionally stays
    /// separate from the native nucleo matcher used by other slash surfaces.
    pub fn rank_pi_model_selector<T, F>(
        &self,
        items: &[T],
        query: &str,
        mut key_fn: F,
    ) -> Vec<usize>
    where
        F: FnMut(&T) -> &str,
    {
        let tokens: Vec<String> = query
            .trim()
            .split(|ch: char| ch.is_whitespace() || ch == '/')
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            return (0..items.len()).collect();
        }

        let mut matches: Vec<(usize, f64)> = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let text = key_fn(item).to_lowercase();
                tokens
                    .iter()
                    .try_fold(0.0, |score, token| {
                        pi_fuzzy_match(token, &text).map(|match_score| score + match_score)
                    })
                    .map(|score| (index, score))
            })
            .collect();
        matches.sort_by(|left, right| left.1.total_cmp(&right.1));
        matches.into_iter().map(|(index, _)| index).collect()
    }

    /// Extract fuzzy match highlight indices for the most recent pattern.
    ///
    /// Returns character positions in `text` that matched the pattern.
    pub fn indices(&mut self, text: &str) -> Vec<u32> {
        let mut indices = Vec::new();
        if text.is_empty() {
            return indices;
        }
        let s = Utf32String::from(text);
        let pattern = self.pattern.column_pattern(0);
        pattern.indices(s.slice(..), &mut self.matcher, &mut indices);
        indices
    }

    /// Match `query` against display text and return display-relative indices.
    pub fn indices_for(&mut self, query: &str, text: &str) -> Option<Vec<u32>> {
        let query = query.trim();
        if query.is_empty() || text.is_empty() {
            return None;
        }
        self.pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, false);
        let text = Utf32String::from(text);
        self.pattern
            .score(std::slice::from_ref(&text), &mut self.matcher)?;
        let mut indices = Vec::new();
        self.pattern
            .column_pattern(0)
            .indices(text.slice(..), &mut self.matcher, &mut indices);
        Some(indices)
    }
}

fn pi_fuzzy_match(query: &str, text: &str) -> Option<f64> {
    pi_fuzzy_match_strict(query, text).or_else(|| {
        swap_alpha_numeric_query(query)
            .and_then(|swapped| pi_fuzzy_match_strict(&swapped, text).map(|score| score + 5.0))
    })
}

fn pi_fuzzy_match_strict(query: &str, text: &str) -> Option<f64> {
    if query.is_empty() {
        return Some(0.0);
    }
    if query.chars().count() > text.chars().count() {
        return None;
    }

    let mut query_chars = query.chars();
    let mut wanted = query_chars.next()?;
    let mut score = 0.0;
    let mut last_match = None;
    let mut consecutive_matches = 0_u32;

    for (index, ch) in text.chars().enumerate() {
        if ch != wanted {
            continue;
        }
        let is_word_boundary = index == 0
            || text
                .chars()
                .nth(index.saturating_sub(1))
                .is_some_and(|previous| {
                    previous.is_whitespace() || matches!(previous, '-' | '_' | '.' | '/' | ':')
                });
        if last_match == Some(index.saturating_sub(1)) {
            consecutive_matches += 1;
            score -= f64::from(consecutive_matches) * 5.0;
        } else {
            consecutive_matches = 0;
            if let Some(last) = last_match {
                score += (index - last - 1) as f64 * 2.0;
            }
        }
        if is_word_boundary {
            score -= 10.0;
        }
        score += index as f64 * 0.1;
        last_match = Some(index);

        match query_chars.next() {
            Some(next) => wanted = next,
            None => {
                if query == text {
                    score -= 100.0;
                }
                return Some(score);
            }
        }
    }

    None
}

fn swap_alpha_numeric_query(query: &str) -> Option<String> {
    let split_at = query.find(|ch: char| ch.is_ascii_digit())?;
    let (first, second) = query.split_at(split_at);
    if !first.is_empty()
        && first.chars().all(|ch| ch.is_ascii_alphabetic())
        && !second.is_empty()
        && second.chars().all(|ch| ch.is_ascii_digit())
    {
        return Some(format!("{second}{first}"));
    }

    let split_at = query.find(|ch: char| ch.is_ascii_alphabetic())?;
    let (first, second) = query.split_at(split_at);
    (!first.is_empty()
        && first.chars().all(|ch| ch.is_ascii_digit())
        && !second.is_empty()
        && second.chars().all(|ch| ch.is_ascii_alphabetic()))
    .then(|| format!("{second}{first}"))
}

#[cfg(test)]
mod tests {
    use super::FuzzyMatcher;

    #[test]
    fn indices_for_are_relative_to_display() {
        let mut matcher = FuzzyMatcher::new();
        assert_eq!(matcher.indices_for("ssh", "ssh-wrap"), Some(vec![0, 1, 2]));
        assert_eq!(matcher.indices_for("sw", "ssh-wrap"), Some(vec![0, 4]));
        assert_eq!(matcher.indices_for("fix s", "ssh-wrap"), None);
    }

    #[test]
    fn empty_query_yields_insertion_order() {
        let mut matcher = FuzzyMatcher::new();
        let items = ["alpha", "beta", "gamma"];
        let hits = matcher.rank(&items, "", items.len(), |item| item);
        assert_eq!(hits, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn ranked_results_prioritize_matches() {
        let mut matcher = FuzzyMatcher::new();
        let items = ["model", "help", "history"];
        let hits = matcher.rank(&items, "mod", items.len(), |item| item);
        assert_eq!(hits.first().map(|&(idx, _)| items[idx]), Some("model"));
    }

    #[test]
    fn limit_caps_results() {
        let mut matcher = FuzzyMatcher::new();
        let items = ["aaa", "aab", "aac", "aad", "aae"];
        let hits = matcher.rank(&items, "a", 2, |item| item);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn empty_items_returns_empty() {
        let mut matcher = FuzzyMatcher::new();
        let items: [&str; 0] = [];
        let hits = matcher.rank(&items, "test", 10, |item| item);
        assert!(hits.is_empty());
    }

    /// Single-letter `/p` ties many `p*` commands at the same nucleo score; ordering comes entirely from the secondary tiebreaks (display, builtin, MRU, etc.).
    #[test]
    fn query_p_ties_personas_and_pager_headless_at_same_score() {
        let mut matcher = FuzzyMatcher::new();
        let items = ["personas", "pager-headless", "plan", "plugins"];
        let hits = matcher.rank(&items, "p", items.len(), |item| item);
        let score_of = |name: &str| -> Option<u32> {
            hits.iter()
                .find(|&&(idx, _)| items[idx] == name)
                .map(|&(_, s)| s)
        };
        let personas = score_of("personas").expect("personas matches p");
        let pager = score_of("pager-headless").expect("pager-headless matches p");
        assert_eq!(personas, pager, "expected equal fuzzy scores for /p case");
        assert!(personas > 0);
        // Matcher limit=1 secondary sort is ascending key text, so pager-headless wins
        let top1 = matcher.rank(&items, "p", 1, |item| item);
        assert_eq!(items[top1[0].0], "pager-headless");
    }

    #[test]
    fn pi_model_selector_prefers_exact_provider_prefixed_search() {
        let matcher = FuzzyMatcher::new();
        let items = [
            "proxy proxy/openai-codex/gpt-5.5 proxy openai-codex/gpt-5.5",
            "openai-codex openai-codex/gpt-5.5 openai-codex gpt-5.5",
        ];
        let ranked = matcher.rank_pi_model_selector(&items, "openai-codex/gpt-5.5", |item| item);
        assert_eq!(ranked, vec![1, 0]);
    }

    #[test]
    fn pi_model_selector_matches_swapped_alpha_numeric_tokens() {
        let matcher = FuzzyMatcher::new();
        let items = ["openai openai/gpt-5.2-codex openai gpt-5.2-codex"];
        assert_eq!(
            matcher.rank_pi_model_selector(&items, "codex52", |item| item),
            vec![0]
        );
    }
}
