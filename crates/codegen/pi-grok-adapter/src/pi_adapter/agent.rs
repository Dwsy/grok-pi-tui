use super::*;

#[async_trait::async_trait(?Send)]
impl acp::Agent for PiAgent {
    async fn initialize(
        &self,
        _arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        // Only advertise recap when its injected bridge command is available.
        let meta = json!({ "sessionRecap": recap_extension_enabled() })
            .as_object()
            .cloned();
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(
                        acp::PromptCapabilities::new()
                            .image(true)
                            .embedded_context(true),
                    ),
            )
            .agent_info(acp::Implementation::new("pi", env!("CARGO_PKG_VERSION")).title("Pi"))
            .meta(meta))
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::new())
    }

    async fn new_session(
        &self,
        _arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let result = self
            .rpc
            .request(json!({ "type": "new_session" }))
            .await
            .map_err(acp_internal)?;
        let cancelled = result
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bootstrap = if cancelled {
            self.state.borrow().bootstrap.clone()
        } else {
            self.refresh().await.map_err(acp_internal)?
        };
        if !cancelled {
            self.state.borrow_mut().acp_session_id = bootstrap.state.session_id.clone();
        }
        self.publish_bootstrap(&bootstrap).await;
        // Fresh session starts outside plan mode and receives its own JSONL
        // sidecar plan file rather than sharing the configured session root.
        {
            let mut state = self.state.borrow_mut();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = crate::plan_mode::PiPlanTracker::with_plan_file(plan_path);
        }
        // Mirror load_session: push Pi's baseline contextUsage so the pager
        // context bar does not keep the previous session's numerator.
        if !cancelled {
            self.refresh_context_usage().await;
        }
        let mut response = acp::NewSessionResponse::new(bootstrap.state.session_id.clone());
        if let Some(models) = bootstrap.acp_models() {
            response = response.models(Some(models));
        }
        response = response.modes(Some(self.acp_session_modes()));
        self.persist_plan_mode_state().map_err(acp_internal)?;
        self.sync_plan_mode_control().map_err(acp_internal)?;
        self.publish_plan_file_path().await;
        Ok(response)
    }

    async fn load_session(
        &self,
        arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        let requested = arguments.session_id.0.to_string();
        let active = self.state.borrow().bootstrap.state.session_id.clone();
        if requested != active {
            let session_path = self
                .state
                .borrow()
                .session_paths
                .get(&requested)
                .cloned()
                .ok_or_else(|| {
                    acp::Error::invalid_params().data(format!(
                        "Pi session {requested} is not in the local catalog"
                    ))
                })?;
            let result = self
                .switch_session(&session_path, &requested)
                .await
                .map_err(acp_internal)?;
            if result.cancelled {
                return Err(acp::Error::invalid_params().data("Pi session switch cancelled"));
            }
        }
        let bootstrap = self.state.borrow().bootstrap.clone();
        if requested != bootstrap.state.session_id {
            return Err(acp::Error::invalid_params().data(format!(
                "Pi switched to {}, not requested session {requested}",
                bootstrap.state.session_id
            )));
        }
        {
            let mut state = self.state.borrow_mut();
            state.acp_session_id = requested.clone();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = load_plan_tracker(&plan_path).map_err(acp_internal)?;
        }
        self.replay_history().await.map_err(acp_internal)?;
        if bridge_command_is_registered(&bootstrap.commands, SUBAGENT_REPLAY_COMMAND) {
            self.replay_subagents("load").await?;
        }
        self.publish_bootstrap(&bootstrap).await;
        self.refresh_context_usage().await;
        let mut response = acp::LoadSessionResponse::new();
        if let Some(models) = bootstrap.acp_models() {
            response = response.models(Some(models));
        }
        response = response.modes(Some(self.acp_session_modes()));
        self.sync_plan_mode_control().map_err(acp_internal)?;
        self.publish_plan_file_path().await;
        Ok(response)
    }

    async fn set_session_mode(
        &self,
        arguments: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        let mode_id = arguments.mode_id.0.to_string();
        let mode = mode_id.as_str();
        let (changed, current_mode_id) = {
            let mut state = self.state.borrow_mut();
            let turn_in_flight = !state.active_prompts.is_empty();
            let changed = if mode == "plan" {
                state.plan_mode.enter_pending()
            } else {
                // Any non-plan mode exits plan mode (default/ask/agent).
                let was_plan = state.plan_mode.state() != crate::plan_mode::PiPlanState::Inactive;
                if was_plan {
                    state.plan_mode.user_exit(turn_in_flight);
                    true
                } else {
                    false
                }
            };
            // ACP display state follows the request. ExitPending is an internal
            // turn-drain state only; Pager must immediately confirm default.
            let current = if mode == "plan" { "plan" } else { mode };
            (changed, current.to_string())
        };
        if changed {
            self.persist_plan_mode_state().map_err(acp_internal)?;
            self.sync_plan_mode_control().map_err(acp_internal)?;
            self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                acp::CurrentModeUpdate::new(acp::SessionModeId::new(current_mode_id)),
            ))
            .await;
        }
        Ok(acp::SetSessionModeResponse::new())
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        if let Some(command) = direct_bash_command(&arguments.prompt) {
            return self.execute_bash(command, arguments.meta.as_ref()).await;
        }

        let (message, images) = prompt_to_pi(&arguments.prompt);
        let display_message = message.clone();
        if message.trim().is_empty() && images.is_empty() {
            return Err(acp::Error::invalid_params().data("Pi prompt is empty"));
        }
        let client_prompt_id = arguments
            .meta
            .as_ref()
            .and_then(|meta| meta.get("promptId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let plan_file_to_seed = {
            let state = self.state.borrow();
            (state.plan_mode.state() == crate::plan_mode::PiPlanState::Pending)
                .then(|| state.plan_mode.plan_file_path().to_path_buf())
        };
        if let Some(plan_file) = plan_file_to_seed {
            ensure_plan_file(&plan_file).map_err(acp_internal)?;
        }

        enum Disposition {
            Queued {
                id: String,
            },
            Direct {
                operation_id: u64,
                message: String,
                streaming_behavior: Option<&'static str>,
                pin_primary_running: bool,
            },
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        let disposition = {
            let mut state = self.state.borrow_mut();
            let streaming_behavior =
                prompt_streaming_behavior(Self::adapter_busy(&state), arguments.meta.as_ref());
            // Steer requests are held locally too (lane is the only
            // difference): the row stays editable/cancellable until the
            // assistant message_end safe-point flush forwards it to Pi.
            if let Some(lane) = streaming_behavior.map(|behavior| {
                if behavior == "steer" {
                    QueueLane::Steering
                } else {
                    QueueLane::FollowUp
                }
            }) {
                let id = state.queue_mirror.enqueue_local(
                    client_prompt_id.clone(),
                    message,
                    display_message,
                    images.clone(),
                    lane,
                    QueueOrigin::Client,
                );
                state
                    .queued_prompt_completions
                    .insert(id.clone(), completion_tx);
                Disposition::Queued { id }
            } else {
                let execution_message = Self::prepare_execution_text(&mut state, message);
                let operation_id = state.next_prompt_id;
                state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
                if let Some(lane) = streaming_behavior.and_then(queue_lane_for_behavior)
                    && let Some(client_id) = client_prompt_id.as_deref()
                {
                    state.queue_mirror.reserve(
                        client_id.to_string(),
                        execution_message.clone(),
                        display_message.clone(),
                        images.clone(),
                        lane,
                        QueueOrigin::Client,
                    );
                }
                let pin_primary_running = streaming_behavior.is_none()
                    && client_prompt_id.as_deref().is_some_and(|id| !id.is_empty());
                if pin_primary_running {
                    let client_id = client_prompt_id.clone().expect("checked non-empty");
                    state.live_prompt_id = Some(client_id.clone());
                    state.queue_mirror.set_running_primary(
                        client_id,
                        execution_message.clone(),
                        display_message,
                        images.clone(),
                        QueueOrigin::Client,
                    );
                }
                state.agent_running = true;
                state.active_prompts.push(ActivePrompt {
                    id: operation_id,
                    client_prompt_id: client_prompt_id.clone(),
                    completion: completion_tx,
                    agent_started: false,
                    cancelled: false,
                });
                Disposition::Direct {
                    operation_id,
                    message: execution_message,
                    streaming_behavior,
                    pin_primary_running,
                }
            }
        };

        self.persist_plan_mode_state().map_err(acp_internal)?;
        self.sync_plan_mode_control().map_err(acp_internal)?;

        match disposition {
            Disposition::Queued { id } => {
                self.publish_queue_snapshot().await;
                self.dispatch_next_queued().await;
                let completion = completion_rx.await.unwrap_or(PromptCompletion {
                    reason: acp::StopReason::Cancelled,
                    client_prompt_id: Some(id.clone()),
                });
                Ok(prompt_response(
                    completion.reason,
                    completion.client_prompt_id.as_deref().or(Some(id.as_str())),
                ))
            }
            Disposition::Direct {
                operation_id,
                message,
                streaming_behavior,
                pin_primary_running,
            } => {
                let mut request = json!({ "type": "prompt", "message": message });
                if !images.is_empty() {
                    request["images"] = Value::Array(images);
                }
                if let Some(streaming_behavior) = streaming_behavior {
                    request["streamingBehavior"] = Value::String(streaming_behavior.to_string());
                }
                if let Err(error) = self.rpc.request(request).await {
                    if let Some(client_id) = client_prompt_id.as_deref() {
                        self.state
                            .borrow_mut()
                            .queue_mirror
                            .release_reservation(client_id);
                    }
                    self.remove_prompt(operation_id);
                    if pin_primary_running {
                        let mut state = self.state.borrow_mut();
                        state.agent_running = false;
                        state.live_prompt_id = None;
                        state.queue_mirror.clear_running();
                    }
                    self.publish_queue_snapshot().await;
                    return Err(acp_internal(error));
                }
                if pin_primary_running {
                    self.rebroadcast_queue_mirror().await;
                }
                let probe = self.clone();
                tokio::task::spawn_local(async move {
                    probe.probe_prompt_without_agent().await;
                });
                let completion = completion_rx.await.unwrap_or(PromptCompletion {
                    reason: acp::StopReason::Cancelled,
                    client_prompt_id: client_prompt_id.clone(),
                });
                Ok(prompt_response(
                    completion.reason,
                    completion
                        .client_prompt_id
                        .as_deref()
                        .or(client_prompt_id.as_deref()),
                ))
            }
        }
    }

    async fn cancel(&self, arguments: acp::CancelNotification) -> Result<(), acp::Error> {
        let child_session_id = arguments.session_id.0.as_ref();
        let subagent_id = subagent_cancel_target(
            &self.state.borrow().subagent_session_to_id,
            child_session_id,
        );
        if let Some(subagent_id) = subagent_id {
            self.run_bridge_command(SUBAGENT_CANCEL_COMMAND, &subagent_id)
                .await?;
            return Ok(());
        }

        let (command, queued, running) = {
            let mut state = self.state.borrow_mut();
            for active in &mut state.active_prompts {
                active.cancelled = true;
            }
            state.cancelling = true;
            let command = if state.bash_running {
                "abort_bash"
            } else {
                "abort"
            };
            let queued = state.queue_mirror.clear_local();
            let running = state.queue_mirror.clear_running();
            state.queue_mirror.clear();
            (command, queued, running)
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

        // Do not await Pi's abort RPC. AgentSession.abort() waits for idle, and
        // extension continuations can otherwise keep ACP cancel blocked forever.
        if let Err(error) = self.rpc.notify(json!({ "type": command })) {
            tracing::warn!(%error, "failed to notify Pi abort");
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.settle_cancelled_prompts().await;
        });
        Ok(())
    }

    async fn set_session_model(
        &self,
        arguments: acp::SetSessionModelRequest,
    ) -> Result<acp::SetSessionModelResponse, acp::Error> {
        let model_id = arguments.model_id.0.to_string();
        let model = self
            .state
            .borrow()
            .model_map
            .get(&model_id)
            .cloned()
            .ok_or_else(|| {
                acp::Error::invalid_params().data(format!("unknown Pi model: {model_id}"))
            })?;
        let requested_effort = arguments
            .meta
            .as_ref()
            .and_then(|meta| meta.get("reasoningEffort"))
            .and_then(Value::as_str);
        let pi_effort = requested_effort
            .map(|effort| {
                model.pi_level_for_acp_effort(effort).ok_or_else(|| {
                    acp::Error::invalid_params().data(format!(
                        "Pi model {} does not support reasoning effort {effort}",
                        model.label
                    ))
                })
            })
            .transpose()?;
        let model_is_current = self
            .state
            .borrow()
            .bootstrap
            .state
            .model
            .as_ref()
            .is_some_and(|current| current.provider == model.provider && current.id == model.id);
        if !model_is_current {
            self.rpc
                .request(json!({
                    "type": "set_model",
                    "provider": model.provider,
                    "modelId": model.id,
                }))
                .await
                .map_err(acp_internal)?;
        }
        if let Some(level) = pi_effort {
            self.rpc
                .request(json!({
                    "type": "set_thinking_level",
                    "level": level,
                }))
                .await
                .map_err(acp_internal)?;
        }
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        self.publish_bootstrap(&bootstrap).await;
        Ok(acp::SetSessionModelResponse::new())
    }

    async fn ext_method(&self, arguments: acp::ExtRequest) -> Result<acp::ExtResponse, acp::Error> {
        match arguments.method.as_ref() {
            "x.ai/interject" => self.handle_steer_message(arguments.params.get()).await,
            "x.ai/terminal/background" => {
                self.handle_bash_background_request(arguments.params.get())
                    .await
            }
            "x.ai/task/kill" => self.handle_bash_kill_request(arguments.params.get()).await,
            "x.ai/scheduler/delete" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let task_id = string(&params, &["taskId", "task_id"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("taskId is required"))?;
                self.run_bridge_command(LOOP_DELETE_COMMAND, task_id)
                    .await?;
                ext_response(json!({ "taskId": task_id, "deleted": true })).map_err(acp_internal)
            }
            "x.ai/recap" => self.handle_recap_request(arguments.params.get()).await,
            "x.ai/btw" => self.handle_btw_request(arguments.params.get()).await,
            "x.ai/compact_conversation" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let mut request = json!({ "type": "compact" });
                if let Some(instructions) =
                    string(&params, &["customInstructions", "instructions", "context"])
                    && !instructions.trim().is_empty()
                {
                    request["customInstructions"] = Value::String(instructions.to_string());
                }
                let data = self.rpc.request(request).await.map_err(acp_internal)?;
                ext_response(data).map_err(acp_internal)
            }
            "pi/session/list" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let cwd = string(&params, &["cwd"])
                    .filter(|cwd| !cwd.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let all = string(&params, &["scope"]) == Some("all");
                let use_psm_index = params
                    .get("usePsmIndex")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.publish_session_catalog(cwd, all, use_psm_index).await;
                ext_response(json!({})).map_err(acp_internal)
            }
            // Full-text search across Pi sessions via PSM SQLite FTS5.
            "pi/session/search" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let query = string(&params, &["query"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let cwd = string(&params, &["cwd"]).filter(|c| !c.trim().is_empty());
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                if query.is_empty() {
                    return ext_response(json!({ "results": [], "total": 0 }))
                        .map_err(acp_internal);
                }
                let cwd_path = cwd.map(PathBuf::from);
                let results = tokio::task::spawn_blocking(move || {
                    crate::psm_session_catalog::full_text_search(cwd_path.as_deref(), &query, limit)
                })
                .await
                .unwrap_or(None)
                .unwrap_or_default();
                let total = results.len();
                ext_response(json!({
                    "results": results.iter().map(|r| json!({
                        "sessionId": r.session_id,
                        "cwd": r.cwd,
                        "summary": r.summary,
                        "snippet": r.snippet,
                        "score": r.score,
                        "matchedFields": r.matched_fields,
                        "updatedAt": r.updated_at,
                    })).collect::<Vec<_>>(),
                    "total": total,
                }))
                .map_err(acp_internal)
            }
            // Session message preview for /resume picker (PSM message_entries).
            "pi/session/messages" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let session_path = string(&params, &["sessionPath", "session_path"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let session_id = string(&params, &["sessionId", "session_id"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
                let messages = tokio::task::spawn_blocking(move || {
                    let path = if !session_path.is_empty() {
                        Some(session_path)
                    } else if !session_id.is_empty() {
                        crate::psm_session_catalog::resolve_session_path(&session_id)
                    } else {
                        None
                    };
                    match path {
                        Some(p) => crate::psm_session_catalog::load_session_messages(&p, limit)
                            .unwrap_or_default(),
                        None => Vec::new(),
                    }
                })
                .await
                .unwrap_or_default();
                ext_response(json!({
                    "messages": messages.iter().map(|m| json!({
                        "role": m.role,
                        "content": m.content,
                    })).collect::<Vec<_>>(),
                    "total": messages.len(),
                }))
                .map_err(acp_internal)
            }
            // Pi session entry tree (read-only projection of get_tree).
            "pi/session/tree" => {
                let tree = self.fetch_session_tree().await.map_err(acp_internal)?;
                ext_response(json!({
                    "leafId": tree.leaf_id,
                    "nodes": tree.rows.iter().map(|row| json!({
                        "id": row.id,
                        "parentId": row.parent_id,
                        "depth": row.depth,
                        "isLeaf": row.is_leaf,
                        "isCurrent": row.is_current,
                        "onActivePath": row.on_active_path,
                        "role": row.role,
                        "preview": row.preview,
                        "detail": row.detail,
                        "label": row.label,
                        "labelTimestamp": row.label_timestamp,
                        "entryType": row.entry_type,
                        "timestamp": row.timestamp,
                        "childIds": row.child_ids,
                        "hasText": row.has_text,
                    })).collect::<Vec<_>>(),
                }))
                .map_err(acp_internal)
            }
            // Navigate leaf via injected extension command → ctx.navigateTree.
            "pi/session/navigate_tree" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let summarize = params
                    .get("summarize")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let custom_instructions = string(&params, &["customInstructions", "instructions"]);
                let data = self
                    .navigate_session_tree(entry_id, summarize, custom_instructions)
                    .await?;
                ext_response(data).map_err(acp_internal)
            }
            // Set/clear entry label via injected extension → ctx.setLabel.
            "pi/session/tree_label" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let clear = params
                    .get("clear")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let label = if clear {
                    None
                } else {
                    string(&params, &["label", "text"])
                };
                let data = self.set_session_tree_label(entry_id, label).await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /fork message catalog (RPC get_fork_messages).
            "pi/session/fork_messages" => {
                let data = self.fetch_fork_messages().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /fork: create branched session file from a user message entry.
            "pi/session/fork" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                let data = self.fork_from_entry(entry_id).await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /clone: duplicate current leaf into a new session file.
            "pi/session/clone" => {
                let data = self.clone_current_session().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Pi /reload: settings + resources via injected ctx.reload().
            "pi/session/reload" => {
                let data = self.reload_session_resources().await?;
                ext_response(data).map_err(acp_internal)
            }
            // Queue delivery mode: set Pi's follow-up / steering drain mode.
            // "one-at-a-time" (default): deliver one queued message per turn.
            // "all": deliver all queued messages at once.
            "pi/queue/mode" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let mode = string(&params, &["mode"])
                    .map(str::trim)
                    .filter(|m| *m == "all" || *m == "one-at-a-time")
                    .ok_or_else(|| {
                        acp::Error::invalid_params().data("mode must be 'all' or 'one-at-a-time'")
                    })?;
                if params.get("steering").and_then(Value::as_bool) == Some(true) {
                    self.rpc
                        .request(json!({ "type": "set_steering_mode", "mode": mode }))
                        .await
                        .map_err(acp_internal)?;
                } else {
                    self.rpc
                        .request(json!({ "type": "set_follow_up_mode", "mode": mode }))
                        .await
                        .map_err(acp_internal)?;
                }
                ext_response(json!({ "mode": mode })).map_err(acp_internal)
            }
            // Tree file rollback: preview via injected extension command.
            "pi/session/rollback_preview" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                self.run_bridge_command("__pi_rollback_preview", entry_id)
                    .await?;
                ext_response(json!({ "entryId": entry_id })).map_err(acp_internal)
            }
            // Tree file rollback: execute via injected extension command.
            "pi/session/rollback_execute" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let entry_id = string(&params, &["entryId", "id", "targetId"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("entryId is required"))?;
                self.run_bridge_command("__pi_rollback_execute", entry_id)
                    .await?;
                ext_response(json!({ "entryId": entry_id, "executed": true })).map_err(acp_internal)
            }
            "x.ai/session/rename" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let title = string(&params, &["title", "name"])
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("session title is empty"))?;
                self.rpc
                    .request(json!({ "type": "set_session_name", "name": title }))
                    .await
                    .map_err(acp_internal)?;
                if let Ok(bootstrap) = self.refresh().await {
                    self.publish_bootstrap(&bootstrap).await;
                } else {
                    self.send_session_title(Some(title)).await;
                }
                ext_response(json!({})).map_err(acp_internal)
            }
            // Grok `/context` and context-bar click fetch this; map Pi
            // get_session_stats (+ message estimate) into native ContextInfo.
            "x.ai/session/info" => self.handle_session_info().await,
            "x.ai/workflows/list" => {
                let cwd = std::env::current_dir().ok();
                let listings = xai_grok_shell::session::workflow::list_workflows(cwd.as_deref());
                let workflows: Vec<Value> = listings
                    .into_iter()
                    .map(|w| {
                        json!({
                            "name": w.name,
                            "description": w.description,
                            "source": w.source,
                            "path": w.path,
                            "builtin": w.source == "builtin",
                        })
                    })
                    .collect();
                ext_response(json!({ "workflows": workflows })).map_err(acp_internal)
            }
            "x.ai/workflow/launch" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let name = string(&params, &["name", "workflow"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("name is required"))?;
                let args = string(&params, &["args", "input"]).unwrap_or("");
                let data = self.handle_workflow_request(name, args).await?;
                ext_response(data).map_err(acp_internal)
            }
            "x.ai/workflow/pause" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let run_id = string(&params, &["runId", "run_id", "name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("runId is required"))?;
                let ok = self.workflow_pause(run_id).await?;
                ext_response(json!({ "runId": run_id, "paused": ok })).map_err(acp_internal)
            }
            "x.ai/workflow/stop" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let run_id = string(&params, &["runId", "run_id", "name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("runId is required"))?;
                let ok = self.workflow_cancel(run_id).await?;
                ext_response(json!({ "runId": run_id, "stopped": ok })).map_err(acp_internal)
            }
            "x.ai/subagent/cancel" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).map_err(acp_internal)?;
                let subagent_id = string(&params, &["subagentId", "subagent_id"])
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| acp::Error::invalid_params().data("subagentId is required"))?;
                self.run_bridge_command(SUBAGENT_CANCEL_COMMAND, subagent_id)
                    .await?;
                ext_response(json!({
                    "subagentId": subagent_id,
                    "cancelled": true,
                    "outcome": "cancelled",
                }))
                .map_err(acp_internal)
            }
            method => Err(acp::Error::new(
                acp::ErrorCode::MethodNotFound.into(),
                format!("Method not found: {method}"),
            )),
        }
    }

    async fn ext_notification(&self, arguments: acp::ExtNotification) -> Result<(), acp::Error> {
        match arguments.method.as_ref() {
            "pi/extension_command" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let command = string(&params, &["command"])
                    .map(str::trim)
                    .filter(|command| command.starts_with('/'));
                if let Some(command) = command {
                    let is_btw_history = command
                        .split_whitespace()
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case("/btw-history"));
                    match self
                        .rpc
                        .request(json!({ "type": "prompt", "message": command }))
                        .await
                    {
                        Ok(_) if is_btw_history => {
                            if let Err(error) = self.refresh_entry_replay_cache().await {
                                tracing::warn!(%error, "failed to refresh Pi BTW history");
                                self.send_ui_notification(
                                    "Failed to load BTW history.",
                                    Some("error"),
                                )
                                .await;
                            } else {
                                self.send_current_btw_history("command").await;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%error, "failed to invoke Pi extension command");
                        }
                    }
                } else {
                    tracing::warn!("ignored malformed Pi extension command notification");
                }
                Ok(())
            }
            // Experimental Remote TUI: keys go to extension host via keyfile
            // (no Pi source patch / no custom stdin RPC types).
            "pi/ui/remote_tui/input" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                if let Some(data) = string(&params, &["data"]) {
                    if let Err(error) = append_remote_tui_key_event(json!({
                        "op": "input",
                        "data": data,
                    })) {
                        tracing::debug!(%error, "remote_tui keyfile input failed");
                    }
                }
                Ok(())
            }
            "pi/ui/remote_tui/cancel" => {
                if let Err(error) = append_remote_tui_key_event(json!({ "op": "cancel" })) {
                    tracing::debug!(%error, "remote_tui keyfile cancel failed");
                }
                Ok(())
            }
            // Native extension-shortcut dispatch (Pager match_key → RPC prompt →
            // hidden /__pi_shortcut_dispatch → extension handler).
            // Prefer RPC over keyfile; independent of remote-tui.
            "pi/ui/shortcut_dispatch" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                if let Some(key) = string(&params, &["key"]) {
                    let key = key.trim();
                    if !key.is_empty() {
                        let message = bridge_command_message(SHORTCUT_DISPATCH_COMMAND, key);
                        if let Err(error) = self
                            .rpc
                            .request(json!({ "type": "prompt", "message": message }))
                            .await
                        {
                            tracing::warn!(%error, %key, "shortcut_dispatch RPC prompt failed");
                            // Fallback: legacy keyfile if env/meta present.
                            if let Err(file_err) = append_shortcut_dispatch_event(json!({
                                "op": "dispatch",
                                "key": key,
                            })) {
                                tracing::debug!(%file_err, "shortcut_dispatch keyfile fallback failed");
                            }
                        }
                    }
                }
                Ok(())
            }
            "x.ai/queue/remove" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let expected_version = params
                    .get("expectedVersion")
                    .or_else(|| params.get("version"))
                    .and_then(Value::as_u64);
                let removed = self
                    .state
                    .borrow_mut()
                    .queue_mirror
                    .take_local(id, expected_version);
                if let Some(entry) = removed {
                    self.finish_queued_entries(vec![entry], acp::StopReason::Cancelled);
                } else {
                    self.send_ui_notification(
                        "This queue row is already running or belongs to Pi's external queue",
                        Some("warning"),
                    )
                    .await;
                }
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/clear" => {
                let removed = self.state.borrow_mut().queue_mirror.clear_local();
                self.finish_queued_entries(removed, acp::StopReason::Cancelled);
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/edit" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let new_text = string(&params, &["newText", "text"])
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                let edited = new_text
                    .is_some_and(|text| self.state.borrow_mut().queue_mirror.edit_local(id, text));
                if !edited {
                    self.send_ui_notification(
                        "Only pending adapter queue rows can be edited",
                        Some("warning"),
                    )
                    .await;
                }
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/reorder" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let ordered_ids = params
                    .get("orderedIds")
                    .or_else(|| params.get("ids"))
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.state
                    .borrow_mut()
                    .queue_mirror
                    .reorder_local(&ordered_ids);
                self.publish_queue_snapshot().await;
                Ok(())
            }
            "x.ai/queue/interject" => {
                let params: Value =
                    serde_json::from_str(arguments.params.get()).unwrap_or_default();
                let id = string(&params, &["id"]).unwrap_or_default();
                let expected_version = params
                    .get("expectedVersion")
                    .or_else(|| params.get("version"))
                    .and_then(Value::as_u64);
                let new_text = string(&params, &["newText", "text"])
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                if !self
                    .interject_local_queue(id, expected_version, new_text)
                    .await
                {
                    self.send_ui_notification(
                        "Only a pending adapter queue row can be sent now",
                        Some("warning"),
                    )
                    .await;
                    self.publish_queue_snapshot().await;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl PiAgent {
    async fn handle_bash_background_request(
        &self,
        params_raw: &str,
    ) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let tool_call_id = string(&params, &["terminalId", "toolCallId"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("terminalId is required"))?;
        let control_meta = self.bash_control_meta.as_deref().ok_or_else(|| {
            acp::Error::invalid_params().data("Pi Bash background control is disabled")
        })?;
        append_bash_background_control(control_meta, tool_call_id).map_err(acp_internal)?;
        ext_response(json!({ "accepted": true, "terminalId": tool_call_id })).map_err(acp_internal)
    }

    /// Kill a Pi-owned background Bash task via the private control channel.
    ///
    /// Pager clicks the native task-card kill control and sends `x.ai/task/kill`.
    /// The adapter only validates the task id against the extension-published
    /// `runningTaskIds` set and appends a control event; the extension owns the
    /// child process and emits `task_completed` after the kill settles.
    async fn handle_bash_kill_request(
        &self,
        params_raw: &str,
    ) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let task_id = string(&params, &["taskId", "task_id"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("taskId is required"))?;
        let control_meta = self.bash_control_meta.as_deref().ok_or_else(|| {
            acp::Error::invalid_params().data("Pi Bash background control is disabled")
        })?;
        let outcome = append_bash_kill_control(control_meta, task_id).map_err(acp_internal)?;
        // `ext_response` wraps the payload under `result`, matching
        // `ExtMethodResult<KillTaskResponse>` expected by Pager.
        ext_response(json!({
            "taskId": task_id,
            "outcome": outcome,
        }))
        .map_err(acp_internal)
    }

    /// Fire-and-forget session recap via injected `__pi_grok_recap` extension.
    ///
    /// Params: `{ sessionId?, auto?, model?, models?, customInstructions? }`.
    /// Language is taken from process locale.
    async fn handle_recap_request(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).unwrap_or_else(|_| json!({}));
        let auto = params.get("auto").and_then(Value::as_bool).unwrap_or(false);
        let reserved = {
            let mut state = self.state.borrow_mut();
            reserve_recap_request(&mut state.recap_in_flight)
        };
        if !reserved {
            return ext_response(json!({ "ok": true, "auto": auto, "skipped": "in_flight" }))
                .map_err(acp_internal);
        }
        let models = model_chain_from_params(&params);
        let model = models.first().cloned().or_else(|| {
            string(&params, &["model", "modelId", "recapModel"])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        });
        let thinking_level = string(&params, &["thinkingLevel", "thinking_level"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let recap_mermaid = params
            .get("recapMermaid")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let terminal_width = params
            .get("terminalWidth")
            .or_else(|| params.get("terminal_width"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let custom_instructions = string(
            &params,
            &["customInstructions", "custom_instructions", "instructions"],
        )
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
        let language = system_language_tag();
        let payload = json!({
            "auto": auto,
            "model": model,
            "models": models,
            "thinkingLevel": thinking_level,
            "recapMermaid": recap_mermaid,
            "terminalWidth": terminal_width,
            "language": language,
            "customInstructions": custom_instructions,
        });
        let args = payload.to_string();
        // Extension emits custom message asynchronously; adapter projects it.
        // Await preflight so extension errors surface before we ack.
        let result = self.run_bridge_command(RECAP_COMMAND, &args).await;
        self.state.borrow_mut().recap_in_flight = false;
        result?;
        ext_response(json!({ "ok": true, "auto": auto })).map_err(acp_internal)
    }

    /// Blocking side question via injected `__pi_grok_btw` extension.
    ///
    /// Params: `{ sessionId?, question, models?[] }`.
    /// Returns `{ result: { answer } }` matching stock Grok pager parse path.
    async fn handle_btw_request(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        if !btw_extension_enabled() {
            return Err(acp::Error::method_not_found().data(
                "Native /btw is off. F2 → Agent → Pi /btw → on, fully quit, then restart grok-pi.",
            ));
        }
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let question = string(&params, &["question", "text", "q"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| acp::Error::invalid_params().data("question is required"))?;
        let models = model_chain_from_params(&params);
        let thinking_level = string(&params, &["thinkingLevel", "thinking_level"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let request_id = format!("btw-{}", uuid::Uuid::now_v7());
        let (tx, rx) = oneshot::channel();
        self.state
            .borrow_mut()
            .pending_btw
            .insert(request_id.clone(), tx);
        let payload = json!({
            "requestId": request_id.clone(),
            "question": question,
            "models": models,
            "thinkingLevel": thinking_level,
        });
        if let Err(error) = self
            .run_bridge_command(BTW_COMMAND, &payload.to_string())
            .await
        {
            self.state.borrow_mut().pending_btw.remove(&request_id);
            return Err(error);
        }
        let result = match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(Ok(answer))) => Ok(answer),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err("side question channel closed".into()),
            Err(_) => {
                self.state.borrow_mut().pending_btw.remove(&request_id);
                Err("side question timed out".into())
            }
        };
        match result {
            Ok(answer) => ext_response(json!({ "answer": answer })).map_err(acp_internal),
            Err(error) => Err(acp::Error::internal_error().data(error)),
        }
    }

    async fn handle_steer_message(&self, params_raw: &str) -> Result<acp::ExtResponse, acp::Error> {
        let params: Value = serde_json::from_str(params_raw).map_err(acp_internal)?;
        let blocks = params
            .get("content")
            .cloned()
            .and_then(|value| serde_json::from_value::<Vec<acp::ContentBlock>>(value).ok());
        let (message, images) = if let Some(blocks) = blocks.as_deref() {
            prompt_to_pi(blocks)
        } else {
            (
                string(&params, &["text"]).unwrap_or_default().to_string(),
                Vec::new(),
            )
        };
        if message.trim().is_empty() && images.is_empty() {
            return Err(acp::Error::invalid_params().data("Pi interjection is empty"));
        }
        let client_id = string(&params, &["interjectionId", "promptId"])
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        if let Some(client_id) = client_id.as_deref() {
            self.state.borrow_mut().queue_mirror.reserve(
                client_id.to_string(),
                message.clone(),
                message.clone(),
                images.clone(),
                QueueLane::Steering,
                QueueOrigin::Client,
            );
        }
        let mut request = json!({
            "type": "prompt",
            "message": message,
            "streamingBehavior": "steer",
        });
        if !images.is_empty() {
            request["images"] = Value::Array(images);
        }
        let data = match self.rpc.request(request).await {
            Ok(data) => data,
            Err(error) => {
                if let Some(client_id) = client_id.as_deref() {
                    self.state
                        .borrow_mut()
                        .queue_mirror
                        .release_reservation(client_id);
                }
                return Err(acp_internal(error));
            }
        };
        ext_response(data).map_err(acp_internal)
    }
}
