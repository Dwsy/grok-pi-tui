//! The Skills tab's listing: which fetch the tab waits for, and the rows for the folders the
//! backend couldn't scan.

use xai_grok_shell::extensions::skills::{SkillScanError, SkillsListResponse};

use super::{ExtensionsModalState, TabDataState};

impl ExtensionsModalState {
    /// Numbers a new skills fetch. Only the newest fetch's answer shows, so neither an older
    /// listing nor an older error replaces a newer one.
    pub fn next_skills_fetch(&mut self) -> u64 {
        self.skills_fetch += 1;
        self.skills_fetch
    }

    /// Shows the answer to `fetch`, a listing or the error that replaced it, unless the tab has
    /// asked again since. Returns whether it did.
    pub fn show_skills_listing(
        &mut self,
        fetch: u64,
        result: Result<SkillsListResponse, String>,
    ) -> bool {
        if fetch != self.skills_fetch {
            return false;
        }
        match result {
            Ok(listing) => self.apply_skills_listing(listing),
            Err(error) => self.skills_data = TabDataState::Error(error),
        }
        true
    }

    pub fn apply_skills_listing(&mut self, listing: SkillsListResponse) {
        self.seed_skills_groups_once(&listing.skills);
        self.skills_data = TabDataState::Loaded(listing);
    }
}

/// A header that counts the folders, then one row per folder with its error; nothing while the
/// user searches. The picker selects no header row, so every row is one.
pub(super) fn scan_error_rows(errors: &[SkillScanError], query: &str) -> Vec<String> {
    if errors.is_empty() || !query.is_empty() {
        return Vec::new();
    }
    let header = match errors.len() {
        1 => "Couldn't scan (1 folder) · r to reload".to_owned(),
        count => format!("Couldn't scan ({count} folders) · r to reload"),
    };
    std::iter::once(header)
        .chain(
            errors
                .iter()
                .map(|error| format!("{}: {}", error.path, error.message)),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use xai_grok_tools::implementations::skills::types::SkillInfo;

    use crate::views::extensions_modal::{ExtensionsTab, render_extensions_modal};

    #[test]
    fn an_error_for_the_newest_fetch_replaces_the_listing() {
        let mut modal = ExtensionsModalState::new(ExtensionsTab::Skills);
        let first = modal.next_skills_fetch();
        modal.show_skills_listing(first, Ok(listing(&["deploy"], &["/work/locked"])));
        let retry = modal.next_skills_fetch();

        assert!(modal.show_skills_listing(retry, Err("scan failed".to_owned())));

        assert!(matches!(&modal.skills_data, TabDataState::Error(error) if error == "scan failed"));
    }

    #[test]
    fn scan_errors_list_under_a_header_and_hide_while_searching() {
        let errors = vec![SkillScanError {
            path: "/work/locked".to_owned(),
            message: "permission denied".to_owned(),
        }];

        assert_eq!(
            vec![
                "Couldn't scan (1 folder) · r to reload".to_owned(),
                "/work/locked: permission denied".to_owned(),
            ],
            scan_error_rows(&errors, "")
        );
        assert!(scan_error_rows(&errors, "dep").is_empty());
    }

    #[test]
    fn the_skills_tab_renders_scan_errors_after_the_skills() {
        let mut modal = ExtensionsModalState::new(ExtensionsTab::Skills);
        // Set directly: a first listing collapses the skill groups
        modal.skills_data = TabDataState::Loaded(listing(&["deploy"], &["/work/locked"]));
        let area = Rect::new(0, 0, 110, 30);
        let mut buf = Buffer::empty(area);

        render_extensions_modal(&mut buf, area, &mut modal, None, false, 0);

        let text = rendered(&buf);
        let at = |needle: &str| {
            assert_eq!(1, text.matches(needle).count(), "{needle:?} in {text}");
            text.find(needle)
        };
        let skill = at("deploy");
        let header = at("Couldn't scan (1 folder) · r to reload");
        at("/work/locked: permission denied");
        assert!(skill < header, "the skills come first: {text}");
    }

    fn listing(names: &[&str], unreadable: &[&str]) -> SkillsListResponse {
        SkillsListResponse {
            skills: names
                .iter()
                .map(|name| SkillInfo {
                    name: (*name).to_owned(),
                    description: format!("{name} skill"),
                    path: format!("/skills/{name}/SKILL.md"),
                    ..SkillInfo::default()
                })
                .collect(),
            scan_errors: unreadable
                .iter()
                .map(|path| SkillScanError {
                    path: (*path).to_owned(),
                    message: "permission denied".to_owned(),
                })
                .collect(),
        }
    }

    fn rendered(buf: &Buffer) -> String {
        let area = *buf.area();
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .filter_map(|x| buf.cell((x, y)).map(|cell| cell.symbol()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
