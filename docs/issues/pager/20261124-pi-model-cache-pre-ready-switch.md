---
id: 20261124-pi-model-cache-pre-ready-switch
title: Pi 模型列表本地缓存与启动前切换模型（严格消息发送前时序）
status: done
created: 2026-11-24
area: pager
---

# 背景与问题

启动 `grok-pi` 时，Pi 后端拉取模型目录（`SessionCreated`）耗时较长。当前 `app.models` 在收到 `SessionCreated` 之前为空，导致用户无法在 Pi 加载完毕前打开 `/model`（`Ctrl+M`）或切换模型；此外：
- `Action::NextModel` 在 `session_id` 未就绪时未乐观更新当前显示模型；
- `Action::CycleThinkingLevel` 在 `session_id` 未就绪时直接返回空操作；
- `handle_session_loaded` 中 `model_switch_pending = true` 设置在 `maybe_drain_queue` 之后，若用户在恢复会话完成前切换模型并发送消息，消息可能在模型切换完成前发出。

# 方案设计

1. **模型目录本地缓存**（`crates/codegen/xai-grok-pager/src/pi_model_cache.rs`）：
   - 缓存路径：`${GROK_HOME:-~/.grok-pi}/pi-models-cache.json`（versioned JSON 直接序列化 `acp::SessionModelState`，无损保留 `provider::id`、`meta`、`reasoningEfforts`、`totalContextTokens` 等全部字段）。
   - **首次冷启动降级**：若缓存不存在，直接读取 Pi 已有配置（`~/.pi/agent/models.json` 与 `~/.pi/agent/settings.json` 中的 `defaultProvider` / `defaultModel` / `defaultThinkingLevel` / `enabledModels`）构造初始 `SessionModelState`。
   - **每次加载覆盖刷新**：每次 `SessionCreated`、`WorktreeSessionCreated`、`SessionLoaded` 收到 Pi 最新 `new_models` 后，后台异步原子写入最新缓存并替换内存态。
2. **启动前切换模型与严格消息时序**：
   - `event_loop.rs` 在 `AppView::new` 前即为外部 Pi 会话预热 `initial_models`，首次启动即可打开 `/model`、`Ctrl+M`、`NextModel`、`CycleThinkingLevel`。
   - 未收到 `session_id` 前切换模型或思考强度时，乐观更新状态栏显示并暂存至 `agent.session.deferred_model_switch`。
   - `handle_session_created`、`handle_worktree_session_created`、`handle_session_loaded` 统一在 `maybe_drain_queue` 之前将 `agent.session.model_switch_pending = true` 并将 `Effect::SwitchModel` 置前派发，确保用户排队消息严格等待 `TaskResult::SwitchModelComplete` 后再发出。
