---
id: "2026-11-16-herdr-authority-id-path-session-lock"
title: "grok-pi Herdr 时好时坏：session 引用被 ID 锁死，path 无法接管"
status: "fix-implemented-runtime-pending"
created: "2026-11-16"
updated: "2026-11-16"
category: "grok-pi"
tags: ["grok-pi", "herdr", "lifecycle", "authority", "session"]
---

# grok-pi Herdr lifecycle authority 时好时坏

## 症状

同一份 grok-pi binary、同样 `pig` 裸启动，多个 Herdr pane 的 authority 判定不一致：
部分 pane 拿到 `full_lifecycle_hook_authority`（Header 状态准），部分永远停在
`default_known_agent_idle_fallback`（扫屏幕猜状态，working/idle 显示错乱）。

判定标准（`herdr agent explain <pane> --json`）：
- 好：`screen_detection_skipped=true` + `screen_detection_skip_reason=full_lifecycle_hook_authority`
- 坏：`screen_detection_skipped=false` + `fallback_reason=default_known_agent_idle_fallback`

## 受控实验（2026-11-16，决定性证据）

用 `herdr agent start grok-pi-experiment --kind pi --pane wNH:p1 -- grok-pi --session-dir /tmp/herdr-experiment-sessions`
开全新 pane，干净启动、无 resume。观察：

| 时刻 | session 文件 | 桥接上报 | Herdr session kind | authority |
|---|---|---|---|---|
| 启动瞬间 | 即将创建 | `agent_session_id`（ID） | id | BAD |
| 跑完整一轮后 | 已落盘 66KB，含 user/assistant/toolResult | `agent_start` 再次 `reportSession()`（应报 path） | **仍是 id** | **仍 BAD** |

结论：session 落盘后 `getSessionFile()` 已有值，`agent_start` 也调用了 `reportSession()`，
但 **Herdr 没有把 session 引用从 ID 更新成 path**。同 source（`herdr:pi`）下，
Herdr 用第一次收到的 `agent_session_id` 锁死了该 pane 的 session 引用，后续 path 上报不生效。

## 被证伪的假设（不要再走）

- ❌ `bf635fde` 是回归：新 binary（11-15 22:58 构建）今天启动的 pane 同样有好有坏
  （p12 13:16 坏、p14 13:56 好）。
- ❌ session 没落盘才报 ID：受控实验里 session 已落盘且跑完一轮，session kind 仍是 ID。
- ❌ 新 pane 就好 / 旧 pane 才坏：p12、p14 都是今天的新 pane，一坏一好。
- ❌ 双扩展打架：host 通过 `herdr_extension.rs::is_managed_pi_integration()` 在
  `pi_herdr=true` 时过滤掉全局 `herdr-agent-state.ts`，不会双开。
- ❌ 外部上报能修复：demo（`tools/herdr-report-demo/`）和 `herdr pane report-agent*`
  即便用真实 path + 超大 seq，socket 返回 ack，但 `agent explain` 和 session kind 纹丝不动。
  Herdr 对 pane 内进程的权威状态有归属校验。

## 根因

`extensions/pi-grok-herdr/index.ts` 的 `session_start` 处理：

```ts
pi.on("session_start", (event, ctx) => {
  if (ctx?.hasUI !== true) return;
  rootInteractiveSession = true;
  refreshSession(ctx);              // 此时 getSessionFile() 常为 undefined
  reportSession(event?.reason);     // 于是退而报 agent_session_id
  agentWorking = ctx?.isIdle?.() === false;
  publish(true);
});
```

`session_start` 触发早于 session 文件落盘（Pi 的 `SessionManager._persist()` 要等第一条
user/assistant message 才创建文件），所以 `refreshSession()` 拿不到 path，`sessionFields()`
退回 `agent_session_id`。Herdr 用这个 ID 建立引用后就不再接受 path 覆盖 → 永远 fallback。

`p14` 之所以是 path，推测是它启动那一刻 `getSessionFile()` 恰好已有值
（具体触发方式待确认，可能是 resume / --session）。

## 官方基线对照

`~/.pi/agent/extensions/herdr-agent-state.ts`（herdr 官方 stock-Pi 集成，VERSION=9）同样
用 `getSessionFile()` 优先、无则 ID，但它的 gate 是 `ctx.mode !== "tui"`（stock pi 的
TUI mode 下 session_start 时文件通常已在）。grok-pi 跑在 `--mode rpc`，内置桥接改用
`ctx.hasUI !== true` gate 才有意义（不能照抄 mode gate，否则完全不激活）。

## 已实施：方案 A（最小）

`extensions/pi-grok-herdr/index.ts`：彻底移除 `sessionId` 及 `agent_session_id` 上报。
`sessionFields()` 现在只返回绝对路径；session 文件未落盘（`getSessionFile()` 为
undefined）时返回空对象，`reportSession()` 的既有守卫会跳过 session 上报，
`reportState()` 也不带任何 session 引用。第一次带上 session 引用时必是 path
（发生在 `agent_start`，文件已落盘），Herdr 从一开始就拿 path 建立引用，不会再锁死成 ID。

`index.test.mjs` 更新为契约测试：
- `session_start` 阶段（有 ID、无 file）断言不发送任何 session report、状态和 session
  引用都不含 `agent_session_id`、`agent_session_path`。
- `agent_start` 阶段（file 已存在）断言发送 path session report，且永不带 ID。
- resume 新 path 正常替换。
- 错误重试测试改为提供 path 以确保 session report 真的发出。

验证：`node --test extensions/pi-grok-herdr/index.test.mjs` 4/4 通过；
`node --check` 源码/测试、`git diff --check` 均通过。

## 仍待验收（需重建 grok-pi）

源码用 `include_str!` 嵌入二进制，改源码**不影响已启动 pane**。需授权构建新 grok-pi 后，
在全新 Herdr pane 启动（`pig`），确认 `herdr agent explain <pane> --json` 显示
`full_lifecycle_hook_authority` 且 session kind 为 path。按项目 AGENTS.md 未跑 Cargo。

## 备选方案 B（未采用）

`session_start` 用 ID 报一次后，`agent_start` 拿到 path 时先 `release-agent` 再重新用 path
上报，强制 Herdr 换引用。风险：release 时序 / seq 语义需验证；方案 A 更干净，故未采用。

## 复现脚本

`tools/herdr-report-demo/report.mjs` + README：可复刻"socket ack 成功但 authority 不变"。
干净复现命令见本文"受控实验"。
