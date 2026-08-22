# ADR: Subagents V2 Team 协作协议

- Date: 2026-08-22
- Status: Accepted
- Scope: `extensions/pi-grok-subagents`, grok-pi embedded extension bundle

## Context

现有 grok-pi V1 子代理已经拥有独立 Pi child session、外置 agent Markdown 定义、后台运行、wait/cancel/history 与 parent→child steer/follow-up，但缺少稳定 agent identity、child→parent / child→child 协作、嵌套 spawn 和可复用 team preset。

此前 `pi-grok-subagent/v1` 曾把 progress / child delta 高频写入 parent JSONL，证明 UI transport 不能承担 agent communication。V2 必须把**模型可见的语义消息**和**TUI-only lifecycle/state**分开。

Codex MultiAgentV2 提供了值得复用的语义：稳定 `/root/...` path、spawn、queue-only message、triggering follow-up、wait、interrupt、list，以及 role/config 分层。Pi SDK 则提供 child `AgentSession`, `customTools`, `sendCustomMessage`, steer/followUp queue 和 extension API，足够在官方 API 内实现，无需修改 Pi Core。

## Decision

### 1. V1 与 V2 并存

V1 保持兼容；V2 由 `PI_GROK_SUBAGENTS_V2=1` 显式开启。不开启时不注册 V2 tool surface。

### 2. Stable agent path + per-run native identity

Root 固定 `/root`。每个 V2 spawn 使用 lowercase/digit/underscore task name 形成 child path；nested spawn 追加 segment。`/root/...` path 是 V2 的稳定寻址键。

完成后的 agent 进入 `IDLE`。后续 follow-up **复用同一个 Pi child session/history**，但创建新的 V1 subagent UUID 作为新的 native run identity。原因是 Pager/headless 对已完成的 subagent ID 有 tombstone，不能复活同一个 run ID。稳定 team identity 与 native lifecycle identity 因此明确分层。

### 3. In-process coordinator, not JSONL polling

同一 grok-pi 进程内的 V2 agents 共享 coordinator registry。消息直接投递到目标 Pi session；`team_wait` 等待 coordinator activity promise。JSONL 仅承担 Pi 自己的 session persistence，不作为实时消息总线。

### 4. Semantic message channel

使用 `pi-grok-team-message/v2` custom message：

- `MESSAGE`: queue-only semantic message
- `NEW_TASK`: follow-up / trigger-turn semantic message
- `FINAL_ANSWER`: child terminal response automatically returned to parent

Root 通过 `pi.sendMessage` 接收；child 通过 `AgentSession.sendCustomMessage` 接收。running recipient 使用 steer/followUp delivery；idle `MESSAGE` 只追加上下文、不触发 turn；idle `NEW_TASK`/`FINAL_ANSWER` 在复用 child session 的前提下创建新的 native run 并触发 turn。`FAILED`/`CANCELLED` recipient 不被静默复活。

### 5. Same control plane in root and children

Root 用 `pi.registerTool` 暴露 V2 tools；child 用 Pi SDK `customTools` 注入同一 handler 语义。因此 child 可以 message sibling/parent，也可以 nested spawn。

V2 control tools 不属于业务 capability allowlist；agent definition 仍控制 read/bash/edit/plugin 等业务工具。这样 `explore` 仍是 read-only，但可参与团队通信。

### 6. External configuration

Agent role 配置继续使用现有：

- project `.grok-pi/agents/*.md`
- global `~/.grok-pi/agents/*.md`

Team preset 新增：

- project `.grok-pi/teams/*.json`
- global `~/.grok-pi/teams/*.json`
- bundled `extensions/pi-grok-subagents/teams/*.json`

优先级 project > global > bundled；同名高优先级定义覆盖低优先级。JSON 用于表达 members 数组，避免在 Markdown frontmatter 里发明复杂嵌套语法。

### 7. Bundled presets

提供 `research`, `implementation`, `review` 三个外置 JSON preset。preset 只定义组合和 prompt template，不实现第二套 workflow engine；真正的 DAG/脚本编排仍由现有 `xai-workflow`/Rhai 负责。

### 8. Reliability and queue semantics

V1/V2 共用后台并发上限（当前为 4）。runtime 显式区分 running / queued：queued cancel 会在执行前移除；对 queued agent 的 follow-up 不允许绕过并发上限抢跑，而是在原 queued run 完成后再重新激活。

`spawn_team` 必须先成功创建完整 roster 再启动任何 member；若中途创建失败，已创建但未启动的成员会回滚，避免半个 team 残留。nested child 完成时，如果 parent 已进入 `IDLE`，`FINAL_ANSWER` 会重新激活 parent，而不是因 parent terminal state 丢消息。

Runtime metrics/maxTurns 订阅必须绑定最终 canonical `SubagentRecord`，不能更新临时 spread 前对象；该约束有独立回归测试。

## Alternatives rejected

### Reuse `pi-grok-subagent/v1` bridge as message bus

拒绝。它是 parent TUI lifecycle seam，历史上高频 progress 已造成 JSONL 膨胀；语义通信必须走模型上下文通道。

### Implement V2 inside Pi Core / private RPC

拒绝。Pi SDK 已有 `customTools`, `AgentSession`, `sendCustomMessage`; 修改上游会扩大维护面并破坏官方 API 边界。

### Encode team presets as Rhai workflows

拒绝作为默认方案。Workflow 适合确定性 orchestration；Team V2 的重点是长期 agent identity 和 peer messaging。二者应互补：workflow 可以 spawn agents，team preset 负责协作拓扑。

### Hard-code team presets in TypeScript

拒绝。用户要求配置外置；bundled presets 也以 JSON 文件随 extension bundle 分发，project/global 可覆盖。

## Consequences

- `index.ts` 需要先拆分，避免继续增长超长文件。
- grok-pi Rust injector 需要把新增 TS 和 bundled JSON 一起物化到临时 extension bundle。
- V2 message 会进入对应 recipient session JSONL，因为它是模型可见的语义上下文；但数量由显式协作动作决定，不再按 token/progress 高频增长。
- 一个稳定 agent path 可能对应多个历史 V1 run UUID，但这些 run 共享同一个 Pi child session；Pager lifecycle 与 V2 addressing 不再混用同一 identity。
- 旧 grok-pi 读取含 V2 custom message 的 session 时应保持容错；V1 state schema 不变。
- 产品级使用说明与排障契约见 `docs/usage/subagents-v2.md` / `docs/usage/subagents-v2.zh-CN.md`。

## Rollback

首选运行时 rollback：关闭 `PI_GROK_SUBAGENTS_V2`。若需要代码 rollback，只移除 V2 registration/coordinator/team loader/bundled presets；V1 runtime 与 `pi-grok-subagent-state/v1` 无需迁移。
