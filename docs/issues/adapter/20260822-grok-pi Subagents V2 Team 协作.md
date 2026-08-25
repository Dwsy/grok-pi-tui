---
id: "2026-08-22-grok-pi-subagents-v2-team"
title: "grok-pi Subagents V2 Team 协作"
status: "done"
created: "2026-08-22"
updated: "2026-08-22"
category: "adapter"
tags: ["subagents", "team", "multi-agent", "pi-grok", "codex-v2"]
---

# Issue: grok-pi Subagents V2 Team 协作

## Goal

在不破坏现有 V1 `spawn_subagent` 契约的前提下，为 grok-pi 增加一个**显式可选**的 Subagents V2 协作层：主代理与子代理、子代理与子代理之间可以用稳定 agent path 互发语义消息；可以用外置 team preset 一次启动一组预设角色；agent 与 team 配置都保持产品隔离、可覆盖、可审计。

## Complexity

**L4 / 架构改动。** 跨 Pi extension runtime、child-session tool 注入、配置发现和 grok-pi extension bundling；必须有 ADR、回滚方案、窄测与真实 extension-load 验证。

## Reference

参考 `~/Dev/AI/codex` 的 MultiAgentV2 语义，而不是复制其 Rust 实现：

- `codex-rs/core/src/session/multi_agents.rs`: root/subagent usage hints 与 V2 mode
- `codex-rs/core/src/tools/handlers/multi_agents_v2/{spawn,send_message,followup_task,wait}.rs`
- `codex-rs/protocol/src/agent_path.rs`: `/root/...` 稳定 agent path
- `codex-rs/core/src/config/agent_roles.rs`: 外置 role config 与分层覆盖

## Architecture

| 层 | 职责 |
|---|---|
| V1 runtime | 保持现有 `spawn_subagent` / wait / cancel / history / message 行为与 Grok 原生投影 |
| V2 coordinator | 维护 `/root/...` agent registry、父子关系、消息投递、activity wait、终态回传 |
| Child session | 通过 Pi SDK `customTools` 获得同一套 team control-plane tools；业务工具仍受 agent definition 限制 |
| Agent config | 继续使用 `.grok-pi/agents/*.md` 与 `~/.grok-pi/agents/*.md` |
| Team config | `.grok-pi/teams/*.json` > `~/.grok-pi/teams/*.json` > bundled presets |
| Parent JSONL | 只写模型可见的真实语义；V1 UI lifecycle/state 全部迁出到 socket + sidecar |

## V2 protocol

- Root path 固定为 `/root`；子代理 path 为 `/root/<task_name>`，嵌套子代理继续追加 path segment。
- V2 control tools: `spawn_team_agent`, `team_send_message`, `team_followup_task`, `team_wait`, `team_list`, `team_interrupt`。
- `team_send_message`：不单独触发 idle recipient turn；running recipient 走 steer queue。
- `team_followup_task`：触发/排队 recipient 新任务。
- 子代理 final answer 自动作为 `FINAL_ANSWER` 语义消息回传 parent。
- 消息使用 `pi-grok-team-message/v2` custom message；这是低频、模型可见的协作语义，不复用 `pi-grok-subagent/v1` UI bridge。
- V2 默认关闭；`PI_GROK_SUBAGENTS_V2=1` 时注册 V2 tools / team commands。V1 不受影响。

## Team preset contract

每个 JSON 文件包含：`name`, `description`, `enabled`, `instructions`, `members[]`。member 支持 `name`, `agent`, `description`, `task`, `model`, `max_turns`。`task` 支持 `{{task}}`, `{{team}}`, `{{agent_path}}`, `{{parent_path}}` 模板变量。

Bundled presets 至少提供：

1. `research`：researcher + critic + synthesizer
2. `implementation`：implementer + reviewer
3. `review`：explorer + reviewer + verifier

## Slices

- [x] S0 研究 Codex V2、Pi SDK、现有 V1/Workflow seam；建立 Issue + ADR
- [x] S1 将 699 行 `index.ts` 拆为 runtime / V1 registration / thin entrypoint，V1 行为不变
- [x] S2 实现 team JSON discovery、override、validation 与 bundled presets
- [x] S3 实现 V2 coordinator、六个 control tools、nested spawn、message/final delivery
- [x] S4 grok-pi injector 打包新增 TS/JSON；增加 `/subagent-teams` 发现入口
- [x] S5 回归测试、extension-load、Rust injector test、binary build、文档回写
- [x] S6 产品化 hardening：canonical runtime metrics、idle resume、queued cancel/follow-up、atomic team startup、nested FINAL_ANSWER、独立中英文使用指南

