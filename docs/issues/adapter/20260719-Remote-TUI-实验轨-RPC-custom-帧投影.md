---
id: "2026-07-19-Remote-TUI-实验轨"
title: "Remote TUI 实验轨：RPC custom() 帧投影到 Grok"
status: "in_progress"
created: "2026-07-19"
updated: "2026-10-30"
# note: L1 code landed; interactive grok-pi hand-test still pending
category: "adapter"
tags: ["workhub", "adapter", "experimental", "remote-tui", "rpc", "custom", "extension"]
---

# Issue: Remote TUI 实验轨 — RPC `custom()` 帧投影

## Goal

在 **env 门控** 下，让 Pi RPC 的 `ctx.ui.custom(factory)` 在 **Pi 进程内** 真正运行 Component，并把可序列化帧投到 Grok 实验视口，形成：

`显示 → 键控 → done(result) → 关闭` 的 L1 闭环。

**不是** 产品化接管全部 Pi TUI 扩展；**是** 证明 RPC 边界上的创新可能。

## 约束

1. 默认关闭：`PI_GROK_REMOTE_TUI=1` 才启用；关时行为与今日完全一致。
2. Adapter 保持 headless：只搬帧/键/开关，不渲染 Component、不拥有 terminal。
3. Component 永远在 Pi 进程内创建与 `handleInput`；跨进程只传 JSON。
4. 第一刀不做 rpiv-ask 全量；用探针扩展 `/remote-tui`（SelectList）验证。
5. 优先复用已有原生表面：`pi/ui/widget` 显示行；键拦截放在允许 seam（`app_view` / `acp_handler`）。
6. 明确标 `experimental`；不写入 FEATURE_MATRIX「已适配」列。

## 架构

```text
Extension: ctx.ui.custom(factory)
  → [RPC, env on] Mini host in rpc-mode
  → factory(tuiStub, theme, keybindings, done) → Component
  → render(width) → lines[]
  → extension_ui_request: remote_tui_open | remote_tui_frame | remote_tui_close
  → adapter → ExtNotification pi/ui/remote_tui
  → Pager: set_external_widget(lines) + remote_tui session state
  → Key → Effect → ExtNotification pi/ui/remote_tui/input
  → adapter → stdin { type: remote_tui_input, id, data }
  → component.handleInput → re-frame
  → done(result) → close
```

## L2 Pi 对齐补充（2026-07-31）

- `ansi_remote_tui_line` 保留 24-bit ANSI 背景色；`▀` 半块帧的背景色是下半像素，不能统一覆盖为 Pager 背景。
- Remote TUI host 通过隐藏的 `__pi_grok_remote_tui_layout__` widget 传递 `ctx.ui.custom()` 的 `overlay`、`overlayOptions.width/maxHeight/anchor/row/col/offsetX/offsetY`；Pager 在 native buffer 内居中/裁剪/定位，不把 overlay 帧堆入 editor widget rows。
- `KeyEventKind::Repeat/Release` 转成 Pi-TUI Kitty `CSI-u`（`:2`/`:3`），保留 `wantsKeyRelease` 组件语义；Esc 按下仍优先取消，Esc 释放转发给组件。
- Remote TUI host 默认复现 Pi `custom()` 的 inline/editor 承载；显式 `overlay: true` 才启用弹窗。弹窗宽度优先级为 `overlayOptions.width`、factory 返回组件的 `width`、Pi 默认 `min(80, terminalWidth)`。

## 协议

### Pi stdout（fire-and-forget）

| method | 字段 | 含义 |
|---|---|---|
| `remote_tui_open` | `id`, `title?`, `width?` | 打开会话 |
| `remote_tui_frame` | `id`, `lines: string[]` | 推送一帧 |
| `remote_tui_close` | `id` | 关闭会话 |

### Pi stdin

| type | 字段 | 含义 |
|---|---|---|
| `remote_tui_input` | `id`, `data` | 终端键序列（如 `\x1b[A`） |
| `remote_tui_cancel` | `id` | 取消并 `done(undefined)` |

### ACP

| 方向 | method |
|---|---|
| Agent → Client | `pi/ui/remote_tui` `{ op: open\|frame\|close, id, lines?, title? }` |
| Client → Agent | `pi/ui/remote_tui/input` `{ id, data }` |
| Client → Agent | `pi/ui/remote_tui/cancel` `{ id }` |

## 非目标（本 Issue）

- 多 overlay 栈、header/footer/editor factory
- Kitty 协议完整仿真、硬件光标、IME
- 默认开启 / 社区扩展 100% 兼容
- 在 adapter 内解释 Component AST
- 修改 juicesharp 插件源码

## 验收

- [x] `PI_GROK_REMOTE_TUI` 未设置：`custom()` 仍 stub；无新协议流量（代码路径门控）
- [ ] env=1：`/remote-tui` 显示列表帧到 Grok widget 面（待手测）
- [x] 方向键/Enter 能选择；进程内 host 单元探针 PASS（Node）
- [x] 帧投影前剥离 `ESC_pi:c BEL` 硬件光标标记，将 `ESC_pi:fc BEL` / `ESC_pi:/fc BEL` 转为 SGR 7/27 反显；Input/Editor 光标保持可见，帧内无 `_pi:c`、`_pi:fc` 或 `_pi:/fc`。
- [ ] Esc 取消并关闭帧（待手测）
- [x] `cargo test -p pi-grok-adapter` PASS；`cargo check -p xai-grok-pager --lib` PASS；`cargo test -p xai-grok-pager-bin --bin grok-pi remote_tui` PASS
- [ ] `xai-grok-pager --lib` 全量测试：上游/既有测试编译失败（voice helper、layout arity 等），与本变更无关
- [x] 文档标明 experimental

## 实现切片

