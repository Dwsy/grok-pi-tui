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
- [x] A20 `PI_GROK_SUBAGENTS_V2=1` 时只注册 V2 team 工具面：`spawn_subagent`/`send_message_to_subagent`/`get_command_or_subagent_output`/`kill_command_or_subagent`/`list_subagents` 不注册，V1 命令与 Rust bridge 命令（`__pi_grok_subagent_replay`、`__pi_grok_subagent_cancel`）保留。
- [x] A21 `spawn_team` 除 `team`/`task` 外支持 `members` 子集、`model`/`thinking_level`/`max_turns`/`capability_mode` 统一覆盖、`instructions`、`timeout_ms`；`spawn_team_agent` 同步支持 `description`/`capability_mode`。
- [x] A22 一对多 spawn（`spawn_team`）不产生 `ParentTaskMetadata`，保留 Pager 自身 tool-call `raw_input`；启动前取消会回滚全部已创建成员，不启动任何成员。
- [x] A23 child 通过 `team_report(state, summary, question)` 声明本轮语义结果；`ready`/`blocked`/`failed` 唤醒 parent，`dropped` 静默（agent 停在 `IDLE`，`team_list` 保留 summary）；未调用的 child 保持旧的 `FINAL_ANSWER` 无条件回传。`team_report` 仅对 child 注册。
- [x] A24 `team_send_message` 对同一 sender→target 的连续消息合并为单条 steer（编号、去重），在下一个 team 动作前或 turn 结束时 flush（root 走 `turn_end`，child 走 `handleFinished`）；`team_list` 每行显示静默时长与待决问题；`blocked` report 产出 `WAITING_DECISION` 状态且仍可收消息/被 followup 重激活；`team_followup_task` 对 concurrency-queued agent 调用 `prioritizeBackground` 插队。
- [x] A25 child session 显式注入默认扩展（存在时 `~/.pi/agent/npm/node_modules/@ff-labs/pi-fff`）；`bindExtensions` 后仅追加扩展新注册的工具，不重置 session loadout，`capability_mode` 仍生效。`reactivateAgent` 清空上一轮 `report`，避免旧 `dropped` 吞掉新结果或旧 `blocked` question 被重放；`team_followup_task` 与 `deliver`/`reactivateAgent` 统一用 `canReceiveMessage`，`WAITING_DECISION` 可被重激活。`usageHint` 重写：禁止无条件 fan-out、给出 report 四态判定、消息价值判据与证据规则。
- [x] A26 `team_wait` 支持 `teams` 并行等待多个 preset、按 `timeout_ms` 时间等待，并在任一匹配 team 的首个消息/事件到达时唤醒一次；未知 preset 名前置报错；preset 成员的嵌套 child 活动计入所属 preset。root 可调用并按时间等待，child 在没有匹配的活跃 agent 时立即返回以释放执行槽。

## Rollback

1. 运行时立即回滚：取消 `PI_GROK_SUBAGENTS_V2=1`，V1 继续工作。
2. 代码回滚：删除 V2 coordinator/team loader/bundled team files，并从 injector/entrypoint 移除注册；不需要迁移 V1 session schema。
3. 已产生的 `pi-grok-team-message/v2` custom messages 保持历史可读；旧版本将其视为普通 custom message，不影响 V1 subagent state replay。

## Status 更新日志

