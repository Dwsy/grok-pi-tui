# pi-grok-bash

`pi-grok-bash` is the Grok-specific Pi extension that owns enhanced Bash execution and the Eval runtime used by this build. Despite the historical directory name, it now contains two related execution subsystems:

- managed Bash with foreground/background task integration;
- Eval v1/v2, including the v2 host-tool bridge, explicit cross-cell state, concurrency control, and background Eval tasks.

The extension is intentionally implemented at the Pi extension layer so Pager/native task UI stays authoritative while execution, cancellation, task lifetime, and model-facing tool semantics remain under one owner.

## Current behavior at a glance

| Capability | Eval v1 | Eval v2 |
| --- | --- | --- |
| JavaScript | yes | yes |
| Python | yes | no |
| Top-level `await` | yes | yes |
| Bare-expression result | yes | yes |
| Lexical bindings survive cells | yes | **no** |
| Explicit `store()` / `load()` persistence | no | **yes** |
| Call active Pi tools from a cell | no | **yes** |
| Tool/skill discovery | no | **yes** |
| `parallel()` / `pipeline()` | no | **yes** |
| `agent()` helper | no | **yes** |
| one-shot `completion()` | no | when host exposes `pi.complete` |
| Background Eval jobs | no | **yes** |
| Shared task APIs | Bash only when available | Bash and Eval v2 |

Eval v1 is deliberately compatibility-oriented. Eval v2 is the production bridge with stricter semantics and explicit state boundaries.

## Files

- `index.ts` — extension registration, tool schemas, v1/v2 feature gates, host-call dispatch, Bash/Eval task registry, session shutdown. Prompt prose is intentionally not stored inline here.
- `prompts.ts` — centralized Eval/Bash prompt bundles (`description`, `promptSnippet`, `promptGuidelines`, and Eval code-parameter description), with explicit v1/v2 builders rather than nested registration-time ternaries.
- `eval.ts` — Eval workers, `PersistentEvalKernel`, v2 worker protocol, per-cell scope reset, `store/load`, output capture, timeout/abort handling, `HostCallGate`.
- `tool-bridge.ts` — observes Pi's registered/active tools, captures metadata and wrapped extension tools, and invokes active tools from Eval v2.
- `eval-tasks.ts` — isolated background Eval v2 tasks and task snapshots/output files.
- `bash-tasks.ts` — managed Bash process lifecycle, foreground-to-background promotion, output truncation, Pager status channel, Bash task results.
- `shared.ts` — output and timeout limits plus process-tree termination.
- `test-v2.1.mjs` — focused production regression suite for v1/v2 boundaries, host bridge behavior, cancellation, concurrency, task management, and fallback invocation paths.

## Prompt orchestration

Model-facing prose is centralized in `prompts.ts` instead of being interleaved with execution code in `index.ts`.

`buildEvalPrompts(evalVersion, completionAvailable)` returns one Eval prompt bundle containing the tool description, code-parameter description, snippet, and guidelines. v1 is a stable compatibility bundle; v2 is assembled explicitly and conditionally appends only the completion-related text when the host actually exposes `pi.complete`.

`buildBashPrompts(evalVersion, nativeDescription, nativeGuidelines)` composes Pi's native Bash prompt metadata with this extension's Eval/Bash routing guidance and `task_name` requirement. The v1/v2 language is selected inside the builder with ordinary control flow, so `registerTool()` does not contain long conditional strings.

This separation is deliberate: prompt semantics are part of the version contract and should be reviewable independently from tool execution. When v2 behavior changes, update the v2 bundle and its regression expectations rather than scattering new ternaries through `index.ts`.

## Architecture

```text
Pi / ExtensionRunner
        |
        | registers tools / exposes active tool catalog
        v
+---------------------------+
| extensions/pi-grok-bash   |
|         index.ts          |
+---------------------------+
   |                    |
   | Bash               | Eval
   v                    v
bash-tasks.ts       PersistentEvalKernel
   |                    |
   | child process       | child worker process
   |                    |
   |                    +-----------------------------+
   |                    | Eval v1                     |
   |                    | JS REPL / Python namespace  |
   |                    +-----------------------------+
   |                    |
   |                    +-----------------------------+
   |                    | Eval v2 JS worker           |
   |                    | fresh REPL context per cell |
   |                    | explicit store/load Map     |
   |                    +-----------------------------+
   |                                  |
   |                                  | protocol fd 3
   |                                  v
   |                         HostCallGate (cap = 4)
   |                                  |
   |                                  v
   |                         EvalSessionToolBridge
   |                                  |
   +----------------------+-----------+
                          |
                          v
                 active Pi tools / skills /
                 completion / spawn_subagent

Background work:

Bash task --------------------+
                              +--> get_task_output / wait_tasks / kill_task
Eval v2 isolated task --------+
```

