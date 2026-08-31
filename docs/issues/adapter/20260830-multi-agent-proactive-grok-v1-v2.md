---
id: "2026-08-30-multi-agent-proactive-grok-v1-v2"
title: "grok-pi 内置 multi-agent-proactive skill 与 V1/V2 子代理适配"
status: "in-progress"
created: "2026-08-30"
updated: "2026-08-30"
category: "adapter"
tags: ["workhub", "multi-agent-proactive-grok-v1-v2"]
---

# Issue: grok-pi 内置 multi-agent-proactive skill 与 V1/V2 子代理适配

## Goal

把现有 `multi-agent-proactive` 适配为 grok-pi V1/V2 子代理的父级主动编排规范，并由 `pi-grok-subagents` 作为内置 skill 提供；子代理不得因为父会话处于 Proactive/Ultra 而继承该主动编排模式。

## 背景/问题

现有 skill 使用旧的 `spawn_agent` / `send_message` / `followup_task` / `wait_agent` 语义，并明确要求 thread-spawn subagent 继续继承 Proactive、递归 spawn。当前 grok-pi 实现已经分为 V1 `spawn_subagent` 工具面与 V2 team 工具面，两者最终共用 `SubagentRuntime.createRecord()`。该 runtime 已通过 `noExtensions: true`、`noSkills: true` 隔离父资源，只允许 agent definition 显式声明 `extensions` / `skills`。

## 验收标准 (Acceptance Criteria)

- [ ] A1 `PI_GROK_SUBAGENTS=1` 时，`pi-grok-subagents` SHALL 通过 `resources_discover` 暴露内置 `multi-agent-proactive` skill。
- [ ] A2 skill SHALL 使用真实 V1 工具 `spawn_subagent` / `send_message_to_subagent` / `get_command_or_subagent_output` 与真实 V2 工具 `spawn_team_agent` / `team_send_message` / `team_followup_task` / `team_wait`。
- [ ] A3 Proactive/Ultra SHALL 是父级编排策略；child SHALL NOT 因父级启用而自动继承该模式或递归 fan-out。
- [ ] A4 V1/V2 child SHALL 继续使用隔离 ResourceLoader：默认不加载父 extensions/skills，只加载 agent definition 显式配置的资源。
- [ ] A5 Rust grok-pi extension injector SHALL 把内置 skill 一起 materialize，确保发布后的临时扩展路径可用。
- [ ] A6 本地源 skill 与扩展内置副本 SHALL 保持一致；相关 TS/Rust 窄测和 `git diff --check` 通过。
- [x] A7 V2 `team_wait` 在调用者之外没有 running/queued agent 时 SHALL 立即返回 idle 提示，让 child 结束当前 turn 进入 IDLE，而不是占住后台 turn 等到 timeout。
- [x] A8 V2 root 的 `team_wait` SHALL 始终立即返回；root 启动后台 agent 后通过结束当前 turn 挂起，后续由 `FINAL_ANSWER` 自动触发新 turn，而不是主动阻塞等待。

## 实施阶段

### Phase 1: 规划和准备
- [x] 定位 V1/V2 注册入口与共享 runtime
- [x] 核对 Pi `resources_discover.skillPaths` 契约
- [x] 确认 child loader 已有父资源隔离边界

### Phase 2: 执行
- [ ] 重写 `multi-agent-proactive` 为 parent-only + V1/V2 工具语义
- [ ] 在 `pi-grok-subagents` 注册内置 skill
- [ ] 更新 Rust bundle injector 与回归断言
- [x] 修复 V2 无 peer / root 主动等待时 `team_wait` 持续占用 turn 的挂起问题

### Phase 3: 验证
- [ ] 运行 subagent extension TS 测试/typecheck
- [ ] 运行 Rust injector 窄测
- [ ] `git diff --check` 与 diff 范围审查

## 关键决策

| 决策 | 理由 |
|------|------|
| 不修改 Grok 原生 subagent prompt/fork 实现 | V1/V2 适配都由 Pi extension 持有，且共享 runtime 已提供资源隔离；改 Rust 核心会扩大影响面。 |
| 内置 skill 由 `resources_discover` 暴露 | Pi 官方 extension resource seam，支持正常 skill catalog/触发，不需要把全文硬塞进 system prompt。 |
| child 默认不继承 Proactive，但允许 agent definition 显式选择技能 | “不继承”与“禁止显式配置”是两回事；保留已有 definition 能力，不额外造限制。 |

## 相关资源

- `extensions/pi-grok-subagents/`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/subagent_extension.rs`
- `docs/issues/adapter/20260728-grok-pi-subagent-config.md`
- Pi `docs/extensions.md` 的 `resources_discover` 契约

## Status 更新日志

- **2026-08-30**: 状态变更 → in-progress，完成真实 V1/V2 入口、child resource loader 和 skill 现状盘点。
- **2026-08-30**: V2 `team_wait` 挂起语义收口：root 永不阻塞；child 无 active peer 立即进入 IDLE。V2 窄测 14/14、Rust bundle 窄测 3/3、`git diff --check` 通过。