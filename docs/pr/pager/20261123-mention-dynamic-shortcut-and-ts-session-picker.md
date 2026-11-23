---
id: "2026-11-23-mention-dynamic-shortcut-and-ts-session-picker"
title: "feat(mentions): 支持 @xx 动态快捷键提示与 TS 插件化会话选择器 TUI"
status: "ready"
created: "2026-11-23"
updated: "2026-11-23"
category: "pager"
tags: ["workhub", "pr", "mentions", "remote-tui", "session-picker"]
---

# feat(mentions): 支持 @xx 动态快捷键提示与 TS 插件化会话选择器 TUI

> 使用 Rust 作为通用基座，通过 TS 插件化能力实现 `@xx` 补全高亮时底部动态展示快捷键；以 `@session`（`@会话`）为例，按 `Ctrl+O` 唤起带完整边框、左右分栏、支持搜索与预览且支持鼠标操作的 TS 会话选择器 TUI。

## 背景与目的 (Why)

目前输入 `@session` / `@会话` 时，下拉列表仅展示最近 50 条会话的精简标题，缺少完整会话元数据、消息内容搜索与会话预览能力。同时，Rust 基座不应硬编码具体对象类型（如 `@session`），需要保持“Rust 提供通用协议与渲染基座、TS 扩展声明具体提供者与交互 UI”的架构边界。

## 变更内容概述 (What)

1. **Rust 基座（通用 `@mention` 动态快捷键与回调插槽）**：
   - 在 `MentionProvider` 新增可选 `action: Option<MentionProviderAction>`（`{ key, label, command }`），由 TS 插件按需声明。
   - `FileSearchState::active_mention_action()` 在 `@` 下拉可见且选中带 `action` 的 provider header/item 或处于 `@<kind>:<query>` 作用域时返回当前快捷键动作。
   - 底部快捷键栏（`views/agent.rs` `build_hints`）动态置顶显示该快捷键（如 `Ctrl+O:session picker`），并避免与低优先级同键位提示冲突。
   - `AppView` 在 `@` 下拉可见时优先拦截匹配的 provider 快捷键，通过 `Effect::MentionAction` → `pi/ui/mention_action` → `pi-grok-adapter` RPC 调用对应隐藏斜杠命令。
   - 新增 `__pi_grok_mention_insert__` → `pi/ui/mention_insert` 回传通道，`PromptWidget::insert_mention_chip()` 将 TS 选择器选中的对象原子替换当前 `@...` token 为样式化 chip。
   - 将 `mentions_extension.rs` 升级为多文件 `TempDir` bundle 注入（包含 `index.ts`、`session-data.ts`、`session-picker-tui.ts`）。

2. **TS 插件（`extensions/pi-grok-mentions/` `@session` 会话选择器 TUI）**：
   - `session-data.ts`：只读查询 PSM SQLite (`~/.pi/agent/sessions/sessions.db`) 并 `LEFT JOIN session_details_cache`，按需回退 JSONL；**搜索与预览均严格只包含用户与 agent 消息，隐藏工具调用 (`toolCall`/`tool_use`/`toolResult`) 与思考 (`thinking`/`redacted_thinking`/`<think>`)**；Token 统一格式化为 `M` 单位。
   - `session-picker-tui.ts`：基于 `ctx.ui.custom(..., { overlay: true, overlayOptions })` 实现带完整圆角边框（`╭─╮│╰─╯` + `┬│┴`）的左右分栏 TUI：
     - 顶部支持 **Global / Project** 切换（默认 **Global**）与 `[✕]` 关闭按钮。
     - 左侧会话列表展示标题、相对时间、项目、短 ID、消息数、Token（`M` 单位）、模型或命中片段。
     - 右侧元数据卡片完整展示：**项目 (Project)、ID、创建时间 (Created)、更新时间 (Updated)、会话消息数量 (Msgs，含 U/A 分布)、Token（M 单位）、模型 (Model)**，下方为可滚动的 User/Agent 对话预览（支持搜索词高亮）。
     - 完整鼠标支持：点击切换 Global/Project、点击 `[✕]` 关闭、单击左侧选择会话、双击左侧确认插入、左右两栏独立鼠标滚轮滚动。
   - `index.ts`：为 `@session` provider 声明 `action: { key: "ctrl+o", label: "session picker", command: "__pi_mention_session_picker" }`，注册隐藏命令并在选中后通过 `__pi_grok_mention_insert__` 插入 `@session:<shortId>` chip。

## 关联 Issue

- **Issues:** N/A（用户直接发起的 Workhub PR 任务）

## 测试与验证结果 (Test Result)

