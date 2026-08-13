//! grok-pi settings panel — the F2 surface.
//!
//! A tabbed panel over the shared settings registry: a native tab bar per
//! [`crate::settings::SettingCategory`], a section sidebar within each tab, and
//! the focused row's description pinned to a fixed block under the list.
//!
//! This module is grok-pi's own; it deliberately shares nothing with upstream's
//! [`crate::views::settings_modal`] beyond the read-only registry, so upstream
//! merges never collide with it. The registry answers *what* settings exist;
//! [`layout`] answers *where this panel draws them*, and [`actions`] answers
//! *which `Action` a change dispatches*.
//!
//! Reached through [`crate::app::actions::Action::OpenPiSettings`]. Upstream's
//! `Action::OpenSettings` still opens the original modal.

mod actions;
mod input;
pub mod layout;
mod render;
mod state;

#[cfg(test)]
mod tests;

pub use input::{handle_key, handle_mouse, handle_paste};
pub use render::render_pi_settings;
pub use state::{MODAL_TITLE, ModeKind, Outcome, PiSettingsState, Row};
