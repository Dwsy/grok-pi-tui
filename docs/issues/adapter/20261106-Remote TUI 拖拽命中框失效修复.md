---
id: "2026-11-06-Remote TUI 拖拽命中框失效修复"
title: "Remote TUI 拖拽命中框失效修复"
status: "done"
created: "2026-11-06"
updated: "2026-11-06"
category: "adapter"
tags: ["workhub", "Remote TUI", "mouse", "drag", "hitbox"]
---

# Issue: Remote TUI 拖拽命中框失效修复

## Goal

修复 Remote TUI 中 `ctx.ui.custom()` 组件拖拽被 Pager 丢弃、release 退化成 click 的问题，使真实终端 `Down → Drag → Up` 能完整送达 Pi 侧组件。

## 背景/问题

`extensions/pi-grok-remote-tui/host.ts` 已实现 Pi TUI 的 `handleMouse()` 镜像；GAPP 看板 `tui.mjs` 也实现了 `press/capture → drag → release → move_card`。但 Pager 在组件每次重绘帧时把 `remote_tui_rect` 清为 `None`，而 `app_view.rs::remote_tui_mouse_event()` 在 rect 暂缺时静默丢弃 `Drag/Moved`。同一输入 batch 内不会每事件重绘，于是拖拽事件丢失；host 的 `mousePressMoved` 不置位，release 后合成 `click`，看板执行 `advanceCard()`。

## 验收标准 (Acceptance Criteria)

- [x] WHEN Remote TUI 组件在 press 后重绘但帧行数/布局/偏移不变，THEN Pager SHALL 保留 `remote_tui_rect`，后续 `Drag(Left)` 与 `Up(Left)` 仍产生 `RemoteTuiMouse` effect。
- [x] WHEN `remote_tui_rect` 因几何变化暂缺且存在 active mouse gesture，THEN Pager SHALL 用上一帧命中框继续转发 `drag/move/release`，而不是静默丢弃。
- [ ] WHEN 真实 `Down → Drag → Up` 序列到达，THEN host SHALL 收到 `press`、`drag`、`release`，不合成 `click`，组件拖放逻辑可调用 `move_card(targetStatus)`。（真实 PTY 手工验收待补）
- [x] 现有 click、双击、滚轮、inline/overlay、区域外回退与 stale session 行为 SHALL 不回归。
- [x] 指定 Bun / Rust 测试、生产 check、构建与格式检查 SHALL 通过。

## 实施阶段

### Phase 1: 规划和准备
- [x] 分析需求和依赖
- [x] 设计技术方案
- [x] 确定实施计划

### Phase 2: 执行
- [x] `agent_view/notices.rs`：纯文本帧变化不再清 `remote_tui_rect`，仅几何变化时失效。
- [x] `app_view.rs`：有 active gesture 且 rect 暂缺时，用 `remote_tui_last_rect` 继续转发；无法恢复时清理手势。
- [x] `app_view_tests.rs`：补「press → 帧内容变化 → Drag → Up」回归。
- [x] 复用现有 `index.test.ts` 的 `press + drag + release` 不合成 click 回归，未新增 host 侧用例。

### Phase 3: 验证
- [x] Bun 扩展测试：14 passed / 0 failed。
- [x] Pager Remote TUI 定向 Rust 测试：24 passed / 0 failed。
- [x] 生产 `grok-pi` bin check：通过。
- [x] 直接 `cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi`：通过。
- [x] `rustfmt --check` 与 `git diff --check`：通过。

### Phase 4: 交付
- [x] 更新本 Issue 验证记录
- [x] 创建 PR 记录
- [ ] 合并主分支（由用户决定）

## 关键决策

| 决策 | 理由 |
|------|------|
| 不改 Pi 源码、不改 GAPP `tui.mjs`、不改 `host.ts` 的 click 合成语义 | 根因在 Pager 输入/重绘竞态；改 host 会破坏普通点击与双击 |
| 用 `geometry_changed` 区分帧内容变化与命中框几何变化 | 拖拽只改列头 marker 和提示行，行数/布局不变，rect 本应稳定 |
| 保留 `remote_tui_last_rect` 作为 gesture 期间的 fallback | 几何变化仍需重算，但不能让已捕获的手势因一帧延迟被丢弃 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-11-06 | `cargo-shared.sh` 默认 20 GiB 空闲空间阈值拦截生产 check | 用 `CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0` 复跑；生产 check 与构建通过，未改代码 |
| 2026-11-06 | `rustfmt --check` 报 `notices.rs` 折行差异 | 按 rustfmt 输出调整 `geometry_changed` 表达式折行后复跑通过 |

## 相关资源

- [x] 相关文档: `docs/issues/adapter/20261029-Remote-TUI鼠标事件对齐.md`
- [x] 相关文档: `docs/VERIFICATION.md`
- [x] 相关文档: `docs/pr/adapter/20261106-Remote TUI 拖拽命中框失效修复.md`
- [x] 相关代码: `extensions/pi-grok-remote-tui/host.ts`
- [x] 相关代码: `crates/codegen/xai-grok-pager/src/app/app_view.rs`
- [x] 相关代码: `crates/codegen/xai-grok-pager/src/app/agent_view/notices.rs`

## Notes

验证命令：

```bash
bun test extensions/pi-grok-remote-tui/index.test.ts extensions/pi-grok-remote-tui/transport.test.ts
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh test -p xai-grok-pager --lib remote_tui
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
CARGO_MIN_FREE_GIB=1 CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi
```

新增回归 `remote_tui_drag_survives_content_only_frame_update_and_missing_hitbox` 在修复前失败于「content-only frame update must not invalidate the hit box」，修复后通过。真实终端 PTY 手工鼠标协议验收仍未完成，需用户用重编译后的 `grok-pi` 实测看板跨列拖放。

---

## Status 更新日志

- **[2026-11-06]**: 状态变更 → in_progress，备注: 完成根因 review，开始修复。
- **[2026-11-06]**: 状态变更 → done，备注: Pager 修复、回归、生产 check 与构建完成；PTY 手工验收保留为后续动作。
