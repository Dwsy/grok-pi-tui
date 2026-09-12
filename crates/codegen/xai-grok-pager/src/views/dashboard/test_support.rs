//! Helpers shared by the dashboard render and chrome test modules.

use ratatui::buffer::Buffer;

use super::row::DashboardRow;
use super::state::{DashboardRowId, RowState};

/// Helper: read buffer row-by-row so multi-cell substring checks see the visible text in left-to-right order.
pub(super) fn buf_to_text(buf: &Buffer) -> String {
    let mut content = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            content.push_str(buf[(x, y)].symbol());
        }
        content.push('\n');
    }
    content
}

/// Helper for the group-header tests: build a top-level row with the given id and state, all other fields filled with sensible defaults.
pub(super) fn header_test_row(id: u32, state: RowState, label: &str) -> DashboardRow {
    use crate::app::agent::AgentId;
    DashboardRow {
        id: DashboardRowId::TopLevel(AgentId(id as usize)),
        label: label.to_string(),
        subtitle: None,
        state,
        activity: None,
        secondary_line: None,
        cwd_display: String::new(),
        cwd: std::path::PathBuf::from("/tmp"),
        last_change_at: std::time::SystemTime::now(),
        pinned: false,
        badges: Vec::new(),
        context_pct: None,
        indent: 0,
        parent_label: None,
        is_more_placeholder: false,
        more_count: 0,
    }
}
