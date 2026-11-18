---
id: "20261118-proactive-plan-mode-orchestrator"
title: "Orchestrator 式主动进入/退出 Plan Mode 与 Team V2 闭环优化"
status: "done"
created: "2026-11-18"
updated: "2026-11-18"
category: "adapter"
tags: ["plan-mode", "orchestrator", "subagents-v2", "pi-grok-adapter", "rsi"]
---

# Issue: Orchestrator 式主动进入/退出 Plan Mode 与 Team V2 闭环优化

## Goal

补齐 `grok-pi` 中代理**主动进入 Plan Mode**（`enter_plan_mode`）与**同步/主动退出 Plan Mode**（`exit_plan_mode`）的双阶段闭环，修复现有 `exit_plan_mode` 异步悬空、误将 `abandoned` 当成 `approved`、忽略空反馈拒绝与误给空计划文件注入 YAML frontmatter 等缺陷，并将 Plan Mode 只读门控（`capability_mode: "read-only"` 放行）与 `Re-plan if:` / `write_scope` 协议接入 Subagents V2 编排闭环。

## 背景与根因

1. **缺少 Agent-Initiated Entry（`enter_plan_mode` 缺失）**：
   - 原生 `xai-grok-pager` 已经支持 `enter_plan_mode` 工具块识别（`RenderBlock::is_plan_mode_tool`）与无 `plan_mode_pending` 时的 `PlanModeTransition::EnteredByAgent`（`SessionEvent::PlanModeEnteredByAgent`），但 `extensions/pi-grok-plan-mode/index.ts` 与 `PiPlanTracker`（`crates/codegen/pi-grok-adapter/src/plan_mode.rs`）此前仅支持用户侧 `/plan` / `/plan-mode` / `Ctrl+Shift+T` 切换。
2. **`exit_plan_mode` 异步握手失效与状态语义缺陷**：
   - `extensions/pi-grok-plan-mode/index.ts` 立即返回 `"Plan submitted for user approval."`，未等待宿主完成 `x.ai/exit_plan_mode` 审批；同时 `pi_adapter/tools.rs` 在 `handle_tool_start` 中直接 `.await request_plan_approval(...)` 阻塞了 RPC 事件主循环。
   - `tools.rs` 原本将 `outcome == "abandoned"` 并入 `approved = true`，错误通知模型“方案已批准、可以开始修改文件”。
   - 用户未输入修改意见直接拒绝（`outcome == "cancelled"` 且 `feedback == None`）时，未向模型回传结构化修订反馈。
   - `normalize_plan_document` 在计划文件缺失时报错，且会给空白文件写入 YAML frontmatter，导致空白计划被误判为有内容。
3. **缺少主动进/退指引与 Plan ↔ Team V2 边界**：
   - 工具未提供 `promptSnippet` / `promptGuidelines`，且 `plan_mode.rs` 提示未约束模型必须在写完计划后同一轮主动调用 `exit_plan_mode`。
   - `tool_call` 门控仅拦截 `bash`/`edit`/`write`，未阻止 Plan Mode 下启动非只读子代理（`spawn_team_agent` / `spawn_team` / `spawn_subagent`）；同时 `teams/research.json` 与 V2 skills 存在早停丢消息与文档漂移问题。

## 架构与 IPC 契约

- **严格守住边界**：不修改 `xai-grok-pager` 与 `pi-main`；`pi-grok-adapter` 保持无终端无 UI；状态与协议归 `pi-grok-adapter` + `extensions/pi-grok-plan-mode`。
- **私有 TempDir + 原子控制文件/应答文件**：
  - `plan_mode_extension.rs` 将扩展源码与控制文件 `control.json` 置于同一个 `0700` 私有 `TempDir`（保持 `source_path()` / `control_path()` 接口不变）。
  - `sync_plan_mode_control()` 写入 `{"active": bool, "planFilePath": string, "syncResponses": true}`。
  - `handle_tool_start` 通过 `tokio::task::spawn_local` 异步处理 `enter_plan_mode` / `exit_plan_mode`（不阻塞 RPC reader），并在严格先 `persist_plan_mode_state()` + `sync_plan_mode_control()` 之后，原子写入同目录 `plan-resp-<sha256(toolCallId)>.json`，由扩展同步轮询读取并清理。
- **`tool_call` Plan 门控**：
  - `active === true` 时：阻止 `bash`；阻止目标非 `planFilePath` 的 `edit`/`write`；对 `spawn_team_agent` / `spawn_team` / `spawn_subagent`，仅当 `capability_mode === "read-only"` 时放行，否则阻止。

## 实施阶段（自主 RSI 迭代）

- [x] Phase 0: 建立 Issue 记录与 baseline allowlist 声明
- [x] Phase 1: Rust 侧私有 `TempDir`、`PiPlanTracker::activate_from_tool`、同步握手与 `abandoned`/`cancelled`/空文件修复
- [x] Phase 2: `extensions/pi-grok-plan-mode/index.ts`（`enter_plan_mode`、`exit_plan_mode`、子代理门控、主动提示词）及 `index.test.mjs` 单测
- [x] Phase 3: Team V2 (`v2.ts`、`teams/research.json`、`multi-agent-proactive`、`multitask`、`v2.test.ts`) Plan↔Execute 闭环与文档修正
- [x] Phase 4: 两轮完整 RSI 验证（非 Cargo 单测、Pi RPC 加载验证、`rustfmt --check`、`verify_native_grok.py`、`git diff --check`、自审与文档同步）

### RSI 迭代自审发现与修复记录

1. **Pass 1 发现并修复**：
   - `handle_enter_plan_mode` 在 `sync_plan_mode_control()` 失败时补齐了 `PiPlanTracker::from_snapshot_with_plan_file` 状态回滚，避免内存状态与控制文件不一致。
   - `request_plan_approval` 通过 `AdapterState::active_plan_approval_id` 隔离过期 `old.send_stale_cancel()`，防止上一轮被替代的审批响应清除新一轮 `awaiting_plan_approval`；并在 `agent_settled` 时清理悬挂审批标记。
   - 修复了 `tools.rs:413` 与 `plan_mode.rs:572` 的 `rustfmt` 换行细节，以及 `teams/research.json` 早停丢消息问题（`researcher`/`critic` 改用 `team_followup_task` 唤醒 `/root/synthesizer`）。

## 验收标准

- [x] `enter_plan_mode` 幂等激活 `PiPlanTracker`，先写入 `control.json (active: true)` 后回写应答文件，向 Pager 发出 `CurrentModeUpdate("plan")`。
- [x] `exit_plan_mode` 同步等待宿主 `x.ai/exit_plan_mode` 决议：仅 `approved` 允许实施；`abandoned` 退出 Plan Mode 且禁止实施；`cancelled` 保持 `Active` 并返回反馈。
- [x] Plan Mode 下 `spawn_team_agent`/`spawn_team`/`spawn_subagent` 仅允许 `capability_mode: "read-only"`，其余被拦截。
- [x] 扩展单测（6/6 + 83/83）、Pi 1.5.2 RPC 组合真实加载、`rustfmt --check`、`verify_native_grok.py`（0 新增回归）与 `git diff --check` 全部通过（注：严格遵循 `AGENTS.md` 规则未运行 Cargo）。
