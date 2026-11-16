---
id: "2026-09-30-eval-v2-pi-codemode-mcp"
title: "Eval v2 学习 Pi Codemode 并复用 MCP"
status: "in-progress"
created: "2026-09-30"
updated: "2026-09-30"
category: "adapter"
tags: ["workhub", "grok-pi", "eval-v2", "codemode", "mcp", "tool-bridge"]
---

# Issue: Eval v2 学习 Pi Codemode 并复用 MCP

## Goal

让 Eval v2 吸收 Pi Codemode 已验证的程序化工具编排契约，并通过 Pi 官方 MCP Extension 复用 MCP Server 能力，同时保持 Pi、Eval runtime 与 Grok Pager 的所有权边界不变。

本 Issue 是 Pi Codemode/MCP 接入的实施 SSOT。当前已落地 Pi `0.99.0+` 最低版本门禁与普通模式下可选的 `builtin:codemode` 加载；Eval v2 复用 Pi 出站 MCP Tool Registry 仍按后续阶段推进。

## 结论

```text
MCP Server
    │  stdio / Streamable HTTP / OAuth
    ▼
Pi built-in MCP Extension
    │  mcp__<server>__<tool>
    ▼
Pi Tool Registry / tool pipeline
    ├── Pi Codemode（普通模式可用）
    └── EvalSessionToolBridge
            ▼
        Eval v2: tool.<name>() / tools.*
```

必须复用 Pi 的 MCP Client、连接生命周期、认证、资源读取和工具管线；**不要在 Eval Worker 内再实现 MCP Client**，也不要把 Pi Codemode QuickJS 直接嵌套到当前 Eval Worker。

Eval v2 继续保留自己的产品能力：Python、持久 Worker、每 cell lexical isolation、`store/load`、`parallel/pipeline`、`completion()`、blocking `agent()`、后台任务和 Pager 原生 task projection。

## 背景与当前事实

### Pi Codemode/MCP 基线

本方案依据本机 `~/.pi/pi` 当前 checkout：

```text
HEAD 1b347794e fix(durable): cover ownership cancellation races (Package 18)
```

关键源码和文档：

- `~/.pi/pi/packages/codemode/README.md`
- `~/.pi/pi/packages/codemode/src/`
- `~/.pi/pi/packages/coding-agent/docs/mcp.md`
- `~/.pi/pi/packages/coding-agent/docs/cli.md` 的 `Enable codemode`
- `~/.pi/pi/packages/coding-agent/src/extensions/codemode/tool.ts`
- `~/.pi/pi/packages/coding-agent/src/extensions/codemode/execute.ts`
- `~/.pi/pi/packages/coding-agent/src/extensions/mcp/index.ts`
- `~/.pi/pi/packages/coding-agent/examples/sdk/14-codemode-mcp.ts`
- `~/.pi/pi/packages/coding-agent/CHANGELOG.md`（Codemode/MCP 从 `0.99.0` 开始）

版本前提：grok-pi 的最低支持版本直接提升为 Pi `0.99.0`，该版本已经提供 built-in Codemode/MCP。无需为旧 Pi 保留兼容分支、能力探测或备用 MCP Client；宿主沿用统一的最低版本门禁，低于 `0.99.0` 直接拒绝启动。

已确认的 Pi 语义：

1. Codemode 在 QuickJS WASM 中执行 model-written JavaScript；脚本只能通过注入的 tools/globals 访问宿主，没有 Node、filesystem、network、timer 或 module import 能力。
2. 脚本通过 `tools.<name>()` 调用工具，`ALL_TOOLS`、`searchTools()`、`describeTool()` 做发现；只有脚本 output/return value 回到模型，nested tool result 不进入模型上下文。
3. `store/load` 由 coding-agent Extension 以 `codemode-store` custom entry 持久化到当前 session branch。
4. MCP 配置位于 `~/.pi/agent/mcp.json` 或 trusted project 的 `.pi/mcp.json`；支持 stdio 与 Streamable HTTP，HTTP 可使用 OAuth。
5. MCP 默认 `exposure = "codemode"`；工具名为 `mcp__<server>__<tool>`，全部取值为 `direct`、`codemode`（默认）、`codemode-deferred`、`deferred`、`hidden`（`src/core/mcp-servers.ts:18`）。
   - 注册时经过 `toToolExposure()` 映射（`extensions/mcp/tools.ts:38`）：`codemode-deferred` 被降级为 `deferred`。
   - 因此 Eval bridge 若只读 `ToolInfo.exposure`，拿到的是 `deferred`，**无法区分** “给 codemode 脚本用但不列在描述里” 与 “给 `tool_search` 加载”。需要该区分时必须读 MCP 侧原始 exposure（`getMcpToolExposure`）或已解析的 `mcp.json`，而不是 `ToolExposure`。该差异直接影响 `tools.list()` 是否收录这类工具，必须在 Phase 2 显式决策。
