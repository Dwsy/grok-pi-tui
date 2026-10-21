//! `/tree-map` — minimalist branch map showing only user messages on the main path + forks.
//!
//! Subtractive design: from the full Pi session tree, only user messages are
//! displayed. The main (active) path forms a vertical spine; fork branches are
//! indented with `├─`/`└─` connectors. Clicking a user message navigates to
//! that branch point via `ctx.navigateTree`.

use crate::app::actions::SessionTreeNode;
use crate::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// One visible row in the tree map (a user message with topology info).
#[derive(Debug, Clone)]
pub struct TreeMapRow {
    /// Index into `TreeMapState::nodes`.
    pub node_index: usize,
    /// Entry ID for navigation.
    pub entry_id: String,
    /// User message text (single line).
    pub text: String,
    /// Whether this node is on the active path (root → current leaf).
    pub on_active_path: bool,
    /// Whether this node is the current leaf.
    pub is_current: bool,
    /// Fork depth: 0 = main path, 1+ = branch off a fork.
    pub fork_depth: usize,
    /// Whether this is the last sibling at its fork level.
    pub is_last_sibling: bool,
    /// Whether the parent node is a fork point (multiple children).
    pub parent_is_fork: bool,
}

/// State for the `/tree-map` modal.
#[derive(Debug, Clone)]
pub struct TreeMapState {
    /// Full node list from Pi (same as `/tree`).
    pub nodes: Vec<SessionTreeNode>,
    /// Current leaf entry ID.
    pub leaf_id: Option<String>,
    /// Filtered + flattened visible rows (user messages only).
    pub rows: Vec<TreeMapRow>,
    /// Currently selected row index.
    pub selected: usize,
    /// Scroll offset.
    pub scroll: usize,
    /// Whether the tree is still loading.
    pub loading: bool,
    /// Status message.
    pub status: Option<String>,
    /// List area from last render (for mouse hit-testing).
    pub list_rect: Option<Rect>,
}

impl TreeMapState {
    pub fn loading() -> Self {
        Self {
            nodes: Vec::new(),
            leaf_id: None,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            loading: true,
            status: Some("Fetching Pi session tree…".into()),
            list_rect: None,
        }
    }

    pub fn with_nodes(nodes: Vec<SessionTreeNode>, leaf_id: Option<String>) -> Self {
        let mut state = Self::loading();
        state.loading = false;
        state.status = None;
        state.leaf_id = leaf_id.clone();
        state.nodes = nodes;
        state.rebuild_rows();
        // Select the last row on the active path (nearest to current position).
        state.selected = state
            .rows
            .iter()
            .rposition(|r| r.on_active_path)
            .unwrap_or(state.rows.len().saturating_sub(1));
        state
    }

    pub fn replace_nodes(&mut self, nodes: Vec<SessionTreeNode>, leaf_id: Option<String>) {
        self.nodes = nodes;
        self.leaf_id = leaf_id;
        self.loading = false;
        self.status = None;
        self.rebuild_rows();
        self.clamp_selected();
    }

