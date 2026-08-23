//! Session / message code-review modal (PSM code-review → native Pager).
//!
//! Left: filterable file list. Center: embedded [`BlockViewerPane`] (same TUI as
//! Enter-on-edit). Optional lower Q&A panel: review-scoped Ask using the /btw model chain.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use similar::ChangeTag;
use unicode_width::UnicodeWidthStr;

use crate::render::line_utils::truncate_str;
use crate::scrollback::block::RenderBlock;
use crate::scrollback::blocks::ToolCallBlock;
use crate::scrollback::entry::{EntryId, ScrollbackEntry};
use crate::scrollback::state::ScrollbackState;
use crate::theme::Theme;
use crate::views::block_viewer::BlockViewerPane;

/// Which tool ops appear in the review list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewKindFilter {
    /// Edit + write only (default).
    #[default]
    Changes,
    /// Edit + write + read (`r` toggle / F2 `review_include_reads`).
    All,
    #[allow(dead_code)]
    Reads,
    #[allow(dead_code)]
    Shell,
}

impl ReviewKindFilter {
    pub fn includes_reads(self) -> bool {
        matches!(self, Self::All | Self::Reads)
    }

    pub fn with_reads(self, include: bool) -> Self {
        if include { Self::All } else { Self::Changes }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFileKind {
    Edit,
    Write,
    Read,
    #[allow(dead_code)]
    Shell,
}

#[derive(Debug, Clone)]
pub struct ReviewFileItem {
    pub path: String,
    pub kind: ReviewFileKind,
    /// Scrollback entry id for the primary (latest) edit op on this path.
    pub entry_id: EntryId,
    pub additions: usize,
    pub deletions: usize,
    pub is_error: bool,
    pub op_count: usize,
    /// Fallback plain text if `for_edit` cannot open (empty hunks).
    pub plain_fallback: String,
}

/// Focus target inside the review modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFocus {
    /// Left file list (j/k change files; `/` filters list).
    List,
    /// Right BlockViewerPane (full edit-viewer TUI).
    Preview,
    /// Right-side QA panel (`a` to activate; Enter submits via /btw model).
    Ask,
}

/// One visible row in the left pane (flat or tree).
#[derive(Debug, Clone)]
pub struct ReviewTreeRow {
    pub depth: u16,
    pub label: String,
    /// `Some(i)` → `files[i]` leaf; `None` → directory header.
    pub file_idx: Option<usize>,
    pub additions: usize,
    pub deletions: usize,
    pub kind: Option<ReviewFileKind>,
    pub is_error: bool,
    pub is_dir: bool,
    /// Stable dir identity (ancestor labels joined by `/`) for collapse state;
    /// `None` on leaf rows. Survives rebuilds so expand/collapse persists.
    pub dir_key: Option<String>,
}

/// QA response state inside the review modal.
#[derive(Debug, Clone)]
pub enum ReviewAskResponse {
    /// Streaming or waiting for the btw model to respond.
    Loading {
        content: Box<crate::scrollback::blocks::markdown_content::MarkdownContent>,
        scroll_offset: usize,
    },
    /// Response received — scrollable markdown.
    Done {
        content: Box<crate::scrollback::blocks::markdown_content::MarkdownContent>,
        scroll_offset: usize,
    },
    /// Request failed.
    Error { error: String },
}

/// State for the inline "Right Ask" QA panel in the review modal.
#[derive(Debug, Clone)]
pub struct ReviewAskState {
    /// Current input text (empty when not actively typing).
    pub input: String,
    /// UTF-8 byte offset within `input`, always on a character boundary.
    pub cursor: usize,
    /// Last submitted question (shown as context while loading/done).
    pub last_question: String,
    /// Response state.
    pub response: Option<ReviewAskResponse>,
}

impl Default for ReviewAskState {
    fn default() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            last_question: String::new(),
            response: None,
        }
    }
}

impl ReviewAskState {
    /// True when a request is in-flight.
    pub fn is_loading(&self) -> bool {
        matches!(self.response, Some(ReviewAskResponse::Loading { .. }))
    }

    /// True when there is a response or an in-flight streamed answer to display.
    pub fn has_response(&self) -> bool {
        self.response.is_some()
    }

    /// Begin a fresh streamed answer for the current review question.
    pub fn start_loading(&mut self) {
        self.response = Some(ReviewAskResponse::Loading {
            content: Box::new(
                crate::scrollback::blocks::markdown_content::MarkdownContent::new(String::new()),
            ),
            scroll_offset: 0,
        });
    }

    /// Append a streamed answer delta for the current review question.
    pub fn append_delta(&mut self, delta: &str) {
        if let Some(ReviewAskResponse::Loading { content, .. }) = self.response.as_mut() {
            let next = format!("{}{}", content.text(), delta);
            *content =
                Box::new(crate::scrollback::blocks::markdown_content::MarkdownContent::new(next));
        }
    }

    /// Set the final response from the btw result.
    pub fn set_response(&mut self, result: Result<String, String>) {
        match result {
            Ok(text) => {
                self.response = Some(ReviewAskResponse::Done {
                    content: Box::new(
                        crate::scrollback::blocks::markdown_content::MarkdownContent::new(text),
                    ),
                    scroll_offset: 0,
                });
            }
            Err(error) => {
                self.response = Some(ReviewAskResponse::Error { error });
            }
        }
    }

    /// Scroll the current streamed or completed response.
    pub fn scroll_response(&mut self, delta: i32, max_offset: usize) {
        let scroll_offset = match self.response.as_mut() {
            Some(ReviewAskResponse::Loading { scroll_offset, .. })
            | Some(ReviewAskResponse::Done { scroll_offset, .. }) => scroll_offset,
            _ => return,
        };
        if delta < 0 {
            *scroll_offset = scroll_offset.saturating_sub((-delta) as usize);
        } else {
            *scroll_offset = (*scroll_offset + delta as usize).min(max_offset);
        }
    }

    /// Max scroll offset for the streamed or completed response at given dimensions.
    pub fn max_response_scroll(&self, content_width: usize, visible_lines: usize) -> usize {
        let content = match &self.response {
            Some(ReviewAskResponse::Loading { content, .. })
            | Some(ReviewAskResponse::Done { content, .. })
                if content_width > 0 =>
            {
                content
            }
            _ => return 0,
        };
        let total = content.with_wrapped_lines(content_width, |w| w.lines.len());
        total.saturating_sub(visible_lines)
    }

    /// Clear the response panel (Esc when not typing).
    pub fn dismiss_response(&mut self) {
        self.response = None;
    }
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut boundary = index.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = floor_char_boundary(text, cursor);
    floor_char_boundary(text, cursor.saturating_sub(1))
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = floor_char_boundary(text, cursor);
    if cursor >= text.len() {
        return text.len();
    }
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(0)
}

pub struct ReviewState {
    pub title: String,
    pub files: Vec<ReviewFileItem>,
    /// Indices into `files` after list filter.
    pub filtered: Vec<usize>,
    /// Cursor into `nav_rows()` (filtered flat indices or tree rows).
    pub selected: usize,
    pub focus: ReviewFocus,
    pub filter: ReviewKindFilter,
    /// Entry index range used for extract (None = whole session).
    pub entry_range: Option<std::ops::Range<usize>>,
    /// File-list filter query (`/` when list focused).
    pub list_query: String,
    /// True while typing into the file-list filter bar.
    pub list_filter_active: bool,
    /// Tree layout (cwd-relative + compact Java packages). Default from F2.
    pub tree_mode: bool,
    /// Collapsed directory keys in tree mode (see `ReviewTreeRow::dir_key`).
    /// Cleared implicitly while a list filter query is active (auto-expand).
    pub collapsed_dirs: std::collections::BTreeSet<String>,
    /// `?` help overlay over the review surface; any key/click dismisses.
    pub help_visible: bool,
    /// Session cwd used to strip absolute prefixes in tree mode.
    pub cwd: String,
    /// Built when `tree_mode` (and after filter changes).
    pub tree_rows: Vec<ReviewTreeRow>,
    /// Right-pane viewer (rebuilt when selection changes).
    pub viewer: Option<BlockViewerPane>,
    /// Inline QA "Right Ask" state (reuses /btw model chain).
    pub ask: ReviewAskState,
    /// Hit areas from last render.
    pub list_area: Rect,
    /// Exact rows rect used for file items (inside border/title/filter bar).
    pub list_body_area: Rect,
    /// First nav-row index drawn at `list_body_area.y` last frame.
    pub list_view_start: usize,
    pub preview_area: Rect,
    pub popup_area: Rect,
    /// Lower Q&A panel area (for mouse hit-testing).
    pub ask_area: Rect,
}

impl ReviewState {
    pub fn new(
        title: impl Into<String>,
        files: Vec<ReviewFileItem>,
        filter: ReviewKindFilter,
    ) -> Self {
        Self::with_options(title, files, filter, false, String::new())
    }

    pub fn with_options(
        title: impl Into<String>,
        files: Vec<ReviewFileItem>,
        filter: ReviewKindFilter,
        tree_mode: bool,
        cwd: impl Into<String>,
    ) -> Self {
        Self::with_options_range(title, files, filter, tree_mode, cwd, None)
    }

    pub fn with_options_range(
        title: impl Into<String>,
        files: Vec<ReviewFileItem>,
        filter: ReviewKindFilter,
        tree_mode: bool,
        cwd: impl Into<String>,
        entry_range: Option<std::ops::Range<usize>>,
    ) -> Self {
        let filtered: Vec<usize> = (0..files.len()).collect();
        let mut state = Self {
            title: title.into(),
            files,
            filtered,
            selected: 0,
            focus: ReviewFocus::List,
            filter,
            entry_range,
            list_query: String::new(),
            list_filter_active: false,
            tree_mode,
            cwd: cwd.into(),
            collapsed_dirs: std::collections::BTreeSet::new(),
            help_visible: false,
            tree_rows: Vec::new(),
            viewer: None,
            ask: ReviewAskState::default(),
            list_area: Rect::default(),
            list_body_area: Rect::default(),
            list_view_start: 0,
            preview_area: Rect::default(),
            popup_area: Rect::default(),
            ask_area: Rect::default(),
        };
        state.rebuild_tree();
        state
    }

    /// Replace file set after filter change (preserve path cursor when possible).
    pub fn replace_files(&mut self, files: Vec<ReviewFileItem>, include_reads: bool) {
        let keep = self.current_file().map(|f| f.path.clone());
        self.files = files;
        self.filter = self.filter.with_reads(include_reads);
        self.list_query.clear();
        self.list_filter_active = false;
        self.filtered = (0..self.files.len()).collect();
        self.rebuild_tree();
        if let Some(path) = keep {
            self.select_path(&path);
        } else {
            self.selected = 0;
        }
        self.viewer = None;
        // Title count is best-effort; leave as-is if empty title pattern unknown.
        if let Some(prefix) = self.title.split('·').next() {
            let n = self.files.len();
            self.title = format!("{}· {} file(s)", prefix.trim_end(), n);
        }
    }

