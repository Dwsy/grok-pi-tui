# grok-pi 原版 Grok MCP 编译闭包拆除进度报告

> **快照日期**：2026-11-16
>
> **工程目录**：`/Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116`
>
> **工作分支**：`refactor/pi-slim-20261116`
>
> **已核对 HEAD**：`4d4ee673c323828749f996d3bb6431038c6a76af`
>
> **执行策略**：不改主工作树、不改 Pi 源码、Workflow 暂不拆、不逐提交跑 Cargo/CI；集中完成代码边界后再一次性手动验证。

## 1. 执行摘要

**总体判断：原版 Grok MCP 的拆除处于进行中，尚未从 grok-pi 的编译闭包中消失。**

已经完成**源码层面**的配置/协议逆依赖治理，并为 Workspace 建立编译期 `stock-mcp` 开关，将自己的 MCP 运行时依赖、session 状态、Hub 注册/解绑及 `configure_mcp` RPC 放在 stock-only 边界内；默认 stock 路径仍启用。**这些变更均已提交，但关闭该特性后的 Rust 编译尚未验证。**

真正未解决的路径是 `xai-grok-shell -> xai-grok-mcp -> rmcp`。Shell 还在 ACP session / Agent 初始化 / 工具调用、MCP 服务器管理、重连和 credential 路径中使用原版 MCP，Pager、Pi Adapter 和 bin 又直接依赖 Shell。当前没有证据证明 Pi binary 不再构建 Grok MCP 或 `rmcp`。

**注意目标区别**：用户要求去掉的是**原版 Grok Rust MCP 的 Pi 编译依赖**，不是删除 grok-pi 独立的 Eval MCP。此前误删 Pi Eval MCP 的改动已由 `f59deb0f` 恢复；`registerEvalPi`、`PI_GROK_EVAL_MCP` 和 F2 `pi_eval_mcp` 入口仍在代码中。**Workflow 不在本轮拆除范围。**

## 2. 状态矩阵

| 工作流 / 模块 | 进度 | 可核实的完成事实 | 尚缺的验收 |
| --- | --- | --- | --- |
| S01：Pi jemalloc 默认特性 | 已完成，历史 CI 已覆盖 | Pi 使用 `--no-default-features`，保留 `sandbox-enforce`；先前 ARM64 CI 成功 | 与最终代码 SHA 的集成回归 |
| S02：`cryptify` stock-only | 已完成，历史 CI 已覆盖 | `stock-obfuscation` 切分；CI run `43798146593` success，对应较早 SHA `ed616ce9` | 与最终代码 SHA 的集成回归 |
| S03：Pager Render → Tools | 代码已提交 | Render 改为依赖较轻的图像/终端辅助 crate | 整体编译、运行回归 |
| S04：Pager Diff → Tools | 代码已提交 | 编辑详情上移 `xai-tool-types`，Diff 不再直接使用完整 Tools | serde wire / 编译回归 |
| S05–S07：PDF / S3 / GCS | 代码已提交 | 可选 stock 特性与 Pi 路径隔离设计已落地 | 最终目标架构 Cargo tree 负向断言 |
| S08a-1～8：基础类型及逆依赖 | 代码已提交 | TaskKind、认证、Hooks、Memory、ClientType、MCP OAuth 等轻量类型回归其所有者模块 | 各 crate 编译、stock/Pi 回归 |
| **S08a-9：Grok MCP 协议/配置下沉** | **源码完成，待编译** | 名称解析至 `xai-grok-workspace-types`；MCP 服务配置与去重策略至 `xai-grok-config-types`；Workspace Config/Permission 不再直接导入运行时 MCP | 工具 ID 校验、配置 serde、stock MCP 测试 |
| **S08a-10：Workspace `stock-mcp`** | **源码完成，待编译** | `xai-grok-mcp` 和 `xai-computer-hub-mcp-adapter` 为 optional；MCP Session/Handle/Hub 路径受 feature 控制；stock 默认特性启用 | `cargo check`、`cargo tree`，验证非 stock 模式与默认 stock 模式 |
| **S08a-11：Shell MCP 元数据类型下沉** | **源码完成，待编译** | `McpServerMetaConfig`、`McpMetaConfigMap`、`parse_mcp_meta_config` 位于 Config Types，原路径 re-export | 编译、ACP `_meta.mcpConfig` 数据契约 |
| **S08d：Shell 原版 MCP 运行时** | **未完成** | 已识别实际耦合与文件位置 | Shell 的编译期隔离、下游特性传播、Pi 负向依赖树 |
| S08b：Pager external 与 stock agent 隔离 | 尚未实施 | 原生 Pager UI 边界已梳理 | 实际移除 stock Agent 闭包 |
| S08c：Workflow Host 拆分 | **暂停** | 用户要求 Workflow 先不拆，源码未作该方向变更 | 不在本轮验收范围 |
| S09：统一验收 / 性能测量 | 未执行 | 手动 CI 工作流已配置验收步骤 | ARM64 构建、真实 smoke、Herdr/Remote TUI、CI 与性能对照 |

