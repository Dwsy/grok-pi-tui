use std::time::{Duration, Instant};

use crate::extensions::memory::{MemoryDreamDisposition, MemoryDreamResponse};
use crate::extensions::notification::SessionUpdate;
use crate::session::SessionActor;
use crate::session::acp_session::memory_capture::resolve_memory_model_and_effort;
use crate::session::batch_dream::{
    BatchDreamLimit, BatchDreamOptions, BatchDreamReport, BatchDreamStop, ModelReply,
    run_batch_dream,
};
use xai_grok_memory::SharedV2Clock;
use xai_grok_memory::batch_dream::CatalogTier;
use xai_grok_sampling_types::{SamplingError, StopReason};
use xai_grok_telemetry::memory_telemetry::{
    MemoryV2BatchDreamEndStatus, MemoryV2BatchDreamEnded, MemoryV2BatchDreamLimit,
    MemoryV2CatalogTier, MemoryV2DreamDisposition, MemoryV2DreamLifecycle, MemoryV2FailureClass,
    MemoryV2ModelUsage,
};

const MAX_REQUEST_BYTES: usize = 480 * 1024;
const MODEL_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MODEL_CALL_ATTEMPTS: usize = 2;
const MODEL_RETRY_DELAY: Duration = Duration::from_secs(2);
const MODEL_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

