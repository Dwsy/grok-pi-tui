---
id: "2026-11-03-子代理V2 TUI管理与team技能 AGENTS注入"
title: "子代理V2 TUI管理与team技能 AGENTS注入"
status: "doing"
created: "2026-11-03"
updated: "2026-11-03"
category: "adapter"
tags: ["workhub", "子代理V2 TUI管理与team技能 AGENTS注入"]
---

# Issue: 子代理V2 TUI管理与team技能 AGENTS注入

## Goal

V2 team 子代理默认继承全局/项目技能与 AGENTS.md 上下文（definition 可显式关闭），并提供对标 role-persona-zero 的完整 TUI 控制中心。

## 背景/问题

1. 当前无 V2 TUI 管理面：只有 `team_list` 工具与分散的 `/subagents`、`/subagent-teams` 编辑器，无统一控制中心（参考 `~/.pi/agent/extensions/role-persona-zero/role-control-center.ts`）。
2. team 子代理零继承：`runtime.ts` 以 `noSkills: true` + `noContextFiles: true` 创建子会话，仅加载 `definition.skills`，全局/项目技能与 AGENTS.md 全部丢失；`systemPromptOverride` 仅替换 system prompt，`agentsFiles` 为空。

## 验收标准 (Acceptance Criteria)

- [x] WHEN spawning V2/V1 child 且 definition 未显式关闭，系统 SHALL 继承全局/项目 skills（`definition.skills` 叠加）
- [x] WHEN spawning child 且 definition 未显式关闭，系统 SHALL 继承 AGENTS.md 上下文（`noContextFiles: false`）
- [x] WHERE definition frontmatter 置 `inherit_skills: false` 或 `inherit_context: false`，系统 SHALL 恢复隔离行为
- [x] WHERE 新 `/subagent-control` TUI，系统 SHALL 提供状态/定义/preset/注入预览/关闭（对标 role control-center，用现有 `ctx.ui.select` 原语）
- [x] IF 新增 TS 模块，THEN Rust injector `subagent_extension.rs` SHALL 同步 materialize（含单测回归守卫通过）

## 实施阶段

### Phase 1: 继承语义（definitions + runtime）
- [x] `definitions.ts`: 新增 `inheritSkills`/`inheritContext`（默认 true），解析/序列化/clone
- [x] `runtime.ts`: `noSkills`/`noContextFiles` 由 definition 标志驱动（默认继承），注释更新
- [ ] 单测：继承默认开、显式关闭恢复隔离（`bun test` 当前缺 `azure.json` 生成文件，待补）

### Phase 2: TUI 控制中心
- [x] 新增 `control-center.ts`: `/subagent-control`（状态/定义/preset/注入预览/关闭），复用 `config-ui`/`team-ui` 原语
- [x] `tools-v1.ts`: 注册命令（V1/V2 常驻）；V2 快照后续迭代直连 coordinator
- [x] `subagent_extension.rs`: materialize 新模块 + 单测通过（4/4）

### Phase 3: 验证
- [x] `cargo check -p xai-grok-shared -p xai-grok-shell` 通过
- [ ] 扩展单测（`bun test` 缺生成文件，改前已存在）
- [x] 文档：`docs/usage/subagents-v2.md` + `.zh-CN.md` 更新继承语义与新命令

## 关键决策

| 决策 | 理由 |
|------|------|
| [决策 1] | [理由] |
| [决策 2] | [理由] |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| [YYYY-MM-DD] | [错误描述] | [如何解决] |

## 相关资源

- [ ] 相关文档: `docs/architecture/xxx.md`
- [ ] 相关 Issue: `docs/issues/ISSUE-xxx.md`
- [ ] 参考资料: [链接]

## Notes

[记录研究过程、临时想法、待确认事项]

---

## Status 更新日志

- **[YYYY-MM-DD HH:MM]**: 状态变更 → [新状态]，备注: [变更原因]