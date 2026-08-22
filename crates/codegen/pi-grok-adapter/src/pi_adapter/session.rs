use super::*;

impl PiAgent {
    /// Publish Pi's local session catalog for Grok's existing native picker.
    ///
    /// Pi keeps ownership of the JSONL format and of switching; this read-only
    /// metadata projection only gives the pager a selectable catalog.
    pub async fn publish_session_catalog(&self, cwd: PathBuf, all: bool, use_psm_index: bool) {
        let session_dir = {
            let state = self.state.borrow();
            catalog_session_dir(&state.bootstrap.state, &state.session_dir)
        };
        let psm_cwd = cwd.clone();
        let sessions = tokio::task::spawn_blocking(move || {
            if use_psm_index {
                if let Some(sessions) = crate::psm_session_catalog::load_catalog(&psm_cwd, all) {
                    return sessions;
                }
            }
            if all {
                scan_local_sessions(&session_dir)
            } else {
                scan_local_sessions_for_cwd(&session_dir, &cwd)
            }
        })
        .await
        .unwrap_or_default();
        let paths: HashMap<_, _> = sessions
            .iter()
            .map(|session| (session.id.clone(), session.path.clone()))
            .collect();
        self.state.borrow_mut().session_paths.extend(paths);
        self.send_ext_notification(
            "pi/ui/session_catalog",
            json!({
                "scope": if all { "all" } else { "current" },
                "sessions": sessions.into_iter().map(|session| json!({
                    "id": session.id,
                    "summary": session.name.as_deref().unwrap_or(&session.first_message),
                    "name": session.name,
                    "firstMessage": session.first_message,
                    "sessionPath": session.path,
                    "cwd": session.cwd,
                    "createdAt": session.created_at,
                    "updatedAt": session.modified_at,
                    "modelId": session.model_id,
                    "totalTokens": session.total_tokens,
                    "totalCost": session.total_cost,
                    "messageCount": session.message_count,
                    "parentSessionPath": session.parent_session_path,
                })).collect::<Vec<_>>(),
            }),
        )
        .await;
    }

