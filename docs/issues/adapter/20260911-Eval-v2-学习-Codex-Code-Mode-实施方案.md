---
id: "2026-09-11-eval-v2-learn-codex-code-mode"
title: "Eval v2 学习 Codex Code Mode 实施方案"
status: "planned"
created: "2026-09-11"
updated: "2026-09-11"
category: "adapter"
tags: ["workhub", "grok-pi", "eval", "code-mode", "runtime", "codex", "rlm", "tool-bridge"]
---

# Issue: Eval v2 学习 Codex Code Mode 实施方案

## 结论

我们应该学习 Codex Code Mode 的 **runtime contract**，而不是复制它的系统规模。

当前 Eval v2.1 已经解决了 Pi 内部最重要的一层：模型可以通过 JS/Python 程序调用 nested tools、`completion()`、`agent()`，并依靠 `HostCallGate` 做 executionMode-aware 调度；已有 focused regression 证明 absolute wall timeout、abort、并发/FIFO、background task、tool/skill discovery 和 image forwarding 等关键行为可用。

Codex 真正领先的是另一层：它把一次 code execution 从“一个 child process 里的 eval”提升为 **session-owned、cell-oriented、可观察、可终止、可远程承载、具备服务级背压的 runtime**。

因此 v2.2+ 的方向不是做 Codex clone，而是按收益/复杂度分阶段吸收以下 contract：

1. **Session-owned durable state**：持久状态不再绑定 interpreter lifetime。
2. **Runtime provider boundary**：调用方不关心执行是在本地 child process、worker、isolate 还是远端 host。
3. **Service-level bounds**：在 `HostCallGate` 之上增加 host-wide active-cell / pending-call / queue 限制。
4. **Typed/deferred nested tool catalog**：从纯动态 `search/describe` 进化到 compact signature + 长尾 deferred catalog。
5. **Live cell lifecycle**：只有真实 workload 需要跨 turn 保持程序状态时，才引入 `yield/wait/terminate/notify`。
6. **Cancellation tree / terminal state machine**：随 live cell 一起上，不提前复制 actor 复杂度。
7. **Canonical programmatic value**：最终把程序值和模型展示文本分离，但不能拿 Pi 现有 `details` 字段冒充稳定 value contract。
8. **Remote provider**：只有部署隔离、共享 runtime 或独立升级真正需要时再做。

> “Everything should be made as simple as possible, but no simpler.” —— 这类 runtime 最容易犯的错，就是把 Codex 已经付过的复杂度账单原样抄一份，却没有对应 workload。

## 本 Issue 的目标

把 2026-09-11 对 `~/Dev/AI/codex` 的源码研究整理成一个可执行的工程计划，明确：

- 我们到底应该学什么；
- Codex 对应的真实源码路径、类型和函数；
- grok-pi 当前对应代码在哪里；
- 每项能力如何以最小改动接入；
- 哪些能力现在明确不做；
- 每阶段验收标准和回归命令是什么。

本 Issue **不直接实现功能**。它作为 v2.2+ 的实施 SSOT，后续每个阶段应拆成独立可验证子任务。

## 当前基线

### grok-pi 当前实现

主要代码：

- `extensions/pi-grok-bash/index.ts`
  - extension 入口；创建 `EvalSessionToolBridge`、`HostCallGate`、`PersistentEvalKernel`。
  - v2 host call 路由由 `invokeEvalHostCall` 统一进入。
  - `evalHostCallGate.run(...)` 对 `completion()` 和 tool dispatch 做并发/串行约束。
  - foreground/background Eval、任务提升和 task API 都在这里接线。
- `extensions/pi-grok-bash/eval.ts`
  - `PersistentEvalKernel`
  - `HostCallGate`
  - `EvalHostCallHandler`
  - v2 worker wire protocol、`store/load`、`tool.*`、`tools.*`、`skills.*`、`completion()` 等 runtime helper。
- `extensions/pi-grok-bash/tool-bridge.ts`
  - `EvalSessionToolBridge`
  - `EvalToolMetadata`
  - active tool capture、tool catalog、wrapped tool 调用、programmatic `{text, content}` envelope。
- `extensions/pi-grok-bash/eval-tasks.ts`
  - `EvalBackgroundTask`
  - `startEvalBackgroundTask()`
  - `promoteEvalTask()`
  - `armEvalAutoBackground()`
  - `waitForEvalTask()`
  - `killEvalTask()`
- `extensions/pi-grok-bash/prompts.ts`
  - `buildEvalPrompts()` / `buildBashPrompts()` 相关模型 contract。
