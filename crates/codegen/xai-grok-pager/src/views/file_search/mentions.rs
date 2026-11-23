//! Extensible `@` object-mention catalog and query matcher.
//!
//! Rust provides the generic capability (catalog state, prefix/alias matching,
//! dropdown rendering, and atomic chip insertion); TypeScript extensions
//! provide concrete object providers (`skill`, `thread`, `mcp`, etc.) and
//! prompt-time URI resolution (`grok-pi://<kind>/<id>`).

use serde::Deserialize;

/// Full catalog of `@` mention providers published over `pi/ui/mentions_catalog`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct MentionCatalog {
    #[serde(default)]
    pub providers: Vec<MentionProvider>,
}

/// Optional contextual shortcut action declared by a TS mention provider
/// (e.g. `@session` declares `ctrl+o` → "session picker" → `/__pi_mention_session_picker`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct MentionProviderAction {
    /// Pi KeyId format (e.g. `"ctrl+o"`, `"alt+s"`).
    pub key: String,
    /// Short label rendered in the bottom shortcuts bar when this provider is active.
    pub label: String,
    /// Hidden Pi slash command invoked via RPC when the shortcut is pressed.
    pub command: String,
}

/// One extensible mention provider (e.g. `skill`, `session`, `mcp`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct MentionProvider {
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub items: Vec<MentionItem>,
    /// Optional contextual shortcut action shown in the bottom bar when this provider is active.
    #[serde(default)]
    pub action: Option<MentionProviderAction>,
}

/// One selectable mention item owned by a provider.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct MentionItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default, rename = "insertText", alias = "insert")]
    pub insert_text: String,
    #[serde(default, rename = "searchText")]
    pub search_text: String,
}

/// A single row rendered in the `@` completion dropdown for mentions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionEntry {
    /// Category header row (e.g. `@skill:`) that drills into the provider on Tab/Right.
    ProviderHeader {
        kind: String,
        label: String,
        description: String,
        item_count: usize,
    },
    /// Concrete item row (e.g. `@skill:grok-pi-rsi`) that inserts an atomic chip.
    Item {
        kind: String,
        id: String,
        title: String,
        subtitle: String,
        uri: String,
        insert_text: String,
    },
}

/// How an `@` query should combine mention entries with fuzzy file search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionMatchResult {
    /// Explicit `@file:<subquery>` (or alias `@f:`, `@path:`, `@文件:`):
    /// route directly to the fuzzy file matcher with `<subquery>`.
    FileSubquery(String),
    /// Explicit `@<kind>:<subquery>` for a non-file provider:
    /// show only matching items from that provider.
    EntriesOnly(Vec<MentionEntry>),
    /// Bare `@` or `@<partial>` without a provider colon:
    /// show matching provider headers above normal fuzzy file results.
    HeadersPlusFiles(Vec<MentionEntry>),
}

/// Return true if `token` is the built-in file mention kind or one of its aliases.
pub fn is_file_kind_alias(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.eq_ignore_ascii_case("file")
        || trimmed.eq_ignore_ascii_case("f")
        || trimmed.eq_ignore_ascii_case("path")
        || trimmed == "文件"
}

/// Split `@<kind>:<subquery>` or `@<kind>：<subquery>` (full-width Chinese colon).
///
/// Returns `None` when the prefix looks like a file path (contains `/`, `\`, or `.`),
/// so `@src/main.rs:10` stays a normal file reference.
pub fn split_kind_query(raw: &str) -> Option<(&str, &str)> {
    let (idx, sep_len) = raw.char_indices().find_map(|(i, ch)| match ch {
        ':' => Some((i, 1)),
        '：' => Some((i, '：'.len_utf8())),
        _ => None,
    })?;
    let prefix = &raw[..idx];
    if prefix.is_empty()
        || prefix.contains('/')
        || prefix.contains('\\')
        || prefix.contains('.')
    {
        return None;
    }
    Some((prefix, &raw[idx + sep_len..]))
}

