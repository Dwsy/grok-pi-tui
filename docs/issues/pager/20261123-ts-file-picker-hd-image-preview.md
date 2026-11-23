---
id: "2026-11-23-ts-file-picker-hd-image-preview"
title: "TS @file 选择器高清图片直接预览（Kitty/iTerm2/Herdr 原生叠加层 + 高性能缓存 + 半块降级）"
status: "completed"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "issue", "image-preview", "herdr", "kitty", "iterm2", "ts-tui-kit"]
---

# TS `@file` 选择器高清图片直接预览

## Goal

在 TS 插件化的 `@file` 文件浏览器右栏直接渲染高清图片预览，兼顾高性能、零闪烁与多终端兼容（重点支持 **Herdr**、Kitty、Ghostty、WezTerm、iTerm2，并在不支持原生图形协议的终端上自动降级为 Truecolor 半块渲染）：

1. **Rust 基座高清图形叠加层协议**：
   - 在 `xai-grok-pager` 增加 `__pi_grok_remote_tui_image__` 控件通道（复用现有 `setWidget` 路由，无需改 `pi-grok-adapter`），支持 TS Remote TUI 弹窗声明相对坐标的图片叠加区域 `{ path, x, y, width, height }`。
   - 在 `draw_remote_tui_overlay` 后复用 `overlay::static_centered_read_viewer` + `inline_media_cache` (64MB 有界缓存) + `MediaFileStamp` 负缓存，实现稳态零重传、切图/关闭自动清理（含 iTerm2 网格重绘）。
   - 在 `xai-grok-pager-render` 的 `scrollback_inline_overlay_active` 中补充放行 **Herdr** (`MultiplexerKind::Herdr` + `GraphicsProtocol::Kitty`)。
2. **共享 TS 图片模块 (`extensions/pi-grok-tui-kit/image.ts`)**：
   - 零依赖快速二进制头部解析 (`probeImageMetadata`)，读取前 64KB 毫秒级提取 PNG / JPEG / GIF / WebP / BMP / SVG 分辨率与格式。
   - 终端图形协议探测 (`detectTerminalImageProtocol`)，识别 **Herdr** (`HERDR_ENV` / `TERM_PROGRAM=herdr`)、Kitty、Ghostty、WezTerm、Warp、iTerm2。
   - Truecolor `▀` 半块降级渲染 + LRU 缓存，确保无图形协议终端也能流畅预览。
3. **`@file` 集成与测试**：
   - `file-data.ts` 识别图片文件返回 `kind: "image"` 元数据。
   - `file-picker-tui.ts` 在右栏展示图片分辨率/格式/协议徽标，并按终端能力发射高清叠加层或绘制 Truecolor 半块预览；切换非图片文件或关闭弹窗时自动清理。

## Phases

- [x] **Phase 1: 调研图形叠加层、Herdr 协议矩阵与 Remote TUI 渲染链路**
- [x] **Phase 2: 扩展 Rust `xai-grok-pager` / `xai-grok-pager-render` 支持 Remote TUI 高清图片叠加层与 Herdr**
- [x] **Phase 3: 实现 `pi-grok-tui-kit/image.ts` 并接入 `@file` 浏览器与 Rust 内嵌包**
- [x] **Phase 4: 补齐 TS 单元测试、Workhub PR、CHANGELOG 并提交** (`docs/pr/pager/20261123-ts-file-picker-hd-image-preview.md`)