6. MCP nested call 经过 Pi tool pipeline，因此 tool validation、permission 和 `tool_call/tool_result` hooks 继续生效。
7. MCP 的 resource 通过 `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` 访问；大文本和二进制不应无界地进入模型上下文。

### grok-pi 当前语义

主要代码：

- `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs`
  - 启动时主动加入 `--no-extensions`，再显式注入经过宿主资源策略批准的 bridge extensions。
  - 当前没有把 Pi built-in `mcp` / `codemode` 加入 startup extension plan。
- `extensions/pi-grok-bash/index.ts`
  - 注册 Eval v1/v2、HostCallGate、后台任务和 Eval v2-only policy。
- `extensions/pi-grok-bash/eval.ts`
  - 当前 JS/Python Worker、wire protocol、`tool.*`、`tools.*`、`skills.*`、`parallel/pipeline`、`completion/agent`。
- `extensions/pi-grok-bash/tool-bridge.ts`
  - `EvalSessionToolBridge` 发现 Pi registered/active tools，优先走 `ExtensionAPI.invokeTool`，否则走 captured wrapped tool。
- `extensions/pi-grok-bash/eval-pi-mcp.ts`
  - 已实现反向的 Eval MCP facade：外部 MCP Client 连接当前 live Eval v2，不是 MCP Server Client。
- `docs/issues/grok-pi/20260929-eval-pi-mcp.md`
  - 已有 Eval v2-only MCP 的 URL secret、binding ID、图片 Resource、tokenizer 和 shutdown contract。

当前最重要的边界：

```text
现有 eval-pi-mcp：外部 Agent ──MCP──> 当前 grok-pi Eval v2
本 Issue 目标：Eval v2 ──Pi Tool Registry──> 外部 MCP Server
```

两者方向相反，不应合并成一个模糊的 “MCP mode”。

## 非目标

- 不修改 `~/.pi/pi` 或 `pi-main` 的 Pi core 源码来扩展 RPC。
- 不在 Eval Worker 中安装或连接 MCP SDK；MCP Client 只属于 Pi Extension 层。
- 不用 Pi QuickJS Codemode 替换 Eval v2 的 Node/Python Worker。
- 不把 upstream `codemode` 作为 Eval v2 的递归 nested tool；`eval -> codemode -> eval` 必须 fail closed。
- 不默认启动任意 MCP stdio 子进程或访问远程 MCP Server；接入必须显式 opt-in，并遵循 Pi project trust。
- 不读取 Grok native `[mcp_servers]` 配置来冒充 Pi MCP；Pi MCP 使用 Pi 的 `mcp.json`，Grok native MCP 仍是独立边界。
- 不改变既有 `eval-pi-mcp` 的认证、绑定轮换、图片资源和 token accounting contract。
- 不因为 MCP 接入而把 `eval-v2-only` 变成第二套顶层工具选择器。
- 不在本 Issue 中实现 live cell、remote Eval provider、Pi Core canonical `value` API 或新的调度器；这些沿用既有 `20260911-Eval-v2-学习-Codex-Code-Mode-实施方案.md` 的后续路线。

## 目标架构与契约

### 1. Pi MCP 是唯一 MCP Client

Pi built-in MCP Extension 负责：

- 读取 `~/.pi/agent/mcp.json` 与 trusted project `.pi/mcp.json`；
- 启动/关闭 stdio Server；
- Streamable HTTP、OAuth、重连和 server/tool list change；
- MCP tool/resource registration；
- Pi 原生 tool validation、permission、tool hooks 和 timeout。

Eval v2 只观察 Pi Tool Registry，不直接持有 `McpClient`、transport、OAuth token 或 server process。

### 2. 两种模型暴露模式

