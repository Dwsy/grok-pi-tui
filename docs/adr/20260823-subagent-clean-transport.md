# ADR: 子代理 UI 传输与父会话持久化隔离

- Date: 2026-08-23
- Status: Accepted
- Scope: `extensions/pi-grok-subagents`, `pi-grok-adapter`

## Context

子代理 transcript 是 TUI 投影状态，不应进入父模型上下文或父 session append log。当前实现同时通过 socket 发送完整 live sequence，并以 `appendEntry` 写入 lifecycle/state。两个通道无法提供统一顺序，且父 JSONL 被 UI-only 数据污染。

## Decision

1. V1、Eval v2 和 V2 的 `spawned → child_update* → finished` 全部使用同一进程私有 socket；adapter 按 NDJSON 顺序逐条处理，并等待 Pager 对每条 ACP notification 的响应后再处理下一条。
2. extension 在创建 child 或执行恢复前必须等待 socket `ready`；endpoint 缺失或连接失败时在 child 启动前失败，不创建空 UI。
3. 删除 `pi.appendEntry(BRIDGE_TYPE, ...)` 与 `pi.appendEntry(STATE_ENTRY_TYPE, ...)`，也不使用 `ui.setStatus` 复制 lifecycle。
4. 每个父 Pi session 使用独立 `<parent-session-file>.subagents.jsonl` append-only sidecar 保存低频快照；恢复由 `session/load` 完成父 replay 后显式调用隐藏 replay command，不在 extension `session_start` 抢跑。extension 在 socket 尾部发送 request-scoped `replay_complete`，adapter 收到并完成 Pager ACK 后才允许 `session/load` 返回。
5. 每个 V1/V2 run 使用唯一 Pager `childSessionId`；复用的底层 Pi `AgentSession` 另存 `agentSessionId`，并以 start/end leaf 边界限定该 run 的恢复 transcript。
6. child transcript 保持在 Pi child session JSONL；普通 tool call/result 和 V2 `pi-grok-team-message/v2` 继续由 Pi 正常持久化。

## Consequences

- live UI 只有一个有序数据源；不再维护跨 status/socket 的 buffer、双 sequence 或终态重排。
- 父 JSONL 不再包含子代理 UI bridge/state 条目。
- 父会话文件与 sidecar 必须一起迁移才能保留子代理目录索引；child JSONL 本身仍可独立审计。
- 不迁移旧 parent JSONL bridge 条目；新版本只读取 sidecar。

## Rejected

- lifecycle 与 child delta 分走 status/socket：需要跨通道 barrier、buffer 和两套 sequence，复杂且难以证明顺序。
- lifecycle 同时写 socket 与 parent append log：仍会跨通道乱序并污染父 JSONL。
- 将 child delta 全部恢复到 `appendEntry`：重新引入高频膨胀。
- 修改 Pi Core RPC：官方 extension API 与本地 IPC 已足够，不扩大上游维护面。
