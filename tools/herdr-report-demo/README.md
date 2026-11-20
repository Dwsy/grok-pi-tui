# Herdr lifecycle report demo

一个最小 Node.js demo，模拟 grok-pi 内置 `pi-grok-herdr` 桥接：**只做上报**，不依赖 Pi、不跑 UI。
用来验证两件事：

1. 本地 socket 的上报协议（newline-delimited JSON + ack 校验）是否工作。
2. 上报被 Herdr ack 之后，是否真的拿到 lifecycle authority。

## 运行

需要三个环境变量（Herdr pane 内会自动注入；从外部测试需手动设置）：

```sh
HERDR_ENV=1 \
HERDR_SOCKET_PATH=/Users/dengwenyu/.config/herdr/herdr.sock \
HERDR_PANE_ID=wXX:pY \
  node report.mjs <path|id> <sessionValue> <state> [message]
```

- `path` → 发送 `agent_session_path`（绝对路径）
- `id` → 发送 `agent_session_id`
- 依次上报 `pane.report_agent_session` 和 `pane.report_agent`，每条都校验 ack 的 `id` / `result` / `!error`，失败重试一次。

例：

```sh
node report.mjs path /Users/dengwenyu/.pi/agent/sessions/xxx.jsonl working "demo thinking"
node report.mjs id   01a1deba-...                                  idle
```

## 实测结论（2026-11-16）

- 两条上报都能拿到 **ack**（`exit=0`）——协议实现正确。
- 但 ack **不等于** authority。对一个有 pi 进程的坏 pane（`wNB:p13`）用真实 session path 上报后，
  `herdr agent explain` 仍是 `screen_detection_skipped=false` /
  `fallback_reason=default_known_agent_idle_fallback`，没有拿到 `full_lifecycle_hook_authority`。
- 对照：同批的 `wNB:p14` 报 path，处于 `full_lifecycle_hook_authority`；
  `pE`/`p12`/`p13` 报 ID，全部 fallback。

判断 authority 的正确方式（而不是看 socket 返回）：

```sh
herdr agent explain <PANE> --json
# 好：screen_detection_skipped=true && screen_detection_skip_reason=full_lifecycle_hook_authority
# 坏：screen_detection_skipped=false && fallback_reason=default_known_agent_idle_fallback
```

## 关键发现：全局官方扩展 vs 内置复刻扩展

用户所指的"全局家目录下的 herdr 继承插件"是
`~/.pi/agent/extensions/herdr-agent-state.ts`——herdr 官方安装/管理的 stock-Pi 集成
（`HERDR_INTEGRATION_ID=pi`, VERSION=9）。在 `~/.pi/agent/settings.json` 里以
`+extensions/herdr-agent-state.ts` 显式启用。它是 authority 稳定的**好基线**：
research 文档里的对照显示 stock pi 用它始终拿到 `full_lifecycle_hook_authority`。

grok-pi 在 `pi_herdr=true`（默认）时，会通过
`crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/herdr_extension.rs` 的
`is_managed_pi_integration()` 把这个全局文件从自动发现里**过滤掉**，换成内置复刻的
`extensions/pi-grok-herdr/index.ts`（`include_str!` 编进二进制）。

两份的关键差异（都不是简单的代码退化，而是 grok-pi RPC 架构下的设计取舍）：

| 点 | 全局官方版 | 内置复刻版 |
|---|---|---|
| TUI gate | `ctx.mode !== "tui"` → return | `ctx.hasUI !== true` → return |
| session/state 顺序 | `await reportSession()` 后再 state | 塞进 FIFO（不 await） |
| ack 校验 | 任意 socket data 当作成功 | 校验 newline JSON 的 id/result/!error |

grok-pi 内 pi 跑在 `--mode rpc`（`rpc-mode.ts:325` 里 `mode:"rpc"`），
官方版的 `mode !== "tui"` gate 会让它完全不激活；grok-pi 的 TUI 是宿主画的，
所以内置版改用 `hasUI` 是**有意为之**，不能照抄官方 gate（照抄就没人上报了）。

## 未决

- `p13` 的 session 文件已在磁盘上（03:45Z 创建，pane 13:47 启动），但桥接启动时仍报 ID，
  "session 是否落盘"不是 path/ID 选择的唯一因素。
- 同为今天启动的新 binary，p14 报 path（authority 成立）、p13/p12 报 ID（fallback），
  差异来源未定。外部 socket 上报（demo / CLI，含超大 seq）无法改变既有 pane 的 authority，
  说明 Herdr 对 pane 内进程的权威状态有归属校验，必须从 pane 内上报侧解释。