| 模式 | 顶层模型看到的工具 | Eval v2 能力 |
|---|---|---|
| 普通模式 + Pi MCP | 由 Pi active tool policy 和 MCP exposure 决定；Codemode 可按 upstream 规则自动启用 | 当前 active MCP tool；后续可由显式 policy 扩展到允许的 registry tool |
| `pi_eval_v2_only` | 只保留 `eval`；upstream `codemode`/`tool_search` 不得成为模型顶层工具 | 允许的 registered MCP tool 仅在 `tool.<name>()` 中可见 |

`eval-v2-only` 下可加载 Codemode Extension 作为 MCP 的 discovery dependency，但必须由宿主在 `before_agent_start` 和 MCP tool-list 变化后重新收敛 active set，避免 `codemode` 泄漏为顶层模型工具。Eval bridge 同时排除 `eval`、`codemode`、`tool_search` 本身，防止递归和双重 discovery。

### 3. Eval v2 的 MCP tool API

第一版保持现有 API，不引入新的 `mcp.*` namespace：

```js
const candidates = tools.search("github issue");
const result = await tool.mcp__github__create_issue({
  owner: "example",
  repo: "demo",
  title: "bug",
});
result.text;
```

规则：

- 工具真实名称使用 Pi 的 `mcp__<server>__<tool>`；名称含非 JavaScript identifier 字符时使用 `tool["..."]`。
- `tools.list/search/describe` 保持现有 Eval v2 API；可增加 Codemode 兼容的 `ALL_TOOLS`、`searchTools` 别名，但不删除现有 API。
- MCP tool result 继续经过 `EvalSessionToolBridge`；不得通过 worker 内部 HTTP 绕过 Pi pipeline。
- 如果 Pi runtime 提供 `structuredContent/outputSchema`，优先保留结构化值；没有 canonical value 时返回稳定 `{text, content}` envelope，不根据 text 是否像 JSON 自动 `JSON.parse`。
- MCP image/resource link 必须保持 content/resource 语义，不能把 base64 拼入普通 text。
- `executionMode` 继续由 HostCallGate 解释；未知模式 fail closed 为 sequential。

### 4. Tool discovery：Eval v2 自己的 search，不复用 `tool_search`

#### 4.1 为什么不能桥接 Pi 的 `tool_search`

`tool_search` 与 `codemode` 在 Pi 中的 `exposure` 都是 `"model-only"`（`extensions/tool-search/tool.ts:243`、`extensions/codemode/tool.ts:428`）。`model-only` 的定义是（`core/extensions/types.ts:505`）：

> `model-only`: declared to the model while active, **never callable**. Use it for orchestrating or interactive tools.

`getActiveTools()` 的契约进一步确认只有两类 exposure 可从脚本调用（`core/extensions/types.ts:1715`）：

> Tools with `codemode` or `deferred` exposure stay callable from codemode scripts whether active or not.

因此：

| 工具 | exposure | Eval bridge 能否调用 | 结论 |
|---|---|---|---|
| `tool_search` | `model-only` | ❌ 永不可调用 | 不得桥接 |
| `codemode` | `model-only` | ❌ 永不可调用 | 不得桥接（与本 Issue 既有结论一致） |
| `mcp__*`（`codemode`） | `codemode` | ✅ 无论 active 与否 | 正常投影 |
| `mcp__*`（`deferred`） | `deferred` | ✅ 无论 active 与否 | 正常投影 |

这把“`tool.eval`、`tool.codemode`、`tool.tool_search` 均 fail closed”从**策略选择**升级为**平台语义强制**：即使 Eval bridge 不写任何排除逻辑，Pi 也不会把 `tool_search` 交给 `ctx.executeTool()`。排除逻辑仍需保留，作用是给出可诊断的错误信息，而不是充当安全边界。

#### 4.2 两个 search 的语义不同，不能合并

| | Pi `tool_search` | Eval v2 需要的 search |
|---|---|---|
| 搜索对象 | 未声明到模型的工具（`codemode` + `deferred` exposure） | 已注册且被 allow policy 允许的工具 |
| 副作用 | **调用 `setActiveTools()` 激活匹配项**，改变下一次模型调用的声明 | 无。纯只读查询 |
| 结果去向 | 进入顶层模型上下文（下轮声明） | 只回到脚本，不进入模型上下文 |
| 触发者 | 顶层模型 | 脚本代码 |
| 实现位置 | `extensions/tool-search/tool.ts` | `extensions/pi-grok-bash/eval.ts` Worker 内 |