    /// Rebuild visible rows: filter to user messages, compute fork topology.
    fn rebuild_rows(&mut self) {
        let nodes = &self.nodes;
        let leaf_id = self.leaf_id.clone();

        // Build active path set (root → leaf).
        let active_path = build_active_path_set(nodes, leaf_id.as_deref());

        // Build parent→children map and id→index map.
        let id_to_idx: std::collections::HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();

        let mut rows: Vec<TreeMapRow> = Vec::new();

        // Walk tree DFS, collecting user messages with fork topology.
        // Roots are nodes with parent_id == None or parent not found.
        let roots: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.parent_id.is_none()
                    || n.parent_id
                        .as_deref()
                        .is_some_and(|p| !id_to_idx.contains_key(p))
            })
            .map(|(i, _)| i)
            .collect();

        // Sort roots: active path first.
        let mut sorted_roots = roots;
        sorted_roots.sort_by_key(|&i| {
            if active_path.contains(nodes[i].id.as_str()) {
                0
            } else {
                1
            }
        });

        // DFS stack: (node_index, fork_depth, sibling_index, sibling_count, parent_is_fork)
        let mut stack: Vec<(usize, usize, usize, usize, bool)> = Vec::new();
        for (i, &root_idx) in sorted_roots.iter().enumerate().rev() {
            stack.push((root_idx, 0, i, sorted_roots.len(), false));
        }

        while let Some((idx, fork_depth, sib_idx, sib_count, parent_is_fork)) = stack.pop() {
            let node = &nodes[idx];

            // Collect user messages.
            if node.entry_type == "message" && node.role == "user" {
                let text = node.preview.replace('\n', " ").trim().to_string();
                if !text.is_empty() {
                    rows.push(TreeMapRow {
                        node_index: idx,
                        entry_id: node.id.clone(),
                        text,
                        on_active_path: active_path.contains(node.id.as_str()),
                        is_current: leaf_id.as_deref() == Some(node.id.as_str()),
                        fork_depth,
                        is_last_sibling: sib_idx == sib_count.saturating_sub(1),
                        parent_is_fork,
                    });
                }
            }

            // Push children.
            let children: Vec<usize> = node
                .child_ids
                .iter()
                .filter_map(|cid| id_to_idx.get(cid.as_str()).copied())
                .collect();

            if children.is_empty() {
                continue;
            }

            let is_fork = children.len() > 1;
            let child_depth = if is_fork { fork_depth + 1 } else { fork_depth };

            // Sort children: active path first.
            let mut sorted_children = children;
            sorted_children.sort_by_key(|&ci| {
                if active_path.contains(nodes[ci].id.as_str()) {
                    0
                } else {
                    1
                }
            });

            for (i, &child_idx) in sorted_children.iter().enumerate().rev() {
                stack.push((child_idx, child_depth, i, sorted_children.len(), is_fork));
            }
        }

        self.rows = rows;
    }

    pub fn selected_row(&self) -> Option<&TreeMapRow> {
        self.rows.get(self.selected)
    }

    pub fn selected_entry_id(&self) -> Option<String> {
        self.selected_row().map(|r| r.entry_id.clone())
    }

    pub fn clamp_selected(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.rows.len() as isize;
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = ((self.selected as isize + delta).rem_euclid(len)) as usize;
        self.ensure_visible(12);
    }

    pub fn ensure_visible(&mut self, viewport: usize) {
        let viewport = viewport.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport {
            self.scroll = self.selected + 1 - viewport;
        }
    }

    /// Map absolute terminal (col, row) to a visible row index, if any.
    pub fn hit_test_row(&self, col: u16, row: u16) -> Option<usize> {
        let list = self.list_rect?;
        if col < list.x || col >= list.x.saturating_add(list.width) {
            return None;
        }
        if row < list.y || row >= list.y.saturating_add(list.height) {
            return None;
        }
        let rel = (row - list.y) as usize;
        let index = self.scroll + rel;
        if index < self.rows.len() {
            Some(index)
        } else {
            None
        }
    }
}

/// Build the set of entry IDs on the active path (root → leaf).
fn build_active_path_set(
    nodes: &[SessionTreeNode],
    leaf_id: Option<&str>,
) -> std::collections::HashSet<String> {
    let mut active = std::collections::HashSet::new();
    let Some(leaf) = leaf_id else {
        return active;
    };
    let id_to_parent: std::collections::HashMap<&str, Option<&str>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.parent_id.as_deref()))
        .collect();
    let mut current = Some(leaf);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        active.insert(id.to_string());
        current = id_to_parent.get(id).copied().flatten();
    }
    active
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the tree map modal content into `area`.
pub fn render_tree_map(buf: &mut Buffer, area: Rect, state: &mut TreeMapState, theme: &Theme) {
    if area.width < 10 || area.height < 4 {
        state.list_rect = None;
        return;
    }
    buf.set_style(area, Style::default().bg(theme.bg_base));

    // Layout: header (1) + list (rest) + help (1)
    let header_h = 1u16;
    let help_h = 1u16;
    let list_h = area.height.saturating_sub(header_h + help_h);
    let list_area = Rect {
        x: area.x,
        y: area.y + header_h,
        width: area.width,
        height: list_h,
    };
    let help_area = Rect {
        x: area.x,
        y: area.y + header_h + list_h,
        width: area.width,
        height: help_h,
    };

    state.list_rect = Some(list_area);
    let viewport = list_area.height as usize;
    state.ensure_visible(viewport.max(1));

    // Header
    let header = if state.loading {
        Line::from(Span::styled(
            format!("  {}", state.status.as_deref().unwrap_or("Loading…")),
            Style::default().fg(theme.accent_user),
        ))
    } else {
        Line::from(vec![
            Span::styled(
                "  Branch Map",
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} user messages", state.rows.len()),
                Style::default().fg(theme.gray),
            ),
        ])
    };
    Paragraph::new(header).render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: header_h,
        },
        buf,
    );

    // List
    let mut lines: Vec<Line> = Vec::new();

    if state.loading {
        lines.push(Line::from(Span::styled(
            "  Waiting for Pi session tree…",
            Style::default().fg(theme.text_secondary),
        )));
    } else if state.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No user messages in session",
            Style::default().fg(theme.gray),
        )));
    } else {
        let start = state.scroll.min(state.rows.len().saturating_sub(1));
        let end = (start + viewport).min(state.rows.len());

        // Scroll up indicator
        if start > 0 {
            lines.push(Line::from(Span::styled(
                format!("    ↑ {} more", start),
                Style::default().fg(theme.gray_dim),
            )));
        }

        for i in start..end {
            let row = &state.rows[i];
            let is_selected = i == state.selected;
            lines.push(build_row_line(row, is_selected, area.width, theme));
        }

        // Scroll down indicator
        if end < state.rows.len() {
            lines.push(Line::from(Span::styled(
                format!("    ↓ {} more", state.rows.len() - end),
                Style::default().fg(theme.gray_dim),
            )));
        }
    }

    Paragraph::new(lines).render(list_area, buf);

    // Help line
    let help = Line::from(Span::styled(
        "  ↑↓ navigate · Enter/click switch · Esc close",
        Style::default().fg(theme.gray),
    ));
    Paragraph::new(help).render(help_area, buf);
}

