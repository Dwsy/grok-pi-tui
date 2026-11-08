---
id: "2026-11-08-Bash session 级临时环境变量管理"
title: "Bash 项目级/Session 级环境变量工具"
status: "completed"
created: "2026-11-08"
updated: "2026-11-08"
category: "adapter"
tags: ["workhub", "bash", "env", "session", "dotenv", "pi-grok-bash"]
---

# Issue: Bash 项目级/Session 级环境变量工具

> 文件名沿用创建时的标题；范围经用户多轮澄清后由「仅 session 级」扩展为「.env + 项目级 + session 级」。

## Goal

提供单一 `bash_env_tool`，让 Agent 自主管理项目级（`<cwd>/.pi/env.json`）与 session 级（增量、可随 `/resume` 恢复）环境变量，并只读加载项目 `.env`；按 `base < .env < project < session` 合并进 grok-pi 增强 Bash 的 spawn env，使变量跨多次 Bash 调用保持有效。

## 背景/问题

- Bash 每次调用都是新的 `/bin/bash -c`（Windows 为 `bash -c`），命令内 `export` 只活在该子进程，下一轮即丢失。
- 前台 Bash env 来自 Pi `getShellEnv()` 快照（`options.env`），后台直接 `process.env`；两者都没有用户可管理的持久层。
- 用户要求极简：`bash_env_tool` 支持增加/删除、项目级文件放 `.pi`、Agent 自主管理；session 层为渐进式增量覆盖。后续选择「两层一次做完」并追加 `.env` 只读加载（`k=v` → `ENV_<KEY>`）。
- 决策演变已记入 `docs/adr/rejected.md`：先否决「首版全量 L3 多文件改造」（用户后续选择两层一次做完，构成恢复该能力的新证据），再否决「引入第三方 dotenv / 转写库源码」（用户确认优先 Node 内置 `util.parseEnv`）。

## 验收标准 (Acceptance Criteria)

- [x] WHEN 增强 Bash 启用，系统 SHALL 注册单一 `bash_env_tool`，支持 `action: set|delete|list` 与 `scope: project|session`（默认 `session`）。
- [x] WHERE `scope=project`，`set`/`delete` SHALL 读写 `<cwd>/.pi/env.json`（不存在视为空，非法 JSON 明确报错），并 SHALL 由 `.gitignore` 忽略，不进版本库。
- [x] WHERE `scope=session`，变更 SHALL 以 `pi.appendEntry("pi-grok-session-env/v1", { set, delete })` 持久化（不进入 LLM context）；首次调用及 session leaf 变化时 SHALL 按 `ctx.sessionManager.getBranch()` 顺序重放，末次事件生效。
- [x] WHEN `/resume` 加载同一 session，系统 SHALL 恢复该 session 的 env；新建 session SHALL 为空；切换到其他分支 SHALL 不串味。
- [x] WHERE `<cwd>/.env` 存在，系统 SHALL 用 Node 内置 `util.parseEnv` 解析 `k=v` 行，把键暴露为 `ENV_<KEY>`（已有 `ENV_` 前缀保持不变）；`.env` SHALL 只读，且不引入第三方依赖。
- [x] WHEN 前台或后台 Bash spawn，系统 SHALL 按 `base env < .env < project < session` 合并；运行中任务不受后续变更影响。
- [x] IF `PI_GROK_BASH=0` 或 CLI 禁用 `bash`，THEN `bash_env_tool` SHALL 不注册。
- [x] WHERE 键名属于保留前缀 `PI_`/`GROK_` 或保留名 `PATH/HOME/SHELL/PWD/TMPDIR/USER/LOGNAME/SystemRoot/COMSPEC/PATHEXT/WINDIR`，或不符合 `^[A-Za-z_][A-Za-z0-9_]*$`，或值含 NUL，系统 SHALL 拒绝并给出明确错误。
- [x] `list` 与 `set` 结果 SHALL 只返回键名与状态，不回显值。
- [x] Bash 命令文本 SHALL 不被改写（不采用 `export` 前缀），Pager 工具卡片仍显示原命令。
- [x] 文档（扩展 README、Feature Matrix 中英）、Node 回归、Rust injector diff 审查 SHALL 通过；`git diff --check` 无空白错误。

