---
id: "2026-08-18-eval-bridge-v2-rlm-runtime"
title: "Eval Bridge v2 与 RLM runtime"
status: "in_progress"
created: "2026-08-18"
updated: "2026-08-19"
category: "adapter"
tags: ["workhub", "grok-pi", "eval", "rlm", "omp", "agent-runtime", "tool-bridge"]
---

# Issue: Eval Bridge v2 与 RLM runtime

## Goal

在保留 Eval v1 兼容性的前提下，为 `grok-pi` 提供一个必须显式 opt-in、与 v1 互斥的 JavaScript-only Eval Bridge v2。v1 继续保留 Python/JavaScript；v2 只把持久 Node.js Eval 从单向 REPL 升级为持久执行环境 + 双向 host RPC + one-shot completion + tool/subagent 编排，并作为后续 Recursive Language Models（RLMs）、程序化上下文管理和更强 agent orchestration 的研发基础。

本 Issue 是后续研发 SSOT。当前 v2 baseline 已实现，但 budget/depth、structured completion、per-call model selection、隔离、安全边界和真实 workload 评测仍需继续开发，因此状态保持 `in_progress`。

## 背景

Eval v1 由 `extensions/pi-grok-bash/index.ts` 提供：Python 与 JavaScript 各自拥有持久子进程 kernel，支持顶层 await、跨 cell 状态、reset/timeout/abort/cwd-change reset。v1 wire protocol 只有 host 发 `{id, code}`、worker 最终回 `{id, ok, value/error}`，cell 执行期间不能主动调用当前 Pi session 的工具或模型。

历史文档：`docs/issues/adapter/20260806-为 pi-grok-bash 添加 Python JavaScript Eval.md`。

对 `/Users/dengwenyu/Dev/AI/oh-my-pi` 的研究显示，OMP Eval 的关键不是多几个 helper，而是把 persistent kernel 变成轻量 orchestration layer：从 Eval 内调用 session tool、one-shot `completion()`、`agent()`、`parallel()` / `pipeline()`，让模型能在同一程序状态中完成读取环境、计算筛选、调工具/模型、聚合和继续计算。

## 非目标

- 不删除或静默替换 Eval v1。
- 不允许 v1/v2 同时注册或运行。
- 不让 Eval 绕过 Pi tool validation、extension interception、rollback/permission 链路。
- `agent()` 不实现第二套 subagent runtime，只包装现有 `spawn_subagent`。
- Pager/adapter 不承担 agent scheduler 职责。
- 不声称 Eval v2 本身就是完整 RLM 系统。
- baseline 暂不实现 explicit recursion depth、token budget、per-call model selection、structured output schema。
- 当前版本选择保持 config-only、restart-required；暂不新增 F2 UI selector。

## v1 / v2 严格互斥

启动配置：

```toml
[ui]
pi_eval = "v2"
```

规则：未设置、`v1`、非法值都走 v1；只有精确 `v2` opt-in v2。修改后重启 `grok-pi`。这是单值 version selector，不是两个 boolean，因此不存在双活状态。

启动器把选择注入 `PI_GROK_EVAL_VERSION=v1|v2`。`pi-grok-bash` 在 extension factory 初始化时只解析一次：v1 创建 Python/JS 两个 kernel；v2 只创建 JavaScript kernel，且 tool schema 只暴露 `language="js"`。模型侧始终只有一个 `eval` 工具名。

兼容纪律：默认必须保持 v1；v2 prompt/helper 不得泄漏给 v1；v2 故障不得在同进程自动双开 v1 兜底。

### Eval v2 only 隔离模式

F2 `[ui].pi_eval_v2_only = true` 是独立覆盖层：不改用户保存的 `pi_eval` 版本选择或 built-in tool 偏好，但下次启动会强制 Eval v2；当用户没有显式传 `--tools` / `--no-tools` 时保留 Pi registry，并由 host extension 在 `session_start` 把顶层 active tool 收敛为 `eval`。这样 Eval 内仍可程序化调用 registry 中被允许的工具，而顶层模型只看到 `eval`；本次进程会跳过保存的 F2 built-in tool 选择，避免它把 nested Eval 需要的工具从 registry 裁掉，但不会改写该配置。`--exclude-tools` / `--no-builtin-tools` 仍由 Pi 原生 registry policy 生效。关闭该开关后恢复原有版本与工具偏好，显式 CLI tool policy 始终优先。

Eval v2 的 skill 目录来自当前 Pi session 实际加载的 `before_agent_start.systemPromptOptions.skills`，只暴露 `disableModelInvocation != true` 的条目。程序侧提供 `skills.list/search/describe/read`；`skills.read(name)` 仅能读取该 session 白名单中的 skill `filePath`，不做额外磁盘扫描。

## 架构

```text
Pi AgentSession
  ├─ active wrapped tools ── ExtensionAPI.invokeTool()
  ├─ configured model     ── ExtensionAPI.complete()
  └─ pi-grok-bash / Eval Bridge v2
       ├─ stdin JSONL: eval / host_result
       ├─ fd3 JSONL: host_call / eval_result
       └─ Node.js REPL persistent context
```

### Pi Core host seam

v2 新增两个窄 public ExtensionAPI：

```ts
pi.invokeTool(toolName, args, signal?)
pi.complete(prompt, options?, signal?)
```

`invokeTool()` 只允许 active tool，继续经过 `prepareArguments`、`validateToolArguments`、`tool_call` block、`tool_execution_start/end`、`tool_result` patch，再执行 wrapped AgentTool。因此 nested call 不绕过 rollback、permission 和其他 extension policy。

`complete()` 使用当前 session 已配置的 model/provider/auth/retry transport，但构造一个全新的 context：无 session history、无 tools。它不是 agent loop。

之所以改 Pi Core，是因为原 ExtensionAPI 只有工具 metadata，没有从 extension 安全调用当前 session tool 的公开入口；直接访问 AgentSession 私有 registry 会形成脆弱耦合并绕过 lifecycle。

## Eval v2 wire protocol

Transport 继续使用 child process pipe，不新增 socket：host→worker 走 stdin JSONL；worker→host 走 fd3 JSONL；stdout/stderr 仍是 cell output。同语言同一时刻只允许一个 active cell，但该 cell 内可以有多个并发 host calls。

执行 cell：

```json
{"type":"eval","id":"eval-id","code":"..."}
```

worker 请求工具：

```json
{"type":"host_call","id":"host-id","evalId":"eval-id","method":"tool","tool":"read","args":{"path":"README.md"}}
```

worker 请求 one-shot completion：

```json
{"type":"host_call","id":"host-id","evalId":"eval-id","method":"completion","prompt":"classify this","options":{"reasoning":"low"}}
```

host 回答：

```json
{"type":"host_result","id":"host-id","ok":true,"value":{}}
```

cell 最终结束：

```json
{"type":"eval_result","id":"eval-id","ok":true,"value":"..."}
```

## v2 helpers

### tool.<name>()

JavaScript：

```js
const r = await tool.read({ path: "README.md" })
```

Eval Bridge v2 不提供 Python worker；Python 仅由 v1 保留。

返回标准化为 `{text, data?, content}`；若 text 是 JSON，则附带 parsed `data`。host call 必须 await；tool 必须 active；`tool.eval` 显式禁止，防止直接自递归；tool failure 会回到当前 cell 成为 Eval error。不直接序列化任意 tool details，避免 circular/huge payload。

### completion(prompt, options)

这是最接近经典 RLM recursive model call 的 helper。当前支持 `systemPrompt`、`temperature`、`maxTokens`、`reasoning`。语义是使用当前配置模型、无主 session transcript、无 tools、不启动 agent loop。

适合分类、抽取、评分、局部总结、候选筛选和短 synthesis。当前不支持 per-call model override 与 structured output schema。

### agent(prompt, options)

