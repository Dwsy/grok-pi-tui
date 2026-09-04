---
id: "2026-09-04-持久化 Subagents V2 team 索引并展示嵌套代理树"
title: "持久化 Subagents V2 team 索引并展示嵌套代理树"
status: "done"
created: "2026-09-04"
updated: "2026-09-04"
category: "subagents"
tags: ["workhub", "持久化 Subagents V2 team 索引并展示嵌套代理树"]
---

# Issue: 持久化 Subagents V2 team 索引并展示嵌套代理树

## Goal

让 Subagents V2 的稳定 team 路径与 Pi 子会话 JSONL 建立持久索引，并让 Grok TUI/PSM 都能恢复完整父子树，同时保持子代理自身仍使用现有顶部代理视图。

## 背景/问题

当前 V2 `/root/...` topology 只保存在 `TeamCoordinator` 内存中；sidecar 虽记录 `childSessionFile`，却没有稳定 `agentPath/parentAgentPath/team`。此外 adapter 强制 bridge 的 `parentSessionId` 等于 root session，导致孙代理 lifecycle 被拒绝；Pager 对 child-session 通知也只按普通 child 内容处理，无法递归建立孙代理视图。PSM 的 grok-pi 插件因此既缺 durable team 索引，也没有 team tree 视图。

## 验收标准 (Acceptance Criteria)

- [x] WHEN V2 agent 被创建或恢复，sidecar SHALL 持久化其稳定 agent path、parent path、team（如有）与 child JSONL path。
- [x] WHEN 子代理再创建孙代理，adapter/Pager SHALL 将 lifecycle 路由到对应 child AgentView，并在主代理可见的 team 树中保留层级。
- [x] WHERE 用户打开某个子代理，子代理 SHALL 继续使用现有 AgentView 顶部代理样式，而不是被全局树替换。
- [x] WHEN PSM 打开 grok-pi session，插件 SHALL 能以 Pi 持久化索引递归发现 team，并提供 tree-style session view。
- [x] 验证 SHALL 使用 TypeScript/前端与目标 Rust 单测或静态检查的窄范围命令；不运行 Cargo，不清理缓存。

## 实施阶段

### Phase 1: 规划和准备
- [x] 分析 V2 runtime/bridge、adapter/Pager 与 PSM plugin 边界
- [x] 设计 sidecar durable index + live Pager run identity 双身份方案
- [x] 确定 root tree / child flat view 的 UI 分层

### Phase 2: 执行
- [x] V2 sidecar 写入 `agentPath` / `parentAgentPath` / `team`，bridge 单独携带 Pager parent run identity
- [x] adapter/Pager 支持递归 child-session lifecycle 与 descendant session routing
- [x] root Tasks pane 展示递归代理树；PSM grok-pi 插件新增 Team tree view

### Phase 3: 验证
- [x] `bun test ./v2.test.ts`：14 pass
- [x] bridge sidecar validation：1 pass；PSM plugin tests：8 pass
- [x] `pnpm run typecheck:extensions`、两仓库 `git diff --check`、Rust `rustfmt --emit stdout` 语法解析通过

### Phase 4: 交付
- [x] 更新 Issue / PR 变更记录
- [x] 创建 Workhub PR 记录
- [ ] 合并主分支（未请求，不执行）

## 关键决策

| 决策 | 理由 |
|------|------|
| 复用 `<session>.subagents.jsonl` 作为 durable team index，不新增数据库 | sidecar 已有 `childSessionFile`，只需补稳定 V2 path/team 字段即可递归发现，兼容旧 V1 记录 |
| 区分 Pi 持久 session id 与 Pager live child run id | 嵌套 bridge 必须路由到当前父 AgentView；持久索引仍保留真实 Pi parent/session JSONL 关系 |
| root Tasks pane 用树，child AgentView 保持现有 flat/top 样式 | 满足主代理总览层级，同时避免改变子代理交互模型 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-09-04 | adapter 仅接受 root `parentSessionId`，孙代理 lifecycle 被拒绝 | bridge 增加 live parent run identity；adapter 只接受 root 或已注册 child run 并按直接父 session 路由 |
| 2026-09-04 | Pager `SessionMatch::Child` 只处理一层且 child ext path 忽略 subagent lifecycle | 增加递归 descendant lookup，并将 nested spawn/progress/finish 应用到直接父 AgentView |
| 2026-09-04 | Bun 全套 bridge/runtime 测试受 socket fixture / `node:test` 嵌套限制干扰 | 改用目标 V2 与 sidecar validation 窄范围用例；未运行 Cargo |

## 相关资源

- [ ] 相关文档: `docs/architecture/xxx.md`
- [ ] 相关 Issue: `docs/issues/ISSUE-xxx.md`
- [ ] 参考资料: [链接]

## Notes

- PR 记录：`docs/pr/subagents/20260904-Subagents V2 team 持久索引与嵌套代理树.md`
- 外部配套变更：`~/Dev/AI/pi-session-manager/extensions/psm-grok-pi-tui/`
- 按用户明确限制未运行 Cargo，也未清理任何构建缓存；Rust 仅做源码语法解析，因此编译/类型检查留给有空间的环境。

---

## Status 更新日志

- **[2026-09-04]**: 状态变更 → done，备注: durable team index、nested Pager routing、root tree/child flat UI 与 PSM Team view 已实现；非 Cargo 验证通过。