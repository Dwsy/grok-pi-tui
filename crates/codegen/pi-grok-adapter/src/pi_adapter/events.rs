use super::*;

const CANCEL_IDLE_CONFIRMATIONS: u8 = 3;

fn cancellation_idle_confirmed(idle_polls: &mut u8, is_streaming: bool) -> bool {
    if is_streaming {
        *idle_polls = 0;
        return false;
    }
    *idle_polls = (*idle_polls).saturating_add(1);
    *idle_polls >= CANCEL_IDLE_CONFIRMATIONS
}

fn cancellation_probe_still_current(cancelling: bool, has_running_prompt: bool) -> bool {
    cancelling && !has_running_prompt
}

impl PiAgent {
    pub(super) async fn handle_event(&self, event: Value) -> Result<()> {
        let event_type = event
            .get("type")
            .or_else(|| event.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let cancelling = self.state.borrow().cancelling;
        if cancelling && matches!(event_type, "agent_start" | "turn_start") {
            // Pi RPC exposes abort but not clearQueue(). A residual steering
            // message can therefore open another turn after the first abort;
            // keep cancelling each continuation until Pi emits agent_settled.
            if let Err(error) = self.rpc.notify(json!({ "type": "abort" })) {
                tracing::warn!(%error, "failed to re-abort Pi continuation during cancellation");
            }
            return Ok(());
        }
        let suppress_cancelled_stream = cancelling
            && matches!(
                event_type,
                "turn_end"
                    | "message_start"
                    | "message_update"
                    | "message_end"
                    | "tool_execution_start"
                    | "tool_execution_update"
                    | "tool_execution_end"
                    | "agent_end"
                    | "queue_update"
            );
        if suppress_cancelled_stream {
            return Ok(());
        }
        match event_type {
            "agent_start" => {
                let now = utc_now_ms();
                let claimed_parked = {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = true;
                    for active in &mut state.active_prompts {
                        active.agent_started = true;
                    }
                    if !state.cancelling {
                        state.turn_start_ms = Some(now);
                        state.stream_start_ms = Some(now);
                        if state.live_prompt_id.is_none() {
                            state.live_prompt_id = state
                                .active_prompts
                                .iter()
                                .rev()
                                .find_map(|p| p.client_prompt_id.clone())
                                .or_else(|| {
                                    state.queue_mirror.running().map(|entry| entry.id.clone())
                                });
                        }
                    }
                    // Server-chained turn (e.g. a Pi-queued goal reminder that
                    // starts right after the `/goal` command turn): claim the
                    // free running slot so the turn is broadcast to the pager
                    // and receives its paired prompt_complete at settle.
                    if !state.cancelling
                        && state.active_prompts.is_empty()
                        && state.queue_mirror.running().is_none()
                    {
                        state.queue_mirror.promote_parked_running()
                    } else {
                        false
                    }
                };
                if claimed_parked {
                    self.publish_queue_snapshot().await;
                }
            }
            "agent_settled" => {
                self.refresh_context_usage().await;
                let (mode_update, running) = {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = false;
                    state.cancelling = false;
                    state.turn_start_ms = None;
                    state.stream_start_ms = None;
                    state.live_prompt_id = None;
                    state.bash_stream_output.clear();
                    let running = state.queue_mirror.clear_running();
                    let mode_update = if matches!(
                        state.plan_mode.state(),
                        crate::plan_mode::PiPlanState::ExitPending
                    ) {
                        state.plan_mode.complete_deferred_exit();
                        Some(acp::SessionModeId::new("default"))
                    } else {
                        None
                    };
                    (mode_update, running)
                };
                if let Some(mode_id) = mode_update {
                    self.persist_plan_mode_state()?;
                    self.sync_plan_mode_control()?;
                    self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                        acp::CurrentModeUpdate::new(mode_id),
                    ))
                    .await;
                }
                self.rebroadcast_queue_mirror().await;
                self.finish_prompts(acp::StopReason::EndTurn);
                if let Some(entry) = running
                    && entry.origin != QueueOrigin::Client
                {
                    self.send_server_prompt_complete(&entry, acp::StopReason::EndTurn)
                        .await;
                }
                let dispatched = self.dispatch_next_queued().await;
                if !dispatched {
                    self.maybe_continue_goal().await;
                }
            }
            // `agent_end` is not the Pi idle barrier. Retry, compaction and
            // extension handlers can continue after it; `agent_settled` owns
            // ACP prompt completion.
            "agent_end" => {}
            "turn_start" => {
                // Multi-turn agent loops: each turn is a new stream segment.
                let now = utc_now_ms();
                let mut state = self.state.borrow_mut();
                if state.turn_start_ms.is_none() {
                    state.turn_start_ms = Some(now);
                }
                state.stream_start_ms = Some(now);
            }
            "turn_end" => self.refresh_context_usage().await,
            "message_start" => self.handle_message_start(&event),
            "message_update" => self.handle_message_update(&event).await,
            "message_end" => {
                if self.handle_btw_bridge_message(&event).await
                    || self.handle_recap_bridge_message(&event).await?
                    || self.handle_background_bash_bridge_message(&event).await?
                    || self.handle_workflow_bridge_message(&event).await?
                    || self.handle_goal_bridge_message(&event).await?
                    || self.handle_loop_bridge_message(&event).await?
                {
                    // Bridge custom messages are display/control traffic.
                } else if !self.handle_subagent_bridge_message(&event).await? {
                    self.handle_message_end(&event).await;
                }
            }
            // Live subagent bridge traffic is persisted with appendEntry so it
            // cannot enter Pi's steering/follow-up queues while the parent is
            // streaming. RPC exposes that append as entry_appended.
            "entry_appended" => {
                if self.handle_eval_tool_bridge_entry(&event).await {
                    // Eval-v2-only nested tools are display-only native ACP rows.
                } else if self.handle_btw_bridge_message(&event).await {
                    // /btw deltas and answers are appended custom entries.
                } else if self.handle_recap_bridge_message(&event).await? {
                    // Recap summaries are appended custom entries.
                } else if !self.handle_workflow_bridge_message(&event).await?
                    && !self.handle_goal_bridge_message(&event).await?
                    && !self.handle_loop_bridge_message(&event).await?
                {
                    self.handle_subagent_bridge_message(&event).await?;
                }
            }
            "tool_execution_start" => self.handle_tool_start(&event).await,
            "tool_execution_update" => self.handle_tool_update(&event).await,
            "tool_execution_end" => self.handle_tool_end(&event).await,
            "extension_ui_request" => self.handle_extension_ui(event).await?,
            "extension_error" => {
                let message = event
                    .get("error")
                    .map(json_text)
                    .filter(|text| !text.is_empty())
                    .or_else(|| string(&event, &["message"]).map(ToOwned::to_owned))
                    .unwrap_or_else(|| "Pi extension error".to_string());
                self.send_ui_notification(&message, Some("error")).await;
            }
            "compaction_start" | "auto_compaction_start" => {
                self.handle_compaction_start(&event).await;
            }
            "compaction_end" | "auto_compaction_end" => {
                self.handle_compaction_end(&event).await;
            }
            "auto_retry_start" => {
                let attempt = event.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let maximum = event
                    .get("maxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let delay_ms = event.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
                let error =
                    string(&event, &["errorMessage", "message", "reason"]).unwrap_or_default();
                let mut text = if maximum > 0 {
                    format!("Retrying {attempt}/{maximum}")
                } else {
                    "Retrying".to_string()
                };
                if delay_ms > 0 {
                    text.push_str(&format!(" in {:.1}s", delay_ms as f64 / 1000.0));
                }
                if !error.is_empty() {
                    text.push_str(": ");
                    text.push_str(error);
                }
                self.send_status("retry", Some(&text)).await;
            }
            "auto_retry_end" => {
                self.send_status("retry", None).await;
                if event.get("success").and_then(Value::as_bool) == Some(false)
                    && let Some(error) = string(&event, &["finalError", "errorMessage"])
                {
                    self.send_ui_notification(error, Some("error")).await;
                }
            }
            "queue_update" => {
                // Pi emits full text arrays; mirror them into the native queue
                // pane so optimistic server rows confirm and dequeue.
                self.apply_pi_queue_update(&event).await;
            }
            "thinking_level_changed" | "session_info_changed" => match self.refresh().await {
                Ok(bootstrap) => self.publish_bootstrap(&bootstrap).await,
                Err(error) => {
                    tracing::warn!(%error, "failed to refresh Pi state after state change");
                }
            },
            "adapter_diagnostic" => {
                if let Some(message) = string(&event, &["message"]) {
                    self.send_ui_notification(message, Some("warning")).await;
                }
            }
            "adapter_process_exit" => {
                let message = string(&event, &["message"]).unwrap_or("Pi RPC process exited");
                let intentional = event
                    .get("intentional")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let (queued, running) = {
                    let mut state = self.state.borrow_mut();
                    state.agent_running = false;
                    state.cancelling = false;
                    // An interrupted session transition cannot complete against
                    // a dead child; clear it so post-recovery reattach can
                    // begin a fresh one.
                    state.subagent_bridge_sequences.clear();
                    state.subagent_session_to_id.clear();
                    state.pending_subagent_replays.clear();
                    let queued = state.queue_mirror.clear_local();
                    let running = state.queue_mirror.clear_running();
                    state.queue_mirror.clear();
                    (queued, running)
                };
                self.finish_queued_entries(queued, acp::StopReason::Cancelled);
                self.finish_prompts(acp::StopReason::Cancelled);
                if let Some(entry) = running
                    && entry.origin != QueueOrigin::Client
                {
                    self.send_server_prompt_complete(&entry, acp::StopReason::Cancelled)
                        .await;
                }
                self.publish_queue_snapshot().await;
                // Background shells are children of the Pi process, so they died
                // with it. This holds for a deliberate teardown too — only the
                // toast and recovery round below are crash-specific.
                self.settle_orphaned_background_bash().await;
                // Deliberate teardown (respawn, probes) is not a crash: no
                // toast, no recovery round.
                if !intentional {
                    self.send_ui_notification(message, Some("error")).await;
                    // Recovery waits for a request-scoped subagent socket
                    // marker. Running it inline would deadlock this sole event
                    // consumer, which must stay free to receive that marker.
                    let recovery = self.clone();
                    tokio::task::spawn_local(async move {
                        recovery.recover_rpc_connection().await;
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_message_start(&self, event: &Value) {
        if message_role(event) == Some("assistant") {
            let now = utc_now_ms();
            let mut state = self.state.borrow_mut();
            state.live_assistant = Some(StreamSeen::default());
            // New assistant message = new LLM stream segment (same semantics
            // as Grok shell StreamStarted → record_stream_start).
            if state.turn_start_ms.is_none() {
                state.turn_start_ms = Some(now);
            }
            state.stream_start_ms = Some(now);
        }
    }

    pub(super) async fn handle_message_update(&self, event: &Value) {
        let (text, thought) = extract_delta(event);
        {
            let mut state = self.state.borrow_mut();
            let seen = state.live_assistant.get_or_insert_with(StreamSeen::default);
            seen.text |= !text.is_empty();
            seen.thought |= !thought.is_empty();
        }
        if !thought.is_empty() {
            self.send_update(acp::SessionUpdate::AgentThoughtChunk(text_chunk(thought)))
                .await;
        }
        if !text.is_empty() {
            self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk(text)))
                .await;
        }
    }

    pub(super) async fn handle_message_end(&self, event: &Value) {
        if message_role(event) != Some("assistant") {
            return;
        }
        let seen = self
            .state
            .borrow_mut()
            .live_assistant
            .take()
            .unwrap_or_default();
        let Some(message) = event.get("message") else {
            return;
        };
        // Prefer the assistant message's own usage for a low-latency bar update;
        // agent_settled still revalidates via get_session_stats.
        if let Some(tokens) = message.get("usage").and_then(context_tokens_from_usage) {
            self.note_context_tokens(tokens);
        }
        let terminal_error = string(message, &["errorMessage", "error_message"])
            .filter(|error| !error.is_empty())
            .map(ToOwned::to_owned);
        for entry in parse_messages(&json!({ "messages": [message] })) {
            match entry.item {
                PiHistoryItem::ToolStart {
                    id,
                    usage: Some(usage),
                    ..
                } => {
                    self.state.borrow_mut().tool_usage.insert(id, usage);
                }
                PiHistoryItem::AgentThought(text) if !seen.thought => {
                    self.send_update(acp::SessionUpdate::AgentThoughtChunk(text_chunk(text)))
                        .await;
                }
                PiHistoryItem::AgentText(text) if !seen.text => {
                    self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk(text)))
                        .await;
                }
                _ => {}
            }
        }
        if seen.text
            && let Some(error) = terminal_error
        {
            self.send_ui_notification(&error, Some("error")).await;
        }
        self.flush_pending_steering().await;
    }

    pub(super) fn finish_prompts(&self, requested_reason: acp::StopReason) {
        let active_prompts = {
            let mut state = self.state.borrow_mut();
            // Drop stream anchors so idle notifications do not re-trigger
            // Pager Thinking pre-create with a stale streamStartMs.
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.live_prompt_id = None;
            state.bash_stream_output.clear();
            state.tool_usage.clear();
            std::mem::take(&mut state.active_prompts)
        };
        for active in active_prompts {
            let reason = if active.cancelled {
                acp::StopReason::Cancelled
            } else {
                requested_reason.clone()
            };
            let _ = active.completion.send(PromptCompletion {
                reason,
                client_prompt_id: active.client_prompt_id,
            });
        }
    }

    pub(super) fn remove_prompt(&self, id: u64) {
        let mut state = self.state.borrow_mut();
        if let Some(index) = state
            .active_prompts
            .iter()
            .position(|active| active.id == id)
        {
            state.active_prompts.remove(index);
        }
    }

    pub(super) fn allocate_operation_id(&self) -> u64 {
        let mut state = self.state.borrow_mut();
        let id = state.next_prompt_id;
        state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
        id
    }

    /// Poll until Pi is truly idle after a fire-and-forget abort. ACP prompt
    /// completion already happened synchronously in `cancel()`; this only
    /// re-opens the adapter scheduler once the underlying Pi run has stopped.
    pub(super) async fn settle_cancelled_prompts(&self) {
        const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
        const SETTLE_POLL_DEADLINE: Duration = Duration::from_secs(30);
        let deadline = Instant::now() + SETTLE_POLL_DEADLINE;
        let mut idle_polls = 0;
        loop {
            if !self.state.borrow().cancelling {
                return;
            }
            let Ok(value) = self.rpc.request(json!({ "type": "get_state" })).await else {
                return;
            };
            let is_streaming = parse_state(&value).is_streaming;
            if cancellation_idle_confirmed(&mut idle_polls, is_streaming) {
                {
                    let mut state = self.state.borrow_mut();
                    if !cancellation_probe_still_current(
                        state.cancelling,
                        state.queue_mirror.running().is_some(),
                    ) {
                        return;
                    }
                    state.agent_running = false;
                    state.cancelling = false;
                    state.live_prompt_id = None;
                    state.queue_mirror.clear_running();
                }
                self.publish_queue_snapshot().await;
                self.dispatch_next_queued().await;
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!("Pi still streaming after asynchronous cancel deadline");
                self.state.borrow_mut().cancelling = false;
                return;
            }
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        }
    }

    pub(super) async fn probe_prompt_without_agent(&self) {
        tokio::time::sleep(Duration::from_millis(40)).await;
        let should_probe = {
            let state = self.state.borrow();
            state
                .active_prompts
                .iter()
                .any(|active| !active.agent_started)
                || (state.queue_mirror.running().is_some() && state.agent_running)
        };
        if !should_probe {
            return;
        }
        let Ok(value) = self.rpc.request(json!({ "type": "get_state" })).await else {
            return;
        };
        if parse_state(&value).is_streaming {
            return;
        }
        let running = {
            let mut state = self.state.borrow_mut();
            state.agent_running = false;
            state.cancelling = false;
            state.live_prompt_id = None;
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.queue_mirror.clear_running()
        };
        self.publish_queue_snapshot().await;
        self.finish_prompts(acp::StopReason::EndTurn);
        if let Some(entry) = running
            && entry.origin != QueueOrigin::Client
        {
            self.send_server_prompt_complete(&entry, acp::StopReason::EndTurn)
                .await;
        }
        let dispatched = self.dispatch_next_queued().await;
        if !dispatched {
            self.maybe_continue_goal().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_probe_requires_stable_idle() {
        let mut idle_polls = 0;
        assert!(!cancellation_idle_confirmed(&mut idle_polls, false));
        assert!(!cancellation_idle_confirmed(&mut idle_polls, false));
        assert!(cancellation_idle_confirmed(&mut idle_polls, false));

        assert!(!cancellation_idle_confirmed(&mut idle_polls, true));
        assert_eq!(idle_polls, 0);
        assert!(!cancellation_idle_confirmed(&mut idle_polls, false));
    }

    #[test]
    fn stale_cancellation_probe_cannot_clear_successor_goal_turn() {
        assert!(cancellation_probe_still_current(true, false));
        assert!(!cancellation_probe_still_current(false, false));
        assert!(!cancellation_probe_still_current(true, true));
    }
}