`agent()` 是 active `spawn_subagent` tool 的 convenience wrapper。当前可转发 description/label、subagent_type、background、model、max_turns、capability_mode。只有 `spawn_subagent` active 时可用。

它更接近 multi-agent delegation，不应和 RLM recursive completion 混为一谈。

### parallel() / pipeline()

`parallel()` 用于同一 cell 的独立 async fan-out；`pipeline()` 做 staged fan-out/fan-in。当前是轻量 helper，还没有统一 concurrency budget、backpressure、stage schema 或 partial-failure policy。

## Timeout 与 cancellation

v2 的 host tool/completion/agent 可能合理超过默认 cell timeout，因此当前实现：本地 JS 计算时 timer 正常；有 host call in-flight 时暂停 cell timer；全部 host call 返回后重新 arm；session abort 仍 reset kernel。

这是 baseline 的重要残留风险：若 host call 自身永久悬挂，cell timeout 目前不负责终止它。后续必须增加独立 host-call deadline、budget 和 cancellation tree。

## Node REPL rejected top-level await 修复

实现 error smoke 时发现 Node REPL 的 `server.eval()` callback 在顶层 rejected await 时不会被调用，错误进入内部 domain。若 `await tool.x()` reject，而 worker 只等 callback，cell 会一直卡到 timeout。

v2 增加单一 `finishEval()` guard 和 `server._domain` error handler，把 rejected top-level await 重新转成 `eval_result{ok:false}`，并防止 callback/domain 双终结。该路径已有独立 Node probe 与 host-error smoke 覆盖。

## RLMs 是什么

RLM 通常指 Recursive Language Model(s)。重点不是简单“LLM 再调用 LLM”，而是让顶层模型把一个可执行环境作为外部工作空间，对大上下文进行程序化处理，并按需递归调用语言模型处理局部子问题。

典型流程：

1. 大量代码、文档或数据保存在模型 context 外。
2. 顶层模型用程序搜索、切块、过滤、统计、排序。
3. 只对选出的局部材料发起一个或多个语言模型调用。
4. 子调用结果写回外部程序状态。
5. 程序继续聚合、比较、筛选。
6. 对仍不确定的子集再次递归调用模型。
7. 最后做 synthesis。

这把 context window 从唯一工作记忆变成一个推理组件；deterministic 数据处理和大量 intermediate state 由程序承担。

### Eval v2 与 RLM 的映射

- Persistent JavaScript kernel：外部 working memory / controller。
- `tool.*`：读取和操作环境。
- `completion()`：无状态 recursive model call，最接近经典 RLM 子调用。
- `agent()`：更重的 delegated agent loop / multi-agent path。
- `parallel()`：并行子问题求解。
- `pipeline()`：decomposition / aggregation。
- 跨 cell state：保存候选集、评分、索引和 intermediate results。

Eval v2 本身只是 execution substrate。只有主模型真正采用“程序化 decomposition → recursive model calls → 保存/聚合中间结果 → 必要时继续递归”的策略时，整体行为才是 RLM-like。

### 典型 RLM workload

分析超大 repo 时可以：先用 `tool.grep/find` 找几百个候选；在 Eval 内做 deterministic filter 到 30 个片段；并行发 30 个低 reasoning `completion()` 做相关性判断；程序评分后只保留 6 个；对其中 2 个歧义候选再次递归 completion；必要时再用 read-only `agent()` 验证调用链；最后只把小型 evidence 集交回主 agent。

这个模式的目标是降低主 context 压力，并让筛选/聚合过程留在可检查的程序状态中。

## 当前安全与控制边界

已具备：active-tool gate、schema validation、extension interception、`tool.eval` prohibition、AbortSignal 传播、v1/v2 mutual exclusion、单语言单 active cell、kernel reset、输出截断、JS rejected-await termination。

尚缺：recursive depth limit、per-cell/turn/session token & cost budget、host-call timeout、parallel concurrency cap、completion model policy、structured schema、untrusted tool-output hardening、RLM trace、deterministic replay、write-agent worktree isolation。

## Validation Evidence

Pi core focused tests：

```text
PASS (50) FAIL (0)
```

覆盖 `pi.invokeTool()` 的 validation/interception/result patch，以及 `pi.complete()` 的无历史/no-tools context 和 systemPrompt/temperature/maxTokens/reasoning 透传。

TypeScript：本次变更文件 diagnostics=0。完整 coding-agent tsgo 当前仍受 3 个与本 Issue 无关的既有 CLI 调用签名错误阻断：`config-selector.ts`、`startup-ui.ts`、`interactive-mode.ts`。不得为了本 Issue 顺手改无关 CLI。

历史 baseline smoke 曾覆盖 v1 JS/Python 与 v2 JS/Python。v2.1 已收敛为 JavaScript-only；后续 focused smoke 只要求 v1 JS/Python 与 v2 JS host tool/completion/parallel/agent/error propagation。关键标记：

```text
eval-v1-v2-smoke-ok
eval-v2-agent-error-smoke-ok
eval-v2-completion-smoke-ok
```

Rust 已通过：

```text
bash_extension_source_is_a_loadable_typescript_module ... ok
eval_bridge_defaults_v1_and_only_explicit_v2_opts_in ... ok
```

root repo 与独立 `pi-main` worktree 的 feature-file `git diff --check` 均通过。注意 `pi-main` 是独立 Git worktree，后续必须分别执行 root status 和 `git -C pi-main status`。

## 相关文件

Root repo：

- `extensions/pi-grok-bash/index.ts`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/bash_extension.rs`
- `README.md`

Pi worktree：

- `pi-main/packages/coding-agent/src/core/agent-session.ts`
- `pi-main/packages/coding-agent/src/core/extensions/types.ts`
- `pi-main/packages/coding-agent/src/core/extensions/loader.ts`
- `pi-main/packages/coding-agent/src/core/extensions/runner.ts`
- `pi-main/packages/coding-agent/src/core/extensions/index.ts`
- `pi-main/packages/coding-agent/src/index.ts`
- `pi-main/packages/coding-agent/test/extensions-runner.test.ts`
- `pi-main/packages/coding-agent/test/suite/agent-session-model-extension.test.ts`

## 外部项目参考基线

后续研发不只参考 OMP，还固定对照 Codex 和 Pi Fabric。目标不是照搬 API，而是抽取已经被这些 runtime 证明有价值的 control-plane invariants。

### OMP

- `/Users/dengwenyu/Dev/AI/oh-my-pi/packages/coding-agent/src/prompts/tools/eval.md`
- `/Users/dengwenyu/Dev/AI/oh-my-pi/packages/coding-agent/src/eval/agent-bridge.ts`
- `/Users/dengwenyu/Dev/AI/oh-my-pi/packages/coding-agent/src/eval/completion-bridge.ts`
- `/Users/dengwenyu/Dev/AI/oh-my-pi/packages/coding-agent/src/eval/concurrency-bridge.ts`

OMP 主要参考点：persistent Eval 作为 orchestration substrate、tool/completion/agent bridge、parallel/pipeline，以及把执行 helper 写进 prompt contract。

### Codex：`~/Dev/AI/codex`

重点文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/service.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/session.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/rollout_budget.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/context/rollout_budget.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/features/src/feature_configs.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/handlers/multi_agents_v2.rs`

可借鉴 invariant：