- `extensions/pi-grok-bash/test-v2.1.mjs`
  - 当前 focused production regression。

### 现有关键 contract

当前 v2.1 已经值得保留的能力：

- `PersistentEvalKernel.execute(...)` 有 absolute wall timeout，等待 nested host call 也计入 wall clock。
- 每次 run 有统一 abort 传播，超时/outer abort/reset 会终止 nested call。
- `HostCallGate`：
  - parallel tool 有固定并发上限；
  - sequential tool 是 FIFO barrier；
  - sequential 排队后，后续 parallel 不越过；
  - running call 不会因为 caller 先 abort 就提前释放真实 slot。
- nested tool 的 programmatic 返回固定 `{text, content}`，不再猜 JSON。
- `EvalSessionToolBridge.catalog()` 暴露 active tool metadata。
- Eval v2 only 模式下，顶层能力可收敛到 `eval`，nested registry 仍保留。
- `completion()` 是无 history、无 tools 的 one-shot model leaf。
- `agent()` 是 blocking subagent leaf，不是第二套 scheduler。
- JS/Python 可按 `PI_GROK_EVAL_V2_LANGUAGE=js|py|all` 选择。
- background Eval 已经有统一 task UX，因此不应为了“看起来像 Codex”立即换成 live-cell actor。

### 当前验证基线

已在前序研究中实际运行：

```bash
node extensions/pi-grok-bash/test-v2.1.mjs
```

结果：

```text
Eval v2.1 focused production regression: PASS
```

覆盖包括：

- JS/Python all-mode；
- isolated lexical bindings + explicit `store/load`；
- absolute wall timeout；
- outer abort / orphan nested call cancellation；
- HostCallGate parallel cap / sequential FIFO barrier / slot accounting；
- foreground/background Eval；
- `completion()` / `agent()`；
- eval-v2-only；
- tools/skills discovery；
- `display(image)`。

## 正确比较对象：不是 `codex exec` CLI，而是 Code Mode

`codex exec` 是完整 headless agent runner，负责 config/auth、thread/turn、resume/fork/review、sandbox/approval、worktree、JSONL events 和最终 response schema。

对应入口：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/exec/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/exec/src/cli.rs`

它和 Eval v2 不是同层。

真正需要对比的是 Codex Code Mode：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-host/`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/`

## Codex 关键源码地图

### 1. Session/runtime API

文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/service.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/runtime.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/session.rs`

关键代码：

- `InProcessCodeModeSession`
- `CodeModeSession`
- `ExecuteRequest`
- `WaitRequest`
- `WaitOutcome`
- `ExecuteToPendingOutcome`
- `WaitToPendingOutcome`
- `RuntimeResponse`
- `StartedCell`
- `CodeModeSessionCellExecutionLimits`

关键方法：

```rust
InProcessCodeModeSession::execute(...)
InProcessCodeModeSession::execute_to_pending(...)
InProcessCodeModeSession::wait(...)
InProcessCodeModeSession::wait_to_pending(...)
InProcessCodeModeSession::terminate(...)
InProcessCodeModeSession::shutdown(...)
```

这组 API 最重要的地方不是 Rust，而是 contract：**session 是 owner，cell 是独立生命周期实体**。

### 2. 模型侧 `exec` / `wait`

文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/execute_handler.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/execute_spec.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/wait_handler.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/wait_spec.rs`

关键类型：

- `CodeModeExecuteHandler`
- `CodeModeWaitHandler`
- `ExecWaitArgs`

`CodeModeExecuteHandler` 创建 cell；`CodeModeWaitHandler` 对已有 `cell_id` 做 wait 或 terminate。

这比“长任务自动转 background job”更强：原 cell 本身仍然活着，程序栈/状态不需要迁移到另一套 kernel。

### 3. Provider boundary

文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/remote_session.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/grpc_session/mod.rs`

公开 provider：

- `ProcessOwnedCodeModeSessionProvider`
- `GrpcCodeModeSessionProvider`
- `DisabledCodeModeSessionProvider`

这里的设计价值是：Core 依赖 session contract，而不是 V8 的具体位置。

### 4. Host service / 背压

