# grok-pi Pi-only 编译瘦身：整体复盘与下一 Agent 交接

> 交接日期：2026-11-16
> 仓库：https://github.com/Dwsy/grok-pi-tui
> 隔离工作树：/Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116
> 工作分支：refactor/pi-slim-20261116
> 交接时 HEAD：39beb84c0dcbeaed43ac374a249dc6082883ad64
> 最后成功的本路线 ARM64 CI：#43798146593，SHA ed616ce9（**不是当前 HEAD**）
> **性质：审计与决策交接，不是「已经成功构建」的证明。**

## 0. 给下一位 Agent 的核心指令（优先读）

**停止继续追逐原版 Grok MCP 的零散条件编译。** 上一阶段在一个子目标上投入过多，33 个提交中已跨越 155 个文件，却没有完成最新 SHA 的一次真实编译验证。下一步必须从总体目标、实测依赖闭包与真实编译瓶颈出发，先验证再选择要做的改动；不是再开始 S08a-16、17、18 的 MCP 函数门控循环。

**真正目标：把 grok-pi 构建成足够薄的 Pi RPC + 原生 Pager 产品，同时保持原版 Grok 的默认能力。** 速度收益必须以实际编译 wall-clock、Cargo compilation units、target-specific dependency tree、真实功能回归为证据。

**本交接不授权直接运行 Cargo 或触发 CI。** 前续明确要求不要每刀 Cargo/CI；所有修改已积累成批，应向现有执行约束对齐，在允许集中验收时一次性运行现有手动工作流。不要通过静态检查冒称代码已可编译。

## 1. 产品唯一所有权与必须保持的能力

| 子系统 | 正确所有者 | 不允许的退化 |
|---|---|---|
| Agent loop、Provider、工具、扩展、上下文压缩、会话 | 系统安装的 Pi（通过 JSONL RPC） | 回到原版 Grok Agent 或再写第二套 agent loop |
| 用户可见 UI | 原生 Grok Pager/TUI | 用另一套 TUI 替换；丢掉 Diff、Slash、滚动、Dashboard、树、QuestionView |
| 编排与运行态 | grok-pi、Pi 扩展 | 破坏 Herdr 默认桥接、Remote TUI、Subagents、Goal、Fork/Dashboard |
| Pi Eval MCP | **独立的 Pi Eval v2 daemon / extension** | 误删或误判成需要拆除的「原版 Grok MCP」 |
| Workflow | 既有单套 Workflow 实现 | **用户明确要求本轮先不拆 Workflow**，不得另写第二套或迁走 |
| 本地配置与安全 | grok-pi 的目录和显式 sandbox-enforce | 退回 stock ~/.grok；因为 no-default-features 失去 sandbox |
| 原版 Grok | stock 默认构建 | 默认功能丢失；用运行时开关冒充编译期隔离 |

Pi 源码和 Workflow Host 的业务实现不作为本批拆除范围。重构遵循「简单、单一所有者、不复制业务逻辑、不增加假 MCP stub」。

## 2. 真实基线与证据（必须区分）

