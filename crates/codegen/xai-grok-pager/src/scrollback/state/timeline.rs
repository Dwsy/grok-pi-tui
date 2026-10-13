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
    use super::super::test_util::*;
    use super::*;
    use pretty_assertions::assert_eq;

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

    #[test]
    fn timeline_entries_one_per_turn_in_order() {
        let mut state = ScrollbackState::new();
        state.push_block(stub_block("session banner")); // 0: pre-turn
        state.push_block(user_block("first question")); // 1
        state.push_block(agent_block("first answer")); // 2
        state.push_block(user_block("second question")); // 3
        state.push_block(tool_block("ls")); // 4
        state.prepare_layout(80, 10);

        let entries = state.timeline_entries();
        assert_eq!(entries.len(), 2);
        let [first, second] = entries.as_slice() else {
            panic!("expected two timeline entries: {entries:?}");
        };
        assert_eq!(first.turn_idx, 0);
        assert_eq!(state.index_of_id(first.prompt_entry_id), Some(1));
        assert_eq!(first.preview, "first question");
        assert_eq!(second.turn_idx, 1);
        assert_eq!(state.index_of_id(second.prompt_entry_id), Some(3));
        assert_eq!(second.preview, "second question");
    }

    #[test]
    fn preview_takes_first_nonempty_line_and_caps_length() {
        let mut state = ScrollbackState::new();
        state.push_block(user_block("\n\n  leading blanks skipped  \nsecond line"));
        let long = "x".repeat(500);
        state.push_block(user_block(&long));
        state.prepare_layout(80, 10);

        let entries = state.timeline_entries();
        let [first, second, ..] = entries.as_slice() else {
            panic!("expected two timeline entries: {entries:?}");
        };
        assert_eq!(first.preview, "leading blanks skipped");
        assert_eq!(second.preview.chars().count(), 120);
        assert!(second.preview.ends_with('\u{2026}'));
    }

    #[test]
    fn active_turn_tracks_viewport_top() {
        let mut state = ScrollbackState::new();
        state.push_block(user_block("Q1")); // 0
        state.push_block(tall_agent_block()); // 1
        state.push_block(user_block("Q2")); // 2
        state.push_block(tall_agent_block()); // 3
        state.push_block(user_block("Q3")); // 4
        state.push_block(tall_agent_block()); // 5
        state.prepare_layout(80, 6);

        state.goto_top();
        assert_eq!(state.active_turn_for_viewport(), Some(0));

        state.scroll_to_entry_top(2);
        assert_eq!(state.active_turn_for_viewport(), Some(1));

        state.goto_bottom();
        assert_eq!(state.active_turn_for_viewport(), Some(2));
    }

    #[test]
    fn active_turn_stays_top_anchored_at_the_bottom() {
        // A screenful of short trailing turns: even at the bottom the active turn is the one owning the top row (the web-timeline rule)
        // This replaced a clamp to the newest turn, whose highlight leapt at one step off the bottom and whose ▲ chevron stuck
        let mut state = clustered_trailing_turns();
        state.goto_bottom();
        let at_bottom = state.active_turn_for_viewport().expect("active at bottom");
        assert!(at_bottom < 6, "top-anchored, not the newest: {at_bottom}");

        // Nudging off the bottom moves the highlight at most one boundary (the old clamp leapt from the newest turn to the top-anchored one)
        state.scroll_up(1);
        let nudged = state.active_turn_for_viewport().expect("still in a turn");
        assert!(
            at_bottom - nudged <= 1,
            "no highlight leap: {at_bottom} -> {nudged}"
        );
    }

    /// One render frame and chevron click, wired exactly like the app.
    /// render.rs builds the rail from viewport state; mouse.rs resolves the hit through `chevron_target` and jumps.
    /// `None` means the chevron was dim.
    fn click_chevron(state: &mut ScrollbackState, viewport_height: u16, up: bool) -> Option<usize> {
        use crate::views::timeline::{RailViewport, TimelineHit, chevron_target, compute_rail};
        state.prepare_layout(80, viewport_height);
        let area = ratatui::layout::Rect::new(0, 0, 80, viewport_height);
        let vp = RailViewport {
            active: state.active_turn_for_viewport(),
            up_target: state.turn_above_viewport_top(),
            down_target: state.turn_below_viewport_top(),
            at_bottom: !state.has_content_below(),
        };
        let rail = compute_rail(area, 78, state.turn_count(), vp).expect("rail eligible");
        let hit = if up {
            TimelineHit::Up
        } else {
            TimelineHit::Down
        };
        let target = chevron_target(&rail, hit)?;
        state.jump_to_turn(target);
        Some(target)
    }

    /// One tall response, then six short trailing turns that cluster in the last 80x12 screenful.
    fn clustered_trailing_turns() -> ScrollbackState {
        let mut state = ScrollbackState::new();
        state.push_block(user_block("Q1"));
        state.push_block(tall_agent_block());
        for i in 2..8 {
            state.push_block(user_block(&format!("Q{i}")));
            state.push_block(agent_block("ok"));
        }
        state.prepare_layout(80, 12);
        state
    }

    #[test]
    fn chevrons_walk_the_conversation_end_to_end_without_sticking() {
        // The stuck-▲ shape: one tall response, then six short turns that all cluster inside the final screenful
        let mut state = clustered_trailing_turns();
        state.goto_bottom();

        // ▲ to the very top: every click moves the viewport up, one boundary per click once on a prompt row, no sticking
        let mut up_visits = Vec::new();
        while up_visits.len() < 16 {
            let before = state.scroll_offset();
            let Some(target) = click_chevron(&mut state, 12, true) else {
                break;
            };
            assert!(
                state.scroll_offset() < before,
                "▲ #{} must move the viewport up",
                up_visits.len()
            );
            up_visits.push(target);
        }
        assert_eq!(state.scroll_offset(), 0, "▲ walk reaches the top");
        assert_eq!(up_visits.last(), Some(&0), "▲ walk ends at the first turn");
        assert!(
            up_visits
                .windows(2)
                .all(|w| matches!(w, [a, b] if *a - *b == 1)),
            "one boundary per click: {up_visits:?}"
        );
        assert_eq!(click_chevron(&mut state, 12, true), None, "▲ dim at top");

        // ▼ back down: strictly forward, never sticking, and it terminates (dims) rather than repeating a turn or running forever
        let mut down_visits = Vec::new();
        while down_visits.len() < 16 {
            let Some(target) = click_chevron(&mut state, 12, false) else {
                break;
            };
            if let Some(&prev) = down_visits.last() {
                assert!(
                    target > prev,
                    "▼ moves strictly forward: {down_visits:?} then {target}"
                );
            }
            down_visits.push(target);
        }
        assert!(!down_visits.is_empty(), "▼ steps down from the top");
        assert!(
            down_visits.len() < 16,
            "▼ walk terminates (dims), no sticking: {down_visits:?}"
        );
    }

    #[test]
    fn down_chevron_enters_trailing_turns_at_the_bottom() {
        // Reported bug: short turns cluster in the final screenful and ▼ sat dim at the bottom, even though clicking their ticks jumped to them
        // ▼ now targets the next turn, the same turn a tick click resolves to (both go through jump_to_turn)
        let mut state = clustered_trailing_turns();
        state.goto_bottom();

        let active = state.active_turn_for_viewport().expect("active at bottom");
        assert!(active < 7, "trailing turns sit below the top-anchored turn");
        assert_eq!(
            state.turn_below_viewport_top(),
            Some(active + 1),
            "▼ has a target below the top-anchored turn"
        );
        assert_eq!(
            click_chevron(&mut state, 12, false),
            Some(active + 1),
            "▼ steps to the next turn instead of dimming"
        );
    }

    #[test]
    fn up_chevron_snaps_to_the_current_prompt_mid_turn() {
        // Midway through a response ▲ first aligns the current turn's own prompt to the top (matching the h key), then steps to older turns
        let mut state = ScrollbackState::new();
        state.push_block(user_block("Q1"));
        state.push_block(tall_agent_block());
        state.push_block(user_block("Q2"));
        state.push_block(tall_agent_block());
        state.prepare_layout(80, 6);
        state.goto_top();
        state.scroll_down(3);

        assert_eq!(state.active_turn_for_viewport(), Some(0));
        assert_eq!(
            click_chevron(&mut state, 6, true),
            Some(0),
            "snap to own prompt"
        );
        assert_eq!(state.scroll_offset(), 0);
        assert_eq!(
            click_chevron(&mut state, 6, true),
            None,
            "then dim at the top"
        );
    }

    #[test]
    fn chevrons_when_everything_fits_on_one_screen() {
        // Fits with room to spare: the first turn owns the top
        // ▲ dims (nothing above), but ▼ still enters the next turn, anchoring it to the top like clicking its tick, rather than dimming
        let mut state = ScrollbackState::new();
        state.push_block(user_block("Q1"));
        state.push_block(agent_block("a1"));
        state.push_block(user_block("Q2"));
        state.push_block(agent_block("a2"));
        state.prepare_layout(80, 40);

        assert_eq!(state.active_turn_for_viewport(), Some(0));
        assert_eq!(
            click_chevron(&mut state, 40, true),
            None,
            "▲ dim at first turn"
        );
        assert_eq!(
            click_chevron(&mut state, 40, false),
            Some(1),
            "▼ enters the second turn"
        );
    }

    #[test]
    fn pre_turn_content_dims_up_and_down_enters_the_first_turn() {
        let mut state = ScrollbackState::new();
        for i in 0..10 {
            state.push_block(stub_block(&format!("banner {i}")));
        }
        state.push_block(user_block("Q1"));
        state.push_block(tall_agent_block());
        state.push_block(user_block("Q2"));
        state.push_block(agent_block("ok"));
        state.prepare_layout(80, 12);
        state.goto_top();

        // Pre-turn content focuses the first tick; ▲ is dim while ▼ enters that first turn rather than skipping it
        assert_eq!(state.active_turn_for_viewport(), Some(0));
        assert_eq!(click_chevron(&mut state, 12, true), None);
        assert_eq!(click_chevron(&mut state, 12, false), Some(0));
        assert_eq!(state.active_turn_for_viewport(), Some(0));
        assert_eq!(
            click_chevron(&mut state, 12, true),
            None,
            "▲ dim on the first turn (nothing above)"
        );
    }

    #[test]
    fn active_turn_is_first_before_first_prompt() {
        let mut state = ScrollbackState::new();
        for i in 0..10 {
            state.push_block(stub_block(&format!("banner {i}")));
        }
        state.push_block(user_block("Q1"));
        state.push_block(tall_agent_block());
        state.prepare_layout(80, 4);

        state.goto_top();
        assert_eq!(state.active_turn_for_viewport(), Some(0));

        state.goto_bottom();
        assert_eq!(state.active_turn_for_viewport(), Some(0));
    }
}