## 实施阶段

### Phase 1: 规划和准备
- [x] Workhub 查重（`环境变量/env/resume/session`）→ 无同类 Issue/PR；`rejected.md` 原仅 1 条无关否决。
- [x] 定位真实路径：Bash env 注入点、`pi.appendEntry`、`getBranch()` 顺序、`appendCustomEntry` 推进 leaf、`session_start(reason=resume)`、`.pi` 项目目录约定与 `.gitignore`。
- [x] 澄清全部决策点（范围、持久化、注入、安全、工具形态、`.env` 映射、库依赖）并定稿。
- [x] 记录历史取舍到 `docs/adr/rejected.md`。

### Phase 2: 执行
- [x] 新增 `extensions/pi-grok-bash/env-tool.ts`：`.env`/project 文件读写、session event log 重放、保留键校验、env 合并、tool 定义。
- [x] `index.ts` 接入：注册 `bash_env_tool`、`session_start` 同步、前台/后台 `startTask` env 合并。
- [x] `bash_extension.rs` 物化新模块并扩展 injector 单测。
- [x] `.gitignore` 忽略 `.pi/env.json`。
- [x] 新增 `test-env-tool.mjs` 聚焦回归。
- [x] 更新扩展 README 与 Feature Matrix（中英）。
- [x] 工具名定稿为 `bash_env_tool`，重写 description/promptSnippet/promptGuidelines，并在 `prompts.ts` 增加 Bash guideline 联动。

### Phase 3: 验证
- [x] `node extensions/pi-grok-bash/test-env-tool.mjs` → PASS（8 项）。
- [x] `node extensions/pi-grok-bash/test-v2.1.mjs` → PASS（既有回归不退化）。
- [x] `git diff --check` → 通过；变更范围审查通过。
- [x] Rust injector：按用户指示「无需运行 cargo」，未执行 Cargo 测试/check；仅做 diff 审查。

### Phase 4: 交付
- [x] 创建 PR 记录并关联本 Issue：`docs/pr/adapter/20261108-Bash 环境变量工具与 .env 加载.md`。
- [x] 更新状态为 completed。

## 关键决策

| 决策 | 理由 |
|------|------|
| 一个 `bash_env_tool` + `scope` 参数，而非两个 tool | 用户选择；单一契约、最少注册与文档面 |
| 项目级 `<cwd>/.pi/env.json` 且加入 `.gitignore` | 用户指定 Pi 项目资源目录；避免 Agent 写入的 token 进版本库 |
| 项目级为底、session 级增量覆盖 | 「session tool 是渐进式的」；用户选择两层一次做完 |
| session 层用 `pi.appendEntry` + `getBranch()` 重放 | Pi 官方 state persistence 语义，不改 Pi 源码、不直接写 JSONL；`/resume`/树导航天然可用 |
| 按 leaf id 变化触发重放，而非每次调用全量重放 | 避免刚写入但尚未进入 branch 的事件被冲掉，也避免每次 Bash 遍历整条 branch |
| `.env` 只读 + `ENV_<KEY>` 前缀 | 用户选择；避免污染系统变量，键名一眼可辨 |
| Node 内置 `util.parseEnv`，不引入第三方库/源码转写 | 用户最终决定；项目 Node 基线 22.19 支持（API 自 v20.12/v21.7 起），无运行时依赖风险 |
| spawn env 合并，不用 `export` 命令前缀 | 用户选择；避免 shell 转义/命令注入，不改写 UI 上的命令文本 |
| 禁保留键 + 不回显值 | 用户选择；防止破坏 Pi 注入的 `PI_*`/`PATH`，减少密钥进入 LLM 上下文 |
| Bash 关闭时不注册 tool | env 无处生效，避免暴露无效工具给模型 |
| 复用 `pi-grok-bash` bundle，不新建扩展 | 复用现有注入与生命周期，避免新增 Rust 接线 |
| 工具名 `bash_env_tool` + 精简提示词 | 用户指定名称；snippet 只答「何时调用」，guidelines 只放 schema 无法表达的 `.env` 前缀、恢复语义与安全边界；校验仍留在 `execute`，不靠提示词 |

