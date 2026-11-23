---
id: "2026-11-23-review-session-message-ts-plugin"
title: "refactor(review): 将 /review-session 与 /review-message 迁移为精简独立 TS 插件并共享文件库"
status: "ready"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "pr", "review", "remote-tui", "ts-plugin"]
---

# refactor(review): 将 /review-session 与 /review-message 迁移为精简独立 TS 插件并共享文件库

> 将 `grok-pi` 下的 `/review-session` 与 `/review-message` 迁移为独立 TS 扩展 `extensions/pi-grok-review/`，直接复用 `extensions/pi-grok-mentions/` 的 Nerd 图标、文件预览与边框排版基础能力，并对冗余杂乱的快捷键做彻底减法。

## 背景与目的 (Why)

原有 Rust 原生 review 模态框承载了过多复杂快捷键与多层嵌套状态，交互心智负担重且迭代成本高。借本次 `@mention` 文件库与 TS Remote TUI 基座落地的机会，将 `/review-session` 与 `/review-message` 拆分为独立、精简的 TS 插件，统一视觉语言与交互范式。

## 变更内容概述 (What)

1. **独立 TS 插件 (`extensions/pi-grok-review/`)**：
   - **共享基础库**：直接引用 `../pi-grok-mentions/file-data.ts`（`getFileIcon`、`detectNerdFontSupport`、`loadFileOrDirPreview`）与 `../pi-grok-mentions/session-picker-tui.ts`（`fitWidth`），零重复造轮子。
   - **数据提取 (`review-data.ts`)**：解析当前会话 JSONL 中的 `edit`/`write` 工具调用并按用户轮次（Turn）分组，计算紧凑 hunk diff；若会话尚无工具编辑，`/review-session` 自动回退至 `git diff` 工作区改动；提供变更文件目录树构建（`buildReviewFileTree`）。
   - **极简做减法 TUI (`review-tui.ts`)**：圆角边框左右分栏布局，去掉杂乱快捷键，只保留核心操作：
     - `↑`/`↓` / 单击 / 滚轮：切换轮次或文件
     - `Tab` / 点击 `[Diff]`/`[File]`：切换 Diff 与完整文件预览
     - `t` / 点击 `[Tree]`/`[List]`：切换变更文件树与扁平列表
     - `Enter` / `c` / 双击文件：单行内联写评审批注
     - `Ctrl+S` / 点击 `[Send N]`：将所有批注格式化为 `@<path>: <note>` 发送给 Agent
     - `Esc` / 点击 `[✕]`：退出或返回轮次列表
   - **命令注册 (`index.ts`)**：同时注册公开命令（`/review-session`、`/review-message`，兼容独立 `pi`）与隐藏桥接命令（`__pi_review_session`、`__pi_review_message`）。

2. **Rust 路由与注入 (`dispatch/review.rs`, `grok-pi.rs`, `review_extension.rs`)**：
   - `dispatch_review_show_session` / `dispatch_review_show_message_picker` 在 `app.external_agent == true` 时通过 `Effect::MentionAction` 路由至 TS 插件，同时保留 stock Grok（`external_agent == false`）路径与全部既有测试绿灯。
   - 新增 `grok_pi/review_extension.rs`，在同一 `TempDir` 中同步物化 `pi-grok-review/` 与共享的 `pi-grok-mentions/` 模块，并在 `grok-pi.rs` 注册注入。

## 测试与验证结果 (Test Result)

- [x] TS 单元测试通过：`bun test --cwd extensions/pi-grok-review`（4/4 通过）与 `bun test --cwd extensions/pi-grok-mentions`（23/23 通过）
- [x] 格式与空白检查通过：`git diff --check`
- [ ] 本地 Cargo 编译/测试：按仓库 `AGENTS.md` 规范未在本地执行，已完成 Rust 改动静态核对。

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `extensions/pi-grok-review/index.ts` | 新增 | 独立 TS 插件入口，注册 `/review-session`、`/review-message` 及桥接命令 |
| `extensions/pi-grok-review/review-data.ts` | 新增 | 会话 JSONL `edit`/`write` 提取、轮次分组、hunk diff、git diff 回退与文件树 |
| `extensions/pi-grok-review/review-tui.ts` | 新增 | 精简带边框左右分栏 Review TUI（共享 Nerd 图标与文件预览，支持全鼠标与内联批注） |
| `extensions/pi-grok-review/review.test.ts` | 新增 | Review TS 单元测试（4 个用例） |
| `extensions/pi-grok-review/tsconfig.json` | 新增 | TS 类型检查配置 |
| `crates/codegen/xai-grok-pager/src/app/dispatch/review.rs` | 修改 | `external_agent` 下将 `/review-session` 与 `/review-message` 路由至 TS 插件 |
| `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/review_extension.rs` | 新增 | 物化 `pi-grok-review` 与共享 `pi-grok-mentions` 模块的 `TempDir` bundle |
| `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` | 修改 | 注入 `pi-grok-review` 扩展并保持生命周期 |

## 回滚方案 (Rollback Plan)

- 直接 `git revert <commit_sha>` 即可回退至原 Rust review 模态框，零持久化数据影响。
