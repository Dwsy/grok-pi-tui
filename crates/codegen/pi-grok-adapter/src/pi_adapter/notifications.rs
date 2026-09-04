use super::*;

impl PiAgent {
    pub(super) async fn send_update(&self, update: acp::SessionUpdate) {
        let mut notification = acp::SessionNotification::new(self.session_id(), update);
        // Stamp the same live timing fields stock Grok shell puts on every
        // SessionNotification. Without streamStartMs the pager never pre-creates
        // Thinking… and the first-token empty window has no breathing accent.
        let mut meta = acp::Meta::new();
        {
            let state = self.state.borrow();
            if let Some(tokens) = state.last_context_tokens {
                meta.insert("totalTokens".into(), json!(tokens));
            }
            if let Some(ms) = state.turn_start_ms {
                meta.insert("turnStartMs".into(), json!(ms));
            }
            if let Some(ms) = state.stream_start_ms {
                meta.insert("streamStartMs".into(), json!(ms));
            }
            // Same pin stock Grok shell puts on every live notification.
            // Without promptId the pager can still render when current_prompt_id
            // is set locally, but turn-status / gate adoption stay incomplete.
            if let Some(pid) = state.live_prompt_id.as_ref() {
                meta.insert("promptId".into(), json!(pid));
            }
        }
        meta.insert("agentTimestampMs".into(), json!(utc_now_ms()));
        notification = notification.meta(Some(meta));
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, "Grok pager closed while sending Pi session update");
        }
    }

    pub(super) async fn send_update_for_session(
        &self,
        session_id: &str,
        update: acp::SessionUpdate,
        replay: bool,
        event_id: &str,
    ) {
        let mut notification =
            acp::SessionNotification::new(acp::SessionId::new(session_id.to_string()), update);
        let mut meta = acp::Meta::new();
        if replay {
            meta.insert("isReplay".into(), Value::Bool(true));
        }
        meta.insert("eventId".into(), Value::String(event_id.to_string()));
        notification = notification.meta(Some(meta));
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, session_id, "Grok pager closed while sending Pi child session update");
        }
    }

    pub(super) fn accept_subagent_bridge_sequence(
        &self,
        subagent_id: &str,
        sequence: u64,
        replay: bool,
    ) -> bool {
        accept_subagent_sequence(
            &mut self.state.borrow_mut().subagent_bridge_sequences,
            subagent_id,
            sequence,
            replay,
        )
    }

    pub(super) async fn handle_recap_bridge_message(&self, event: &Value) -> Result<bool> {
        let Some(projection) = parse_recap_message(event) else {
            return Ok(false);
        };
        let session_id = self.session_id().0.to_string();
        let notification = session_recap_notification(&session_id, &projection);
        self.send_ext_notification("x.ai/session/update", notification)
            .await;
        Ok(true)
    }

    pub(super) async fn handle_btw_bridge_message(&self, event: &Value) -> bool {
        let Some(projection) = parse_btw_message(event) else {
            return false;
        };
        match projection {
            crate::btw_bridge::BtwProjection::Delta { request_id, delta } => {
                self.send_ext_notification(
                    "x.ai/review_ask_delta",
                    json!({ "requestId": request_id, "delta": delta }),
                )
                .await;
            }
            crate::btw_bridge::BtwProjection::Complete {
                request_id, result, ..
            } => {
                let sender = self.state.borrow_mut().pending_btw.remove(&request_id);
                if let Some(tx) = sender {
                    let _ = tx.send(result);
                } else {
                    tracing::debug!(
                        request_id = %request_id,
                        "btw bridge message with no pending waiter"
                    );
                }
            }
        }
        true
    }

    pub(super) async fn handle_subagent_bridge_message(&self, event: &Value) -> Result<bool> {
        let root_session_id = self.session_id().0.to_string();
        let Some(projection) = parse_bridge_message(event, &root_session_id)? else {
            return Ok(false);
        };
        let parent_session_id = projection.parent_session_id.clone();
        let parent_known = parent_session_id == root_session_id
            || self
                .state
                .borrow()
                .subagent_session_to_id
                .contains_key(&parent_session_id);
        if !parent_known {
            bail!("subagent bridge parentSessionId is not a known Pager session");
        }
        if !self.accept_subagent_bridge_sequence(
            &projection.subagent_id,
            projection.sequence,
            projection.replay,
        ) {
            return Ok(true);
        }
        if projection.kind == "spawned" {
            self.state.borrow_mut().subagent_session_to_id.insert(
                projection.child_session_id.clone(),
                projection.subagent_id.clone(),
            );
        }
        let replay = projection.replay;
        let event_id = format!(
            "pi-grok-subagent:{}:{}",
            projection.subagent_id, projection.sequence
        );
        for operation in projection.operations {
            match operation {
                BridgeOperation::ParentTaskMetadata {
                    tool_call_id,
                    raw_input,
                } => {
                    let reconcile = raw_input
                        .get("reconcile")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let mut fields = acp::ToolCallUpdateFields::new().raw_input(Some(raw_input));
                    if !replay && !reconcile {
                        fields = fields.status(Some(acp::ToolCallStatus::InProgress));
                    }
                    let update = acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        acp::ToolCallId::new(tool_call_id),
                        fields,
                    ));
                    if parent_session_id == root_session_id {
                        self.send_update(update).await;
                    } else {
                        self.send_update_for_session(
                            &parent_session_id,
                            update,
                            replay,
                            &event_id,
                        )
                        .await;
                    }
                }
                BridgeOperation::ParentLifecycle(notification) => {
                    let method = if replay {
                        "x.ai/session/update"
                    } else {
                        "x.ai/session_notification"
                    };
                    self.send_ext_notification(method, notification).await;
                }
                BridgeOperation::ChildUpdate {
                    child_session_id,
                    update,
                } => {
                    self.send_update_for_session(&child_session_id, update, replay, &event_id)
                        .await;
                }
                BridgeOperation::ReplayComplete { request_id } => {
                    if let Some(tx) = self
                        .state
                        .borrow_mut()
                        .pending_subagent_replays
                        .remove(&request_id)
                    {
                        let _ = tx.send(());
                    }
                }
            }
        }
        Ok(true)
    }

    /// Pull Pi's current context-window estimate and push it to the pager bar.
    ///
    /// Grok's context bar needs `_meta.totalTokens` on session updates plus the
    /// model window (`totalContextTokens`). Pi owns the estimate via
    /// `get_session_stats.contextUsage`.
    pub(super) async fn refresh_context_usage(&self) {
        let data = match self
            .rpc
            .request(json!({ "type": "get_session_stats" }))
            .await
        {
            Ok(data) => data,
            Err(error) => {
                tracing::debug!(%error, "failed to fetch Pi session stats for context bar");
                return;
            }
        };
        let Some(tokens) = context_tokens_from_stats(&data) else {
            return;
        };
        let changed = {
            let mut state = self.state.borrow_mut();
            if state.last_context_tokens == Some(tokens) {
                false
            } else {
                state.last_context_tokens = Some(tokens);
                true
            }
        };
        if changed {
            // Empty chunk is a no-op in the tracker but still carries
            // `_meta.totalTokens` for confirm_context_used.
            self.send_update(acp::SessionUpdate::AgentMessageChunk(text_chunk("")))
                .await;
        }
    }

    pub(super) fn note_context_tokens(&self, tokens: u64) {
        if tokens == 0 {
            return;
        }
        self.state.borrow_mut().last_context_tokens = Some(tokens);
    }

    pub(super) async fn send_ext_notification(&self, method: &str, params: Value) {
        let Ok(raw) = serde_json::value::to_raw_value(&params) else {
            return;
        };
        let notification = acp::ExtNotification::new(method, raw.into());
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, method, "Grok pager closed while sending Pi UI notification");
        }
    }

    pub(super) async fn send_ui_notification(&self, message: &str, kind: Option<&str>) {
        self.send_ext_notification(
            "pi/ui/notify",
            json!({ "message": message, "notifyType": kind }),
        )
        .await;
    }

    pub(super) async fn handle_compaction_start(&self, event: &Value) {
        self.refresh_context_usage().await;
        let notification = (|| {
            let mut state = self.state.borrow_mut();
            state.compaction_started_at = Some(Instant::now());
            let tokens_used = state.last_context_tokens?;
            let context_window = state
                .bootstrap
                .state
                .model
                .as_ref()
                .and_then(|model| model.context_window)
                .filter(|window| *window > 0)?;
            Some(compaction_start_notification(
                &state.acp_session_id,
                event,
                tokens_used,
                context_window,
            ))
        })();
        if let Some(notification) = notification {
            self.send_ext_notification("x.ai/session/update", notification)
                .await;
        }
        self.send_status("compaction", Some("Compacting context…"))
            .await;
    }

    pub(super) async fn handle_compaction_end(&self, event: &Value) {
        let (session_id, elapsed_ms) = {
            let mut state = self.state.borrow_mut();
            let elapsed_ms = state
                .compaction_started_at
                .take()
                .and_then(|started| started.elapsed().as_millis().try_into().ok());
            (state.acp_session_id.clone(), elapsed_ms)
        };
        if let Some(notification) = compaction_end_notification(&session_id, event, elapsed_ms) {
            self.send_ext_notification("x.ai/session/update", notification)
                .await;
        }
        if let Some(summary) = event
            .get("result")
            .and_then(|result| string(result, &["summary"]))
            .filter(|summary| !summary.trim().is_empty())
        {
            self.send_compaction_summary(summary, false, None).await;
        }
        self.send_status("compaction", None).await;
        if let Some(error) = string(event, &["errorMessage", "error"])
            && !error.is_empty()
        {
            self.send_ui_notification(error, Some("error")).await;
        }
    }

    /// Build Grok-native `x.ai/session/info` from Pi session stats.
    ///
    /// Mirrors the intent of the pi-context extension (`getContextUsage` +
    /// system/tool estimates) but returns the ACP envelope that the pager's
    /// `ContextInfoBlock` already knows how to render — no second UI.
    pub(super) async fn handle_session_info(&self) -> Result<acp::ExtResponse, acp::Error> {
        let stats = self
            .rpc
            .request(json!({ "type": "get_session_stats" }))
            .await
            .map_err(acp_internal)?;
        // Best-effort breakdown. Prefer live messages; fall back to branch entries
        // (session file shape) so empty/failed get_messages still yields a bar.
        let messages = match self.rpc.request(json!({ "type": "get_messages" })).await {
            Ok(value)
                if value
                    .get("messages")
                    .and_then(Value::as_array)
                    .is_some_and(|m| !m.is_empty())
                    || value.as_array().is_some_and(|m| !m.is_empty()) =>
            {
                Some(value)
            }
            _ => self
                .rpc
                .request(json!({ "type": "get_entries" }))
                .await
                .ok()
                .map(entries_to_messages_value),
        };
        let breakdown = self.fetch_context_breakdown().await;
        // Best-effort raw entries for cache graph (pi-cache-graph alignment).
        let entries_for_cache = self
            .rpc
            .request(json!({ "type": "get_entries" }))
            .await
            .ok();
        let (session_id, model, cached_tokens, session_file, session_name) = {
            let state = self.state.borrow();
            (
                state.acp_session_id.clone(),
                state.bootstrap.state.model.clone(),
                state.last_context_tokens,
                state.bootstrap.state.session_file.clone(),
                state.bootstrap.state.session_name.clone(),
            )
        };
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut response = build_session_info_response(
            &stats,
            messages.as_ref(),
            &session_id,
            &cwd,
            model.as_ref(),
            cached_tokens,
            breakdown.as_ref(),
            session_file.as_deref(),
            session_name.as_deref(),
        );
        if let Some(entries) = entries_for_cache.as_ref() {
            let metrics = crate::cache_metrics::collect_cache_session_metrics(entries);
            response = crate::context_projection::attach_cache_metrics(response, metrics);
        }
        if let Some(used) = response
            .get("context")
            .and_then(|context| context.get("used"))
            .and_then(Value::as_u64)
            .filter(|&tokens| tokens > 0)
        {
            self.note_context_tokens(used);
        }
        ext_response(response).map_err(acp_internal)
    }

    /// Best-effort system/tool/agents breakdown via the injected bridge extension.
    ///
    /// Failure is non-fatal: projection falls back to system/tools = 0.
    pub(super) async fn fetch_context_breakdown(
        &self,
    ) -> Option<crate::context_projection::ContextBreakdownRaw> {
        let path = self.context_breakdown.as_ref()?;
        if let Err(error) = self
            .run_immediate_bridge_command(CONTEXT_BREAKDOWN_COMMAND, "")
            .await
        {
            tracing::debug!(?error, "context breakdown bridge failed");
            return None;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => return None,
            Err(error) => {
                tracing::debug!(?error, path = %path.display(), "context breakdown file missing");
                return None;
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => Some(parse_context_breakdown(&value)),
            Err(error) => {
                tracing::debug!(?error, "context breakdown JSON invalid");
                None
            }
        }
    }

    pub(super) async fn send_status(&self, key: &str, text: Option<&str>) {
        self.send_ext_notification(
            "pi/ui/status",
            json!({ "statusKey": key, "statusText": text }),
        )
        .await;
    }
}