/// Build a single row line with gutter + text.
fn build_row_line(row: &TreeMapRow, is_selected: bool, width: u16, theme: &Theme) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();

    // Cursor
    if is_selected {
        spans.push(Span::styled("› ", Style::default().fg(theme.accent_user)));
    } else {
        spans.push(Span::raw("  "));
    }

    // Gutter: fork connectors
    if row.fork_depth > 0 {
        // Indent for depth
        for _ in 0..row.fork_depth.saturating_sub(1) {
            spans.push(Span::styled("│ ", Style::default().fg(theme.gray_dim)));
        }
        // Connector
        let connector = if row.is_last_sibling {
            "└─"
        } else {
            "├─"
        };
        let connector_color = if row.on_active_path {
            theme.accent_user
        } else {
            theme.gray
        };
        spans.push(Span::styled(
            format!("{connector} "),
            Style::default().fg(connector_color),
        ));
    }

    // Text
    let max_text_width = (width as usize)
        .saturating_sub(2) // cursor
        .saturating_sub(row.fork_depth * 2) // gutter
        .saturating_sub(2) // connector
        .max(10);
    let text = truncate_str(&row.text, max_text_width);

    let text_style = if is_selected {
        Style::default()
            .fg(theme.accent_user)
            .add_modifier(Modifier::BOLD)
    } else if row.is_current {
        Style::default().fg(theme.accent_success)
    } else if row.on_active_path {
        Style::default().fg(theme.text_primary)
    } else {
        Style::default().fg(theme.gray)
    };
    spans.push(Span::styled(text, text_style));

    // Current marker
    if row.is_current {
        spans.push(Span::styled(
            " ●",
            Style::default().fg(theme.accent_success),
        ));
    }

    Line::from(spans)
}

/// Truncate a string to fit within `max_width` display columns (approximate).
fn truncate_str(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_width.saturating_sub(1)).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, parent: Option<&str>, role: &str, preview: &str) -> SessionTreeNode {
        SessionTreeNode {
            id: id.to_string(),
            parent_id: parent.map(|p| p.to_string()),
            depth: 0,
            is_leaf: false,
            is_current: false,
            on_active_path: false,
            role: role.to_string(),
            preview: preview.to_string(),
            detail: String::new(),
            label: None,
            label_timestamp: None,
            entry_type: "message".to_string(),
            timestamp: None,
            child_ids: Vec::new(),
            has_text: true,
        }
    }

    #[test]
    fn filters_to_user_messages_only() {
        let mut n1 = make_node("1", None, "user", "Hello");
        let n2 = make_node("2", Some("1"), "assistant", "Hi there");
        let n3 = make_node("3", Some("2"), "user", "Second question");
        n1.child_ids = vec!["2".to_string()];

        let nodes = vec![n1, n2, n3];
        let state = TreeMapState::with_nodes(nodes, Some("3".to_string()));

        assert_eq!(state.rows.len(), 2);
        assert_eq!(state.rows[0].text, "Hello");
        assert_eq!(state.rows[1].text, "Second question");
    }

    #[test]
    fn fork_detection() {
        let mut n1 = make_node("1", None, "user", "Start");
        let mut n2 = make_node("2", Some("1"), "user", "Branch A");
        let n3 = make_node("3", Some("1"), "user", "Branch B");
        n1.child_ids = vec!["2".to_string(), "3".to_string()];
        n2.child_ids = vec![];

        let nodes = vec![n1, n2, n3];
        let state = TreeMapState::with_nodes(nodes, Some("2".to_string()));

        // n1 is on main path at depth 0
        assert_eq!(state.rows[0].fork_depth, 0);
        // n2 and n3 are fork children at depth 1
        let fork_rows: Vec<_> = state.rows.iter().filter(|r| r.fork_depth > 0).collect();
        assert_eq!(fork_rows.len(), 2);
        assert!(fork_rows.iter().all(|r| r.parent_is_fork));
    }

    #[test]
    fn hit_test_returns_correct_index() {
        let n1 = make_node("1", None, "user", "Msg 1");
        let n2 = make_node("2", Some("1"), "user", "Msg 2");
        let nodes = vec![n1, n2];
        let mut state = TreeMapState::with_nodes(nodes, Some("2".to_string()));
        state.list_rect = Some(Rect {
            x: 0,
            y: 2,
            width: 80,
            height: 10,
        });
        state.scroll = 0;

        assert_eq!(state.hit_test_row(5, 2), Some(0));
        assert_eq!(state.hit_test_row(5, 3), Some(1));
        assert_eq!(state.hit_test_row(5, 12), None); // out of bounds
    }
}
