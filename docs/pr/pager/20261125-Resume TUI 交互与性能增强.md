---
id: "2026-11-25-Resume TUI 交互与性能增强"
title: "Resume TUI 交互与性能增强"
status: "review"
created: "2026-11-25"
updated: "2026-11-25"
category: "pager"
tags: ["workhub", "pr", "Resume TUI 交互与性能增强"]
---

# Resume TUI 交互与性能增强

> 对齐 Pi 原版会话快捷键，修复搜索/滚动卡顿，复用 Pi Markdown，并给 `/resume` 加内存级分割线拖拽。

## 背景与目的 (Why)

`/resume` 与相关 bundled TS overlay 在实际高频使用下暴露几个问题：长标题省略号难看、输入搜索词时本地全量扫描导致卡顿、快速滚轮滚动详情反复 wrap、缺少 Pi 原版的会话管理快捷键、详情正文没有复用 Pi Markdown、`/resume` 默认全局 scope 与项目工作流不符、`/review` 深层路径树缩进过长。

## 变更内容概述 (What)

- 会话列表截断去掉 `…`。
- 搜索防抖覆盖本地过滤 + SQLite FTS，加入 `searching` 状态、Enter flush、Esc/关闭取消。
- 新增共享 `RenderCoalescer`，file/skill/tree/review/session 的右侧滚轮合并渲染；各 overlay 按目标+宽度缓存预览行。
- session details 用 `Markdown` from `@earendil-works/pi-tui` 渲染，主题由 overlay theme 映射，未新增 coding-agent 顶层值导入。
- `Ctrl+S/N/D/P/R`：sort 循环、named 过滤、删除确认、path 开关、rename；删除/重命名只写 JSONL，不写 SQLite。
- `/resume` 默认 project scope；全屏分割线支持拖动，比例只保存在内存。
- `/review` 文件树每 3 级路径合并为一个目录节点。
- 取消 300 条会话上限并优化万级会话加载/过滤/滚动性能（精简全量加载 + DB/WAL 指纹缓存 + 右栏懒加载详情 + `WeakMap` 搜索索引 + O(1) 行映射）。
- 新增跨平台、懒扫描、高性能的 `sessions-lite.db` 回退索引（仅在未安装 PSM 无 `sessions.db` 时启用，头尾定界探测 + 增量同步 + 损坏自愈，绝不写入 PSM `sessions.db`）。

## 关联 Issue

- **Issue:** `docs/issues/pager/20261125-Resume TUI 交互与性能增强.md`

## 测试与验证结果 (Test Result)

- [x] 单元测试通过：`bun test extensions/pi-grok-tui-kit/ extensions/pi-grok-mentions/ extensions/pi-grok-tree/ extensions/pi-grok-review/` → **67 pass, 0 fail, 682 expects**
- [x] `git diff --check` 通过
- [x] 静态检查：PSM `sessions.db` 保持严格只读（`sessions-lite.db` 仅在无 `sessions.db` 时独立维护）；无 coding-agent 顶层值导入；无新增 tui-kit 文件
- [ ] 手动回归测试通过（需人工在真实终端确认快捷键手感与拖拽体验）

## 风险与影响评估 (Risk Assessment)

- 删除会话会调用 `trash` 并回退 `unlink`，属于用户显式确认后的破坏性操作；当前会话路径已阻止删除。
- 重命名写入 JSONL `session_info` 条目；PSM SQLite 索引不会立即更新，由 PSM 自身重新索引后再反映到跨进程列表。当前 UI 内已即时更新。
- 会话列表/详情的 `…` 被完全移除，用户失去“被截断”的显式提示。
- `/resume` 默认 scope 从 global 变为 project，跨项目查找需要 Tab 切换。
- 分割线比例不持久化，重开 overlay 后恢复默认 64%。

## 回滚方案 (Rollback Plan)

1. 先 `git status --short` 确认工作区状态。
2. 回退本 PR 涉及的 TS 文件与测试文件（见下表），必要时使用 `git restore <具体文件>`；不要使用 `git restore .`。
3. 删除 workhub 记录：`docs/issues/pager/20261125-Resume TUI 交互与性能增强.md`、`docs/pr/pager/20261125-Resume TUI 交互与性能增强.md`。
4. 重跑 `bun test extensions/pi-grok-tui-kit/ extensions/pi-grok-mentions/ extensions/pi-grok-tree/ extensions/pi-grok-review/` 与 `git diff --check`。

## 变更类型