文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-host/src/lib.rs`

关键常量和类型：

```rust
MAX_IN_FLIGHT_REQUESTS = 256
MAX_ACTIVE_CELLS = 128
MAX_RECENT_REQUEST_IDS = 4096
MAX_RECENT_SESSION_IDS = 4096
```

- `HostLimits`
- `HostState`
- `RequestRegistry`
- `ActiveRequest`
- `RequestKind`

关键机制：

- `Semaphore` 限制 host-wide request / active cell；
- request registry 持有 `CancellationToken`；
- disconnect 时统一 cancel / shutdown；
- execute / wait / terminate / shutdown 都是显式 request kind。

这解决的是 **整个 runtime service 在压力下怎么不炸**。它和我们 `HostCallGate` 解决的“一个 Eval program 内如何尊重 Pi tool executionMode”不是同一个问题，两者应该叠加。

### 5. Cancellation

关键文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/service.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-host/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/cell_actor/`

关键类型：

- `CancellationToken`
- session/runtime request cancellation；
- per-cell actor cancellation；
- nested tool callback cancellation。

核心 invariant：取消不是“传一个 signal 参数”这么简单，而是 parent → cell → callback 的 ownership tree。

### 6. Typed output / image/audio

关键文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/runtime/value.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/response_adapter.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/runtime.rs`

Codex cell 输出不是单个自由文本，而是 typed response item 流，包含 text/image/audio 等类型，并由 exec/wait output token budget 约束。

### 7. Nested tool catalog / prompt API

关键文件：

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/description.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/runtime/globals.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/spec_plan.rs`

Codex 根据真实 tool spec 生成 code-mode nested tool declarations，并在 CodeModeOnly 下收敛顶层工具。

我们已经有：

- `EvalSessionToolBridge.catalog()`；
- `tools.list()`；
- `tools.search()`；
- `tools.describe()`；
- eval-v2-only。

因此这里不需要重写，只需要给常用工具补 compact typed signature，并保留 long-tail deferred discovery。

## 核心差异

| 维度 | Eval v2.1 | Codex Code Mode | 判断 |
|---|---|---|---|
| 定位 | Pi 内嵌 programmable orchestration bridge | 一等 code execution runtime/service | Codex 更完整，但代价高 |
| 语言 | JS + Python | JS/V8 | 保留我们优势 |
| Interpreter | persistent Node/Python worker | per-cell V8 isolate/context | 不直接复制 |
| Durable state | worker 内 `store/load` | session-owned state | **应该学** |
| Tool 调度 | executionMode-aware FIFO `HostCallGate` | host/delegate/runtime limits | 两套能力应该叠加 |
| 长任务 | foreground + auto background task | live cell + wait/terminate | workload 证明后再学 |
| Cancel | run AbortController | tree-shaped CancellationToken | live cell 前不急 |
| Service bounds | 主要单 run/call gate | host-wide semaphores/queues | **应该学** |
| Provider | extension 直接起 worker | process/gRPC/disabled provider | **应该学边界** |
| Tool catalog | dynamic list/search/describe | generated typed declarations | **应该增量学** |
| Output | `{text, content}` envelope | typed stream | 长期学，不硬塞进 v2.2 |
| Remote runtime | 无 | 有 | P2 |

## 我们需要学习什么

## P0：Session-owned durable state

### 现状问题

当前 `store/load` 的 durable state 实际寄居在 `PersistentEvalKernel` 对应的 worker 里。

这会把两个生命周期绑死：

```text
Eval session state lifetime == interpreter process lifetime
```

worker reset/crash/切换实现时，状态恢复能力天然受限。

### Codex 参考

- `InProcessCodeModeSession`：session 是 runtime owner。
- cell 只是一轮 disposable execution。
- provider 可以变化，session contract 不变。

### 目标设计

在 `extensions/pi-grok-bash/` 中新增最小 state owner：

```ts
export interface EvalSessionStore {
  set(key: string, value: JsonValue): void;
  get(key: string): JsonValue | undefined;
  has(key: string): boolean;
  delete(key: string): boolean;
  clear(): void;
  snapshot(): Readonly<Record<string, JsonValue>>;
}
```

建议落点：

- 新文件：`extensions/pi-grok-bash/eval-session-store.ts`
- `index.ts`：每个 Eval extension/session 创建一个 store owner。
- `PersistentEvalKernel`：只接收 store bridge，不再把 store truth 放在 worker global。

### Wire 调整

当前 worker 内部 `store/load` 是本地函数。目标改为：

```text
worker store(key, value)
  -> host_call(method="state_set")
  -> EvalSessionStore.set(...)

worker load(key)
  -> host_call(method="state_get")
  -> EvalSessionStore.get(...)
```

为避免每次状态访问都走 RPC，也可以第一版做 snapshot-in / delta-out：

