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

fn prompt_preview(text: &str) -> String {
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

    /// Whether a turn contains a persisted compaction-summary block.
    ///
    /// Timeline navigation remains turn-based; this is display metadata only.
    pub fn turn_has_compaction_summary(&self, turn_idx: usize) -> bool {
        let Some(turn) = self.turns.get(turn_idx) else {
            return false;
        };
        turn.range().any(|entry_idx| {
            self.entries.get_index(entry_idx).is_some_and(|(_, entry)| {
                matches!(
                    &entry.block,
                    RenderBlock::SessionEvent(block)
                        if matches!(&block.event, crate::scrollback::blocks::SessionEvent::CompactionSummary { .. })
                )
            })
        })
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
    fn compaction_summary_marks_only_its_own_turn() {
        let mut state = ScrollbackState::new();
        state.push_block(RenderBlock::user_prompt("first prompt"));
        state.push_block(RenderBlock::agent_message("first answer"));
        state.push_block(RenderBlock::session_event(
            crate::scrollback::blocks::SessionEvent::CompactionSummary {
                summary: "preserved context".into(),
            },
        ));
        state.push_block(RenderBlock::user_prompt("second prompt"));
        state.push_block(RenderBlock::agent_message("second answer"));

        assert!(state.turn_has_compaction_summary(0));
        assert!(!state.turn_has_compaction_summary(1));
    }
}