**不要以“已提交的刀数 / 总刀数”计算所谓完成百分比**：最终验收对象是 Pi **完整 Cargo 可达编译依赖树**，不是代码改动数量。

## 3. 已完成：MCP 相关实际代码变更

### 3.1 去掉纯类型对 MCP 运行时的错误依赖

1. `xai-grok-workspace-types/src/mcp_names.rs` 成为 `server__tool` 格式解析的拥有者；`xai-grok-mcp::servers` 保留旧 API 的 re-export。
2. `xai-grok-config-types/src/mcp.rs` 拥有 MCP server 名称、配置去重/数量上限、ACP `_meta.mcpConfig` 和 OAuth 配置 DTO。原 runtime 使用同一份类型/解析代码。
3. Workspace `config.rs` 与 Permission 的 `manager/mod.rs`、`prompter.rs` 去掉 `xai_grok_mcp::` 的直接调用。
4. Shell MVP Agent 的配置读取改为直接引用 Config Types，而不是为了一个 `McpMetaConfigMap` 依赖整个原版 MCP runtime。

### 3.2 Workspace stock-only 边界

在 `xai-grok-workspace/Cargo.toml`：

```toml
[features]
default = ["sandbox-enforce", "stock-mcp"]
stock-mcp = ["dep:xai-grok-mcp", "dep:xai-computer-hub-mcp-adapter"]
```

两个实际依赖都声明了 `optional = true`。Session 的 MCP 运行状态字段、Workspace Handle 的 start/reload/converge/teardown、Hub 的 configure MCP 与 unbind callback 采用 `#[cfg(feature = "stock-mcp")]`。非 stock 分支不再创建 MCP Hub unbind callback。

**边界限制**：Workspace 的 `default` **仍启用** `stock-mcp`，而 Shell / Pager / Render / Terminal / Bin 等消费者的 workspace 依赖边尚未统一设置 `default-features = false`。即使单独的 Workspace 禁用默认特性成功，Pi 的 Cargo feature-unification 仍可能重新激活它；况且 Shell 自己也有不可选的 Grok MCP 依赖。

## 4. 当前仍然存在的阻塞依赖

```text
grok-pi  [xai-grok-pager-bin]
   ├── xai-grok-shell ───────────────────────┐
   ├── xai-grok-pager ─── xai-grok-shell ───┤
   ├── pi-grok-adapter ── xai-grok-shell ───┤
   │                    （Workflow 等）      │
   └── xai-grok-workspace [stock-mcp 默认开]  │
                                               │
                xai-grok-shell                  │
                    └── xai-grok-mcp ◄─────────┘
                         └── rmcp
```

**已在 manifest 中直接确认**：`xai-grok-shell/Cargo.toml` 的 `xai-grok-mcp = { workspace = true }` 仍是强制依赖；Workspace 虽改成 optional，但默认开启它。以上是基于 Cargo manifest 和源码的可达性判断，**不是本轮运行 `cargo tree` 后的结果**。

Shell 的关键耦合包括：