1. execute 前 host 把 immutable state snapshot 下发；
2. cell 内 `store/load` 操作 local shadow；
3. cell settle 时返回 state mutation delta；
4. host 原子 commit delta。

**推荐第一版使用 host RPC 直写，不先造事务系统。** store/load 频率通常远低于 tool call；先拿到 ownership correctness，再优化性能。

### 必须保持的行为

- value 仍只允许 JSON-safe clone；
- JS/Python 语义一致；
- lexical binding 仍每 cell 隔离；
- kernel reset 不再清空 session store，除非用户明确请求 session reset；
- extension shutdown 清理 store。

### 验收

新增 focused test：

1. cell A `store("x", {a:1})`；
2. 强制 reset/recreate kernel；
3. cell B `load("x")` 仍得到 `{a:1}`；
4. 新 extension/session 不继承旧 store；
5. invalid non-JSON value fail fast。

## P0：Runtime provider boundary

### 现状问题

`index.ts` 直接 `new PersistentEvalKernel(...)`，调用方和 child-process implementation 强耦合。

当前代码点：

```ts
new PersistentEvalKernel("js", evalVersion, invokeEvalHostCall)
new PersistentEvalKernel("py", evalVersion, invokeEvalHostCall)
```

这使得以后想换：

- fresh worker；
- per-cell worker；
- isolate；
- remote host；
- crash-recoverable process；

都必须改上层 extension wiring。

### Codex 参考

- `ProcessOwnedCodeModeSessionProvider`
- `GrpcCodeModeSessionProvider`
- `DisabledCodeModeSessionProvider`

### 目标设计

不要照抄 Codex provider hierarchy。第一版只需要一个最小接口：

```ts
export type EvalExecuteRequest = {
  language: EvalLanguage;
  code: string;
  timeoutMs?: number;
  signal?: AbortSignal;
  tools: readonly EvalToolMetadata[];
  skills: readonly EvalSkillMetadata[];
};

export interface EvalRuntimeSession {
  execute(request: EvalExecuteRequest): Promise<EvalExecution>;
  reset(reason?: Error): void;
  shutdown(): void;
}

export interface EvalRuntimeProvider {
  open(language: EvalLanguage): EvalRuntimeSession;
  shutdown(): void;
}
```

建议新文件：

- `extensions/pi-grok-bash/eval-runtime-provider.ts`

第一版实现：

```ts
class ChildProcessEvalRuntimeProvider implements EvalRuntimeProvider
```

内部仍旧复用 `PersistentEvalKernel`，不改变 protocol。

### 改动点

`index.ts`：

```text
before:
  evalKernels: Partial<Record<language, PersistentEvalKernel>>

after:
  evalRuntimeProvider
  evalSessions: Partial<Record<language, EvalRuntimeSession>>
```

`eval-tasks.ts` 的 `kernel: PersistentEvalKernel` 改为窄接口：

```ts
runtime: Pick<EvalRuntimeSession, "execute" | "reset" | "shutdown">
```

不要让 background task 再依赖具体 kernel class。

### 验收

- `test-v2.1.mjs` 全通过；
- no behavior change；
- 新 provider 单测证明 open/reset/shutdown 转发；
- `index.ts` 不再直接依赖 child process lifecycle 细节。

## P0：Service-level bounds

### 现状问题

`HostCallGate` 只约束单个 Eval execution 的 nested host call 排队语义。

它不回答：

- 同一 session 同时多少 foreground/background Eval；
- host 总共多少 pending nested call；
- output/message queue 最大多大；
- background tasks 是否可以无限堆积。

### Codex 参考

`code-mode-host/src/lib.rs`：

```rust
MAX_IN_FLIGHT_REQUESTS = 256
MAX_ACTIVE_CELLS = 128
```

`HostLimits` 用两个 semaphore 明确做 service overload protection。

### 我们的最小实现

先不做通用 scheduler。增加 extension-local `EvalRuntimeLimits`：

```ts
export type EvalRuntimeLimits = {
  maxActiveExecutions: number;
  maxPendingHostCalls: number;
};
```

建议默认值不要照抄 Codex，先按现有 Pi workload 保守设置，例如：

```text
maxActiveExecutions = 8
maxPendingHostCalls = 32
```

但默认值必须通过 stress test 定，不在本 Issue 硬编码最终数字。

实现位置：

- `eval-runtime-provider.ts`：active execution permit；
- `HostCallGate` 外层或独立 `HostCallBudget`：pending host call permit。

### 不要做的事

