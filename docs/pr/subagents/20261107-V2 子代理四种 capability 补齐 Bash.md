---
id: "2026-11-07-V2 子代理 capability 收敛为 read-only 与 all"
title: "V2 子代理 capability 收敛为 read-only / all 并补齐 Bash"
status: "open"
created: "2026-11-07"
updated: "2026-11-07"
category: "subagents"
tags: ["workhub", "pr", "subagents", "capability_mode", "bash"]
---

# V2 子代理 capability 收敛为 read-only / all 并补齐 Bash

> 记录同一批改动的最终状态：先给四种 capability 补齐 Bash，随后按用户决策「做减法」，只保留 `read-only` 与 `all` 两种模式。

## 背景与目的 (Why)

排查确认：`read-write` 能力的 agent 确实没有 shell/bash 工具，而 `read-only` 反而有。

- `extensions/pi-grok-subagents/shared.ts` 的 `CAPABILITY_TOOLS` 里，`read-write` = `read,grep,find,ls,edit,write`，不含 `bash`；`read-only`/`execute`/`all` 都含 `bash`。
- `explore` profile 用 `capabilityFloor: "execute"` 把 `read-write` 偷偷提升为 `execute` 才拿到 Bash，掩盖了缺工具的事实。
- 原生 Grok 侧 `crates/codegen/xai-grok-tools/src/implementations/grok_build/task/types.rs` 的 `allowed_tool_kinds` 中 `ReadWrite` 不含 `ToolKind::Execute`，单测 `filter_read_write_keeps_edit_removes_execute` 明确断言移除 bash，`16-subagents.md` 写 "No shell" —— 是上游设计，不是 bug。
- 另有一套 `extensions/pi-grok-workflows/index.ts` 的 `CAPABILITY_TOOLS`，`read-only` 与 `read-write` 都无 bash。

用户决策演进：四种模式都补 Bash、限制只靠注入的提示词 → `execute` 也应有 `edit`/`write` → **做减法，只保留 `read-only` 与 `all`**；`read-only` 仍裁剪 `edit`/`write`，旧值 `execute`/`read-write` 静默映射到 `all`。范围仅 V2 子代理扩展，不改 workflows 扩展与 Rust 原生侧，不运行 cargo。

> 注：「旧值静默映射」与项目「不保留向后兼容」原则相悖，是用户显式选择，按用户要求执行。

## 变更内容概述 (What)

- `CAPABILITY_TOOLS` 只保留 `read-only` / `all` 两个 key；两者都含 Bash。`read-only` 省略内置 `edit`/`write` 并注入 `READ_ONLY_CONTRACT`，`all` 用全量内置工具。
- `requireCapability()` 把旧值 `execute`/`read-write` 静默映射为 `all`，非法值仍 fail fast 报错。
- `explore` / `plan` 的默认 `capabilityMode` 从 `execute` 改为 `all`；它们不再靠工具裁剪，改由各自 system prompt 禁止改动文件。
- `bridge.ts` 的持久化校验仍接受四值，旧快照可加载（只加注释说明）。
- `tools-v1.ts` / `v2.ts` 的 `capability_mode` 工具描述更新为两模式 + 旧值映射说明。
- 删除已失效的 `capabilityFloor` / `CAPABILITY_ORDER` / `widest()` 与 `resolveProfile()` 的模式提升逻辑。
- `resolveSubagentBusinessTools()` 无条件补 `bash`，definition 显式 `tools:` 列表也保留 Bash。
- 单测与中英文使用文档同步。

最终矩阵（`bun` 实测 `shared.ts`）：

| mode | bash | edit | write | 注入契约 | 工具集 |
|---|---|---|---|---|---|
| `read-only` | ✅ | ❌ | ❌ | `READ_ONLY_CONTRACT` | read,bash,grep,find,ls |
| `all` | ✅ | ✅ | ✅ | 无 | read,bash,edit,write,grep,find,ls |

## 关联 Issue

- **Issues:** `docs/issues/adapter/20260822-grok-pi Subagents V2 Team 协作.md`（V2 子代理主 Issue，`capability_mode` 调用级覆盖由 A21 引入）

## 测试与验证结果 (Test Result)

