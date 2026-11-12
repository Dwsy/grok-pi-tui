---
id: "2026-11-10-grok-pi 子代理配置 UI 重写为 pi-tui 全屏组件（含鼠标交互）"
title: "grok-pi 子代理配置 UI 重写为 pi-tui 全屏组件（含鼠标交互）"
status: "in-progress"
created: "2026-11-10"
updated: "2026-11-10"
category: "subagents"
tags: ["subagents", "pi-tui", "remote-tui", "mouse", "configuration", "extension"]
---

# Issue: grok-pi 子代理配置 UI 重写为 pi-tui 全屏组件（含鼠标交互）

## Goal

grok-pi 的子代理配置功能**一律**经 Pi 的 TUI 完成：以 `ctx.ui.custom` + `@earendil-works/pi-tui` 全屏组件实现配置界面（走 Remote TUI bridge 投影进 Pager），选择、勾选、输入、确认、资源选择等全部配置交互同时支持鼠标与键盘。替换现有 `ctx.ui.select`/`ctx.ui.input` 菜单链。新增「后台并行度」配置项（默认 25，替代硬编码常量）。

## 背景/问题

- 现有配置 UI（`extensions/pi-grok-subagents/config-ui.ts`、`team-ui.ts`）是 `ctx.ui.select/input/confirm` 菜单链，多选/资源选择靠魔法标题前缀 envelope（`__pi_grok_multi_select_v1__` / `__pi_grok_resource_picker_v1__`）借道原生 Pager 面；菜单链逐屏问答、鼠标能力不统一。
- Remote TUI 鼠标事件已对齐（`docs/issues/adapter/20261029-Remote-TUI鼠标事件对齐.md`，implemented）：Pager 捕获的鼠标事件按序转发给 pi-tui 组件 `handleMouse()`；运行时 pi-tui **1.5.0** 的 `SelectList`/`SettingsList`/`Input`/`Editor`/`Box`/`MouseRegion` 均已实现鼠标接口。重写为 pi-tui 组件即可获得鼠标交互。
- 后台并行度此前为 `MAX_BACKGROUND_CONCURRENCY` 硬编码（已按用户要求定为 25），需改为可配置并保留 25 为默认值。

用户决策（2026-11-10，ask_user_question 确认）：

1. 配置范围 = **重写为 pi-tui 全屏组件**（不是只收敛 ctx.ui 菜单，也不是迁移 F2 开关；F2 的 `pi_subagents`/`pi_subagents_v2` 开关保持现状）。
2. 鼠标目标 = **ctx.ui 菜单的鼠标操作**：点击选项、勾选、按钮确认、输入框聚焦必须可用鼠标完成。

## 边界

| 层 | 职责 |
|---|---|
| Pi Core / pi-tui | 不修改；pi-tui 1.5.0 组件契约（含 `handleMouse`）是参照与积木 |
| Remote TUI bridge | 不修改传输/宿主；复用既有 mouse/keyboard 转发 |
| `extensions/pi-grok-subagents` | 配置 UI 全部重写为 pi-tui 组件；`definitions.ts` Markdown 持久化契约不变 |
| `pi-grok-adapter` | envelope 缝（multi-select/resource-picker）本次不动、不再从扩展调用；遗留清理单独记录 |
| Grok 原有子代理实现 | 继续只读 |

禁止：改 Pi 源码/RPC；改 definitions frontmatter 契约；改 spawn/runtime 行为语义（并行度来源除外）；改 Pi 资源启用设置。

## 验收标准 (Acceptance Criteria)

- [ ] A1 WHEN 打开 `/subagents`、team 编辑或运行时设置，THEN 全部配置操作在 pi-tui 全屏组件内完成，不再出现 select/input 菜单链。
- [ ] A2 WHEN 使用鼠标，THEN 可完成：点选列表项、勾选/取消多选、按钮确认/取消、输入框聚焦编辑、滚轮滚动列表。
- [ ] A3 WHEN 仅用键盘，THEN 既有操作路径不回归（方向键/Enter/Esc/文本输入/搜索过滤）。
- [ ] A4 WHEN 保存定义/团队，THEN Markdown frontmatter 读写契约与既有一致（既有 round-trip 测试全绿）。
- [ ] A5 WHEN 配置后台并行度，THEN 值经 UI 设置、正整数校验、持久化并在新 spawn 生效；默认 25；queued/cancel/prioritize 语义不变。
- [ ] A6 WHEN 验证，THEN 扩展测试全绿（含新增组件单测：鼠标+键盘路径）、`tsgo` 对本次文件无错误、diff 审查通过。

## 实施阶段

### Phase 1: 规划和准备
- [x] 分析需求和依赖（pi-tui 1.5.0 组件/鼠标契约、Remote TUI 转发现状、现有 UI 清单）
- [x] 设计技术方案（本 Issue）
- [x] 确定实施计划（S1–S6）