1. **Cell lifecycle 不应只有 execute→result。** Codex code mode 把 cell 建模为可观察实体，提供 execute、wait、pending frontier、terminate、shutdown。长 host call 与“cell 仍活着”是一级状态，而不是靠单一 timeout 猜测。
2. **Cancellation 是树状能力。** Nested tool delegate 显式收到 `CancellationToken`；host/session disconnect 和 cell termination 都可以沿调用边界传播。Eval v2 后续应把当前 AbortSignal 从“传下去”升级为可证明的 parent→host-call→child cancellation tree。
3. **Execution limit 与 observe/yield 分离。** Codex 暴露 `max_yield_time_ms`、heap 等 cell execution limit；这提示我们不要继续把 Eval cell computation timeout、host-call wall time、agent timeout 混成一个数字。
4. **Context token budget 与 rollout budget 分开。** Codex 的 TokenBudget 面向 context-window/auto-compaction；RolloutBudget 面向整棵 root-thread session tree 的 weighted token 消耗。后者按 sampling/prefill 权重累计，并把 remaining budget 作为 developer context 提醒每个 thread。
5. **Rollout budget 是 shared tree state。** 不是每个 subagent 各自计数；root session tree 共享一个累计值，且 reminder delivery 按 thread/window 做幂等跟踪。这一点应作为 grok-pi RLM budget 的目标模型。
6. **MultiAgentV2 有明确 concurrency/wait contract。** `max_concurrent_threads_per_session` 与 min/default/max wait timeout 都属于 runtime config，而不是 prompt 建议。

### Pi Fabric：`~/.pi/agent/npm/node_modules/pi-fabric`

重点文件：

- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/skills/fabric-rlm/SKILL.md`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/docs/agents.md`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/docs/configuration.md`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/dist/agents/budget-ledger.d.ts`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/dist/agents/semaphore.d.ts`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/dist/agents/worktree-manager.d.ts`
- `/Users/dengwenyu/.pi/agent/npm/node_modules/pi-fabric/dist/agents/types.d.ts`

Pi Fabric 对本 Issue 更直接，因为它已经把 Pi 做成一个 programmable orchestration runtime。需要重点吸收：

1. **RLM 用于 context size，不是“任务难所以递归”。** `fabric-rlm` 明确要求：能放进一个 context 的 leaf 直接 agent；只有仍然 oversized 的 partition 才递归。这个原则应进入未来 grok-pi RLM prompt/runtime policy。
2. **Context-as-variable。** 大 source、parsed values、partitions 和中间 finding 留在执行环境里，只把 path/handle/context-sized slice 交给 child；不要把完整 corpus 重新序列化进 prompt。跨 turn 再考虑 durable binding/store。
3. **递归必须有硬边界。** Fabric 同时有 `maxDepth`、`maxConcurrent`、`maxPerExecution`、`maxTokensPerChild`、`budgetUsd`。单独一个 token budget 不够。
4. **Shared recursion-tree cost ledger。** Fabric 使用 append-only JSONL ledger 跨进程累计 cost/tokens，并通过环境变量传给 descendants。并发下 budget check 是 best-effort，会有轻微 overshoot，所以还用 per-execution call count 作为 race-free ceiling。这是非常值得直接借鉴的双层设计。
5. **Batching 是预算控制手段。** `fabric-rlm` 不一次 fan-out 全部 partition，而是小 batch；若一个 batch 全失败则停止后续 spend；`budget.remaining()` 只反映已完成 usage，因此 batch size 本身就是 overshoot 控制器。
6. **Partial result 是合法终态。** 递归树不应该因为一个 partition 失败就自动重跑整棵树。Fabric 保留 completed findings、failed/not_started paths、coverage，并只针对 gap 重试。
7. **Structured child result。** Agent request 可带 JSON Schema，validated value 直接进入 workflow。这比让 parent 从自由文本二次解析可靠，应该成为 Eval v2 `agent()` / `completion()` 下一阶段重点。
8. **Worktree isolation 是写代理并发的默认候选。** Fabric 已有 `WorktreeManager` 和 `worktree: true`，并明确要求并发 child 不得编辑同一文件。未来 grok-pi recursive write agent 应采用同类 ownership/isolation 规则。
9. **Global semaphore。** Fabric 的 `Semaphore.acquire(signal?)` 说明 concurrency limit 应属于 runtime，并能被 abort，而不是仅靠 `Promise.all` helper。
10. **Usage 是可归因数据。** Agent result 记录 input/output/cacheRead/cacheWrite/cost、turns、toolCalls、nestedAgents、budget；未来 RLM trace 至少应有同等可观测性。

### DeepSeek Harness PTC：`~/.dsh/deepseek-harness`

PTC 在 DeepSeek Harness 中落实为 **Code Mode / programmatic tool calling**：标准工具能力仍存在，但模型可以只看到一个 `run_code` transport，并在一段 TypeScript 程序里通过生成 SDK 调用工具。它和 Eval Bridge v2 的目标高度相似，但有几条比当前 v2 更成熟的 contract，后续必须固定参考。

重点文件：

- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/feature/2026-06-15-code-mode.zh.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/packages/core/tools/src/code-mode.ts`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/feature/2026-07-20-code-mode-typed-tool-returns.zh.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/architecture/2026-07-20-canonical-tool-output-contract.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/feature/2026-07-26-code-mode-live-parallel-dispatch.zh.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/feature/2026-07-31-code-mode-language-dispatch.zh.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/feature/2026-07-26-code-dispatch-log-spill.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/.agents/notes/implemented/bug-fix/2026-08-07-code-mode-executor-collapse.zh.md`
- `/Users/dengwenyu/.dsh/deepseek-harness/packages/code-runtime/code-runtime-worker-thread/README.zh.md`

需要吸收的 invariant：

1. **Programmatic value 与 native presentation 必须分离。** Harness tool contract 有 canonical JSON `value` / output schema；Native UI/text 只是 projection。Code Mode 的程序拿 `value`，不会把展示字符串再 parse 成业务数据。当前 Eval v2 的 `evalHostToolValue()` 会拼 `content.text`，再尝试 `JSON.parse` 成 `data`，这是需要在 v2.1 修掉的临时桥。
2. **中间值不进入模型 transcript。** nested dispatch 的完整 structured value 直接进入程序；session durable log 可以单独 shape/spill，不能反向改变程序拿到的值。大量 RLM intermediate state 因而留在执行环境，而不是重复消耗 context。
3. **并行不是裸 `Promise.all`。** Code Mode 有 per-run scheduler：只让 dispatch body 并行；pre/post policy stage 保持有序；parallel/exclusive classification 与 native scheduler 一致；exclusive barrier 持续到 commit；结果按 submission order commit；abort 时 queued-unstarted call 被 abandon，live call 被取消并 drain。
4. **Nested execution 有 lineage token。** Code Mode sub-call 带 `rootCallId`、`parentCallId`、`subCallId` 和 opaque `parent` execution token。这既用于日志/trace，也用于区分模型直接工具调用与 programmatic nested call。
5. **Code-only 必须在 executor 强制。** Harness 曾出现“wire schema 只公布 `run_code`，但模型直呼 native tool 仍能执行”的漏洞；修复后 `mode=code` 在执行解析层拒绝 top-level native call，而带合法 parent token 的 SDK nested call 仍能访问完整工具表。以后 grok-pi 若引入 PTC-style code-only，绝不能只隐藏 schema。
6. **运行时至少有绝对 wall-clock hard stop。** Harness 把 `computeMs`（实测 event-loop busy time）与 `maxWallMs`（永不暂停）分开，再加 maxOutputBytes 和 heap cap。慢工具 await 不消耗 compute budget，但任何永不 resolve 的 promise 最终都受 maxWallMs 约束。当前 Eval v2 在 host call in-flight 时暂停唯一 timer，会允许悬挂 host call 无限延长 cell；v2.1 必须增加 never-paused wall ceiling 和 per-host-call deadline。
7. **失败是正交 taxonomy。** Code runtime 把 parse/transform、exception、invalid-output、output-limit、budget、abort、worker-exit 等作为结构化失败，而不是全部压成字符串异常。RLM runtime 后续需要同类可编程错误分类。
8. **输出/日志也需要背压。** Durable dispatch log 可以 spill；慢 spill backend 会通过 pending-work 上限反向限制新 sub-call，避免完整结果在内存无限堆积。程序拿完整 value 与日志存储策略互不干扰。
9. **参数与返回值只允许无损 JSON 边界。** Worker/host 双侧重新验证，不信任模型代码伪造的协议消息；绑定只解析 own property；`__proto__` / constructor 等原型路径不能成为能力逃逸。
10. **PTC 的 fresh worker 与我们有意不同。** Harness `run_code` 每次使用全新 worker，强调可重放和无跨 run 状态；Eval v2 为 RLM/context-as-variable 有意保留 persistent JavaScript kernel。因此我们借它的数据面、调度、预算和安全 contract，但不复制“每次 fresh worker”这一点。