- 不把 `HostCallGate` 替换成 generic semaphore；
- 不丢 sequential FIFO barrier；
- 不让 service bounds 改变 per-tool executionMode 语义。

### 验收

- 8/16/32 并发 stress；
- 超限必须明确 reject 或排队，不能 silent hang；
- abort 后 permit 必须最终归还；
- sequential barrier 行为保持原样。

## P0/P1：Typed + deferred nested tool catalog

### 现状

`EvalSessionToolBridge.catalog()` 已经提供 tool metadata；program 侧有：

- `tools.list()`
- `tools.search(query)`
- `tools.describe(name)`

这已经比把全部 schema 塞进 prompt 更节省 context。

### Codex 参考

Codex 会从真实 tool spec 生成 TS-like declaration，让模型直接写：

```ts
await tools.some_tool({ ... })
```

并在 CodeModeOnly 下把顶层普通工具隐藏。

### 目标

保留动态 discovery，同时为“高频小 schema”提供 compact signature：

```ts
type EvalToolMetadata = {
  name: string;
  description: string;
  executionMode: BridgeExecutionMode;
  signature?: string;
  deferred?: boolean;
};
```

例如：

```text
read(path: string, offset?: number, limit?: number): ToolResult
bash(command: string, timeout?: number): ToolResult
```

### 生成策略

不要自己手写 TS parser。直接从 Pi tool JSON schema 生成最小可读 signature：

- object properties；
- required；
- primitive/array/object；
- enum；
- 复杂 union/schema 超过阈值则 `deferred=true`，要求 `tools.describe()`。

### Prompt 策略

`buildEvalPrompts()`：

- inline 最常用/短 signature；
- 长尾只提示 `tools.search()` / `tools.describe()`；
- 不生成完整 SDK 文件；
- 不复制 Codex 全量 declaration machinery。

### 验收

- 常用工具 prompt token size 不明显增加；
- tool 参数错误率在真实 eval workload 上下降；
- 复杂 schema 仍可通过 `describe()` 获取；
- tool registry 变化后 signature 自动同步。

## P1：Live cell lifecycle

### 什么时候才值得做

只有出现下面真实需求时才进入实现：

- 一个 cell 内有长时间等待，但我们不希望迁移成独立 background kernel；
- 需要在下一模型 turn 继续观察同一个 JS/Python stack；
- 需要 program 主动 yield partial observation；
- background task 当前的 state migration 语义无法表达 workload。

如果没有这些 workload，继续用现有 background Eval 更便宜。

### Codex 参考

- `execute()` 返回 `StartedCell`；
- `wait(WaitRequest)`；
- `terminate(cell_id)`；
- `RuntimeResponse`；
- `WaitOutcome::LiveCell` / `MissingCell`；
- worker globals 中的 `yield_control()` / `notify()`。

### 目标 API

不要一上来暴露 Codex 同名 API。grok-pi 可以先内部建模：

```ts
type EvalCellId = string;

type EvalCellStatus =
  | "running"
  | "completed"
  | "failed"
  | "aborted";

type EvalCellSnapshot = {
  id: EvalCellId;
  status: EvalCellStatus;
  output: string;
  images: EvalDisplayImage[];
};

interface EvalLiveRuntimeSession extends EvalRuntimeSession {
  start(request: EvalExecuteRequest): Promise<EvalCellSnapshot>;
  wait(cellId: EvalCellId, options?: { timeoutMs?: number }): Promise<EvalCellSnapshot>;
  terminate(cellId: EvalCellId): Promise<EvalCellSnapshot>;
}
```

模型侧是否新增独立 `eval_wait`，还是复用现有 task API，应在 workload 试验后决定。

### 必须同步上的能力

一旦 live cell 存在，以下能力必须一起实现，不能半拉子上线：

- cell registry；
- single terminal linearization point；
- cancellation tree；
- max active cells；
- disconnect/shutdown cleanup；
- output incremental cursor；
- missing/closed cell 明确错误；
- race tests：complete vs terminate / wait vs close / abort vs callback。

这就是为什么它是 P1，而不是“顺手加个 wait”。

## P1：Cancellation tree + terminal state machine

### 当前够用的部分

v2.1 run-scoped AbortController 对单个 foreground/background execution 已经简单可靠。

### Live cell 之后需要升级

目标 ownership：

```text
EvalRuntimeSession
  └─ EvalCell
      ├─ nested tool call
      ├─ completion call
      └─ agent call
```

每层都有 parent-owned AbortController/token。

推荐类型：

