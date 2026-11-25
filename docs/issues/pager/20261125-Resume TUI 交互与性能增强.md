---
id: "2026-11-25-Resume TUI 交互与性能增强"
title: "Resume TUI 交互与性能增强"
status: "done"
created: "2026-11-25"
updated: "2026-11-25"
category: "pager"
tags: ["workhub", "Resume TUI 交互与性能增强", "pi-grok-mentions", "pi-grok-tui-kit"]
---

# Issue: Resume TUI 交互与性能增强

## Goal

把 `/resume` 与相关 bundled TS overlay 的交互、性能和会话管理能力对齐 Pi 原版：快捷键可完整管理会话，搜索和滚动不卡，详情用 Pi Markdown 渲染，并支持内存级分割比例拖拽。

## 背景/问题

- `/resume` 会话列表把长标题截断成 `…`，视觉噪声大。
- 搜索只在 SQLite 全文查询外包了防抖，本地 `filterSessionRecords` 每次按键仍扫描 300 条完整正文（实测 300 × 64K 字符约 30ms/次）。
- 快速滚轮/触控板滚动 session details 会反复 wrap 全部分享消息。
- `@session` / `/resume` 缺少 Pi 原版的 sort / named / delete / path / rename 快捷键。
- session details 用自写 wrap，没有复用 Pi 的 Markdown 组件。
- `/resume` 默认全局 scope，不符合当前项目工作流的默认预期。
- `/review` 文件树把每一级路径单独渲染，深层会话路径形成一长串缩进。

## 验收标准 (Acceptance Criteria)

- [x] WHEN 会话标题超过列宽，系统 SHALL 直接裁掉多余字符，不显示 `…`。
- [x] WHEN 用户连续输入搜索词，系统 SHALL 只做一次本地过滤 + SQLite 全文查询，并显示 `searching`。
- [x] WHEN Enter 在防抖待执行时按下，系统 SHALL 立即 flush 搜索结果；Esc/关闭 SHALL 取消定时器。
- [x] WHEN 快速滚动详情，系统 SHALL 立即更新滚动位置，但合并渲染请求，并复用已 wrap 的预览行。
- [x] WHEN `/resume` 中按 `Ctrl+S/N/D/P/R`，系统 SHALL 分别执行 sort / named / delete / path / rename；删除需确认且禁止删当前会话；重命名只写 JSONL。
- [x] WHERE 全屏 `/resume`，系统 SHALL 在底部显示 `ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path (off/on) · ctrl+r rename`。
- [x] WHEN 渲染 session details 消息正文，系统 SHALL 使用 `@earendil-works/pi-tui` 的 `Markdown` 组件。
- [x] WHEN 打开 `/resume`，系统 SHALL 默认进入 project scope。
- [x] WHEN 拖动全屏左右分割线，系统 SHALL 调整比例并限制两侧最小宽度；比例仅保存在内存。
- [x] WHEN `/review` 文件树包含 6 级目录，系统 SHALL 每 3 级合成一个树节点（显示 2 个合并节点）。

## 实施阶段

### Phase 1: 规划和准备
- [x] 分析 pi-main 原版 keybindings 与 `session-selector.ts` 语义
- [x] 确认 PSM 的 JSONL `session_info` 命名格式与 trash/unlink 删除流程
- [x] 明确约束：不写 SQLite、不新增 tui-kit 文件、不跑 cargo

### Phase 2: 执行
- [x] 去掉 session picker 的截断省略号
- [x] 本地过滤 + SQLite 查询合并防抖，Enter flush / Esc cancel / searching 状态
- [x] `RenderCoalescer` 共享工具，推广到 file/skill/tree/review/session 的滚轮路径
- [x] file/skill/tree/review 预览行缓存
- [x] session details 注入 Pi `Markdown` renderer
- [x] `Ctrl+S/N/D/P/R` 快捷键、删除确认、重命名 JSONL、path/named/sort 状态
- [x] `/resume` 默认 project scope
- [x] 全屏分割线拖拽（内存级比例）
- [x] `/review` 长路径按 3 段合并显示

### Phase 3: 验证
- [x] 单元测试：`bun test extensions/pi-grok-tui-kit/ extensions/pi-grok-mentions/ extensions/pi-grok-tree/ extensions/pi-grok-review/`
- [x] `git diff --check`
- [x] 静态检查：无 SQLite 写语句、无 coding-agent 顶层值导入、无新增 tui-kit 文件

### Phase 4: 交付
- [x] 写入 workhub Issue/PR
- [ ] 合并主分支

## 关键决策

| 决策 | 理由 |
|------|------|
| 删除/重命名只改 JSONL，不写 SQLite | 用户明确要求；PSM 索引由它自己重扫，UI 内即时更新 |
| 用 `@earendil-works/pi-tui` 的 `Markdown`，主题从 overlay theme 映射 | coding-agent barrel 会被可选依赖 `lovely-mermaid` 拖炸；Markdown 本体在 pi-tui 中可直接复用 |
| `RenderCoalescer` 放在既有 `frame.ts` | 避免新增 tui-kit 文件触发 Rust injector 镜像断言 |
| `/resume` 默认 project，`@session` 保持 global | 用户只要求 resume 改默认值；mention 入口仍需跨项目搜索 |
| 分割比例只放组件字段 | 用户要求内存级，不落盘 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-11-25 | lazy import coding-agent 的 `getMarkdownTheme()` 被 `lovely-mermaid` 缺失打断，实际永远走 fallback | 改成顶层 import pi-tui 的 `Markdown`，用 overlay theme 构造 `MarkdownTheme` |
| 2026-11-25 | `globalCount/projectCount` 改为可变后 TS2564 未初始化 | 字段初始化为 0，构造函数内 `recountSessions()` |
| 2026-11-25 | 新 wheel 测试对 group 行正则误加 `$`，`fitWidth` 补空格导致 0 命中 | 正则去掉结尾 `$` |
| 2026-11-25 | Markdown 注入测试没注入 `loadPreview`，renderer 收不到真实消息文本 | 测试注入 previewMap；同时把终端行数调到 30 让渲染行进入可见窗口 |

## 相关资源

- [x] 相关 Issue: `docs/issues/pager/20261125-Resume TUI 交互与性能增强.md`
- [x] 相关 PR: `docs/pr/pager/20261125-Resume TUI 交互与性能增强.md`
- [x] 参考源码: `pi-main/packages/coding-agent/src/modes/interactive/components/session-selector.ts`
- [x] 参考源码: `pi-main/packages/coding-agent/src/core/keybindings.ts`
- [x] 参考源码: `pi-main/packages/tui/src/components/markdown.ts`

## Notes

本次任务跨度较大，采用两轮并行子代理分别处理 `file/skill` 和 `tree/review` 的滚轮合并/预览缓存；主代理负责 tui-kit、session picker、index 接线与 Markdown 集成。所有子代理产出均已由主代理复核 diff 与测试。

---

## Status 更新日志

- **2026-11-25**: 状态变更 → done，代码与测试全部完成，workhub 记录已写入。
