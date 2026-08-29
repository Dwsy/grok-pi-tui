use super::*;
use crate::btw_bridge::BtwHistoryEntry;

impl PiAgent {
    /// Publish Pi-owned session metadata title. This is distinct from an
    /// extension's temporary `ctx.ui.setTitle()`: Pager needs the durable
    /// session value to restore the native terminal title when Pi reloads and
    /// clears extension UI state.
    pub(super) async fn send_session_title(&self, title: Option<&str>) {
        let title = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Pi");
        self.send_ext_notification(
            "pi/ui/title",
            json!({ "title": title, "source": "session" }),
        )
        .await;
    }

    /// Publish an extension-owned temporary terminal title.
    pub(super) async fn send_extension_title(&self, title: &str) {
        self.send_ext_notification(
            "pi/ui/title",
            json!({ "title": title, "source": "extension" }),
        )
        .await;
    }

    pub(super) async fn send_commands(&self, commands: &[PiCommand]) {
        self.send_update(acp::SessionUpdate::AvailableCommandsUpdate(
            acp::AvailableCommandsUpdate::new(command_catalog(commands, self.workflows_enabled)),
        ))
        .await;
    }

    pub(super) async fn send_models(&self, bootstrap: &PiBootstrap) {
        let Some(models) = bootstrap.acp_models() else {
            return;
        };
        match serde_json::to_value(models) {
            Ok(value) => {
                self.send_ext_notification("x.ai/models/update", value)
                    .await;
            }
            Err(error) => tracing::warn!(%error, "failed to serialize Pi model state"),
        }
    }

    pub(super) async fn publish_bootstrap(&self, bootstrap: &PiBootstrap) {
        self.send_commands(&bootstrap.commands).await;
        self.send_models(bootstrap).await;
        self.send_session_title(bootstrap.state.session_name.as_deref())
            .await;
    }

    pub(super) async fn replay_history(&self) -> Result<()> {
        // `get_messages` exposes Pi's current LLM context, which is truncated at
        // compaction. Keep the append-log cache hot so resume renders the full
        // active branch while branch switches transfer only entries added since
        // the previous snapshot. A second post-navigation poll is intentionally
        // retained: it is an empty delta in the common case and prevents a stale
        // cache if Pager delays or retries session/load.
        let refreshed = if let Err(error) = self.refresh_entry_replay_cache().await {
            tracing::warn!(%error, "Pi get_entries unavailable; falling back to compacted messages");
            false
        } else {
            true
        };
        let (history, btw_history) = if refreshed {
            let state = self.state.borrow();
            (
                state.entry_replay_cache.replay_entries(),
                state.entry_replay_cache.btw_history_entries(),
            )
        } else {
            let data = self.rpc.request(json!({ "type": "get_messages" })).await?;
            (parse_messages(&data), Vec::new())
        };
        for entry in history {
            self.replay_history_item(entry).await;
        }
        if refreshed {
            self.send_btw_history(btw_history, "replay").await;
        }
        Ok(())
    }