pub fn find_provider_by_kind<'a>(
    catalog: &'a MentionCatalog,
    kind: &str,
) -> Option<&'a MentionProvider> {
    let trimmed = kind.trim();
    if trimmed.is_empty() {
        return None;
    }
    catalog
        .providers
        .iter()
        .find(|p| p.kind.eq_ignore_ascii_case(trimmed))
}

pub fn find_provider_by_token<'a>(
    catalog: &'a MentionCatalog,
    token: &str,
) -> Option<&'a MentionProvider> {
    catalog
        .providers
        .iter()
        .find(|p| provider_matches_token(p, token))
}

fn provider_matches_token(provider: &MentionProvider, token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }
    provider.kind.eq_ignore_ascii_case(token)
        || provider
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(token) || alias == token)
}

fn provider_matches_prefix(provider: &MentionProvider, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q_lower = query.to_lowercase();
    provider.kind.to_lowercase().starts_with(&q_lower)
        || provider.label.to_lowercase().contains(&q_lower)
        || provider.aliases.iter().any(|alias| {
            let a_lower = alias.to_lowercase();
            a_lower.starts_with(&q_lower) || alias.starts_with(query)
        })
}

/// Lightweight fuzzy matcher for a single lowercased term against a lowercased candidate.
///
/// Ranking tiers:
/// - Exact match: 1000
/// - Prefix match: 800 - length penalty
/// - Contiguous substring match: 600 - position penalty (+40 on word boundary)
/// - Ordered subsequence fuzzy match (e.g. `gpr` → `grok-pi-rsi`): 200 + boundary/consecutive bonuses
fn fuzzy_score_term(haystack: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack == needle {
        return Some(1000);
    }
    if haystack.starts_with(needle) {
        let penalty = (haystack.chars().count().saturating_sub(needle.chars().count()) as i32).min(150);
        return Some(800 - penalty);
    }
    if let Some(byte_pos) = haystack.find(needle) {
        let char_pos = haystack[..byte_pos].chars().count() as i32;
        let boundary = byte_pos == 0
            || haystack[..byte_pos]
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, '-' | '_' | '/' | ':' | ' ' | '.'));
        let bonus = if boundary { 40 } else { 0 };
        return Some((600 + bonus - char_pos * 2).max(300));
    }

    // Ordered subsequence fuzzy match.
    let mut needle_chars = needle.chars();
    let mut target = needle_chars.next()?;
    let mut score: i32 = 200;
    let mut prev_match_idx: Option<usize> = None;
    let mut prev_ch: Option<char> = None;

    for (idx, ch) in haystack.char_indices() {
        if ch == target {
            if let Some(prev) = prev_match_idx {
                if idx == prev + prev_ch.map_or(1, char::len_utf8) {
                    score += 18; // consecutive run bonus
                } else {
                    score -= 2; // small gap penalty
                }
            }
            if prev_ch.is_none_or(|c| matches!(c, '-' | '_' | '/' | ':' | ' ' | '.')) {
                score += 24; // word-boundary / acronym bonus
            }
            prev_match_idx = Some(idx);
            match needle_chars.next() {
                Some(next) => target = next,
                None => return Some(score.max(50)),
            }
        }
        prev_ch = Some(ch);
    }
    None
}

/// Score a mention item against `query` (higher is better; `None` means no match).
/// Empty query returns `Some(0)` so caller preserves original provider order (e.g. recency for `@session:`).
fn score_mention_item(item: &MentionItem, query: &str) -> Option<i32> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    let q_lower = trimmed.to_lowercase();
    let id_lower = item.id.to_lowercase();
    let title_lower = item.title.to_lowercase();
    let haystack = if item.search_text.is_empty() {
        format!("{} {} {}", item.id, item.title, item.subtitle).to_lowercase()
    } else {
        item.search_text.to_lowercase()
    };
    let mut total = 0i32;
    for term in q_lower.split_whitespace() {
        let best = fuzzy_score_term(&id_lower, term)
            .map(|s| s + 60)
            .into_iter()
            .chain(fuzzy_score_term(&title_lower, term).map(|s| s + 30))
            .chain(fuzzy_score_term(&haystack, term))
            .max()?;
        total += best;
    }
    Some(total)
}