| 位置 | 仍在使用的 MCP 能力 | 为什么不能直接删 |
| --- | --- | --- |
| `session/acp_session.rs`、`session/acp_session_impl/{mcp,mcp_init,run_loop}.rs` | `McpState`、连接初始化、重试、通知、搜索/工具回调、池状态 | 与 stock ACP Session actor 的字段及事件循环交织 |
| `session/{mcp_servers,mcp_dispatcher,mcp_restart,mcp_elicitation}.rs` | MCP transport、动态工具 dispatch、reconnect 和 elicitation | 需按边界禁用实际运行能力，而非提供一个虚假的 MCP 服务 |
| `extensions/mcp.rs`、`mcp_doctor.rs` | 原版 Grok MCP 扩展协议与检查逻辑 | 需 stock-only 模块边界 |
| `agent/mvp_agent/*`、`session/commands.rs` | MCP 控制命令与会话初始化选项 | 不能让 Pi 所需的共享 Session/Workflow 类型一起消失 |
| `xai-grok-pager-bin`、`xai-grok-pager`、`pi-grok-adapter` | 直接或间接链接 Shell / Workspace | Cargo feature-unification 能从任一启用源重新拉入 MCP |

## 5. 验证证据与不能宣称的结论

### 已执行

- 最近这批代码执行了 `rustfmt --check`、`git diff --check`、Cargo TOML/lock 结构和 dependency direction 的静态检查。
- 对部分挪移的名称解析及元数据定义执行了与原版文本的静态对照，并补了单元测试代码。
- 手动工作流 `.github/workflows/pi-slim-ci.yml` 配置为 `workflow_dispatch`，不在每个提交上自动运行。
- 已经将 Workspace 的 **不启用 stock MCP** 检查加入手动 CI：目标架构 `cargo tree` 负向断言，后接 `cargo check --lib`。**这些步骤尚未运行。**
- 所有本轮提交在隔离 worktree 完成，并推送到远程分支；未修改 Workflow 或 Pi 源码。

### 尚未执行，禁止误报

- 没有在 `4d4ee673` 运行 Cargo 编译或测试；不能称为构建成功或验证闭包完全断开。
- 没有在此 SHA 上执行新的 ARM64 CI；历史成功的 run `43798146593` 仅对应早先的 S02 SHA `ed616ce9`。
- 没有可靠的本轮前后编译 wall-clock / codegen unit 对比，也没有得到 MCP 单独带来的缩时百分比。
- 没有新 beta 发布，没有 Herdr / Remote TUI / Dashboard / Workflow 的新二进制实测结果。

## 6. 下一批唯一主线（Workflow 冻结）

| 优先级 | 具体动作 | 退出条件 |
| --- | --- | --- |
| P0 | **完整隔离 Shell stock MCP runtime**：先分组整理 `session/acp_session` 的 MCP 状态和命令消费，再处理 `extensions/mcp`、`session/mcp_*`、`mcp_doctor`、Agent 初始化等可编译路径 | Shell 的无 MCP 构建不需要 `xai-grok-mcp`，stock 构建仍保留原 MCP 实现 |
| P0 | Shell Cargo 加明确 `stock-mcp` 特性，依赖变 optional，传递给 Workspace；控制 stock 默认路径与 `default-bazel` | 不留 runtime `if` 冒充编译隔离；不复制 MCP actor / 协议实现 |
| P0 | 审核所有 Grok Shell / Workspace 的下游 Cargo 边与特性统一，组合 Pi feature 与 stock feature | `grok-pi` 构建组合下没有任何路径重新启用 MCP |
| P1 | **集中执行一次验收**：Pi `cargo tree --locked -p xai-grok-pager-bin --no-default-features --features release-dist,sandbox-enforce --target aarch64-apple-darwin -e normal,build` | 不含 `xai-grok-mcp`、`rmcp` 和独占 MCP SDK/bridge 编译单元 |
| P1 | ARM64 Pi 构建与 smoke；stock Grok MCP 正向回归；Pi RPC/ACP、Herdr、Remote TUI、Dashboard、Goal、Eval v2、Workflow 行为回归 | 两个产品模式都真实通过，且 Workflow 未被拆除 |
| P2 | 同 runner、同 profile、冷热缓存分别比较 wall-clock、Cargo units、binary size | 给出有来源、可复现的瘦身收益报告 |