    pub fn set_tree_mode(&mut self, enabled: bool) {
        if self.tree_mode == enabled {
            return;
        }
        let keep = self.current_file().map(|f| f.path.clone());
        self.tree_mode = enabled;
        self.rebuild_tree();
        if let Some(path) = keep {
            self.select_path(&path);
        } else {
            self.selected = 0;
        }
        self.viewer = None;
    }

    pub fn toggle_tree_mode(&mut self) -> bool {
        self.set_tree_mode(!self.tree_mode);
        self.tree_mode
    }

    /// Expand the selected dir row. True when the view changed.
    pub fn expand_selected_dir(&mut self) -> bool {
        let Some(key) = self
            .tree_rows
            .get(self.selected)
            .filter(|r| r.is_dir)
            .and_then(|r| r.dir_key.clone())
        else {
            return false;
        };
        if !self.collapsed_dirs.remove(&key) {
            return false;
        }
        self.reselect_after_tree_rebuild(&key);
        true
    }

    /// Collapse the selected expanded dir; if on a leaf/collapsed dir, move to
    /// the nearest visible ancestor dir (neo-tree style `h`).
    pub fn collapse_dir_or_goto_parent(&mut self) -> bool {
        // Collapse in place when sitting on an expanded dir row.
        let expanded_key = match self.tree_rows.get(self.selected) {
            Some(row) if row.is_dir => match &row.dir_key {
                Some(k) if !self.collapsed_dirs.contains(k) => Some(k.clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(key) = expanded_key {
            self.collapsed_dirs.insert(key.clone());
            self.reselect_after_tree_rebuild(&key);
            return true;
        }
        // Otherwise jump up to the nearest ancestor header.
        let cur_depth = self
            .tree_rows
            .get(self.selected)
            .map(|r| r.depth)
            .unwrap_or(0);
        if cur_depth == 0 {
            return false;
        }
        let parent = (0..self.selected).rev().find(|&i| {
            self.tree_rows
                .get(i)
                .is_some_and(|r| r.is_dir && r.depth < cur_depth)
        });
        if let Some(i) = parent {
            self.selected = i;
            self.viewer = None;
            return true;
        }
        false
    }

    /// Toggle expand/collapse of the selected dir row (Enter in tree mode).
    pub fn toggle_selected_dir(&mut self) -> bool {
        let expanded_now = self.expand_selected_dir();
        if expanded_now {
            return true;
        }
        let Some(row) = self.tree_rows.get(self.selected) else {
            return false;
        };
        if !row.is_dir {
            return false;
        }
        let Some(key) = row.dir_key.clone() else {
            return false;
        };
        self.collapsed_dirs.insert(key.clone());
        self.reselect_after_tree_rebuild(&key);
        true
    }

    /// Rebuild rows and keep the given dir row selected (by stable key).
    fn reselect_after_tree_rebuild(&mut self, dir_key: &str) {
        let keep = self.current_file().map(|f| f.path.clone());
        self.rebuild_tree();
        if let Some(i) = self
            .tree_rows
            .iter()
            .position(|r| r.dir_key.as_deref() == Some(dir_key))
        {
            self.selected = i;
        } else if let Some(path) = keep {
            self.select_path(&path);
        }
        self.viewer = None;
    }

    fn rebuild_tree(&mut self) {
        if self.tree_mode {
            // While filtering, expand everything so matches stay visible.
            let force_expand = !self.list_query.is_empty();
            self.tree_rows = build_tree_rows(
                &self.files,
                &self.filtered,
                &self.cwd,
                &self.collapsed_dirs,
                force_expand,
            );
        } else {
            self.tree_rows.clear();
        }
        let max = self.nav_len().saturating_sub(1);
        self.selected = self.selected.min(max);
    }

    pub fn nav_len(&self) -> usize {
        if self.tree_mode {
            self.tree_rows.len()
        } else {
            self.filtered.len()
        }
    }

    pub fn current_file(&self) -> Option<&ReviewFileItem> {
        if self.tree_mode {
            let row = self.tree_rows.get(self.selected)?;
            let idx = row
                .file_idx
                .or_else(|| self.first_file_under_tree(self.selected))?;
            self.files.get(idx)
        } else {
            self.filtered
                .get(self.selected)
                .and_then(|&i| self.files.get(i))
        }
    }

    fn first_file_under_tree(&self, start: usize) -> Option<usize> {
        self.tree_rows[start..].iter().find_map(|r| r.file_idx)
    }

    fn select_path(&mut self, path: &str) {
        if self.tree_mode {
            if let Some(i) = self.tree_rows.iter().position(|r| {
                r.file_idx
                    .and_then(|fi| self.files.get(fi))
                    .is_some_and(|f| f.path == path)
            }) {
                self.selected = i;
            }
        } else if let Some(i) = self
            .filtered
            .iter()
            .position(|&fi| self.files.get(fi).is_some_and(|f| f.path == path))
        {
            self.selected = i;
        }
    }

    pub fn apply_list_filter(&mut self) {
        let keep = self.current_file().map(|f| f.path.clone());
        let q = self.list_query.to_lowercase();
        if q.is_empty() {
            self.filtered = (0..self.files.len()).collect();
        } else {
            self.filtered = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
        }
        self.rebuild_tree();
        if let Some(path) = keep {
            self.select_path(&path);
        } else {
            self.selected = 0;
        }
        self.viewer = None;
    }

    pub fn move_sel(&mut self, delta: i32) {
        let len = self.nav_len();
        if len == 0 {
            return;
        }
        let max = len as i32 - 1;
        let next = (self.selected as i32 + delta).clamp(0, max) as usize;
        if next != self.selected {
            self.selected = next;
            self.viewer = None;
        }
    }

    pub fn select_filtered(&mut self, idx: usize) {
        let len = self.nav_len();
        if len == 0 {
            return;
        }
        let new = idx.min(len - 1);
        if new != self.selected {
            self.selected = new;
            self.viewer = None;
        }
    }

    /// Keep `selected` in the last-drawn list viewport; no-op when all rows fit.
    pub fn ensure_list_visible(&mut self, viewport: usize) {
        let len = self.nav_len();
        if len == 0 {
            self.list_view_start = 0;
            return;
        }
        let viewport = viewport.max(1);
        if len <= viewport {
            self.list_view_start = 0;
            return;
        }
        let max_start = len - viewport;
        if self.selected < self.list_view_start {
            self.list_view_start = self.selected;
        } else if self.selected >= self.list_view_start + viewport {
            self.list_view_start = self.selected + 1 - viewport;
        }
        self.list_view_start = self.list_view_start.min(max_start);
    }

    /// Build / refresh the right-pane BlockViewerPane for the current file.
    pub fn ensure_viewer(&mut self, scrollback: &ScrollbackState) {
        let Some(file) = self.current_file().cloned() else {
            self.viewer = None;
            return;
        };
        if self
            .viewer
            .as_ref()
            .is_some_and(|v| v.entry_id == file.entry_id)
        {
            return;
        }
        let entry = scrollback.get_by_id(file.entry_id);
        self.viewer = match (file.kind, entry) {
            (ReviewFileKind::Read, Some(e)) => BlockViewerPane::for_read(file.entry_id, e)
                .or_else(|| Some(plain_review_fallback(&file.path, &file.plain_fallback))),
            (_, Some(e)) => BlockViewerPane::for_edit_review(file.entry_id, e)
                .or_else(|| Some(plain_review_fallback(&file.path, &file.plain_fallback))),
            (_, None) => Some(plain_review_fallback(&file.path, &file.plain_fallback)),
        };
    }
}

/// Plain-text fallback when an edit entry has no hunks (still searchable/wrappable).
fn plain_review_fallback(path: &str, text: &str) -> BlockViewerPane {
    // Number each line so empty-hunk writes still show a line gutter.
    let numbered = text
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{:>4} │ {}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let body = if numbered.is_empty() {
        text.to_string()
    } else {
        numbered
    };
    BlockViewerPane::for_plain_text(path, &body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewInput {
    Dismissed,
    /// Selection / focus changed — caller should ensure_viewer.
    Changed,
    /// Tree/flat toggled — caller persists `SetReviewFileTree(tree_mode)`.
    ToggleTree,
    /// Include-reads toggled — caller persists `SetReviewIncludeReads`.
    ToggleIncludeReads,
    /// Ctrl+click (or path hit) — open current file in OS default app.
    OpenPath,
    /// QA question submitted via the Right Ask bar — caller fires /btw.
    AskSubmit(String),
    Consumed,
}

pub fn handle_review_list_key(state: &mut ReviewState, key: &KeyEvent) -> ReviewInput {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return ReviewInput::Consumed;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return ReviewInput::Dismissed;
    }

    // Help overlay: any key closes it.
    if state.help_visible {
        state.help_visible = false;
        return ReviewInput::Changed;
    }

    // Enter on a dir row toggles its fold instead of jumping to preview.
    let enter_toggles_dir = state.tree_mode
        && state
            .tree_rows
            .get(state.selected)
            .is_some_and(|r| r.is_dir);

    // List filter bar active.
    if state.list_filter_active {
        match key.code {
            KeyCode::Esc => {
                state.list_filter_active = false;
                state.list_query.clear();
                state.apply_list_filter();
                return ReviewInput::Changed;
            }
            KeyCode::Enter => {
                state.list_filter_active = false;
                return ReviewInput::Changed;
            }
            KeyCode::Backspace => {
                state.list_query.pop();
                state.apply_list_filter();
                return ReviewInput::Changed;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.list_query.push(c);
                state.apply_list_filter();
                return ReviewInput::Changed;
            }
            _ => return ReviewInput::Consumed,
        }
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => ReviewInput::Dismissed,
        KeyCode::Enter if enter_toggles_dir => {
            state.toggle_selected_dir();
            ReviewInput::Changed
        }
        KeyCode::Tab | KeyCode::Right | KeyCode::Enter => {
            state.focus = ReviewFocus::Preview;
            ReviewInput::Changed
        }
        // h/l fold dirs in tree mode (neo-tree style).
        KeyCode::Left | KeyCode::Char('h') => {
            if state.tree_mode && state.collapse_dir_or_goto_parent() {
                ReviewInput::Changed
            } else {
                ReviewInput::Consumed
            }
        }
        KeyCode::Char('l') => {
            if state.tree_mode && state.expand_selected_dir() {
                ReviewInput::Changed
            } else {
                ReviewInput::Consumed
            }
        }
        KeyCode::Char('/') => {
            state.list_filter_active = true;
            ReviewInput::Changed
        }
        // t toggles tree/flat; caller persists via SetReviewFileTree.
        KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => ReviewInput::ToggleTree,
        // r toggles include-reads; caller persists via SetReviewIncludeReads.
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
            ReviewInput::ToggleIncludeReads
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_sel(-1);
            ReviewInput::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_sel(1);
            ReviewInput::Changed
        }
        KeyCode::PageUp => {
            state.move_sel(-10);
            ReviewInput::Changed
        }
        KeyCode::PageDown => {
            state.move_sel(10);
            ReviewInput::Changed
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.select_filtered(0);
            ReviewInput::Changed
        }
        KeyCode::End | KeyCode::Char('G') => {
            let last = state.nav_len().saturating_sub(1);
            state.select_filtered(last);
            ReviewInput::Changed
        }
        // . / , switch files without leaving the modal (hunk convention;
        // n/N stay reserved for viewer search-next/prev).
        KeyCode::Char('.') => {
            state.move_sel(1);
            ReviewInput::Changed
        }
        KeyCode::Char(',') => {
            state.move_sel(-1);
            ReviewInput::Changed
        }
        // a activates the Right Ask QA input bar.
        KeyCode::Char('a') if key.modifiers == KeyModifiers::NONE => {
            state.focus = ReviewFocus::Ask;
            ReviewInput::Changed
        }
        KeyCode::Char('?') => {
            state.help_visible = true;
            ReviewInput::Changed
        }
        _ => ReviewInput::Consumed,
    }
}

/// Keys handled at the review shell when preview is focused (before routing to viewer).
pub fn handle_review_preview_shell_key(
    state: &mut ReviewState,
    key: &KeyEvent,
) -> Option<ReviewInput> {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return Some(ReviewInput::Consumed);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(ReviewInput::Dismissed);
    }

    // Help overlay: any key closes it.
    if state.help_visible {
        state.help_visible = false;
        return Some(ReviewInput::Changed);
    }

    // Don't steal keys while viewer is in search/filter/visual mode.
    // (While a search matcher is active, n/N pass through to the viewer.)
    if let Some(v) = state.viewer.as_ref() {
        if v.list_state.input_mode().is_some() || v.list_state.visual_mode {
            return None;
        }
    }

    match key.code {
        KeyCode::Left | KeyCode::Char('h') => {
            state.focus = ReviewFocus::List;
            Some(ReviewInput::Changed)
        }
        // ] / [ jump hunk-by-hunk through the diff preview.
        KeyCode::Char(']') | KeyCode::Char('[') => {
            let next = key.code == KeyCode::Char(']');
            let jumped = state
                .viewer
                .as_mut()
                .is_some_and(|v| v.jump_hunk(if next { 1 } else { -1 }));
            if jumped {
                Some(ReviewInput::Changed)
            } else {
                Some(ReviewInput::Consumed)
            }
        }
        // . / , switch files without leaving preview focus (hunk convention;
        // n/N stay reserved for the viewer's search-next/prev).
        KeyCode::Char('.') => {
            state.move_sel(1);
            Some(ReviewInput::Changed)
        }
        KeyCode::Char(',') => {
            state.move_sel(-1);
            Some(ReviewInput::Changed)
        }
        // d / u half-page scroll (less convention); g/G/Ctrl-d/u pass through.
        KeyCode::Char('d') | KeyCode::Char('u') if key.modifiers == KeyModifiers::NONE => {
            let down = key.code == KeyCode::Char('d');
            if let Some(v) = state.viewer.as_mut() {
                v.half_page_scroll(down);
            }
            Some(ReviewInput::Changed)
        }
        KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => Some(ReviewInput::ToggleTree),
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
            Some(ReviewInput::ToggleIncludeReads)
        }
        // a activates the Right Ask QA bar.
        KeyCode::Char('a') if key.modifiers == KeyModifiers::NONE => {
            state.focus = ReviewFocus::Ask;
            Some(ReviewInput::Changed)
        }
        KeyCode::Char('?') => {
            state.help_visible = true;
            Some(ReviewInput::Changed)
        }
        KeyCode::Tab => {
            state.focus = ReviewFocus::List;
            Some(ReviewInput::Changed)
        }
        _ => None,
    }
}

/// Key handling when the Right Ask QA bar is focused.
pub fn handle_review_ask_key(state: &mut ReviewState, key: &KeyEvent) -> ReviewInput {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return ReviewInput::Consumed;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return ReviewInput::Dismissed;
    }