```ts
type EvalCellTerminal =
  | { kind: "completed"; result: EvalExecution }
  | { kind: "failed"; error: Error }
  | { kind: "aborted"; reason?: unknown }
  | { kind: "timed_out"; timeoutMs: number };
```

Cell terminal result 必须只 settle 一次。

### 测试必须覆盖

- parent abort cancels queued + running nested calls；
- child failure 不反向取消 sibling，除非调用语义要求；
- terminate/complete race 只产生一个 terminal state；
- abort 时 HostCallGate slot 不泄漏；
- runtime shutdown 后所有 cell 都终态化。

## P1/P2：Canonical programmatic value

### 当前不要误做

Pi `AgentToolResult` 的 `content` 是模型展示内容，`details` 是 UI/log arbitrary data。它们都不是稳定 typed business value。

因此不能这样做：

```ts
return result.details; // 错：把 UI/log details 冒充 programmatic contract
```

### 长期目标

Pi Core 需要先有真正的 canonical tool output contract，例如：

```ts
type AgentToolResult<TValue = unknown> = {
  content: ToolContent[];
  value?: TValue;
  outputSchema?: JsonSchema;
  details?: unknown;
  usage?: Usage;
};
```

之后 Eval 才能返回：

```ts
{
  text,
  content,
  value
}
```

### 为什么不现在做

这不是 pi-grok-bash 一处 extension 的局部改动，而是 Pi Core / tool ecosystem contract migration。没有 core-wide producer/consumer 约束时，extension 自己定义 `value` 只会再造一个假标准。

## P2：Remote runtime / gRPC provider

### Codex 参考

- `GrpcCodeModeSessionProvider`
- process-owned provider
- code-mode-host

### 只有这些场景才做

- runtime 需要独立 sandbox / 权限边界；
- 多个 Pi process 共享 runtime host；
- 需要独立升级/重启 code runtime；
- 本地主进程资源隔离不足；
- crash blast radius 需要进一步缩小。

### 我们的前置条件

先完成 `EvalRuntimeProvider`。做到这一点后，remote provider 应该只是新增实现，而不是改上层调用方。

协议建议继续用现有 typed JSON message 模型，不要提前引 gRPC 依赖。只有真的需要跨机器/强 schema/流式双向通道时再选 transport。

## 明确不学 / 不照抄

### 不删除 Python

Codex 是 V8-only 不代表我们也应该 V8-only。当前 JS/Python 双语言是实际产品能力。

### 不用 generic scheduler 替换 HostCallGate

`HostCallGate` 理解 Pi `executionMode`，这是和 Pi tool lifecycle 绑定的本地优势。

### 不为了 fresh isolate 重写 3 万行 runtime

fresh isolate 的收益主要是安全隔离、crash containment、生命周期清晰。先通过 provider/state separation 拿到 80% 的架构收益，再决定是否值得换 interpreter。

### 不提前复制 pending frontier / observer actor

这些复杂度只为 live-cell race 服务。没有 live cell 就没有必要。

### 不把 protocol field 当成熟实现

Codex protocol 中存在 `max_heap_size_bytes`，但研究时确认当前 in-process runtime 构造会把它清成 `None`，没有证据证明 V8 heap enforcement 已完整生效。

因此“Codex 有 heap cap”不能作为我们实现 heap cap 的依据。

### 不把 `codex exec` CLI 能力塞进 Eval

thread/resume/fork/review/worktree/sandbox/approval 都属于 agent runner，不属于 Eval runtime。

## 分阶段实施计划

## Phase 1 — Runtime ownership cleanup（P0）

范围：

1. 新增 `EvalSessionStore`；
2. 新增 `EvalRuntimeProvider` / `EvalRuntimeSession`；
3. `ChildProcessEvalRuntimeProvider` 包住现有 `PersistentEvalKernel`；
4. `eval-tasks.ts` 改依赖 runtime interface；
5. store 从 worker lifetime 中解耦；
6. 不改变公开 tool schema 和模型 prompt。

预计文件：

- 新增 `extensions/pi-grok-bash/eval-session-store.ts`
- 新增 `extensions/pi-grok-bash/eval-runtime-provider.ts`
- 修改 `extensions/pi-grok-bash/eval.ts`
- 修改 `extensions/pi-grok-bash/index.ts`
- 修改 `extensions/pi-grok-bash/eval-tasks.ts`
- 修改 `extensions/pi-grok-bash/test-v2.1.mjs` 或拆一个 focused v2.2 test

验收：