把 `tool_search` 桥进 Eval 会导致脚本调用它时**改写顶层 active set**，直接破坏 `pi_eval_v2_only` 的“顶层只暴露 `eval`”契约。这是必须显式禁止的反模式，见“代码边界”一节。

#### 4.3 实现缺口：BM25 ranker 对外不可达

Eval v2 需要自己的 search 实现。Pi 侧的可复用件全部**未导出**：

- `createToolSearchDocument(tool, namespace)` — `extensions/tool-search/tool.ts:107`
- `Bm25Ranker` — `extensions/tool-search/tool.ts:118`
- `tokenize(text)` — `extensions/tool-search/tool.ts:72`

`packages/coding-agent/src/index.ts` 只导出 `createToolSearchExtension`（第 408 行），因此外部扩展（grok-pi 属于此类）无法复用 Pi 的 ranker 与文档构造。三个选项：

| 方案 | 改动面 | 评价 |
|---|---|---|
| A. 请求 Pi 导出 `Bm25Ranker` + `createToolSearchDocument` | Pi 侧一行 export + 版本依赖 | 最优；避免双实现漂移，但需等待 Pi 发版，且 grok-pi 不能修改 Pi source |
| B. grok-pi 内部实现等价 BM25 | ~80 行 + focused test | 可行但重复实现，长期有漂移风险 |
| C. 先用 `tools.list()` + 大小写不敏感子串/分词过滤 | ~15 行 | 最小可用；等 A 落地再替换内部实现 |

**建议**：Phase 3 先落 C，把 `tools.search()` 的签名与返回结构定死；A 作为后续优化，B 不推荐。接口稳定比排序质量更重要——脚本的可发现性不依赖 BM25 的精确度，而 `tools.list()` + `describeTool()` 已经覆盖精确查找路径。若后续实测发现大 catalog（30+ 工具）下 C 的召回明显不足，再做 B。

#### 4.4 Eval v2 search API 契约（第一版）

保持现有 `tools.*`，不引入第二个 namespace：

```js
tools.list()                        // 全部允许的工具 metadata
tools.search(query)                 // 只读；不改变任何 active set
tools.search(query, { limit })      // limit 为新增可选参数，默认 8，向后兼容
tools.describe(name)                // 单工具完整声明 + schema
```

新增 Codemode 兼容只读别名（保持与 Pi Codemode 脚本一致的学习曲线）：

```js
ALL_TOOLS                            // 只读 catalog，等同 tools.list()
searchTools(query, { limit, namespace })  // 等同 tools.search()
describeTool(name)                   // 等同 tools.describe()
```

强制约束：

- `ALL_TOOLS` 必须 `Object.freeze()`；`searchTools`/`describeTool` 在脚本侧只读，不得暴露任何写入能力或对 `setActiveTools` 的间接可达路径。
- 搜索输入在脚本内做类型校验（非字符串、空串、非法 `limit` 抛 `TypeError`），与现有 `tools.search(query)` 的校验风格一致。
- `searchTools` 的 `namespace` 选项按 `mcp__<server>` 前缀过滤；未知 namespace 返回空数组，不抛错。
- 搜索结果只返回 `{ name, description }`，**不返回 schema**；schema 通过 `describeTool()` 显式获取，避免大 MCP server 的 schema 灌满 cell 输出。
- catalog 每个 cell 重新构建（见 Phase 2 第 1 点），搜索不得缓存跨 cell 结果，否则会违反“旧工具消失后 fail closed”。
- 现有 `tools.list/search/describe` 的行为、返回结构和错误语义保持向后兼容；别名是新增，不是替换。

#### 4.5 catalog 规模上限

大 MCP server（例如 30+ 工具）不能把全部 schema 塞进 prompt 或 cell：

- `tools.list()` 返回 metadata（name / description / namespace），**不含 schema**。
- `describeTool()` 返回单个工具的完整声明，由脚本按需调用。
- Eval v2 prompt 只声明 `tools.*` 的用法，**不内联任何工具签名**——这是与 Pi Codemode 的关键差异：Codemode 把声明放进描述并受 `inlineBudget`（默认 3000 token）约束，Eval v2 走运行时发现，prompt 不随工具数量增长。
- cell 输出沿用现有 `MAX_OUTPUT_BYTES` 限制；`tools.list()` 结果过大时由既有截断机制处理。
- 需为 inline 部分设定明确上限并写进验收：prompt 中与工具目录相关的 token 必须是常数级，不随 MCP server 数量或工具数量增长。