    let ask = &mut state.ask;
    ask.cursor = floor_char_boundary(&ask.input, ask.cursor);

    match key.code {
        KeyCode::Esc => {
            if ask.is_loading() {
                // Can't dismiss while loading.
                ReviewInput::Consumed
            } else if ask.has_response() {
                ask.dismiss_response();
                ReviewInput::Changed
            } else {
                // Return to list focus.
                state.focus = ReviewFocus::List;
                ReviewInput::Changed
            }
        }
        KeyCode::Enter => {
            let q = ask.input.trim().to_string();
            if q.is_empty() || ask.is_loading() {
                return ReviewInput::Consumed;
            }
            ask.last_question = q.clone();
            ask.input.clear();
            ask.cursor = 0;
            ask.start_loading();
            ReviewInput::AskSubmit(q)
        }
        KeyCode::Backspace => {
            if ask.cursor > 0 {
                let previous = previous_char_boundary(&ask.input, ask.cursor);
                ask.input.remove(previous);
                ask.cursor = previous;
            }
            ReviewInput::Changed
        }
        KeyCode::Delete => {
            if ask.cursor < ask.input.len() {
                ask.input.remove(ask.cursor);
            }
            ReviewInput::Changed
        }
        KeyCode::Left => {
            ask.cursor = previous_char_boundary(&ask.input, ask.cursor);
            ReviewInput::Changed
        }
        KeyCode::Right => {
            ask.cursor = next_char_boundary(&ask.input, ask.cursor);
            ReviewInput::Changed
        }
        KeyCode::Home => {
            ask.cursor = 0;
            ReviewInput::Changed
        }
        KeyCode::End => {
            ask.cursor = ask.input.len();
            ReviewInput::Changed
        }
        KeyCode::Up => {
            // Scroll streamed or completed response up.
            if matches!(
                &ask.response,
                Some(ReviewAskResponse::Loading { .. }) | Some(ReviewAskResponse::Done { .. })
            ) {
                let max = ask.max_response_scroll(
                    state.ask_area.width.saturating_sub(2) as usize,
                    state.ask_area.height.saturating_sub(2) as usize,
                );
                ask.scroll_response(-3, max);
            }
            ReviewInput::Changed
        }
        KeyCode::Down => {
            // Scroll streamed or completed response down.
            if matches!(
                &ask.response,
                Some(ReviewAskResponse::Loading { .. }) | Some(ReviewAskResponse::Done { .. })
            ) {
                let max = ask.max_response_scroll(
                    state.ask_area.width.saturating_sub(2) as usize,
                    state.ask_area.height.saturating_sub(2) as usize,
                );
                ask.scroll_response(3, max);
            }
            ReviewInput::Changed
        }
        KeyCode::Tab => {
            state.focus = ReviewFocus::List;
            ReviewInput::Changed
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            ask.input.insert(ask.cursor, c);
            ask.cursor += c.len_utf8();
            ReviewInput::Changed
        }
        _ => ReviewInput::Consumed,
    }
}

