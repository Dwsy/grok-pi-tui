---
id: "2026-09-04-Subagents V2 team 持久索引与嵌套代理树"
title: "Subagents V2 team 持久索引与嵌套代理树"
status: "ready"
created: "2026-09-04"
updated: "2026-09-04"
category: "subagents"
tags: ["workhub", "pr", "Subagents V2 team 持久索引与嵌套代理树"]
---

# Subagents V2 team 持久索引与嵌套代理树

> 将 Subagents V2 的稳定 team 路径写入现有 Pi sidecar 索引，修复孙代理的 live Pager 路由，并在主代理/PSM 提供递归 team 树，同时保持子代理现有 AgentView 样式。

## 背景与目的 (Why)

V2 team topology 原先只保存在 `TeamCoordinator` 内存中；sidecar 只有 child JSONL 文件，PSM 无法从磁盘恢复 `/root/...` team 层级。与此同时，adapter 把 bridge `parentSessionId` 锁死为 root，而 Pager 的 child-session 路由也只支持一层，导致孙代理 lifecycle/内容无法正确进入直接父子代理视图。

## 变更内容概述 (What)

- 在现有 `.subagents.jsonl` 快照中增加可选 `agentPath`、`parentAgentPath`、`team`，保留 V1 兼容性。
- 将 Pi 持久 session identity 与 Pager live child-run identity 分离；nested bridge 只允许 root 或已注册 child-run 作为直接父会话。
- Pager 支持递归 descendant session lookup 与 nested spawn/progress/finish；主 AgentView 的 Tasks/Subagents 面板显示树，child AgentView 继续使用原有 flat/top 样式。
- `~/Dev/AI/pi-session-manager` 的 `psm-grok-pi-tui` 插件递归读取 sidecar/`childSessionFile`，新增 Team session-tree view。

## 关联 Issue

- **Issue:** `docs/issues/subagents/20260904-持久化 Subagents V2 team 索引并展示嵌套代理树.md`

## 测试与验证结果 (Test Result)
- [x] Subagents V2 目标测试：14 pass / 0 fail
- [x] sidecar 新字段 validation：1 pass / 0 fail
- [x] PSM grok-pi Team + protocol：8 pass / 0 fail
- [x] `pnpm run typecheck:extensions` 通过
- [x] 两仓库 `git diff --check` 通过；编辑的 Rust 文件经 `rustfmt --emit stdout` 做语法解析通过
- [x] 按用户要求未运行任何 Cargo 命令，也未清理缓存

## 风险与影响评估 (Risk Assessment)

主要风险位于 nested live routing：持久 Pi session id 不能直接代替 Pager 当前 run id。本实现把二者显式分离，并在 adapter 侧只接受 root 或已知 child-run，避免任意 session id 注入 Pager。PSM 仅使用现有 `sessions:read` 权限递归读取相关 sidecar，不新增数据库或网络接口。由于禁止 Cargo，本次未进行 Rust 类型检查/编译验证。

## 回滚方案 (Rollback Plan)

可分别回退三层：移除 V2 sidecar 可选 team 字段/bridge live parent identity；恢复 adapter/Pager 一层 child 路由；移除 PSM Team view 注册与读取模块。sidecar 新字段为可选字段，回退后历史文件仍可由旧读取逻辑忽略。

---

## 变更类型

- [x] 🐛 Bug Fix
- [x] ✨ New Feature
- [x] 📝 Documentation
- [ ] 🚀 Refactoring
- [ ] ⚡ Performance
- [ ] 🔒 Security
- [x] 🧪 Testing

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `extensions/pi-grok-subagents/{bridge,runtime,v2}.ts` | 修改 | 持久化 V2 team identity，并使用直接父 child-run 作为 nested live bridge identity |
| `crates/codegen/pi-grok-adapter/src/{subagent_projection.rs,pi_adapter/notifications.rs}` | 修改 | 允许已知 child-run 作为 nested parent，并按直接父会话投递 lifecycle/tool metadata |
| `crates/codegen/xai-grok-pager/src/app/...`、`views/tasks_pane.rs` | 修改 | 递归 descendant routing/lifecycle；root Tasks tree，child view 保持 flat |
| `extensions/pi-grok-subagents/{v2,bridge}.test.ts` | 修改 | 覆盖 nested parent identity 与 sidecar team 字段 |
| `~/Dev/AI/pi-session-manager/extensions/psm-grok-pi-tui/*` | 修改/新增 | 从 sidecar 递归恢复 team 并显示 Team tree view |
| `docs/issues/subagents/...`、本 PR | 新增 | L3 SSOT 任务与变更记录 |