- **2026-08-22**: 状态 → in_progress。完成 Codex V2 / Pi SDK / 现有 V1 与 Workflow seam 研究，进入实现。
- **2026-08-22**: 状态 → done。V2 coordinator、外置 team preset、injector、文档与测试完成；V2 单测 4/4、injector 2/2、Pi source-load、`git diff --check`、直接 Rust `grok-pi` build 通过。全仓 `./build.sh` 仍被本任务无关的 pi-main 3 个既有 `TS2554` 错误阻塞。
- **2026-08-22**: 产品化 review/hardening 完成。修复 canonical runtime metrics/maxTurns、idle agent resume、queued cancellation/follow-up、partial team rollback、nested FINAL_ANSWER；稳定 path 与 native run UUID 分层。runtime + V2 回归扩展到 17/17，并新增中英文产品使用指南。
- **2026-08-22**: 二次产品 review 修复 bundled `review` preset 的 late-evidence 路由：explorer 改用 `team_followup_task`，确保 reviewer/verifier 即使已 `IDLE` 也会处理后到证据；文档明确 V2 path 仅在当前 root Pi session/extension runtime 内稳定，并记录 malformed 高优先级 preset 的 fail-closed shadow 语义。验证：Subagents 18/18、Eval V2.1 PASS、`grok-pi` 96/96、`cargo check -p xai-grok-pager-bin --bin grok-pi`、`git diff --check` 通过。
- **2026-11-01**: V2 改为纯 team 工具面。`registerV1Tools` 拆为 `registerV1Commands`（3 个用户命令 + 2 个 Rust bridge 命令）与 `registerV1AgentTools`（5 个单代理工具），V2 模式只注册前者；`spawn_team` 补齐 `members`/`model`/`thinking_level`/`max_turns`/`capability_mode`/`instructions`/`timeout_ms`，`spawn_team_agent` 补齐 `description`/`capability_mode`；修复伪造 `parentToolCallId`（Pager orphan update）、取消后仍启动成员、`team_list` 不显示 preset 归属三个缺陷。Rust `spawned` 投影改为 `parentToolCallId` 可选。验证：Subagents 41/41、`cargo test -p pi-grok-adapter` 202/202、扩展 `tsgo` 无新增错误。
- **2026-11-01**: 引入 `team_report`，把 child 结束时的隐式文本回传改为显式语义声明。`ready`/`blocked`/`failed` 唤醒 parent，`dropped` 静默并保留 `team_list` 摘要；`blocked` 强制携带 `question`；root 不注册该工具。`handleFinished` 拆出 `deliverFinalAnswer`，`pendingTasks` 处理不再被 `dropped` 短路。设计参考 deepseek-ai/dsh-superagent-prototype 的 `orchestrator_report` 与 `reportRelay` 注意力路由，但去掉不适用的 `progress`（本实现里 `team_send_message` 已覆盖非唤醒消息）。验证：Subagents 48/48、扩展 `tsgo` 无新增错误。
- **2026-11-01**: 团队通信降噪与排队改进。`team_send_message` 改为 per sender→target 缓冲合并（重复文本丢弃、多条编号合并），flush 点为下一个非 send 的 team 工具、`handleFinished` 与 root `turn_end`；`team_list` 增加每 agent 静默时长、待决问题与 `report`/`quietMs` details；`TeamAgentStatus` 新增 `WAITING_DECISION`（由 `blocked` report 推导，仍可收消息和被 followup 重激活）；`Runtime.prioritizeBackground` 支持 `team_followup_task` 对 concurrency-queued agent 插队。修复 `team_wait` 在 flush 期间丢失活动的竞态（改用 `enteredAt` 版本比较）。验证：Subagents 53/53、扩展 `tsgo` 无新增错误。
- **2026-11-01**: 内置技能 `multi-agent-proactive` 同步到新能力面：任务书模板要求 `team_report`；新增「并行推进」（root 派完即结束 turn）、「消息内容纪律」（删掉会不会做错）、「父级如何处置 report」（四种 state 的处置）、「状态解读」（含 `WAITING_DECISION` 与 quiet 读法）；写入纪律补 V2 共享 cwd；验收清单与反模式同步扩充。
- **2026-11-01**: 两轮注入审查后的修复。发现 child prompt 走 `systemPromptOverride`，Pi 只渲染 preamble，`promptSnippet`/`promptGuidelines` 仅对 root 生效（已写入文档并加注释，避免后续误改）。修复 `reactivateAgent` 不清 `report` 导致的两个真 bug：旧 `dropped` 静默吞掉重激活后的结果、旧 `blocked` question 被当作新问题重放。`team_followup_task` 遗漏的 `canReceiveMessage` 检查补齐。child 默认扩展注入 `@ff-labs/pi-fff`（存在时），`bindExtensions` 后只追加扩展新增工具。`usageHint` 从 8 段重写为 10 段，消除「鼓励 spawn」与 SKILL 禁止递归 fan-out 的冲突。验证：Subagents 56/56、injector 3/3、`tsgo` 无新增错误。
- **2026-11-13**: `team_wait` 升级为多 team 时间等待。新增 `teams: string[]` preset 过滤（未知名称 fail-fast）、活动按 team 记录（嵌套 child 归属于祖先 preset）、匹配时只唤醒一次；root 不再被短路，可按 `timeout_ms` 等待被 `team_followup_task` 重新激活的 team；child 保留无匹配活跃 agent 时立即返回以释放执行槽。补 4 条回归（未知 preset、嵌套 child 消息、root 空闲等待/重激活、超时与取消）。验证：扩展 suite 80 项 79 通过（唯一失败为既有 `handleMouse` fixture 问题）、`tsgo` 仅剩 3 条既有错误、`git diff --check` 通过。
