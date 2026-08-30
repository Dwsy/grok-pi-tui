---
id: "2026-08-30-修复 grok-pi 前台 Bash Eval 消息中断"
title: "修复 grok-pi 前台 Bash Eval 消息中断"
status: "done"
created: "2026-08-30"
updated: "2026-08-30"
category: "adapter"
tags: ["workhub", "grok-pi", "interrupt", "bash", "eval"]
---

# Issue: 修复 grok-pi 前台 Bash Eval 消息中断

## Goal

让用户在 grok-pi 前台 Bash / Eval 执行期间发送新消息时，可靠触发 send-now 抢占并取消当前前台工具，同时保持后台任务 wait 只中断等待、不误杀后台任务的既有语义。

## 背景/问题

当前 `send a message to interrupt` 只覆盖 `get_task_output` / `wait_tasks` / Await / foreground subagent 等 blocking wait。前台 Bash 和 Eval 被投影为 `ToolRunning`，shell 不打开 auto-send-now window，因此普通消息只排队；这与用户对“发消息中断当前长任务”的预期不一致。

## 验收标准 (Acceptance Criteria)

- [x] WHEN 前台 `bash` 或 `eval` 正在执行且用户发送新消息，系统 SHALL 将该消息作为 send-now 接管当前 turn，并通过现有 AbortSignal 取消前台工具。
- [x] WHERE Bash / Eval 已后台化，系统 SHALL 保持 `get_task_output` / `wait_tasks` 的语义：消息只中断等待，后台 task 继续运行，除非显式 `kill_task`。
- [x] WHEN Pager 展示可被消息抢占的前台 Bash / Eval，系统 SHALL 给出与真实行为一致的 interrupt affordance，不对普通不可抢占工具撒谎。
- [x] IF 已存在 held user queue 或 goal run 正在执行，THEN 系统 SHALL 保留现有队列/goal 约束，不擅自抢占。
- [x] Foreground subagent 既有 sendable-wait 行为 SHALL 不回归。

## 实施阶段

### Phase 1: 规划和准备
- [x] 分析 blocking wait、send-now、Bash/Eval AbortSignal 和 Pager activity 链路
- [x] 识别现有脏工作区并避开无关用户改动
- [x] 设计“前台可抢占工具”独立于 wait-tool synthetic result 的方案

### Phase 2: 执行
- [x] shell 为前台 Bash / Eval 打开 auto-send-now window，后台调用除外
- [x] Pager 从 canonical ToolCall 元数据识别前台可抢占工具
- [x] Pager 为可抢占 ToolRunning 展示 interrupt affordance，并绕过 leader/steer gate 发送普通纯文本消息

### Phase 3: 验证
- [x] shell 单元测试覆盖 foreground Bash/Eval 分类与 background 排除
- [x] Pager 单元测试覆盖 ToolCall 元数据识别、状态栏提示和 immediate-send 路由
- [x] `git diff --check` 通过；既有 wait/subagent 回归测试通过

### Phase 4: 交付
- [x] 更新 Issue Notes 与验收状态
- [ ] 创建 PR（本次未请求）
- [ ] 合并主分支（本次未请求）

## 关键决策

| 决策 | 理由 |
|------|------|
| 前台 Bash/Eval 不复用 `is_interruptible_wait_tool` synthetic wait 分支 | 那条分支只应丢弃 wait future 并返回“wait interrupted”；前台工具必须取消整个 turn，才能触发现有 AbortSignal 真正终止进程/Kernel |
| 仅标记前台 `bash` / `eval`，不泛化到所有 ToolRunning | Read/Edit/MCP 等工具的取消安全性不同，不能因为 UI 统一而扩大抢占面 |
| Pager 从 `x.ai/tool.name` 识别工具，旧 shell 仅做精确 title fallback | 避免 ToolCall 后续 title 被 command/code 覆盖，同时兼容旧事件 |
| 普通消息继续走 `session/prompt`，不复用 Pager 当前 `SendPromptNow` action | 当前 `SendPromptNow` 已重定向为 steer/interject，不具备 cancel-and-restart 语义；由 shell auto-send-now 保持服务端权威 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-08-30 | Issue 模板第一次整块替换因末尾换行差异未命中 | 重新读取原文后按小块精确替换 |
| 2026-08-30 | 定向 `cargo fmt --check -- <files>` 仍触发仓库级 rustfmt/edition 与既有缺失 PTY 模块问题 | 不运行全局 fmt 避免改动无关脏文件；手工对齐新增片段并用 `git diff --check` 校验 |
| 2026-08-30 | shell 新 helper 测试首次编译时 tests 模块未导入外层函数 | 在现有 wait_interrupt_tests import 中显式导入 helper；重跑定向测试通过 |

## 相关资源

- [x] `crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs`
- [x] `crates/codegen/xai-grok-shell/src/tools/tool_context.rs`
- [x] `crates/codegen/xai-grok-pager/src/acp/tracker.rs`
- [x] `crates/codegen/xai-grok-pager/src/app/agent_view/queue.rs`
- [x] `crates/codegen/xai-grok-pager/src/app/dispatch/queue.rs`
- [x] `crates/codegen/xai-grok-pager/src/views/turn_status.rs`
- [x] `extensions/pi-grok-bash/index.ts`（确认现有 AbortSignal 已杀前台 Bash 进程树 / abort foreground Eval）

## Notes

- `blocking_wait_depth > 0` 是 shell 普通 user prompt 自动升级为 send-now 的服务端权威判定；本修复复用同一个计数窗口，但保留 wait-tool 与 foreground-tool 的不同执行语义。
- Foreground Bash 的 AbortSignal listener 已调用进程树 kill；foreground Eval v2 已连接 task controller abort，因此无需新增 extension 级 kill 通道。
- `is_interruptible_wait_tool` 继续只覆盖 task-output/wait/Await；`bash`/`eval` 使用独立 `is_message_interruptible_foreground_tool`，避免返回 synthetic TaskOutput。
- Pager 的 `SendPromptNow` action 当前走 steer/interject，因此不能用于本修复的 cancel-and-send；普通消息通过 `session/prompt` 交给 shell auto-send-now。
- 现有工作区有与本 Issue 无关的未提交修改；本次未执行全仓 fmt，也未回退或覆盖这些改动。

---

## Status 更新日志

- **2026-08-30**: 状态变更 → in_progress，备注: 完成根因分析并开始修复。
- **2026-08-30**: 状态变更 → done，备注: shell/Pager 路径与定向回归测试完成；PR/合并未在本次请求范围内。