- [x] `bun` 直接 import `shared.ts` 实测：`Object.keys(CAPABILITY_TOOLS)` = `read-only,all`；两模式工具集如上表
- [x] `requireCapability()` 实测：`execute` → `all`、`read-write` → `all`、`read-only`/`all`/`undefined` 原样或默认 `all`、`bogus` 抛 `unsupported capability_mode: bogus`
- [x] `git diff --check` — PASS（exit 0）
- [x] 源码复核：扩展内无残留 `CAPABILITY_TOOLS.execute` / `CAPABILITY_TOOLS["read-write"]` / `capabilityMode: "execute"` 索引；`bridge.ts` 四值校验为有意保留
- [ ] `runtime.test.ts` 单测套件未运行：用户明确要求不运行 cargo；且 Bun 加载 `definitions.ts` 会因 `pi-main` 缺 `data/*.json` 失败（既有环境问题，非本次引入）
- [ ] 全量 TS/Cargo 测试与构建未运行（按用户要求）

## 风险与影响评估 (Risk Assessment)

- **UX**：`read-only` 仍无内置 `edit`/`write`，但 Bash 可执行任意命令，「只读」约束力主要来自注入契约；`explore`/`plan` 现在拿到 `edit`/`write` 工具，靠各自 system prompt 禁止改动，属于提示词级而非工具级边界。无 OS/container 沙箱。
- **DX**：模式从四档收敛为两档，删除 floor 死代码；旧值静默映射让既有 preset/definition 无需改动，但保留了与项目「不保留向后兼容」原则相悖的兼容分支。
- **agent-legibility**：能力真相收敛到 `CAPABILITY_TOOLS` 两行 + 注入 prompt；测试钉住「两模式」「旧值映射」「显式 tools 保留 Bash」「非法值报错」。
- **范围外不一致（未改，已知）**：`extensions/pi-grok-workflows/index.ts` 仍是四模式表且 `read-only`/`read-write` 无 bash；Rust 原生 `SubagentCapabilityMode` 仍是四档、read-write = No shell；`docs/CHANGELOG.zh-CN.md:32` 与原生 `16-subagents.md:173` 仍描述旧语义（前者为历史变更记录）。

## 回滚方案 (Rollback Plan)

仅恢复以下 8 个文件到改动前状态即可，无配置、数据或 schema 变更，不需要重建 Pi extension bundle：

```bash
git restore extensions/pi-grok-subagents/shared.ts \
  extensions/pi-grok-subagents/definitions.ts \
  extensions/pi-grok-subagents/runtime.test.ts \
  extensions/pi-grok-subagents/bridge.ts \
  extensions/pi-grok-subagents/tools-v1.ts \
  extensions/pi-grok-subagents/v2.ts \
  docs/usage/subagents-v2.md \
  docs/usage/subagents-v2.zh-CN.md
```

本 PR 文件为新增记录，按需保留或移入回收站。

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `extensions/pi-grok-subagents/shared.ts` | 修改 | 收敛为两模式；旧值映射；无条件补 bash |
| `extensions/pi-grok-subagents/definitions.ts` | 修改 | explore/plan 默认改 `all`；删除 capabilityFloor 提升逻辑 |
| `extensions/pi-grok-subagents/runtime.test.ts` | 修改 | 两模式工具集、旧值映射、非法值断言 |
| `extensions/pi-grok-subagents/bridge.ts` | 修改 | 旧快照四值校验加说明注释 |
| `extensions/pi-grok-subagents/tools-v1.ts` | 修改 | `capability_mode` 工具描述 |
| `extensions/pi-grok-subagents/v2.ts` | 修改 | 两处 `capability_mode` 工具描述 |
| `docs/usage/subagents-v2.md` | 修改 | 参数表、矩阵、约束说明 |
| `docs/usage/subagents-v2.zh-CN.md` | 修改 | 同上，中文 |
| `docs/pr/subagents/20261107-V2 子代理四种 capability 补齐 Bash.md` | 新增 | 本变更记录（文件名沿用创建时标题） |

## 破坏性变更

- [x] 是：`capability_mode` 枚举从四值变为两值；但 `execute`/`read-write` 静默映射为 `all`，既有调用不报错。`explore`/`plan` 现在有 `edit`/`write` 工具，行为约束改由注入的 system prompt 承担。

## 最终状态

- **合并时间:** 未合并（改动仍在工作树，未提交）
- **Commit Hash:** 无
