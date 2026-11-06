---
id: "2026-11-06-Remote TUI 拖拽命中框失效修复"
title: "Remote TUI 拖拽命中框失效修复"
status: "open"
created: "2026-11-06"
updated: "2026-11-06"
category: "adapter"
tags: ["workhub", "pr", "Remote TUI", "mouse", "drag", "hitbox"]
---

# Remote TUI 拖拽命中框失效修复

> 修复 `ctx.ui.custom()` 组件在 Remote TUI 中拖拽事件被 Pager 丢弃、release 退化成 click 的问题。

## 背景与目的 (Why)

Remote TUI 的 Pi 侧 `host.ts` 与 GAPP 看板 `tui.mjs` 都已支持 `press/capture → drag → release → move_card`。但 Pager 在组件重绘任意帧内容时清掉 `remote_tui_rect`，`remote_tui_mouse_event()` 在 rect 暂缺时静默丢弃 `Drag/Moved`，导致 host 未收到移动、release 后合成 `click`，看板只执行 `advanceCard()`。

## 变更内容概述 (What)

- 区分 Remote TUI 帧「内容变化」与「命中框几何变化」；纯文本变化不再清 `remote_tui_rect`。
- 新增 `AgentView::remote_tui_last_rect`，在 active mouse gesture 且当前 rect 暂缺时作为 fallback 继续转发 `drag/move/release`。
- 无法恢复 rect 时清理手势与 last rect，避免 stale capture。
- 新增 Rust 回归覆盖「press → 帧内容变化 → 几何暂缺 → Drag → Up」。

## 关联 Issue

- **Issues:** `docs/issues/adapter/20261106-Remote TUI 拖拽命中框失效修复.md`
- **相关记录:** `docs/issues/adapter/20261029-Remote-TUI鼠标事件对齐.md`

## 测试与验证结果 (Test Result)

- [x] 单元测试通过
- [x] 集成/定向测试验证
- [ ] 真实终端 PTY 手工回归（待用户用重编译 binary 实测）

```text
bun test index.test.ts transport.test.ts ................ 14 passed / 0 failed
cargo-shared.sh test -p xai-grok-pager --lib remote_tui . 24 passed / 0 failed
cargo-shared.sh check -p xai-grok-pager --lib ............ PASS
cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi  PASS
cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi  PASS
rustfmt --check（5 个改动 Rust 文件）.................... PASS
git diff --check ....................................... PASS
```

生产 check 初次被 `cargo-shared.sh` 的 20 GiB 空闲空间阈值拦截；用 `CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0` 复跑通过。构建产物为 `target/debug/grok-pi`（共享 target 链接）。既有 warning：`non_shorthand_field_patterns` 与 `pi-grok-adapter` dead_code，均为工作区既有，非本次引入。

## 风险与影响评估 (Risk Assessment)

- 命中框保留策略只在 Remote TUI 会话期间生效，不改变普通 Pager 鼠标逻辑。
- `remote_tui_last_rect` 按 session id 变化、`Down` 越界、`Up` 无手势、release 完成后清理；仍存在「几何变化后一帧内使用上一帧 rect」的窗口，但比丢弃整个拖拽手势安全。
- 未改 `host.ts` / `include_str!` 内嵌扩展，不需要刷新 Pi extension bundle；只重编译 Rust binary 即可。
- 真实终端/PTY 鼠标协议验收未完成，不宣称所有终端与 multiplexer 已覆盖。

## 回滚方案 (Rollback Plan)

仅恢复以下 5 个 Rust 文件到本次改动前状态，并删除/保留 workhub 记录按需：

```bash
git status --short
git restore crates/codegen/xai-grok-pager/src/app/agent_view/mod.rs \
  crates/codegen/xai-grok-pager/src/app/agent_view/session.rs \
  crates/codegen/xai-grok-pager/src/app/agent_view/notices.rs \
  crates/codegen/xai-grok-pager/src/app/app_view.rs \
  crates/codegen/xai-grok-pager/src/app/app_view_tests.rs
```

