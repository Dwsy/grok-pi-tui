---
id: "2026-11-13-Pi Working message 覆盖 TurnActivity"
title: "Pi Working message 覆盖 TurnActivity（F2 可选）"
status: "in-progress"
created: "2026-11-13"
updated: "2026-11-13"
category: "pager"
tags: ["workhub", "working-message", "turn-status", "f2", "pi-ui"]
---

# Issue: Pi Working message 覆盖 TurnActivity（F2 可选）

## Goal

让 Pi 扩展通过 `ctx.ui.setWorkingMessage()` 设置的文本，在 grok-pi 的 turn status 行取代
`TurnActivity` 的 `Thinking` / `ToolRunning` / `WritingToolCall` / `Waiting` 四种标签；
由 F2 开关控制，**默认关闭**。不修改 whimsical 扩展。

## 背景/问题

Pi 在 RPC 模式下把 working message 相关 API 全部实现为空操作：

```ts
// pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:183
setWorkingMessage(_message?: string): void {
    // Working message not supported in RPC mode - requires TUI loader access
},
setWorkingVisible(_visible: boolean): void {},
setWorkingIndicator(_options?: WorkingIndicatorOptions): void {},
setHiddenThinkingLabel(_label?: string): void {},
```

grok-pi 通过 JSONL RPC 驱动 Pi，因此扩展推送的 working message 到不了 Pager。
`pi-grok-adapter` 的 `handle_extension_ui` 也不转发这四个方法（落在 `_ =>` 兜底分支）。

结果：whimsical 这类扩展在原生 Pi TUI 里能显示语录，在 grok-pi 里完全失效。

## 现状调研

### grok 自身的 Working message 类型全集

Turn status 行布局：`⠧ <label> 0.2s             1m20s ⇣12k [stop]`

渲染入口 `crates/codegen/xai-grok-pager/src/views/turn_status.rs::compute_activity()`。
两层状态来源：

**AgentState（`app/agent.rs:557`）**

| 状态 | 标签 |
|---|---|
| `Idle` | 整行隐藏 |
| `TurnRunning` | 交给 TurnActivity |
| `TurnCancelling` / `CommandCancelling` | `Cancelling…`（`accent_error`） |
| `CommandRunning { command }` | `{command.display_name()}…` |

**TurnActivity（`acp/tracker.rs:289`）**

| Variant | 标签 | 颜色 |
|---|---|---|
| `Thinking` | `Thinking…` | text_secondary |
| `Responding` | `Responding…` | text_secondary |
| `ToolRunning { title, description }` | 有 desc → `{desc}…`；`Ask:` → `Waiting on answers for X`；`Web search:` → `Search {q}`；`Fetch:` → `Fetch {url}`；其他 → `Run {cmd}` | 绿 / secondary |
| `AutoCompacting` | `Compacting…` | text_secondary |
| `Retrying { attempt }` | `Retrying (attempt N)…` | warning |
| `WritingToolCall` | `Writing file…` / `Writing command…` / `Writing subagent prompt…` / `Preparing MCP tool…` / `Updating todo list…` 等 11 种 | text_secondary |
| `Waiting(reason)` | 见 `WaitingReason::label()` | text_secondary |

`WaitingReason`（`acp/tracker.rs:115`）7 种：`Model` → `Waiting for response…`、
`Subagent{display}` → `{display}…`、`TaskOutput{subject}` → `{subject}…`、
`TasksComplete` → `Waiting on tasks…`、`Sleep` → `Sleeping…`、
`Hooks{event,count}` → `Running N {event} hooks…`、`PromptAck` → `Waiting for the agent to accept the prompt…`

叠加规则：`goal_verifying` → `Verifying…`；bash turn 无 activity → `Running…`；
`is_pending_user_input` 时 spinner 换成脉冲菱形 `◆`。

优先级在 `Tracker::activity()`（`acp/tracker.rs:582`）硬编码：
retry > compaction > hooks > blocking wait > writing tool call > thinking > tool running > responding。

### 已有的绕过先例

