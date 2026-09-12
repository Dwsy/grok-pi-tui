//! The location line shared by the welcome top bar and the dashboard header: git branch, worktree badge, and the
//! tilde-collapsed cwd with its `(worktree of …)` suffix. Pure over the per-cwd git probe; nothing here spawns `git`.

use std::path::Path;

use ratatui::style::Style;
use ratatui::text::Span;

use crate::git_info;
use crate::theme::Theme;

/// The `worktree ` marker painted before the path of a linked worktree, matching the session status bar (accent_user).
pub(crate) fn worktree_badge(theme: &Theme) -> Span<'static> {
    Span::styled("worktree ", Style::default().fg(theme.accent_user))
}

/// The unstyled pieces of a location line, so each surface (welcome top bar, dashboard header) can style them on its own.
pub(crate) struct LocationParts {
    /// The checked-out branch, `detached` for a detached HEAD, `None` outside a git repo.
    pub branch: Option<String>,
    pub is_worktree: bool,
    /// The tilde-collapsed cwd, with a `(worktree of …)` suffix for a linked worktree.
    pub cwd_display: String,
}

/// The dashboard header passes its staged `app.cwd` so the line tracks a `/cd` immediately, before (or even if) `Effect::SetWorkingDir` moves the process cwd.
/// Safe to call during render: it reads the per-cwd git cache and never blocks or spawns `git`.
pub(crate) fn location_parts(cwd: &Path) -> LocationParts {
    location_parts_from(cwd, git_info::cwd_git_info_lazy(cwd))
}

/// Pure mapping from the git probe result to the location pieces; `None` is a cache miss or a non-repo cwd.
/// Takes `info` by value so the branch moves out instead of being cloned on every frame.
fn location_parts_from(cwd: &Path, info: Option<git_info::CwdGitInfo>) -> LocationParts {
    let cwd_display = format_cwd_display(cwd, info.as_ref());
    let is_worktree = info.as_ref().is_some_and(|i| i.is_worktree);
    let branch = info.and_then(|i| i.branch).map(|b| {
        if b.is_empty() {
            "detached".to_owned()
        } else {
            b
        }
    });
    LocationParts {
        branch,
        is_worktree,
        cwd_display,
    }
}

/// The tilde-collapsed cwd, with a `(worktree of …)` suffix when `info` reports a linked worktree's main repo.
/// Matches the session status bar; the `worktree ` badge itself is [`worktree_badge`].
/// Pure formatting over the per-cwd git probe; nothing here spawns `git`.
fn format_cwd_display(cwd: &Path, info: Option<&git_info::CwdGitInfo>) -> String {
    let display = collapse_home(cwd);
    let main_repo = info.and_then(|i| i.main_repo.as_deref());
    format_cwd_parts(&display, main_repo)
}

/// Pure formatting for the cwd display; no global state.
fn format_cwd_parts(display: &str, main_repo: Option<&str>) -> String {
    if let Some(main_repo) = main_repo {
        format!("{display} (worktree of {main_repo})")
    } else {
        display.to_string()
    }
}