### 5. MCP resources

Pi built-in MCP resource tools作为普通 registered tools进入 Eval catalog：

```js
const resources = await tool.list_mcp_resources({ server: "docs" });
const page = await tool.read_mcp_resource({
  server: "docs",
  uri: resources.resources[0].uri,
});
```

第一版不在 Eval Worker 增加 `fetch()` 或 MCP URI resolver。文本限制、图片资源、临时文件和 resource link 仍由 Pi MCP resource tools及既有 Eval output limits负责。

## 实施阶段

### Phase 0：文档和边界基线（本次）

- [x] 读取 `~/.pi/pi` Codemode/MCP 实现和文档。
- [x] 记录 grok-pi 当前 `--no-extensions` 与显式 bridge injection 边界。
- [x] 区分“Pi 连接外部 MCP”与“外部 Agent 连接 Eval v2”。
- [x] 修正 `extensions/pi-grok-bash/README.md` 中 Eval v2 Python 能力的过时表述。
- [x] 将本方案加入 Issue、README 和 Feature Matrix 的文档入口。
- [x] 将 grok-pi 最低 Pi 版本统一提升到 `0.99.0+`。
- [x] F2 Built-in tools 增加默认关闭的 `codemode`；显式选择时宿主加载 `builtin:codemode`，未选择时不启动该扩展。

### Phase 1：官方 Pi MCP Extension 接入（P0，默认关闭）

目标：让 grok-pi 能在不修改 Pi source 的情况下加载官方 `builtin:mcp`，并按 Pi 语义连接 `mcp.json`。

计划：

1. 增加 grok-pi 自有的 `pi_mcp` host feature，默认关闭、需重启；它与已有 `pi_eval_mcp` 明确分开。
2. 只在 `pi_mcp` 开启时，通过官方 extension path/resource loader 显式加入 `builtin:mcp`。
3. 同时加入 `builtin:codemode`，因为 Pi MCP 默认 `exposure=codemode` 需要 Codemode discovery tool；不复制 Codemode source。
4. 普通模式保持 upstream MCP exposure 和 `/mcp` manager 语义；不把 Grok native `/mcps` 混进来。
5. `pi_eval_v2_only` 模式下，Codemode 只作为内部 discovery dependency，不进入顶层 active tools。
6. 首次连接、project trust、server failure、reconnect、session shutdown 分别做可观测性检查。

预期涉及：

- `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/` 的 host feature manifest/wiring
- `crates/codegen/xai-grok-pager/src/settings/defs.rs`
- `crates/codegen/xai-grok-pager/src/settings/registry.rs`
- `crates/codegen/xai-grok-pager/src/app/dispatch/`
- `extensions/pi-grok-web-config/web/ui-config.json` 与 i18n
- 对应 F2/native setting tests

强约束：

- 不直接编辑 `pi-main` 或 `~/.pi/pi`。
- 不把 MCP 服务器配置复制到 `.grok-pi`；先遵循 Pi `getAgentDir()` / `PI_CODING_AGENT_DIR` 和 trusted project `.pi/mcp.json` 的现有语义。
- 不启动 MCP Server，除非用户开启 `pi_mcp` 且 Pi 配置中存在 enabled entry。

验收：

- `pi_mcp=false` 时启动命令不含 `builtin:mcp`/`builtin:codemode`，端口/子进程均不产生。
- `pi_mcp=true` 时 `/mcp` 能列出 connected/failed/needs-auth 状态。
- project untrusted 时不读取 project stdio MCP 配置。
- shutdown、`/new`、session switch、`/reload` 不留下 MCP 子进程。
- 正常模式下 Codemode 可调用一个本地 fixture MCP Server。

### Phase 2：EvalSessionToolBridge MCP projection（P0）

目标：在 MCP tool 已由 Pi 注册后，让 Eval v2 复用其注册、schema、hooks、permission 和 lifecycle。

计划：

1. `catalog()` 每个 cell 重新观察 Pi registry，支持 MCP Server 动态 tool list 变化。
2. 对 registered-but-inactive tool 采用显式 allow policy：
   - 普通 Eval v2：只允许 Pi active tools；
   - `eval-v2-only`：允许已注册且非 hidden 的 MCP/Pi tool；
   - `eval`、`codemode`、`tool_search` 永远排除；后两者本就是 `model-only`（见 §4.1），此处排除是为了产出可诊断错误，而非安全边界；
   - CLI `--exclude-tools`、`--no-tools`、`--no-builtin-tools` 仍优先。