The worker protocol is separated from stdout/stderr. Cell output is captured from stdout/stderr, while structured Eval/host-call messages travel over file descriptor 3. This avoids confusing ordinary program output with control messages.

## Why Eval v2 uses isolated cell scope

### The problem with a persistent REPL lexical scope

The first v2 implementation reused one Node REPL context for every cell. That made ordinary top-level declarations accumulate forever:

```js
const result = 1;
```

followed by another cell:

```js
const result = 2;
```

could fail with a redeclaration error. In an agent loop this is especially brittle because generated variable names such as `result`, `data`, `r`, `items`, and `response` are naturally reused across turns.

A long-lived process is useful for low startup cost and explicit state, but a long-lived lexical environment is not a good conversational cell boundary.

### Codex research

The implementation was compared against Codex code-mode under `codex-rs/code-mode-runtime`.

The relevant Codex design is:

1. each code-mode cell gets a fresh V8 isolate/context;
2. top-level lexical declarations therefore belong only to that cell;
3. cross-cell state is explicit through `store(key, value)` and `load(key)`;
4. stored values are serialized data rather than retained JS object/function bindings.

Codex service/runtime tests explicitly write state in one cell and load it in a later cell. Its runtime keeps `stored_values` separately from the JavaScript scope and records `stored_value_writes` as part of cell completion.

### Adaptation used here

Eval v2 keeps a persistent Node worker process because that preserves the existing low-overhead REPL protocol, top-level `await`, bare-expression return values, stdout/stderr capture, and host bridge. Before each cell, however, it calls Node REPL's `resetContext()` and reinstalls the v2 helper globals.

This produces the same important user-visible lexical boundary as Codex without replacing the established worker protocol:

- process: persistent until kernel reset;
- lexical scope: fresh for every cell;
- supported Eval cross-cell data API: explicit and serialized through `store/load`.

This is **lexical isolation, not a security sandbox or a fresh process/isolate**. The Node worker itself remains alive, so process-level or external side effects can still outlive a cell—for example mutations to `process.env`, Node module-cache/singleton state, filesystem writes, or state owned by an invoked host tool. Code should not use those effects as an implicit Eval memory mechanism; `store/load` is the intentional bridge-managed data channel.

For example, this is valid across consecutive cells:

```js
const result = 21;
result * 2
```

```js
const result = 40;
await Promise.resolve();
result + 2
```

Both cells can return `42`; the second declaration does not collide with the first.

## Eval v2 explicit state: `store()` and `load()`

Use `store(key, value)` only when later cells genuinely need data from the current cell:

```js
const rows = await tool.read({ path: "data.json" });
store("rows", { text: rows.text });
```

Later:

```js
const saved = load("rows");
saved.text.length
```

Rules:

- keys are converted to strings;
- values must be JSON-serializable;
- `load()` returns `undefined` for a missing key;
- values are cloned through JSON serialization, so later mutation does not mutate the stored copy by reference;
- functions, cyclic structures, and other non-serializable values are rejected;
- `reset: true`, timeout, abort, cwd change, worker/process failure, or session shutdown destroys the kernel and therefore clears stored state.

Do not use `store/load` as a replacement for normal local variables inside a single cell. The purpose is to make cross-cell dependencies intentional and inspectable.

## Eval v1 compatibility model

Eval v1 remains intentionally unchanged:

- JavaScript and Python are both exposed;
- each language has a persistent per-language worker;
- JS/Python state can survive calls until reset/restart;
- v1 does not expose `is_background` for Eval;
- v1 does not expose the v2 host-tool bridge or `store/load` semantics.

This separation prevents a v2 architecture change from silently changing established v1 behavior.

## Eval v2 host-tool bridge

Eval v2 can invoke active Pi tools directly from JavaScript:

```js
const result = await tool.grep({
  pattern: "TODO",
  path: "src",
});
result.text
```

Every host call must be awaited when its result matters. `tool.eval` is intentionally unavailable to prevent recursive Eval invocation.

### Tool discovery

The cell receives only the current active-tool surface:

```js
Object.keys(tool)
tools.list()
tools.search("git")
tools.describe("grep")
```

Per-tool metadata is also exposed:

```js
tool.grep.description
tool.grep.schema
tool.grep.meta
```