fn count_hunk_changes(hunks: &[xai_grok_pager_diff::DiffHunk]) -> (usize, usize) {
    let mut additions = 0;
    let mut deletions = 0;
    for hunk in hunks {
        for line in hunk {
            match line.tag {
                ChangeTag::Insert => additions += 1,
                ChangeTag::Delete => deletions += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    (additions, deletions)
}

/// Extract reviewable file ops. `range` = entry indices; `None` = whole session.
pub fn extract_review_files(
    scrollback: &ScrollbackState,
    range: Option<std::ops::Range<usize>>,
    filter: ReviewKindFilter,
) -> Vec<ReviewFileItem> {
    if matches!(filter, ReviewKindFilter::Shell) {
        return Vec::new();
    }
    let want_changes = matches!(filter, ReviewKindFilter::Changes | ReviewKindFilter::All);
    let want_reads = filter.includes_reads();
    if !want_changes && !want_reads {
        return Vec::new();
    }

    let (start, end) = match range {
        Some(r) => (r.start, r.end.min(scrollback.len())),
        None => (0, scrollback.len()),
    };

    let mut by_path: Vec<(String, ReviewFileItem)> = Vec::new();
    let mut index_of = std::collections::HashMap::<String, usize>::new();

    for (idx, (entry_id, entry)) in scrollback.iter_entries().enumerate() {
        if idx < start || idx >= end {
            continue;
        }
        match &entry.block {
            RenderBlock::ToolCall(ToolCallBlock::Edit(edit)) if want_changes => {
                let is_write = edit.prefix.starts_with("Creating");
                let kind = if is_write {
                    ReviewFileKind::Write
                } else {
                    ReviewFileKind::Edit
                };
                let path = edit.path.clone();
                let (additions, deletions) = count_hunk_changes(&edit.hunks);
                let plain_fallback = if edit.hunks.is_empty() {
                    if is_write {
                        format!("(new file) {path}\n")
                    } else {
                        format!("(no diff captured) {path}\n")
                    }
                } else {
                    edit.copy_text()
                };
                upsert_review_item(
                    &mut by_path,
                    &mut index_of,
                    path,
                    kind,
                    entry_id,
                    additions,
                    deletions,
                    edit.error.is_some(),
                    plain_fallback,
                    /*prefer_kind_over_read*/ true,
                );
            }
            RenderBlock::ToolCall(ToolCallBlock::Read(read)) if want_reads => {
                let path = read.path.clone();
                let plain_fallback = read.content.clone().unwrap_or_else(|| {
                    read.error
                        .clone()
                        .unwrap_or_else(|| format!("(empty read) {path}\n"))
                });
                upsert_review_item(
                    &mut by_path,
                    &mut index_of,
                    path,
                    ReviewFileKind::Read,
                    entry_id,
                    0,
                    0,
                    read.error.is_some(),
                    plain_fallback,
                    /*prefer_kind_over_read*/ false,
                );
            }
            _ => {}
        }
    }

    by_path.into_iter().map(|(_, item)| item).collect()
}

fn upsert_review_item(
    by_path: &mut Vec<(String, ReviewFileItem)>,
    index_of: &mut std::collections::HashMap<String, usize>,
    path: String,
    kind: ReviewFileKind,
    entry_id: EntryId,
    additions: usize,
    deletions: usize,
    is_error: bool,
    plain_fallback: String,
    prefer_kind_over_read: bool,
) {
    if let Some(&i) = index_of.get(&path) {
        let item = &mut by_path[i].1;
        item.op_count += 1;
        item.additions += additions;
        item.deletions += deletions;
        if is_error {
            item.is_error = true;
        }
        // Prefer latest change over pure read when both exist for same path.
        let replace = match (item.kind, kind) {
            (ReviewFileKind::Read, _) if prefer_kind_over_read => true,
            (_, ReviewFileKind::Read)
                if !prefer_kind_over_read && item.kind != ReviewFileKind::Read =>
            {
                // Keep existing edit/write as primary viewer; still count op.
                false
            }
            _ => true,
        };
        if replace {
            item.entry_id = entry_id;
            item.kind = kind;
            item.plain_fallback = plain_fallback;
        }
    } else {
        index_of.insert(path.clone(), by_path.len());
        by_path.push((
            path.clone(),
            ReviewFileItem {
                path,
                kind,
                entry_id,
                additions,
                deletions,
                is_error,
                op_count: 1,
                plain_fallback,
            },
        ));
    }
}

pub fn turn_range_for_prompt(
    scrollback: &ScrollbackState,
    prompt_id: EntryId,
) -> Option<std::ops::Range<usize>> {
    let entry_idx = scrollback.index_of_id(prompt_id)?;
    scrollback
        .turns()
        .iter()
        .find(|t| t.prompt_index == entry_idx)
        .map(|t| t.range())
}

/// Render modal chrome + file list + BlockViewer content.
pub fn render_review_modal(
    buf: &mut Buffer,
    area: Rect,
    state: &mut ReviewState,
    scrollback: &ScrollbackState,
) {
    let theme = Theme::current();
    if area.width < 20 || area.height < 6 {
        return;
    }

    state.ensure_viewer(scrollback);
    state.popup_area = area;

    let base = Style::default().fg(theme.text_primary).bg(theme.bg_base);
    Clear.render(area, buf);
    buf.set_style(area, base);

    let footer = match state.focus {
        ReviewFocus::List if state.list_filter_active => {
            format!(" filter: {}  Enter=done  Esc=clear ", state.list_query)
        }
        ReviewFocus::List => {
            let fold = if state.tree_mode { "h/l fold  Enter=dir " } else { "" };
            format!(
                " j/k  /=filter  .=next ,=prev  {}t={}  r={}  Enter→preview  ?=help  Esc ",
                fold,
                if state.tree_mode { "flat" } else { "tree" },
                if state.filter.includes_reads() { "on" } else { "off" }
            )
        }
        ReviewFocus::Preview => {
            " ]/[ hunk  ./, file  /search n/N  d/u half  g/G  w=wrap  y=copy  a=ask  ?=help  ←list  Esc ".into()
        }
        ReviewFocus::Ask => " Enter=send  ↑↓ scroll answer  Tab=files  Esc=back ".into(),
    };

    let border = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.gray_dim))
        .title(Span::styled(
            format!(" {} ", state.title),
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(theme.gray)));
    let inner = border.inner(area);
    border.render(area, buf);
    if inner.width < 10 || inner.height < 3 {
        return;
    }

    let ask_visible = state.focus == ReviewFocus::Ask
        || !state.ask.input.is_empty()
        || state.ask.has_response()
        || state.ask.is_loading();

    if ask_visible {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
            .split(inner);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
            .split(chunks[1]);
        state.list_area = chunks[0];
        state.preview_area = right[0];
        state.ask_area = right[1];

        render_file_list(buf, chunks[0], state, &theme);
        render_preview_pane(buf, right[0], state, scrollback, &theme);
        render_ask_bar(buf, right[1], state, &theme);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
            .split(inner);
        state.list_area = chunks[0];
        state.preview_area = chunks[1];
        state.ask_area = Rect::default();

        render_file_list(buf, chunks[0], state, &theme);
        render_preview_pane(buf, chunks[1], state, scrollback, &theme);
    }

    // Help overlay sits on top of the whole modal; any key/click dismisses.
    if state.help_visible {
        render_help_overlay(buf, area, &theme);
    }
}

/// Full keymap overlay (`?`). Static content; dismissed by any key or click.
fn render_help_overlay(buf: &mut Buffer, outer: Rect, theme: &Theme) {
    let width = (outer.width.saturating_sub(4)).min(78).max(20);
    let height = (outer.height.saturating_sub(2)).min(26).max(6);
    let x = outer.x + (outer.width.saturating_sub(width)) / 2;
    let y = outer.y + (outer.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    Clear.render(area, buf);
    let border = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent_tool))
        .title(Span::styled(
            " Review keybindings ",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " any key closes ",
            Style::default().fg(theme.gray),
        ));
    let inner = border.inner(area);
    border.render(area, buf);

    let section = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme.warning));
    let key_span = |k: &str| Span::styled(k.to_string(), Style::default().fg(theme.accent_tool));
    let desc = |d: &str| Span::styled(d.to_string(), Style::default().fg(theme.text_primary));
    let row = |k: &str, d: &str| Line::from(vec![key_span(k), desc(d)]);

    let lines = vec![
        Line::from(section("Files list")),
        row("  j/k g/G      ", "move / jump"),
        row("  . ,          ", "next / previous file"),
        row("  /            ", "filter files (auto-expands tree)"),
        row(
            "  h l Enter    ",
            "collapse / expand / toggle dir (tree, or click)",
        ),
        row("  t r          ", "toggle tree mode / include reads"),
        row("  a Tab        ", "ask panel / switch focus"),
        Line::from(section("Diff preview")),
        row("  ] [          ", "next / previous diff hunk"),
        row("  j/k d/u g/G  ", "line / half-page / ends"),
        row("  / then n N   ", "search, next / previous match"),
        row("  f w y v      ", "filter / wrap / copy patch / select"),
        row("  . , ←       ", "switch file / back to list"),
        Line::from(section("Ask panel")),
        row("  Enter ↑↓     ", "send question / scroll answer"),
        Line::from(section("Global")),
        row("  ? q Esc      ", "this help / dismiss modal"),
        row("  Ctrl+click   ", "open file in OS default app"),
    ];
    Paragraph::new(lines).render(inner, buf);
}

fn render_file_list(buf: &mut Buffer, area: Rect, state: &mut ReviewState, theme: &Theme) {
    let focused = state.focus == ReviewFocus::List;
    // TOP|RIGHT so the title owns a real top border row — content y matches hit-test.
    let list_border = Block::default()
        .borders(Borders::TOP | Borders::RIGHT)
        .border_style(Style::default().fg(theme.gray_dim))
        .title(Span::styled(
            format!(
                " {} ({}/{}) ",
                if state.tree_mode { "Tree" } else { "Files" },
                state.nav_len(),
                state.files.len()
            ),
            Style::default().fg(if focused {
                theme.accent_tool
            } else {
                theme.gray
            }),
        ));
    let list_inner = list_border.inner(area);
    list_border.render(area, buf);

    // Filter bar row at bottom of list when active / non-empty query.
    let (rows_area, filter_row) = if state.list_filter_active || !state.list_query.is_empty() {
        if list_inner.height < 2 {
            (list_inner, None)
        } else {
            let body = Rect::new(
                list_inner.x,
                list_inner.y,
                list_inner.width,
                list_inner.height.saturating_sub(1),
            );
            let bar = Rect::new(
                list_inner.x,
                list_inner.y + list_inner.height.saturating_sub(1),
                list_inner.width,
                1,
            );
            (body, Some(bar))
        }
    } else {
        (list_inner, None)
    };

    // Cache exact body geometry for mouse hit-testing (must match draw loop).
    state.list_body_area = rows_area;

    if let Some(bar) = filter_row {
        let label = if state.list_filter_active {
            format!("/{}", state.list_query)
        } else {
            format!("~{}", state.list_query)
        };
        let line = Line::from(Span::styled(
            truncate_str(&label, bar.width as usize),
            Style::default().fg(theme.accent_tool).bg(theme.bg_dark),
        ));
        buf.set_style(bar, Style::default().bg(theme.bg_dark));
        buf.set_line(bar.x, bar.y, &line, bar.width);
    }

    let nav_len = state.nav_len();
    if nav_len == 0 {
        state.list_view_start = 0;
        let empty = Paragraph::new(Line::from(Span::styled(
            if state.files.is_empty() {
                "No file changes"
            } else {
                "No match"
            },
            Style::default().fg(theme.gray),
        )));
        empty.render(rows_area, buf);
        return;
    }

    let visible = rows_area.height as usize;
    // Sticky viewport: only scroll when selection leaves the window.
    // When the whole tree fits, force start=0 so the top is never clipped.
    state.ensure_list_visible(visible);
    let start = state.list_view_start;
    let end = (start + visible).min(nav_len);

    for (row, nav_i) in (start..end).enumerate() {
        let y = rows_area.y + row as u16;
        if y >= rows_area.y + rows_area.height {
            break;
        }
        let selected = nav_i == state.selected;
        let row_bg = if selected {
            theme.bg_highlight
        } else {
            theme.bg_base
        };
        let row_area = Rect::new(rows_area.x, y, rows_area.width, 1);
        buf.set_style(row_area, Style::default().bg(row_bg));

        if state.tree_mode {
            let collapsed = state
                .tree_rows
                .get(nav_i)
                .and_then(|r| r.dir_key.as_ref())
                .is_some_and(|k| state.collapsed_dirs.contains(k));
            render_tree_row(
                buf,
                rows_area.x,
                y,
                rows_area.width,
                &state.tree_rows[nav_i],
                selected,
                row_bg,
                theme,
                collapsed,
            );
        } else {
            let file_i = state.filtered[nav_i];
            let file = &state.files[file_i];
            render_flat_row(
                buf,
                rows_area.x,
                y,
                rows_area.width,
                file,
                selected,
                row_bg,
                theme,
            );
        }
    }
}

fn render_flat_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    file: &ReviewFileItem,
    selected: bool,
    row_bg: ratatui::style::Color,
    theme: &Theme,
) {
    let kind_tag = kind_tag(file.kind);
    let icon = file_type_icon(&file.path, false);
    let prefix_w = 2 + icon.map(|_| 2).unwrap_or(0); // "~ " + optional "󰈙 "
    let name = truncate_str(
        &basename(&file.path),
        width.saturating_sub(prefix_w) as usize,
    );
    let kind_fg = kind_fg(file.kind, theme);
    let mut spans = vec![Span::styled(
        format!("{kind_tag} "),
        Style::default().fg(kind_fg).bg(row_bg),
    )];
    if let Some(ic) = icon {
        spans.push(Span::styled(
            format!("{ic} "),
            Style::default().fg(theme.accent_tool).bg(row_bg),
        ));
    }
    spans.push(Span::styled(
        name,
        Style::default()
            .fg(if file.is_error {
                theme.accent_error
            } else {
                theme.text_primary
            })
            .bg(row_bg)
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    ));
    buf.set_line(x, y, &Line::from(spans), width);
}