3. 保留 `executionMode` metadata；MCP 未声明时按 sequential。
4. 使用 `mcp__server__tool` 名称建立 namespace，必要时从 registered definition/source metadata 补充描述。
5. 动态 MCP tool 消失后，旧 catalog 的调用必须明确返回 unavailable，不得调用陈旧闭包。
6. 调用路径优先 `pi.invokeTool()`；fallback wrapper 继续经过现有 preflight、tool events、result patch 和 UI projection。
7. 不把 MCP server process、OAuth token、transport 状态暴露给 Eval Worker。

预期涉及：

- `extensions/pi-grok-bash/tool-bridge.ts`
- `extensions/pi-grok-bash/eval.ts`
- `extensions/pi-grok-bash/index.ts`
- 必要时 `extensions/pi-grok-bash/prompts.ts`

验收：

- Eval v2 cell 能调用 fixture MCP tool，并观察到参数校验和 permission block。
- MCP tool 的完整 nested result 不进入主模型 transcript。
- Eval v2-only 顶层仍只有 `eval`；MCP tool 仅在 `tool.*` 中出现。
- `tool.eval`、`tool.codemode`、`tool.tool_search` 均 fail closed。
- server tool list 变化后，下一 cell 的 `tools.list/search/describe` 反映新目录。
- MCP tool error、abort、timeout 和 session shutdown 不遗留 host call。

### Phase 3：Codemode contract 对齐（P1）

只吸收低风险、可验证的行为，不替换 Eval interpreter：

1. 在 Eval v2 prompt 中说明 `tools.*` 只做发现、`tool.*` 才执行；这与 Pi Codemode 的 `tools`/callable tools 分离一致。
2. 增加 `ALL_TOOLS` 的轻量只读 catalog 和 `searchTools(query, options)` 兼容别名；**不得复用 `tool_search`**（`model-only`，不可调用，且会改写顶层 active set），实现方案与缺口见 §4。搜索不引入第二份 MCP catalog，只读同一份 Pi registry 投影。
3. 让 catalog 支持 compact signature + deferred long-tail schema；大 MCP server 不把全部 schema塞入 prompt。
4. 统一 nested call result projection：优先 structured content，否则 `{text, content}`；output、image、resource link 分离。
5. 统一 script cancellation：script end/timeout/outer abort 取消未 await nested call；HostCallGate slot 按真实 host work settlement 归还。
6. `store/load` 继续显式、JSON-safe；session-owned durable store 的后续重构沿既有 Codex Code Mode Issue，不在这里复制 Pi `codemode-store` 机制。

不做：

- 不让 Eval v2 依赖 QuickJS WASM。
- 不在同一 cell 同时维护 Node REPL 与 QuickJS 两个解释器。
- 不把 `Promise.all` 的表面语法当成 HostCallGate 的并发契约。
- 不把 upstream Codemode output header 当成 Eval v2 transcript contract。

验收：

- 现有 `tools.list/search/describe` 向后兼容。
- 大 MCP catalog 的 prompt inline token 有明确上限。
- schema 复杂或超过阈值时可通过 `describeTool`/`tools.describe` 获取。
- nested tool result、image、resource link 的程序值和展示值不混淆。

### Phase 4：Eval v2-only + MCP 的压力与安全验证（P1）

测试矩阵：

| 场景 | 必须证明 |
|---|---|
| stdio MCP server | process group 可关闭，stderr/失败状态可诊断 |
| Streamable HTTP MCP | loopback/remote config 按 Pi policy 工作，断线可重连 |
| OAuth MCP | token 不进入 Eval catalog、prompt、session transcript |
| project `.pi/mcp.json` | untrusted project 不启动 stdio |
| `exposure=codemode` | normal mode 可走 Pi Codemode；Eval-only 不泄漏顶层 codemode |
| `exposure=direct/deferred/hidden` | Eval bridge 按 exposure/allow policy 处理，hidden 永不可调用 |
| 动态 tool list | 新 cell 看到新工具，旧工具消失后 fail closed |
| 并发调用 | HostCallGate cap=4 与 sequential barrier 保持 |
| 图片/resource | 不把 base64 无界打印到模型 text |
| shutdown/switch | MCP connection、Eval kernel、background task 全部收敛 |