    /// Request Pi to replace its active session. The adapter publishes the new
    /// session identity only after Pi accepts the switch and its replacement
    /// state can be loaded successfully.
    pub async fn switch_session(
        &self,
        session_path: &Path,
        expected_session_id: &str,
    ) -> Result<PiSessionSwitch> {
        self.state
            .borrow_mut()
            .pending_subagent_bridge
            .begin(expected_session_id)?;
        let response = match self
            .rpc
            .request(json!({
                "type": "switch_session",
                "sessionPath": session_path,
            }))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.state
                    .borrow_mut()
                    .pending_subagent_bridge
                    .abandon(expected_session_id);
                return Err(error);
            }
        };
        let result = parse_session_switch(&response);
        if result.cancelled {
            self.state
                .borrow_mut()
                .pending_subagent_bridge
                .abandon(expected_session_id);
            return Ok(result);
        }
        let bootstrap = match PiBootstrap::load(&self.rpc).await {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                self.state
                    .borrow_mut()
                    .pending_subagent_bridge
                    .abandon(expected_session_id);
                return Err(error);
            }
        };
        if bootstrap.state.session_id != expected_session_id {
            self.state
                .borrow_mut()
                .pending_subagent_bridge
                .abandon(expected_session_id);
            bail!(
                "Pi switched to {}, not requested session {expected_session_id}",
                bootstrap.state.session_id
            );
        }
        self.replace_bootstrap(bootstrap);
        Ok(result)
    }

    /// Read-only projection of Pi's current entry tree (`get_tree`).
    ///
    /// Parse + flatten + drop of the nested Value happen on a large-stack
    /// worker: long sessions produce trees deep enough to overflow the default
    /// Tokio worker stack even after serde_json recursion limits are disabled.
    pub(super) async fn fetch_session_tree(&self) -> Result<PiSessionTree> {
        let (tree, _) = self.fetch_session_tree_with_editor_text(None).await?;
        Ok(tree)
    }

    pub(super) async fn fetch_session_tree_with_editor_text(
        &self,
        entry_id: Option<&str>,
    ) -> Result<(PiSessionTree, Option<String>)> {
        let entry_id = entry_id.map(str::to_owned);
        let data = self.rpc.request(json!({ "type": "get_tree" })).await?;
        tokio::task::spawn_blocking(move || {
            crate::pi_rpc::with_large_stack(move || {
                let editor_text = entry_id
                    .as_deref()
                    .and_then(|entry_id| tree_entry_editor_text(&data, entry_id));
                let tree = parse_session_tree(&data);
                drop(data);
                (tree, editor_text)
            })
        })
        .await
        .map_err(|error| anyhow!("Pi get_tree worker failed: {error}"))
    }

    /// Refresh the retained flat entry log. Once a session has been loaded,
    /// later branch switches ask Pi only for append-log entries after the last
    /// known id. Older hosts that reject `since` are retried with a full request;
    /// duplicate ids are ignored by the cache if they return a full payload.
    pub(super) async fn refresh_entry_replay_cache(&self) -> Result<()> {
        let (session_id, since) = {
            let state = self.state.borrow();
            let since = state
                .entry_replay_cache
                .matches_session(&state.acp_session_id)
                .then(|| state.entry_replay_cache.since_id().map(str::to_owned))
                .flatten();
            (state.acp_session_id.clone(), since)
        };
        let mut incremental = since.is_some();
        let request = match since.as_deref() {
            Some(since) => json!({ "type": "get_entries", "since": since }),
            None => json!({ "type": "get_entries" }),
        };
        let data = match self.rpc.request(request).await {
            Ok(data) => data,
            Err(error) if incremental => {
                tracing::debug!(%error, "Pi get_entries(since) failed; retrying full history");
                incremental = false;
                self.rpc.request(json!({ "type": "get_entries" })).await?
            }
            Err(error) => return Err(error),
        };
        let mut state = self.state.borrow_mut();
        if state.acp_session_id != session_id {
            bail!("Pi session changed while refreshing branch history");
        }
        if incremental && state.entry_replay_cache.matches_session(&session_id) {
            state.entry_replay_cache.append(&data);
        } else {
            state.entry_replay_cache.reset(&session_id, &data);
        }
        tracing::debug!(
            session_id,
            incremental,
            entries = state.entry_replay_cache.entry_count(),
            leaf_id = ?state.entry_replay_cache.leaf_id(),
            "refreshed Pi active-branch replay cache"
        );
        Ok(())
    }

    /// Run a read-only bridge command without entering `active_prompts`.
    ///
    /// Pi executes extension commands immediately even during streaming and
    /// acknowledges RPC `prompt` only after their handler finishes. Tracking
    /// this as a normal prompt would instead bind it to the current turn's
    /// `agent_settled`, delaying `/context` until the agent becomes idle.
    pub(super) async fn run_immediate_bridge_command(
        &self,
        command: &str,
        args: &str,
    ) -> Result<(), acp::Error> {
        self.require_bridge_command(command)?;
        let message = bridge_command_message(command, args);
        self.rpc
            .request(json!({ "type": "prompt", "message": message }))
            .await
            .map_err(acp_internal)?;
        Ok(())
    }

    /// Run a stateful hidden bridge extension command (`/__pi_*`) and wait for
    /// the non-agent preflight probe to complete.
    pub(super) async fn run_bridge_command(
        &self,
        command: &str,
        args: &str,
    ) -> Result<(), acp::Error> {
        self.require_bridge_command(command)?;
        let message = bridge_command_message(command, args);
        let (completion_tx, completion_rx) = oneshot::channel();
        let prompt_id = {
            let mut state = self.state.borrow_mut();
            let prompt_id = state.next_prompt_id;
            state.next_prompt_id = state.next_prompt_id.wrapping_add(1).max(1);
            state.active_prompts.push(ActivePrompt {
                id: prompt_id,
                client_prompt_id: None,
                completion: completion_tx,
                agent_started: false,
                cancelled: false,
            });
            prompt_id
        };
        let request = json!({ "type": "prompt", "message": message });
        if let Err(error) = self.rpc.request(request).await {
            self.remove_prompt(prompt_id);
            return Err(acp_internal(error));
        }
        let probe = self.clone();
        tokio::task::spawn_local(async move {
            probe.probe_prompt_without_agent().await;
        });
        let _ = completion_rx.await;
        Ok(())
    }

    pub(super) fn require_bridge_command(&self, command: &str) -> Result<(), acp::Error> {
        if bridge_command_is_registered(&self.state.borrow().bootstrap.commands, command) {
            return Ok(());
        }
        Err(acp::Error::method_not_found().data(format!(
            "Pi bridge command /{command} is unavailable because its extension is not loaded."
        )))
    }

    pub(super) fn ensure_workflow_host(&self) -> Result<()> {
        if !self.workflows_enabled {
            bail!(
                "Pi workflows is off. F2 → Agent → Pi workflows → on, fully quit, then restart grok-pi (extension injects only at startup)."
            );
        }
        if self.workflow_host.borrow().is_some() {
            return Ok(());
        }
        let (session_id, cwd, session_dir) = {
            let state = self.state.borrow();
            let session_id = state.acp_session_id.clone();
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let session_dir = state
                .bootstrap
                .state
                .session_file
                .as_ref()
                .and_then(|p| Path::new(p).parent().map(|p| p.to_path_buf()))
                .or_else(|| Some(state.session_dir.clone()));
            (session_id, cwd, session_dir)
        };
        let host = std::sync::Arc::new(WorkflowHost::new(
            session_id,
            cwd,
            session_dir,
            self.workflow_bridge_tx.clone(),
        ));
        *self.workflow_host.borrow_mut() = Some(host);
        Ok(())
    }

    pub(super) fn workflow_host_arc(&self) -> Result<std::sync::Arc<WorkflowHost>> {
        self.ensure_workflow_host()?;
        self.workflow_host
            .borrow()
            .clone()
            .ok_or_else(|| anyhow!("workflow host missing after ensure"))
    }

    pub(super) async fn emit_workflow_notifications(&self) {
        let payloads = match self.workflow_host.borrow().as_ref() {
            Some(host) => host.notification_payloads(),
            None => Vec::new(),
        };
        for payload in payloads {
            self.send_ext_notification("x.ai/session_notification", payload)
                .await;
        }
    }

    pub(super) async fn handle_workflow_request(
        &self,
        name: &str,
        args: &str,
    ) -> Result<Value, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let request = parse_workflow_request(name, args)
            .map_err(|e| acp::Error::invalid_params().data(e.to_string()))?;
        match request {
            WorkflowRequest::Launch {
                name,
                objective,
                args,
            } => {
                let (run_id, outcome_rx) = host
                    .launch_named(&name, objective, args)
                    .await
                    .map_err(acp_internal)?;
                self.emit_workflow_notifications().await;
                let agent = self.clone();
                let host_bg = host.clone();
                let run_id_ret = run_id.clone();
                // Fire-and-forget when called without a response file (ACP methods).
                // The tool path waits via `run_workflow_tool_to_completion`.
                tokio::task::spawn_local(async move {
                    let _ = host_bg
                        .drive_until_outcome(outcome_rx, |payload| {
                            let agent = agent.clone();
                            tokio::task::spawn_local(async move {
                                agent
                                    .send_ext_notification("x.ai/session_notification", payload)
                                    .await;
                            });
                        })
                        .await;
                    agent.emit_workflow_notifications().await;
                });
                Ok(json!({ "runId": run_id_ret, "started": true }))
            }
            WorkflowRequest::Manage { op, target } => {
                match op.as_str() {
                    "pause" => {
                        let ok = host.pause(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "pause", "target": target, "ok": ok }))
                    }
                    "stop" => {
                        let ok = host.cancel(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "stop", "target": target, "ok": ok }))
                    }
                    other => Err(acp::Error::invalid_params()
                        .data(format!("unsupported workflow op: {other}"))),
                }
            }
        }
    }

    /// Launch (or manage) and block until terminal outcome — used by the Pi
    /// `workflow` tool so the parent turn receives the real report text.
    pub(super) async fn run_workflow_tool_to_completion(
        &self,
        name: &str,
        args: &str,
    ) -> Result<Value, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let request = parse_workflow_request(name, args)
            .map_err(|e| acp::Error::invalid_params().data(e.to_string()))?;
        match request {
            WorkflowRequest::Manage { op, target } => {
                match op.as_str() {
                    "pause" => {
                        let ok = host.pause(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "pause", "target": target, "ok": ok }))
                    }
                    "stop" => {
                        let ok = host.cancel(&target).await;
                        self.emit_workflow_notifications().await;
                        Ok(json!({ "op": "stop", "target": target, "ok": ok }))
                    }
                    other => Err(acp::Error::invalid_params()
                        .data(format!("unsupported workflow op: {other}"))),
                }
            }
            WorkflowRequest::Launch {
                name,
                objective,
                args,
            } => {
                let (run_id, outcome_rx) = host
                    .launch_named(&name, objective, args)
                    .await
                    .map_err(acp_internal)?;
                self.emit_workflow_notifications().await;
                let agent = self.clone();
                let host_bg = host.clone();
                let outcome = host_bg
                    .drive_until_outcome(outcome_rx, |payload| {
                        let agent = agent.clone();
                        tokio::task::spawn_local(async move {
                            agent
                                .send_ext_notification("x.ai/session_notification", payload)
                                .await;
                        });
                    })
                    .await
                    .map_err(acp_internal)?;
                agent.emit_workflow_notifications().await;
                let text = format_outcome_for_tool(&run_id, &outcome);
                Ok(json!({
                    "runId": run_id,
                    "outcome": outcome_to_json(&outcome),
                    "text": text,
                }))
            }
        }
    }

    pub(super) async fn workflow_pause(&self, run_id: &str) -> Result<bool, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let ok = host.pause(run_id).await;
        self.emit_workflow_notifications().await;
        Ok(ok)
    }

    pub(super) async fn workflow_cancel(&self, run_id: &str) -> Result<bool, acp::Error> {
        let host = self.workflow_host_arc().map_err(acp_internal)?;
        let ok = host.cancel(run_id).await;
        self.emit_workflow_notifications().await;
        Ok(ok)
    }

    pub(super) async fn emit_goal_updated_from_control(&self, control: &GoalControl) {
        let session_id = self.session_id().0.to_string();
        let payload = {
            let host = self.goal_host.borrow();
            let Some(host) = host.as_ref() else {
                return;
            };
            host.notification_payload(&session_id, control)
        };
        self.send_ext_notification("x.ai/session_notification", payload)
            .await;
    }

    pub(super) async fn refresh_goal_from_disk(&self) -> Option<GoalControl> {
        let mut host = self.goal_host.borrow_mut();
        let host = host.as_mut()?;
        host.load()
    }

    /// Extension bridge: control file already written; reload + GoalUpdated.
    pub(super) async fn handle_goal_bridge_message(&self, event: &Value) -> Result<bool> {
        if self.goal_host.borrow().is_none() {
            return Ok(false);
        }
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-goal/v1") {
            return Ok(false);
        }
        if let Some(control) = self.refresh_goal_from_disk().await {
            self.emit_goal_updated_from_control(&control).await;
        } else if let Some(control) = message
            .get("details")
            .or_else(|| message.get("data"))
            .and_then(|d| d.get("control"))
            .and_then(|c| serde_json::from_value::<GoalControl>(c.clone()).ok())
        {
            if let Some(host) = self.goal_host.borrow_mut().as_mut() {
                host.apply_control(control.clone());
            }
            self.emit_goal_updated_from_control(&control).await;
        }
        Ok(true)
    }

    /// Extension bridge: scheduled task created/fired/deleted → native tasks pane.
    pub(super) async fn handle_loop_bridge_message(&self, event: &Value) -> Result<bool> {
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-loop/v1") {
            return Ok(false);
        }
        let details = message
            .get("details")
            .or_else(|| message.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some((event_name, task)) = loop_host::parse_loop_bridge(&details) else {
            return Ok(true);
        };
        let session_id = self.session_id().0.to_string();
        if let Some((method, payload)) =
            loop_host::scheduled_task_notification(&session_id, &event_name, &task)
        {
            self.send_ext_notification(&method, payload).await;
        }
        Ok(true)
    }

    /// On idle: if goal Active, inject follow-up continuation (legacy path).
    pub(super) async fn maybe_continue_goal(&self) {
        if self.goal_host.borrow().is_none() {
            return;
        }
        // Avoid stacking continuations when the queue already has work.
        {
            let snap = self.state.borrow().queue_mirror.snapshot();
            if snap.follow_up_count > 0
                || snap.steering_count > 0
                || snap.running_prompt_id.is_some()
            {
                return;
            }
        }
        let control = match self.refresh_goal_from_disk().await {
            Some(c) if c.is_active() => c,
            Some(c) => {
                self.emit_goal_updated_from_control(&c).await;
                return;
            }
            None => return,
        };
        let guard_armed = self
            .goal_host
            .borrow()
            .as_ref()
            .is_some_and(|host| host.continuation_guard_armed(&control));
        if guard_armed {
            let blocked = self.goal_host.borrow_mut().as_mut().and_then(|host| {
                match host.block_continuation_loop(&control) {
                    Ok(blocked) => Some(blocked),
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "failed to persist goal continuation loop guard"
                        );
                        None
                    }
                }
            });
            if let Some(blocked) = blocked {
                self.emit_goal_updated_from_control(&blocked).await;
            }
            return;
        }
        self.emit_goal_updated_from_control(&control).await;
        let directive = GoalHost::continuation_directive(&control);
        self.enqueue_extension_message(directive, Vec::new(), Some("followUp"))
            .await;
        if let Some(host) = self.goal_host.borrow_mut().as_mut() {
            host.record_continuation(&control);
        }
    }

    pub(super) async fn handle_workflow_bridge_message(&self, event: &Value) -> Result<bool> {
        let message = event
            .get("message")
            .or_else(|| event.get("entry"))
            .unwrap_or(event);
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .or_else(|| message.get("type").and_then(Value::as_str));
        if custom_type != Some("pi-grok-workflow/v1") {
            return Ok(false);
        }
        let details = message
            .get("details")
            .or_else(|| message.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let kind = details.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "tool_request" {
            let name = details
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = details
                .get("args")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let response_path = details
                .get("responsePath")
                .or_else(|| details.get("response_path"))
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from);
            let agent = self.clone();
            // Never block the RPC event loop for long Rhai runs: write the
            // tool response file from a local task; the extension polls it.
            tokio::task::spawn_local(async move {
                let result = if response_path.is_some() {
                    agent.run_workflow_tool_to_completion(&name, &args).await
                } else {
                    agent.handle_workflow_request(&name, &args).await
                };
                match (result, response_path) {
                    (Ok(value), Some(path)) => {
                        if let Err(error) = std::fs::write(
                            &path,
                            serde_json::to_vec_pretty(&value).unwrap_or_default(),
                        ) {
                            tracing::warn!(%error, path = %path.display(), "workflow tool response write failed");
                        }
                    }
                    (Err(error), Some(path)) => {
                        let payload = json!({
                            "error": error.to_string(),
                            "text": format!("Workflow request failed: {error}"),
                        });
                        let _ = std::fs::write(
                            &path,
                            serde_json::to_vec_pretty(&payload).unwrap_or_default(),
                        );
                        agent
                            .send_ui_notification(
                                &format!("Workflow request failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                    (Err(error), None) => {
                        agent
                            .send_ui_notification(
                                &format!("Workflow request failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                    (Ok(_), None) => {}
                }
            });
            return Ok(true);
        }
        self.emit_workflow_notifications().await;
        Ok(true)
    }

    /// Navigate Pi's leaf via the injected `__pi_navigate_tree` extension
    /// command (official `ctx.navigateTree`).
    pub(super) async fn navigate_session_tree(
        &self,
        entry_id: &str,
        summarize: bool,
        custom_instructions: Option<&str>,
    ) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("tree entry id is empty"));
        }
        let busy = {
            let state = self.state.borrow();
            // `bootstrap.state.is_streaming` is a refresh-time snapshot and is
            // not cleared by the live `agent_settled` event. Using it here can
            // therefore leave tree navigation permanently blocked after the
            // response has finished. The live lifecycle flags below are the
            // authoritative idle barrier.
            state.agent_running || !state.active_prompts.is_empty()
        };
        if busy {
            return Err(acp::Error::invalid_params()
                .data("wait for the current Pi response before navigating the session tree"));
        }
        let mut args = entry_id.to_string();
        if summarize {
            args.push_str(" --summarize");
        }
        if let Some(instructions) = custom_instructions.map(str::trim).filter(|s| !s.is_empty()) {
            // Extension parses --instructions <rest-of-line>.
            args.push_str(" --instructions ");
            args.push_str(instructions);
        }
        self.run_bridge_command(NAVIGATE_TREE_COMMAND, &args)
            .await?;

        // The leaf moved inside the same session file. Reuse Pi's flat append
        // log instead of reloading models, commands, state, and the full nested
        // tree. The following Pager session/load consumes this fresh snapshot.
        self.refresh_entry_replay_cache()
            .await
            .map_err(acp_internal)?;
        let (session_id, leaf_id, editor_text) = {
            let state = self.state.borrow();
            let session_id = state.acp_session_id.clone();
            let leaf_id = state.entry_replay_cache.leaf_id().map(str::to_owned);
            let editor_text = state.entry_replay_cache.editor_text(entry_id);
            (session_id, leaf_id, editor_text)
        };
        Ok(json!({
            "sessionId": session_id,
            "leafId": leaf_id,
            "editorText": editor_text,
            "cancelled": false,
        }))
    }

    pub(super) async fn set_session_tree_label(
        &self,
        entry_id: &str,
        label: Option<&str>,
    ) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("tree entry id is empty"));
        }
        let args = match label.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => format!("{entry_id} {text}"),
            None => format!("{entry_id} --clear"),
        };
        self.run_bridge_command(LABEL_TREE_COMMAND, &args).await?;
        let tree = self.fetch_session_tree().await.map_err(acp_internal)?;
        Ok(json!({
            "leafId": tree.leaf_id,
            "entryId": entry_id,
            "label": label,
        }))
    }

    /// Read-only list of user messages available for Pi `/fork`.
    pub(super) async fn fetch_fork_messages(&self) -> Result<Value, acp::Error> {
        let data = self
            .rpc
            .request(json!({ "type": "get_fork_messages" }))
            .await
            .map_err(acp_internal)?;
        let messages = data.get("messages").cloned().unwrap_or_else(|| json!([]));
        Ok(json!({ "messages": messages }))
    }

    /// Fork Pi into a new session file from a user-message entry.
    ///
    /// On success the adapter rebinds to the new session identity (same process,
    /// new JSONL) so subsequent `session/load` replays the forked history.
    pub(super) async fn fork_from_entry(&self, entry_id: &str) -> Result<Value, acp::Error> {
        let entry_id = entry_id.trim();
        if entry_id.is_empty() {
            return Err(acp::Error::invalid_params().data("entryId is empty"));
        }
        let response = self
            .rpc
            .request(json!({
                "type": "fork",
                "entryId": entry_id,
            }))
            .await
            .map_err(acp_internal)?;
        let cancelled = response
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = response
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cancelled {
            return Ok(json!({
                "cancelled": true,
                "entryId": entry_id,
            }));
        }
        let bootstrap = self.rebind_after_session_branch().await?;
        Ok(json!({
            "cancelled": false,
            "entryId": entry_id,
            "sessionId": bootstrap.state.session_id,
            "sessionFile": bootstrap.state.session_file,
            "text": text,
        }))
    }

    /// Reload Pi settings, extensions, skills, prompts, themes, and context files.
    ///
    /// Uses injected `__pi_reload` → official `ctx.reload()` (RPC has no bare
    /// `reload` command). Refreshes adapter bootstrap so command/model catalogs
    /// match the reloaded runtime.
    pub(super) async fn reload_session_resources(&self) -> Result<Value, acp::Error> {
        {
            let mut adapter_state = self.state.borrow_mut();
            if !reserve_reload_request(&mut adapter_state.reload_in_flight) {
                return Err(acp::Error::internal_error().data("Reload is already in progress."));
            }
        }

        let result = self.reload_session_resources_inner().await;
        self.state.borrow_mut().reload_in_flight = false;
        result
    }

    pub(super) async fn reload_session_resources_inner(&self) -> Result<Value, acp::Error> {
        let state = parse_state(
            &self
                .rpc
                .request(json!({ "type": "get_state" }))
                .await
                .map_err(acp_internal)?,
        );
        if state.is_streaming {
            return Err(acp::Error::internal_error()
                .data("Wait for the current response to finish before reloading."));
        }
        if state.is_compacting {
            return Err(acp::Error::internal_error()
                .data("Wait for compaction to finish before reloading."));
        }
        // Pi interactive calls `resetExtensionUI()` after the same gates and
        // before `session.reload()`. Its extension runner is about to be
        // replaced, so Pager must not retain widgets/statuses/shortcuts from
        // extensions that may no longer be loaded. Keep this as a narrow UI
        // notification; the adapter remains headless.
        self.send_ext_notification("pi/ui/reset_extension_ui", json!({}))
            .await;
        self.run_bridge_command(RELOAD_COMMAND, "").await?;
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        self.publish_bootstrap(&bootstrap).await;
        Ok(json!({
            "ok": true,
            "sessionId": bootstrap.state.session_id,
        }))
    }

    /// Duplicate the current Pi leaf into a new session file (`position: "at"`).
    pub(super) async fn clone_current_session(&self) -> Result<Value, acp::Error> {
        let response = self
            .rpc
            .request(json!({ "type": "clone" }))
            .await
            .map_err(acp_internal)?;
        let cancelled = response
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if cancelled {
            return Ok(json!({ "cancelled": true }));
        }
        let bootstrap = self.rebind_after_session_branch().await?;
        Ok(json!({
            "cancelled": false,
            "sessionId": bootstrap.state.session_id,
            "sessionFile": bootstrap.state.session_file,
        }))
    }

    /// After Pi fork/clone replaces the runtime session file, rebind adapter state.
    pub(super) async fn rebind_after_session_branch(&self) -> Result<PiBootstrap, acp::Error> {
        let bootstrap = self.refresh().await.map_err(acp_internal)?;
        if let Some(path) = bootstrap
            .state
            .session_file
            .as_deref()
            .filter(|path| !path.is_empty())
        {
            self.state
                .borrow_mut()
                .session_paths
                .insert(bootstrap.state.session_id.clone(), PathBuf::from(path));
        }
        {
            let mut state = self.state.borrow_mut();
            let plan_path = plan_file_path(&bootstrap.state, &state.session_dir);
            state.plan_mode = load_plan_tracker(&plan_path).map_err(acp_internal)?;
        }
        self.publish_bootstrap(&bootstrap).await;
        // Fork/clone rebinds session identity; refresh bar from the new JSONL.
        self.refresh_context_usage().await;
        Ok(bootstrap)
    }

    pub(super) fn replace_bootstrap(&self, bootstrap: PiBootstrap) {
        let mut state = self.state.borrow_mut();
        let session_changed = state.acp_session_id != bootstrap.state.session_id;
        state.acp_session_id = bootstrap.state.session_id.clone();
        state.model_map = bootstrap
            .models
            .iter()
            .cloned()
            .map(|model| (model_key(&model), model))
            .collect();
        // A session change invalidates the previous session's isolated queue.
        // Dropping completion senders resolves waiting ACP requests as cancelled.
        state.queue_mirror = QueueMirror::default();
        state.queued_prompt_completions.clear();
        state.agent_running = false;
        state.cancelling = false;
        // Drop cached context usage so publish_bootstrap cannot re-stamp the
        // previous session's totalTokens onto a fresh AgentView (context bar).
        if session_changed {
            state.entry_replay_cache = PiEntryReplayCache::default();
            state.last_context_tokens = None;
            state.turn_start_ms = None;
            state.stream_start_ms = None;
            state.live_prompt_id = None;
            state.bash_stream_output.clear();
        }
        state.bootstrap = bootstrap;
    }

    pub(super) fn session_id(&self) -> acp::SessionId {
        acp::SessionId::new(self.state.borrow().acp_session_id.clone())
    }

    /// ACP session modes advertised on new/load session responses.
    ///
    /// Pager plan-mode UI is driven by `modes` + `CurrentModeUpdate`, not by
    /// initialize-time agent capabilities. Mirror Grok's default/plan pair so
    /// Shift+Tab / F2 plan toggle can reach the adapter.
    pub(super) fn acp_session_modes(&self) -> acp::SessionModeState {
        let current = {
            let state = self.state.borrow();
            match state.plan_mode.state() {
                crate::plan_mode::PiPlanState::Pending | crate::plan_mode::PiPlanState::Active => {
                    "plan"
                }
                crate::plan_mode::PiPlanState::ExitPending
                | crate::plan_mode::PiPlanState::Inactive => "default",
            }
        };
        acp::SessionModeState::new(
            acp::SessionModeId::new(current),
            vec![
                acp::SessionMode::new(acp::SessionModeId::new("default"), "Agent"),
                acp::SessionMode::new(acp::SessionModeId::new("plan"), "Plan Mode"),
            ],
        )
    }

    /// Atomically publish the plan gate inputs to the injected Pi extension.
    ///
    /// The adapter is the sole writer, and this method has no await point, so
    /// no two adapter tasks can interleave writes. Rename makes readers observe
    /// either the prior complete JSON document or the next complete document.
    pub(super) fn sync_plan_mode_control(&self) -> Result<()> {
        let (control_path, active, plan_file_path) = {
            let state = self.state.borrow();
            let Some(control_path) = state.plan_mode_control.clone() else {
                return Ok(());
            };
            (
                control_path,
                state.plan_mode.is_active(),
                state.plan_mode.plan_file_path().display().to_string(),
            )
        };
        let body = serde_json::to_vec(&json!({
            "active": active,
            "planFilePath": plan_file_path,
        }))?;
        atomic_write(&control_path, &body)
    }

    /// Notify the Pager of the session plan file path so `/view-plan` and the
    /// plan preview overlay can locate the Pi-owned sidecar.
    pub(super) async fn publish_plan_file_path(&self) {
        let plan_path = self
            .state
            .borrow()
            .plan_mode
            .plan_file_path()
            .display()
            .to_string();
        self.send_ext_notification("pi/ui/plan_file", json!({ "planFilePath": plan_path }))
            .await;
    }

    /// Persist the tracker after every durable state transition. The data is
    /// private to the Pi session sidecar; the Pi core remains unaware of it.
    pub(super) fn persist_plan_mode_state(&self) -> Result<()> {
        let (path, snapshot) = {
            let state = self.state.borrow();
            (
                plan_state_path(state.plan_mode.plan_file_path()),
                state.plan_mode.snapshot(),
            )
        };
        let body = serde_json::to_vec(&snapshot)?;
        atomic_write(&path, &body)
    }
}
