---
id: "2026-11-23-skill-picker-ts-tui"
title: "feat(mentions): 复用动态快捷键基座实现 TS 插件化 @skill 技能选择器 TUI"
status: "ready"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "pr", "mentions", "remote-tui", "skill-picker"]
---

# feat(mentions): 复用动态快捷键基座实现 TS 插件化 @skill 技能选择器 TUI

> 零新增 Rust 协议，直接复用 `@mention` 动态快捷键与 `pi/ui/mention_insert` 基座，在 TS 扩展中实现带完整边框、左右分栏、支持搜索与 `SKILL.md` 预览、全局/项目切换（默认全局）及完整鼠标操作的 `@skill`（`@技能`）技能选择器 TUI。

## 背景与目的 (Why)

在完成 `@mention` 通用动态快捷键基座与 `@session` 会话选择器后，`@skill`（`@技能`）同样需要一个可视化、可搜索且能即时预览 `SKILL.md` 正文与目录资源的交互式选择器，验证“Rust 通用基座 + TS 插件化扩展”的可复用性。

## 变更内容概述 (What)

1. **TS 技能数据层 (`extensions/pi-grok-mentions/skill-data.ts`)**：
   - 直接复用 Pi 运行时注册的技能列表（零启动期磁盘扫描），按路径自动区分 `Project`（当前工作区目录内）与 `Global`（全局/用户目录）归属。
   - 提供 `parseSkillMarkdown()`（轻量解析 YAML frontmatter 并剥离正文）与 `loadSkillPreviewDetail()`（仅在选中某技能时懒加载 `SKILL.md`，统计行数、文件大小、修改时间并探测同级 `scripts/`、`references/`、`assets/` 资源数量）。
   - 提供 `filterSkillRecords()`，支持 Global / Project 作用域切换与多关键词加权排序（精确/前缀匹配 ID 优先，其次描述、路径与正文）。

2. **TS 技能选择器 TUI (`extensions/pi-grok-mentions/skill-picker-tui.ts`)**：
   - 完整圆角边框左右分栏布局（`╭─╮│╰─╯` + `┬│┴`），复用 `fitWidth` / `wrapTextToWidth` 保证 CJK 与 ANSI 样式下边框严格对齐。
   - 顶部提供可点击的 `[Global N]` / `[Project M]` 作用域切换标签（默认 **Global**）与 `[✕]` 关闭按钮。
   - 左侧展示技能列表（名称、`[Project]` / `[Global]` 标签、描述摘要）；右侧顶部展示完整元数据卡片（Scope、Path、Updated、Size、Extras、Desc），下方展示带标题高亮与搜索词反色高亮的可滚动 `SKILL.md` 正文预览。
   - 完整鼠标与键盘支持：单击选择、双击确认插入、点击切换 Global/Project、点击 `[✕]` 关闭、左右栏独立滚轮滚动。

3. **插件注册与 Bundle 注入**：
   - `extensions/pi-grok-mentions/index.ts`：为 `@skill` provider 声明 `action: { key: "ctrl+o", label: "skill picker", command: "__pi_mention_skill_picker" }`，注册隐藏命令并通过公共 `emitMentionInsert()` 回传原子 `@skill:<id>` chip。
   - `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs`：将 `skill-data.ts` 与 `skill-picker-tui.ts` 加入多文件 `TempDir` bundle 注入并补充断言。

## 测试与验证结果 (Test Result)

- [x] TS 单元测试通过：`bun test --cwd extensions/pi-grok-mentions`（15/15 通过，包含 `skill-picker.test.ts` 7 个用例与 `session-picker.test.ts` 8 个用例）
- [x] 格式与空白检查通过：`git diff --check`
- [ ] 本地 Cargo 编译/测试：按仓库 `AGENTS.md` 与用户要求未在本地执行，已完成 `mentions_extension.rs` 静态核对。

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `extensions/pi-grok-mentions/skill-data.ts` | 新增 | 技能作用域判定、搜索排序、frontmatter 解析与 `SKILL.md` 懒加载 |
| `extensions/pi-grok-mentions/skill-picker-tui.ts` | 新增 | 带边框左右分栏 `@skill` 技能选择器 TUI（含预览与鼠标交互） |
| `extensions/pi-grok-mentions/skill-picker.test.ts` | 新增 | 技能选择器 TS 单元测试（7 个用例） |
| `extensions/pi-grok-mentions/index.ts` | 修改 | 为 `@skill` 声明 `Ctrl+O` 动作、注册 `__pi_mention_skill_picker` 并抽离 `emitMentionInsert()` |
| `extensions/pi-grok-mentions/tsconfig.json` | 修改 | 纳入 `skill-data.ts` 与 `skill-picker-tui.ts` |
| `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs` | 修改 | 将新 TS 模块加入 `TempDir` bundle 注入与单元测试断言 |

## 回滚方案 (Rollback Plan)

- 直接 `git revert <commit_sha>` 即可回退，零 Rust 协议或状态迁移影响。
