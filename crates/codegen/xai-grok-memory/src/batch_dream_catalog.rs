//! `MEMORY.md` stays small for chat turns, so Dream gets its own full topic catalog.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::batch_dream::{BatchDreamError, BatchDreamStore, Result, TOPICS_DIR};

pub const CATALOG_BUDGET_BYTES: usize = 256 * 1024;
const SHORT_DESCRIPTION_BYTES: usize = 160;
const MAX_CATALOG_TOPICS: usize = 100_000;
pub(crate) const LIST_PAGE_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogEntry {
    pub path: String,
    pub title: String,
    pub description: String,
    pub bytes: u64,
    #[serde(skip)]
    pub modified: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTier {
    Full,
    ShortDescriptions,
    TitlesOnly,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicCatalog {
    entries: Vec<CatalogEntry>,
    listed: BTreeSet<String>,
    tier: CatalogTier,
    rendered: String,
}

impl TopicCatalog {
    pub fn build(store: &BatchDreamStore, budget: usize) -> Result<TopicCatalog> {
        let excluded = store.excluded_topics()?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(store.scope_dir.join(TOPICS_DIR))? {
            store.control.check()?;
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = format!("{TOPICS_DIR}/{name}");
            if !entry.file_type()?.is_file()
                || name.starts_with('.')
                || !name.ends_with(".md")
                || excluded.contains(&path)
            {
                continue;
            }
            if entries.len() == MAX_CATALOG_TOPICS {
                return Err(BatchDreamError::Invalid(format!(
                    "more than {MAX_CATALOG_TOPICS} topics"
                )));
            }
            let metadata = entry.metadata()?;
            let (title, description) =
                crate::v2::summarize_file(&entry.path(), crate::v2::MAX_DESCRIPTION_BYTES)?;
            entries.push(CatalogEntry {
                path,
                title: single_line(&title),
                description: single_line(&description),
                bytes: metadata.len(),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
                    .unwrap_or(0),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(TopicCatalog::render(entries, budget))
    }

    fn render(entries: Vec<CatalogEntry>, budget: usize) -> TopicCatalog {
        let all: Vec<&CatalogEntry> = entries.iter().collect();
        for tier in [
            CatalogTier::Full,
            CatalogTier::ShortDescriptions,
            CatalogTier::TitlesOnly,
        ] {
            let rendered = render_tier(&all, entries.len(), tier);
            if rendered.len() <= budget {
                let listed = entries.iter().map(|entry| entry.path.clone()).collect();
                return TopicCatalog {
                    entries,
                    listed,
                    tier,
                    rendered,
                };
            }
        }
        let mut recent: Vec<&CatalogEntry> = entries.iter().collect();
        recent.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.path.cmp(&right.path))
        });
        let mut used = header(entries.len(), CatalogTier::Partial).len();
        let mut chosen = Vec::new();
        for entry in recent {
            let line = render_entry(entry, CatalogTier::TitlesOnly);
            if used + line.len() > budget {
                break;
            }
            used += line.len();
            chosen.push(entry);
        }
        chosen.sort_by(|left, right| left.path.cmp(&right.path));
        let rendered = render_tier(&chosen, entries.len(), CatalogTier::Partial);
        let listed = chosen.iter().map(|entry| entry.path.clone()).collect();
        TopicCatalog {
            entries,
            listed,
            tier: CatalogTier::Partial,
            rendered,
        }
    }

    #[must_use]
    pub fn rendered(&self) -> &str {
        &self.rendered
    }

    #[must_use]
    pub fn tier(&self) -> CatalogTier {
        self.tier
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn byte_totals(&self) -> (u64, u64) {
        self.entries.iter().fold((0, 0), |(total, largest), entry| {
            (total.saturating_add(entry.bytes), largest.max(entry.bytes))
        })
    }

    pub(crate) fn page(&self, after: Option<&str>) -> (Vec<&CatalogEntry>, Option<String>) {
        let start = after.map_or(0, |after| {
            self.entries
                .partition_point(|entry| entry.path.as_str() <= after)
        });
        let page: Vec<&CatalogEntry> = self
            .entries
            .iter()
            .skip(start)
            .take(LIST_PAGE_ENTRIES)
            .collect();
        let next = (start + page.len() < self.entries.len())
            .then(|| page.last().map(|entry| entry.path.clone()))
            .flatten();
        (page, next)
    }

    pub fn changes_since(
        &self,
        store: &BatchDreamStore,
        changed: &BTreeSet<String>,
    ) -> Result<String> {
        let mut lines = String::new();
        for path in changed {
            let absolute = store.scope_dir.join(path);
            if !absolute.try_exists()? {
                continue;
            }
            let (title, description) =
                crate::v2::summarize_file(&absolute, crate::v2::MAX_DESCRIPTION_BYTES)?;
            let status = if self
                .entries
                .binary_search_by(|entry| entry.path.as_str().cmp(path))
                .is_ok()
            {
                "changed"
            } else {
                "new"
            };
            let entry = CatalogEntry {
                path: path.clone(),
                title: single_line(&title),
                description: single_line(&description),
                bytes: std::fs::metadata(&absolute)?.len(),
                modified: 0,
            };
            lines.push_str(&format!(
                "({status}) {}",
                render_entry(&entry, CatalogTier::Full)
            ));
        }
        Ok(lines)
    }

    #[must_use]
    pub fn is_listed(&self, path: &str) -> bool {
        self.listed.contains(path)
    }
}

fn header(total: usize, tier: CatalogTier) -> String {
    let detail = match tier {
        CatalogTier::Full => "path | size | title | description",
        CatalogTier::ShortDescriptions => {
            "path | size | title | description (shortened to fit; read a topic for more)"
        }
        CatalogTier::TitlesOnly => "path | size | title (descriptions omitted to fit)",
        CatalogTier::Partial => {
            "path | size | title; only recently changed topics are listed, use the list action for the rest"
        }
    };
    format!("{total} topics. Each line: {detail}\n")
}

fn render_tier(entries: &[&CatalogEntry], total: usize, tier: CatalogTier) -> String {
    let mut rendered = header(total, tier);
    for entry in entries {
        rendered.push_str(&render_entry(entry, tier));
    }
    rendered
}

fn render_entry(entry: &CatalogEntry, tier: CatalogTier) -> String {
    let description = match tier {
        CatalogTier::Full => entry.description.as_str(),
        CatalogTier::ShortDescriptions => {
            let end = entry
                .description
                .floor_char_boundary(SHORT_DESCRIPTION_BYTES);
            entry.description.get(..end).unwrap_or_default()
        }
        CatalogTier::TitlesOnly | CatalogTier::Partial => "",
    };
    if description.is_empty() {
        format!("{} | {} | {}\n", entry.path, entry.bytes, entry.title)
    } else {
        format!(
            "{} | {} | {} | {description}\n",
            entry.path, entry.bytes, entry.title
        )
    }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl BatchDreamStore {
    pub(crate) fn excluded_topics(&self) -> Result<BTreeSet<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT REPLACE(relative_path, char(92), '/') FROM memory_v2_tombstones
             UNION SELECT REPLACE(relative_path, char(92), '/') FROM memory_v2_quarantined_paths",
        )?;
        let paths = statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<BTreeSet<String>, _>>()?;
        Ok(paths)
    }
}