`EvalSessionToolBridge` observes Pi's registered tools and active-tool set. Normal v2 exposes and invokes only active tools. When grok-pi applies its host-owned eval-v2-only policy, the process skips the saved F2 built-in selection so the registry remains intact while the top-level active set is collapsed to `eval`; explicit CLI exclusions still apply, and the bridge exposes the remaining registered tools only inside Eval while keeping them hidden from the top-level model; those inactive nested calls use captured wrapped extension/core tools instead of native `ExtensionAPI.invokeTool`, so Pi tool lifecycle hooks still run. Explicit CLI tool policy does not enable this widening.

Unknown or missing `executionMode` fails closed to `sequential`; only an explicit `parallel` declaration is treated as parallel-safe.

## Skills inside Eval v2

At `before_agent_start`, the extension snapshots Pi-loaded skills that are model-invokable. Skills with `disableModelInvocation` are excluded.

Inside a cell:

```js
skills.list()
skills.search("database")
skills.describe("my-skill")
const body = await skills.read("my-skill")
```

`skills.read()` is admitted only for a skill in the current snapshot; arbitrary file paths cannot be supplied through the skill helper.

## `parallel()` and `pipeline()`

For independent asynchronous operations:

```js
const results = await parallel([
  () => tool.first({ id: 1 }),
  () => tool.second({ id: 2 }),
]);
```

For fan-out/fan-in stages:

```js
const results = await pipeline(
  items,
  item => tool.fetch({ id: item.id }),
  fetched => transform(fetched),
);
```

`parallel()` expresses logical independence, but the host bridge still enforces each tool's execution mode and the global parallel host-call cap.

## HostCallGate: concurrency and cancellation

Eval v2 routes host-side work through `HostCallGate` with a fixed parallel cap of 4.

The gate has two modes:

- `parallel` jobs may overlap up to the cap;
- `sequential` jobs are FIFO barriers: they wait for all currently running parallel work and block later work until completion.

A subtle cancellation rule is essential: aborting a call settles the Eval caller immediately, but **does not release the gate slot until the underlying host work actually settles**.

Why: a host implementation may react to `AbortSignal` asynchronously or may not stop immediately. Releasing capacity at the abort race would allow replacement work to start while cancelled work is still physically running, exceeding cap=4 or crossing a sequential barrier.

The regression suite includes a deliberately delayed-abort host tool to verify actual live host work never exceeds the cap after cancellation.

## Cell settlement and orphan host work

Each foreground cell owns a run-scoped `AbortController`. When a cell settles, its outstanding host calls are aborted. This prevents fire-and-forget host calls from leaking into the next cell.

Timeout or outer abort is stronger: `PersistentEvalKernel` destroys the worker process and rejects the active cell. The next call starts a clean worker.

Only one cell may execute in a given foreground kernel at a time.

## `agent()` and `completion()`

### `agent()`

`agent(prompt, options)` is a convenience wrapper over the active `spawn_subagent` tool.

```js
const result = await agent("Inspect this subsystem", {
  model: "...",
  background: false,
});
```

Supported forwarded options include `subagent_type`, `model`, `max_turns`, `capability_mode`, and `background`.

If `background: true` is used, the returned subagent handle is managed through the host's active task-output mechanism; it is not an Eval background task.

### `completion()`

When the Pi host exposes `pi.complete`, Eval v2 also exposes:

```js
const r = await completion("Classify this value", {});
r.text
```

This is a one-shot model call with no session history and no tools. It is intended for local classification/extraction/synthesis subproblems rather than an agent loop.

## Background Eval v2 tasks

Eval v2 adds `is_background: true` to the Eval schema. v1 does not expose this parameter.

Example tool payload conceptually:

```json
{
  "language": "js",
  "code": "await doLongWork()",
  "title": "Long Eval analysis",
  "is_background": true
}
```

A background Eval task:

- gets its own `PersistentEvalKernel` rather than sharing the foreground kernel;
- receives the current tool and skill snapshots;
- has an independent `AbortController`;
- gets an `eval-<uuid>` task ID;
- writes final rendered output to a temporary `output.log`;
- closes its isolated kernel when it settles;
- publishes start/completion state to the Pager-compatible task status channel when UI is attached.

Foreground and background Eval kernels are therefore independent: long background work cannot occupy the foreground cell slot or contaminate its lexical/state context.

## Managed Bash

When Bash is enabled, this extension replaces the stock execution path with a managed process owner while retaining Pi's Bash tool definition/UX.

Important behavior:

- every Bash child is owned by `bash-tasks.ts`;
- foreground Bash can be promoted into the existing background task UI without rerunning the command;
- a foreground Bash task still running at the configured max-wait threshold is automatically promoted through that same path;
- explicit background Bash returns a task ID immediately;
- output is retained in memory up to `MAX_OUTPUT_BYTES` and also written to a task output file;
- process groups are killed when possible so cancellation terminates the command tree rather than only the direct child;
- terminal task state is published through an out-of-band `ui.setStatus` channel so Pager can converge even when conversation follow-up delivery is aborted or detached.

`task_name` is required for Bash calls and is the short human-readable label displayed by the terminal UI.

## Unified task API

When Bash is enabled **or** Eval v2 is selected, the extension registers the shared task tools:

### `get_task_output`

Poll one or more task IDs, optionally waiting up to `timeout_ms`.

```json
{
  "task_ids": ["eval-..."],
  "timeout_ms": 1000
}
```

### `wait_tasks`

Wait for one or all tasks:

```json
{
  "task_ids": ["eval-...", "..."],
  "mode": "wait_all",
  "timeout_ms": 30000
}
```

Modes are `wait_any` and `wait_all`.

### `kill_task`

Terminate a running Bash or Eval v2 task by task ID.

Task lookup is unified, but task-specific cancellation remains owned by the appropriate subsystem: process-tree termination for Bash and `AbortController` + kernel reset for Eval.

The task APIs remain registered for Eval v2 even when enhanced Bash itself is disabled. When max-wait is enabled, each blocking wait call is capped independently; a still-running result is expected and the agent can re-call the wait tool in the next turn. No background heartbeat message is emitted.

## Configuration

### `PI_GROK_EVAL_VERSION`

Selects Eval implementation:

- `v1` — default; JavaScript + Python persistent-state compatibility runtime;
- `v2` — JavaScript-only isolated cells, explicit `store/load`, host-tool bridge, helpers, and background Eval tasks.

Any other value is rejected during extension initialization.

### `PI_GROK_BASH`

Enhanced Bash is enabled by default. Values `0`, `false`, `off`, or `no` disable it.

Disabling Bash does **not** disable Eval v2 or its background task management.

### `PI_GROK_BASH_MAX_WAIT_MINS`

Controls both the foreground Bash auto-background threshold and the maximum duration of one blocking `wait_tasks` / `get_task_output` call. Default: `4.5` minutes. `0` or a negative value disables both behaviors and preserves the old manual-promotion / uncapped-wait behavior.

When launched through `grok-pi`, `--bash-max-wait-mins <MIN>` has highest precedence, then an inherited `PI_GROK_BASH_MAX_WAIT_MINS`, then the `4.5` default. The Bash process `timeout` remains independent: whichever fires first, process timeout or auto-background, takes effect first.

The 4.5-minute default leaves roughly 30 seconds of headroom against a 5-minute prompt-cache TTL; lower it if model/tool-turn latency regularly consumes that margin.

### `PI_GROK_BUILTIN_TOOLS`

Optional comma-separated allow-list used by this extension when deciding whether the host Bash tool is enabled. If provided and `bash` is absent, enhanced Bash is not registered.

### `PI_GROK_EXCLUDE_TOOLS`

Optional comma-separated exclusion list. If it contains `bash`, enhanced Bash is not registered.

### `PI_GROK_PYTHON`

Eval v1 only. Overrides the Python executable used by the Python worker. Otherwise the runtime chooses `python3` on non-Windows platforms and `python` on Windows.

### `PI_PACKAGE_DIR`

Used by `EvalSessionToolBridge` as an optional host-package discovery root when it needs to load the running Pi package's runtime module for extension-tool capture/wrapping.

## Limits and lifecycle

- Eval default timeout: 30 seconds.
- Eval timeout `0`: disabled.
- Maximum accepted timeout: `2_147_483.647` seconds.
- In-memory captured output limit: 50 KiB; older bytes are discarded and the result is marked truncated.
- Foreground kernel is reset on explicit reset, timeout, abort, cwd change, process failure, or session shutdown.
- On `session_shutdown`:
  - tool-bridge listeners are disposed;
  - foreground Eval kernels are closed;
  - running Bash tasks are terminated and marked as session-restart/orphaned where appropriate;
  - running background Eval tasks are killed.

## Choosing Eval vs Bash

Use Eval for computation that is naturally JavaScript (or Python in v1): calculations, parsing, transformations, collection analysis, experiments, and logic that benefits from tool calls or structured async composition.

Use Bash for shell-native work: filesystem/process control, Git, builds, package managers, shell pipelines, command-line programs, and operations whose semantics are fundamentally shell/process oriented.

