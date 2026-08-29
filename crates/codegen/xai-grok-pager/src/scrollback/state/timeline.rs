//! Viewport-derived turn navigation for the timeline sidebar.

use super::*;

const PREVIEW_MAX_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEntry {
    pub turn_idx: usize,
    pub prompt_entry_id: EntryId,
    /// Prompt creation time for Jump/timeline pickers.
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
    pub preview: String,
}

/// Semantic kind of one marker in the narrow timeline rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineMarkerKind {
    Prompt,
    Compaction,
}

/// One visual marker in the timeline rail, ordered exactly like scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineMarker {
    pub entry_id: EntryId,
    pub entry_index: usize,
    pub turn_idx: Option<usize>,
    pub kind: TimelineMarkerKind,
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
    pub preview: String,
}

fn preview_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut preview: String = line.chars().take(PREVIEW_MAX_CHARS).collect();
    if preview.chars().count() == PREVIEW_MAX_CHARS && line.chars().nth(PREVIEW_MAX_CHARS).is_some()
    {
        preview.pop();
        preview.push('…');
    }
    preview
}

fn prompt_preview(text: &str) -> String {
    preview_line(text)
}

fn compaction_preview(summary: &str) -> String {
    let summary = preview_line(summary);
    if summary.is_empty() {
        "Compaction summary".to_string()
    } else {
        format!("Compaction summary — {summary}")
    }
}

impl ScrollbackState {
    pub fn timeline_entries(&self) -> Vec<TimelineEntry> {
        self.turns
            .iter()
            .enumerate()
            .filter_map(|(turn_idx, turn)| {
                let (prompt_entry_id, entry) = self.entries.get_index(turn.prompt_index)?;
                let RenderBlock::UserPrompt(prompt) = &entry.block else {
                    return None;
                };
                Some(TimelineEntry {
                    turn_idx,
                    prompt_entry_id: *prompt_entry_id,
                    created_at: entry.created_at,
                    preview: prompt_preview(&prompt.text),
                })
            })
            .collect()
    }