/// Resolve an `@` query string (without the leading `@`) against the current catalog.
pub fn resolve_mention_entries(catalog: &MentionCatalog, raw_query: &str) -> MentionMatchResult {
    // Explicit `@!` hidden-file mode always goes straight to file search.
    if raw_query.starts_with('!') {
        return MentionMatchResult::HeadersPlusFiles(Vec::new());
    }

    if let Some((prefix, subquery)) = split_kind_query(raw_query) {
        if is_file_kind_alias(prefix) {
            return MentionMatchResult::FileSubquery(subquery.to_owned());
        }
        if let Some(provider) = catalog
            .providers
            .iter()
            .find(|p| provider_matches_token(p, prefix))
        {
            let mut scored: Vec<(i32, usize, MentionEntry)> = provider
                .items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| {
                    let score = score_mention_item(item, subquery)?;
                    Some((
                        score,
                        idx,
                        MentionEntry::Item {
                            kind: provider.kind.clone(),
                            id: item.id.clone(),
                            title: item.title.clone(),
                            subtitle: item.subtitle.clone(),
                            uri: item.uri.clone(),
                            insert_text: item.insert_text.clone(),
                        },
                    ))
                })
                .collect();
            // Higher score first; tie-break by original provider order (preserves recency for `@session:`).
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let entries = scored.into_iter().map(|(_, _, entry)| entry).collect();
            return MentionMatchResult::EntriesOnly(entries);
        }
        return MentionMatchResult::HeadersPlusFiles(Vec::new());
    }

    // Paths (e.g. `src/main.rs` or `Cargo.toml`) stay pure file search.
    if raw_query.contains('/') || raw_query.contains('\\') || raw_query.contains('.') {
        return MentionMatchResult::HeadersPlusFiles(Vec::new());
    }

    let mut headers: Vec<MentionEntry> = catalog
        .providers
        .iter()
        .filter(|p| {
            // Keep bare `@` uncluttered by hiding the `file` header when query is empty
            // (bare `@` already lists workspace files directly below provider headers).
            !(raw_query.is_empty() && p.kind.eq_ignore_ascii_case("file"))
                && provider_matches_prefix(p, raw_query)
        })
        .map(|p| MentionEntry::ProviderHeader {
            kind: p.kind.clone(),
            label: if p.label.is_empty() {
                p.kind.clone()
            } else {
                p.label.clone()
            },
            description: p.description.clone(),
            item_count: p.items.len(),
        })
        .collect();

    // Offer the built-in `@file:` header when typing `@file`, `@path`, or `@文件`
    // (kept out of bare `@` since bare `@` already lists workspace files directly).
    if !raw_query.is_empty() && !catalog.providers.iter().any(|p| p.kind == "file") {
        let q_lower = raw_query.to_lowercase();
        let matches_file = "file".starts_with(&q_lower)
            || "path".starts_with(&q_lower)
            || "文件".starts_with(raw_query);
        if matches_file {
            headers.push(MentionEntry::ProviderHeader {
                kind: "file".to_owned(),
                label: "File".to_owned(),
                description: "Search workspace files (@file:<path>)".to_owned(),
                item_count: 0,
            });
        }
    }

    MentionMatchResult::HeadersPlusFiles(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> MentionCatalog {
        MentionCatalog {
            providers: vec![
                MentionProvider {
                    kind: "skill".to_owned(),
                    label: "Skill".to_owned(),
                    aliases: vec!["sk".to_owned(), "技能".to_owned()],
                    description: "Workspace & global skills".to_owned(),
                    action: None,
                    items: vec![MentionItem {
                        id: "grok-pi-rsi".to_owned(),
                        title: "grok-pi-rsi".to_owned(),
                        subtitle: "RSI guide".to_owned(),
                        uri: "grok-pi://skill/grok-pi-rsi".to_owned(),
                        insert_text: "@skill:grok-pi-rsi".to_owned(),
                        search_text: "grok-pi-rsi rsi guide".to_owned(),
                    }],
                },
                MentionProvider {
                    kind: "session".to_owned(),
                    label: "Session".to_owned(),
                    aliases: vec!["会话".to_owned(), "历史".to_owned(), "thread".to_owned()],
                    description: "Past conversations (SQLite index)".to_owned(),
                    action: Some(MentionProviderAction {
                        key: "ctrl+o".to_owned(),
                        label: "session picker".to_owned(),
                        command: "__pi_mention_session_picker".to_owned(),
                    }),
                    items: vec![MentionItem {
                        id: "abc123".to_owned(),
                        title: "Fix flicker".to_owned(),
                        subtitle: "2026-11-21".to_owned(),
                        uri: "grok-pi://session/abc123".to_owned(),
                        insert_text: "@session:abc123".to_owned(),
                        search_text: "abc123 fix flicker".to_owned(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn file_alias_routes_to_file_subquery() {
        let catalog = sample_catalog();
        assert_eq!(
            resolve_mention_entries(&catalog, "file:src/main"),
            MentionMatchResult::FileSubquery("src/main".to_owned())
        );
        assert_eq!(
            resolve_mention_entries(&catalog, "文件：Cargo"),
            MentionMatchResult::FileSubquery("Cargo".to_owned())
        );
        assert_eq!(
            resolve_mention_entries(&catalog, "f:lib.rs"),
            MentionMatchResult::FileSubquery("lib.rs".to_owned())
        );
    }

    #[test]
    fn chinese_alias_and_fullwidth_colon_resolve_items() {
        let catalog = sample_catalog();
        let res = resolve_mention_entries(&catalog, "技能：rsi");
        match res {
            MentionMatchResult::EntriesOnly(entries) => {
                assert_eq!(entries.len(), 1);
                assert!(matches!(
                    &entries[0],
                    MentionEntry::Item { kind, id, .. } if kind == "skill" && id == "grok-pi-rsi"
                ));
            }
            other => panic!("expected EntriesOnly, got {other:?}"),
        }
    }

    #[test]
    fn fuzzy_subsequence_matches_skill_and_session() {
        let catalog = sample_catalog();
        match resolve_mention_entries(&catalog, "skill:gpr") {
            MentionMatchResult::EntriesOnly(entries) => {
                assert_eq!(entries.len(), 1);
                assert!(matches!(
                    &entries[0],
                    MentionEntry::Item { kind, id, .. } if kind == "skill" && id == "grok-pi-rsi"
                ));
            }
            other => panic!("expected EntriesOnly, got {other:?}"),
        }
        match resolve_mention_entries(&catalog, "session:flk") {
            MentionMatchResult::EntriesOnly(entries) => {
                assert_eq!(entries.len(), 1);
                assert!(matches!(
                    &entries[0],
                    MentionEntry::Item { kind, id, .. } if kind == "session" && id == "abc123"
                ));
            }
            other => panic!("expected EntriesOnly, got {other:?}"),
        }
    }

    #[test]
    fn path_with_line_number_is_not_treated_as_kind() {
        assert_eq!(split_kind_query("src/main.rs:42"), None);
        assert_eq!(split_kind_query("Cargo.toml:1"), None);
    }

    #[test]
    fn bare_at_lists_provider_headers() {
        let catalog = sample_catalog();
        match resolve_mention_entries(&catalog, "") {
            MentionMatchResult::HeadersPlusFiles(headers) => {
                assert_eq!(headers.len(), 2);
            }
            other => panic!("expected HeadersPlusFiles, got {other:?}"),
        }
    }
}