`extensions/pi-grok-remote-tui/` 已经示范了不改 Pi 源码的做法：
monkey-patch `ctx.ui`（`host.ts::installCustomPatch`），把组件在 Pi 进程内跑起来，
帧通过 `ctx.ui.setWidget()` 投影到 Pager。

`extensions/pi-grok-rpc-compat/index.ts` 进一步示范了 patch Pi 宿主模块：
通过 `hostUrl("core/extensions/runner.js")` 动态 import 宿主 dist，改
`ExtensionRunner.prototype.setUIContext`，把 `mode === "rpc"` 改写成 `"tui"`。
文件头有明确警告：**不要重新赋值 ESM named exports**（Node 会冻结），
要包 `process.stdout.write` 再 `takeOverOutput()`。

### 现有通路能力

| Pi API | RPC 下可用 | adapter 转发 | Pager 落点 |
|---|---|---|---|
| `setStatus` | ✓ | `pi/ui/status` → `handle_pi_ui_status` | `app.set_external_status` → footer statuses |
| `setWidget` | ✓ | `pi/ui/widget` → `handle_pi_ui_widget` | `app.set_external_widget`（仅 Above/BelowEditor） |
| `setWorkingMessage` | ✗ 空操作 | 无 | 无 |

## 方案

采用「**双写 + adapter 新增通路 + Pager 消费**」，不改 Pi 源码、不改 whimsical。

### 关键决策：为什么不用 Remote TUI 那套 frame 投影

turn status 是 Pager 每帧在 `agent_view/render.rs:2883` 直接 `render_turn_status()` 画进 `Buffer` 的
单行 widget，它不经过 `ExternalWidget` 通道。走 frame 投影等于在 Pager 里再叠一层渲染器，
与架构不变量 #1（Grok Pager 是唯一终端 UI）冲突。正确做法是**让 Pi 的文本成为一个
`TurnActivity` 级的输入**，由 Pager 原生渲染。

### 1. Pi 侧（新扩展，不改 whimsical）

新建 `extensions/pi-grok-working-message/index.ts`，用与 remote-tui 相同的手法：

- `session_start` 时 monkey-patch `ctx.ui.setWorkingMessage`
- 原文同时保留原生调用（原生 Pi TUI 保持原行为）并镜像到
  `ctx.ui.setStatus("__pi_grok_working_message__", text)`
- 只在 `shouldInstallHost()`（`PI_GROK=1`）时启用，原生 `pi` 完全无感

这样任何扩展（含 whimsical）调用 `setWorkingMessage` 都会自动流到 Pager，无需感知。

### 2. adapter（`crates/codegen/pi-grok-adapter/src/pi_adapter/tools.rs`）

`handle_extension_ui` 的 `"setstatus"` 分支里，为
`EXTENSION_WORKING_MESSAGE_STATUS_KEY` 单独开一条通知：

```rust
"setstatus" => {
    match key {
        EXTENSION_WORKING_MESSAGE_STATUS_KEY => {
            self.send_ext_notification(
                "pi/ui/working_message",
                json!({ "text": text }),
            ).await;
        }
        // …现有 queue / bash-task 分支…
    }
}
```

### 3. Pager

- `app/app_view.rs`：`ExternalUiState` 增 `working_message: Option<String>`；
  新增 `set_external_working_message()`；`reset_extension_ui()` 一并清空
- `app/acp_handler/mod.rs`：`"pi/ui/working_message" => handle_pi_ui_working_message`
- `views/turn_status.rs`：`TurnStatusArgs` 增 `pi_working_message: Option<&str>`
- `app/agent_view/render.rs:2883`：把 `external_ui.working_message` 传进去

### 4. 覆盖规则（核心约束）

在 `compute_activity` 的 match **之前**插入一个前置判定，只在同时满足下列条件时生效：

```rust
let override_label = pi_working_message.filter(|_| {
    !is_pending_user_input                    // permission / question / MCP elicitation
        && !goal_verifying                    // plan approval 的 Verifying…
        && !state.is_cancelling()             // TurnCancelling / CommandCancelling
        && matches!(
            activity,
            Some(
                TurnActivity::Thinking
                    | TurnActivity::ToolRunning { .. }
                    | TurnActivity::WritingToolCall(_)
                    | TurnActivity::Waiting(_)
            )
        )
});
```