### 对 grok-pi 的合并结论

综合 OMP、Codex、Pi Fabric 与 DeepSeek Harness PTC，下一版 control plane 不应做一个模糊的 `budget` 对象，也不能继续把 native tool presentation 当 programmatic API。应明确拆成两条正交主线：

- **Control plane**：ContextBudget / RolloutBudget / ExecutionBudget、cell/host-call lifecycle、cancellation tree、bounded scheduler、lineage。
- **Data plane**：canonical JSON value / output schema、native presentation、durable log/spill 三者分离。

预算仍明确区分三类资源：

- **Context budget**：当前主/子 agent context window 与 compaction 阈值；参考 Codex TokenBudget。
- **Rollout budget**：整棵用户任务/recursive tree 的 weighted tokens、cost、model-call count；参考 Codex RolloutBudget + Fabric shared ledger。
- **Execution budget**：cell CPU/wall time、host-call timeout、parallel concurrency、agent maxDepth/maxPerExecution；参考 Codex code-mode limits + Fabric hard caps。

同时必须补 programmatic value contract。当前 Pi `AgentToolResult<T>` 只有 `content`、`details`、`usage` 等字段：`content` 是给模型看的文本/图片，`details` 明确只是日志/UI 任意结构，没有 output schema，也没有稳定 canonical value。因此 v2.1 不能靠现有 `details` 冒充 typed tool return。

Cell 本身不应被一个互斥 enum 过度简化：程序可能在有 pending host call 的同时继续本地计算。v2.1 应把 **cell terminal lifecycle** 与 **host-call activity** 分开记录：cell 是 `running → settling → completed|failed|aborted|timed_out`，另有 `hostCalls: Map<id, HostCallRecord>` 表示 queued/running/settled/cancelled/timed_out。Codex 的 pending frontier / terminate 语义作为未来异步 UI/API 的参考，而不是现在硬塞一个假 `yielded` 状态。

## 对抗性 Review：v2.1 最小闭环（最终实施范围）

前面的 OMP / Codex / Pi Fabric / PTC 研究提供了很多正确方向，但如果一次全部实现，会把一个 Eval bridge 演变成新的 scheduler/budget/runtime 子系统。v2.1 的目标改为：**只修当前 baseline 已经真实存在的可靠性和语义问题，用户 API 尽量不变，核心改动尽量少。**

### v2.1 只做四件事

#### A. 稳定的 programmatic envelope，不做 typed SDK

当前 `evalHostToolValue()` 会把所有 text 拼起来，然后“如果恰好是 JSON 就自动 parse 成 data”。这很魔法：读一个 JSON 文件和一个真正返回结构化对象的工具，在程序里无法区分来源。

v2.1 简化为固定 envelope：

```ts
{
  text: string
  content: ToolContent[]
}
```

规则：

- 不再自动 `JSON.parse(text)`；需要 JSON 的程序显式 `JSON.parse(result.text)` / `json.loads(...)`。
- 不改 `AgentToolResult`，不新增 `value`、`outputSchema`，不批量迁移 core tools。
- PTC 的 canonical typed value 仍然是正确长期方向，但放到 v2.2；先把 v2.1 的行为做得可预测。
- 这样 Pi Core 不需要为了 v2.1 引入新的 tool output 类型系统。

#### B. 一个绝对 wall timeout + 一个 run AbortController

当前 v2 的 `hostCallsInFlight > 0` 会暂停唯一 cell timer，悬挂 host call 因而可以无限延长 cell。v2.1 删除 pause/resume 逻辑，直接把现有 `timeout` 定义成 **整个 Eval cell 的 wall-clock timeout，包括等待 tool/completion/agent 的时间**。

实现只需要：

- 每次 `execute()` 创建一个 run-scoped `AbortController`；
- outer signal abort → run controller abort；
- existing `timeout` 到期 → run controller abort + reset kernel；
- 所有 `pi.invokeTool()` / `pi.complete()` 都拿同一个 run signal；
- cell settle/reset/shutdown 时 abort run controller，避免 orphan nested calls；
- `timeout=0` 继续保留“显式无 timeout”的现有 escape hatch，不再新增 `maxWallMs` / `hostCallTimeoutMs` 等用户参数。

这没有 PTC 的 compute-vs-wall 双预算那么精细，但简单、可解释，并彻底修掉当前“host await 可绕过 timeout”的真实 bug。真正 compute busy-time 和 per-host-call deadline 延后。

#### C. 一个很小的 FIFO HostCallGate

不复制 PTC ordered commit scheduler，也不抽 Pi Core scheduler。v2.1 只增加一个 extension 内部 FIFO gate：

- `ToolInfo` 多暴露已有 `executionMode` 字段；这是唯一需要的 core metadata 增量。
- `executionMode === "parallel"` 的工具可以并行，否则 **fail-closed 当 sequential**。
- sequential call 等当前 active calls 清空后独占执行；它排队后，后面的 parallel call 不越过它。
- parallel call 只允许固定内部并发上限；v2.1 不增加用户配置项。
- `completion()` 视为可并行 host call；`agent()` 当前实际走 `spawn_subagent` tool，因此按该 tool 的 executionMode 处理。
- gate 覆盖完整 `pi.invokeTool()` promise；不做 pre/body/post 三阶段拆分，也不做 ordered result commit。
- run abort 时 queued call 直接 reject，running call 依赖共同 AbortSignal 取消。

这是保守但容易证明正确的调度：最多损失一点并行度，不会因为为了追求 PTC 完全等价而复制一套复杂 scheduler。

#### D. 保持现有 helper，不再加新能力

v2.1 保留已有：`tool.*`、`completion()`、`agent()`、`parallel()`、`pipeline()`。

明确不做：

- RolloutBudget / ContextBudget / `budget.remaining()`；
- recursion depth / cost ledger；
- per-call model selection；
- structured completion/agent schema；
- root/parent execution token；
- async cell `wait/terminate/pending frontier` API；
- PTC-style code-only；
- generated SDK / TypeScript types；
- durable intermediate-value store / replay system；
- 细粒度 error taxonomy。

这些都不是当前 v2 baseline 能否稳定使用的前置条件，统一放到 v2.2+。

#### E. Eval 内部 agent loop：Eval 是唯一 orchestrator，`agent()` 是 blocking leaf

对抗性 review 后，不给 Eval 再增加一套 agent controller/state machine。结构固定为：

```text
Eval program (唯一 orchestration loop)
  ├─ tool.*()       deterministic/environment work
  ├─ completion()   one-shot model leaf
  ├─ agent()        one blocking autonomous agent episode
  ├─ parallel()     explicit fan-out
  └─ pipeline()     explicit staged composition
```

规则：

