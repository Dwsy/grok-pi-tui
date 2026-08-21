# 20260821 - 修复 recap / btw 桥接消息注入 Agent Context

## 状态

已实现（静态修改完成；按用户要求未运行 cargo 验证，测试用例已随改动补齐）

## 根因

用户报告 recap（及 btw）生成的历史消息进入了 agent loop（LLM context）。

链路（全部已用源码验证）：

1. `extensions/pi-grok-recap/index.ts` `emitSummary()` 用
   `pi.sendMessage({customType, content, details}, {triggerTurn: false})`。
2. Pi `sendCustomMessage()`（`pi-main/packages/coding-agent/src/core/agent-session.ts:1429-1462`）：
   - 非流式：`agent.state.messages.push(appMessage)` + 持久化 `custom_message` entry
     + 发 `message_start`/`message_end` 事件。
   - 流式且未指定 `deliverAs`：默认 `agent.steer(appMessage)` 注入当前 turn
     （agent-loop.ts:180-188）。
3. `convertToLlm`（`pi-main/packages/coding-agent/src/core/messages.ts:148-186`）把
   `role: "custom"` 消息转成 `role: "user"` 发给 LLM。`display: false` 只影响 TUI 显示，
   不影响 context。
4. 会话 reload 时 `entriesToMessages`（`pi-main/packages/agent/src/harness/session/session.ts:112-118`）
   把 `custom_message` entry 还原进 `agent.state.messages`，再次污染。

BTW live 流量（`extensions/pi-grok-btw/index.ts` `emit()`）同样用 `sendMessage`；
其持久化历史已正确使用 `appendEntry`（`custom` entry 默认不进 context，session.ts:132-133）。

## 方案

不修改 Pi 源码（AGENTS.md 不变量 5）。全部修复在 extensions/ + adapter/：

1. **扩展侧**：live 桥接消息从 `pi.sendMessage` 改为 `pi.appendEntry(BRIDGE_TYPE, data)`。
   与 `extensions/pi-grok-subagents/index.ts` `emit()` 非 replay 路径一致（仓库已验证模式：
   sendMessage 会 steer parent / push 进 state.messages 造成 phantom turn 或 context 污染）。
   - recap：`emitSummary` -> `appendEntry(BRIDGE_TYPE, {version:1, ok:true, auto, summary})`；
     auto 去重 `lastSuccessfulRecapTurnCount` 改扫 `type === "custom" && customType === BRIDGE_TYPE`
     的 `data`。
   - btw：`emit()` delta/complete 全部 -> `appendEntry(BRIDGE_TYPE, {version:1, requestId, ok,
     phase, ...})`；历史 entry 不变。
2. **adapter 侧**：
   - `btw_bridge.rs` `parse_btw_message` 支持 entry shape（`event.entry.data`，对齐
     subagent_projection `bridge_details()` 双 shape 模式）；当前只解析 message_end 的
     `message.details`，appendEntry 产生的 `entry_appended` 事件解析失败。
   - `recap_bridge.rs` `parse_recap_message` 同样支持 entry shape。
   - `events.rs` `entry_appended` 分发加入 recap（btw 已在）。
   - parser 保留 message_end shape 兼容（subagents replay 仍走 sendMessage）。
3. **兜底**：两个扩展注册 `pi.on("context")` 过滤 `role === "custom"` 且 `customType` 为
   本桥接类型的遗留消息，清理旧会话文件的 context 污染。ContextEventResult 返回
   `{messages}`（runner.ts:984-1010，返回值替换 currentMessages）。

## 验证计划

- `./scripts/cargo-shared.sh test -p pi-grok-adapter`（btw_bridge / recap_bridge parser
  单测补 entry shape 用例）
- `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi`（injector source 断言同步）
- `./build.sh` + 真实会话冒烟：/recap、/btw 后续 turn 的 LLM context 不再包含桥接内容

## 文档同步

FEATURE_MATRIX.md / FEATURE_MATRIX.zh-CN.md：recap 条目 "does not write session history"
在修复后才真实成立；btw 条目补充 "桥接消息不进 agent context"。CHANGELOG 中英同步。
