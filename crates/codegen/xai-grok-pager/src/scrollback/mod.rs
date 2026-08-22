//! Scrollback — conversation display with blocks, scroll, selection, turns.
//!
//! This module owns the scrollback rendering pipeline:
//! - `block.rs` / `blocks/` — content block types (agent, thinking, tool, etc.)
//! - `entry.rs` — ScrollbackEntry wraps a block with display state
//! - `state.rs` — ScrollbackState manages entries, scroll, selection, turns
//! - `layout.rs` — HorizontalLayout for entry column structure
//! - `sticky.rs` — Sticky header computation for turn prompts
//! - `selection.rs` — SelectionBox rendering
//! - `render.rs` — Scroll-aware rendering with scratch buffers
//! - `types.rs` — Core types (BlockLine, BlockOutput, DisplayMode, etc.)
//! - `wrappers/` — Rendering composition (EntryRenderer, BlockRenderer, etc.)

pub mod block;
pub mod blocks;
pub mod entry;
pub mod export;
pub mod layout;
pub mod link_map;
pub mod render;
pub mod scrollback_pane;
pub mod search;
pub mod selection;
pub mod state;
pub mod sticky;
pub mod table_geometry;
pub mod text_selection;
pub mod types;
pub mod wrappers;

// Re-exports for convenience
pub use block::{BlockContent, RenderBlock};
pub use blocks::{
    AgentMessageBlock, SystemMessageBlock, ThinkingBlock, ToolCallBlock, UserPromptBlock,
};
pub use entry::{EntryId, ScrollbackEntry};
pub use layout::HorizontalLayout;
pub use link_map::{VisibleLink, VisibleLinkMap};
pub use render::ScratchBuffer;
pub use scrollback_pane::ScrollbackPane;
pub use search::{ScrollbackMatch, ScrollbackSearchIndex, ScrollbackSearchState};
pub use selection::{RenderOutput, SelectionBox};
pub use state::{EntryLayoutInfo, ScrollbackState};
pub use text_selection::*;
pub use types::*;

/// Reserved right-gutter columns for message-block timestamp overlays.
///
/// Covers the widest non-hover label (`  MM-DD HH:MM`). Keep in sync with the
/// three reservation sites: `EntryRenderer::timestamp_reserved()`,
/// `scrollback_pane.rs`'s sticky-header reservation, and
/// [`render::timestamp_reserved_for_block`].
pub(crate) const TIMESTAMP_RESERVE: u16 = 13;

/// Timestamp label overlaid on the first content row of message blocks.
///
/// Today's messages show time only; earlier messages prefix the numeric date.
/// Hover appends seconds. Numeric, locale-free formats throughout — no English
/// month names or AM/PM markers. The two leading spaces match the historical
/// overlay offset from the content edge.
pub(crate) fn message_timestamp_label(
    ts: chrono::DateTime<chrono::Local>,
    hovered: bool,
) -> String {
    let is_today = ts.date_naive() == chrono::Local::now().date_naive();
    let base = match (is_today, hovered) {
        (true, false) => ts.format("%H:%M"),
        (true, true) => ts.format("%H:%M:%S"),
        (false, false) => ts.format("%m-%d %H:%M"),
        (false, true) => ts.format("%m-%d %H:%M:%S"),
    };
    format!("  {base}")
}
