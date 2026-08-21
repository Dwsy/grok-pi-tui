//! Tab and sidebar-section taxonomy for the grok-pi settings panel.
//!
//! Section membership is owned by [`crate::settings::layout`] so both this
//! panel and the upstream `settings_modal` render the same sidebar. This
//! module keeps only the panel-private helpers: the short [`tab_label`] used
//! for the tab bar, and [`widest_section_name`] used to size the sidebar
//! column so the divider never shifts when switching tabs.

use crate::settings::SettingCategory;

pub use crate::settings::layout::{OTHER_SECTION, section_for, sections_for};

/// Short tab-bar label for a category. Tabs share one row, so these drop the
/// `&`-conjunctions that `SettingCategory::label` carries.
pub fn tab_label(category: SettingCategory) -> &'static str {
    category.tab_label()
}

/// Widest declared section name across every tab. The sidebar column is sized
/// from this so the divider never shifts when switching tabs.
pub fn widest_section_name() -> usize {
    use unicode_width::UnicodeWidthStr;
    SettingCategory::ALL
        .iter()
        .flat_map(|cat| sections_for(*cat))
        .map(|name| name.width())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsRegistry;

    #[test]
    fn every_setting_has_a_section() {
        let registry = SettingsRegistry::defaults();
        let orphans: Vec<&str> = registry
            .all()
            .iter()
            .filter(|meta| section_for(meta.key) == OTHER_SECTION)
            .map(|meta| meta.key)
            .collect();
        assert!(
            orphans.is_empty(),
            "settings with no declared sidebar section: {orphans:?} — \
             add them to crate::settings::layout::section_for",
        );
    }

    #[test]
    fn every_section_is_declared_for_its_category() {
        let registry = SettingsRegistry::defaults();
        for meta in registry.all() {
            let section = section_for(meta.key);
            let declared = sections_for(meta.category);
            assert!(
                declared.contains(&section),
                "`{}` maps to section `{section}`, which is not in \
                 sections_for({:?}) = {declared:?}",
                meta.key,
                meta.category,
            );
        }
    }

    #[test]
    fn declared_sections_are_unique_per_category() {
        for category in SettingCategory::ALL {
            let mut seen = std::collections::HashSet::new();
            for section in sections_for(*category) {
                assert!(
                    seen.insert(*section),
                    "duplicate section `{section}` in sections_for({category:?})",
                );
            }
        }
    }

    #[test]
    fn tab_labels_are_non_empty_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for category in SettingCategory::ALL {
            let label = tab_label(*category);
            assert!(!label.is_empty(), "empty tab label for {category:?}");
            assert!(seen.insert(label), "duplicate tab label `{label}`");
        }
    }
}
