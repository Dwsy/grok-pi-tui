use super::*;

impl PiAgent {
    /// Publish Pi's authoritative queue as Grok's native shared-queue surface.
    pub(super) async fn publish_queue_snapshot(&self) {
        let (session_id, snapshot) = {
            let state = self.state.borrow();
            (state.acp_session_id.clone(), state.queue_mirror.snapshot())
        };
        let steering_text =
            (snapshot.steering_count > 0).then(|| format!("{} steering", snapshot.steering_count));
        let follow_up_text = (snapshot.follow_up_count > 0)
            .then(|| format!("{} follow-up", snapshot.follow_up_count));
        self.send_status("steering", steering_text.as_deref()).await;
        self.send_status("follow-up", follow_up_text.as_deref())
            .await;
        self.send_ext_notification(
            "x.ai/queue/changed",
            queue_changed_params(&session_id, &snapshot),
        )
        .await;
    }

    pub(super) async fn apply_pi_queue_update(&self, event: &Value) {
        let steering = string_list(event.get("steering"));
        let follow_up = string_list(event.get("followUp"));
        {
            let mut state = self.state.borrow_mut();
            state.queue_mirror.apply_queue_update(&steering, &follow_up);
        }
        self.publish_queue_snapshot().await;
    }

    pub(super) async fn rebroadcast_queue_mirror(&self) {
        self.publish_queue_snapshot().await;
    }

    pub(super) fn adapter_busy(state: &AdapterState) -> bool {
        state.agent_running
            || state.cancelling
            || state.bash_running
            || !state.active_prompts.is_empty()
            || state.queue_mirror.running().is_some()
    }

    pub(super) fn prepare_execution_text(state: &mut AdapterState, mut message: String) -> String {
        if let Some(reminder) = state.plan_mode.build_reminder_for_prompt() {
            if message.is_empty() {
                message = reminder;
            } else {
                message = format!("{reminder}\n\n{message}");
            }
        }
        message
    }

    pub(super) fn finish_queued_entries(&self, entries: Vec<QueueEntry>, reason: acp::StopReason) {
        let completions = {
            let mut state = self.state.borrow_mut();
            entries
                .into_iter()
                .filter_map(|entry| {
                    state
                        .queued_prompt_completions
                        .remove(&entry.id)
                        .map(|completion| (entry.id, completion))
                })
                .collect::<Vec<_>>()
        };
        for (id, completion) in completions {
            let _ = completion.send(PromptCompletion {
                reason: reason.clone(),
                client_prompt_id: Some(id),
            });
        }
    }

    pub(super) async fn send_server_prompt_complete(
        &self,
        entry: &QueueEntry,
        reason: acp::StopReason,
    ) {
        self.send_ext_notification(
            "x.ai/session/prompt_complete",
            json!({
                "sessionId": self.session_id().0,
                "promptId": entry.id,
                "stopReason": stop_reason_wire(&reason),
                "agentResult": Value::Null,
            }),
        )
        .await;
    }

    pub(super) fn activate_queued_client(state: &mut AdapterState, entry_id: &str) -> Option<u64> {
        let completion = state.queued_prompt_completions.remove(entry_id)?;
        let id = state.next_prompt_id;
        state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
        state.active_prompts.push(ActivePrompt {
            id,
            client_prompt_id: Some(entry_id.to_string()),
            completion,
            agent_started: false,
            cancelled: false,
        });
        Some(id)
    }

    pub(super) fn take_active_prompt(&self, id: u64) -> Option<ActivePrompt> {
        let mut state = self.state.borrow_mut();
        let index = state
            .active_prompts
            .iter()
            .position(|active| active.id == id)?;
        Some(state.active_prompts.remove(index))
    }

    pub(super) async fn dispatch_next_queued(&self) -> bool {
        loop {
            let Some((entry, active_id)) = ({
                let mut state = self.state.borrow_mut();
                if Self::adapter_busy(&state) {
                    None
                } else {
                    state.queue_mirror.pop_next_local().map(|mut entry| {
                        entry.execution_text =
                            Self::prepare_execution_text(&mut state, entry.execution_text);
                        let active_id = Self::activate_queued_client(&mut state, &entry.id);
                        state.agent_running = true;
                        state.live_prompt_id = Some(entry.id.clone());
                        state.queue_mirror.set_running(entry.clone());
                        (entry, active_id)
                    })
                }
            }) else {
                return false;
            };

            if let Err(error) = self.persist_plan_mode_state() {
                tracing::debug!(%error, "failed to persist plan mode before queued dispatch");
            }
            if let Err(error) = self.sync_plan_mode_control() {
                tracing::debug!(%error, "failed to sync plan mode before queued dispatch");
            }
            self.publish_queue_snapshot().await;

            let mut request = json!({
                "type": "prompt",
                "message": entry.execution_text,
            });
            if !entry.images.is_empty() {
                request["images"] = Value::Array(entry.images.clone());
            }
            match self.rpc.request(request).await {
                Ok(_) => {
                    // Extension-owned rows have no ACP completion waiter, but
                    // they still need the same idle probe. Another input
                    // handler may consume the promoted RPC prompt without
                    // producing agent_start/agent_settled; without this probe
                    // the native queue would remain pinned to a ghost run.
                    let probe = self.clone();
                    tokio::task::spawn_local(async move {
                        probe.probe_prompt_without_agent().await;
                    });
                    return true;
                }
                Err(error) => {
                    tracing::warn!(%error, prompt_id = %entry.id, "queued Pi prompt failed");
                    let active = active_id.and_then(|id| self.take_active_prompt(id));
                    {
                        let mut state = self.state.borrow_mut();
                        state.agent_running = false;
                        state.live_prompt_id = None;
                        state.queue_mirror.clear_running();
                    }
                    if let Some(active) = active {
                        let _ = active.completion.send(PromptCompletion {
                            reason: acp::StopReason::Cancelled,
                            client_prompt_id: active.client_prompt_id,
                        });
                    } else {
                        self.send_server_prompt_complete(&entry, acp::StopReason::Cancelled)
                            .await;
                    }
                    self.send_ui_notification(
                        &format!("Queued Pi message failed: {error}"),
                        Some("error"),
                    )
                    .await;
                    self.publish_queue_snapshot().await;
                }
            }
        }
    }

