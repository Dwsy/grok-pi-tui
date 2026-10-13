//! `SessionActor` methods that start, abort, and commit the per-turn dashboard summary.
//!
//! Pure prompt helpers live in [`crate::session::helpers::turn_summary`].

use super::*;

impl SessionActor {
    /// Any generation still running is aborted: its result would describe an older turn.
    /// Cancellation can only land before that block, never inside it.
    /// Generation is also checked immediately before commit, so a task that finishes after abort cannot write a stale summary.
    pub(crate) fn restart_turn_summary(self: &Arc<Self>, prompt_id: String) {
        if !self.turn_summary_enabled || self.startup_hints.is_subagent {
            return;
        }
        // A queued follow-up promoted by `maybe_start_running_task` is already running when this fires from the completion arm
        // A snapshot taken now would contain that turn's user message, so bail; the running turn's own completion re-fires
        if self
            .current_prompt_id
            .lock()
            .expect("current_prompt_id mutex poisoned")
            .is_some()
        {
            return;
        }
        self.abort_turn_summary();
        let generation = self.turn_summary_generation.get().wrapping_add(1);
        self.turn_summary_generation.set(generation);
        let actor = self.clone();
        let task = tokio::task::spawn_local(async move {
            actor.generate_turn_summary(&prompt_id, generation).await;
            // Drop the slot only if we are still the registered task
            // An abort-and-respawn can replace the handle before we finish
            if actor.turn_summary_generation.get() == generation {
                *actor.turn_summary_task.borrow_mut() = None;
            }
        });
        *self.turn_summary_task.borrow_mut() = Some(task);
    }

    /// Abort a running turn-summary generation.
    /// Callers: real prompt accept ([`Self::invalidate_side_calls_for_new_prompt`]), conversation rewind, and session shutdown.
    /// Cancel is not one of them: a running summary describes a prior successful turn, so it finishes and shows until the next one replaces it.
    pub(crate) fn abort_turn_summary(&self) {
        // Invalidate so a finishing aborted task cannot clear a later spawn or pass the pre-commit generation gate.
        self.turn_summary_generation
            .set(self.turn_summary_generation.get().wrapping_add(1));
        if let Some(task) = self.turn_summary_task.borrow_mut().take() {
            task.abort();
        }
    }

    /// The turn-summary side-call body: one small tool-free model call over the last turn, then persist to `summary.json` and broadcast transiently to clients.
    /// Display-only and best-effort: failures log and drop, the turn is already over.
    /// `generation` is the spawn-time token; if it no longer matches at commit time, this result is stale and is dropped.
    async fn generate_turn_summary(&self, prompt_id: &str, generation: u64) {
        use crate::session::helpers::turn_summary;

        let settings = crate::util::config::resolve_turn_summary_settings_from_disk();
        let Some((user_text, reply)) = turn_summary::last_turn(
            &self.chat_state_handle.get_conversation().await,
            settings.user_message_max_chars,
            settings.agent_reply_max_chars,
        ) else {
            return;
        };

        self.refresh_token_if_expired().await;
        let session_config = self.reconstruct_full_config().await;
        // Resolve the helper's own endpoint and credentials; the session endpoint may not serve it.
        let aux_config = if self.models_manager.model_in_catalog(&settings.model) {
            self.resolve_aux_sampler_config(&settings.model).await
        } else {
            None
        };
        let mut config = match aux_config {
            Some(mut cfg) => {
                crate::agent::config::stamp_session_local_sampler_fields(
                    &mut cfg,
                    &session_config,
                    self.client_identifier.clone(),
                    session_config.max_retries,
                );
                cfg
            }
            None => session_config,
        };
        if self
            .models_manager
            .model_supports_reasoning_effort_value(&config.model, settings.reasoning_effort)
        {
            self.models_manager.apply_supported_effort(
                &mut config,
                Some(settings.reasoning_effort),
                &self.session_info.id,
                crate::sampling::EffortTarget::SummaryClient,
            );
        }
        let model = config.model.clone();
        let reasoning_effort = config.reasoning_effort;
        let client = match xai_grok_sampler::SamplingClient::new(config) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "turn summary: failed to prepare sampling client");
                return;
            }
        };
        let request = ConversationRequest {
            items: vec![
                ConversationItem::system(turn_summary::TURN_SUMMARY_SYSTEM),
                ConversationItem::user(turn_summary::turn_summary_user_message(&user_text, &reply)),
            ],
            model: Some(model),
            reasoning_effort,
            x_grok_conv_id: Some(format!("turn-summary-{}", uuid::Uuid::new_v4())),
            x_grok_req_id: Some(format!("xai-turn-summary-{}", uuid::Uuid::new_v4())),
            x_grok_session_id: Some(self.session_info.id.to_string()),
            x_grok_agent_id: Some(xai_grok_telemetry::id::agent_id()),
            length_policy: xai_grok_sampling_types::LengthPolicy::Fail,
            ..Default::default()
        };

        let response = match tokio::time::timeout(
            settings.timeout,
            client.conversation_collect(request),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "turn summary: model call failed");
                return;
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = settings.timeout.as_millis() as u64,
                    "turn summary: model call timed out"
                );
                return;
            }
        };
        let summary = turn_summary::clean_turn_summary_text(&response.assistant_text());
        if summary.is_empty() {
            tracing::debug!("turn summary: model returned empty summary");
            return;
        }

        // Stale after an abort or a newer spawn: do not persist or broadcast
        if self.turn_summary_generation.get() != generation {
            tracing::debug!("turn summary: discarded stale generation");
            return;
        }

        // Commit block: no await between here and the end, so an abort can never leave the persisted and broadcast copies disagreeing
        tracing::info!(chars = summary.len(), "turn summary generated");
        let _ = self
            .notifications
            .persistence_tx
            .send(PersistenceMsg::LastTurnSummary(Some((
                summary.clone(),
                prompt_id.to_string(),
            ))));
        self.send_xai_notification_transient(
            crate::extensions::notification::SessionUpdate::LastTurnSummary {
                summary,
                prompt_id: Some(prompt_id.to_string()),
            },
        );
    }
}