    pub(super) async fn send_btw_history(&self, entries: Vec<BtwHistoryEntry>, source: &str) {
        let session_id = self.state.borrow().acp_session_id.clone();
        let entries = entries
            .into_iter()
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "question": entry.question,
                    "answer": entry.answer,
                    "createdAt": entry.created_at_ms,
                    "modelUsed": entry.model_used,
                })
            })
            .collect::<Vec<_>>();
        self.send_ext_notification(
            "pi/ui/btw_history",
            json!({
                "version": 1,
                "sessionId": session_id,
                "source": source,
                "entries": entries,
            }),
        )
        .await;
    }

    pub(super) async fn send_current_btw_history(&self, source: &str) {
        let entries = self.state.borrow().entry_replay_cache.btw_history_entries();
        self.send_btw_history(entries, source).await;
    }

    pub(super) async fn send_compaction_summary(
        &self,
        summary: &str,
        is_replay: bool,
        timestamp_ms: Option<i64>,
    ) {
        let session_id = self.session_id();
        self.send_ext_notification(
            "pi/ui/compaction_summary",
            json!({
                "sessionId": session_id.0.as_ref(),
                "summary": summary,
                "isReplay": is_replay,
                "agentTimestampMs": timestamp_ms,
            }),
        )
        .await;
    }

    pub(super) async fn replay_history_item(&self, entry: PiReplayEntry) {
        let timestamp_ms = entry.timestamp_ms;
        let update = match entry.item {
            PiHistoryItem::UserText(text) => acp::SessionUpdate::UserMessageChunk(text_chunk(text)),
            PiHistoryItem::UserImage { data, mime_type } => {
                acp::SessionUpdate::UserMessageChunk(content_chunk(acp::ContentBlock::Image(
                    acp::ImageContent::new(data, mime_type),
                )))
            }
            PiHistoryItem::AgentText(text) => {
                acp::SessionUpdate::AgentMessageChunk(text_chunk(text))
            }
            PiHistoryItem::CompactionSummary(summary) => {
                self.send_compaction_summary(&summary, true, timestamp_ms)
                    .await;
                return;
            }
            PiHistoryItem::AgentThought(text) => {
                acp::SessionUpdate::AgentThoughtChunk(text_chunk(text))
            }
            PiHistoryItem::ToolStart {
                id,
                name,
                arguments,
                usage,
            } => {
                let arguments = normalize_tool_raw_input(&name, arguments);
                if let Some(args) = arguments.clone() {
                    self.state.borrow_mut().tool_args.insert(id.clone(), args);
                }
                let mut tool_call = acp::ToolCall::new(acp::ToolCallId::new(id), name.clone())
                    .kind(tool_kind(&name))
                    .status(acp::ToolCallStatus::InProgress)
                    .content(
                        edit_diff_content(&name, arguments.as_ref(), None).unwrap_or_default(),
                    )
                    .locations(Vec::new())
                    .raw_input(arguments);
                if let Some(usage) = usage {
                    let mut meta = acp::Meta::new();
                    meta.insert("piToolUsage".into(), usage);
                    tool_call = tool_call.meta(Some(meta));
                }
                acp::SessionUpdate::ToolCall(tool_call)
            }
            PiHistoryItem::ToolEnd {
                id,
                name,
                content,
                raw_output,
                is_error,
            } => {
                let mut raw = raw_output.unwrap_or(Value::Null);
                // History often stores `details` as raw_output and the body in
                // separate content blocks. Fold text into the payload so bash/read
                // projection still sees stdout / file text.
                if pi_result_text(&raw).is_empty() {
                    let text = content
                        .iter()
                        .filter_map(|item| match item {
                            PiToolContent::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        raw = json!({ "content": [{ "type": "text", "text": text }] });
                    }
                }
                let args = self.state.borrow_mut().tool_args.remove(&id);
                let normalized = normalize_tool_raw_output(&name, args.as_ref(), &raw, is_error);
                let mut fields = acp::ToolCallUpdateFields::new()
                    .title(Some(name.clone()))
                    .status(Some(if is_error {
                        acp::ToolCallStatus::Failed
                    } else {
                        acp::ToolCallStatus::Completed
                    }))
                    .raw_output(Some(normalized));
                if tool_kind(&name) == acp::ToolKind::Edit {
                    fields = fields.content(edit_diff_content(&name, args.as_ref(), Some(&raw)));
                } else {
                    fields = fields.content(Some(history_tool_content(content)));
                }
                // Project todo-plugin snapshots onto the native TodoPane before
                // the tool card update so resume restores badge state.
                if let Some(plan) = plan_update_for_tool(&name, &raw, is_error) {
                    self.send_update(acp::SessionUpdate::Plan(plan)).await;
                }
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(id),
                    fields,
                ))
            }
        };
        self.send_replay_update(update, timestamp_ms).await;
    }

    /// Send a session update during history replay, stamping the original
    /// message timestamp (`agentTimestampMs`) so the pager can display the real
    /// creation time instead of the resume wall-clock time.
    pub(super) async fn send_replay_update(
        &self,
        update: acp::SessionUpdate,
        timestamp_ms: Option<i64>,
    ) {
        let mut notification = acp::SessionNotification::new(self.session_id(), update);
        let mut meta = acp::Meta::new();
        meta.insert("isReplay".into(), Value::Bool(true));
        if let Some(ms) = timestamp_ms {
            meta.insert("agentTimestampMs".into(), json!(ms));
        }
        if let Some(tokens) = self.state.borrow().last_context_tokens {
            meta.insert("totalTokens".into(), json!(tokens));
        }
        notification = notification.meta(Some(meta));
        if let Err(error) = acp_send(notification, &self.client_tx).await {
            tracing::debug!(%error, "Grok pager closed while sending Pi replay update");
        }
    }
}