For Eval v2, do not carry local declarations across cells. Recompute local setup when cheap and use `store/load` only for the minimum serialized state that genuinely must survive.

## Eval v2 examples

### Computation with top-level await

```js
const values = await Promise.resolve([1, 2, 3]);
values.reduce((a, b) => a + b, 0)
```

### Reuse a common variable name safely in the next cell

```js
const values = [40, 2];
values[0] + values[1]
```

There is no redeclaration collision because the new cell has a fresh lexical scope.

### Explicit persistence

Cell 1:

```js
store("summary", {
  count: 3,
  ids: ["a", "b", "c"],
});
```

Cell 2:

```js
const summary = load("summary");
summary.ids.length
```

### Tool metadata discovery

```js
const candidates = tools.search("read");
const info = tools.describe(candidates[0].name);
({ name: info.name, schema: info.schema })
```

### Parallel independent calls

```js
const [a, b] = await parallel([
  () => tool.first({ query: "a" }),
  () => tool.second({ query: "b" }),
]);
({ a: a.text, b: b.text })
```

## Design decisions and non-goals

### Explicit state beats accidental state

Eval v2 intentionally does not preserve lexical bindings across cells. This is not a missing persistence feature; it is the isolation contract.

### Background Eval is isolated, not a second foreground slot

`is_background` creates a dedicated kernel and task lifecycle. It does not reuse the foreground kernel concurrently.

### Host concurrency declarations are trusted conservatively

Only tools explicitly marked `parallel` are parallelized. Missing/unknown metadata becomes `sequential`.

### Abort is not the same as physical completion

Caller cancellation can be immediate while underlying host work is still winding down. Resource accounting therefore follows actual settlement, not merely the abort race.

### Eval does not recursively call Eval

`tool.eval` is excluded to prevent nested kernels/cycles through the same bridge.

## Research notes: why not just wrap every cell in `{ ... }`?

A block wrapper was tested as a lightweight way to isolate declarations. It was rejected for two reasons:

1. Node REPL parsing can reinterpret a leading `{ ... }` in expression position, producing syntax behavior that differs from ordinary cell input.
2. forcing statement/block form loses the existing REPL completion-value behavior, so a cell such as `20 + 22` would no longer naturally return `42`.

`resetContext()` preserves both the REPL's top-level-await transform and completion-value semantics while removing previous lexical declarations, so it is a better fit for this bridge.

## Regression coverage

`test-v2.1.mjs` is the focused production regression. The current suite covers at least:

1. v1 JavaScript/Python foreground behavior and absence of a v1 background-Eval parameter;
2. v2 JavaScript-only schema/runtime;
3. v2 per-cell lexical isolation plus explicit `store/load` persistence;
4. exact host-tool return envelope `{ text, content }`;
5. active-tool discovery and schema/metadata exposure;
6. model-invokable skill discovery/read restrictions;
7. absolute wall timeout and kernel restart;
8. outer abort propagation;
9. parallel host-call cap = 4;
10. delayed-abort accounting: caller abort does not release a still-live host slot;
11. sequential FIFO barrier behavior;
12. orphan host-call cancellation on cell settlement;
13. `agent()` foreground/background forwarding and parallel safety;
14. one-shot `completion()`;
15. Eval v2 background task start/wait/get/kill with Bash disabled;
16. task management remaining available when builtin Bash is suppressed;
17. captured extension-tool fallback when `ExtensionAPI.invokeTool` is unavailable.

A successful focused run ends with:

```text
Eval v2.1 focused production regression: PASS
```

## Review checklist for future changes

When changing this extension, verify all of the following rather than only the immediate feature:

- v1/v2 schema boundaries remain intentional;
- v2 remains JavaScript-only;
- v2 cell lexical scope is fresh on every call;
- `store/load` remains the supported bridge-managed v2 cross-cell data API and prompts do not rely on implicit lexical persistence;
- bare expression values and top-level await still work;
- host tools remain restricted to active tools in normal v2; host-owned eval-v2-only may expose registered tools only inside Eval, and Eval recursion remains blocked;
- unknown execution modes fail closed to sequential;
- parallel cap remains enforced against actually live work, including delayed aborts;
- cell settlement aborts orphan host work;
- timeout/abort resets the foreground kernel cleanly;
- background Eval uses an isolated kernel;
- Bash-off mode still retains Eval v2 task tools;
- session shutdown tears down workers/tasks/listeners;
- prompt/tool descriptions do not suggest v2 implicit variable/function persistence;
- focused regression passes and changed files contain no conflict markers or trailing whitespace.