    /// Visual markers for the sidebar timeline, in scrollback order.
    ///
    /// `/jump` and review remain turn-based through [`Self::timeline_entries`];
    /// the narrow rail additionally surfaces persisted compaction summaries as
    /// first-class markers so context boundaries are visible between prompts.
    pub fn timeline_markers(&self) -> Vec<TimelineMarker> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(entry_index, (entry_id, entry))| {
                let turn_idx = self.turn_containing(entry_index);
                match &entry.block {
                    RenderBlock::UserPrompt(prompt) => Some(TimelineMarker {
                        entry_id: *entry_id,
                        entry_index,
                        turn_idx,
                        kind: TimelineMarkerKind::Prompt,
                        created_at: entry.created_at,
                        preview: prompt_preview(&prompt.text),
                    }),
                    RenderBlock::SessionEvent(block) => match &block.event {
                        crate::scrollback::blocks::SessionEvent::CompactionSummary { summary } => {
                            Some(TimelineMarker {
                                entry_id: *entry_id,
                                entry_index,
                                turn_idx,
                                kind: TimelineMarkerKind::Compaction,
                                created_at: entry.created_at,
                                preview: compaction_preview(summary),
                            })
                        }
                        _ => None,
                    },
                    _ => None,
                }
            })
            .collect()
    }

    /// Resolve active/previous/next marker indices from the current viewport.
    ///
    /// Marker indices are stable for the frame that built `markers` and are the
    /// indices stored in [`crate::views::timeline::TimelineHit::Tick`].
    pub fn timeline_marker_viewport(
        &self,
        markers: &[TimelineMarker],
    ) -> (Option<usize>, Option<usize>, Option<usize>) {
        if markers.is_empty() {
            return (None, None, None);
        }
        if self.view_mode == ViewMode::SingleTurn {
            let active = self.current_turn.and_then(|turn_idx| {
                markers.iter().position(|marker| {
                    marker.kind == TimelineMarkerKind::Prompt && marker.turn_idx == Some(turn_idx)
                })
            });
            let up = active.and_then(|index| index.checked_sub(1));
            let down = active
                .and_then(|index| index.checked_add(1))
                .filter(|index| *index < markers.len());
            return (active, up, down);
        }

        let Some(cache) = self.layout_cache.as_ref() else {
            return (None, None, None);
        };
        let range = self.visible_entry_range();
        let Some(base) = cache.virtual_y.get(range.start).copied() else {
            return (None, None, None);
        };
        let top = base + self.scroll_offset;
        let marker_y = |marker: &TimelineMarker| cache.virtual_y.get(marker.entry_index).copied();
        let active = markers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, marker)| marker_y(marker).is_some_and(|y| y <= top))
            .map(|(index, _)| index);
        let up = markers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, marker)| marker_y(marker).is_some_and(|y| y < top))
            .map(|(index, _)| index);
        let down = markers
            .iter()
            .enumerate()
            .find(|(_, marker)| marker_y(marker).is_some_and(|y| y > top))
            .map(|(index, _)| index);
        (active, up, down)
    }

    /// Jump to a sidebar marker, including a compaction block inside a turn.
    pub fn jump_to_timeline_marker(&mut self, marker_idx: usize) -> bool {
        let Some(marker) = self.timeline_markers().into_iter().nth(marker_idx) else {
            return false;
        };
        let Some(entry_idx) = self.index_of_id(marker.entry_id) else {
            return false;
        };
        self.set_selected(Some(entry_idx));
        if self.view_mode == ViewMode::AllTurns {
            self.scroll_to_entry_top(entry_idx);
        } else {
            self.scroll_offset = 0;
            self.follow_mode = false;
            self.ensure_selected_visible(NavDirection::default());
        }
        self.bump_generation();
        true
    }

    /// Preview text for one turn, used by the timeline rail hover card.
    pub fn turn_preview(&self, turn_idx: usize) -> Option<String> {
        let turn = self.turns.get(turn_idx)?;
        let entry = self.entries.get_index(turn.prompt_index)?.1;
        let RenderBlock::UserPrompt(prompt) = &entry.block else {
            return None;
        };
        let line = prompt
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        let mut preview: String = line.chars().take(PREVIEW_MAX_CHARS).collect();
        if line.chars().nth(PREVIEW_MAX_CHARS).is_some() {
            preview.pop();
            preview.push('…');
        }
        Some(preview)
    }

    /// Prompt creation time for one turn, used by the timeline rail hover card.
    pub fn turn_created_at(&self, turn_idx: usize) -> Option<chrono::DateTime<chrono::Local>> {
        let turn = self.turns.get(turn_idx)?;
        self.entries.get_index(turn.prompt_index)?.1.created_at
    }

    /// The turn owning the viewport top, if any.
    pub fn active_turn_for_viewport(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn;
        }
        if self.turns.is_empty() {
            return None;
        }
        Some(self.prompts_above_top(false)?.saturating_sub(1))
    }

    /// Jump to a turn's prompt and anchor it at the viewport top.
    pub fn jump_to_turn(&mut self, turn_idx: usize) -> bool {
        if turn_idx >= self.turns.len() {
            return false;
        }
        self.activate_turn(turn_idx);
        true
    }

    /// The nearest turn strictly above the viewport top.
    pub fn turn_above_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn?.checked_sub(1);
        }
        self.prompts_above_top(true)?.checked_sub(1)
    }

    /// The nearest turn below the viewport top.
    pub fn turn_below_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            let next = self.current_turn?.checked_add(1)?;
            return (next < self.turns.len()).then_some(next);
        }
        let next = self.prompts_above_top(false)?;
        (next < self.turns.len()).then_some(next)
    }

    fn prompts_above_top(&self, strict: bool) -> Option<usize> {
        let cache = self.layout_cache.as_ref()?;
        let range = self.visible_entry_range();
        let base = *cache.virtual_y.get(range.start)?;
        let top = base + self.scroll_offset;
        Some(self.turns.partition_point(|turn| {
            cache
                .virtual_y
                .get(turn.prompt_index)
                .is_some_and(|&prompt_y| {
                    if strict {
                        prompt_y < top
                    } else {
                        prompt_y <= top
                    }
                })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_markers_include_compaction_in_scrollback_order() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("first answer"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "preserved context and latest tool results".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.push_block(RenderBlock::agent_message("second answer"));

        let markers = state.timeline_markers();
        assert_eq!(markers.len(), 3);
        assert_eq!(markers[0].kind, TimelineMarkerKind::Prompt);
        assert_eq!(markers[0].entry_index, 0);
        assert_eq!(markers[1].kind, TimelineMarkerKind::Compaction);
        assert_eq!(markers[1].entry_index, 2);
        assert_eq!(markers[1].turn_idx, Some(0));
        assert!(markers[1].preview.starts_with("Compaction summary — "));
        assert!(markers[1].preview.contains("preserved context"));
        assert_eq!(markers[2].kind, TimelineMarkerKind::Prompt);
        assert_eq!(markers[2].entry_index, 3);
    }

    #[test]
    fn timeline_marker_jump_targets_compaction_block() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("first answer"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "preserved context".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.prepare_layout(80, 10);

        assert!(state.jump_to_timeline_marker(1));
        assert_eq!(state.selected(), Some(2));
        assert_eq!(state.current_turn(), Some(0));
    }
}