- **循环属于 Eval 程序本身。** 需要 retry、refine、map/reduce、条件分支时，直接写普通 JavaScript；host 不隐藏第二套 agent loop。
- **`agent()` 永远是 blocking leaf。** Eval helper 不再接受/转发 `background=true`；内部固定按 foreground `spawn_subagent` 等待完成并返回最终 envelope。需要多个 agent 并发时，使用 `parallel([() => agent(...), ...])`。
- **不在 Eval `agent()` 中暴露 handle/wait/status/steer/follow-up。** 这些属于 native subagent UI/runtime 的低层控制面；把它们塞进 Eval helper 会产生跨 cell 生命周期和额外状态机。
- **默认 child 保持 leaf capability。** 当前 grok 子代理的 builtin capability 只包含 `read/bash/edit/write/grep/find/ls` 的子集，默认没有 `eval` 或 `spawn_subagent`。这个隔离应保留，不自动继承父 Eval kernel，也不把 agent→agent recursion 作为 v2.1 能力。
- **child 看不到父 conversation / kernel variables。** Eval 程序必须把需要的最小上下文显式写入 prompt；这与 context-as-variable 一致，避免把整个 parent state 隐式复制给 agent。
- **`completion()` 与 `agent()` 分工明确。** 分类、抽取、评分、短 synthesis 优先 `completion()`；只有需要工具循环/自主探索时才使用 `agent()`。
- **`max_turns` 可继续作为可选 soft safety cap，但不是 orchestration primitive。** 当前 subagent 的语义是在达到上限后注入一次 end-and-summarize steering，因此不要把它当精确 hard budget 或递归深度控制。
- **`capability_mode`、`subagent_type`、`model` 保留。** 它们只是一次 leaf episode 的执行配置，不改变 ownership。
- generic `tool.spawn_subagent(...)` 仍是底层 native tool 能力；Eval 的推荐/稳定 agent API 是 `agent()`。若未来要做 background actor 模型，应另立版本设计，不把它偷偷塞回 `agent()`。

这样 agent 结构只有两个层级：**Eval controller → leaf agent**。递归需要由 Eval 代码显式再次调用 leaf，而不是 child 隐式再 spawn child。真正的 recursive agent tree、shared budget、structured child schema 等统一留到 v2.2+，并且只有真实 workload 证明必要时才实现。

### v2.1 最小内部结构

不引入 `EvalRunContext` / `HostCallRecord` 大状态机。现有 `PendingEval` 只需增加/调整少量字段：

```ts
type PendingEval = {
  id: string
  resolve: ...
  reject: ...
  output: Buffer
  truncated: boolean
  timer?: Timeout
  outerSignal?: AbortSignal
  runController: AbortController
}
```

`hostCallsInFlight`、`suspendTimerForHostCall()`、`resumeTimerAfterHostCall()` 删除。HostCallGate 是 kernel/extension 级独立小对象，不进入 wire protocol。

Wire protocol 继续保持当前 v2 的四种消息：`eval` / `host_call` / `host_result` / `eval_result`。不为了 v2.1 做 protocol version bump。

### v2.1 验收标准

必须新增/保留这些 focused tests：

1. v1 JS/Python 行为不变。
2. v2 tool schema 只允许 `language="js"`，不创建 Python v2 kernel；v2 JS tool/completion/agent smoke 继续通过。
3. tool 返回文本 `{"x":1}` 时，programmatic result 仍是 text，不再自动产生 `data.x`。
4. 一个永不 resolve 的 host call 会被 cell `timeout` 终止，并且 host signal 已 aborted。
5. outer abort 会取消当前 host call 并 reset kernel。
6. 两个显式 parallel tool 可 overlap，但不超过内部 cap。
7. sequential tool 与任何其他 nested tool 不 overlap；后来的 parallel call 不能越过已排队 sequential call。
8. cell 正常结束后没有 queued/running orphan call。
9. Eval `agent()` 收到 `background=true` 时明确 fail fast；稳定 helper 永远不会返回 background handle。
10. 多个 agent 的并发只通过 `parallel([() => agent(...), ...])` 完成，而不是 `agent(background=true)` + wait/status。
11. 默认 Eval agent child 的 active tools 不包含 `eval` / `spawn_subagent`，保持 leaf 隔离。
12. `git diff --check`、focused core/extension tests、Rust embedding/config tests继续通过。

### v2.1 可执行论证 Demo

在修改 production runtime 前，先用 `extensions/pi-grok-bash/demo-v2.1.mjs` 对四个最小不变量做零依赖可执行论证：

1. **稳定返回：** 定义 `E(C) = { text(C), C }`。同一 `content=C` 唯一映射到同一 envelope，不存在 JSON-looking text 触发的第二条解析分支。
2. **绝对终止：** 对 timeout `T`，cell 的可观察终止时间满足 `T_cell <= T + ε`（`ε` 为 event-loop scheduling jitter）；host call 不能暂停 wall timer，run-scoped abort signal 同时传播给正在等待的 host work。
3. **有界 FIFO 并发：** 若内部 parallel cap 为 `K`，任意时刻 `0 <= P(t) <= K`；队首 sequential call 只有在 `P(t)=0` 时启动，并作为 barrier 阻止其后的 parallel call 越过。
4. **leaf agent：** 稳定 helper 的隐式 agent 深度固定为 `D_implicit = 1`；`background=true` fail fast，默认 child tool set 不含 `eval` / `spawn_subagent`。多 agent 并发由父 Eval 程序显式 `parallel()` 组合。

运行：

```bash
node extensions/pi-grok-bash/demo-v2.1.mjs
```

该 demo 先证明 v2.1 设计可以用最小结构满足上述 safety/liveness 约束；production 实现随后由 `extensions/pi-grok-bash/test-v2.1.mjs` 直接驱动实际 extension/kernel 做 focused regression。

### 对抗性结论

v2.1 的核心不是“做成完整 RLM runtime”，而是把现有 v2 从 prototype 变成**简单、可预测、不会挂死、不会乱并发**的 stable bridge。只要这四件事完成，就应该停止 v2.1 scope；RLM budget、typed canonical value、code-only 等高级能力必须通过后续真实 workload 证明有需求后再加。

## v2.2+ 设计素材（非 v2.1 实施范围）

以下内容保留为研究记录，不是下一轮实现 checklist。

### 1. 未来设计原则

以下原则只作为 v2.2+ 研究方向，不是 v2.1 的实施要求：

1. v1 行为与默认值保持不变；后续版本仍只演进显式 opt-in 的 v2。
2. persistent kernel 是核心能力，不能为了复制 PTC 而改成每 cell fresh worker。
3. programmatic nested call 最终应得到 canonical structured value，而不是依赖 presentation text parse。
4. security/approval policy 必须继续作用于 nested calls；presentation/log shaping 不得偷偷改变 canonical value。
5. 若未来引入完整预算系统，cell compute、absolute wall time、单个 host call、整棵 recursive rollout 应分开建模。
6. 若未来需要更强并发，应统一 bounded scheduler，并尊重 native tool 的 parallel/sequential 语义。
7. cancellation 应能从 outer eval call 向 queued/running host calls 传播；cell 结束后不能留下 orphan work。
8. 若未来引入 lineage，每个 nested call 可有 root/parent/subcall identity，用于预算、日志、调试和 code-only enforcement。
9. 若错误处理复杂度确有需求，再引入结构化 failure kind；模型可见文本只是 projection。
10. protocol/data 边界继续只接受可验证的无损 JSON；大中间值以后再通过 handle/store 扩展。

### 2. Programmatic Tool Result Contract（P0）

目标是在 Pi core 增加真正的 programmatic result，而不是继续扩展 `evalHostToolValue()` 的猜测逻辑。

候选兼容扩展：

```ts
interface AgentToolResult<TDetails> {
  content: (TextContent | ImageContent)[]
  details: TDetails
  usage?: Usage
  value?: JsonValue
  // existing fields...
}

interface AgentTool<...> {
  // existing fields...
  outputSchema?: TSchema
}
```

语义：

- `content`：Native/model presentation；可以被 truncation、preview、spill locator 等策略塑形。
- `details`：UI/log/debug metadata；继续允许任意结构，不作为程序 API。
- `value`：可选 canonical programmatic JSON value；如果存在且声明 outputSchema，则 runtime 验证后才允许交给 Eval。
- `tool_result` hook 后续扩展 event/patch 同样携带可选 `value`。只 patch `content` 的 presentation extension 不影响 `value`；确实需要 redact/replace programmatic output 的 policy 必须显式 patch `value`。
- 如果未来引入 `value`，没有该字段的旧工具必须继续兼容；稳定 fallback 至少保留 `content` / `text`。
- `data` convenience 字段不应成为长期 contract；v2.1 已选择更简单的方向：删除自动 JSON 推断，由调用程序显式解析 text。