fn render_tree_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    row: &ReviewTreeRow,
    selected: bool,
    row_bg: ratatui::style::Color,
    theme: &Theme,
    collapsed: bool,
) {
    // Tree gutters: "│ " rails + branch marker; less double-space clutter.
    let mut gutter = String::new();
    for d in 0..row.depth {
        if d + 1 == row.depth {
            gutter.push_str(if row.is_dir { "├─" } else { "└─" });
        } else {
            gutter.push_str("│ ");
        }
    }
    let glyph = if row.is_dir {
        if collapsed { "▸ " } else { "▾ " }
    } else {
        " "
    };
    let tag = row.kind.map(kind_tag).unwrap_or(" ");
    // Tree leaf labels are basenames; dirs use folder glyph.
    let icon = file_type_icon(&row.label, row.is_dir);
    let kind_part = format!("{tag} ");
    let icon_part = icon.map(|ic| format!("{ic} ")).unwrap_or_default();
    let prefix = format!("{gutter}{glyph}");
    let name_w =
        width.saturating_sub(prefix.len() as u16 + kind_part.len() as u16 + icon_part.len() as u16);
    let name = truncate_str(&row.label, name_w as usize);
    let kind_fg = row
        .kind
        .map(|k| kind_fg(k, theme))
        .unwrap_or(theme.gray_bright);
    let mut spans = vec![
        Span::styled(prefix, Style::default().fg(theme.gray_dim).bg(row_bg)),
        Span::styled(kind_part, Style::default().fg(kind_fg).bg(row_bg)),
    ];
    if !icon_part.is_empty() {
        spans.push(Span::styled(
            icon_part,
            Style::default()
                .fg(if row.is_dir {
                    theme.warning
                } else {
                    theme.accent_tool
                })
                .bg(row_bg),
        ));
    }
    spans.push(Span::styled(
        name,
        Style::default()
            .fg(if row.is_error {
                theme.accent_error
            } else if row.is_dir {
                theme.gray_bright
            } else {
                theme.text_primary
            })
            .bg(row_bg)
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    ));
    buf.set_line(x, y, &Line::from(spans), width);
}

fn kind_tag(kind: ReviewFileKind) -> &'static str {
    match kind {
        ReviewFileKind::Write => "+",
        ReviewFileKind::Edit => "~",
        ReviewFileKind::Read => "r",
        ReviewFileKind::Shell => "$",
    }
}

fn kind_fg(kind: ReviewFileKind, theme: &Theme) -> ratatui::style::Color {
    match kind {
        ReviewFileKind::Write => theme.accent_success,
        ReviewFileKind::Edit => theme.warning,
        ReviewFileKind::Read => theme.accent_tool,
        ReviewFileKind::Shell => theme.gray,
    }
}

/// Nerd Font file/dir glyph when ambient probe says PUA is safe; else `None`.
/// Uses [`crate::git_info::nerd_fonts_available`] (env `GROK_NERD_FONTS` override).
fn file_type_icon(path: &str, is_dir: bool) -> Option<&'static str> {
    if !crate::git_info::nerd_fonts_available() {
        return None;
    }
    Some(nerd_file_icon(path, is_dir))
}

/// Minimal nvim-web-devicons-style map (common review paths only).
fn nerd_file_icon(path: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "\u{f07b}"; // nf-fa-folder
    }
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    match name.as_str() {
        // Match nvim-web-devicons: nf-seti-rust U+E68B (), not nf-dev-rust U+E7A8 (often a blob).
        "cargo.toml" | "cargo.lock" => return "\u{e68b}",
        "license" | "license.md" | "license.txt" => return "\u{f02d}",
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "\u{f1d3}",
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => return "\u{f308}",
        "makefile" | "justfile" => return "\u{f489}",
        "package.json" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" => return "\u{e718}",
        "tsconfig.json" | "jsconfig.json" => return "\u{e628}",
        "readme.md" | "readme" => return "\u{f48a}",
        "security.md" => return "\u{f21b}",
        _ => {}
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "rs" => "\u{e68b}", // nf-seti-rust 
        "ts" | "tsx" | "mts" | "cts" => "\u{e628}",
        "js" | "jsx" | "mjs" | "cjs" => "\u{e781}",
        "py" => "\u{e73c}",
        "go" => "\u{e724}",
        "java" => "\u{e738}",
        "kt" | "kts" => "\u{e634}",
        "c" | "h" => "\u{e61e}",
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
        "cs" => "\u{f81a}",
        "rb" => "\u{e739}",
        "php" => "\u{e73d}",
        "swift" => "\u{e755}",
        "md" | "mdx" | "markdown" => "\u{f48a}",
        "json" | "jsonc" => "\u{e60b}",
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => "\u{e615}",
        "xml" | "html" | "htm" => "\u{e736}",
        "css" | "scss" | "sass" | "less" => "\u{e749}",
        "vue" => "\u{e6a0}",
        "svelte" => "\u{e697}",
        "sh" | "bash" | "zsh" | "fish" | "ps1" => "\u{f489}",
        "sql" => "\u{f1c0}",
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => "\u{f1c5}",
        "pdf" => "\u{f1c1}",
        "txt" | "log" => "\u{f15c}",
        "lock" => "\u{f023}",
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" => "\u{f1c6}",
        "wasm" => "\u{e6a1}",
        "proto" => "\u{e60a}",
        "graphql" | "gql" => "\u{e662}",
        _ => "\u{f15b}", // default file
    }
}

fn render_preview_pane(
    buf: &mut Buffer,
    area: Rect,
    state: &mut ReviewState,
    scrollback: &ScrollbackState,
    theme: &Theme,
) {
    let focused = state.focus == ReviewFocus::Preview;
    let title = state
        .current_file()
        .map(|f| {
            let ops = if f.op_count > 1 {
                format!(" ×{}", f.op_count)
            } else {
                String::new()
            };
            format!(" {}{} ", f.path, ops)
        })
        .unwrap_or_else(|| " (no selection) ".into());

    let border = Block::default().title(Span::styled(
        title,
        Style::default().fg(if focused {
            theme.accent_tool
        } else {
            theme.gray
        }),
    ));
    let inner = border.inner(area);
    border.render(area, buf);

    let Some(viewer) = state.viewer.as_mut() else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "Select a file",
            Style::default().fg(theme.gray),
        )));
        empty.render(inner, buf);
        return;
    };

    // Dummy entry for plain-text / missing; prefer live scrollback entry.
    let entry_owned;
    let entry: &ScrollbackEntry = if let Some(e) = scrollback.get_by_id(viewer.entry_id) {
        e
    } else {
        entry_owned = ScrollbackEntry::new(RenderBlock::system(String::new()));
        &entry_owned
    };

    // No preamble in review sidebar — full height for ListPane content.
    let prepend: &[Line<'static>] = &[];
    viewer.render_content(inner, buf, entry, focused, prepend);
}

/// Render the lower review Q&A panel.
fn render_ask_bar(buf: &mut Buffer, area: Rect, state: &ReviewState, theme: &Theme) {
    if area.width < 4 || area.height < 1 {
        return;
    }
    let ask = &state.ask;
    let focused = state.focus == ReviewFocus::Ask;

    let border_color = if focused {
        theme.warning
    } else {
        theme.gray_dim
    };
    let status = match &ask.response {
        Some(ReviewAskResponse::Loading { .. }) => "streaming",
        Some(ReviewAskResponse::Done { .. }) => "answer",
        Some(ReviewAskResponse::Error { .. }) => "error",
        None => "",
    };
    let title = if status.is_empty() {
        " Ask ".to_string()
    } else {
        format!(" Ask · {status} ")
    };
    let border = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default().fg(if focused { theme.warning } else { theme.gray }),
        ));
    let inner = border.inner(area);
    border.render(area, buf);
    if inner.width < 2 || inner.height < 3 {
        return;
    }

    let input_y = inner.y + inner.height - 1;
    let separator_y = input_y.saturating_sub(1);
    let response_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        separator_y.saturating_sub(inner.y),
    );
    buf.set_string(
        inner.x,
        separator_y,
        "─".repeat(inner.width as usize),
        Style::default().fg(theme.gray_dim),
    );

    let placeholder = ask.input.is_empty();
    let max_input_w = inner.width.saturating_sub(2) as usize;
    let input_line = Line::from(vec![
        Span::styled(
            "❯ ",
            Style::default().fg(if focused { theme.warning } else { theme.gray }),
        ),
        Span::styled(
            truncate_str(
                if placeholder {
                    "Ask about this review…"
                } else {
                    &ask.input
                },
                max_input_w,
            ),
            Style::default().fg(if placeholder {
                theme.gray_dim
            } else if focused {
                theme.text_primary
            } else {
                theme.gray_bright
            }),
        ),
    ]);
    buf.set_line(inner.x, input_y, &input_line, inner.width);

    // The cursor belongs only to text currently being edited, never to a submitted question.
    if focused && !placeholder && max_input_w > 0 {
        let cursor = floor_char_boundary(&ask.input, ask.cursor);
        let cursor_col =
            UnicodeWidthStr::width(&ask.input[..cursor]).min(max_input_w.saturating_sub(1)) as u16;
        let cursor_x = inner.x + 2 + cursor_col;
        if cursor_x < inner.x + inner.width
            && let Some(cell) = buf.cell_mut((cursor_x, input_y))
        {
            cell.set_style(Style::default().fg(theme.bg_base).bg(theme.warning));
        }
    }

    if response_area.height > 0 {
        render_ask_response(buf, response_area, ask, theme);
    }
}

fn render_ask_response(buf: &mut Buffer, area: Rect, ask: &ReviewAskState, theme: &Theme) {
    let mut response_area = area;
    if !ask.last_question.is_empty() {
        let question = Line::from(vec![
            Span::styled(" Q  ", Style::default().fg(theme.warning)),
            Span::styled(
                truncate_str(&ask.last_question, area.width.saturating_sub(4) as usize),
                Style::default().fg(theme.gray_bright),
            ),
        ]);
        buf.set_line(area.x, area.y, &question, area.width);
        if response_area.height == 1 {
            return;
        }
        response_area.y += 1;
        response_area.height -= 1;
    }

    match &ask.response {
        Some(ReviewAskResponse::Loading {
            content,
            scroll_offset,
        }) if content.text().is_empty() => {
            let spinner = Line::from(Span::styled(
                "  ⠿ thinking…",
                Style::default().fg(theme.gray),
            ));
            buf.set_line(
                response_area.x,
                response_area.y,
                &spinner,
                response_area.width,
            );
        }
        Some(ReviewAskResponse::Loading {
            content,
            scroll_offset,
        })
        | Some(ReviewAskResponse::Done {
            content,
            scroll_offset,
        }) => render_ask_content(buf, response_area, content, *scroll_offset, theme),
        Some(ReviewAskResponse::Error { error }) => {
            let err_line = Line::from(Span::styled(
                truncate_str(&format!("  ✗ {error}"), response_area.width as usize),
                Style::default().fg(theme.accent_error),
            ));
            buf.set_line(
                response_area.x,
                response_area.y,
                &err_line,
                response_area.width,
            );
        }
        None => {
            let hint = Line::from(Span::styled(
                "  Ask a question about the selected changes",
                Style::default().fg(theme.gray_dim),
            ));
            buf.set_line(response_area.x, response_area.y, &hint, response_area.width);
        }
    }
}