然后重新运行 `cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi`。不要使用 `git restore .` 或 `reset --hard`。

---

## 变更类型

- [x] 🐛 Bug Fix
- [ ] ✨ New Feature
- [x] 📝 Documentation
- [ ] 🚀 Refactoring
- [ ] ⚡ Performance
- [ ] 🔒 Security
- [x] 🧪 Testing

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `crates/codegen/xai-grok-pager/src/app/agent_view/mod.rs` | 修改 | 新增 `remote_tui_last_rect` 字段 |
| `crates/codegen/xai-grok-pager/src/app/agent_view/session.rs` | 修改 | 初始化 `remote_tui_last_rect` |
| `crates/codegen/xai-grok-pager/src/app/agent_view/notices.rs` | 修改 | 仅几何变化清 `remote_tui_rect` |
| `crates/codegen/xai-grok-pager/src/app/app_view.rs` | 修改 | 手势期间 fallback 到 last rect，清理 stale 状态 |
| `crates/codegen/xai-grok-pager/src/app/app_view_tests.rs` | 修改 | 新增拖拽命中框回归 |
| `docs/issues/adapter/20261106-Remote TUI 拖拽命中框失效修复.md` | 新增 | workhub Issue |
| `docs/pr/adapter/20261106-Remote TUI 拖拽命中框失效修复.md` | 新增 | workhub PR 记录 |

## 详细变更说明

### 1. 帧内容变化不再失效命中框

**问题：** `set_remote_tui_surface()` 对任意 line 文本变化都执行 `remote_tui_rect = None`。

**方案：** 以 `line_count` 与 `layout` 计算 `geometry_changed`，只在几何变化时清 rect；`changed` 仍保留用于子视图传播。

**影响范围：** Remote TUI inline/overlay 命中测试；不影响帧渲染内容。

### 2. 已捕获手势使用 last rect

**问题：** `remote_tui_mouse_event()` 在 rect 暂缺时丢弃 `Drag/Moved`，host 因此合成 click。

**方案：** 每次拿到有效 rect 时写入 `remote_tui_last_rect`；当前 rect 暂缺时用 last rect 继续构造归一化事件，release 后清理。

**影响范围：** Remote TUI 鼠标手势路由；普通点击、双击、滚轮、区域外回退保持原语义。

## 测试命令

```bash
bun test extensions/pi-grok-remote-tui/index.test.ts extensions/pi-grok-remote-tui/transport.test.ts
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh test -p xai-grok-pager --lib remote_tui
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi
```

## 破坏性变更

**是否有破坏性变更？**

- [x] 否
- [ ] 是 - [描述破坏性变更及迁移指南]

## 性能影响

**是否有性能影响？**

- [x] 无影响
- [ ] 提升 - [描述性能提升]
- [ ] 下降 - [描述性能下降及原因]

## 依赖变更

**是否引入新的依赖？**

- [x] 否
- [ ] 是 - [列出新增依赖及理由]

## 安全考虑

**是否有安全影响？**

- [x] 否
- [ ] 是 - [描述安全影响及缓解措施]

## 文档变更

**是否需要更新文档？**

- [ ] 否
- [x] 是 - 新增 workhub Issue 与本 PR 记录；`docs/FEATURE_MATRIX.md` 未改，因为行为契约未变化。

## 代码审查检查清单

### 功能性
- [x] 代码实现了需求
- [x] 边界情况已处理
- [x] 现有行为有回归测试覆盖
- [x] 新增回归在修复前失败、修复后通过

### 质量
- [x] 改动范围最小
- [x] 无新增依赖
- [x] `rustfmt --check` 与 `git diff --check` 通过

### 验证
- [x] 单元测试通过
- [x] 生产 check 通过
- [x] 二进制构建通过
- [ ] 真实 PTY 手工验收（待用户）