```text
kernel reset 后 store 仍在
session shutdown 后 store 消失
JS/Python 语义一致
existing v2.1 regression 100% PASS
```

## Phase 2 — Service bounds + tool signatures（P0/P1）

范围：

1. active execution permit；
2. global pending host call bound；
3. compact signature generator；
4. deferred long-tail tool metadata；
5. prompt 只 inline 高频小 signature。

预计文件：

- `extensions/pi-grok-bash/eval-runtime-provider.ts`
- `extensions/pi-grok-bash/eval.ts`
- `extensions/pi-grok-bash/tool-bridge.ts`
- `extensions/pi-grok-bash/prompts.ts`
- focused tests

验收：

```text
stress 下无 permit leak
超限行为可预测
sequential FIFO 不回退
registry 变更自动反映 signature
prompt token size 有上限
```

## Phase 3 — Live cell experiment（条件触发 P1）

前置条件：必须先提供真实 workload，证明 background Eval 不够。

范围：

1. cell registry；
2. start/wait/terminate 内部接口；
3. incremental output cursor；
4. cancellation tree；
5. terminal state machine；
6. race tests。

不要在这个阶段同时做 remote transport。

## Phase 4 — Canonical value（跨 Pi Core，P1/P2）

前置条件：Pi Core 接受稳定 `value` / output schema contract。

范围：

1. Core tool result contract；
2. built-in / extension tool migration策略；
3. Eval programmatic return plane；
4. transcript/UI projection 与 value 分离；
5. structured completion/agent result 可复用同一 schema contract。

这是独立 architecture/API change，应单开 Issue/ADR。

## Phase 5 — Remote provider（条件触发 P2）

前置条件：有 sandbox/shared runtime/独立升级的明确需求。

先实现 provider contract，再选择 transport；不要反过来从 gRPC 开始设计。

## 具体代码改动建议

### `extensions/pi-grok-bash/index.ts`

现有职责太多，但本轮不要大拆文件。只做接线级变化：

- 创建 `EvalSessionStore`；
- 创建 `EvalRuntimeProvider`；
- 用 runtime session 替代直接 `PersistentEvalKernel` construction；
- 保留 `EvalSessionToolBridge`；
- 保留 `HostCallGate`；
- service-level bounds 放 provider 层，不塞进 tool-bridge。

不要顺手重构 Bash task 部分。

### `extensions/pi-grok-bash/eval.ts`

目标是逐步把它从“state owner + process owner + protocol + scheduler + helpers”降成：

```text
worker protocol + child process implementation
```

应该逐步移出的职责：

- durable session store → `eval-session-store.ts`
- runtime lifecycle abstraction → `eval-runtime-provider.ts`

应该保留：

- worker source；
- wire encoding/decoding；
- `PersistentEvalKernel` 作为 child-process backend implementation；
- `HostCallGate` 可先继续留在这里，直到有明确理由独立。

### `extensions/pi-grok-bash/tool-bridge.ts`

保留 `EvalSessionToolBridge` 作为 Pi registry / wrapped tool seam。

新增的最小能力：

- catalog signature；
- deferred flag；
- schema-to-signature helper。

不要让它管理 runtime session/cell。

### `extensions/pi-grok-bash/eval-tasks.ts`

把 concrete `PersistentEvalKernel` 依赖改成窄 runtime interface。

这样 background task UX 可以继续存在，即使未来 backend 换成 isolate 或 remote provider。

### `extensions/pi-grok-bash/prompts.ts`

只描述用户实际可用 contract。

如果 Phase 1 只是内部 provider/store 重构，prompt **不应该变化**。

只有 Phase 2 typed signature 真正上线时才修改 prompt。

## 设计约束

### 1. Fail closed

未知 tool executionMode 继续按 sequential；未知 provider 状态直接报错；不存在 silent fallback。

### 2. 不保持错误的 backward compatibility

如果后续 runtime contract 证明旧内部结构错误，可直接迁移内部 API；但公开 `eval` tool 行为除非另有 Issue，不应无关变化。

### 3. 只有一个 state truth

Phase 1 完成后：

```text
EvalSessionStore = durable state SSOT
worker local state = execution cache / transport detail
```

不能 host/worker 双写、靠“最后一次赢”。

### 4. Cancellation ownership 必须清楚

谁创建 controller/token，谁负责 abort；child 不得在没有 ownership 的情况下清理 parent signal。

### 5. 不让 presentation 反向定义 data contract

`content.text`、UI `details`、日志都不能成为 future canonical value 的来源。

## 验证矩阵