fn render_ask_content(
    buf: &mut Buffer,
    area: Rect,
    content: &crate::scrollback::blocks::markdown_content::MarkdownContent,
    scroll_offset: usize,
    theme: &Theme,
) {
    let content_w = area.width.saturating_sub(1) as usize;
    if content_w == 0 {
        return;
    }
    content.with_wrapped_lines(content_w, |wrapped| {
        let total = wrapped.lines.len();
        let visible = area.height as usize;
        let start = scroll_offset.min(total.saturating_sub(visible));
        let end = (start + visible).min(total);
        for (row, line_idx) in (start..end).enumerate() {
            let y = area.y + row as u16;
            if y >= area.y + area.height {
                break;
            }
            buf.set_line(
                area.x + 1,
                y,
                &wrapped.lines[line_idx],
                area.width.saturating_sub(1),
            );
        }
        if total > visible {
            let pct = start * 100 / (total - visible);
            let indicator = format!(" {pct}% ");
            let ind_x = area.x + area.width.saturating_sub(indicator.len() as u16 + 1);
            buf.set_string(
                ind_x,
                area.y,
                &indicator,
                Style::default().fg(theme.gray_dim),
            );
        }
    });
}

/// Map a mouse position to a filtered file index using last-frame draw geometry.
pub fn list_row_at(state: &ReviewState, col: u16, row: u16) -> Option<usize> {
    let area = state.list_body_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    if col < area.x || col >= area.x + area.width || row < area.y || row >= area.y + area.height {
        return None;
    }
    let offset = (row - area.y) as usize;
    let idx = state.list_view_start.saturating_add(offset);
    (idx < state.nav_len()).then_some(idx)
}

pub fn handle_review_mouse(state: &mut ReviewState, mouse: &MouseEvent) -> ReviewInput {
    // Any click over the modal dismisses the help overlay first.
    if state.help_visible {
        if matches!(mouse.kind, MouseEventKind::Down(_))
            && point_in(state.popup_area, mouse.column, mouse.row)
        {
            state.help_visible = false;
            return ReviewInput::Changed;
        }
        return ReviewInput::Consumed;
    }
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let delta: i32 = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                -3
            } else {
                3
            };
            let step = delta.signum();
            // Scroll the ask response when hovering the ask bar.
            if point_in(state.ask_area, mouse.column, mouse.row) {
                let max = state.ask.max_response_scroll(
                    state.ask_area.width.saturating_sub(2) as usize,
                    state.ask_area.height.saturating_sub(2) as usize,
                );
                state.ask.scroll_response(delta, max);
                return ReviewInput::Changed;
            }
            // Scroll the pane under the cursor.
            if point_in(state.list_area, mouse.column, mouse.row) {
                state.move_sel(step);
                return ReviewInput::Changed;
            }
            if point_in(state.preview_area, mouse.column, mouse.row) {
                state.focus = ReviewFocus::Preview;
                if let Some(v) = state.viewer.as_mut() {
                    v.handle_scroll(delta);
                }
                return ReviewInput::Changed;
            }
            // Fallback: scroll focused pane.
            match state.focus {
                ReviewFocus::List => {
                    state.move_sel(step);
                    ReviewInput::Changed
                }
                ReviewFocus::Preview => {
                    if let Some(v) = state.viewer.as_mut() {
                        v.handle_scroll(delta);
                    }
                    ReviewInput::Changed
                }
                ReviewFocus::Ask => ReviewInput::Consumed,
            }
        }
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            // Click on ask bar → focus it.
            if point_in(state.ask_area, mouse.column, mouse.row) {
                state.focus = ReviewFocus::Ask;
                return ReviewInput::Changed;
            }
            let ctrl = mouse.modifiers.contains(KeyModifiers::CONTROL);
            if let Some(idx) = list_row_at(state, mouse.column, mouse.row) {
                state.select_filtered(idx);
                if ctrl {
                    // Ctrl+click file/dir row → open leaf path in OS default app.
                    return ReviewInput::OpenPath;
                }
                // Clicking a dir header toggles its fold (tree mode);
                // clicking a file keeps the select-and-preview behavior.
                if state.tree_rows.get(idx).is_some_and(|r| r.is_dir) {
                    state.toggle_selected_dir();
                    return ReviewInput::Changed;
                }
                // Click selects and jumps to preview (edit-viewer TUI).
                state.focus = ReviewFocus::Preview;
                return ReviewInput::Changed;
            }
            // Ctrl+click preview title path (first row of preview pane).
            if ctrl
                && point_in(state.preview_area, mouse.column, mouse.row)
                && mouse.row == state.preview_area.y
            {
                return ReviewInput::OpenPath;
            }
            if point_in(state.preview_area, mouse.column, mouse.row) {
                state.focus = ReviewFocus::Preview;
                if let Some(v) = state.viewer.as_mut() {
                    v.handle_mouse(mouse.kind, mouse.column, mouse.row);
                }
                return ReviewInput::Changed;
            }
            ReviewInput::Consumed
        }
        MouseEventKind::Drag(_) | MouseEventKind::Up(_) | MouseEventKind::Moved => {
            if state.focus == ReviewFocus::Preview
                && let Some(v) = state.viewer.as_mut()
            {
                v.handle_mouse(mouse.kind, mouse.column, mouse.row);
                return ReviewInput::Changed;
            }
            ReviewInput::Consumed
        }
        _ => ReviewInput::Consumed,
    }
}

