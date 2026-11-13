---
id: "2026-11-13-Dashboard 多 agent 切换与 Pi 联动修复"
title: "Dashboard 多 agent 切换与 Pi 联动修复"
status: "draft"
created: "2026-11-13"
updated: "2026-11-13"
category: "grok-pi"
tags: ["workhub", "pr", "dashboard", "pi-adapter", "multi-agent", "session-binding"]
---

# Dashboard 多 agent 切换与 Pi 联动修复

> 修复 Dashboard 多 agent 切换时 Pi 单 RPC host 未重新绑定导致的 prompt/cancel/子会话/删除串线问题

## 背景与目的 (Why)

Pi 通过单个 RPC host 运行一个 mutable active session。grok-pi 的 Dashboard 支持多 AgentView 并存，但切换 agent 后 Pi 侧不重新绑定 —— 用户在 agent B 打字，消息进入 agent A 的 Pi session。删除行、给 subagent 发消息、Esc 取消也全部串线。

## 变更内容概述 (What)

### 1. 多 agent 切换 → Pi session rebind 管道

**问题：** 切换 agent 后 prompt/cancel 进入错误的 Pi session

**方案：**
- adapter 新增 `x.ai/session/rebind`：轻量切换 RPC host 绑定（不重放历史）
- pager 新增 `Effect::RebindPiSession` + `TaskResult::PiSessionRebound/Failed`
- `rebind_pi_session_for()` helper 统一发 effect，挂到 6 个切换入口：
  - `dispatch_dashboard_attach` TopLevel（Enter 进行）
  - `dispatch_dashboard_overlay_cycle`（`[‹][›]` cycle）
  - `focus_if_session_already_open`（会话列表 / /resume / Roster attach）
  - `remove_agent_and_cleanup` fallback（关会话后自动切到邻居）
  - `settings/ui.rs` 两处 switch_to_agent
- `prompt()` 自动 rebind 兜底（防异步竞态：切换后立即打字不受影响）

### 2. cancel 跨会话保护

**问题：** 旧 agent 的 Esc 取消杀掉新 session 的 turn

**方案：** `cancel()` 校验 `session_id` 与当前绑定匹配，跨会话静默忽略

### 3. 子会话消息路由

**问题：** 给 subagent 发消息实际进入根 session

**方案：** 新增 `__pi_grok_subagent_send` 桥接命令（扩展 tools-v1.ts + adapter agent.rs 双侧），`sendNow`→steer / 默认→follow_up

### 4. Dashboard 删行

**问题：** `x.ai/session/delete` 未实现，删行报 Method not found

**方案：** adapter 补实现（trash-cli 优先 / unlink 兜底 / 拒删当前会话），与 Pi 自身交互式 session-selector 一致

### 5. replace_bootstrap 注册 session_path

**问题：** 新建会话切换回去报 "not in local catalog"

**方案：** `replace_bootstrap` 将 `session_file` 注册到 `session_paths` HashMap

### 6. leader_mode 短路 external_agent

**问题：** `dispatch_open_dashboard` 和 event_loop 轮询里 `leader_mode` 判断在 `external_agent` 前面，两者同时为 true 时 Pi catalog 永远拉不到

**方案：** 调整判断顺序，`external_agent` 优先

### 7. Esc cancelling 状态重复发 CancelTurn

**问题：** cancelling 中按 Esc 重新发取消并提示 Ctrl+C

**方案：** `try_handle_esc_policy` cancelling 分支吞掉 Esc（改为 `InputOutcome::Changed`）

### 8. home husk promote / 丢弃

**问题：** attach home husk 后 NewSession 误删活会话；dashboard New Agent 留脏行

**方案：**
- `switch_to_agent` 切到 home husk 时清 `home_session_agent` / `optimistic_home_husk`
- `new_session_inner_with_id` 创建新 agent 前调用 `abandon_unused_home_session`

### 9. modal loading 契约

**问题：** `dispatch_show_session_picker` 将 modal `loading` 设为 `external_agent`，native 模式下不等自己的 fetch

**方案：** 统一 `loading: true`（两种模式都等 fetch）

### 10. 设置注册缺口

**问题：** `show_status_tool_calls` / `follow_up_behavior` / `codemode_expanded_by_default` / `contextual_hints.export_copy` 缺 dispatch arm 和 sidebar section

**方案：** 补齐 4 处

## 关联 Issue

- **Issue:** `docs/issues/grok-pi/20261113-Dashboard 多 agent 切换与 Pi 联动修复.md`
- **相关:** `docs/issues/pager/20260717-Pi Dashboard 完整适配.md`
- **相关:** `docs/issues/grok-pi/20261111-fork-to-dashboard.md`

## 测试与验证结果 (Test Result)

- [x] `cargo check -p xai-grok-pager -p pi-grok-adapter` 编译通过
- [x] `pi_fork_to_dashboard` 系列测试通过（修复 leader_mode 短路后）
- [x] `esc_while_cancelling` 系列测试通过
- [x] `external_dashboard_dispatch_blocks_while_turn_is_running` 通过
- [ ] 全量 `cargo test`（待跑：`GROK_HOME=$(mktemp -d) cargo test -p xai-grok-pager --lib`）

## 风险与影响评估 (Risk Assessment)