fn collapse_home(dir: &std::path::Path) -> String {
    // Match the session status bar: Path::strip_prefix via abbreviate_path, not a string prefix of USERPROFILE
    // `C:\Users\foo` must not collapse neighbor `C:\Users\foobar` to `~bar`
    crate::util::abbreviate_path(&dir.to_string_lossy()).into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn format_cwd_plain_repo() {
        assert_eq!(format_cwd_parts("~/xai", None), "~/xai");
    }

    /// A linked worktree shows the `(worktree of …)` suffix (matching the session status bar) regardless of the worktree's human label.
    /// The label is no longer shown here; the `worktree ` badge stands in for it.
    #[test]
    fn format_cwd_worktree_shows_main_repo() {
        assert_eq!(
            format_cwd_parts("~/wt/session-1", Some("~/xai")),
            "~/wt/session-1 (worktree of ~/xai)"
        );
    }

    /// The header shows the ACTUAL cwd, not the git repo root: switching into a subdirectory of a repo reflects the subdirectory.
    /// (`/work/...` is outside `$HOME`, so `collapse_home` leaves it verbatim.)
    #[test]
    fn format_cwd_display_shows_subdir_not_repo_root() {
        let info = git_info::CwdGitInfo {
            branch: Some("main".into()),
            is_worktree: false,
            main_repo: None,
            worktree_label: None,
        };
        assert_eq!(
            format_cwd_display(Path::new("/work/xai/frontend/apps"), Some(&info)),
            "/work/xai/frontend/apps",
        );
    }

    /// A worktree subdirectory shows the `(worktree of …)` suffix (matching the session status bar) while still showing the real subdirectory path.
    #[test]
    fn format_cwd_display_worktree_subdir_shows_main_repo() {
        let info = git_info::CwdGitInfo {
            branch: Some("kevin/x".into()),
            is_worktree: true,
            main_repo: Some("~/xai".into()),
            worktree_label: Some("location-picker".into()),
        };
        assert_eq!(
            format_cwd_display(Path::new("/work/wt/location-picker/frontend"), Some(&info)),
            "/work/wt/location-picker/frontend (worktree of ~/xai)",
        );
    }

    /// On a cache miss (`info == None`) the header still shows the raw cwd.
    #[test]
    fn format_cwd_display_cache_miss_shows_raw_cwd() {
        assert_eq!(
            format_cwd_display(Path::new("/work/xai/frontend/apps"), None),
            "/work/xai/frontend/apps",
        );
    }

    /// The location pieces per git probe outcome: a named branch, a detached HEAD (`Some("")` from the probe), a repo with no HEAD
    /// (`branch: None`), and a cache miss. The worktree flag and the `(worktree of …)` suffix travel with the probe.
    #[test]
    fn location_parts_from_maps_each_probe_outcome() {
        let cwd = Path::new("/work/wt/feature");
        let probe = |branch: Option<&str>, is_worktree: bool| git_info::CwdGitInfo {
            branch: branch.map(str::to_owned),
            is_worktree,
            main_repo: is_worktree.then(|| "~/xai".to_owned()),
            worktree_label: None,
        };

        let named = location_parts_from(cwd, Some(probe(Some("main"), false)));
        assert_eq!(named.branch.as_deref(), Some("main"));
        assert!(!named.is_worktree);
        assert_eq!(named.cwd_display, "/work/wt/feature");

        let detached = location_parts_from(cwd, Some(probe(Some(""), true)));
        assert_eq!(detached.branch.as_deref(), Some("detached"));
        assert!(detached.is_worktree);
        assert_eq!(detached.cwd_display, "/work/wt/feature (worktree of ~/xai)");

        let no_head = location_parts_from(cwd, Some(probe(None, false)));
        assert_eq!(no_head.branch, None);

        let miss = location_parts_from(cwd, None);
        assert_eq!(miss.branch, None);
        assert!(!miss.is_worktree);
        assert_eq!(miss.cwd_display, "/work/wt/feature");
    }

    /// String-prefix `strip_prefix($HOME)` would collapse a neighbor profile (`$HOMEbar`) into `~bar`.
    /// Path-component matching must leave it intact while still collapsing a real child of `$HOME`.
    #[test]
    fn collapse_home_does_not_eat_neighbor_profile() {
        let Some(home) = git_info::home_dir() else {
            return;
        };
        if home.is_empty() {
            return;
        }
        let neighbor = PathBuf::from(format!("{home}bar")).join("src");
        let collapsed_neighbor = collapse_home(&neighbor);
        assert_eq!(
            collapsed_neighbor,
            neighbor.display().to_string(),
            "string prefix would produce ~bar/src (or ~bar\\src)"
        );

        let child = PathBuf::from(&home).join("src");
        let expected = format!("~/{}", Path::new("src").display());
        assert_eq!(collapse_home(&child), expected);
    }
}
