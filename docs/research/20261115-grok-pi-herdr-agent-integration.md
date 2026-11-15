# grok-pi × Herdr 生命周期集成：证据与修复

> 日期：2026-11-15｜范围：仅 grok-pi 内置 Pi Extension｜Herdr 源码不变
> 状态：扩展已改、Node 回归通过；新二进制实机接管尚未验收

## 核心结论

Herdr 已收到 grok-pi 的 session 身份，但没有获得完整生命周期 authority。旧报告推断是 Pi RPC 进程名 `pi-rpc`；**这一推断已被运行现场证伪**：`herdr pane process-info --pane wNB:pE` 显示当前前台进程组存在 `argv0=pi` 的 Node 子进程。因此不设置 `process.title`，也不改 Herdr 的安全门控。

真正未证实的是状态报告是否到达、到达次序以及 Herdr 内部是否接受。原代码 `socket.on("data") => true` 无条件把任意返回当作成功；`agent_start` 的 session report 与 state report 还会并发。

## 现场事实

- Herdr 0.9.5；grok-pi 0.1.14-beta.3；`[ui].pi_herdr=true`。
- grok-pi pane `wNB:pE`：有 `herdr:pi` session 身份，`screen_detection_skipped=false`，`fallback_reason=default_known_agent_idle_fallback`。
- stock Pi pane：`screen_detection_skipped=true` 且 `screen_detection_skip_reason=full_lifecycle_hook_authority`。
- `pane process-info` 显示 grok-pi 的 foreground PGID=28841，其中一个 Node 子进程 PID=28889、`argv0=pi`。此证据不能单独证明 Herdr 内部 `detected_agent` 一定等于 Pi。
- Herdr `src/terminal/state.rs` 的 full lifecycle route 还要求有效进程、已锚定的会话、非 stale/exit suppression 以及可接受的 seq；Socket API 成功 JSON 不保证事件已进入 authority。

## 已落地的最小修复

在 `extensions/pi-grok-herdr/index.ts`：

1. Session 和 state 共用串行 FIFO；每条报告入队时固定 session ref 和单调递增 seq，确保 `session_start` 和 `agent_start` 都先送 session 再送 state。
2. 解析 newline-delimited JSON 应答，校验请求 ID、`result` 存在且无 `error`。错误/断连重试一次（保留相同 ID 和 seq），失败只输出一次诊断到 Pi RPC stderr。
3. 不阻塞 Pi 的事件处理；相邻未发出的状态可以合并，但不能越过 session 报告；新会话重置旧的 blocked 深度。
4. 保留 `ctx.hasUI` RPC 入口、`herdr:pi` / `pi` 配对、原有 idle/working/blocked 映射和不主动 release 的设计。

## 验证

- `node --test extensions/pi-grok-herdr/index.test.mjs`：4/4，通过。覆盖非 Herdr no-op，session→state 和 working/blocked/idle，`agent_start` 与 session replacement 有序，延迟应答阻塞后续请求，API error 和拆包响应。
- `node --check extensions/pi-grok-herdr/index.test.mjs` 与 `git diff --check`：通过。
- 没有运行 Cargo、重建或重启运行中的 grok-pi（项目 AGENTS.md 限制）；当前 live pane 仍使用旧二进制，因此不能宣称 Header 已恢复。

## 新二进制的验收门槛

需在明确授权构建后，让新 grok-pi 进入 **全新的独立 Herdr pane**（不能用已经运行的旧进程判断新代码）。收集：

```sh
herdr pane process-info --pane <PANE_ID>
herdr agent explain <PANE_ID> --json
herdr agent list
```

必须看到 `agent=pi`、`herdr:pi` session ref、`screen_detection_skipped=true`、`screen_detection_skip_reason=full_lifecycle_hook_authority`；再检验 Working → Idle、Blocked → 恢复。若 Socket 已确认 `result` 但 authority 仍不成立，查 Herdr 的 process presence、session anchor、stale/exit suppression 和 seq。**不得把 API 成功误读为 Herdr 已接纳。**

## 部署与回滚

Rust binary 以 `include_str!` 嵌入这份 TypeScript，因此更改源码不会影响已启动 pane；需要后续授权构建并启动新进程。回滚只需恢复扩展对应提交并重建，既不迁移会话数据也不修改 Herdr。
