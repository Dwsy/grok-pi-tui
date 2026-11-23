---
id: "2026-11-23-offload-extensions-and-tree-modals-to-ts"
title: "P1 共享基建收口与 P2 重型弹窗 TS 化（/tree 会话树与后续 pi_config 资源管理）"
status: "in_progress"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "issue", "ts-tui-kit", "session-tree", "pi-config", "offload"]
---

# P1 共享基建收口与 P2 重型弹窗 TS 化

## Goal

1. **Phase 1 (P1 共享基建收口 — 已完成)**：
   - 抽离 Rust 公共 `grok_pi::tui_kit::materialize_tui_kit` helper，消除各 `*_extension.rs` 对 `pi-grok-tui-kit` 的重复 `include_str!` 拷贝。
   - 在 `extensions/pi-grok-tui-kit/list.ts` 抽离通用虚拟列表与双击状态机 (`ListController`)。
   - 为 `extensions/pi-grok-review/` 接入高清图片直接预览（复用 `pi-grok-tui-kit/image.ts` + `__pi_grok_remote_tui_image__`）。
2. **Phase 2 (P2-1 `/tree` 会话树弹窗 TS 化)**：
   - 新增独立插件 `extensions/pi-grok-tree/`，直接读取当前会话 JSONL 树结构，左栏渲染带分支连线的树视图，右栏展示消息/工具/标签详情预览，支持 `All` / `Messages` / `Labeled` 过滤、多词搜索与 `Enter` 导航分支（在 `grok-pi` 下通过 `pi/ui/mention_insert` 派发 `Action::NavigateSessionTree` 复用 Rust 既有导航/回放链路；独立 Pi 下调用 `ctx.navigateTree`）。
   - 在 `xai-grok-pager/src/app/dispatch/session/tree.rs` 中将 `dispatch_show_session_tree` 路由至 `__pi_session_tree`。
3. **Phase 3 (P2-2 资源管理 `pi_config` 架构说明)**：
   - `grok-pi` 实际使用的资源管理是 `views/pi_config.rs` + `pi_resource_config.rs` / `pi_resource_policy.rs`（而非 Grok 原生 `extensions_modal.rs`），坚持“Rust 提供 `PiResourceCatalog` / `ResourcePolicy` API，TS 仅负责渲染与交互”的原则，避免在 TS 侧重复实现策略逻辑。

## Phases

- [x] **Phase 1 (P1): Rust `tui_kit` 公共挂载 + TS `ListController` + `/review` 高清图预览**
- [x] **Phase 2 (P2-1): 独立 TS `/tree` 会话树插件 (`extensions/pi-grok-tree/`)** (`docs/pr/pager/20261123-offload-session-tree-to-ts.md`)
- [ ] **Phase 3 (P2-2): Rust `pi_resource_config` API 桥接 + TS 渲染层规划/实施**
