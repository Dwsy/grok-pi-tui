---
id: "2026-07-22-grok-pi-goal"
title: "grok-pi Goal 模式（F2 默认 off）"
status: "in_progress"
created: "2026-07-22"
updated: "2026-08-15"
category: "adapter"
tags: ["goal", "pi-grok", "f2", "extension"]
---

# Issue: grok-pi Goal 模式

## Goal

在 **grok-pi（External ACP + Pi Core）** 上提供与 Grok 原生 `/goal` 对齐的**可用** goal loop：Pager 原生 `GoalUpdated` UI + `/goal` slash + `update_goal` 工具；**F2 开关默认关闭**。

## 边界（不可违反）

| 层 | 职责 |
|---|---|
| Grok Pager | 唯一 TUI；`GoalDisplayState` / `goal_detail` / status 条零仿造 |
| Pi Core | Agent loop、会话、工具执行 |
| `pi-grok-adapter` | headless GoalHost：状态机 + `x.ai/session_notification` GoalUpdated + turn-end continuation |
| Injected extension | `/goal` slash + `update_goal` tool；写 control 文件；不渲染 UI |

**禁止：** 在 adapter 画 UI；改 Pi 源码扩 RPC；把 shell `SessionActor` 当 grok-pi agent 跑完整 multi-agent classifier stack。

## 为何不能直接开 `GROK_GOAL`

上游 goal 编排在 `xai-grok-shell::SessionActor`（planner / classifier / strategist / skeptic / summarizer）。grok-pi 的 agent 是 Pi，不是 shell SessionActor，因此 **`GROK_GOAL` 对 External 路径无效**。Workflow Issue residual 已注明 *Goal driver | 另 Issue*。

## MVP 范围（legacy 路径）

对齐上游 **legacy `update_goal` 路径**（workflows 关时的简化模型），**不**首发完整 adversarial multi-skeptic：

1. F2 `[ui].pi_goal` default **false**，`restart_required` + `external_only`
2. `/goal <objective> [--budget N]` / `status` / `pause` / `resume` / `clear`
3. 模型 `update_goal`：progress / completed / blocked_reason
4. GoalHost 发 `GoalUpdated` → 原生 goal 状态条与 detail overlay
5. `agent_settled` 且 Active → follow-up continuation directive（防提前收工）
6. 无 planner/classifier/strategist 子代理（后续切片）

## 切片

| ID | 内容 | 验收 |
|----|------|------|
| S0 | Issue + 边界 | 本文 |
| S1 | F2 `pi_goal` 全链路默认 off | registry assert default OFF |
| S2 | GoalHost + GoalUpdated JSON | adapter unit test |
| S3 | extension `/goal` + `update_goal` + control file | 插件静态 + inject |
| S4 | adapter 接线 + continuation | adapter test + binary check |
| S5 | FEATURE_MATRIX / 手测清单 | 文档回写 |
| S6 | cancel idle probe successor ownership 门禁 | adapter 回归测试 + 原场景手测 |

## Acceptance

- [ ] A1 F2 `pi_goal` 默认 off；开后需重启才注入 extension
- [ ] A2 开启后 `/goal` 出现在 slash catalog
- [ ] A3 `/goal <obj>` 发 GoalUpdated，Pager 显示 goal UI
- [ ] A4 Active 时 turn 结束后自动 continuation（follow-up）
- [ ] A5 `update_goal(completed:true)` → Complete 并停止 continuation
- [ ] A6 pause/resume/clear 工作
- [ ] A7 adapter headless；无 Pi 源码改动
- [x] A8 cancel idle probe 不得清除刚启动的 Goal successor turn
- [x] A9 Goal 自动续作期间 Pager 保持 Working/Waiting；用户消息进入 follow-up，不触发 `already processing`

## Residual（后续）

| 项 | 说明 |
|----|------|
| Planner / classifier / strategist | 可复用 shell 模块 + Pi spawn backend（类似 workflow） |
| Goal driver on workflow engine | 上游 `goal_runs_on_workflow_engine` 路径 |
| Token budget hard stop | MVP 展示 budget；强执行可后补 |
| Session resume 持久化 goal | control/sidecar 跨进程恢复 |

## Progress

- [x] 研究边界 + Issue
- [x] S1 F2 `pi_goal` default off
- [x] S2 GoalHost + unit tests
- [x] S3 extension `/goal` + `update_goal`
- [x] S4 adapter bridge + continuation + grok-pi inject
- [x] S5 窄测 + cargo check 结果回写
- [x] S6 修复 cancel idle probe 覆盖 Goal successor 状态的竞态

## Regression: cancel → Goal successor 状态丢失（2026-08-15）

真实会话中，取消后的 Goal 自动续作已写入 Pi session，但 Pager/adapter 被旧的 cancel idle probe 重置为 idle；下一条用户消息因此以 bare prompt 提交，被正在处理的 Pi 拒绝为 `Agent is already processing`。

根因位于 `settle_cancelled_prompts()`：`get_state=false` 是 await 前旧取消轮次的快照，返回后未重新确认 cancellation ownership，可能覆盖 `agent_settled` 已启动的 Goal successor。修复只允许仍处于 cancelling 且尚无 successor running slot 的 probe 清状态。

## Verification (2026-07-22)

| Command | Result |
|---|---|
| `cargo test -p pi-grok-adapter --lib goal_host` | PASS 3 |
| `cargo test -p pi-grok-adapter --lib` | PASS 103 |
| `cargo test -p xai-grok-pager-bin --bin grok-pi goal_extension` | PASS 1 |
| `cargo check -p xai-grok-pager-bin --bin grok-pi` | PASS (pre-existing warnings only) |

## Verification (2026-08-15 regression)

| Command / scenario | Result |
|---|---|
| `./scripts/cargo-shared.sh test -p pi-grok-adapter --lib stale_cancellation_probe_cannot_clear_successor_goal_turn` | PASS 1 |
| `./scripts/cargo-shared.sh test -p pi-grok-adapter --lib` | PASS 161 |
| `./scripts/cargo-shared.sh test -p xai-grok-pager --lib queue_changed_still_adopts_running_prompt_not_terminal_in_replay` | PASS 1 |
| `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi` | PASS（既有 warnings） |
| `./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi` | PASS |
| 隔离 `--no-session`：`/goal` → Esc cancel → 自动 continuation → 立即发送普通消息 | PASS：底部持续 `Waiting for response…`，消息显示 `Queued`，无 `already processing` |

### Handtest (recommended)

1. F2 → Agent → **Pi goal mode** → on → fully quit → restart `grok-pi`
2. `/goal` appears in slash menu
3. `/goal Fix the flaky test in foo` → status bar goal UI
4. Model works; on turn end, continuation injects
5. `update_goal(completed:true)` → Complete; no more continuation
6. `/goal pause` / `resume` / `clear`
7. F2 off → restart → `/goal` gone