impl SessionActor {
    pub(super) async fn execute_batch_v2_dream(
        &self,
        cancel: tokio_util::sync::CancellationToken,
        clock: SharedV2Clock,
    ) -> MemoryDreamResponse {
        let Some(storage) = self.memory.storage() else {
            return MemoryDreamResponse::new(MemoryDreamDisposition::Disabled);
        };
        let started_at = Instant::now();
        self.send_xai_notification(SessionUpdate::MemoryDreamQueued)
            .await;
        let sampling_client = match self.prepare_chat_completion(false).await {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(target: xai_grok_telemetry::memory_log::TARGET, error = %error, "batch Dream authentication failed");
                xai_grok_telemetry::session_ctx::log_event(MemoryV2DreamLifecycle {
                    disposition: MemoryV2DreamDisposition::Failed,
                    failure_class: Some(MemoryV2FailureClass::Model),
                    ..MemoryV2DreamLifecycle::default()
                });
                return MemoryDreamResponse::new(MemoryDreamDisposition::Failed);
            }
        };
        let sampling_config = self.chat_state_handle.get_sampling_config().await;
        let context_window: u64 = sampling_config
            .as_ref()
            .map_or(0, |config| config.context_window.get());
        let model = sampling_config
            .map(|config| config.model)
            .unwrap_or_default();
        let (model, reasoning_effort) =
            resolve_memory_model_and_effort(&self.models_manager, model);
        // About four bytes per token; keep half the window for reasoning and output.
        let max_request_bytes = usize::try_from(context_window.saturating_mul(2))
            .ok()
            .filter(|bytes| *bytes > 0)
            .map_or(MAX_REQUEST_BYTES, |bytes| bytes.min(MAX_REQUEST_BYTES));
        let controls = self.memory.v2_config;
        let options = BatchDreamOptions {
            global_dir: storage.global_dir().to_path_buf(),
            workspace_dir: storage.workspace_dir().to_path_buf(),
            owner: format!("session-{}", self.session_info.id),
            clock,
            max_run_time: Duration::from_secs(controls.batch_dream_max_run_secs),
            max_calls_per_batch: controls.batch_dream_max_calls_per_batch,
            max_batch_note_bytes: controls.batch_dream_max_batch_note_bytes,
            max_request_bytes,
        };
        let usage = std::cell::RefCell::new(MemoryV2ModelUsage::default());
        let has_started = std::cell::Cell::new(false);
        let call_index = std::cell::Cell::new(0usize);
        let sampling_retries = std::cell::Cell::new(0usize);
        let report = run_batch_dream(options, cancel, |mut request| {
            request.model = Some(model.clone());
            request.reasoning_effort = reasoning_effort;
            request.x_grok_session_id = Some(self.session_info.id.to_string());
            request.x_grok_agent_id = Some(xai_grok_telemetry::id::agent_id());
            let sampling_client = &sampling_client;
            let model = &model;
            let usage = &usage;
            let has_started = &has_started;
            let call_index = &call_index;
            let sampling_retries = &sampling_retries;
            let request_items = request.items.len();
            async move {
                if !has_started.replace(true) {
                    self.send_xai_notification(SessionUpdate::MemoryDreamStarted {
                        observation_count: 0,
                    })
                    .await;
                }
                let call_number = call_index.get() + 1;
                call_index.set(call_number);
                let call_started = Instant::now();
                let mut attempt = 0usize;
                let (response, truncated) = loop {
                    attempt += 1;
                    let mut attempt_request = request.clone();
                    if attempt > 1 {
                        attempt_request.x_grok_req_id =
                            Some(format!("xai-dream-v2-batch-{}", uuid::Uuid::new_v4()));
                    }
                    match sampling_client
                        .conversation_collect_with_idle_timeout(attempt_request, MODEL_IDLE_TIMEOUT)
                        .await
                    {
                        Ok(response) => {
                            let truncated = response.stop_reason == Some(StopReason::Length);
                            break (Some(response), truncated);
                        }
                        Err(SamplingError::MaxTokensTruncation) => break (None, true),
                        Err(error)
                            if attempt < MODEL_CALL_ATTEMPTS
                                && error.is_retryable()
                                && error.retry_after().is_none_or(|seconds| {
                                    Duration::from_secs(seconds) <= MODEL_RETRY_MAX_DELAY
                                }) =>
                        {
                            sampling_retries.set(sampling_retries.get() + 1);
                            let delay = error
                                .retry_after()
                                .map_or(MODEL_RETRY_DELAY, Duration::from_secs)
                                .max(MODEL_RETRY_DELAY);
                            tracing::warn!(target: xai_grok_telemetry::memory_log::TARGET, call = call_number, attempt, model = %model, delay_ms = delay.as_millis() as u64, error = %error, "batch Dream sampling failed; retrying");
                            tokio::time::sleep(delay).await;
                        }
                        Err(error) => {
                            tracing::warn!(target: xai_grok_telemetry::memory_log::TARGET, call = call_number, attempt, model = %model, ?reasoning_effort, latency_ms = call_started.elapsed().as_millis() as u64, error = %error, "batch Dream sampling failed");
                            return Err(BatchDreamStop::Model);
                        }
                    }
                };
                let Some(response) = response else {
                    tracing::info!(target: xai_grok_telemetry::memory_log::TARGET, call = call_number, model = %model, ?reasoning_effort, latency_ms = call_started.elapsed().as_millis() as u64, "batch Dream model call produced no text before the output limit");
                    return Ok(ModelReply {
                        text: String::new(),
                        truncated: true,
                    });
                };
                let call =
                    crate::session::memory_observation::memory_v2_model_usage(model, &response);
                tracing::info!(
                    target: xai_grok_telemetry::memory_log::TARGET,
                    call = call_number,
                    model = %model,
                    ?reasoning_effort,
                    request_items,
                    latency_ms = call_started.elapsed().as_millis() as u64,
                    stop_reason = ?response.stop_reason,
                    truncated,
                    prompt_tokens = call.prompt_tokens,
                    cached_prompt_tokens = call.cached_prompt_tokens,
                    completion_tokens = call.completion_tokens,
                    reasoning_tokens = call.reasoning_tokens,
                    "batch Dream model call"
                );
                self.memory.record_dream_usage(&call);
                add_usage(&mut usage.borrow_mut(), &call);
                Ok(ModelReply {
                    text: response.assistant_text(),
                    truncated,
                })
            }
        })
        .await;
        let usage = usage.into_inner();
        let latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        log_batch_dream_ended(&report, sampling_retries.get(), latency_ms, &usage);
        let (disposition, event_disposition, failure_class) = classify(&report);
        xai_grok_telemetry::session_ctx::log_event(MemoryV2DreamLifecycle {
            disposition: event_disposition,
            observation_count: report.notes_settled(),
            topic_change_count: report.topics_changed,
            latency_ms,
            failure_class,
            usage,
        });
        if report.notes_settled() > 0 {
            self.memory.record_dream_result(true);
        } else if report.stop == BatchDreamStop::Drained && report.batches == 0 {
            self.memory.record_dream_neutral();
        } else {
            self.memory.record_dream_result(false);
        }
        MemoryDreamResponse {
            disposition,
            observation_count: report.notes_settled(),
            topics_affected: report.topics_changed,
        }
    }
}