- [x] TS 单元测试通过：`bun test --cwd extensions/pi-grok-mentions session-picker.test.ts`（8/8 通过，79 项断言）
- [x] TS 类型检查通过：`./pi-main/node_modules/.bin/tsc --noEmit -p extensions/pi-grok-mentions/tsconfig.json`
- [x] 格式与空白检查通过：`git diff --check`
- [ ] 本地 Cargo 编译/测试：按仓库 `AGENTS.md` 与用户要求未在本地执行，已完成全量静态核对（结构体字面量、导入、match 分支与类型签名），后续可通过 GitHub Actions `pi-slim-linux-checks.yml` 在线验证。

## 风险与影响评估 (Risk Assessment)

- **低风险**：Rust 基座变更完全向后兼容，`MentionProvider.action` 使用 `#[serde(default)]`，未声明 `action` 的既有 `@skill` / `@mcp` provider 行为零变化。
- 快捷键 `Ctrl+O` 仅在 `@` 补全下拉可见且高亮/处于 `@session` 时拦截，不影响正常编辑态或其他视图快捷键。

## 回滚方案 (Rollback Plan)

- 直接 `git revert <commit_sha>` 即可完整回退 Rust 基座与 TS 扩展改动，无数据库 schema 变更或持久化配置迁移。

## 变更类型

- [x] ✨ New Feature
- [x] 🧪 Testing

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `crates/codegen/xai-grok-pager/src/views/file_search/mentions.rs` | 修改 | 新增 `MentionProviderAction` 与 provider 查找辅助函数 |
| `crates/codegen/xai-grok-pager/src/views/file_search/mod.rs` | 修改 | 导出 `MentionProviderAction` |
| `crates/codegen/xai-grok-pager/src/views/file_search/state.rs` | 修改 | 新增 `FileSearchState::active_mention_action()` |
| `crates/codegen/xai-grok-pager/src/app/extension_shortcuts.rs` | 修改 | 新增 `parse_key_id_to_shortcut()` |
| `crates/codegen/xai-grok-pager/src/views/agent.rs` | 修改 | `build_hints()` 动态置顶展示当前 `@xx` provider 快捷键 |
| `crates/codegen/xai-grok-pager/src/app/actions.rs` | 修改 | 新增 `Effect::MentionAction` |
| `crates/codegen/xai-grok-pager/src/app/app_view.rs` | 修改 | `@` 下拉可见时优先拦截 provider 快捷键并派发 `Effect::MentionAction` |
| `crates/codegen/xai-grok-pager/src/app/effects/mod.rs` | 修改 | 发送 `pi/ui/mention_action` ACP 扩展通知 |
| `crates/codegen/xai-grok-pager/src/app/acp_handler/mod.rs` | 修改 | 处理 `pi/ui/mention_insert` 回调 |
| `crates/codegen/xai-grok-pager/src/views/prompt_widget/mod.rs` | 修改 | 新增 `PromptWidget::insert_mention_chip()` |
| `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` | 修改 | 新增 `EXTENSION_MENTION_INSERT_STATUS_KEY` |
| `crates/codegen/pi-grok-adapter/src/pi_adapter/agent.rs` | 修改 | 处理 `pi/ui/mention_action` 并通过 RPC 触发隐藏命令 |
| `crates/codegen/pi-grok-adapter/src/pi_adapter/tools.rs` | 修改 | 转发 `__pi_grok_mention_insert__` 为 `pi/ui/mention_insert` |
| `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs` | 修改 | 升级为多文件 `TempDir` bundle 注入 |
| `extensions/pi-grok-mentions/index.ts` | 修改 | 为 `@session` 声明 `Ctrl+O` 动作并注册会话选择器命令 |
| `extensions/pi-grok-mentions/session-data.ts` | 新增 | PSM SQLite 只读加载、JSONL 回退、隐藏工具/思考过滤、Token `M` 单位格式化与搜索 |
| `extensions/pi-grok-mentions/session-picker-tui.ts` | 新增 | 带边框左右布局会话选择器 TUI（搜索、预览、Global/Project 切换、鼠标与键盘交互） |
| `extensions/pi-grok-mentions/session-picker.test.ts` | 新增 | TS 单元测试套件（8 个测试用例） |
| `extensions/pi-grok-mentions/tsconfig.json` | 新增 | TS 类型检查配置 |

## 测试命令

```bash
# 运行 TS 单元测试
bun test --cwd extensions/pi-grok-mentions session-picker.test.ts

# 运行 TS 类型检查
./pi-main/node_modules/.bin/tsc --noEmit -p extensions/pi-grok-mentions/tsconfig.json

# 检查空白与 diff 格式
git diff --check
```

## 破坏性变更

- [x] 否

## 性能影响

- [x] 无影响（会话列表直接走 PSM SQLite 只读索引，预览按需懒加载并缓存，不扫描磁盘 JSONL）

## 依赖变更

- [x] 否

## 安全考虑

- [x] 否（`pi/ui/mention_action` 对 command 名称校验仅允许 ASCII 字母、数字、下划线与连字符；SQLite 以只读模式打开）