fn point_in(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

// ---------------------------------------------------------------------------
// Tree builder: cwd-relative paths + compact single-child / Java packages
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TrieNode {
    children: std::collections::BTreeMap<String, TrieNode>,
    /// File leaf attached at this segment (basename).
    file_idx: Option<usize>,
    additions: usize,
    deletions: usize,
    kind: Option<ReviewFileKind>,
    is_error: bool,
}

/// Strip `cwd` prefix when path is under the session working directory.
pub fn strip_cwd_prefix(path: &str, cwd: &str) -> String {
    let path_n = path.replace('\\', "/");
    let cwd_n = cwd.replace('\\', "/").trim_end_matches('/').to_string();
    if cwd_n.is_empty() {
        return path_n;
    }
    if let Some(rest) = path_n.strip_prefix(&cwd_n) {
        let rest = rest.trim_start_matches('/');
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    path_n
}

fn is_java_package_seg(s: &str) -> bool {
    // Common Maven/Gradle path folders must stay slash-joined, not dotted.
    const NOT_PKG: &[&str] = &[
        "src",
        "main",
        "test",
        "java",
        "kotlin",
        "scala",
        "groovy",
        "resources",
        "generated",
        "classes",
        "target",
        "build",
        "out",
        "bin",
        "lib",
        "libs",
        "webapp",
        "static",
        "public",
        "assets",
        "dist",
        "node_modules",
        "vendor",
        "pkg",
    ];
    if NOT_PKG.iter().any(|p| *p == s) {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Join compacted segment chain: consecutive java package segs with `.`, else `/`.
pub fn compact_join(parts: &[String]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let mut out = parts[0].clone();
    for i in 1..parts.len() {
        let prev = &parts[i - 1];
        let cur = &parts[i];
        if is_java_package_seg(prev) && is_java_package_seg(cur) {
            out.push('.');
            out.push_str(cur);
        } else {
            out.push('/');
            out.push_str(cur);
        }
    }
    out
}

/// Build tree rows for the filtered file set.
///
/// `collapsed` hides children of dir rows whose `dir_key` is present;
/// `force_expand` ignores it (used while a filter query is active).
pub fn build_tree_rows(
    files: &[ReviewFileItem],
    filtered: &[usize],
    cwd: &str,
    collapsed: &std::collections::BTreeSet<String>,
    force_expand: bool,
) -> Vec<ReviewTreeRow> {
    let mut root = TrieNode::default();
    for &fi in filtered {
        let Some(file) = files.get(fi) else {
            continue;
        };
        let rel = strip_cwd_prefix(&file.path, cwd);
        let segs: Vec<String> = rel
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segs.is_empty() {
            continue;
        }
        let mut node = &mut root;
        for (i, seg) in segs.iter().enumerate() {
            node = node.children.entry(seg.clone()).or_default();
            if i + 1 == segs.len() {
                node.file_idx = Some(fi);
                node.additions = file.additions;
                node.deletions = file.deletions;
                node.kind = Some(file.kind);
                node.is_error = file.is_error;
            }
        }
    }

    let mut rows = Vec::new();
    flatten_trie(&root, 0, collapsed, force_expand, &mut rows);
    rows
}

fn flatten_trie(
    node: &TrieNode,
    depth: u16,
    collapsed: &std::collections::BTreeSet<String>,
    force_expand: bool,
    rows: &mut Vec<ReviewTreeRow>,
) {
    // Root is virtual — only walk children.
    for (name, child) in &node.children {
        flatten_node(
            name,
            child,
            depth,
            String::new(),
            collapsed,
            force_expand,
            rows,
        );
    }
}

fn flatten_node(
    name: &str,
    node: &TrieNode,
    depth: u16,
    parent_key: String,
    collapsed: &std::collections::BTreeSet<String>,
    force_expand: bool,
    rows: &mut Vec<ReviewTreeRow>,
) {
    // Compact single-child pure-dir chains (Java packages, nested folders).
    let mut parts = vec![name.to_string()];
    let mut cur = node;
    loop {
        if cur.file_idx.is_some() || cur.children.len() != 1 {
            break;
        }
        let (child_name, child) = cur.children.iter().next().unwrap();
        // Stop before absorbing a pure file leaf into the chain label.
        if child.file_idx.is_some() && child.children.is_empty() {
            break;
        }
        if child.file_idx.is_none() {
            parts.push(child_name.clone());
            cur = child;
            continue;
        }
        break;
    }

    let label = compact_join(&parts);

    // Pure file leaf.
    if cur.file_idx.is_some() && cur.children.is_empty() {
        rows.push(ReviewTreeRow {
            depth,
            label,
            file_idx: cur.file_idx,
            additions: cur.additions,
            deletions: cur.deletions,
            kind: cur.kind,
            is_error: cur.is_error,
            is_dir: false,
            dir_key: None,
        });
        return;
    }

    // Directory header (aggregated stats), then children unless collapsed.
    let (add, del, err) = aggregate_stats(cur);
    let dir_key = if parent_key.is_empty() {
        label.clone()
    } else {
        format!("{parent_key}/{label}")
    };
    rows.push(ReviewTreeRow {
        depth,
        label,
        file_idx: None,
        additions: add,
        deletions: del,
        kind: None,
        is_error: err,
        is_dir: true,
        dir_key: Some(dir_key.clone()),
    });
    if !force_expand && collapsed.contains(&dir_key) {
        return;
    }

    for (child_name, child) in &cur.children {
        flatten_node(
            child_name,
            child,
            depth + 1,
            dir_key.clone(),
            collapsed,
            force_expand,
            rows,
        );
    }
}

fn aggregate_stats(node: &TrieNode) -> (usize, usize, bool) {
    let mut add = node.additions;
    let mut del = node.deletions;
    let mut err = node.is_error;
    for child in node.children.values() {
        let (a, d, e) = aggregate_stats(child);
        add += a;
        del += d;
        err |= e;
    }
    (add, del, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollback::blocks::tool::EditToolCallBlock;
    use crate::scrollback::blocks::tool::ExecuteToolCallBlock;
    use crate::scrollback::blocks::tool::ReadToolCallBlock;
    use crate::scrollback::entry::ScrollbackEntry;
    use xai_grok_pager_diff::DiffLine;

    fn edit_entry(path: &str, write: bool) -> ScrollbackEntry {
        let hunk = vec![
            DiffLine {
                text: "old\n".into(),
                lo: 1,
                ln: 1,
                tag: ChangeTag::Delete,
            },
            DiffLine {
                text: "new\n".into(),
                lo: 1,
                ln: 1,
                tag: ChangeTag::Insert,
            },
        ];
        let mut block = EditToolCallBlock::new(path, vec![hunk]);
        if write {
            block = block.with_prefix("Creating ");
        }
        ScrollbackEntry::new(RenderBlock::ToolCall(ToolCallBlock::Edit(block)))
    }

    #[test]
    fn extract_skips_read_and_bash_by_default() {
        let mut sb = ScrollbackState::new();
        sb.push(ScrollbackEntry::new(RenderBlock::user_prompt("hi")));
        sb.push(edit_entry("a.rs", false));
        sb.push(edit_entry("b.rs", true));
        sb.push(ScrollbackEntry::new(RenderBlock::ToolCall(
            ToolCallBlock::Read(ReadToolCallBlock::new("c.rs")),
        )));
        sb.push(ScrollbackEntry::new(RenderBlock::ToolCall(
            ToolCallBlock::Execute(ExecuteToolCallBlock::new("ls")),
        )));

        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "b.rs"]);
        assert!(matches!(files[0].kind, ReviewFileKind::Edit));
        assert!(matches!(files[1].kind, ReviewFileKind::Write));
        assert_eq!(files[0].additions, 1);
        assert_eq!(files[0].deletions, 1);
        assert!(sb.get_by_id(files[0].entry_id).is_some());
    }

    #[test]
    fn extract_all_includes_reads() {
        let mut sb = ScrollbackState::new();
        sb.push(edit_entry("a.rs", false));
        sb.push(ScrollbackEntry::new(RenderBlock::ToolCall(
            ToolCallBlock::Read(ReadToolCallBlock::new("c.rs").with_content("hi\n".into(), 1)),
        )));
        let files = extract_review_files(&sb, None, ReviewKindFilter::All);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "c.rs"]);
        assert!(matches!(files[1].kind, ReviewFileKind::Read));
        assert_eq!(files[1].additions, 0);
    }

    #[test]
    fn extract_all_edit_wins_over_same_path_read() {
        let mut sb = ScrollbackState::new();
        sb.push(ScrollbackEntry::new(RenderBlock::ToolCall(
            ToolCallBlock::Read(ReadToolCallBlock::new("a.rs")),
        )));
        let edit_id = sb.push(edit_entry("a.rs", false));
        let files = extract_review_files(&sb, None, ReviewKindFilter::All);
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].kind, ReviewFileKind::Edit));
        assert_eq!(files[0].entry_id, edit_id);
        assert_eq!(files[0].op_count, 2);
    }

    #[test]
    fn extract_merges_same_path_keeps_latest_entry() {
        let mut sb = ScrollbackState::new();
        let id1 = sb.push(edit_entry("a.rs", false));
        let id2 = sb.push(edit_entry("a.rs", false));
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].op_count, 2);
        assert_eq!(files[0].entry_id, id2);
        assert_ne!(files[0].entry_id, id1);
    }

    #[test]
    fn list_filter_narrows_files() {
        let mut sb = ScrollbackState::new();
        sb.push(edit_entry("src/foo.rs", false));
        sb.push(edit_entry("src/bar.ts", true));
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let mut state = ReviewState::new("t", files, ReviewKindFilter::Changes);
        state.list_query = "foo".into();
        state.apply_list_filter();
        assert_eq!(state.filtered.len(), 1);
        assert!(state.current_file().unwrap().path.contains("foo"));
    }

    #[test]
    fn review_ask_edits_utf8_only_on_character_boundaries() {
        let mut state = ReviewState::new("t", Vec::new(), ReviewKindFilter::Changes);
        state.focus = ReviewFocus::Ask;

        for character in ['你', '好', '🙂'] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
            assert!(matches!(
                handle_review_ask_key(&mut state, &key),
                ReviewInput::Changed
            ));
            assert!(state.ask.input.is_char_boundary(state.ask.cursor));
        }
        assert_eq!(state.ask.input, "你好🙂");
        assert_eq!(state.ask.cursor, state.ask.input.len());

        handle_review_ask_key(
            &mut state,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(state.ask.cursor, "你好".len());
        handle_review_ask_key(
            &mut state,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(state.ask.input, "你🙂");
        assert_eq!(state.ask.cursor, "你".len());
        handle_review_ask_key(
            &mut state,
            &KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(state.ask.input, "你");
        assert!(state.ask.input.is_char_boundary(state.ask.cursor));
    }

    #[test]
    fn review_ask_renders_below_preview_with_bottom_input_separator() {
        let scrollback = ScrollbackState::new();
        let mut state = ReviewState::new("t", Vec::new(), ReviewKindFilter::Changes);
        state.focus = ReviewFocus::Ask;
        let area = Rect::new(0, 0, 120, 30);
        let mut buffer = Buffer::empty(area);

        render_review_modal(&mut buffer, area, &mut state, &scrollback);

        assert!(state.ask_area.width > 0);
        assert_eq!(state.ask_area.x, state.preview_area.x);
        assert_eq!(state.ask_area.width, state.preview_area.width);
        assert_eq!(
            state.preview_area.y + state.preview_area.height,
            state.ask_area.y
        );
        let separator_y = state.ask_area.y + state.ask_area.height - 3;
        assert_eq!(buffer[(state.ask_area.x + 1, separator_y)].symbol(), "─");
    }

    #[test]
    fn review_ask_appends_streaming_deltas() {
        let mut ask = ReviewAskState::default();
        ask.start_loading();
        ask.append_delta("first");
        ask.append_delta(" second");
        assert!(matches!(
            ask.response,
            Some(ReviewAskResponse::Loading { .. })
        ));
        let content = match &ask.response {
            Some(ReviewAskResponse::Loading { content, .. }) => content.text(),
            _ => unreachable!(),
        };
        assert_eq!(content, "first second");
    }

    #[test]
    fn strip_cwd_and_compact_java_packages() {
        assert_eq!(
            strip_cwd_prefix("/proj/src/main/java/com/foo/Bar.java", "/proj"),
            "src/main/java/com/foo/Bar.java"
        );
        assert_eq!(
            compact_join(&["com".into(), "example".into(), "app".into(),]),
            "com.example.app"
        );
        assert_eq!(
            compact_join(&["src".into(), "main".into(), "java".into()]),
            "src/main/java"
        );
    }

    #[test]
    fn tree_compacts_java_chain_under_cwd() {
        let files = vec![
            ReviewFileItem {
                path: "/proj/src/main/java/com/example/app/Service.java".into(),
                kind: ReviewFileKind::Edit,
                entry_id: EntryId::new(1),
                additions: 1,
                deletions: 1,
                is_error: false,
                op_count: 1,
                plain_fallback: String::new(),
            },
            ReviewFileItem {
                path: "/proj/src/main/java/com/example/app/Repo.java".into(),
                kind: ReviewFileKind::Write,
                entry_id: EntryId::new(2),
                additions: 2,
                deletions: 0,
                is_error: false,
                op_count: 1,
                plain_fallback: String::new(),
            },
            ReviewFileItem {
                path: "/proj/utils/helper.rs".into(),
                kind: ReviewFileKind::Edit,
                entry_id: EntryId::new(3),
                additions: 1,
                deletions: 0,
                is_error: false,
                op_count: 1,
                plain_fallback: String::new(),
            },
        ];
        let filtered: Vec<usize> = (0..files.len()).collect();
        let rows = build_tree_rows(&files, &filtered, "/proj", &Default::default(), false);
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        // Single-child dir chain src→main→java compacted; java packages dotted.
        assert!(
            labels
                .iter()
                .any(|l| l.contains("com.example.app") || *l == "com.example.app"),
            "expected compacted java package, got {labels:?}"
        );
        assert!(labels.iter().any(|l| *l == "Service.java"));
        assert!(labels.iter().any(|l| *l == "Repo.java"));
        assert!(labels.iter().any(|l| *l == "helper.rs" || *l == "utils"));
    }

    #[test]
    fn list_row_at_uses_body_geometry_not_outer_frame() {
        let mut sb = ScrollbackState::new();
        for name in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            sb.push(edit_entry(name, false));
        }
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let mut state = ReviewState::new("t", files, ReviewKindFilter::Changes);
        // Simulate draw: outer list frame at y=5, body starts one row lower (title border).
        state.list_area = Rect::new(0, 5, 20, 10);
        state.list_body_area = Rect::new(0, 6, 19, 8);
        state.list_view_start = 1; // scrolled so first visible is index 1
        // Click first body row → filtered index 1
        assert_eq!(list_row_at(&state, 2, 6), Some(1));
        // Click third body row → index 3
        assert_eq!(list_row_at(&state, 2, 8), Some(3));
        // Click on title row (outside body) → miss
        assert_eq!(list_row_at(&state, 2, 5), None);
    }

    #[test]
    fn nerd_file_icon_maps_common_extensions() {
        assert_eq!(nerd_file_icon("foo.rs", false), "\u{e68b}");
        assert_eq!(nerd_file_icon("App.tsx", false), "\u{e628}");
        assert_eq!(nerd_file_icon("Cargo.toml", false), "\u{e68b}");
        assert_eq!(nerd_file_icon("README.md", false), "\u{f48a}");
        assert_eq!(nerd_file_icon("src", true), "\u{f07b}");
        // Unknown still returns a default file glyph (caller gates on probe).
        assert_eq!(nerd_file_icon("weird.xyz", false), "\u{f15b}");
    }

    #[test]
    fn ensure_list_visible_no_scroll_when_all_rows_fit() {
        let mut sb = ScrollbackState::new();
        for name in ["a.rs", "b.rs", "c.rs"] {
            sb.push(edit_entry(name, false));
        }
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let mut state = ReviewState::new("t", files, ReviewKindFilter::Changes);
        state.selected = 2;
        state.list_view_start = 1; // would have been force-centered
        state.ensure_list_visible(10); // viewport bigger than 3 rows
        assert_eq!(state.list_view_start, 0);
    }

    #[test]
    fn ensure_list_visible_scrolls_only_when_out_of_window() {
        let mut sb = ScrollbackState::new();
        for name in ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"] {
            sb.push(edit_entry(name, false));
        }
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let mut state = ReviewState::new("t", files, ReviewKindFilter::Changes);
        state.list_view_start = 0;
        state.selected = 1;
        state.ensure_list_visible(3);
        assert_eq!(state.list_view_start, 0, "still in window");
        state.selected = 5;
        state.ensure_list_visible(3);
        assert_eq!(state.list_view_start, 3, "scroll to keep last rows");
        state.selected = 0;
        state.ensure_list_visible(3);
        assert_eq!(state.list_view_start, 0, "scroll up to selection");
    }

    #[test]
    fn click_tree_dir_row_toggles_fold() {
        let mut state = fold_state();
        let src_idx = state
            .tree_rows
            .iter()
            .position(|r| r.is_dir && r.label == "src/main/java")
            .unwrap();
        state.list_body_area = Rect::new(0, 1, 20, 10);
        state.list_view_start = 0;
        let mouse_at = |row: u16| MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // Clicking a dir header folds it; selection stays, no preview jump.
        assert!(matches!(
            handle_review_mouse(&mut state, &mouse_at(1 + src_idx as u16)),
            ReviewInput::Changed
        ));
        assert!(!state.tree_rows.iter().any(|r| r.label == "A.java"));
        assert_eq!(state.selected, src_idx);
        assert_eq!(state.focus, ReviewFocus::List);

        // Clicking a file leaf keeps select-and-preview.
        let helper_idx = state
            .tree_rows
            .iter()
            .position(|r| r.label == "helper.rs")
            .unwrap();
        assert!(matches!(
            handle_review_mouse(&mut state, &mouse_at(1 + helper_idx as u16)),
            ReviewInput::Changed
        ));
        assert_eq!(state.focus, ReviewFocus::Preview);
        assert_eq!(state.selected, helper_idx);
    }

    #[test]
    fn ctrl_click_list_row_opens_path() {
        let mut sb = ScrollbackState::new();
        sb.push(edit_entry("a.rs", false));
        sb.push(edit_entry("b.rs", false));
        let files = extract_review_files(&sb, None, ReviewKindFilter::Changes);
        let mut state = ReviewState::new("t", files, ReviewKindFilter::Changes);
        state.list_body_area = Rect::new(0, 1, 20, 5);
        state.list_view_start = 0;
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::CONTROL,
        };
        assert!(matches!(
            handle_review_mouse(&mut state, &mouse),
            ReviewInput::OpenPath
        ));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn review_file_rows_hide_diff_stats() {
        let theme = Theme::current();
        let file = ReviewFileItem {
            path: "src/a.rs".into(),
            kind: ReviewFileKind::Edit,
            entry_id: EntryId::new(1),
            additions: 12,
            deletions: 3,
            is_error: false,
            op_count: 1,
            plain_fallback: String::new(),
        };
        let area = Rect::new(0, 0, 40, 1);
        let mut flat = Buffer::empty(area);
        render_flat_row(
            &mut flat,
            area.x,
            area.y,
            area.width,
            &file,
            false,
            theme.bg_base,
            &theme,
        );
        let flat_text: String = (0..area.width).map(|x| flat[(x, 0)].symbol()).collect();
        assert!(flat_text.contains("a.rs"));
        assert!(!flat_text.contains("+12"));
        assert!(!flat_text.contains("-3"));

        let row = ReviewTreeRow {
            depth: 1,
            label: "a.rs".into(),
            file_idx: Some(0),
            additions: 12,
            deletions: 3,
            kind: Some(ReviewFileKind::Edit),
            is_error: false,
            is_dir: false,
            dir_key: None,
        };
        let mut tree = Buffer::empty(area);
        render_tree_row(
            &mut tree,
            area.x,
            area.y,
            area.width,
            &row,
            false,
            theme.bg_base,
            &theme,
            false,
        );
        let tree_text: String = (0..area.width).map(|x| tree[(x, 0)].symbol()).collect();
        assert!(tree_text.contains("a.rs"));
        assert!(!tree_text.contains("+12"));
        assert!(!tree_text.contains("-3"));
    }

    // ---- tree fold / search ------------------------------------------------

    fn fold_state() -> ReviewState {
        let files = vec![
            file_item("/proj/src/main/java/A.java", 1),
            file_item("/proj/src/main/java/B.java", 2),
            file_item("/proj/utils/helper.rs", 3),
        ];
        let mut state =
            ReviewState::with_options("t", files, ReviewKindFilter::Changes, true, "/proj");
        state.rebuild_tree();
        state
    }

    fn file_item(path: &str, id: u64) -> ReviewFileItem {
        ReviewFileItem {
            path: path.into(),
            kind: ReviewFileKind::Edit,
            entry_id: EntryId::new(id),
            additions: 1,
            deletions: 0,
            is_error: false,
            op_count: 1,
            plain_fallback: String::new(),
        }
    }

    #[test]
    fn tree_collapse_hides_children_and_expand_restores() {
        let mut state = fold_state();
        let src_idx = state
            .tree_rows
            .iter()
            .position(|r| r.is_dir && r.label == "src/main/java")
            .expect("compacted dir row");
        state.selected = src_idx;

        assert!(state.toggle_selected_dir());
        let labels: Vec<&str> = state.tree_rows.iter().map(|r| r.label.as_str()).collect();
        assert!(!labels.contains(&"A.java"), "children hidden: {labels:?}");
        assert!(labels.contains(&"utils"), "unrelated dirs remain");
        // Selection stays on the collapsed dir row.
        assert_eq!(state.selected, src_idx);

        assert!(state.expand_selected_dir());
        let labels: Vec<&str> = state.tree_rows.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"A.java"), "children restored: {labels:?}");
        assert_eq!(state.selected, src_idx);
    }

    #[test]
    fn tree_filter_force_expands_collapsed_dirs() {
        let mut state = fold_state();
        let src_idx = state
            .tree_rows
            .iter()
            .position(|r| r.is_dir && r.label == "src/main/java")
            .unwrap();
        state.selected = src_idx;
        state.toggle_selected_dir();
        assert!(state.tree_rows.iter().all(|r| r.label != "A.java"));

        // Searching re-expands everything so matches stay visible.
        state.list_query = "A.java".into();
        state.apply_list_filter();
        let labels: Vec<&str> = state.tree_rows.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.contains(&"A.java"),
            "match visible under fold: {labels:?}"
        );
        assert!(state.current_file().unwrap().path.ends_with("A.java"));
    }

    #[test]
    fn collapse_or_goto_parent_walks_up_to_ancestor() {
        let mut state = fold_state();
        let leaf_idx = state
            .tree_rows
            .iter()
            .position(|r| r.file_idx.is_some() && r.label == "A.java")
            .unwrap();
        state.selected = leaf_idx;
        // h on a leaf jumps to its ancestor dir header.
        assert!(state.collapse_dir_or_goto_parent());
        assert!(state.tree_rows[state.selected].is_dir);
        // h again collapses that dir in place.
        assert!(state.collapse_dir_or_goto_parent());
        assert!(!state.tree_rows.iter().any(|r| r.label == "A.java"));
    }

    // ---- help overlay ------------------------------------------------------

    #[test]
    fn help_overlay_opens_and_any_key_closes() {
        let mut state = ReviewState::new("t", Vec::new(), ReviewKindFilter::Changes);
        let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        let key_help = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert!(matches!(
            handle_review_list_key(&mut state, &key_help),
            ReviewInput::Changed
        ));
        assert!(state.help_visible);
        // Any key (even q) closes the overlay instead of dismissing the modal.
        assert!(matches!(
            handle_review_list_key(&mut state, &key_q),
            ReviewInput::Changed
        ));
        assert!(!state.help_visible);
    }

    #[test]
    fn help_overlay_renders_over_modal() {
        let scrollback = ScrollbackState::new();
        let mut state = ReviewState::new("t", Vec::new(), ReviewKindFilter::Changes);
        state.help_visible = true;
        let area = Rect::new(0, 0, 100, 30);
        let mut buffer = Buffer::empty(area);
        render_review_modal(&mut buffer, area, &mut state, &scrollback);
        // Border title proves the overlay is on top (scan every row).
        let found = (0..area.height).any(|y| {
            let text: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
            text.contains("Review keybindings")
        });
        assert!(found, "help overlay title not rendered");
    }

    // ---- preview hunk navigation ------------------------------------------

    #[test]
    fn jump_hunk_walks_hunks_and_stops_at_ends() {
        let hunk = |old: &str, new: &str| {
            vec![
                DiffLine {
                    text: format!("{old}\n"),
                    lo: 1,
                    ln: 1,
                    tag: ChangeTag::Delete,
                },
                DiffLine {
                    text: format!("{new}\n"),
                    lo: 1,
                    ln: 2,
                    tag: ChangeTag::Insert,
                },
            ]
        };
        let block = EditToolCallBlock::new("a.rs", vec![hunk("o1", "n1"), hunk("o2", "n2")]);
        let entry = ScrollbackEntry::new(RenderBlock::ToolCall(ToolCallBlock::Edit(block)));
        let mut viewer = BlockViewerPane::for_edit_review(EntryId::new(9), &entry).unwrap();

        // No cursor → ] lands on the first hunk start.
        assert!(viewer.jump_hunk(1));
        let first = viewer.list_state.selected_id();
        assert!(first.is_some());

        // Forward to the second hunk.
        assert!(viewer.jump_hunk(1));
        let second = viewer.list_state.selected_id();
        assert_ne!(first, second);

        // No hunk past the end / before the start.
        assert!(!viewer.jump_hunk(1));
        assert_eq!(viewer.list_state.selected_id(), second);
        assert!(viewer.jump_hunk(-1));
        assert_eq!(viewer.list_state.selected_id(), first);
        assert!(!viewer.jump_hunk(-1));
        assert_eq!(viewer.list_state.selected_id(), first);
    }
}