优先给 `read/grep/find/ls/bash/edit/write` 设计 canonical output schema。不能直接拿现有 `details`：例如 `grep` details 只有 truncation/limit metadata，真正 matches 已被渲染成文本；`read` details 也主要是 truncation/image metadata。

### 3. Nested Invocation Lineage（P0）

每次 Eval tool execution 建立一个 host-side `EvalRunContext`：

```ts
interface EvalRunContext {
  runId: string
  outerToolCallId: string
  rootExecutionId: string
  language: "js"
  controller: AbortController
  startedAt: number
  phase: "running" | "settling" | "completed" | "failed" | "aborted" | "timed_out"
  hostCalls: Map<string, HostCallRecord>
  executionBudget: ExecutionBudgetState
  rollout?: RolloutBudgetHandle
}

interface HostCallRecord {
  id: string
  parentRunId: string
  sequence: number
  kind: "tool" | "completion" | "agent" | "budget"
  toolName?: string
  state: "queued" | "running" | "completed" | "failed" | "cancelled" | "timed_out"
  startedAt?: number
  settledAt?: number
  controller: AbortController
}
```

Worker 不拥有 authoritative budget/lineage，也不能自行伪造 root identity。`host_call` 里的 id/evalId 只是协议 correlation；真正 root/parent token 由 host 在当前 Eval execute scope 建立，并传入 Pi core nested invocation。

未来若做 PTC-style code-only，executor 用这个 opaque nested token 区分“顶层模型直呼 native tool”与“合法 Eval SDK 子调用”，不能靠 prompt 或 schema omission。

### 4. 三层预算

#### ContextBudget

Owner：AgentSession/model runtime。

负责当前主/子 agent context window、compaction threshold、context usage。Eval 不复制这份计数，只能读取 snapshot。参考 Codex TokenBudget。

#### RolloutBudget

Owner：root user task / recursive session tree。

至少累计：weighted tokens、provider cost、completion calls、agent calls、recursive depth。`completion()` settled 后立即 debit usage；`agent()` 需要从 subagent result 汇总 usage。后续 child AgentSession 必须继承同一个 RolloutBudget handle，而不是每个 session 新建余额。

并发预算采用 Fabric 式“双层约束”：completed-usage ledger 可以 best-effort overshoot，但同时用 `maxDepth`、`maxCalls`、`maxPerExecution`、concurrency cap 提供 race-free hard ceiling。

#### ExecutionBudget

Owner：每个 EvalRunContext。

至少包含：

```ts
interface EvalExecutionLimits {
  maxWallMs: number
  hostCallTimeoutMs: number
  maxHostCalls: number
  maxConcurrentHostCalls: number
  maxOutputBytes: number
  // target capability; implementation may vary by runtime
  computeBudgetMs?: number
}
```

关键变化：

- `maxWallMs` 从 cell 启动起一直计时，**永不因为 host call 暂停**；这是防永久 promise/悬挂 tool 的最后硬边界。
- 每个 host call 有自己的 deadline + AbortController；长 agent 可以有独立 policy，而不是通过暂停 cell timer 获得无限时间。
- 当前 `hostCallsInFlight > 0 => clear timer` 的逻辑在 v2.1 应废弃。
- `computeBudgetMs` 只在能可靠测量时作为“本地计算预算”。Harness Node worker 用 event-loop busy time；我们当前 v2 是 persistent child-process JS，精确 busy-time metering 仍需要单独实现。**在精确 compute metering 落地前，maxWallMs 必须作为不可绕过的安全上限，不能把现有 pause timer 改名冒充 compute budget。**

### 5. Host Call Scheduler（P0）

当前 `parallel()` 是 worker 侧 `Promise.all` / `asyncio.gather`，会并发产生 host_call，但 host 端没有统一调度语义。如果真实 workload 证明 v2.1 的简单 FIFO gate 不够，v2.2+ 才考虑更完整的 authoritative scheduler：

- bounded queue，受 `maxConcurrentHostCalls` 限制；
- queued call 可因 parent abort/deadline 直接 abandon；
- running call 获得 linked AbortSignal；
- tool nested call 必须尊重 tool `executionMode`；parallel tool 可重叠，sequential/exclusive tool 在 barrier 内独占；
- security/validation/pre-execute ordered stage 不应无界并发；tool body 才是主要并发区；
- commit/settle 顺序至少在同一个 Eval run 内可重放，优先按 submission sequence 记录；
- completion/agent 进入相同 call-count/rollout budget，但可以有独立 provider/subagent semaphore；
- cell settle 时先 abort run controller，再 drain queued/running host calls，然后才关闭 run；不得留下 orphan tool/agent。

长期最好抽出 Pi Core `NestedToolExecutionScheduler` 与 native agent-loop 共用，而不是在 `pi-grok-bash` 复制 tool scheduling。PTC 的实践说明“程序化调用”和“native 调用”若拥有两套并发语义，最终会出现 exclusive/tool-policy 漂移。

### 6. Cancellation Tree（P0）

为每次 Eval run 建 root AbortController；每个 host call 创建 child controller，并同时链接：

- outer Eval tool signal；
- Eval `maxWallMs`；
- host-call deadline；
- RolloutBudget hard stop；
- explicit future terminate/cancel。

任何一个触发都 abort child。Eval run 进入 terminal/settling 后：拒绝新 call、abandon queued call、abort running call、等待 bounded drain。worker 如果仍存活，只接受属于当前 run 的一次 settle reply；迟到/重复 `host_result` 丢弃。

这比当前“把同一个 pending.signal 直接传给所有 host call”更清晰，也为未来 Codex-style `terminate(cellId)` 留出接口。

### 7. Error Taxonomy（P0）

协议错误不应只返回任意 `error: string`。建议 v2.1 定义：

```ts
type EvalFailureKind =
  | "invalid-request"
  | "invalid-output"
  | "tool-error"
  | "completion-error"
  | "agent-error"
  | "host-call-timeout"
  | "cell-wall-timeout"
  | "budget-exhausted"
  | "aborted"
  | "kernel-exit"
  | "protocol-error"
  | "output-limit"
```

Wire 继续带 model-readable message，但 parent/kernel 逻辑根据 kind 决策，不再 parse error string。JS helper 可暴露统一的 `EvalHostError(kind, message, callId?)`，使程序可以只捕获可恢复子调用错误。

### 8. Budget / Runtime Introspection（P0/P1）

增加 synthetic host method，而不是让 worker维护权威余额：

```js
const b = await budget.remaining()
// { rollout, execution, depth, calls, concurrency }
```

Snapshot 只读，可能因并发已完成 usage 而变化；prompt 必须说明它是 planning signal，不是 reservation。真正 admission 仍由 host scheduler 原子/硬 cap 决定。

### 9. PTC-style Code-only（P1，非 v2.1 baseline）

继续保持现有“Eval + native tools 都可见”的 both-like 模式。只有在 canonical value、schema projection、nested token、scheduler 等长期基础成熟且真实 workload 证明收益后，才评估可选 code-only：

- wire 只公布 `eval`（或未来独立 program transport）；
- active native tools 仍投影成 Eval SDK/schema；
- top-level 模型直呼 native tool 必须在 executor fail-closed；
- 合法 nested token 才能执行 native tool；
- prompt 仍可保留跨工具路由规则，但明确只能经 Eval programmatic path 调用。

仅隐藏 schema 不算 enforcement，Harness 的 executor-collapse bug 已证明这一点。

### 10. Persistent Kernel 与 Replay