每个 Phase 至少执行：

```bash
node extensions/pi-grok-bash/test-v2.1.mjs
git diff --check
```

Phase 1 额外：

```text
store survives kernel reset
store isolated by session
provider open/reset/shutdown
background task backend interface
```

Phase 2 额外：

```text
parallel saturation
sequential barrier under saturation
abort queued call
abort running call
permit release
signature snapshot
long schema deferred
```

Phase 3 额外：

```text
complete vs terminate race
wait vs close race
abort vs callback race
missing cell
shutdown with live cells
incremental output cursor
```

Phase 4 额外：

```text
value/content/details separation
schema validation
extension tool compatibility
UI projection unchanged
```

## 性能验证

不要先声称更快。至少记录：

- Eval cold/warm execute latency；
- nested tool p50/p95；
- max parallel host call throughput；
- memory per active background/live execution；
- reset/recovery latency；
- prompt token cost before/after typed signatures。

只有这些数据出现回归/收益后才做性能结论。

## 风险与回滚

### Phase 1 最大风险：状态语义改变

如果 host-owned store 引入 regression，回滚点应该只恢复 store ownership；provider interface 可以保留，因为它只是抽象现有 kernel。

### Phase 2 最大风险：过度限流

service-level bound 太小会让合法 parallel workload 变慢。默认值必须通过 stress 数据调，不凭直觉。

### Phase 3 最大风险：生命周期复杂度爆炸

live cell 一旦上线，就会引入 wait/terminate/race/disconnect 状态空间。没有真实 workload 不做。

### Phase 4 最大风险：跨 tool ecosystem API 迁移

canonical value 是 core contract，不能由 pi-grok-bash 单边推动。

## 接受标准

这个路线被认为成功，不是因为代码长得像 Codex，而是因为：

1. kernel/backend 可以替换而不改上层 Eval tool wiring；
2. session durable state 不因 interpreter reset 丢失；
3. host-wide overload 有硬边界；
4. 现有 HostCallGate 语义完整保留；
5. nested tool 编程准确率提高但 prompt 不膨胀；
6. background task 在没有 live-cell workload 前继续作为默认长任务机制；
7. live cell 若实现，具有可证明的单终态/cancellation/race safety；
8. remote runtime 若实现，只新增 provider，不改上层 API；
9. 每阶段都有 focused regression + `git diff --check`；
10. 不引入与目标无关的 runner/thread/worktree/sandbox 逻辑。

## 实施顺序

最务实的顺序：

```text
Phase 1: session store + provider boundary
  ↓
Phase 2: service bounds + compact typed catalog
  ↓
真实 workload 评估
  ├─ background task 足够 → 停
  └─ background task 不够 → Phase 3 live cell
                                 ↓
                         cancellation/state machine

Core 有 canonical value contract → Phase 4
有 remote/sandbox 部署需求     → Phase 5
```

## 参考源码

### grok-pi

- `extensions/pi-grok-bash/index.ts`
- `extensions/pi-grok-bash/eval.ts`
- `extensions/pi-grok-bash/eval-tasks.ts`
- `extensions/pi-grok-bash/tool-bridge.ts`
- `extensions/pi-grok-bash/prompts.ts`
- `extensions/pi-grok-bash/test-v2.1.mjs`
- `docs/issues/adapter/20260818-Eval Bridge v2 与 RLM runtime.md`

### Codex

- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/service.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/session_runtime/mod.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/cell_actor/`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/runtime/value.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/runtime/globals.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-runtime/src/runtime/callbacks.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/session.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/runtime.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-protocol/src/description.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-host/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode-host/src/peer.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/remote_session.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/code-mode/src/grpc_session/mod.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/execute_handler.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/execute_spec.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/wait_handler.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/wait_spec.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/response_adapter.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/code_mode/delegate.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/core/src/tools/spec_plan.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/exec/src/lib.rs`
- `/Users/dengwenyu/Dev/AI/codex/codex-rs/exec/src/cli.rs`

## 最终决策

v2.2 第一刀只做两件核心事：

1. **把 durable state 从 worker lifetime 中拿出来；**
2. **把 `PersistentEvalKernel` 藏到 `EvalRuntimeProvider` 后面。**

然后补 service-level bounds 和 typed/deferred tool catalog。

不要先做 live cell，不要先做 gRPC，不要先改 Pi Core typed value，不要重写 interpreter。

这是最小、可验证、能长期降低耦合的路线。Codex 值得学的是边界和 invariant，不是它的代码体积。
