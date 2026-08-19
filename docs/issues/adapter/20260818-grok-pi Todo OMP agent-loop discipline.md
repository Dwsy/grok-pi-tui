---
id: "2026-08-18-grok-pi Todo OMP agent-loop discipline"
title: "grok-pi Todo OMP agent-loop discipline"
status: "done"
created: "2026-08-18"
updated: "2026-08-18"
category: "adapter"
tags: ["workhub", "grok-pi", "todo", "omp", "agent-loop"]
---

# Issue: grok-pi Todo OMP agent-loop discipline

## Goal

在不复制 OMP renderer/controller/persistence 栈、不修改 Pi Core、不让 adapter/Pager 承担 agent-loop 语义的前提下，把 Todo 从展示协议升级为轻量执行纪律：增加 OMP 式 mid-run 对账提醒，以及 Grok 式 backed-task completion gate。

## 背景/问题

现有 `pi-grok-todo` 已完成 `todo → details.tasks → ACP Plan → native TodoPane` 的数据与 UI 闭环，但 agent loop 只靠 `promptGuidelines` 自觉维护 Todo。OMP 的 `TodoTracker` 证明更有效的行为层包括：连续真实 mutation 后的隐藏对账提醒，以及 agent 停止时对未完成 Todo 的有限次 continuation。Grok 原生 TodoGate 又提供了关键的 async backing 语义：后台任务正在承接的 `in_progress` 不应被 completion gate 当作遗留工作。

本期只做 P0 行为层。更强状态机（正式 `blocked`、自动推进、依赖 cycle 检测、用户手动纠偏、canonical cache）留到后续。

## 边界

- Grok Pager 仍是唯一 Todo UI；不新增/仿造 Todo renderer。
- Pi extension 负责 agent-loop 行为；不修改 Pi Core。
- `pi-grok-adapter` 保持 headless；不接 completion gate。
- 当前 branch transcript snapshot 仍是 Todo canonical state；本期不增加第二份缓存真相源。
- 后台 backing 只通过 Pi 官方 `pi.events` extension bus 发布，不读取 adapter/Pager 私有状态。
- P0 backing 来源限定为内置 background Bash 与 background subagent。

## 验收标准 (Acceptance Criteria)

- [x] A1 WHEN 连续 12 个成功的 `bash/eval/edit/write/ast_edit` tool result 发生且中间没有 `todo` 调用，且仍有未完成 Todo，系统 SHALL 注入一次隐藏 mid-run 对账提醒。
- [x] A2 WHERE 同一用户 prompt cycle，mid-run 对账提醒 SHALL 最多触发 2 次；任意 `todo` 调用 SHALL 清零 mutation debt。
- [x] A3 WHEN Pi 到达 `agent_settled` 且存在 pending 或无后台 backing 的 `in_progress`，系统 SHALL 以隐藏 follow-up continuation 提醒 agent 继续或更新 Todo。
- [x] A4 WHERE completion reminder，系统 SHALL 每个用户 prompt cycle 最多触发 2 次，且上次 reminder 后没有任何 finalized tool result 时 SHALL 静默，避免无进展自激循环。
- [x] A5 WHEN background Bash/subagent 正在运行，系统 SHALL 按 Grok TodoGate 规则用 backing count 覆盖最早的 `in_progress`；若无 pending 且全部 `in_progress` 已 backed，completion gate SHALL 放行。
- [x] A6 WHEN assistant 明确在最后一行向用户提问/请求确认，completion gate SHALL 保守放行，不抢用户回合。
- [x] A7 WHERE Todo 状态读取，系统 SHALL 继续从 current branch 的 `toolResult.details.tasks` 重建，不引入 session-global stale cache。
- [x] A8 WHERE 实现 wiring，系统 SHALL 不修改 Pi Core、adapter agent loop 或 Pager Todo renderer。

## 实施阶段

### Phase 1: 研究与边界
- [x] 接力旧会话并确认上一阶段已完成
- [x] 对照 OMP `TodoTracker`：12 mutation、每 cycle 最多 2 次 mid-run nudge、无进展/等用户/async wake 避让
- [x] 对照 Grok TodoGate：pending 始终触发；backing count 仅覆盖最早的 `in_progress`
- [x] 确认 Pi 官方 lifecycle：completion gate 使用 `agent_settled`

### Phase 2: 执行
- [x] S1 `pi-grok-todo` 增加 per-prompt tracker、mid-run nudge 与 settled-time completion gate
- [x] S2 `pi-grok-bash` 通过 extension bus 发布 active background backing count
- [x] S3 `pi-grok-subagents` 通过 extension bus 发布 active background backing count

### Phase 3: 验证
- [x] TypeScript 语法诊断；本次新增行 source diagnostics = 0（完整 `tsc -p` 另受仓库环境缺失 `@types/node` 阻断）
- [x] Todo extension 注入/静态 Rust 回归：`cargo-shared test -p xai-grok-pager-bin todo --bin grok-pi` 2/2 通过
- [x] 行为 harness 覆盖 nudge threshold、cap、backing partition、无进展与等待用户 guard
- [x] `git diff --check` 与工作树审计

## 关键决策

| 决策 | 理由 |
|------|------|
| gate 放在 Pi extension lifecycle | `grok-pi` 使用 Pi agent，不经过 Grok Shell `SessionActor`；原生 Shell TodoGate 无法直接复用 |
| completion 时点用 `agent_settled` | `agent_end` 后 Pi 仍可能 retry/compact/follow-up，`agent_settled` 才代表不会自动继续 |
| background backing 用 `pi.events` | 官方 extension API，可保持 adapter/Pager headless 且避免私有状态耦合 |
| 保留 branch-snapshot canonical state | fork/branch 安全；P0 不为性能引入可能 stale 的第二真相源 |
| mutation 工具集对齐 OMP | 只计 `bash/eval/edit/write/ast_edit`，read-only 探索不制造 Todo 对账压力 |
| P0 不引入正式 blocked 状态 | 当前 schema 只有 `blockedBy` metadata；正式 blocker 语义属于后续状态机工作 |

## Residual / 后续

- 正式 `blocked`/`blockedReason` 与 waiting-for-external 状态
- 最多一个 `in_progress`、完成后自动推进下一 actionable pending
- dependency 完整校验与 cycle detection
- 用户手动 Todo 纠偏 UI
- branch-change hook 驱动的 canonical cache（仅在能保证不 stale 时）
- 可选 eager Todo；本期不做

## 相关资源

- `extensions/pi-grok-todo/index.ts`
- `extensions/pi-grok-bash/index.ts`
- `extensions/pi-grok-subagents/index.ts`
- `crates/codegen/xai-grok-shell/src/session/acp_session_impl/reminders.rs`
- `/Users/dengwenyu/Dev/AI/oh-my-pi/packages/coding-agent/src/session/todo-tracker.ts`
- `docs/issues/adapter/20260717-Pi rpiv-todo 映射到 Grok 原生 TodoPane.md`
- `docs/issues/adapter/20260722-grok-pi-loop.md`

## Status 更新日志

- **2026-08-18**: 状态变更 → doing，完成 OMP/Grok/Pi lifecycle 对照，进入 P0 实现。
- **2026-08-18**: 状态变更 → done，P0 agent-loop discipline 完成：OMP mid-run nudge、settled completion gate、Bash/Subagent backing bus 已实现并通过行为 harness 与 grok-pi Todo 注入回归。