建议新增：

```text
extensions/pi-grok-bash/test-pi-codemode-mcp.mjs
```

以最小 fixture MCP Server 覆盖：tool list、structured result、text result、image/resource、error、abort 和动态工具更新；不依赖真实第三方云服务。

### Phase 5：文档和交付（每个实现 PR 必须完成）

- [ ] `extensions/pi-grok-bash/README.md` 更新配置、工具命名、Eval-only 语义和安全边界。
- [ ] `README.md` / `docs/README.zh-CN.md` 保持入口和当前状态一致。
- [ ] `docs/FEATURE_MATRIX.md` / `.zh-CN.md` 标注当前 boundary 与已交付状态，不把计划写成已支持。
- [ ] `docs/VERIFICATION.md` 增加 Pi MCP fixture 与 Eval bridge 验证命令。
- [ ] `CHANGELOG.MD` / `docs/CHANGELOG.zh-CN.md` 只在实现合入后记录用户可见行为。
- [ ] `git diff --check`、focused Node tests、必要的 Rust host tests 全部读取完整输出后再报告。

## 代码边界

```text
Pi source / built-in extensions
  owns: MCP transport, OAuth, server lifecycle, exposure, resource tools

pi-grok-bash
  owns: Eval worker, HostCallGate, nested invocation policy,
        Eval-only projection, task lifecycle

pi-grok-adapter
  owns: ACP/event projection only
  must not: connect MCP, run Eval worker, render Codemode UI

Grok Pager
  owns: settings, tool cards, status, task pane, notifications
```

禁止出现以下反模式：

```text
Eval Worker --HTTP--> MCP Server                 # duplicate MCP client
Eval Worker --direct invoke--> Extension Tool    # bypass Pi pipeline
Eval v2 --spawn--> Codemode --spawn--> Eval       # recursive runtime cycle
Eval script --tool.tool_search()--> setActiveTools()  # 脚本改写顶层 active set，破坏 eval-v2-only
Eval script --ALL_TOOLS--> mutation of active set # discovery helper 越权成为配置入口
Rust Pager --read--> ~/.pi/agent/mcp.json        # duplicate config owner
```

## 配置与安全决策

| 决策 | 方案 | 理由 |
|---|---|---|
| Pi MCP 开关 | 新增 grok-pi `pi_mcp`，默认关闭、需重启 | MCP 可能启动任意 stdio 子进程或访问网络，不能静默扩大能力 |
| Codemode 开关 | 不单独暴露给 Eval 用户；作为 Pi MCP discovery dependency | 避免出现第二个顶层编排入口，保持 Eval v2-only 语义 |
| MCP 配置 | 遵循 Pi `~/.pi/agent/mcp.json` / trusted `.pi/mcp.json` | 复用 Pi 官方配置和 trust，不复制 Grok native MCP 配置 |
| Eval MCP facade | 保留现有 `pi_eval_mcp` | 它是外部 Agent → live Eval；与本 Issue 的 Eval → MCP 方向不同 |
| MCP tool naming | 保留 `mcp__server__tool` | 与 Pi Codemode、工具注册和冲突隔离保持一致 |
| MCP transport | 只由 Pi built-in MCP 承担 | 避免双连接、双 OAuth 和双重断线状态 |
| top-level tool policy | Eval v2-only 只显示 `eval` | Codemode/Tool Search 仅内部可用，不泄漏产品语义 |
| structured value | 优先 Pi structured content；没有则稳定 `{text,content}` | 不再通过 JSON-looking text 猜测 programmatic value |

## 与既有 Issue 的关系

- `docs/issues/adapter/20260911-Eval-v2-学习-Codex-Code-Mode-实施方案.md`
  - 继续负责 runtime ownership、service limits、provider boundary、live cell 和长期 canonical value 路线。
  - 本 Issue 只补充 Pi 官方 Codemode/MCP 的接入与工具投影，不重复实现 Codex runtime。
- `docs/issues/grok-pi/20260929-eval-pi-mcp.md`
  - 继续负责外部 MCP Client 访问当前 Eval v2 的安全 facade。
  - 本 Issue 不改变其 URL secret、binding ID、resource 和 tokenizer contract。
- `docs/issues/adapter/20260818-Eval Bridge v2 与 RLM runtime.md`
  - 继续负责 Eval v2/RLM 的 host-call、预算、canonical value 和 cancellation 后续研究。

## 接受标准