PTC 每次 fresh worker 的可重放性不能直接搬到 Eval，因为跨 cell state 是我们做 RLM/context-as-variable 的核心价值。未来若补 replay/trace，可采用以下折衷：

- kernel state 继续跨 cell；
- 每个 cell/host call 都记录 lineage、输入摘要、canonical result metadata、budget delta、terminal kind；
- durable log 不要求记录整个大 intermediate value；大值后续通过 artifact/handle store 提供 provenance；
- `reset=true` 明确形成 state epoch boundary；future trace/replay 以 epoch + cell sequence 为单位，而不是假设 session log 能单独重建任意 JavaScript heap。

### 11. v2.2+ 候选实施顺序

以下顺序只在 v2.1 最小闭环稳定后重新评估：

1. **Canonical value seam**：`AgentToolResult.value` + optional outputSchema + invokeTool 返回 canonical value；移除新代码对 text→JSON parse 的依赖。
2. **EvalRunContext / lineage / failure kind**：先把 host state显式化，再改 timeout。
3. **Never-paused maxWallMs + per-host-call timeout + child AbortController**。
4. **Host bounded scheduler**：max calls / max concurrent / exclusive semantics / drain。
5. **RolloutBudget handle**：先 completion usage，再接 subagent usage/depth/cost。
6. **`budget.remaining()`**。
7. **Structured `completion()` / `agent()` output schema**。
8. 再研究 code-only / generated SDK / durable intermediate store。

前四项完成前，不建议继续堆 map/reduce/recursive helper；否则只会把当前未受控的并发、文本解析和 timeout 问题放大。

## 后续研发路线

### P0 v2.1 minimal reliability

- [x] 删除 text→JSON 自动推断；nested tool 固定返回 `{text, content}` envelope。
- [x] `timeout` 改为 v2 cell 的绝对 wall-clock ceiling；删除 host-call pause/resume timer。
- [x] run-scoped AbortController 统一链接 outer abort、timeout、nested tool/completion，并在 cell settle 时取消剩余工作。
- [x] `ToolInfo` 仅增加 `executionMode` metadata。
- [x] Eval extension 增加简单 FIFO HostCallGate：parallel 固定内部 cap=4；未显式 parallel 的工具 fail-closed sequential；sequential barrier 不允许后来的 parallel 越过。
- [x] focused tests 覆盖 hung host timeout、abort、parallel cap、sequential barrier、no orphan、JSON-looking text 不自动 parse。
- [x] Eval `agent()` 固定为 blocking leaf：`background=true` 明确报错，并发统一通过 `parallel([() => agent(...), ...])`；默认 child 不继承 `eval/spawn_subagent`。
- [x] 不增加任何新的用户配置项或 helper。

P0 验证命令：

```bash
node extensions/pi-grok-bash/demo-v2.1.mjs
node extensions/pi-grok-bash/test-v2.1.mjs
cd pi-main && npm test --workspace=@earendil-works/pi-coding-agent -- test/agent-session-dynamic-tools.test.ts
cd pi-main && npm test --workspace=@earendil-works/pi-coding-agent -- test/suite/agent-session-model-extension.test.ts
cargo test -p xai-grok-pager-bin --bin grok-pi eval_bridge_defaults_v1_and_only_explicit_v2_opts_in
cargo test -p xai-grok-pager-bin --bin grok-pi bash_extension_source_is_a_loadable_typescript_module
git diff --check
```

其中 production regression 覆盖 10 组实际 kernel/host-call 场景并连续 3 次稳定通过；Core metadata regression 4/4 通过；Pi host API regression 14/14 通过；两项 Rust focused test 各实际执行 1 test。

### P1 Programmatic result plane（v2.2+）

- [ ] `AgentToolResult` 增加 optional canonical `value: JsonValue`；tool definition 增加 optional `outputSchema`。
- [ ] runtime 验证 canonical value 与 outputSchema；无 schema 时至少做无损 JSON validation。
- [ ] `tool_result` event/patch 明确区分 `content` 与 `value`；presentation-only patch 不应隐式重写 programmatic value。
- [ ] `pi.invokeTool()` 返回 canonical value；旧工具 fallback 使用稳定 envelope，不再通过 `JSON.parse(content.text)` 猜类型。
- [ ] 优先迁移 `read/grep/find/ls/bash/edit/write` 的 typed value；为每个值定义 truncation/partial semantics。
- [ ] durable nested-call log/spill 与 program value 分离；日志 shaping 不能延迟或改变 program-facing settle。
- [ ] 为大 intermediate value 设计后续 handle/artifact seam，避免无界 JSONL copy；在该 seam 前先给 protocol payload 明确上限/失败 kind。

### P1 RLM control plane（v2.2+）

- [ ] 设计三层预算：ContextBudget / RolloutBudget / ExecutionBudget，避免一个字段同时承担 context、成本和 wall time。
- [ ] RolloutBudget 绑定整棵 root task tree；至少累计 weighted tokens、cost、completion/agent calls，并支持 remaining reminders。
- [ ] 增加 recursion `maxDepth`、每次 Eval `maxPerExecution`、每棵树 `maxCalls` 等 race-free hard cap。
- [ ] `maxWallMs` 作为 never-paused cell hard ceiling；host call 独立 timeout；cell local-compute budget 与 child/agent timeout 分离。
- [ ] 废弃 `hostCallsInFlight > 0` 时暂停唯一 cell timer 的旧 v2 策略。
- [ ] `parallel()` 改为可 abort 的统一 concurrency semaphore / queue，不再直接无限 `Promise.all`；nested tool 调用必须尊重 native parallel/exclusive executionMode。
- [ ] cancellation tree：父 cell terminate/abort 能取消 pending completion/tool/agent；参考 Codex CancellationToken lifecycle。
- [ ] 定义 cell lifecycle / pending frontier，使长 host call 可观察、可 wait、可 terminate，而不是只暴露最终 tool result。
- [ ] 将 `budget.remaining()` / depth / call counts 暴露给 Eval 程序。
- [ ] 并发预算采用“shared ledger + hard call/depth/concurrency caps”双层约束，接受 completed-usage ledger 在并发下的 best-effort overshoot。
- [ ] 递归 fan-out 默认分 batch；全失败 batch 停止后续 spend；保留 partial coverage，不自动重跑成功 partition。

### P1 Completion contract（v2.2+）

- [ ] per-call model selection，并受 scoped-model policy 约束。
- [ ] structured output / JSON schema。
- [ ] 返回统一 model/usage/cost metadata。
- [ ] 明确 cache/session affinity。
- [ ] provider/quota/abort/validation/budget error taxonomy。

### P1 RLM primitives（v2.2+）

- [ ] map/reduce 型结构化 helper。
- [ ] bounded fan-out / chunk iterator。
- [ ] partial failure / retry policy。
- [ ] provenance：子结论关联 source chunk/tool evidence。
- [ ] intermediate-result store，避免大结果反复 JSONL 往返。
- [ ] recursion trace / decomposition tree observability。

### P1 Agent delegation hardening（v2.2+）

- [ ] `agent()` structured output。
- [ ] worktree isolation。
- [ ] nested write-agent conflict handling。
- [ ] 更保守 capability default。
- [ ] agent/completion budget 统一核算。

### P2 RLM workload benchmark（v2.2+）

建立固定 benchmark：大 repo root-cause search、大 JSON/日志 anomaly analysis、多文档 evidence synthesis、代码迁移候选筛选、需要两层递归 refinement 的任务。

至少记录成功率、主 context token、recursive completion token、总模型调用次数、wall time、tool calls、recursion depth、failure/retry count，并和 v1 + 顶层普通 tool loop 做 A/B。

## 研究问题