## Acceptance Criteria

- [x] A1 `PI_GROK_SUBAGENTS_V2` 未启用时，V1 tool surface 与行为不变。
- [x] A2 V2 root/child 使用稳定 `/root/...` path，可 list 并解析绝对 target。
- [x] A3 主→子、子→主、子→子 `team_send_message` 可达；idle recipient 不被该动作单独唤醒。
- [x] A4 `team_followup_task` 可唤醒 idle recipient，并可在 running recipient 后排队。
- [x] A5 V2 子代理可继续 `spawn_team_agent` 产生嵌套 child。
- [x] A6 子代理 final answer 自动回传 parent；不要求轮询 JSONL progress。
- [x] A7 `team_wait` 有上限 timeout，不 busy-poll；activity/terminal event 可提前唤醒。
- [x] A8 `team_interrupt` 可 abort running agent；已终止 agent 返回稳定状态。
- [x] A9 project/global/bundled team preset 按优先级发现；malformed preset 被隔离而不阻断其它 preset。
- [x] A10 bundled `research` / `implementation` / `review` 可由 `spawn_team` 启动。
- [x] A11 agent 业务 tools/models/extensions/skills/maxTurns 继续由外置 Markdown definition 控制；V2 control tools 作为独立 control plane 注入。
- [x] A12 父 session 只写模型可见语义；V1 lifecycle/state/child update 均不写 parent JSONL。
- [x] A13 Pi 能直接加载 extension；Rust injector tests、`git diff --check`、grok-pi build 通过（若全仓 build 被无关 pi-main 错误阻塞，需单独记录）。
- [x] A14 runtime turn/tool/token/maxTurns 统计更新 canonical `SubagentRecord`，不因 refactor spread copy 失真。
- [x] A15 completed V2 agent 进入 `IDLE`；follow-up 复用同一 child session/history，但生成新的 V1 run UUID，兼容 Pager terminal tombstone。
- [x] A16 queued run 可在执行前安全取消；queued agent 的 follow-up 不绕过 4-run 并发上限抢跑。
- [x] A17 `spawn_team` 中途创建失败时原子回滚已创建成员，不留下半启动 team/path 占用。
- [x] A18 nested child 的 `FINAL_ANSWER` 在 parent 已 `IDLE` 时会重新激活 parent，不丢结果；`FAILED`/`CANCELLED` parent 不静默复活。
- [x] A19 提供 `docs/usage/subagents-v2.md` 与 `docs/usage/subagents-v2.zh-CN.md`，覆盖启用/回滚、identity、tool/status、配置 schema、队列、排障与发布验收。

## Rollback

1. 运行时立即回滚：取消 `PI_GROK_SUBAGENTS_V2=1`，V1 继续工作。
2. 代码回滚：删除 V2 coordinator/team loader/bundled team files，并从 injector/entrypoint 移除注册；不需要迁移 V1 session schema。
3. 已产生的 `pi-grok-team-message/v2` custom messages 保持历史可读；旧版本将其视为普通 custom message，不影响 V1 subagent state replay。

## Status 更新日志

- **2026-08-22**: 状态 → in_progress。完成 Codex V2 / Pi SDK / 现有 V1 与 Workflow seam 研究，进入实现。
- **2026-08-22**: 状态 → done。V2 coordinator、外置 team preset、injector、文档与测试完成；V2 单测 4/4、injector 2/2、Pi source-load、`git diff --check`、直接 Rust `grok-pi` build 通过。全仓 `./build.sh` 仍被本任务无关的 pi-main 3 个既有 `TS2554` 错误阻塞。
- **2026-08-22**: 产品化 review/hardening 完成。修复 canonical runtime metrics/maxTurns、idle agent resume、queued cancellation/follow-up、partial team rollback、nested FINAL_ANSWER；稳定 path 与 native run UUID 分层。runtime + V2 回归扩展到 17/17，并新增中英文产品使用指南。
- **2026-08-22**: 二次产品 review 修复 bundled `review` preset 的 late-evidence 路由：explorer 改用 `team_followup_task`，确保 reviewer/verifier 即使已 `IDLE` 也会处理后到证据；文档明确 V2 path 仅在当前 root Pi session/extension runtime 内稳定，并记录 malformed 高优先级 preset 的 fail-closed shadow 语义。验证：Subagents 18/18、Eval V2.1 PASS、`grok-pi` 96/96、`cargo check -p xai-grok-pager-bin --bin grok-pi`、`git diff --check` 通过。
