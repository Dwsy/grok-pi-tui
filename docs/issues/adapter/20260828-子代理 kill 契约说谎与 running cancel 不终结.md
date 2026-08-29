---
id: "2026-08-28-subagent-kill-contract"
title: "子代理 kill 契约说谎与 running cancel 不终结"
status: "todo"
created: "2026-08-28"
updated: "2026-08-28"
category: "adapter"
tags: ["subagents", "cancel", "v1", "v2", "contract"]
---

# Issue: 子代理 kill 契约说谎与 running cancel 不终结

## Goal

V1 `kill_command_or_subagent` 与 V2 `team_interrupt` 对「取消中 / 已取消」口径一致：工具文案不把请求当成终态；running cancel 要么等到 `finished`，要么明确报告 `Cancel requested`；`cancelRequested` 之后禁止再 `prompt`。

## Problem

2026-08-28 在 grok-pi 会话中对 V1 子代理做冒烟：

1. 后台 spawn 长任务 `bb18eb64`。
2. `kill_command_or_subagent` 立刻返回 `Cancelled subagent bb18eb64 (...)`。
3. `list_subagents` 仍为 `[RUNNING]`，孩子继续跑约 2 分钟（7→17 回合）。
4. 第二次 kill + `steer STOP NOW` 后才变成 `[CANCELLED]`。孩子自己的终稿写的是 `KILLED_ACK`（服从 steer 文案），不是 abort 生效。

源码同一条路径，不是偶发误判。

## Complexity

L2：改动集中在 `extensions/pi-grok-subagents/`（`tools-v1.ts`、`runtime.ts`、`v2.ts`、测试）。不改 adapter 投影协议；Pager 已约定 cancel RPC 成功 ≠ UI 终态。

## Contract

- 模型可见文案不得在 `record.finished === false` 时使用过去式 `Cancelled`。
- running cancel 必须 `await session.abort()`（Pi `AgentSession.abort()` 为 async，会 `waitForIdle()`），或在 abort 未完成时只报告 `Cancel requested`。
- `sendMessage`、maxTurns `turn_end` 摘要、`send_message_to_subagent`、`/subagent-message`：`finished || cancelRequested` 时拒绝新 `prompt`。
- 排队未启动的 cancel 保持现状：立刻 `finish(..., "cancelled")`。
- V2 `team_interrupt` 已用 `Interrupt requested` + `finished: false`，V1 向它对齐，不要把 V2 改成 V1 那种假完成。
- adapter / `__pi_grok_subagent_cancel` 仍不伪造 `finished`；UI 终态继续等 `finished{status:"cancelled"}`。

## Acceptance Criteria

- [ ] WHEN 对 **queued** 子代理调用 kill，系统 SHALL 立刻 `finish(cancelled)`，文案可为 Cancelled，`details.finished: true`。
- [ ] WHEN 对 **running** 子代理调用 kill，系统 SHALL 不在 `prompt()` 仍未返回时声称 Cancelled。允许两种实现之一：
  - A. `await abort()` 且 `record.finished === true` 后再返回 Cancelled，`details.finished: true`；
  - B. 立即返回 `Cancel requested`，`details.finished: false`，与 V2 `team_interrupt` 同口径。
- [ ] IF `cancelRequested === true` OR `finished === true`，THEN `sendMessage` / maxTurns steer 摘要 / `send_message_to_subagent` SHALL 拒绝新 `session.prompt()`。
- [ ] WHERE `list_subagents` / `get_command_or_subagent_output`，系统 SHALL 继续只按 `record.finished` 显示 RUNNING vs CANCELLED（这两条已经诚实，不要改坏）。
- [ ] 回归：queued cancel 现有测试保持绿；新增 running-cancel 测试：mock `abort()` 不结束 `prompt` 时，kill 不得报告已死；`cancelRequested` 时 `sendMessage` 必须 no-op 或抛错。
- [ ] 不把「Pi/Gemini abort 是否空操作」扩进本 Issue。若 A 方案落地后 abort 仍等不来 idle，另开 Issue 追 Pi `AgentSession.abort()` / provider AbortSignal。

## 实施阶段

### Phase 1: 收敛契约

- [ ] 选定 A（等 idle）或 B（请求语义）。默认 **B 最小**，与 V2 一致；若产品要求 kill 阻塞到死再用 A。
- [ ] 改 `kill_command_or_subagent` 描述与返回文案，去掉「marked as cancelled」这种未发生的承诺。
- [ ] `runningSubagent()` / `sendMessage()` 增加 `cancelRequested` 守卫。