## 详细变更说明

### 1. Durable V2 team index

**问题：** PSM 只能找到 child JSONL，无法知道稳定 `/root/...` team path。

**方案：** 在现有 append-only sidecar 快照上增加可选 V2 identity 字段，并让 PSM 沿 `childSessionFile` 递归读取，不新增持久层。

**影响范围：** Subagents V2 sidecar、PSM grok-pi 插件；V1 sidecar 不受影响。

### 2. Nested Pager routing

**问题：** adapter 只接受 root parent，Pager 只匹配直接 child，孙代理 lifecycle/内容被丢弃或落错视图。

**方案：** bridge 使用直接父 AgentView 的 live child-run id；adapter 验证 known parent；Pager 递归查找 descendant，并把 nested lifecycle 写到直接父 AgentView。

**影响范围：** pi-grok adapter 与 Pager 子代理 session routing。

### 3. Root tree / child flat UI

**问题：** 主代理缺少完整 team 层级，但对子代理页面直接套全局树会破坏现有交互。

**方案：** root Tasks pane flatten recursive hierarchy 为 `├─/└─` tree rows；child AgentView 仍走现有 `TasksPane.sync()` flat 路径。PSM 提供单独 Team tree view。

**影响范围：** Pager Tasks/Subagents 展示、PSM session tree view。

## 测试命令

```bash
cd extensions/pi-grok-subagents
bun test ./v2.test.ts
bun test --test-name-pattern "sidecar validation" bridge.test.ts

cd ~/Dev/AI/pi-session-manager
pnpm exec vitest run extensions/psm-grok-pi-tui/team.test.ts extensions/psm-grok-pi-tui/protocol.test.ts
pnpm run typecheck:extensions

# 两仓库分别执行
git diff --check

# 仅语法解析编辑过的 Rust 文件；不构建、不运行 Cargo
rustfmt --edition 2024 --emit stdout <file.rs> >/dev/null
```

## 破坏性变更

**是否有破坏性变更？**

- [x] 否
- [ ] 是 - [描述破坏性变更及迁移指南]

## 性能影响

**是否有性能影响？**

- [x] 无显著影响；PSM Team view 仅在打开视图时沿已知 childSessionFile 递归读取 sidecar
- [ ] 提升 - [描述性能提升]
- [ ] 下降 - [描述性能下降及原因]

## 依赖变更

**是否引入新的依赖？**

- [x] 否
- [ ] 是 - [列出新增依赖及理由]

## 安全考虑

**是否有安全影响？**

- [x] 否；仅新增 PSM `sessions:read` 权限读取用户已选 session 的 sidecar，adapter 仍校验 nested parent 必须是 root 或已知 child-run
- [ ] 是 - [描述安全影响及缓解措施]

## 文档变更

**是否需要更新文档？**

- [ ] 否
- [x] 是 - 已更新对应 L3 Issue 与本 PR 变更记录

## 代码审查检查清单

### 功能性
- [x] 代码实现了需求
- [x] 嵌套 parent identity、历史 V1 sidecar、缺失/截断 sidecar 已处理
- [x] PSM 缺失 sidecar 安全返回；adapter 拒绝未知 nested parent

### 代码质量
- [x] 复用现有 sidecar / TasksPane / SessionTree 扩展点，无新增持久层
- [x] Pi persistent identity 与 Pager live identity 命名/职责分离
- [x] root tree 与 child flat 行为由独立代码路径控制

### 测试
- [x] 有对应的 TypeScript/前端与 Rust 源码测试覆盖
- [x] 覆盖 durable index、nested bridge、descendant view 与 tree builder 关键路径
- [x] 可在不运行 Cargo 的范围内执行的目标测试均通过

## 审查日志

- **[YYYY-MM-DD HH:MM] [审查人]**: [审查意见]
  - [ ] 问题 1: [描述]
  - [ ] 建议 1: [描述]

- **[YYYY-MM-DD HH:MM] [作者]**: [回应]
  - 已解决问题 1: [说明]

## 最终状态

- **合并时间:** 未合并（用户未请求）
- **合并人:** —
- **Commit Hash:** —
- **部署状态:** 未部署；PR 记录 ready