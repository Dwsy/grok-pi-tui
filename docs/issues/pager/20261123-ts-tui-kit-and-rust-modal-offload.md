---
id: "2026-11-23-ts-tui-kit-and-rust-modal-offload"
title: "统一 TS TUI 基础库与重型 Rust 弹窗 TS 插件化演进路线"
status: "completed"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "issue", "ts-tui-kit", "mentions", "resume", "offload"]
---

# 统一 TS TUI 基础库与重型 Rust 弹窗 TS 插件化演进路线

## Goal

1. 将工作区未提交的媒体/图片预览渲染改动按模块分组提交，并更新 `CHANGELOG.MD`（双语）与 Workhub 记录。
2. 抽离共享 TS TUI 基础库 `extensions/pi-grok-tui-kit/`，统一所有插件化 TUI 弹窗的视觉与交互设计语言（圆角边框、左右分栏、Header PillBar、搜索栏、Footer 提示、Nerd 图标、鼠标点击与滚轮分发）。
3. 沿着“Rust 提供通用协议基座、TS 负责交互 UI”的架构方向继续瘦身重型 Rust 弹窗：
   - 补齐 `@mcp` 选择器，完成 `@session` / `@skill` / `@file` / `@mcp` 四大对象闭环。
   - 复用 TS `@session` 选择器承接 `/resume` 会话恢复，在 `external_agent` 模式下统一交互体验。

## Phases

- [x] **Phase 1: 分组提交工作区媒体渲染改动并更新 `CHANGELOG.MD`** (`a37fc3b7`, `6a31e052`, `d21a9626`)
- [x] **Phase 2: 抽离 `extensions/pi-grok-tui-kit/` 并重构既有 TS TUI 统一设计语言** (`342bcc82`, PR `docs/pr/pager/20261123-extract-shared-ts-tui-kit.md`)
- [x] **Phase 3: 基于 `pi-grok-tui-kit` 实现 `@mcp` 选择器与 TS 版 `/resume` 会话恢复** (PR `docs/pr/pager/20261123-offload-mcp-picker-and-ts-resume.md`)

## Acceptance Criteria

- 所有新增与重构的 TS 模块拥有完整 `bun test` 单元测试且 100% 通过。
- 严格遵循 `AGENTS.md`：不运行本地 `cargo`，绝不暂存/提交 `pi-main` 子模块指针改动。
- 每个阶段产出独立、原子、清晰的 Git commit 与对应 Workhub PR 记录。