- [x] grok-pi 启动门禁要求 Pi `0.99.0+`；不保留旧 Pi 的 Codemode/MCP 兼容分支。
- [ ] `pi_mcp` 关闭时，grok-pi 不加载 Pi MCP/Codemode built-in，也不启动 MCP server。
- [ ] `pi_mcp` 开启时，Pi 官方 MCP extension 负责连接、认证、资源和 server lifecycle。
- [ ] Eval v2 通过 `EvalSessionToolBridge` 调用 `mcp__<server>__<tool>`，不持有第二个 MCP client。
- [ ] 所有 nested MCP 调用仍经过 Pi tool pipeline、参数校验、权限和 hooks。
- [ ] `pi_eval_v2_only` 顶层仍只暴露 `eval`，MCP 仅作为 Eval nested capability。
- [ ] `tool.eval`、`tool.codemode`、`tool.tool_search` 无法形成递归调用链。
- [ ] `tools.list/search/describe` 与 MCP 动态 tool list 收敛，catalog 不滞后于 Pi registry。
- [ ] `tools.search` / `searchTools` 为纯只读：调用前后 `pi.getActiveTools()` 完全不变（可用 regression 直接断言）。
- [ ] `tool.tool_search` 与 `tool.codemode` 均不可调用，且错误信息指出 `model-only` 语义，而不是笼统的 “not allowed”。
- [ ] `ALL_TOOLS`、`searchTools`、`describeTool` 别名可用，且与 `tools.list/search/describe` 返回结构一致。
- [ ] prompt 中与工具目录相关的 token 为常数级，不随 MCP server 数量或工具数量增长。
- [ ] 搜索结果不含 schema；仅 `describeTool()` 返回完整声明。
- [ ] MCP text、structured content、image 和 resource link 不被错误拼成单一 text/base64。
- [ ] MCP 子进程、HTTP connection、Eval kernel、background task 在 shutdown/session switch 后无泄漏。
- [ ] focused fixture regression、既有 Eval v2.1 regression、Eval MCP regression 与必要 host tests 通过。
- [ ] 文档只描述已实现能力；计划状态明确标为 planned/boundary。

## 当前状态与下一步

当前状态：**in-progress**。已完成 Pi `0.99.0+` 基线与普通模式的可选 Codemode 宿主接入；没有修改 Pi source，也尚未接入 Eval v2 → Pi MCP 的出站连接/Tool Registry 投影。

下一步顺序：

```text
P0  host feature + builtin:mcp/codemode opt-in
  -> P0  EvalSessionToolBridge MCP projection
  -> P1  Codemode catalog/result contract alignment
  -> P1  pressure/security fixture regression
  -> release docs + changelog
```

“不要把两个运行时焊成一个大工具；先共享唯一的 Pi Tool Registry。”

## Status 更新日志

- **2026-09-30**：创建方案，状态 → `planned`；完成 `~/.pi/pi` Codemode/MCP 与现有 Eval v2/Eval MCP 的边界对照。
- **2026-09-30**（复核）：对照 `1b347794e` 逐条核实源码，补入三项。
  - 修正 exposure 枚举：实际 5 个取值（补默认 `codemode`），并记录 `toToolExposure()` 把 `codemode-deferred` 降级为 `deferred` 造成的信息丢失——这是 Eval bridge 读 `ToolInfo.exposure` 时无法区分两类语义的根因。
  - 新增 §4 Tool discovery：确认 `tool_search` 与 `codemode` 同为 `model-only`（永不可调用），把排除逻辑从策略降级为诊断；明确 Eval v2 必须自建只读 search，桥接 `tool_search` 会因 `setActiveTools()` 副作用破坏 eval-v2-only 契约；记录 BM25 ranker 未导出这一实现缺口与三套方案取舍。
  - 验收标准补充只读性、别名兼容、prompt token 常数化三条可断言项。
- **2026-09-30**（决策更新）：最低支持 Pi 提升到 `0.99.0`；删除旧 Pi 的能力探测、兼容分支和备用 MCP Client 计划，统一由宿主版本门禁拒绝低版本。

- **2026-09-30**（beta.9 交付）：最低 Pi 基线提升到 `0.99.0+`；F2 Built-in tools 新增默认关闭的 `codemode`，显式选择后由 grok-pi 加载官方 `builtin:codemode`。Eval v2 → Pi MCP 仍保持 planned/boundary。
