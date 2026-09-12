//! Minimal (scrollback-native) render mode: `grok --minimal`.
//!
//! In this mode finalized conversation blocks are printed once into the terminal's *native* scrollback.
//! The printing goes through `xai_ratatui_inline::Terminal::insert_before` and reuses `EntryRenderer`.
//! A small pinned live region holds the running-turn status, the prompt, and a minimal status line.
//! The interactive `ScrollbackPane` (scroll, fold, selection, mouse) is not used; the terminal owns history.
//!
//! - [`commit`]: committed-frontier logic, display policy, and the per-frame commit-to-scrollback pass.
//! - [`live`]: the pinned live region (tail, todos, `/btw`, status, prompt).
//! - [`todo`]: the persistent todo panel shown above the prompt.
//! - [`auth`]: the in-region sign-in flow shown before a session exists.
//! - [`overlay`]: the inline-overlay host (prompt-anchored dropdowns; grows / shrinks the live viewport).
//!
//! # Wiring
//!
//! `xai-grok-pager` (the lib) does **not** depend on this crate: this crate reads deeply into the pager's [`AppView`] / view model.
//! A reverse dependency would be a cargo cycle.
//! Instead the pager exposes function-pointer hooks ([`xai_grok_pager::minimal_hook`]).
//! The composition-root binary (`xai-grok-pager-bin`) calls [`install`] once at startup to register this crate's [`draw`] entry point.
//! When the hooks are not installed the pager's minimal-mode branches are inert.

pub mod auth;
pub mod commit;
pub mod full_view;
pub mod live;
pub mod overlay;
pub mod panel;
pub mod plan;
pub mod todo;
pub mod welcome;

#[cfg(test)]
mod guard;

use crossterm::QueueableCommand;
use crossterm::terminal::BeginSynchronizedUpdate;

use xai_grok_pager::app::PagerTerminal;
use xai_grok_pager::app::app_view::AppView;

/// Adopt terminal size and open a synchronized update first, or a same-frame resize prints committed blocks at the stale width and hard-wraps them permanently.
/// Size the viewport to post-commit height before `insert_before`; sizing after stranded the prompt at the top of a tall streaming viewport.
/// The synchronized update batches commit scroll/paint with the live redraw; without it a multi-block commit flickers as separate presents.
pub fn draw(app: &mut AppView, terminal: &mut PagerTerminal) {
    let _ = terminal.backend_mut().queue(BeginSynchronizedUpdate);
    let _ = terminal.autoresize();
    // Pending permission/question marks are synced ONCE, up front (see `commit::sync_pending_marks`)
    // The viewport sizing (`sync_viewport` / `tail_height` / `will_commit`) and the commit pass then judge committability against the same state
    commit::sync_pending_marks(app);
    // Advance any in-progress /transcript build by one time-budgeted slice (sets `pending_pager_path` when done; see `full_view::pump_transcript`)
    full_view::pump_transcript(app);
    welcome::maybe_commit_welcome(app, terminal);
    plan::maybe_commit_plan(app);
    overlay::sync_viewport(app, terminal);
    commit::commit_active(app, terminal);
    commit::expand_pending(app, terminal);
    live::draw_live(app, terminal);
}

/// Register the minimal-mode render hooks with `xai-grok-pager`. It installs the function-pointer hooks so the
/// pager's `ScreenMode::Minimal` branches dispatch into this crate.
pub fn install() {
    xai_grok_pager::minimal_hook::install(xai_grok_pager::minimal_hook::MinimalHooks { draw });
}