**不覆盖**的四类：

| 不覆盖项 | 判定来源 | 理由 |
|---|---|---|
| pending user input | `blocking_card()` ∈ {Permission, Question, McpElicitation} | 必须让用户看见在等他 |
| plan approval | `goal_state.verifying_completion` | 同上，`Verifying…` 是行动指令 |
| cancelling | `AgentState::TurnCancelling` / `CommandCancelling` | 用户主动中止的反馈 |
| `Responding` / `AutoCompacting` / `Retrying` | match 到具体 variant | 都是真实进度信号，语录会误导 |

同时保留：spinner 字符、相位计时、`[stop]` 按钮、右侧 turn 计时和 token 计数 ——
只替换 label 文本本身。

### 5. F2 开关

`settings/defs.rs` 新增（`category: Agent`、`owner: Shell`、`external_only: true`）：

```rust
SettingMeta {
    key: "pi_working_message",
    category: SettingCategory::Agent,
    owner: SettingOwner::Shell,
    label: "Pi working message",
    description: "Show the text Pi extensions set via ctx.ui.setWorkingMessage() in the turn \
                  status line, replacing Thinking / tool / writing / waiting labels. Never \
                  replaces permission prompts, plan approval, or cancellation.",
    keywords: &["pi", "working", "message", "spinner", "status", "whimsical"],
    kind: SettingKind::Bool { default: false },
    restart_required: false,
    hidden_in_minimal: false,
    external_only: true,
},
```

配套 7 处（照 `pi_bash_command_format` 的既有模式）：

| 文件 | 改动 |
|---|---|
| `xai-grok-shared/src/ui_config.rs` | `#[serde(default)] pub pi_working_message: bool` |
| `settings/defs.rs` | 上面的 `SettingMeta` |
| `settings/registry.rs` | `get_current_value` 分支 |
| `settings/layout.rs` | 归入 `"Pi features"` 分区 |
| `views/pi_settings/actions.rs` | `Action::SetPiWorkingMessage` |
| `app/dispatch/settings/setters.rs` | `set_pi_working_message{,_inner}` |
| `app/dispatch/settings/ui.rs` | `(key, SettingValue::Bool)` 两个分支 |
| `app/dispatch/router.rs` | `Action` 分发 |

默认关闭 → 现有行为零回归。

## 验收标准

- [ ] WHEN F2 `pi_working_message` 关闭（默认），Pager 行为与现在完全一致
- [ ] WHEN 开关开启且 Pi 推来 working message，`Thinking` 显示该文本而非 `Thinking…`
- [ ] WHEN 开关开启且 activity 为 `ToolRunning` / `WritingToolCall` / `Waiting`，同样被替换
- [ ] IF activity 为 `Responding` / `AutoCompacting` / `Retrying`，THEN 保留原生标签
- [ ] IF 存在 pending permission / question / MCP elicitation，THEN 无论开关如何都显示原生提示
- [ ] IF `goal_verifying` 或 cancelling，THEN 显示原生标签
- [ ] WHEN 开关关闭或 Pi 未推送文本，SHALL 回落到原生 `TurnActivity` 标签
- [ ] spinner 相位计时、`[stop]` 按钮、右侧 turn 计时与 token 计数在覆盖时保持不变
- [ ] 原生 `pi`（非 grok-pi）行为不受影响
- [ ] whimsical 扩展零改动，`/whimsical` 面板功能不变

## 实施阶段

### Phase 1: 数据通路（不改渲染） ✅
- [x] 新增 `extensions/pi-grok-working-message/index.ts`（monkey-patch `setWorkingMessage` → `setStatus`）
- [x] 注入器 `working_message_extension.rs` + `grok-pi.rs` 注册（按加载顺序排在 rpc-compat 之后、用户资源之前）
- [x] adapter 常量 `EXTENSION_WORKING_MESSAGE_STATUS_KEY` + `pi/ui/working_message` 分支
- [x] Pager `ExternalUiState.working_message` + `set_external_working_message` + `handle_pi_ui_working_message` + `reset_external_extension_ui` 清理
- [x] `AgentView::set_external_ui_surface` 增 `working_message` 参数，投影到 `external_working_message`