1. `completion()` 默认继承主模型还是默认 scoped cheap model？
2. RLM budget 应按 Eval call、agent turn 还是整个 user prompt cycle？
3. host-call timeout 与 cell timeout 如何组合，既允许长 subagent 又不悬挂？
4. `parallel()` 并发上限由 provider、tool 类型还是统一 runtime policy 决定？
5. structured output 应复用 Pi provider schema 还是 Eval bridge 自己 validation？
6. recursive completion 的 cache/session affinity 会否影响成本和隔离？
7. intermediate state 何时留在 kernel，何时需要外部 artifact store？
8. 是否允许 completion 间接再触发 completion；若允许最大 depth/budget 如何保证不爆炸？
9. `agent()` 是否应默认 read-only，只有显式 capability 才允许写？
10. 如何记录 RLM trace 又不污染主 transcript？
11. 是否需要把 bridge protocol 正式命名为 `pi-grok-eval/v2` 为 v3 留兼容层？
12. 什么 benchmark 能证明 v2/RLM 相比普通顶层 agent+tool loop 有净收益？

## 开发纪律 / 不得回归

- 不默认打开 v2。
- 不删除 v1 worker。
- 不把两个版本变成两个同时注册的 eval 工具。
- nested tool call 不得绕过 extension interception。
- `completion()` 不得偷偷接入主 transcript。
- `agent()` 不得误描述成 RLM 本身。
- root 与 `pi-main` 两个 worktree 必须分别审计。
- 既有无关 TypeScript/Cargo warning/error 必须单独记录，不得为了全绿扩大 scope。

## Status 更新日志

- **2026-08-18**: 完成 OMP Eval/completion/agent/concurrency bridge 研究；确认 v1 单向协议无法支持执行中 host callback。
- **2026-08-18**: Pi Core 新增 `ExtensionAPI.invokeTool()`，nested tool 继续走 validation 与 extension interception lifecycle。
- **2026-08-18**: 早期 Eval v2 baseline 完成 Python/JS 双向 host RPC、`tool.*`、`parallel()`、`pipeline()`、`agent()`。
- **2026-08-18**: 修复 Node REPL rejected top-level await 导致 host failure timeout 的问题。
- **2026-08-18**: Pi Core 新增无历史/no-tools `ExtensionAPI.complete()`；早期 v2 Python/JS baseline 增加 `completion()`。
- **2026-08-18**: v2.1 收敛为 JavaScript-only；删除 Python v2 worker，v2 schema 只允许 `language="js"`，v1 Python/JS 保持不变。
- **2026-08-18**: `[ui].pi_eval = "v1" | "v2"` 互斥启动选择完成；默认 v1，仅 exact v2 opt-in；README 已补配置说明。
- **2026-08-18**: core focused tests 50/50、worker smoke、Rust embedding/config tests、双 worktree diff audit 通过；本次变更文件 TypeScript diagnostics=0。
- **2026-08-18**: 建立本长期研发文档；状态保持 `in_progress`，后续从 RLM control plane / budget/depth 开始。
- **2026-08-18**: 将 `~/Dev/AI/codex` 与 `~/.pi/agent/npm/node_modules/pi-fabric` 纳入固定外部参考；提炼 Codex cell lifecycle/cancellation/shared RolloutBudget，以及 Fabric context-as-variable、maxDepth/maxConcurrent/maxPerExecution、shared cost ledger、batch/partial-result、structured schema 与 worktree isolation 作为 v2.1 control-plane 基线。
- **2026-08-18**: 纳入 `~/.dsh/deepseek-harness` PTC/Code Mode：确认 typed canonical value 与 native presentation 分离、nested execution token、executor-level code-only collapse、ordered bounded parallel scheduler、compute/wall 双预算、structured failure、durable dispatch-log spill 等设计。由此将 canonical programmatic result plane 升为 v2.1 P0，并明确废弃 text→JSON guessing 与 host-call 暂停唯一 timer 的方向。
- **2026-08-18**: 完成一版扩展型 v2.1 设计草案：Programmatic Tool Result、EvalRunContext/lineage、三层 budget、per-host-call timeout、bounded scheduler 等；随后经对抗性 review 判定范围过大，整体降级为 v2.2+ 研究素材。
- **2026-08-18**: 对抗性 review 将 v2.1 收敛为四项最小闭环：固定 `{text, content}` envelope 并删除自动 JSON 推断；现有 `timeout` 改为绝对 cell wall timeout + run-scoped AbortController；仅暴露 `ToolInfo.executionMode` 并实现 extension 内简单 FIFO HostCallGate；不新增 helper/config。RolloutBudget、typed value/schema、lineage、code-only、structured output 等全部移至 v2.2+。
- **2026-08-18**: 单独 review Eval agent loop：确认当前 builtin child capability 默认不包含 `eval/spawn_subagent`。v2.1 定义为 `Eval program → blocking leaf agent` 两层结构；`agent()` 不承担 background actor/handle/wait/status/递归 controller，并发用已有 `parallel(agent)`，`completion()` 继续承担轻量 one-shot model leaf。
- **2026-08-18**: v2.1 P0 production 实现闭环：固定 `{text, content}`、绝对 wall timeout + run-scoped abort、cap=4 FIFO HostCallGate、blocking leaf `agent()` 全部落地；`ToolInfo` 仅新增 `executionMode` metadata，`spawn_subagent` 显式标记 parallel。production regression 10/10、Core metadata regression 4/4、Rust config/embedding focused tests 通过。
- **2026-08-18**: 完成 `pi-grok-bash` 纯结构拆分：1548 行单体入口收敛为 429 行 `index.ts`，Eval runtime 移至 `eval.ts`，background Bash lifecycle 移至 `bash-tasks.ts`，共享 process/limit helper 移至 `shared.ts`；Rust embedding 从单个 `NamedTempFile` 改为保活 `TempDir` 并 materialize 四个模块，避免引入 bundler/runtime build dependency。
- **2026-08-19**: Eval v2 增加 session-loaded skill discovery/read：`skills.list/search/describe/read` 只投影 Pi 实际加载且允许模型调用的 skills；focused production regression 扩展到 14/14，通过 hidden-skill 与白名单读取覆盖。
- **2026-08-19**: 新增 F2 `pi_eval_v2_only` 覆盖层：强制 v2 + Pi 原生 registry `--tools eval`，显式 `--tools`/`--no-tools` 优先且关闭后恢复底层偏好。真实 Pi 0.84.2 + `3838-completions/ark-code-latest` 独立 tmux 验证：仅设置该开关即可进入 v2，Eval 内 `Object.keys(tool) == []`，`skills.search("models")` 命中 session skills，`skills.read("models-config")` 成功；测试 tmux/临时目录已清理。`./build.sh` 仍被 `pi-main` 既有 3 个 TS2554 阻断，受保护 Cargo binary build、focused Rust tests 与 `git diff --check` 均通过。
- **2026-08-19**: 修复 eval-v2-only 的 host-tool 空目录：不再用 `--tools eval` 过滤 Pi registry；grok-pi 只在 host-owned eval-only policy 生效时向 bridge 标记模式，并由 extension 在 `session_start` 将顶层 active set 收敛到 `eval`。普通 v2 与显式 CLI tool policy 继续只允许 active tools；eval-only 下 Eval catalog 可见 registry 中仍被允许的工具，inactive nested call 绕过原生 `invokeTool` 的 active gate，改走 captured wrapped extension/core 路径并保留 Pi tool lifecycle hooks。
- **2026-08-19**: 完成 eval-v2-only host-tool 修复的真实 Pi 运行时验证：Pi `0.84.x` + `3838-completions/ark-code-latest` 下，`session_start` 后顶层 `getActiveTools()` 为 `["eval"]`；Eval 内 `Object.keys(tool)` 可见 registry 中仍允许的 `read` 等工具，`await tool.read({path:"README.md", ...})` 成功返回文件内容。另以 `--exclude-tools read` 做反向探针，Eval catalog 不再包含 `read`，确认显式 CLI registry policy 仍保持权威。