- **rebind 是新增端点**：不改已有 RPC 协议，旧 adapter 忽略未知 method 返回 Method not found（安全降级）
- **prompt() 自动 rebind**：每次 prompt 多一次 `session_paths` 查找（HashMap lookup，无性能影响）
- **cancel() 校验 session_id**：合法取消不受影响，只拦截跨会话误取消
- **home husk promote**：attach 语义变更，但测试守护契约明确

## 回滚方案 (Rollback Plan)

```bash
git revert <commit-hash>
cargo check -p xai-grok-pager -p pi-grok-adapter
```

新增代码集中在 `pi_adapter/agent.rs`、`pi_adapter/session.rs`、`pi_adapter.rs`、`dispatch/ctx.rs`、`dispatch/dashboard.rs`、`dispatch/session/load.rs`，revert 影响范围明确。

---

## 变更类型

- [x] Bug Fix
- [ ] New Feature
- [ ] Documentation
- [ ] Refactoring
- [ ] Performance
- [ ] Security
- [ ] Testing

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `pi-grok-adapter/src/pi_adapter/agent.rs` | 修改 | prompt/cancel 跨会话保护 + 自动 rebind + delete handler + 子会话路由 |
| `pi-grok-adapter/src/pi_adapter/session.rs` | 修改 | replace_bootstrap 注册 session_path |
| `pi-grok-adapter/src/pi_adapter.rs` | 修改 | SUBAGENT_SEND_COMMAND + delete_pi_session_file helper |
| `xai-grok-pager/src/app/actions.rs` | 修改 | Effect::RebindPiSession + TaskResult variants |
| `xai-grok-pager/src/app/effects/mod.rs` | 修改 | RebindPiSession effect handler |
| `xai-grok-pager/src/app/dispatch/ctx.rs` | 修改 | rebind_pi_session_for() helper + home husk promote |
| `xai-grok-pager/src/app/dispatch/dashboard.rs` | 修改 | attach/cycle rebind hooks + leader_mode 顺序 |
| `xai-grok-pager/src/app/dispatch/session/load.rs` | 修改 | focus_if_session_already_open 返回 rebind effects + modal loading 契约 |
| `xai-grok-pager/src/app/dispatch/session/modal.rs` | 修改 | fallback switch rebind |
| `xai-grok-pager/src/app/dispatch/settings/ui.rs` | 修改 | switch_to_agent rebind hooks |
| `xai-grok-pager/src/app/dispatch/task_result.rs` | 修改 | TaskResult::PiSessionRebound 处理 |
| `xai-grok-pager/src/app/agent_view/prompt.rs` | 修改 | Esc cancelling 吞掉 |
| `xai-grok-pager/src/app/event_loop.rs` | 修改 | 轮询 leader_mode 顺序 |
| `xai-grok-pager/src/app/dispatch/session/lifecycle.rs` | 修改 | new_session_inner 丢弃 home husk |
| `xai-grok-pager/src/settings/layout.rs` | 修改 | sidebar section 补齐 |
| `xai-grok-pager/src/settings/registry.rs` | 修改 | show_status_tool_calls arm |
| `xai-grok-pager/src/views/pi_settings/actions.rs` | 修改 | dispatch arm 补齐 |
| `xai-grok-pager/src/app/dispatch/tests/settings.rs` | 修改 | move_setting_away_from_default arm |
| `xai-grok-pager/src/app/dispatch/tests/dashboard.rs` | 修改 | toast 断言前缀 |
| `extensions/pi-grok-subagents/tools-v1.ts` | 修改 | __pi_grok_subagent_send 桥接命令 |

## 测试命令

```bash
# 编译验证
cargo check -p xai-grok-pager -p pi-grok-adapter

# 全量测试（隔离用户配置）
GROK_HOME=$(mktemp -d) cargo test -p xai-grok-pager --lib

# 针对联动测试
GROK_HOME=$(mktemp -d) cargo test -p xai-grok-pager --lib -- \
  pi_fork_to_dashboard external_dashboard_dispatch esc_while_cancelling
```

## 破坏性变更

- [x] 否 — `x.ai/session/rebind` 是新增端点，旧 adapter 返回 Method not found（安全降级）

## 性能影响

- [x] 无影响 — rebind 是 HashMap lookup + 可选 switch_session RPC

## 依赖变更

- [x] 否

## 安全考虑

- [x] 否 — `x.ai/session/delete` 拒删当前会话，trash-cli 优先（可恢复）

## 文档变更

- [x] 是 — FEATURE_MATRIX 可补充 `x.ai/session/rebind` 和 `x.ai/session/delete` 端点说明

## 代码审查检查清单

### 功能性
- [x] 代码实现了需求
- [x] 边界情况已处理（竞态：prompt 自动 rebind；跨会话 cancel 静默忽略）
- [x] 错误处理完善（rebind failed → toast；delete 路径不存在 → invalid_params）

### 代码质量
- [x] 代码遵循项目规范
- [x] 变量命名清晰
- [x] 没有冗余代码（统一 helper，6 个入口复用）

### 测试
- [ ] 有对应的单元测试（待补充：rebind 管道端到端测试）
- [x] 测试覆盖关键路径（fork/esc/dispatch 测试验证）
- [x] 测试通过

## 最终状态

- **合并时间:** [待合并]
- **合并人:** [待定]
- **Commit Hash:** [待定]
- **部署状态:** 待部署