### Phase 2: F2 开关 ✅（改用 host-feature manifest，比原计划少 6 个文件）
- [x] `xai-grok-shared/ui_config.rs`：`#[serde(default)] pub pi_working_message: bool` + default `false`
- [x] 扩展自带 `grok-pi.json`（`kind: bool` / `default: false` / `restartRequired: false` / `configPath: ["ui","pi_working_message"]` / `f2.order: 55`）
- [x] `xai-grok-pager-render/appearance/cache.rs`：`load_pi_working_message` / `set_pi_working_message` + `sync_from_ui_config` 接线
- [x] `set_host_feature_bool` 通用 setter 里对该 key 同步缓存，保证 F2 切换实时生效

> build.rs 自动扫描 `extensions/*/grok-pi.json`，`feature_spec_by_setting_key` 已把
> registry / layout / actions / ui / router / state 全部通用化，因此**无需**改
> `defs.rs`、`registry.rs`、`layout.rs`、`pi_settings/actions.rs`、`router.rs`、
> `dispatch/settings/ui.rs`。原计划高估了改动面。

### Phase 3: 渲染接入 ✅
- [x] `TurnStatusArgs` 增 `pi_working_message: Option<&str>`（含 3 处测试/最小模式构造点）
- [x] `compute_activity_with_working_message` 前置 override；`working_message_replaces` 白名单四个 variant
- [x] 保护条件：`state.is_turn_running()` / `!goal_verifying` / `!is_pending_user_input`；`Responding`/`AutoCompacting`/`Retrying`/cancelling 天然不在白名单
- [x] `agent_view/render.rs` 用 `load_pi_working_message()` 门控后传入；minimal 模式传 `None`

### Phase 4: 验证（部分完成，未跑 Rust）
- [x] `turn_status.rs` 单测 3 个：四种可覆盖、四种不可覆盖、关闭/空白回落
- [x] `working_message_extension.rs` 注入器单测：源码 materialize 内容断言
- [x] TypeScript `tsc --noEmit` 全绿（新扩展 + whimsical）
- [x] jiti 端到端：mirror 拦到 whimsical 的 `setWorkingMessage` 并发出 `setStatus`，clear 传 `null`；`/whimsical` 命令仍在
- [ ] `cargo check` / 单测 / tmux 实跑 —— **用户明确要求不跑 Rust，未执行**

## 关键决策

| 决策 | 理由 |
|------|------|
| 走 `setStatus` 而非 `setWidget` 帧投影 | turn status 是 Buffer 直绘的单行 widget，不经过 ExternalWidget 通道；帧投影会引入第二渲染器，违反架构不变量 #1 |
| Pi 侧用 monkey-patch 而非改 Pi 源码 | 遵循架构不变量 #5；remote-tui 和 rpc-compat 已有成熟先例 |
| 独立扩展而非改 whimsical | 用户要求不改 whimsical；且任何扩展都应受益 |
| 默认关闭 | 覆盖 spinner 是显示偏好，不应改变既有用户行为 |
| 只替换 label 文本 | spinner、计时、按钮、token 都是真实进度，不能丢 |
| `Responding` 不覆盖 | 正在输出正文时 turn status 是唯一的状态提示，替换成语录会误导 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|

## 相关资源

- `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:183` — RPC 空操作定义
- `pi-main/packages/coding-agent/docs/rpc-extension-ui.md:18` — 限制清单
- `crates/codegen/xai-grok-pager/src/views/turn_status.rs` — 渲染与 `compute_activity`
- `crates/codegen/xai-grok-pager/src/acp/tracker.rs:115,289` — `WaitingReason` / `TurnActivity`
- `crates/codegen/pi-grok-adapter/src/pi_adapter/tools.rs:745` — `handle_extension_ui`
- `extensions/pi-grok-remote-tui/host.ts:134` — `installCustomPatch` 参考实现
- `extensions/pi-grok-rpc-compat/index.ts:37` — `ExtensionRunner` 宿主模块 patch
- `docs/issues/pager/` — 本次 Issue