1. [x] Issue（本文）
2. [x] `rpc-mode.ts` + `rpc-types.ts` + `remote-tui-host.ts`：env 门控 mini host
3. [x] `pi-grok-adapter`：协议透传
4. [x] Pager：`pi/ui/remote_tui` → widget + 键拦截 + Effect
5. [x] 探针 extension + grok-pi 条件注入
6. [~] 验证与回写（自动化部分完成；全链路手测待做）

## 进度

- **[2026-07-19]**: 状态 → in_progress；完成可行性与接缝调研，开始 L1 实现。
- **[2026-07-19]**: L1 代码落地。Pi host 节点探针：`open → frame → input(down) → input(enter) → close`，result=`b`。
- **[2026-07-19]**: 手测命令：`PI_GROK_REMOTE_TUI=1 ./run-local.sh` 后 slash `/remote-tui`。
- **[2026-07-19]**: 手测失败根因：`run-local` 默认 `--pi-bin pi` 指向全局 npm Pi **0.80.10**（无 `remote-tui-host`），扩展加载成功但 `custom()` 仍是 stub → 立即 `undefined` → toast cancelled。已改 `run-local.sh`/`build.sh` 优先 bundled `pi-main/.../dist/cli.js`；RPC 探针验证 open/frame/input/close PASS。
- **[2026-07-19]**: **回滚 Pi 源码补丁**（删除 `remote-tui-host.ts` / rpc-mode 改动）。新路径：注入 extension monkey-patch `ctx.ui.custom` + 官方 `setWidget` 推帧 + tmp keyfile 收键；adapter `pi/ui/remote_tui/input|cancel` 写 keyfile；Pager external widget ANSI 解析。RPC 探针：`setWidget` 帧含 ANSI → keyfile ↓/Enter → `notify selected: beta` PASS。
- **[2026-07-19]**: rpiv `Type something.` 显示 `_pi:c`：这是 Pi `CURSOR_MARKER`（APC 硬件光标定位标记）泄漏，而非键盘输入错误。Remote TUI host 在投影 frame 前剥离 marker；`bun test extensions/pi-grok-remote-tui/index.test.ts` 2 PASS。

## Fake cursor APC 修复（2026-10-30）

- [x] 读取交接与当前实现，保留已有鼠标/transport 未提交改动；扩展基线 14 pass。
- [x] 两条活动投影路径先剥离硬件 marker，再将 fake cursor 的开始/结束标记转换为 SGR 7/27；不改 Pi、Pager theme 或归档副本。
- [x] 回归覆盖三个 marker、多个光标格与已有 ANSI；两个 Bun suite 合并运行 15/15，通过。
- [x] 系统 Pi 1.4.0 真实 Input/Editor 隔离 RPC，4/4 场景通过。
- [x] Rust bin 测试目标 96/96，通过（含 Remote TUI materialization）。
- [x] 生产 Rust check 通过（已有 unused/dead-code 警告）。
- [x] 直接 Rust 组合二进制构建通过；新二进制 + 系统 Pi 1.4.0 的 PTY 验收 4/4，通过。
- [x] 回写实际验证范围、构建阻塞与端到端验收结果；更新输入所有权 follow-up、VERIFICATION 与 workhub PR 记录。

两条活动路径（`host.ts` 与默认关闭的 Rust TUI bridge）各自使用模块级字面量常量；Pi 不公开导出 fake-cursor 标记，因此不使用包内深导入。保留现有 map/replaceAll，未引入新依赖或模块 materialization 变更。

回归证据：主路径测试使用真实 APC fixture，额外覆盖多个光标格与已存在的 ANSI 样式；第二路径通过实际注册的 bridge 捕获 frame 通知。两条新增/扩展断言在修复前均因 `pi:fc` 泄漏失败，修复后主路径 14/14、bridge 1/1 通过。系统 Pi 1.4.0 的隔离 RPC 探针运行真实 Input/Editor，4/4 场景通过：初始帧具有 SGR 7/27，无三种 APC 载荷；字母、大写 Z、左方向键、Enter 提交与 Escape 退出行为正确。探针未调用模型，也未修改用户配置。

Rust 验证证据：Cargo 已编译 bin test target；运行刚生成的 `target/debug/deps/grok_pi-3b05cb2e7fd4e4cf`，退出码 0，96 passed / 0 failed，包含两条投影扩展的内嵌契约与 Remote TUI 依赖闭包 materialization。

构建阻塞（2026-10-30）：`./build.sh` 的可选 `pi-main` coding-agent 重建在 TypeScript 阶段退出 2，依赖产物缺少 `TuiMainScreen`、`setCapabilityOverrides`、`AgentToolCallOutcome` 等导出，并有大量跨包类型不匹配；本任务未改 Pi 源码。生产 Rust check 已通过。改用 `./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi` 直接重建内嵌扩展，并以系统 Pi 1.4.0 验收。

最终验收（2026-10-30）：生产 `cargo check` 通过；直接 Rust build 退出 0，并已重建 `target/debug/grok-pi` 的内嵌扩展。隔离 PTY 使用该新二进制、系统 Pi 1.4.0 及真实 Input/Editor，四个场景（两种组件 × Enter/Escape）全部通过；字母、大写 Z、左方向键、结果 `aZb` 及关闭后重开均正确，实际终端输出没有 `_pi:fc` / `_pi:/fc`。探针通过内嵌 host 执行，没有从工作树单独加载 remote-tui 扩展，也没有调用模型或改用户配置。原截图的第三方 Compaction model 弹窗未单独手测；本轮不据此宣称所有第三方组件已兼容。

变更记录见 [workhub PR](../../pr/adapter/20261030-Remote-TUI-fake-cursor-APC归一化.md)。保留开工前已有未提交改动，未执行 git stage/commit/push。
