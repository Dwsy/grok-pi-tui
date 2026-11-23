---
id: "2026-11-23-file-picker-tree-fd-rg-tui"
title: "feat(mentions): 参考本地 NvChad 实现增强 @file 选择器（目录树 + 预览 + Nerd 图标 + fd 智能搜索 + rg 全文检索）"
status: "ready"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "pr", "mentions", "remote-tui", "file-picker", "fd", "rg", "nerd-icons"]
---

# feat(mentions): 参考本地 NvChad 实现增强 @file 选择器（目录树 + 预览 + Nerd 图标 + fd 智能搜索 + rg 全文检索）

> 参考本地 `~/.config/nvim/`（NvChad v2.5：`nvim-tree.lua` + `nvim-web-devicons` + `telescope.nvim`）交互与视觉风格，基于 Rust 通用 `@mention` 动态快捷键基座实现高性能 TS `@file` 文件选择器 TUI，整合目录树、文件/目录预览、Nerd Font 图标、基于 `fd` 的智能搜索与基于 `rg` 的全文检索。

## 背景与目的 (Why)

原有 `@` 文件补全仅提供单行扁平下拉列表，缺少目录层级浏览、代码行号预览、`rg` 全文检索以及类 Neovim 的 Nerd Font 文件图标体验。通过复用 `@mention` 动态快捷键基座，可在输入 `@file`、`@文件` 或高亮任意工作区文件行时按 `Ctrl+O` 一键唤起完整的三模式文件浏览器。

## 变更内容概述 (What)

1. **Rust 基座轻量适配**：
   - `crates/codegen/xai-grok-pager/src/views/file_search/state.rs`：`active_mention_action()` 新增文件行/`@file:` 回退分支，使裸 `@` 高亮文件行或显式 `@file:` / `@文件:` 均在底部动态置顶 `Ctrl+O:file explorer`。
   - `crates/codegen/xai-grok-pager/src/views/file_search/mentions.rs`：裸 `@` 时不额外重复展示 `file` header，仅在输入 `@file` / `@path` / `@文件` 时展示。
   - `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs`：将 `file-data.ts` 与 `file-picker-tui.ts` 加入 `TempDir` bundle 注入并补充测试断言。

2. **高性能数据层 (`extensions/pi-grok-mentions/file-data.ts`)**：
   - **Nerd Font 图标 (`getFileIcon`, `detectNerdFontSupport`)**：参考 NvChad `nvim-tree.lua`（`` / `` / `` / `󰈚`）与 `nvim-web-devicons` 常用扩展名/文件名图标，支持环境变量覆盖（`PI_GROK_NERD_ICONS` / `NERD_FONT`）、终端自动探测及 ASCII 优雅降级。
   - **懒展开目录树 (`buildVisibleTreeNodes`)**：仅对已展开目录按需调用 `fs.readdirSync`，目录优先、支持隐藏文件开关与树形缩进线（`│`/`├`/`└`）。
   - **基于 `fd` 的智能文件搜索 (`listWorkspaceFilesWithFd`, `filterFileEntries`)**：使用 `fd --color=never --strip-cwd-prefix --max-results 2000` 缓存工作区索引，键入时毫秒级多词模糊打分（文件名命中加权优于路径命中），`fd` 缺失时自动回退至受限 `fs` 遍历。
   - **基于 `rg` 的全文检索 (`grepWorkspaceWithRg`, `parseRgVimgrepOutput`)**：调用 `rg --vimgrep --smart-case`，输入未闭合正则时自动回退 `--fixed-strings`，支持点击/回车插入带行号的 `@path:line`。
   - **高性能预览 (`loadFileOrDirPreview`)**：支持文本文件（128 KB / 500 行上限 + 行号显示）、目录单层大纲与二进制 NUL 探测跳过。

3. **带边框三模式左右分栏 TUI (`extensions/pi-grok-mentions/file-picker-tui.ts`)**：
   - 参考 Telescope 水平布局（左侧 ~42% / 右侧 ~58% 预览 + 顶部 `` 搜索栏），完整圆角边框（`╭─╮│╰─╯` + `┬│┴`）。
   - 支持三模式切换：`[Tree]` / `[Files·fd]` / `[Grep·rg]`（`Tab` 或鼠标点击），以及 `[Hidden]`（`Ctrl+H`）与 `[Nerd]`（`Ctrl+Y`）实时开关。
   - `Tree` 模式下直接键入字符自动切入 `Files·fd` 快速搜索；`Grep·rg` 模式下右侧预览自动居中定位到命中行并以 `▸` 高亮。
   - 完整鼠标与键盘支持：点击模式/开关/关闭按钮、单击选中/展开目录、双击插入 `@path` 或 `@path:line`、左右两栏独立滚轮滚动。

## 测试与验证结果 (Test Result)

- [x] TS 单元测试通过：`bun test --cwd extensions/pi-grok-mentions`（23/23 通过，含 `file-picker.test.ts` 8 个用例、`skill-picker.test.ts` 7 个用例、`session-picker.test.ts` 8 个用例）
- [x] 格式与空白检查通过：`git diff --check`
- [ ] 本地 Cargo 编译/测试：按仓库 `AGENTS.md` 规范未在本地执行，已完成 Rust 改动静态核对。

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `crates/codegen/xai-grok-pager/src/views/file_search/mentions.rs` | 修改 | 裸 `@` 时跳过 `file` provider header，保持原有简洁列表 |
| `crates/codegen/xai-grok-pager/src/views/file_search/state.rs` | 修改 | `active_mention_action()` 支持文件行与 `@file:` 触发 `file` provider 动作 |
| `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs` | 修改 | 将 `file-data.ts` 与 `file-picker-tui.ts` 加入 bundle 注入 |
| `extensions/pi-grok-mentions/file-data.ts` | 新增 | Nerd 图标、懒展开目录树、`fd` 智能搜索、`rg` 全文检索与预览加载 |
| `extensions/pi-grok-mentions/file-picker-tui.ts` | 新增 | 三模式带边框左右分栏文件选择器 TUI（含鼠标与键盘交互） |
| `extensions/pi-grok-mentions/file-picker.test.ts` | 新增 | 文件选择器 TS 单元测试（8 个用例） |
| `extensions/pi-grok-mentions/index.ts` | 修改 | 注册 `file` provider `Ctrl+O` 动作与 `__pi_mention_file_picker` 命令 |
| `extensions/pi-grok-mentions/tsconfig.json` | 修改 | 纳入 `file-data.ts` 与 `file-picker-tui.ts` |

## 回滚方案 (Rollback Plan)

- 直接 `git revert <commit_sha>` 即可回退，零持久化状态或数据库变更。
