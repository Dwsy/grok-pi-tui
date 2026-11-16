---
id: "2026-11-15-ask_user_question-timeout-esc-decline-notifications"
title: "ask_user_question 超时可配 / Esc 直接 decline / 通知正式化"
status: "in_progress"
created: "2026-11-15"
updated: "2026-11-15"
category: "pager"
tags: ["workhub", "ask-user-question", "question-view", "timeout", "notification"]
---

# Issue: ask_user_question 三件套

## 背景

三个缺陷同源：question 不是一等 pending event。

1. 超时 = 30 分钟 hardcode + `timeout_enabled` bool，不暴露 secs；timeout 和 user cancel 共用 `CANCEL_TEXT`。
2. 普通 Grok question 的 Esc 没有 decline 档；只有 Pi extension UI 直接 decline。
3. 系统通知硬编码 `"A question is waiting for your response."`，不进 `NotificationService`，无 session/题目内容。

## 已确认决策（2026-11-15）

| 点 | 决策 |
|---|---|
| Esc | **一步直接 decline**（与 Pi `is_pi_extension_ui` 对齐）；`EscStep::CancelInteraction` 复用 |
| 超时 | **F2 加 `timeout_secs` 项**（shell-owned，restart_required）；pager mirror 展示 effective 值 |
| 通知 | **正式化**：新增 `NotificationEventKind::QuestionRequired`，进 events/hooks/telemetry |

## 验收

- [ ] F2 Agent 分类出现 `Ask-Question timeout secs`，默认 1800，写入 `[toolset.ask_user_question].timeout_secs`；`0` 非法回落 default
- [ ] timeout 触发时 tool result 与 cancel 文案可区分（non-interactive 仍统一 `NO_OPERATOR_TEXT`）
- [ ] 普通 Grok question 的 `Esc` 直接关闭 card + 发 `Cancelled`（不留 parked question）
- [ ] 失焦时 question 抵达走 `NotificationService` + `AskUserQuestionNotifications` gate，body 带 session/首个问题摘要
- [ ] `ApprovalRequired` 通知 body 带 session/tool；`Pi`/Grok 共用同一条通道
- [ ] 现有 Esc 梯子其他 rung（clear_selection / leave_text_input / park / dashboard）保持原语义

## 非目标

- 不改 `question_view` 提交/freeform/preview 逻辑
- 不改 Pi control-dir 协议
- 不做 threat-model/arch 文档