## 成本三轴

- UX：一次设置、后续 Bash 持续生效；`.env` 自动加载、项目级本地共享、session 级 `/resume` 可恢复。代价：值明文落在 `.env`/`.pi/env.json`/session JSONL，且工具结果不回显值。
- DX：新增 1 个 TS 模块 + 1 个回归文件 + injector 1 处物化；`pi-grok-bash` bundle 多一个职责，但省掉新扩展的 launch-plan/manifest 接线。
- Agent-legibility：单一 tool 契约、固定 customType、按 branch 顺序重放；行为由 Node 回归固定，不引入 side channel 或 monkey-patch。

## 已知风险 / 边界

- `.env`、`.pi/env.json` 与 session entry 中的值均为明文；不应用于长期密钥。
- 多个 grok-pi 进程并发写同一 `.pi/env.json` 可能丢更新（本轮不做文件锁）。
- `<cwd>` 取当前工作目录，不向上查找项目根，与 `createBashToolDefinition(process.cwd())` 保持一致。
- 仅增强 Bash 生效；Eval v2、原生 Bash、子代理不注入。
- Rust injector 改动未做 Cargo 编译验证（用户指示无需运行）；仅 diff 审查。

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-11-08 | 初版方案按「仅 session 级」建 Issue，随后用户扩展为两层 + `.env` | 改写本 Issue；历史取舍记入 `rejected.md`，新选择作为恢复证据 |
| 2026-11-08 | 首版回归中「每次调用全量重放 branch」冲掉刚写入的 session 值 | 改为按 leaf id 变化触发重放，并让测试 harness 真实模拟 `appendEntry` 推进 leaf |

## 相关资源

- [x] `extensions/pi-grok-bash/README.md`
- [x] `docs/FEATURE_MATRIX.md` / `docs/FEATURE_MATRIX.zh-CN.md`
- [x] `pi-main/packages/coding-agent/src/core/extensions/types.ts`（`appendEntry`、`session_start`）
- [x] `pi-main/packages/coding-agent/src/core/session-manager.ts`（`getBranch()`、`appendCustomEntry`）
- [x] `docs/adr/rejected.md`
- [x] `docs/pr/adapter/20261108-Bash 环境变量工具与 .env 加载.md`

## Notes

- Workhub 查重关键词输出过大时已落盘 `/tmp/wh-*.txt`，仅保留相关命中。
- `session_tree` 事件不带可用 session context，因此采用「leaf id 变化即重放」，覆盖 `navigateTree` 与 `/resume`。
- `.env` 与项目级文件每次 Bash spawn 前读取（无缓存），保证 Agent 刚写入的值立即生效。
- Node 22.19 文档确认 `util.parseEnv` 自 v20.12.0 / v21.7.0 起可用；本机 v24.15 实测支持引号、`#` 注释、`export K=v`、`${A}` 字面量。

---

## Status 更新日志

- **[2026-11-08]**: 状态变更 → in_progress，备注: 多轮澄清后范围定稿（.env + 项目级 + session 级一个 tool）
- **[2026-11-08]**: 状态变更 → completed，备注: 实现、Node 回归与文档完成；Cargo 按用户指示未运行
- **[2026-11-08]**: 工具名定稿 `bash_env_tool`、提示词重写与 Bash guideline 联动落地；两个 Node 回归重跑 PASS
