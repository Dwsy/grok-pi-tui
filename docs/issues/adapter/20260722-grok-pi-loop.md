---
id: "2026-07-22-grok-pi-loop"
title: "grok-pi /loop 定时任务（F2 默认 off）"
status: "done"
created: "2026-07-22"
updated: "2026-07-22"
category: "adapter"
tags: ["loop", "scheduler", "pi-grok", "f2", "extension"]
---

# Issue: grok-pi /loop（Grok scheduled recurring prompt）

## Goal

在 **grok-pi（External ACP + Pi Core）** 上提供与 Grok 原生 `/loop` 对齐的**可用**定时循环：`/loop [interval] <prompt>` + `scheduler_create/delete/list` + 原生 tasks pane（`ScheduledTask*`）；**F2 开关默认关闭**。

> 与 Pi 用户扩展 `loop.ts`（until `signal_loop_success` 的 agentic loop）**不是同一功能**。本 Issue 仅对齐 Grok Build 的 interval scheduler。

## 边界（不可违反）

| 层 | 职责 |
|---|---|
| Grok Pager | 唯一 TUI；`scheduled_tasks` / tasks pane / status_blocks 零仿造 |
| Pi Core | Agent loop、会话、工具执行 |
| Injected extension | `/loop` + `scheduler_*` 工具 + 进程内 timer；写 control；`appendEntry` bridge |
| `pi-grok-adapter` | headless：bridge → `x.ai/scheduled_task_*` 通知；不跑 shell `SchedulerActor` |

**禁止：** adapter 画 UI；改 Pi 源码扩 RPC；完整复刻 shell SchedulerActor 的 durable/subagent 栈。

## 为何不能直接开 shell scheduler

上游 `/loop` 依赖 `xai-grok-tools` 的 `SchedulerActor` + shell tool runtime。grok-pi agent 是 Pi，没有 shell tool resources / subagent spawn 路径。

## MVP 范围

1. F2 `[ui].pi_loop` default **false**，`restart_required` + `external_only`
2. Extension `/loop [interval] <prompt>`：有明确 interval 时 host 侧直接建任务；否则注入与上游一致的 `loop_schedule_instruction` 让模型调 `scheduler_create`
3. Tools：`scheduler_create` / `scheduler_delete` / `scheduler_list`（session-only，不 durable）
4. Extension `setInterval` 触发 fire → `sendUserMessage(..., followUp)` + bridge
5. Adapter 将 bridge 映射为 `x.ai/scheduled_task_created|fired|deleted`（SessionNotification 形状）→ 原生 tasks pane
6. 约束对齐上游 MVP：min 60s、max 50 tasks、7 天过期；默认 `fire_immediately: true`（与 `/loop` instruction 一致）；默认 foreground（主会话 turn，不做 loop subagent）

## 切片

| ID | 内容 | 验收 |
|----|------|------|
| S0 | Issue + 边界 | 本文 |
| S1 | F2 `pi_loop` 全链路默认 off | registry assert default OFF |
| S2 | extension `/loop` + scheduler tools + timer | 插件静态 + inject |
| S3 | adapter bridge → ScheduledTask* | adapter unit test |
| S4 | grok-pi inject 接线 | binary check |
| S5 | FEATURE_MATRIX / 手测清单 | 文档回写 |

## Acceptance

- [ ] A1 F2 `pi_loop` 默认 off；开后需重启才注入 extension
- [ ] A2 开启后 `/loop` 出现在 slash catalog
- [ ] A3 `/loop 5m ping` 创建任务并出现 tasks pane scheduled 行
- [ ] A4 到期后自动 followUp 注入 prompt
- [ ] A5 `scheduler_delete` / `/loop list|stop` 可取消
- [ ] A6 adapter headless；无 Pi 源码改动

## Residual

| 项 | 说明 |
|----|------|
| Durable tasks | 跨 session 持久化 |
| Loop subagent background fires | 上游 `fire_as_loop_subagent` |
| 完整 generation/revision durability barrier | shell SchedulerVersion |
| 原生 Pager LoopCommand + tools meta 门控 | 当前用 Pi 扩展 `/loop`，避免 External builtins 门控 |

## Progress

- [x] 研究边界 + Issue
- [x] S1 F2 `pi_loop` default off
- [x] S2 extension
- [x] S3 adapter bridge
- [x] S4 inject
- [x] S5 验证 + 文档

## Verification (2026-07-22)

| Command | Result |
|---|---|
| `cargo test -p pi-grok-adapter --lib loop_host` | PASS 3 |
| `cargo test -p pi-grok-adapter --lib` | PASS 116 |
| `cargo test -p xai-grok-pager-bin --bin grok-pi loop_extension` | PASS 1 |
| `cargo test -p xai-grok-pager --lib settings::registry` | PASS 22 |
| `cargo check -p xai-grok-pager-bin --bin grok-pi` | PASS (pre-existing warnings only) |

### Handtest

1. F2 → Agent → **Pi /loop scheduler** → on → fully quit → restart `grok-pi`
2. `/loop` appears in slash menu
3. `/loop 5m check deploy` → tasks pane scheduled row
4. After interval (or immediate fire) → followUp prompt injected
5. `/loop list` / `/loop stop <id>` / `scheduler_delete`
6. F2 off → restart → `/loop` gone

## Regression fix (2026-09-03)

- Fixed native tasks-pane **X** cancellation for Pi `/loop`: Pager's `x.ai/scheduler/delete` now routes through the adapter to the injected loop extension, which clears the live timer instead of only removing the optimistic UI row.
- Regression check: hidden loop-delete bridge command removes the runtime task; `git diff --check` passes. Full extension typecheck remains blocked by the pre-existing missing `highlight.js/lib/index.js` declaration under `pi-main`.