### Phase 2: 执行（Slices）
- [x] S1 config-shell：`ui-primitives.ts`（Panel 容器键鼠路由 + makeSelectPanel/makeMultiSelectPanel/makeInputPanel/makeEditorPanel/makeConfirmPanel + `openCustomPanel`）+ 单测（键鼠双路径）
- [x] S2 V1 定义编辑：`config-ui.ts` 重写为单个全屏 SettingsList 表单（子菜单面板编辑各字段），新建/覆盖/禁用/恢复/删除语义与旧菜单链一致
- [x] S3 资源 picker：extensions（host 注入目录 + tool source paths）/skills（`loadSkills` 发现）在组件内搜索+勾选，未识别路径保留；RESOURCE_PICKER/MULTI_SELECT envelope 调用移除（adapter 侧缝保留）
- [x] S4 V2 team 编辑：`team-ui.ts` 流程不变，全部 UI 原语换为 pi-tui 面板
- [x] S5 运行时设置：`runtime-settings.ts`（`$GROK_HOME/agents/settings.json`，1–256 校验，默认 25）；`runtime.ts` 每次调度读取生效；`/subagents` 菜单提供「Background concurrency」
- [x] S6 接线与文档：`/subagents`、`/subagent-control` 全部走 pi-tui；FEATURE_MATRIX 中英、usage 中英已更新

### Phase 3: 验证
- [x] 扩展单测：`PI_TEST_NODE_MODULES=~/.pi/agent/install/releases/1.5.0/node_modules node --import ./test-installed-pi.mjs --test *.test.ts` → **85 passed, 0 failed**（含 ui-primitives 鼠标/键盘用例、runtime-settings 用例、重写后的 configureSubagents 流程用例）
- [x] `tsgo`：本次文件无错误；残留 3 条既有错误（bridge.test.ts:165、runtime.test.ts:392、v2.test.ts:192，均未触及行）
- [x] `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi subagent_extension` → 4 passed（含 import 闭包断言）；`ext_dev` → 5 passed
- [x] `git diff --check` → 干净
- [ ] 真实终端人工鼠标验收（点击/勾选/滚轮）—— 待用户在 grok-pi 内实测

### Phase 4: 交付
- [ ] 更新文档（FEATURE_MATRIX、usage 中英文、本 Issue 验证记录）
- [ ] 创建 PR 记录

## 关键决策

| 决策 | 理由 |
|------|------|
| 配置 UI 载体 = `ctx.ui.custom` + pi-tui 全屏组件 | 用户选定；pi-tui 1.5.0 自带鼠标契约，Remote TUI 已转发鼠标，一套组件同时覆盖鼠标/键盘 |
| F2 `pi_subagents`/`pi_subagents_v2` 开关不迁移 | 用户选定范围之外；属 grok-pi 启动开关而非配置界面 |
| 资源选择改为组件内 picker 页 | 「一律使用 Pi 的 TUI」不再借道原生 envelope；仍只写 definition frontmatter，不改 Pi 资源启用设置 |
| adapter envelope 缝本次不删 | 避免跨 crate 扩大改动面；清理作为后续独立任务 |
| 后台并行度默认 25、可配置 | 承接本会话前序改动；runtime 语义（queued/cancel/prioritize）不变 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-11-10 | 测试依赖解析：`~/.pi/agent/npm` 的 pi-tui 是 0.75.4（无 `handleMouse`），组件鼠标测试报 `handleMouse is not a function` | `PI_TEST_NODE_MODULES` 指向 `~/.pi/agent/install/releases/1.5.0/node_modules`（pi-tui 1.5.0，与 grok-pi 运行时一致） |
| 2026-11-10 | `Input.setValue` 后光标停在行首，预填值上追加输入产生乱值（测试存了 256 而非 7） | `makeInputPanel` 预填后送 `ctrl+e` 把光标停到行尾，对齐旧 `ctx.ui.input` 行为 |
| 2026-11-10 | 既有阻塞：`xai-grok-pager-bin` 测试编译失败（`ext_dev.rs` 两处 `expect_err` 需要 `Debug`） | 与本次无关，但阻塞注入器测试；最小修复：`#[derive(Debug)]`，ext_dev 5 测试通过 |
| 2026-11-10 | `tsgo` 3 条既有错误（bridge.test.ts:165、runtime.test.ts:392、v2.test.ts:192） | 均在本次未触及的行，记录为存量问题不动 |

## 相关资源

- 相关 Issue: `docs/issues/adapter/20260728-grok-pi-subagent-config.md`（原菜单链实现）
- 相关 Issue: `docs/issues/adapter/20261029-Remote-TUI鼠标事件对齐.md`（鼠标转发契约）
- 相关 Issue: `docs/issues/adapter/20261102-subagents-v2-multitask.md`（并发槽语义）
- 相关 ADR: `docs/adr/20260822-subagents-v2-team-collaboration.md`
- 参考: `~/.pi/agent/install/releases/1.5.0/node_modules/@earendil-works/pi-tui/dist/`（组件与 `handleMouse` 契约）

## Notes

- UX/DX/agent-legibility 三轴：UX = 配置从逐屏问答变一体全屏表单 + 全鼠标操作；DX = UI 层集中到组件、`definitions.ts`/`runtime.ts` 不动，但新增组件代码量大于菜单链；agent-legibility = 组件契约由单测钉住（鼠标/键盘路径各一组）、魔法前缀 envelope 调用点消失更易 grep。
- 待确认（实现中定）：运行时设置的持久化位置（倾向 `$GROK_HOME/agents/settings.json` 全局单份）。

---

## Status 更新日志

- **2026-11-10**: 状态变更 → in-progress，备注: 规划完成（S1–S6），用户已确认方案（pi-tui 全屏组件 + 鼠标）。
- **2026-11-10**: S1–S6 完成，TS 85 测试 / 注入器 4 测试 / ext_dev 5 测试全绿；待真实终端人工鼠标验收。