### Phase 2: runtime

- [ ] `cancel()` running 分支：至少 `void` → 明确 fire-and-forget 且文案匹配，或改为 async 并 await abort。
- [ ] `subscribeRecord` 的 maxTurns `turn_end` 摘要：`cancelRequested` 时 return。
- [ ] `__pi_grok_subagent_cancel` 与 `shutdown()` 不把假完成塞进 bridge（保持 adapter 文档：不伪造 finished）。

### Phase 3: 验证

- [ ] 现有 `runtime.test.ts` queued / pre-cancelled 用例仍过。
- [ ] 新增 running-cancel 与 sendMessage-after-cancel 单测。
- [ ] 手工冒烟：spawn 长任务 → kill → 立即 `list_subagents`；不得再出现「工具说 Cancelled、列表仍 RUNNING 且回合继续涨」。

### Phase 4: 交付

- [ ] 按需更新 `docs/FEATURE_MATRIX.md` / `.zh-CN.md` 子代理 cancel 一句。
- [ ] 创建 PR 文档并关联本 Issue。

## 关键决策

| 决策 | 理由 |
|------|------|
| 一个 Issue 覆盖 V1 kill + sendMessage 复活 + maxTurns 复活 | 同一 `runtime.cancel()` / `session.prompt()` 洞，拆开会重复修 |
| 默认修复 B（Cancel requested），不先上 A | 最短 diff；V2 已是该语义；A 依赖 Pi abort 真正让 loop idle |
| 不在本 Issue 修 Pi 核心 abort | 现场不能单独证明 provider 忽略 AbortSignal；先修 grok-pi 契约谎 |

## 根因（源码锚点）

| 文件 | 行为 |
|------|------|
| `extensions/pi-grok-subagents/tools-v1.ts` `kill_command_or_subagent` | `runtime.cancel()` 后立刻 `Cancelled`，`details.finished: false` |
| `extensions/pi-grok-subagents/runtime.ts` `cancel()` | queued 立刻 finish；running 只 `record.session.abort()` |
| `extensions/pi-grok-subagents/runtime.ts` `sendMessage()` | 不看 `cancelRequested` / `finished` |
| `extensions/pi-grok-subagents/runtime.ts` `subscribeRecord` | `turn_end` 可能再 steer 摘要 prompt |
| `extensions/pi-grok-subagents/v2.ts` `team_interrupt` | 已诚实：`Interrupt requested` + `finished: false` |
| `docs/issues/adapter/20260718-适配Pi原生Grok子代理.md` | adapter 约定：cancel RPC 成功不伪造完成 |
| `docs/FEATURE_MATRIX.md` | model-driven E2E 仍 pending |

`discard()` 是相反的谎：abort 后立刻 `finish()`，UI 可能 cancelled 而 session 仍活。本 Issue 不强制改 discard，只在 Notes 标记。

## Rollback

还原 `tools-v1.ts` / `runtime.ts` / 测试三处；无协议/sidecar 变更，无数据迁移。

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-08-28 | 第一次 kill 返回 Cancelled，list 仍 RUNNING ~2min | 未修；第二次 kill + steer STOP 才 CANCELLED。本 Issue 跟踪 |

## 相关资源

- 设计：`docs/issues/adapter/20260718-适配Pi原生Grok子代理.md`（取消节）
- 传输：`docs/issues/adapter/20260823-子代理无污染实时传输与恢复.md`
- V2：`docs/issues/adapter/20260822-grok-pi Subagents V2 Team 协作.md`
- 矩阵：`docs/FEATURE_MATRIX.md` / `docs/FEATURE_MATRIX.zh-CN.md`

## Notes

- 冒烟 ID：`45d49098` ping 成功；`1ffd2c2b` explore 成功；`bb18eb64` kill 假死；`09d3ee79` steer `STEER_OK` 成功。
- 现场模型：Gemini 3 Flash。不把 provider abort 行为写进验收。
- 测试缺口：`runtime.test.ts` 只覆盖 queued cancel 与 pre-cancelled skip。

## Progress

- **2026-08-28**：现场冒烟 + 源码对照，确认 P0 为工具契约说谎、running cancel 不等待、取消后仍可 sendMessage。Issue 开立，未改代码。

---

## Status 更新日志

- **[2026-08-28]**: 状态 → todo。从 grok-pi 会话「测试子代理工具」审查转入项目 Issue。