fn classify(
    report: &BatchDreamReport,
) -> (
    MemoryDreamDisposition,
    MemoryV2DreamDisposition,
    Option<MemoryV2FailureClass>,
) {
    match report.stop {
        BatchDreamStop::Drained if report.batches == 0 => (
            MemoryDreamDisposition::NoWork,
            MemoryV2DreamDisposition::Noop,
            None,
        ),
        BatchDreamStop::Drained if report.notes_settled() == 0 && report.notes_deferred > 0 => (
            MemoryDreamDisposition::RetryRequired,
            MemoryV2DreamDisposition::Retry,
            None,
        ),
        BatchDreamStop::Drained => (
            MemoryDreamDisposition::Completed,
            MemoryV2DreamDisposition::Committed,
            None,
        ),
        BatchDreamStop::Busy => (
            MemoryDreamDisposition::Busy,
            MemoryV2DreamDisposition::Busy,
            Some(MemoryV2FailureClass::Lease),
        ),
        BatchDreamStop::DefaultPlanPending => (
            MemoryDreamDisposition::RetryRequired,
            MemoryV2DreamDisposition::Retry,
            Some(MemoryV2FailureClass::Lease),
        ),
        BatchDreamStop::Cancelled => (
            MemoryDreamDisposition::Cancelled,
            MemoryV2DreamDisposition::Retry,
            None,
        ),
        BatchDreamStop::Timeout => (
            MemoryDreamDisposition::RetryRequired,
            MemoryV2DreamDisposition::Retry,
            Some(MemoryV2FailureClass::Timeout),
        ),
        BatchDreamStop::Model => (
            MemoryDreamDisposition::RetryRequired,
            MemoryV2DreamDisposition::Retry,
            Some(MemoryV2FailureClass::Model),
        ),
        BatchDreamStop::Storage => (
            MemoryDreamDisposition::RetryRequired,
            MemoryV2DreamDisposition::Retry,
            Some(MemoryV2FailureClass::Storage),
        ),
    }
}

fn log_batch_dream_ended(
    report: &BatchDreamReport,
    sampling_retry_count: usize,
    latency_ms: u64,
    usage: &MemoryV2ModelUsage,
) {
    let end_status = match report.stop {
        BatchDreamStop::Drained => MemoryV2BatchDreamEndStatus::Drained,
        BatchDreamStop::Busy => MemoryV2BatchDreamEndStatus::Busy,
        BatchDreamStop::DefaultPlanPending => MemoryV2BatchDreamEndStatus::DefaultPlanPending,
        BatchDreamStop::Timeout => MemoryV2BatchDreamEndStatus::Timeout,
        BatchDreamStop::Cancelled => MemoryV2BatchDreamEndStatus::Cancelled,
        BatchDreamStop::Model => MemoryV2BatchDreamEndStatus::Model,
        BatchDreamStop::Storage => MemoryV2BatchDreamEndStatus::Storage,
    };
    let catalog_tier = report.catalog_tier.map(|tier| match tier {
        CatalogTier::Full => MemoryV2CatalogTier::Full,
        CatalogTier::ShortDescriptions => MemoryV2CatalogTier::ShortDescriptions,
        CatalogTier::TitlesOnly => MemoryV2CatalogTier::TitlesOnly,
        CatalogTier::Partial => MemoryV2CatalogTier::Partial,
    });
    let limit = report.limit.map(|limit| match limit {
        BatchDreamLimit::CallsPerBatch => MemoryV2BatchDreamLimit::CallsPerBatch,
        BatchDreamLimit::RequestBytes => MemoryV2BatchDreamLimit::RequestBytes,
        BatchDreamLimit::Truncations => MemoryV2BatchDreamLimit::Truncations,
    });
    xai_grok_telemetry::session_ctx::log_event(MemoryV2BatchDreamEnded {
        end_status,
        batch_count: report.batches,
        model_call_count: report.model_calls,
        repair_count: report.repairs,
        truncation_count: report.truncations,
        sampling_retry_count,
        applied_count: report.notes_applied,
        no_change_count: report.notes_no_change,
        deferred_count: report.notes_deferred,
        unplaceable_count: report.notes_unplaceable,
        topic_change_count: report.topics_changed,
        catalog_topic_count: report.catalog_topics,
        topic_bytes: report.topic_bytes,
        largest_topic_bytes: report.largest_topic_bytes,
        catalog_tier,
        limit,
        latency_ms,
        usage: usage.clone(),
    });
}

fn add_usage(total: &mut MemoryV2ModelUsage, call: &MemoryV2ModelUsage) {
    let add = |total: Option<u32>, call: Option<u32>| {
        Some(total.unwrap_or(0).saturating_add(call.unwrap_or(0)))
    };
    total.model_id.clone_from(&call.model_id);
    total.prompt_tokens = add(total.prompt_tokens, call.prompt_tokens);
    total.completion_tokens = add(total.completion_tokens, call.completion_tokens);
    total.reasoning_tokens = add(total.reasoning_tokens, call.reasoning_tokens);
    total.cached_prompt_tokens = add(total.cached_prompt_tokens, call.cached_prompt_tokens);
    total.cache_creation_tokens = add(total.cache_creation_tokens, call.cache_creation_tokens);
    if let Some(cost) = call.cost_usd_ticks {
        total.cost_usd_ticks = Some(total.cost_usd_ticks.unwrap_or(0).saturating_add(cost));
    }
}
