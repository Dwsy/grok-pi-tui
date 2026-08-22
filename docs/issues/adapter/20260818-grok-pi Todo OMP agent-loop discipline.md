---
id: "2026-08-18-grok-pi Todo OMP agent-loop discipline"
title: "grok-pi Todo OMP agent-loop discipline"
status: "done"
created: "2026-08-18"
updated: "2026-08-19"
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

## 2026-08-19 回归修复：mid-run nudge 不得切断 tool batch

真实 `provider-payload.jsonl` 已确认失败请求包含 76 个唯一 `function_call`，但有 83 个 `function_call_output`；7 个额外 output 来自两次 mid-run nudge（5 + 2 个并行 tool calls）。第一处第二次 output 从 Responses `input[74]` 转为 gprivider `messages.52`，可 1:1 复现 `unknown_tool_call_id`。

根因是 `pi-grok-todo` 在 `tool_execution_end` 阶段调用 `sendMessage(..., { deliverAs: "steer" })`。该事件早于当前 assistant turn 的全部 `toolResult` message 完成；steer 消息进入 live context 后，Pi 消息转换把尚未闭合的 tool calls 视为被用户消息打断并合成 `No result provided`，真实结果随后再次序列化，形成重复 output。

修复只改 extension lifecycle：

- mid-run mutation 统计与 nudge 判定改到 `turn_end`；该事件发生在完整 `toolResults` 已加入 context 之后。
- `todo` 仍清零 mutation debt；successful mutating tools 仍按原集合计数。
- 每次真正发出 nudge 只扣一个 12-mutation threshold，保留超过阈值的 debt。
- `sendMessage` 仍使用 `triggerTurn: false, deliverAs: "steer"`；Pi agent-loop 在 `turn_end` 后才读取 `getSteeringMessages()`，因此提醒只能进入下一轮。
- 不修改 Pi Core、adapter 或 Pager。

验证：

- system Pi `0.84.2` 直接以 RPC 模式加载修改后的 `extensions/pi-grok-todo/index.ts`，退出码 0。
- Jiti mock harness 直接执行当前扩展源码：注册 `turn_end=true`、`tool_execution_end=false`；阈值前不提醒，第 12 次 mutation 提醒；`todo` reset 后重新累计；两次提醒均保持 `{ triggerTurn:false, deliverAs:"steer" }`。
- Pi `agent-loop.ts` 确认顺序为：tool results 写入 context → `turn_end` → `getSteeringMessages()`。
- `git diff --check` 通过，改动限定在 Todo 扩展与本 Issue 记录。

## 2026-08-22 v1/v2 版本拆分：`PI_GROK_TODO_VERSION`

本期 steering 机制（mid-run nudge、completion gate、backing bus）整体复杂度偏高，与 Grok 原生 `todo_write` 的"模型自管列表"哲学冲突。`pi-grok-todo` 拆为三个文件、按 `PI_GROK_TODO_VERSION` 切换（默认 `v1`，非法值启动即报错）：

- `index.ts`：版本解析与 wiring 入口。
- `v1.ts`（默认）：对齐 Grok 原生 `todo_write` 语义 —— `{ merge?, todos: [{ id, content?, status? }] }`，字符串 id、merge/replace 增量更新、空 content 回退 id、status-only 忘带 `merge:true` 时自动升级 merge；无任何 steering。工具名仍为 `todo`，快照写入 `details.tasks`（cancelled 不进 pane）+ 权威回放态 `details.todos`。
- `v2.ts`：保留本 Issue 全部行为（6-action、blockedBy、backing event、turn_end nudge、settled completion gate）。快照增加 `version: 2` 标记。

注入器 `grok_pi/todo_extension.rs` 从单 tempfile 改为 TempDir bundle（index/v1/v2 全量落盘），单元测试扩展为断言每个模块存在且含关键内容；`pi-grok-adapter` 的 TodoPane 投影对两版均无需改动。

### 跨版本切换兼容（同会话 v1↔v2）

回放规则统一为"**分支上最后一个 `todo` 快照生效，无论哪个版本写入**"：

- 本版形状 → 直接解析。
- 异版形状 → 一次性转换为本版状态（读时迁移，首次写回即落为本版快照，此后原生回放）。
- v2→v1：subject→content、id 字符串化、deleted 墓碑丢弃。
- v1→v2：content→subject、字符串 id 按 nextId 重新编号（1..n）、cancelled→deleted 墓碑（保持 pane/list 默认隐藏语义）。
- 旧版无标记快照仍归 v2 形状家族，v1 侧同样可从其迁移。

任意次来回切换均正确（latest-wins 不受 own-kind 陈旧快照干扰）。行为回归见 `extensions/pi-grok-todo/compat.test.mjs`（jiti 直载源码 + mock harness）：v2→v1 迁移、v1→v2 重编号与 cancelled 映射、重复切换 latest-wins、legacy 无标记双版本兼容，4/4 通过。

## Status 更新日志

- **2026-08-18**: 状态变更 → doing，完成 OMP/Grok/Pi lifecycle 对照，进入 P0 实现。
- **2026-08-18**: 状态变更 → done，P0 agent-loop discipline 完成：OMP mid-run nudge、settled completion gate、Bash/Subagent backing bus 已实现并通过行为 harness 与 grok-pi Todo 注入回归。
- **2026-08-19**: 状态变更 → in_progress。真实 provider trace 复现 `unknown_tool_call_id`：mid-run nudge 在 `tool_execution_end` 阶段以 steer 消息插入 assistant tool calls 与真实 tool results 之间，Pi 的消息转换随后为未闭合 tool calls 合成 `No result provided`，真实结果到来后形成重复 `function_call_output`。修复策略：只在 `turn_end`（完整工具批次已落地）后排队 nudge，不修改 Pi Core。
- **2026-08-19**: 状态变更 → done。Todo mid-run nudge 已迁移到 `turn_end`，system Pi load、直接源码行为 harness、agent-loop 顺序核对与 `git diff --check` 均通过。
- **2026-08-22**: 状态变更 → done。扩展拆为 `PI_GROK_TODO_VERSION=v1|v2` 双版本三文件（index/v1/v2）：v1 对齐 Grok 原生 todo_write（默认），v2 保留本期全部 steering 行为；注入器改为 TempDir bundle 并扩展回归断言；快照互不串扰，adapter/Pager 零改动。验证：tsgo 0 错误、system Pi RPC 加载 v1/v2 exit 0、`cargo-shared test -p xai-grok-pager-bin todo --bin grok-pi` 2/2。
- **2026-08-22**: 状态变更 → done。补齐同会话跨版本切换兼容：latest-snapshot-wins 回放 + 异版快照读时迁移（v2→v1 字符串化/丢墓碑；v1→v2 重编号/cancelled→deleted），`compat.test.mjs` 行为回归 4/4 通过，tsgo 与 system Pi RPC 加载复验通过。