    pub(super) async fn enqueue_extension_message(
        &self,
        text: String,
        images: Vec<Value>,
        streaming_behavior: Option<&str>,
    ) {
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let lane = if streaming_behavior == Some("steer") {
            QueueLane::Steering
        } else {
            QueueLane::FollowUp
        };
        {
            let mut state = self.state.borrow_mut();
            // Cancel is a hard barrier for extension continuations. In
            // particular, loop.ts may call sendUserMessage() from its terminal
            // handler while Pi is aborting; accepting that follow-up here
            // would restart the loop as soon as cancellation settles.
            if state.cancelling {
                tracing::debug!("dropping extension queue message during cancellation");
                return;
            }
            state.queue_mirror.enqueue_local(
                None,
                text.clone(),
                text,
                images,
                lane,
                QueueOrigin::Extension,
            )
        };
        self.publish_queue_snapshot().await;
        // Steering rows are not interjected here: they wait for the assistant
        // message_end safe-point flush so they stay cancellable until then.
        self.dispatch_next_queued().await;
    }

    pub(super) async fn interject_local_queue(
        &self,
        id: &str,
        expected_version: Option<u64>,
        new_text: Option<String>,
    ) -> bool {
        let mut dispatch_idle = false;
        let steer = {
            let mut state = self.state.borrow_mut();
            let Some(mut entry) = state.queue_mirror.take_local(id, expected_version) else {
                return false;
            };
            if let Some(text) = new_text {
                entry.execution_text = text.clone();
                entry.display_text = text;
                entry.version = entry.version.wrapping_add(1);
            }
            if state.cancelling {
                state.queue_mirror.push_front_local(entry);
                None
            } else if !state.agent_running && state.active_prompts.is_empty() {
                state.queue_mirror.push_front_local(entry);
                dispatch_idle = true;
                None
            } else {
                entry.execution_text =
                    Self::prepare_execution_text(&mut state, entry.execution_text);
                let active_id = Self::activate_queued_client(&mut state, &entry.id);
                state.queue_mirror.reserve(
                    entry.id.clone(),
                    entry.execution_text.clone(),
                    entry.display_text.clone(),
                    entry.images.clone(),
                    QueueLane::Steering,
                    entry.origin,
                );
                Some((entry, active_id))
            }
        };
        self.publish_queue_snapshot().await;
        if dispatch_idle {
            return self.dispatch_next_queued().await;
        }
        let Some((entry, active_id)) = steer else {
            return false;
        };
        let mut request = json!({
            "type": "prompt",
            "message": entry.execution_text,
            "streamingBehavior": "steer",
        });
        if !entry.images.is_empty() {
            request["images"] = Value::Array(entry.images.clone());
        }
        if let Err(error) = self.rpc.request(request).await {
            tracing::warn!(%error, prompt_id = %entry.id, "queued interject failed");
            self.state
                .borrow_mut()
                .queue_mirror
                .release_reservation(&entry.id);
            if let Some(active) = active_id.and_then(|id| self.take_active_prompt(id)) {
                let _ = active.completion.send(PromptCompletion {
                    reason: acp::StopReason::Cancelled,
                    client_prompt_id: active.client_prompt_id,
                });
            }
            self.send_ui_notification(
                &format!("Pi queue interject failed: {error}"),
                Some("error"),
            )
            .await;
            self.publish_queue_snapshot().await;
            return false;
        }
        true
    }

    /// Safe-point delivery for locally held steering rows.
    ///
    /// Pi injects a steer between the current tool calls finishing and the
    /// next LLM call. The earliest adapter-visible point with identical
    /// observable behavior is an assistant `message_end`: forwarding there
    /// still lands the row in Pi's steering lane for the same turn, while
    /// every earlier moment stays cancellable via `x.ai/queue/remove`.
    /// Rows still held when the run settles fall through to
    /// [`Self::dispatch_next_queued`] and become the next turn's prompt.
    pub(super) async fn flush_pending_steering(&self) {
        loop {
            let next = {
                let state = self.state.borrow();
                if state.cancelling || !state.agent_running {
                    None
                } else {
                    state.queue_mirror.next_local_in_lane(QueueLane::Steering)
                }
            };
            let Some((id, version)) = next else {
                return;
            };
            if !self.interject_local_queue(&id, Some(version), None).await {
                return;
            }
        }
    }
}