**本轮明确不做**：Workflow Host 提取/重写，替换原生 Pager，删除 grok-pi 的 Eval MCP，修改 Pi 源码，逐提交 CI，未经授权发布 beta。

## 7. 提交和交接索引

| Commit | 内容 |
| --- | --- |
| `f59deb0f` | 纠正误操作：恢复 grok-pi 自身 Eval MCP 功能 |
| `509129ee` | Grok MCP 纯名称/配置解析移出 runtime |
| `fce70718` | 文档明确区分 stock MCP 与 Pi Eval MCP |
| `310dd096` | Workspace MCP optional + `stock-mcp` 编译门控 |
| `dac560fa` | 记录 Workspace 特性边界 |
| `132a82e6` | ACP MCP 元数据解析下沉 Config Types |
| `4d4ee673` | 添加手动、尚未运行的无 stock MCP Workspace 验收 |

更完整的历史瘦身清单、基线时间与每项实现坐标，见同目录 `20261116-grok-pi-Pi-only编译闭包拆除路线图.md`。

**交接结论**：现在应推进 **Shell 的真实编译边界**，而不是继续拆无状态 helper；只有 Pi target-specific Cargo tree 明确不含 `xai-grok-mcp` / `rmcp`，才可以把 Grok MCP 项目标记为“拆除完成”。

## 8. 增量进度：S08a-12（2026-11-16，待编译）

**此节记录原快照 `4d4ee673` 之后新增的拆分，不改变上面的历史证据。**

1. **Cargo 编译特性**：Shell 的 `xai-grok-mcp` 改为 `optional`，由 `stock-mcp` 启用，并转发给 Workspace。Cargo 根 workspace 引用 Shell 时关闭其默认依赖特性；Bin、Pager、Minimal Renderer、Update 分别设置显式的 stock 转发链，原版 Grok 默认依旧启用 `stock-mcp`，Pi 的 `--no-default-features --features release-dist,sandbox-enforce` 不开启。
2. **静态 Cargo manifest 图**：模拟常规/build 依赖及默认特性传播时，Pi 配置可达 **90 个本地 crate**、不包含 `xai-grok-mcp`；stock 默认配置可达 **92 个本地 crate**，仍包含 `xai-grok-mcp`。**这是非 Cargo 的静态求解，不是实际 target-specific `cargo tree`，不能作为“构建已成功”证据**。
3. **源码编译门控**：Shell `mcp_doctor`、`extensions::mcp`、`session::mcp_*`、managed MCP 及相关 ACP 子模块改成 stock-only；`SessionCommand` 的 12 类 MCP 专属变体、事件循环处理、`SessionHandle` 对应接口增加统一特性门控。Pager 的原版 `mcp` CLI 命令只保留在 stock 构建。原版 ACP 初始化的 MCP 能力仅在 stock 开启。
4. **集中 CI 验收断言**：手动 ARM64 流程已同时要求 Pi 缺席 `xai-grok-mcp`/`rmcp`/MCP Bridge，原版 Grok 包含 `xai-grok-mcp` 与 `rmcp`；**尚未触发**。

**S08a-12 仍未完成的实质阻塞**：`xai-grok-shell::session::acp_session` 仍有直接的 `McpState`、`McpInitStrategy` 和 managed MCP 字段/初始化；`acp_session_impl/run_loop.rs` 仍有 MCP startup/dispatcher 任务；部分 Agent/Session 模块仍非条件引用 MCP API。因此**无 MCP 的 Shell 源码尚未被证明能通过 Rust 编译**。不要用上面的 manifest 结果宣告产品完工，也不要为了掩盖类型问题而增加假的 MCP stub。

**约束不变**：不修改 Workflow 实现；不删除 Pi Eval MCP；不改 Pi 源码；不运行逐提交 Cargo/CI、不自动发布。
