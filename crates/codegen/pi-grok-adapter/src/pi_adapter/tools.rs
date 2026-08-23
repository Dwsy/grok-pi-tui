use super::*;

const EVAL_TOOL_UI_BRIDGE_TYPE: &str = "pi-grok-eval-tool/v1";

impl PiAgent {
    pub(super) async fn execute_bash(
        &self,
        command: String,
        meta: Option<&acp::Meta>,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let serial = self.allocate_operation_id();
        {
            let mut state = self.state.borrow_mut();
            if state.bash_running {
                return Err(
                    acp::Error::invalid_params().data("Pi already has a Bash command running")
                );
            }
            state.bash_running = true;
        }

        let call_id = meta
            .and_then(|meta| meta.get("promptId"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(|id| format!("pi-bash:{id}"))
            .unwrap_or_else(|| format!("pi-bash:{serial}"));
        let title = format!("$ {command}");
        self.send_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(acp::ToolCallId::new(call_id.clone()), title.clone())
                .kind(acp::ToolKind::Execute)
                .status(acp::ToolCallStatus::InProgress)
                .content(Vec::new())
                .locations(Vec::new())
                .raw_input(Some(json!({ "command": command.clone() }))),
        ))
        .await;

        let result = self
            .rpc
            .request(json!({ "type": "bash", "command": command }))
            .await;
        self.state.borrow_mut().bash_running = false;

        match result {
            Ok(result) => {
                let cancelled = result
                    .get("cancelled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let exit_code = result.get("exitCode").and_then(Value::as_i64);
                let failed = cancelled || exit_code.is_some_and(|code| code != 0);
                let output = format_bash_result(&result);
                let raw_output = bash_tool_output(&command, None, &result, failed && !cancelled);
                self.send_update(acp::SessionUpdate::ToolCallUpdate(
                    acp::ToolCallUpdate::new(
                        acp::ToolCallId::new(call_id),
                        acp::ToolCallUpdateFields::new()
                            .title(Some(title))
                            .status(Some(if failed {
                                acp::ToolCallStatus::Failed
                            } else {
                                acp::ToolCallStatus::Completed
                            }))
                            .content(Some(vec![acp::ToolCallContent::from(
                                acp::ContentBlock::Text(acp::TextContent::new(output)),
                            )]))
                            .raw_output(Some(raw_output)),
                    ),
                ))
                .await;
                Ok(acp::PromptResponse::new(if cancelled {
                    acp::StopReason::Cancelled
                } else {
                    acp::StopReason::EndTurn
                }))
            }
            Err(error) => {
                self.send_update(acp::SessionUpdate::ToolCallUpdate(
                    acp::ToolCallUpdate::new(
                        acp::ToolCallId::new(call_id),
                        acp::ToolCallUpdateFields::new()
                            .title(Some(title))
                            .status(Some(acp::ToolCallStatus::Failed))
                            .content(Some(vec![acp::ToolCallContent::from(
                                acp::ContentBlock::Text(acp::TextContent::new(error.to_string())),
                            )])),
                    ),
                ))
                .await;
                Err(acp_internal(error))
            }
        }
    }

    pub(super) async fn handle_tool_start(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let name = string(event, &["toolName", "name"]).unwrap_or("Tool");
        if self.eval_v2_only && name.eq_ignore_ascii_case("eval") {
            return;
        }
        let args = normalize_tool_raw_input(
            name,
            event.get("args").or_else(|| event.get("input")).cloned(),
        );
        if let Some(args) = args.clone() {
            self.state
                .borrow_mut()
                .tool_args
                .insert(id.to_string(), args);
        }
        let content = edit_diff_content(name, args.as_ref(), None).unwrap_or_default();
        let ask_user_args = (name == "ask_user_question")
            .then(|| args.clone())
            .flatten();
        // When the tool is Q&A we still spawn even if args is None so the control
        // file gets an error payload instead of leaving the extension polling forever.
        let open_ask_user = name == "ask_user_question";
        self.send_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), name.to_string())
                .kind(tool_kind(name))
                .status(acp::ToolCallStatus::InProgress)
                .content(content)
                .locations(Vec::new())
                .raw_input(args),
        ))
        .await;
        if name == "exit_plan_mode" {
            self.request_plan_approval(id).await;
        }
        if open_ask_user {
            let agent = self.clone();
            let tool_call_id = id.to_string();
            tokio::task::spawn_local(async move {
                agent
                    .request_ask_user_question(&tool_call_id, ask_user_args)
                    .await;
            });
        }
    }

    /// Bridge Pi extension `ask_user_question` to native Grok QuestionView.
    ///
    /// Writes `{outcome,message}` under a hashed filename in
    /// `PI_GROK_ASK_USER_DIR` so opaque provider ids stay valid on Windows.
    pub(super) async fn request_ask_user_question(&self, tool_call_id: &str, args: Option<Value>) {
        let Some(_dir) = std::env::var_os("PI_GROK_ASK_USER_DIR") else {
            write_ask_user_response(
                tool_call_id,
                json!({
                    "outcome": "error",
                    "message": "ask_user_question host control missing (enable F2 Q&A and restart).",
                }),
            );
            return;
        };
        let questions = normalize_ask_user_questions(args.as_ref());
        if questions.is_empty() {
            write_ask_user_response(
                tool_call_id,
                json!({
                    "outcome": "error",
                    "message": "ask_user_question requires at least one question with options.",
                }),
            );
            return;
        }
        let mode = if self.state.borrow().plan_mode.is_active() {
            "plan"
        } else {
            "default"
        };
        let params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id,
            "questions": questions,
            "mode": mode,
        });
        let raw = match serde_json::value::to_raw_value(&params) {
            Ok(raw) => raw,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("failed to serialize Q&A request: {error}"),
                    }),
                );
                return;
            }
        };
        let request = acp::ExtRequest::new("x.ai/ask_user_question", raw.into());
        let response = match acp_send(request, &self.client_tx).await {
            Ok(response) => response,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("Q&A request failed: {error}"),
                    }),
                );
                return;
            }
        };
        let outer: Value = match serde_json::from_str(response.0.get()) {
            Ok(value) => value,
            Err(error) => {
                write_ask_user_response(
                    tool_call_id,
                    json!({
                        "outcome": "error",
                        "message": format!("invalid Q&A response: {error}"),
                    }),
                );
                return;
            }
        };
        let result = outer.get("result").unwrap_or(&outer);
        write_ask_user_response(tool_call_id, format_ask_user_tool_result(result));
    }

    /// Bridge Pi's extension-owned `exit_plan_mode` tool to the Pager's
    /// native PlanApprovalView. The adapter remains the state authority; the
    /// extension only gives the model a real tool to request this transition.
    pub(super) async fn request_plan_approval(&self, tool_call_id: &str) {
        let plan_file_path = {
            let mut state = self.state.borrow_mut();
            if !state.plan_mode.is_active() || state.plan_mode.is_awaiting_plan_approval() {
                return;
            }
            state.plan_mode.set_awaiting_plan_approval(true);
            state.plan_mode.plan_file_path().to_path_buf()
        };
        if let Err(error) = self.persist_plan_mode_state() {
            tracing::warn!(%error, "failed to persist plan approval state");
            self.state
                .borrow_mut()
                .plan_mode
                .set_awaiting_plan_approval(false);
            return;
        }
        if let Err(error) = self.sync_plan_mode_control() {
            tracing::warn!(%error, "failed to publish plan gate before approval");
            self.state
                .borrow_mut()
                .plan_mode
                .set_awaiting_plan_approval(false);
            return;
        }
        let plan_content = std::fs::read_to_string(&plan_file_path)
            .ok()
            .filter(|content| !content.trim().is_empty());
        let params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id,
            "planContent": plan_content,
        });
        let raw = match serde_json::value::to_raw_value(&params) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize plan approval request");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let request = acp::ExtRequest::new("x.ai/exit_plan_mode", raw.into());
        let response = match acp_send(request, &self.client_tx).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "plan approval request failed");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let response_value: Value = match serde_json::from_str(response.0.get()) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "invalid plan approval response");
                self.state
                    .borrow_mut()
                    .plan_mode
                    .set_awaiting_plan_approval(false);
                return;
            }
        };
        let result = response_value.get("result").unwrap_or(&response_value);
        let outcome = result
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("cancelled");
        let feedback = result
            .get("feedback")
            .and_then(Value::as_str)
            .filter(|feedback| !feedback.trim().is_empty());
        let approved = outcome == "approved" || outcome == "abandoned";
        if approved {
            let changed = self.state.borrow_mut().plan_mode.deactivate_approved();
            if let Err(error) = self.persist_plan_mode_state() {
                tracing::warn!(%error, "failed to persist approved plan-mode exit");
            }
            if let Err(error) = self.sync_plan_mode_control() {
                tracing::warn!(%error, "failed to publish approved plan-mode exit");
            }
            if changed {
                self.send_update(acp::SessionUpdate::CurrentModeUpdate(
                    acp::CurrentModeUpdate::new(acp::SessionModeId::new("default")),
                ))
                .await;
            }
            return;
        }
        self.state
            .borrow_mut()
            .plan_mode
            .set_awaiting_plan_approval(false);
        if let Err(error) = self.persist_plan_mode_state() {
            tracing::warn!(%error, "failed to persist rejected plan approval");
        }
        if let Some(feedback) = feedback {
            let _ = self.rpc.notify(json!({
                "type": "follow_up",
                "message": format!("The user requested plan changes:\n{feedback}"),
            }));
        }
    }

    pub(super) async fn handle_tool_update(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let output = event
            .get("partialResult")
            .or_else(|| event.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let name = string(event, &["toolName", "name"]).unwrap_or_default();
        if self.eval_v2_only && name.eq_ignore_ascii_case("eval") {
            return;
        }
        let args = normalize_tool_raw_input(
            name,
            event
                .get("args")
                .or_else(|| event.get("input"))
                .cloned()
                .or_else(|| self.state.borrow().tool_args.get(id).cloned()),
        );
        if let Some(args) = args.clone() {
            self.state
                .borrow_mut()
                .tool_args
                .insert(id.to_string(), args);
        }
        // Execute streaming: stock Grok shell sends BashOutput with
        // `output_delta` so the pager appends and keeps is_running. Pi only
        // gives growing full text in partialResult; convert the growth into
        // output_delta so Run/bash cards actually breathe mid-command.
        let raw_output = if tool_kind(name) == acp::ToolKind::Execute {
            let command = args
                .as_ref()
                .and_then(|value| string(value, &["command", "cmd"]))
                .unwrap_or_default()
                .to_string();
            let description = args.as_ref().and_then(|value| {
                string(value, &["description", "task_name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
            });
            let full_text = pi_result_text(&output);
            let full_bytes = full_text.as_bytes().to_vec();
            let prev = self
                .state
                .borrow_mut()
                .bash_stream_output
                .insert(id.to_string(), full_bytes.clone())
                .unwrap_or_default();
            let delta = if full_bytes.starts_with(&prev) {
                full_bytes[prev.len()..].to_vec()
            } else {
                // Truncation / reset: send full buffer as delta after empty
                // (tracker append path only; set_execute_output on full replace).
                full_bytes.clone()
            };
            json!({
                "type": "Bash",
                "output": full_bytes,
                "output_for_prompt": full_text,
                "exit_code": 0,
                "command": command,
                "truncated": false,
                "signal": null,
                "timed_out": false,
                "description": description,
                "current_dir": "",
                "output_file": "",
                "total_bytes": full_bytes.len(),
                "was_bare_echo": false,
                "output_delta": delta,
            })
        } else {
            normalize_tool_raw_output(name, args.as_ref(), &output, false)
        };
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(acp::ToolCallStatus::InProgress))
            .raw_output(Some(raw_output));
        if tool_kind(name) != acp::ToolKind::Edit {
            fields = fields.content(Some(tool_content(&output)));
        }
        self.send_update(acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(acp::ToolCallId::new(id.to_string()), fields),
        ))
        .await;
    }

    pub(super) async fn handle_tool_end(&self, event: &Value) {
        let id = string(event, &["toolCallId", "id"]).unwrap_or("pi-tool");
        let output = event.get("result").cloned().unwrap_or(Value::Null);
        let is_error = event.get("isError").and_then(Value::as_bool) == Some(true);
        let status = if is_error {
            acp::ToolCallStatus::Failed
        } else {
            acp::ToolCallStatus::Completed
        };
        let name = string(event, &["toolName", "name"]).unwrap_or_default();
        if self.eval_v2_only && name.eq_ignore_ascii_case("eval") {
            return;
        }
        let args = normalize_tool_raw_input(
            name,
            event
                .get("args")
                .or_else(|| event.get("input"))
                .cloned()
                .or_else(|| self.state.borrow_mut().tool_args.remove(id)),
        );
        self.state.borrow_mut().bash_stream_output.remove(id);
        let raw_output = normalize_tool_raw_output(name, args.as_ref(), &output, is_error);
        let mut fields = acp::ToolCallUpdateFields::new()
            .status(Some(status))
            .raw_output(Some(raw_output));
        if tool_kind(name) == acp::ToolKind::Edit {
            fields = fields.content(edit_diff_content(name, args.as_ref(), Some(&output)));
        } else {
            fields = fields.content(Some(tool_content(&output)));
        }
        self.handle_background_bash_tool_end(name, id, args.as_ref(), &output)
            .await;
        // Goal control file is the SSOT; tool_end reloads if bridge entry lags.
        if name.eq_ignore_ascii_case("update_goal")
            && let Some(control) = self.refresh_goal_from_disk().await
        {
            self.emit_goal_updated_from_control(&control).await;
        }
        // Live path: rpiv-todo (and future TodoSource plugins) publish a full
        // task snapshot under tool result details → native TodoPane via Plan.
        if let Some(plan) = plan_update_for_tool(name, &output, is_error) {
            self.send_update(acp::SessionUpdate::Plan(plan)).await;
        }
        self.send_update(acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(acp::ToolCallId::new(id.to_string()), fields),
        ))
        .await;
    }

    /// Project Eval-v2-only nested host calls onto native ACP tool rows without
    /// adding those calls to Pi's model transcript. The extension sends these
    /// through appendEntry, so this path is display-only by construction.
    pub(super) async fn handle_eval_tool_bridge_entry(&self, event: &Value) -> bool {
        let entry = event.get("entry").unwrap_or(event);
        if entry.get("type").and_then(Value::as_str) != Some("custom")
            || entry.get("customType").and_then(Value::as_str) != Some(EVAL_TOOL_UI_BRIDGE_TYPE)
        {
            return false;
        }
        if !self.eval_v2_only {
            return true;
        }
        let Some(data) = entry.get("data").and_then(Value::as_object) else {
            return true;
        };
        if data.get("version").and_then(Value::as_u64) != Some(1) {
            return true;
        }
        let phase = data
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let projected = Value::Object(data.clone());
        match phase {
            "start" => self.handle_tool_start(&projected).await,
            "update" => self.handle_tool_update(&projected).await,
            "end" => self.handle_tool_end(&projected).await,
            _ => {}
        }
        true
    }

    pub(super) async fn handle_background_bash_bridge_message(
        &self,
        event: &Value,
    ) -> Result<bool> {
        let Some(projection) = parse_background_bash_message(event) else {
            return Ok(false);
        };
        self.project_background_bash(projection).await;
        Ok(true)
    }

    pub(super) async fn handle_background_bash_tool_end(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        args: Option<&Value>,
        result: &Value,
    ) {
        let Some(projection) =
            parse_background_bash_tool_result(tool_name, tool_call_id, args, result)
        else {
            return;
        };
        self.project_background_bash(projection).await;
    }

    /// Consume the private `__pi_grok_bash_task__` status payload.
    ///
    /// This is the channel the task UI actually converges on: unlike the bridge
    /// message, it is not queued behind streaming and is not discarded when the
    /// user aborts the turn.
    pub(super) async fn handle_background_bash_status(&self, payload: &Value) {
        let Some(projection) = parse_background_bash_status(payload) else {
            tracing::warn!("ignored malformed background Bash status payload");
            return;
        };
        self.project_background_bash(projection).await;
    }

    /// Settle every background task the adapter still mirrors as running.
    ///
    /// The shells are children of the Pi process, so its exit ended them too.
    /// Without this, the rows animate forever: the extension instance that owned
    /// them is gone and can never report their terminal state.
    pub(super) async fn settle_orphaned_background_bash(&self) {
        let orphans =
            drain_running_background_bash(&mut self.state.borrow_mut().background_bash_tasks);
        if orphans.is_empty() {
            return;
        }
        tracing::info!(
            count = orphans.len(),
            "settling background Bash tasks orphaned by the Pi process exit"
        );
        for projection in orphans {
            self.emit_background_bash(&projection).await;
        }
    }

    /// Single projection choke point for Pi-owned background Bash tasks: the
    /// mirror decides whether this transition still has news for Pager.
    async fn project_background_bash(&self, projection: BackgroundBashProjection) {
        let forward = record_background_bash(
            &mut self.state.borrow_mut().background_bash_tasks,
            &projection,
        );
        if forward {
            self.emit_background_bash(&projection).await;
        }
    }

    async fn emit_background_bash(&self, projection: &BackgroundBashProjection) {
        if let Some(output) = background_bash_output_update(projection) {
            self.send_update(acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(output["toolCallId"].as_str().unwrap_or_default()),
                    acp::ToolCallUpdateFields::new()
                        .status(Some(acp::ToolCallStatus::InProgress))
                        .raw_output(Some(output["rawOutput"].clone())),
                ),
            ))
            .await;
        }
        let session_id = self.session_id().0.to_string();
        let (method, notification) = background_bash_notification(&session_id, projection);
        self.send_ext_notification(method, notification).await;
    }

    pub(super) async fn handle_extension_ui(&self, event: Value) -> Result<()> {
        let method = string(&event, &["method"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        match method.as_str() {
            "notify" => {
                let message = string(&event, &["message"]).unwrap_or_default();
                let kind = string(&event, &["notifyType", "kind"]);
                self.send_ui_notification(message, kind).await;
            }
            "setstatus" => {
                let key = string(&event, &["statusKey", "key"]).unwrap_or("extension");
                let text = string(&event, &["statusText", "text"]);
                // A few private keys carry a JSON control payload instead of
                // status-bar text; everything else is a real status line.
                match key {
                    EXTENSION_QUEUE_STATUS_KEY => {
                        if let Some(payload) = control_status_payload(text) {
                            let message =
                                string(&payload, &["text"]).unwrap_or_default().to_string();
                            let images = payload
                                .get("images")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            let behavior = string(&payload, &["streamingBehavior", "deliverAs"]);
                            self.enqueue_extension_message(message, images, behavior)
                                .await;
                        }
                    }
                    EXTENSION_BASH_TASK_STATUS_KEY => {
                        if let Some(payload) = control_status_payload(text) {
                            self.handle_background_bash_status(&payload).await;
                        }
                    }
                    _ => {
                        self.send_status(key, text.filter(|text| !text.is_empty()))
                            .await;
                    }
                }
            }
            "setwidget" => {
                // Grok owns the sticky surface and ordering; the adapter only
                // forwards Pi's semantic widget payload.
                self.send_ext_notification("pi/ui/widget", event).await;
            }
            "settitle" => {
                if let Some(title) = string(&event, &["title"]) {
                    self.send_extension_title(title).await;
                }
            }
            "set_editor_text" | "seteditortext" => {
                if let Some(text) = string(&event, &["text"]) {
                    self.send_ext_notification("pi/ui/editor_text", json!({ "text": text }))
                        .await;
                }
            }
            // Experimental Remote TUI: frames projected from Pi-process component host.
            "remote_tui_open" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "open",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                        "title": event.get("title").cloned().unwrap_or(Value::Null),
                        "width": event.get("width").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "remote_tui_frame" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "frame",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                        "lines": event.get("lines").cloned().unwrap_or(json!([])),
                        "width": event.get("width").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "remote_tui_close" => {
                self.send_ext_notification(
                    "pi/ui/remote_tui",
                    json!({
                        "op": "close",
                        "id": event.get("id").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await;
            }
            "select" | "confirm" | "input" | "editor" => {
                let agent = self.clone();
                tokio::task::spawn_local(async move {
                    if let Err(error) = agent.ask_extension_question(event.clone()).await {
                        tracing::warn!(%error, "Pi extension question failed");
                        agent.respond_extension_cancelled(&event);
                        agent
                            .send_ui_notification(
                                &format!("Pi extension dialog failed: {error}"),
                                Some("error"),
                            )
                            .await;
                    }
                });
            }
            _ => self.respond_extension_cancelled(&event),
        }
        Ok(())
    }

    pub(super) fn respond_extension_cancelled(&self, event: &Value) {
        if let Some(id) = event.get("id") {
            let _ = self.rpc.notify(json!({
                "type": "extension_ui_response",
                "id": id,
                "cancelled": true,
            }));
        }
    }

    pub(super) async fn ask_extension_question(&self, event: Value) -> Result<()> {
        let id = event
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("Pi extension UI request has no id"))?;
        let method = string(&event, &["method"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        let raw_title = string(&event, &["title", "message"]).unwrap_or("Pi extension");
        // grok-pi's subagent configuration uses Pi's standard `ui.select`
        // callback, but requests the existing native QuestionView multi-select
        // affordance through a namespaced title envelope. Pi core remains
        // unchanged and other extension selects keep their single-choice
        // semantics.
        let multi_select_title = (method == "select")
            .then(|| extension_multi_select_title(raw_title))
            .flatten();
        let resource_picker = (method == "select")
            .then(|| extension_resource_picker(raw_title))
            .flatten();
        let resource_picker_title = resource_picker
            .as_ref()
            .and_then(|picker| picker.get("title"))
            .and_then(Value::as_str);
        let title = resource_picker_title
            .or(multi_select_title.as_deref())
            .unwrap_or(raw_title);
        let mut options = Vec::new();
        if method == "select" {
            for option in event
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                options.push(json!({
                    "label": option,
                    "description": "",
                    "preview": null,
                    "id": null,
                }));
            }
        } else if method == "confirm" {
            options.push(json!({ "label": "Yes", "description": "", "preview": null, "id": null }));
            options.push(json!({ "label": "No", "description": "", "preview": null, "id": null }));
        }
        let mut question = if method == "confirm" {
            string(&event, &["message"]).unwrap_or(title).to_string()
        } else {
            title.to_string()
        };
        if method == "input"
            && let Some(placeholder) = string(&event, &["placeholder"])
            && !placeholder.is_empty()
        {
            question.push_str("\n\n");
            question.push_str(placeholder);
        }
        let initial_text = if method == "editor" {
            string(&event, &["prefill"]).unwrap_or_default()
        } else {
            ""
        };
        let tool_call_id = extension_tool_call_id(&id);
        let mut params = json!({
            "sessionId": self.session_id().0.to_string(),
            "toolCallId": tool_call_id.clone(),
            "questions": [{
                "question": question,
                "options": options,
                "multiSelect": multi_select_title.is_some(),
                "id": "pi-question",
            }],
            "mode": "default",
            "initialText": initial_text,
            "noFreeform": method == "select" || method == "confirm",
        });
        if let Some(resource_picker) = resource_picker.clone() {
            params
                .as_object_mut()
                .expect("extension question params must be an object")
                .insert("piGrokResourcePicker".to_owned(), resource_picker);
        }
        let raw = serde_json::value::to_raw_value(&params)?;
        let request = acp::ExtRequest::new("x.ai/ask_user_question", raw.into());
        let response = match extension_dialog_timeout(&event) {
            Some(duration) => {
                match tokio::time::timeout(duration, acp_send(request, &self.client_tx)).await {
                    Ok(response) => response.map_err(|error| anyhow!(error.to_string()))?,
                    Err(_) => {
                        // Pi resolves its own dialog promise on the same timeout but
                        // does not emit a cancellation event. Explicitly retract the
                        // native Grok QuestionView so it cannot remain as a zombie
                        // overlay after the extension has resumed.
                        self.send_ext_notification(
                            "pi/ui/cancel_interaction",
                            json!({ "toolCallId": tool_call_id }),
                        )
                        .await;
                        self.respond_extension_cancelled(&event);
                        return Ok(());
                    }
                }
            }
            None => acp_send(request, &self.client_tx)
                .await
                .map_err(|error| anyhow!(error.to_string()))?,
        };
        let outer: Value = serde_json::from_str(response.0.get())?;
        let result = outer.get("result").unwrap_or(&outer);
        if result.get("outcome").and_then(Value::as_str) == Some("cancelled") {
            self.rpc.notify(json!({
                "type": "extension_ui_response",
                "id": id,
                "cancelled": true,
            }))?;
            return Ok(());
        }
        let answer = if resource_picker.is_some() {
            extension_resource_picker_answer(result).unwrap_or_else(|| "[]".to_string())
        } else if multi_select_title.is_some() {
            extension_multi_select_answer(result).unwrap_or_else(|| "[]".to_string())
        } else {
            extension_answer(&method, result).unwrap_or_default()
        };
        let response = match method.as_str() {
            "confirm" => json!({
                "type": "extension_ui_response",
                "id": id,
                "confirmed": answer.eq_ignore_ascii_case("yes"),
            }),
            _ => json!({
                "type": "extension_ui_response",
                "id": id,
                "value": answer,
            }),
        };
        self.rpc.notify(response)?;
        Ok(())
    }
}

/// Decode the JSON body of a private control status. Only the namespaced keys
/// go through here, so an ordinary status line never pays for a parse.
fn control_status_payload(text: Option<&str>) -> Option<Value> {
    serde_json::from_str(text?).ok()
}