| 对象 | 证据 / 数值 | 可得出的结论 |
|---|---|---|
| 历史 ARM64 基线 | [CI #43722291205](https://github.com/Dwsy/grok-pi-tui/actions/runs/43722291205)：约 3013.8 秒、1399 compilation units；未恢复 Rust 缓存 | **只有历史参考值**；不能直接与不同缓存条件的构建相减宣称优化 |
| S01 jemalloc | [CI #43792742800](https://github.com/Dwsy/grok-pi-tui/actions/runs/43792742800)：SHA f1b14cd4，成功，约 2848 秒 | Pi no-default-features、sandbox-enforce、ARM64 release build 与基本 CLI smoke 有成功证据 |
| S02 cryptify | [CI #43798146593](https://github.com/Dwsy/grok-pi-tui/actions/runs/43798146593)：SHA ed616ce9，成功 | S01/S02 在该 SHA 有真实通过；**后续改动均不能沿用它当通过证明** |
| 当前分支 | HEAD 39beb84c，git status 干净，已推送 origin | 代码/文档存在且远端同步；**不证明可编译、运行或提速** |
| 后续 S03–S08a-15 | rustfmt、git diff、manifest/feature 静态审查；部分测试仅写入代码 | 大部分仍标记为「源码已提交，待集中编译及运行验证」 |
| 历史热点 | Shell 约 1612 秒、Pager 约 1517 秒；PDF 约 462 秒、AWS S3 约 332 秒、jemalloc-sys 约 174 秒 | 并行编译任务时间重叠，**不允许相加计算节省量** |

完整来源：同目录「20261116-grok-pi-Pi-only编译闭包拆除路线图.md」及「20261116-grok-pi-逐模块编译瘦身.md」。

## 3. 本分支客观规模

基准提交 **5b679311** → 交接 HEAD **39beb84c**：

- 33 个提交，155 个变动文件，净差异为 **2551 行新增、1045 行删除**。
- 其中 152 个非 docs 文件（2167 行新增 / 1045 行删除），3 个 docs 文件（384 行新增）；不是 155 个互不相关的模块。
- 新增了大量条件编译门控、测试及共享类型重新归属。它们可能有价值，但越大的改动批次越需要及时编译门槛。
- 已有若干性能研究与构建实验文档在基准之前产生；本次不能拿它们的运行结果当 HEAD 的性能证明。

**过程错误**：长期按「修改了多少个 MCP 引用」推进，忘记了「Pi 目标依赖闭包是否真的缩小、整个目标是否可以编译、核心功能是否回归」。这让可维护性与回归风险上升，收益证据却没有同步增加。后续停止这种工作方式。

## 4. 已提交改动：按产品价值而不是按 MCP 轮次

| 类别 | 涉及阶段 | 真实完成状态 | 下一次验收重点 |
|---|---|---|---|
| Pi 构建开关 | S01–S02 | **成功 CI 证据**：stock jemalloc 和 cryptify 在 Pi 模式可排除 | 当前 HEAD 的依赖树是否仍保持 |
| Render / Diff 轻量依赖 | S03–S04 | Render、Diff 改为使用图像/终端辅助库与 xai-tool-types | Rust 编译、Diff wire 类型/JSON、UI 回归 |
| Stock PDF / S3 / GCS | S05–S07 | stock 特性开关和依赖改向，Pi 配置不应启用这些 backends | 最终真实 Cargo tree 无 pdf_oxide、aws-sdk-s3、gcloud-storage |
| 类型逆依赖治理 | S08a-1～8 | ToolKind、TaskKind、认证、Memory、ClientType、OAuth DTO、工具别名等归于现有轻量包 | Stock/Pi 的 serde、类型身份、re-export、测试 |
| 原版 Grok MCP 边界 | S08a-9～15 | Workspace/Shell 的 stock-mcp 可选依赖与 cfg 边界，ACP Session/Agent/命令/测试大量改动 | **Shell 无 MCP 和 stock 默认模式均必须真实编译与回归** |
| Pager external 与 stock Agent 解耦 | S08b | **未实施** | 需首先衡量 Shell/Pager 整体是否值得解耦、能否不碰 Workflow |
| Workflow Host 解耦 | S08c | **按用户决策暂停** | 不拆、不复制实现 |
| 集中验收与性能 | S09 | **未完成** | Cargo tree、check/test、ARM64 Pi binary、smoke、回归、冷/热对照 |

曾错误删除 Pi Eval MCP 的 7210bc13 / 0a253bfd 已由 **f59deb0f** 恢复。不能重演。此后原版 Grok MCP 的目的只是移除 stock transport/runtime 的 Pi 编译依赖，不是破坏 Pi 自己的 Eval 功能。

## 5. 当前架构的真正主要问题

真实 Manifest 关系（交接时检查）：

    grok-pi [xai-grok-pager-bin]
      ├─ pi-grok-adapter ────── xai-grok-shell
      │   └─ xai-workflow      （单套 Workflow，用户要求不拆）
      ├─ xai-grok-pager ────── xai-grok-shell
      │   └─ xai-grok-tools
      ├─ xai-grok-shell ─────── xai-grok-tools
      │   └─ xai-workflow
      └─ xai-grok-workspace
          （stock-mcp 可选边界）

解释：

1. **Shell 与 Pager 依旧在 Pi 的正常编译闭包内。** 原版 Grok MCP 去依赖即使成功，亦不等于消掉历史最大耗时的 Shell 或 Pager。
2. **Pager 依旧直接依赖 Tools。** Render/Diff 不再直接引用 Tools 只能缩短某些路径，不能声称整个应用不编译 Tools。
3. **Pi Adapter 依旧依赖 Shell 的 Workflow/Session/Persistence API。** 在「Workflow 不拆」约束下，不要贸然承诺能够完全移除 Shell；必须清楚写出此架构取舍。
4. **stock-mcp 是已切分的依赖路径，不是已验收产物。** S08a-12 的自制 Python 静态 feature 图显示 Pi 配置不达 xai-grok-mcp、stock 默认可达，然而没有同 SHA 的 Cargo tree / rustc 实证。
5. **原版 Grok 的默认 feature 必须完整保留。** Shell/Workspace 默认 stock-mcp，Pi 通过 --no-default-features、release-dist、sandbox-enforce 选择路径；任何其它默认特性重新统一都可能拉回大依赖。
6. **MCP 条件编译复杂度已经偏高。** 如果下一次编译暴露许多破碎调用，优先检讨边界是否选错，宁可合并/回退某段无法维护的条件编译，也不要继续加几十个重复门控。

**新的主要架构问题不再是「还有哪个 MCP 函数没加 cfg」，而是「Pi-only 闭包里到底还有哪些真正昂贵、又可被安全拆掉的模块」。**

## 6. 下一 Agent 任务排序（必须按顺序执行）

### P0：冻结无效迭代，核对事实

- 在指定 worktree 运行 git status、确认 HEAD、对照本文件和最新路线图。
- 不要修改 Workflow、Pi 原始源码，不要重新删除 Pi Eval MCP。
- 不要开始新一轮 S08a-16 MCP 局部代码清理；先验证已有 33 个提交。
- 复审 CI 是否在当前 feature 组合下检查了 **正确 bin/target**，防止只检查 Workspace/Shell 就误报整个 grok-pi。

### P0：集中验证（遵循现有禁止逐刀 Cargo/CI 的约束）

只在获准进行这一批的统一构建验证时触发手动工作流，不要每次改一行跑 CI。按真实结果修复**最小编译错误**，不是无目的继续重构。

建议使用当前已有文件：

    .github/workflows/pi-slim-ci.yml

对应手动 dispatch（**此文档没有执行**）：

    gh workflow run pi-slim-ci.yml --repo Dwsy/grok-pi-tui --ref refactor/pi-slim-20261116

如果改在本机执行集中检查，严格使用 **相同 target、feature 组合**（以下仅提供命令，交接过程中**没有运行**）：

    cargo tree --locked -p xai-grok-pager-bin --no-default-features --features release-dist,sandbox-enforce --target aarch64-apple-darwin -e normal,build
    cargo tree --locked -p xai-grok-shell --no-default-features --target aarch64-apple-darwin -e normal,build
    cargo check --locked -p xai-grok-shell --lib --no-default-features --target aarch64-apple-darwin
    cargo build --locked --timings --profile release-dist -p xai-grok-pager-bin --bin grok-pi --no-default-features --features release-dist,sandbox-enforce --target aarch64-apple-darwin

要求依序核对：

1. Pi target-specific Cargo tree：jemalloc、cryptify、pdf_oxide、AWS S3 SDK、GCS、xai-grok-mcp、rmcp 等不在正常/构建依赖闭包内。
2. Stock 正向 Cargo tree：stock Grok 默认模式仍包含其 MCP runtime，stock CLI 功能未被破坏。
3. 无 stock-mcp 的 Workspace 和 Shell 的实际 cargo check/test（工作流中已有相应检查）。
4. **整条 Pi ARM64 release-dist binary** 编译、版本/帮助/capabilities smoke、Pi RPC 与原生 TUI 的关键流程。
5. 出具本次 **真实 Run URL、SHA、结果**；一旦失败就记录第一个明确 compiler error 和最小修复提交，不用发散成下一个大型架构项目。

如果发现负向依赖断言通过但整体编译失败，准确记录为「依赖选择正确，源码条件编译不闭合」，而不是宣称已经成功拆除。

### P1：验证核心产品不回归

至少检查 Pi JSONL RPC/ACP 事件、会话恢复与 get_tree、原生 Pager 的渲染/滚动/Slash/工具流式 Diff、Herdr 事件桥接、Remote TUI、Goal/Subagents、Dashboard、Pi Eval MCP、单套 Workflow、sandbox-enforce。

Stock 默认构建至少验证 MCP CLI/ACP initialization 和配置更新路径，不允许 Pi 瘦身单方面破坏原版 Grok。

### P1：重新定量衡量收益

在同类型 Apple Silicon runner、相同 target/profile、相同缓存策略下记录：

- Wall-clock：总 release-dist build 秒数，必须注明冷缓存/热缓存。
- Cargo compilation units：真实 Cargo timing 数据，不把多个并行 crate 时间相加。
- Dependency tree：新增/移除 crate 数量及关键大型依赖的证据。
- Binary size / smoke：能运行并保持产品功能。

历史基线 3013.8 秒、1399 units 是参考，不允许拿缓存不同的单次 run 声称优化百分比。

### P2：验证后再裁决后续架构

仅在确认当前可编译、且知道实际时间热点后，评估三个选择：

- **最小变更**：如果保留 Shell + Pager 已满足性能目标，就停止拆分，清理重复 cfg、合并文档、完成回归。
- **有限 Pager stock/external 分层**：如果 Pager 确实是剩余热点，优先以现有类型与 feature 门控隔离 stock agent 入口，不复制 UI 或引入全新 Agent Core；检查是否能在不动 Workflow 下获得可观收益。
- **Shell 仍重但无法不动 Workflow**：诚实报告约束冲突并停止激进拆分，不要偷拆 Workflow、不写第二套 Workflow。由用户另行决定未来范围。

**禁止**未经真实证据直接以「移除全部 Shell」或「继续追杀 MCP」作为下一阶段目标。

## 7. 最小验收矩阵

| 检查 | 必须给出的证据 | 交接时状态 |
|---|---|---|
| Git 可复现 | HEAD、干净状态、commit 链接 | **已有**：39beb84c |
| Pi feature 负向 tree | 目标 ARM64 Cargo tree 输出 | **未完成**（只有部分旧 CI 和静态图） |
| Stock feature 正向 tree | 同 SHA 的 Cargo tree | **未完成** |
| Shell/Workspace 两模式编译 | check/test 的 exit code、日志 | **未完成** |
| ARM64 grok-pi 编译 | 同 SHA 实际 build 结果 | **未完成** |
| Pi RPC/Herdr/TUI/Workflow/Eval 回归 | 真实进程输出、最小 smoke 记录 | **未完成** |
| 性能提升 | 同 runner、同缓存策略的前后比较 | **未完成** |
| 完整架构裁决 | 保留/回退/继续的证据和成本 | **本报告给出方向，最终等待验收** |

## 8. 交接所需文件与提交

- 本文件：20261116-grok-pi-Pi-only瘦身-整体复盘与下一Agent交接-HANDOFF.md
- 技术路线图：20261116-grok-pi-Pi-only编译闭包拆除路线图.md
- 原版 MCP 深入记录：20261116-grok-pi-原版GrokMCP拆除进度报告.md（只能作为一个子问题的历史记录）
- 初期实验证据：20261116-grok-pi-逐模块编译瘦身.md
- 手动 CI：.github/workflows/pi-slim-ci.yml（workflow_dispatch，当前版本未执行）
- 基准 SHA：5b679311；最近完成验证的 SHA：ed616ce9；交接 HEAD：39beb84c
- 当前分支：https://github.com/Dwsy/grok-pi-tui/tree/refactor/pi-slim-20261116

## 9. 给下一 Agent 的启动提示

> 你接手的是 **grok-pi Pi-only 编译闭包瘦身**，而不是「继续删除 Grok MCP」。首先核对 39beb84c 的工作树和本交接报告。已积累 33 个提交、155 个文件变动，但除较早 S01/S02 外仍未在新 SHA 上通过 ARM64 编译。请首先建立统一验证证据，在被允许的集中 CI 批次中检查真实依赖树和编译/回归结果；所有问题按 first-error 和最小修复组织。之后根据 Shell/Pager 的真实耗时决定要继续分层还是停止。严格保留 Pi RPC、原生 TUI、Herdr、Remote TUI、Goal、Dashboard、Subagents、Pi Eval MCP、单套 Workflow 和 sandbox-enforce；Workflow 本轮绝不拆。禁止重复「每次发现一个 MCP 引用就继续加 cfg」的迭代模式。输出真实 SHA、CI Run、编译树、性能结果和保留/回退/下一步裁决。不要把静态 Cargo manifest 推演当成构建成功。

---

**一句话交接**：目前收获了一批有价值的依赖边界重构，但验证严重滞后，且曾在 MCP 子目标上过度投入。下一位 Agent 的第一职责是**恢复完整构建反馈环，证明或否定已有改动的产品价值**，而不是继续扩大改动面。