- [x] 🐛 Bug Fix
- [x] ✨ New Feature
- [ ] 📝 Documentation
- [ ] 🚀 Refactoring
- [x] ⚡ Performance
- [ ] 🔒 Security
- [x] 🧪 Testing

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `extensions/pi-grok-tui-kit/frame.ts` | 修改 | 新增 `RenderCoalescer` |
| `extensions/pi-grok-tui-kit/index.ts` | 修改 | 导出 `RenderCoalescer` |
| `extensions/pi-grok-tui-kit/kit.test.ts` | 修改 | 合并与 cancel 测试 |
| `extensions/pi-grok-mentions/session-data.ts` | 修改 | `name`/`score` 字段、nameFilter、JSONL helper、取消 300 条上限与性能优化、`sessions-lite.db` 自动回退 |
| `extensions/pi-grok-mentions/session-lite-db.ts` | 新增 | 跨平台懒扫描 `sessions-lite.db` 增量索引与按需详情缓存 |
| `extensions/pi-grok-mentions/session-picker-tui.ts` | 修改 | 省略号、防抖、快捷键盘、Markdown、拖拽分隔线、滚轮缓存、右栏懒详情与 O(1) 行定位 |
| `extensions/pi-grok-mentions/session-picker.test.ts` | 修改 | 会话选择器全量回归用例 |
| `extensions/pi-grok-mentions/index.ts` | 修改 | renderer/currentSessionPath/skipMissingFiles 接线，resume 默认 project |
| `extensions/pi-grok-mentions/file-picker-tui.ts` | 修改 | 右侧滚轮渲染合并 |
| `extensions/pi-grok-mentions/file-picker.test.ts` | 修改 | 滚轮合并测试 |
| `extensions/pi-grok-mentions/skill-picker-tui.ts` | 修改 | 预览行缓存 + 滚轮合并 |
| `extensions/pi-grok-mentions/skill-picker.test.ts` | 修改 | 滚轮合并测试 |
| `extensions/pi-grok-tree/tree-tui.ts` | 修改 | 预览行缓存 + 滚轮合并 |
| `extensions/pi-grok-tree/tree.test.ts` | 修改 | 滚轮合并测试 |
| `extensions/pi-grok-review/review-tui.ts` | 修改 | 预览行缓存 + 滚轮合并 |
| `extensions/pi-grok-review/review.test.ts` | 修改 | 滚轮合并 + 路径合并测试 |
| `extensions/pi-grok-review/review-data.ts` | 修改 | 文件树每 3 段合并 |
| `docs/issues/pager/20261125-Resume TUI 交互与性能增强.md` | 新增 | Issue 记录 |
| `docs/pr/pager/20261125-Resume TUI 交互与性能增强.md` | 新增 | PR 记录 |

## 详细变更说明

### 1. 搜索与滚动性能

**问题：** 每次按键/滚轮都会重算大块内容，快速输入或滚动明显卡顿。

**方案：** 本地过滤与 SQLite 查询合成一次 150ms 防抖；滚轮状态立即更新、渲染用 `RenderCoalescer` 合并；预览行按目标+宽度缓存，滚动只 slice。

**影响范围：** `/resume`、`@session`、`@file`、`@skill`、`/tree`、`/review`。

### 2. 会话管理与 Pi 对齐

**问题：** `/resume` 缺少 Pi 原版的 sort/named/delete/path/rename，且截断省略号干扰阅读。

**方案：** 按 `pi-main` 的 keybindings 与 `session-selector.ts` 语义实现五个快捷键；删除需确认并阻止删当前会话；重命名和删除只写 JSONL；列表截断不再输出 `…`。

**影响范围：** `/resume` 与 `@session` 的 session picker。

### 3. Markdown 与布局

**问题：** session details 自写 wrap，且左右比例固定。

**方案：** 注入 pi-tui `Markdown` 组件渲染消息正文；全屏分割线支持 press/drag/release，比例仅内存保存并做最小宽度钳制。

**影响范围：** `/resume` 全屏 session details。

## 测试命令

```bash
cd /Users/dengwenyu/Dev/AI/pi-grok-build
bun test extensions/pi-grok-tui-kit/ extensions/pi-grok-mentions/ extensions/pi-grok-tree/ extensions/pi-grok-review/
git diff --check
```

## 破坏性变更

**是否有破坏性变更？**

- [x] 否

说明：变更均为 UI 行为增强；`/resume` 默认 scope 改变属于默认值调整，不破坏调用接口。

## 性能影响

**是否有性能影响？**

- [x] 提升 - 搜索按键从多次全量扫描降为一次防抖；滚轮只 slice 缓存行；渲染请求合并。

## 依赖变更

**是否引入新的依赖？**

- [x] 否

说明：复用仓库已有的 `@earendil-works/pi-tui`。

## 安全考虑

**是否有安全影响？**

- [x] 否

说明：删除有二次确认且禁止删当前会话；未触及凭据和 token。

## 文档变更

**是否需要更新文档？**

- [x] 是 - 本 PR 与关联 Issue 由 workhub 创建，作为本次变更的 SSOT 记录。

## 代码审查检查清单

### 功能性
- [x] 代码实现了需求
- [x] 边界情况已处理
- [x] 错误处理完善

### 代码质量
- [x] 代码遵循项目规范
- [x] 变量命名清晰
- [x] 没有冗余代码

### 测试
- [x] 有对应的单元测试
- [x] 测试覆盖关键路径
- [x] 测试通过

## 审查日志

- **2026-11-25 [Alma]**: 自检通过；63 个测试全绿，`git diff --check` 干净，无 SQLite 写入。

## 最终状态

- **合并时间:** 待定
- **合并人:** 待定
- **Commit Hash:** 待定
- **部署状态:** 待部署