## Notes

### 未采用的路线

**A. 扩展直接调 `setStatus`（不改 Rust）**
`setStatus` 在 RPC 下可用，adapter 已完整转发到 footer statuses。
零 Rust 改动即可显示，但落点在 footer 而非 turn status 行，且是纯文本、无 spinner 联动。
作为「十分钟验证」可行，作为正式方案不够。

**B. `setWidget` 到 turn status 位置**
`setWidget` 在 RPC 下可用，但 `ExternalWidgetPlacement` 只有 `AboveEditor` / `BelowEditor`，
不覆盖 turn status 行。要在 turn status 行显示必须新增 placement 或在
`render_turn_status` 里读出 extension slot —— 效果等同方案 B 但绕了一圈。

### 用户决策

- 开关命名 `pi_working_message`，沿用 `pi_bash` / `pi_eval` / `pi_herdr` 风格 ✓
- **不新增环境变量**（用户 13:39 明确要求）。仅复用宿主既有 `PI_GROK=1` 做进程判定，
  扩展始终加载，显示完全由 F2 门控
- 三类状态不得被语录覆盖（用户 13:29 确认）：pending user input / plan approval / cancelling
- **不允许运行 Rust 工具链**（用户 14:01 明确要求）→ `cargo check` / 单测 / 实跑一律未执行，
  只做静态审查

### 风险

- monkey-patch `ctx.ui.setWorkingMessage` 依赖 Pi 内部 uiContext 对象结构。
  已用 `patchedUIs: WeakSet` + 函数 mark 保证每个 uiContext 只 patch 一次；
  `session_start` 在 reload 时也会重发，可覆盖 Pi 重建 runner 的场景。
  已通过读 `runner.ts::wrapUIPromptContext` 与 `agent-session.ts::_applyExtensionBindings`
  确认 `ctx.ui` getter 返回的正是被 patch 的同一对象（`...ui` 浅拷贝保留方法引用）。
- 覆盖后用户可能困惑「为什么看不到工具在跑」，所以默认关闭 + F2 可见开关是关键
- **Rust 侧未编译验证**：`if let ... && ...` let-chain 已确认项目是 edition 2024 / Rust 1.97，
  同文件已有 `&& let Some(...)` 用法；所有 `TurnStatusArgs` / `set_external_ui_surface` /
  `compute_activity_with_working_message` 调用点已逐一静态核对参数个数一致。
- 静态审查抓到并修掉一个真编译错误：`refresh_external_ui_surface` 的
  `self.agents.values_mut()` 闭包里不能直接读 `self.external_ui.working_message`
  （同时可变+不可变借用 `self`）。已改成闭包前 `clone()` 成局部变量再传
  `working_message.as_deref()`。
- `compute_activity` 加了 `#[cfg(test)]`：非测试构建已无人调用，避免 `dead_code` 警告。

### 顺带修复

上一轮为修 `/whimsical` 命令消失加过 `isRpc` 早退，会把本功能的
`setWorkingMessage` 推送一并掐掉。已改为：RPC 下命令与计时循环照常运行
（`setWorkingMessage` 在 Pi RPC 里是 no-op，但 mirror 会拦下转发给 Pager），
只有组件面板仍限 TUI。

---

## Status 更新日志

- **[2026-11-13 13:45]**: 状态 → todo，完成可行性调研（未编译、未运行 cargo），等待确认后实施
- **[2026-11-13 14:05]**: 状态 → in-progress，Phase 1–3 代码完成；TS 类型检查与 jiti 端到端通过；
  按用户要求不跑 Rust，Rust 编译/单测/实跑待后续由用户执行
- **[2026-11-13 14:12]**: 纯静态审查（无 Rust 命令）修掉 borrow 冲突与 `dead_code` 隐患；
  mirror 加 try/catch 隔离；adapter 去掉重复 filter；TS `tsc --noEmit` 与 jiti 端到端复验通过
