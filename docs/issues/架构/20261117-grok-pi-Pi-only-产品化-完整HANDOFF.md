# grok-pi Pi-only 瘦身与产品化——完整 Handoff

> **交接时间**：2026-11-17 08:35（Asia/Shanghai，UTC+8；本次接手前须刷新实时状态）
>
> **类型**：可直接交由下一位编码 Agent 独立继续的执行级交接，不是发布完成声明。
>
> **代码审计基准 SHA**：`a02f4673`（新增此文档后 Git HEAD 会变化；此处特指最后一次代码变更）。
>
> **分支**：`refactor/pi-slim-20261116`；远端 `origin/refactor/pi-slim-20261116`。
>
> **仓库**：https://github.com/Dwsy/grok-pi-tui
>
> **隔离工作树**：`/Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116`
>
> **原主工程目录**：`/Users/dengwenyu/Dev/AI/pi-grok-build`。**所有本批修改应继续在上述隔离工作树进行，禁止误改主工作树。**
>
> **执行设备**：用户 Mac（Apple Silicon / macOS ARM64）；Nexus node ID：`node_d47b2d161027be931e2d590c0a426c9f`；Rust 1.97.0；Pi CLI 1.5.2；Herdr 0.9.5（以前验证时）。
>
> **最近确认**：代码分支干净、已推送；Intel→ARM64 CI 成功；原生 ARM64 CI 在一条 Shell 测试发生 stack overflow；实验性 `pi-slim` profile 构建成功但**未做完整二进制回归**。本机已安装的 `pig` 仍是此前验证过的稳定试用构建。

---

## 0. 给下一位 Agent：必须先读的结论

**真正目标**：把 grok-pi 收敛为 **Pi Agent（JSONL RPC）+ Grok 原生 Pager/TUI + 必要桥接** 的高品质 macOS ARM64 产品。既要减少编译和维护成本，也要保留真实使用功能。用户已授权自主拆分、测试、提交、推送；不要无意义地反复询问。

**立即处理的 P0**：修复原生 CI `#43981067595` 中确凿的测试栈溢出：

- 失败步骤：`Check stock-MCP-free Shell and test fixtures`。
- 用例：`session::acp_session::auth_retry_budget_tests::api_5xx_during_park_does_not_unpark_or_redispatch`。
- 位置：`crates/codegen/xai-grok-shell/src/session/acp_session_tests/turn/auth_retry_budget_tests.rs:718`。
- 日志原义：`thread ... has overflowed its stack`，随后 `fatal runtime error: stack overflow, aborting`，测试二进制 `SIGABRT`，CI exit 101。
- **这是测试运行期失败，而不是 rustc 编译失败；不得把它归结为原版 Grok MCP。**

**不要重复前一阶段的错误**：原版 Grok MCP 从 Pi-only 编译闭包移除已经完成并经编译证明；**停止继续追逐原版 MCP 的零碎 cfg**。不要误删 **Pi Eval MCP**。**Workflow 暂不拆**（用户明确决定）。不用复制 Agent loop、工具、TUI、Workflow；不引入长期维护的空桩、假接口、兼容层。以第一性原理、单一所有者、低复杂度为先。

**保护功能**：Pi JSONL RPC、原生 Pager、Herdr 默认集成、Remote TUI、Goal 会话隔离/重启恢复、Dashboard、Fork、Subagents、Diff、Slash、QuestionView、Pi Eval MCP、sandbox-enforce、原版 stock Grok 默认路径。

**严格区分证据**：`cargo check --tests` ≠ 测试实际运行；`cargo tree` ≠ 可执行产物；模拟 Herdr Socket ≠ 真实 Pane 完整接管；CI 提交 ≠ CI 通过；本机热构建 ≠ GitHub 冷构建；某个 crate 单元时间下降 ≠ 全程序冷构建按同样比例提速。

---

## 1. 最新可核实状态（交接时的快照）

| 对象 | 真实状态 | 证据 / 限制 |
|---|---|---|
| Git | `refactor/pi-slim-20261116`，基准 SHA `a02f4673`，已推送；取状态时仅文档新增之前工作树干净 | `git status -sb`、`git log` |
| Intel→ARM64 CI | **SUCCESS**；Run `43978491852`；SHA `8dc5c087` | https://github.com/Dwsy/grok-pi-tui/actions/runs/43978491852；仅可证明交叉编译、检查等步骤成功，**不能证明 ARM64 代码已在该 runner 上运行** |
| 原生 ARM64 CI | **FAILURE**；Run `43981067595`；SHA `86085c96` | https://github.com/Dwsy/grok-pi-tui/actions/runs/43981067595；在 Shell 实际单测运行期 stack overflow；未运行后续 release build / Adapter tests / 产包步骤 |
| 上一次成功的旧 CI | `43877602002`，SHA `d7489b42`，成功 | 早于 Herdr/Goal/候选包新增改动，不能当做最新 HEAD 的完整通过证明 |
| 原生 runner 调度 | 旧 `macos-15` Run `43903293892` 未分配 runner，`runner_id=0`，无执行步骤；改用 `macos-26` 后成功进到真实编译 | 两种情况必须严格区分 |
| Pi-only 依赖闭包 | **917 个包**；stock：**1016**；减少 **99**（约 **9.7%**） | 选中 target、`normal,build` 依赖包计数，不等于编译时间/可执行大小 |
| 本机完整 ARM64 `release-dist` | 曾**成功**链接；`86,683,320 bytes ≈ 82.7 MiB`；`--version`/`--help`/`--print-capabilities` 均可运行 | 使用了优化覆盖 `LTO=off` / `CGU=16` / `debug=0`，不是 stock 默认 profile |
| 本机 `pi-grok-adapter` Rust 单测 | **212 passed、0 failed**，至少两次本机实际执行 | 包括 Pi RPC、Goal、Remote TUI、会话、工具投影；CI 原生全套未执行到该步骤 |
| 本机原生 Pager + Mock Pi RPC | **通过**；真实 macOS PTY、原生 ARM64 二进制 | `python3 scripts/pi-native-pty-smoke.py` |
| 真实 Pi CLI + 内置 Herdr 扩展 + 隔离 Unix Socket | **通过** session/idle 上报；未向模型发送请求 | `python3 scripts/pi-herdr-pty-smoke.py`，不是 Herdr Pane 完整验收 |
| Herdr Node 生命周期测试 | **通过**；覆盖 `idle → working → blocked → working → idle`，快速事件及断连背压 | `node --test extensions/pi-grok-herdr/lifecycle.test.mjs` |
| 实际 Herdr Pane 权威 | 历史只读快照：5 个 Pi Pane 全部有 `herdr:pi` / idle，**仅 1 个**获 `full_lifecycle_hook_authority`，4 个仍 fallback | 是 2026-11-16 的历史快照，**不能当作当前实时结果** |
| 本机安装的 `pig` | `~/.local/bin/pig → ~/.local/bin/grok-pi`；显示 `0.0.0-slim-local [beta]` | 已与本机 `release-dist/grok-pi` 的 SHA-256 对照一致，**不等于当前源码 HEAD 已安装** |
| Pi-only 实验 profile | `cargo build --profile pi-slim -p xai-grok-tools` **成功，30m20s、736 units** | 本机首次独立 profile，仅构建 Tools crate；**未生成并验收完整 Pager 可执行程序** |

> **重要版本区别**：原生 CI 失败在 `86085c96`；Intel CI 成功在 `8dc5c087`；最近代码 SHA `a02f4673` 比两者新，主要新增测量脚本、runner CI 配置和**默认不启用**的 profile。不要把旧 SHA 的通过当成当前 SHA 完成全链路验证。

## 2. 正确架构与所有权（不允许第二套内核）

```text
用户输入 / 原生 macOS Terminal
        │
        ▼
grok-pi（xai-grok-pager-bin）
        │
        ├── xai-grok-pager：唯一用户可见 TUI（Diff / Slash / Dashboard / Fork / Goal UI）
        │       ├── xai-grok-shell：存量 Session / 配置 / Workflow 接口
        │       └── xai-grok-tools：Pager/Stock 共用工具类型与部分实现
        │
        ├── pi-grok-adapter：ACP ↔ Pi JSONL RPC；会话、工具状态投影、Goal、
        │                      远程 TUI 的必要适配
        │
        ├── extensions/pi-grok-herdr：Pi 生命周期 → Herdr Unix Socket
        │
        └── Pi CLI（外部进程 / 真正的 Agent loop 与工具执行所有者）
                 ├── 模型、上下文、工具、会话、扩展
                 ├── Pi Eval MCP（独立的 Pi 特性，保留）
                 └── 单套 Workflow Host（目前保持不拆）
```

- **外部 Pi 是唯一 Agent runtime**，不是再用 stock Grok agent loop 执行同一套工具。
- **TUI 是 Grok Pager**，不能为了拆包用新框架替代；UI 仍复用若干 stock 类型/实现。
- **原版 Grok 默认构建必须能正常使用原版 MCP**；只在 Pi-only 特性闭包移除 stock transport。
- `xai-grok-shell`、`xai-grok-tools`、`xai-grok-workspace` 仍是 Pi 真实编译闭包，不可因原版 MCP 已剔除便误报“已完全解耦”。
- 用户要求简单、第一性原理、无无意义兼容包袱；共享类型应有唯一所有者，不能复制代码绕过依赖。

## 3. 已完成阶段回顾（不再按 MCP 文件数作为进度）

| 阶段 / 类别 | 完成的工作 | 验证范围 |
|---|---|---|
| S01–S02 | Pi-only 剔除 stock jemalloc、cryptify | 历史 CI 验证，当前依赖闭包无相关包 |
| S03–S04 | Render / Diff 不再直接依赖笨重的 Tools；共享类型归轻量 crate | Pi-only 编译及共享契约验证 |
| S05–S07 | Stock PDF、S3/GCS 等外部能力只留 stock 侧；Pi-only 不拉入 `pdf_oxide` / AWS S3 / GCS | Cargo tree 证据 |
| S08a | ToolKind 等类型所有权下沉；Shell/Workspace stock-mcp 可选边界 | 本机 Pi-only 和 stock 默认 Shell check 通过 |
| S08b | 进一步把大型 Pager 与 stock Shell/Tools 完全切开 | **未实施，不能假装完成**；有大量直接 API 引用 |
| S08c | Workflow Host 拆分 | **用户要求暂停，不能擅自实施** |
| S09 | 完整原生 ARM64 release 链接 + Pi mock RPC TUI smoke；Shell / Pager tests 可编译 | 本机已通过；真实 Shell 全量运行有新失败 |
| Herdr | 内置 bridge、隔离 Socket、会话/idle、状态测试、快速切换 FIFO、队列上限 128 | Node/PTY smoke 通过；真实 Pane Authority 未全覆盖 |
| Goal | 删除无会话约束的无用 `GoalHost::load`，统一用 `load_for_session` | 本机 Adapter 全部 212 单测通过 |
| CI 候选包 | 原生 ARM64 才可上传附 SHA 的非正式发行候选包；Intel 不执行 ARM64 | Workflow + 本机手工打包演练通过；GitHub **尚未成功上传最新候选包** |
| 性能 | 新增 Cargo timing 解析脚本、可选 Pi-only 优化 profile | 详见第 5 节；完整 binary A/B 尚未完成 |

此前 S08a 曾过度投入原版 MCP cfg，历史约 33 个提交触及 155 文件；这一过程已经在旧 Handoff 中复盘。**不要重启“S08a-16 继续追 MCP”的循环。**

## 4. 最高优先级：修复 ARM64 原生 CI 的真实失败

**失败 Run**：https://github.com/Dwsy/grok-pi-tui/actions/runs/43981067595

**复盘到的精确事实**：

- Node Herdr 生命周期检查、依赖闭包断言、Workspace 检查等已经越过。
- 第 12 步 `Check stock-MCP-free Shell and test fixtures` 在原生 ARM64 上执行：
  ```bash
  cargo test --locked --lib -p xai-grok-shell \
    --no-default-features --features test-support \
    --target aarch64-apple-darwin
  ```
- 已打印大量 `test ... ok`，随后准确崩溃用例：
  ```text
  thread 'session::acp_session::auth_retry_budget_tests::api_5xx_during_park_does_not_unpark_or_redispatch' (...) has overflowed its stack
  fatal runtime error: stack overflow, aborting
  process didn't exit successfully: .../xai_grok_shell-... (signal: 6, SIGABRT)
  ```
- 该用例源码在 `crates/codegen/xai-grok-shell/src/session/acp_session_tests/turn/auth_retry_budget_tests.rs:718`，是 `#[tokio::test(flavor = "current_thread", start_paused = true)]`；模拟请求脚本含一次 401、四次 502，使用 `LocalSet` / park / token hot-swap / retry。
- 一整套 Shell 单测**尚未通过**；不应通过 `ignore`、删除用例、无视 panic 或只跑 `cargo check` 掩盖。

**下一位 Agent 优先执行**（单条测试隔离运行，避免全量重编和无关失败）：

```bash
cd /Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116

cargo test --locked --lib -p xai-grok-shell \
  --no-default-features --features test-support \
  --target aarch64-apple-darwin \
  session::acp_session::auth_retry_budget_tests::api_5xx_during_park_does_not_unpark_or_redispatch \
  -- --exact --nocapture --test-threads=1
```

先复现、记录退出码与栈信息；如果本机默认测试栈也溢出，可**仅作为诊断**提高 `RUST_MIN_STACK` 重跑，判断是否深层递归、巨型栈对象或环境栈限额差异；**提高环境变量不是未经分析即可接受的修复方案**。修复后运行相邻 auth_retry 与 Shell 全量测试，再核对 stock 默认路径，最后在新 SHA 重新派发原生 CI。不要取消已经获得 runner 且无回归的 CI 来反复重排。

日志在本机 `/tmp/pi-slim-ci-43981067595-failed.log`，或执行：

```bash
gh run view 43981067595 --repo Dwsy/grok-pi-tui --log-failed
```

## 5. 构建性能：已取得的真实量化结果

### 5.1 旧的完整 Pi ARM64 冷构建

- Cargo timing：`target/cargo-timings/cargo-timing-20261116T115706944Z-2e1ebf7872888d2d.html`。
- 整体墙钟 **4054 秒**（约 67 分 34 秒）、**1288 units**，0 fresh units；最大并发约 7。
- 测试目标 `aarch64-apple-darwin`，`release-dist` 使用 `LTO=off`、`codegen-units=16`、`debug=0` 的 override。

| crate | Total | Frontend | Codegen |
|---|---:|---:|---:|
| `xai-grok-shell` | 1236.31s | 521.78s | 714.53s |
| `xai-grok-tools` | 1219.82s | 230.47s | **989.35s** |
| `xai-grok-workspace` | 959.20s | 359.35s | 599.85s |
| `xai-grok-pager` | 885.68s | 436.87s | 448.81s |
| `aws-lc-sys` build script | 495.25s | — | — |
| `libgit2-sys` build script | 383.95s | — | — |

任务互有重叠，不能把各 crate 时间相加。`rhai` 来自需保留的 Workflow，Mermaid compiler 是 Pager 的真实产品能力路径；不要盲删。

### 5.2 独立的 `pi-slim` Tools 优化实验——**已编译成功但尚未产品验收**

- 提交 `a02f4673` 在根 `Cargo.toml` 添加**非默认** profile `[profile.pi-slim]`：继承 release-dist、`lto=false`、`codegen-units=16`、`debug=0`、`strip=false`；只对 `[profile.pi-slim.package.xai-grok-tools]` 设 `opt-level=1`。
- 实验命令：
  ```bash
  cargo build --locked --profile pi-slim \
    -p xai-grok-tools --no-default-features \
    --target aarch64-apple-darwin -j 6 --timings
  ```
- **成功**，首次独立 profile 构建 **1821 秒（30m20s）/ 736 units**。这是工具库及依赖的构建，**不是完整 grok-pi 构建**。
- timing：`target/cargo-timings/cargo-timing-20261116T232626361Z-2e1ebf7872888d2d.html`。
- 新实验 `xai-grok-tools`：**479.94s total = 129.14s frontend + 350.80s codegen**。
- 旧基线 `xai-grok-tools` codegen **989.35s**，与实验 **350.80s** 相比约减少 **64.5%**；总单元耗时约减少 **60.7%**。**只是跨构建范围/profile 的 crate 粗对照，不是受控 A/B 的因果结论，更不能写作“Pi 构建加速 64.5%”。**
- 仍缺：完整 `grok-pi --profile pi-slim` 原生二进制链接、体积与启动、Pi RPC TUI Smoke、Herdr、Goal/Remote TUI 回归、与相同机器/构建范围/缓存策略的对照。

解析工具已提交 `scripts/pi-cargo-hotspots.py`：

```bash
python3 scripts/pi-cargo-hotspots.py \
  target/cargo-timings/cargo-timing-20261116T115706944Z-2e1ebf7872888d2d.html \
  --focus xai-grok-tools

python3 scripts/pi-cargo-hotspots.py \
  target/cargo-timings/cargo-timing-20261116T232626361Z-2e1ebf7872888d2d.html \
  --focus xai-grok-tools
```

**裁决**：先让 opt-in profile 保持非默认。验证完整产品后再决定是否启用 CI；不要为了“看起来变快”复制整套工具或大量加 feature gate。Stock `release-dist` 保持不变。

## 6. CI 实施方式和下一次重新派发

配置：`.github/workflows/pi-slim-ci.yml`（`workflow_dispatch`，**非正式 Release**）。

- runner 选项：`macos-15-intel`（Intel 交叉编译）、`macos-15`（Apple Silicon）、`macos-26`（Apple Silicon）。
- 目标全部 `aarch64-apple-darwin`。Intel 只编译/check，不执行 ARM64 程序，也**不上传 ARM64 候选安装包**。
- 并发组按分支与 runner 分离，Intel 不应阻塞原生 ARM64。
- 原生 CI 使用 `CARGO_BUILD_JOBS=2`、`CARGO_PROFILE_TEST_DEBUG=0` 控制 GH-hosted ARM64 资源；`release-dist` 使用 `CGU=16`、`LTO=off`、`DEBUG=0`、`STRIP=none` 覆盖。
- 原生全流程包含：Herdr Node 生命周期单测、Pi/stock 依赖闭包、Workspace、Shell **实际 lib tests**、发布级链接、CLI smoke、原生 Pager PTY/RPC、Pi Adapter Rust 全套测试、Diff/Render/Tools contracts、归档 SHA-256、Actions artifact 上传。
- 原生候选 artifact 为 `pi-slim-macos-arm64-<full SHA>`，内含 `grok-pi-macos-arm64-pi-slim-<short SHA>.tar.gz`、`.sha256`、`build-info.txt`，保留期 14 天；不创建 GitHub Release。当前失败的 ARM64 CI artifact 数量 **0**。
- 旧 `release.yml` 与 stock Grok 的正式发布流程不被此测试工作流修改。

**下一次修复后运行**：

```bash
gh workflow run pi-slim-ci.yml \
  --repo Dwsy/grok-pi-tui \
  --ref refactor/pi-slim-20261116 \
  -f runner=macos-26

gh run list --repo Dwsy/grok-pi-tui \
  --workflow pi-slim-ci.yml --limit 5
```

**只有产包 CI 完成 SUCCESS 并出现 artifact 后**，按 `docs/usage/pi-slim-candidate.zh-CN.md` 进行 SHA 校验、解包、本地二进制执行与试用替换。不能把本机成功编译等同于 CI 完成，也不应贸然创建正式 Beta tag。

## 7. Herdr：已修复什么、还缺什么

**已实现并实际测试**：

1. 内置扩展 `extensions/pi-grok-herdr/index.ts`，真实 Pi CLI JSONL RPC 启动后向隔离的 Unix Socket 发送 `pane.report_agent_session` 和 `pane.report_agent(state=idle)`；`source=herdr:pi`。
2. 实际生命周期 Node 测试覆盖 `idle → working → blocked → working → idle`、`seq` 递增、session-before-state、Welcome 页面预热进程不上报。
3. 修复快速 turn 中原队列将未发送的中间状态全部覆盖的问题：正常 Socket 下 FIFO 保留每次状态变化。
4. Socket 拥塞时队列最多 128 条，仅异常超限时压缩为最新会话与之后的最新状态，避免无限内存增长；快速事件和堵塞测试 Red/Green 通过（重复三次）。
5. Herdr 源码**从未修改**。

**尚未验收**：

- 历史真实 Herdr 0.9.5 Pane 中只有 1/5 `full_lifecycle_hook_authority`，其他 4 个 `default_known_agent_idle_fallback`。老进程不代表当前二进制回归；必须基于**新启动的根 Pi 会话**检查 `herdr agent explain <PANE_ID> --json` 的 `screen_detection_skipped=true` / authority 原因。显示 idle 并不等于权威接管。
- **Herdr 的控制规范要求命令在 `HERDR_ENV=1` 的受管理 Pane 内执行。** 当前 Nexus Exec 外部 shell 检查 `HERDR_ENV=false`；不能从外部擅自创建/关闭/聚焦真实 Pane。应在合法 Herdr 上下文内执行，只操作自己创建的临时 Pane，不能打断用户当前会话。
- Workflow / Pi Agent turn 的 `working` 和 `blocked` 在真实 Pane 的全链路行为仍需产品验收；模拟 Socket 不够。

可运行：

```bash
node --test extensions/pi-grok-herdr/lifecycle.test.mjs
python3 scripts/pi-herdr-pty-smoke.py
```

说明：第二条需要本机已安装的真实 Pi CLI；使用独立临时目录和模型固定参数，不向真实提供商发 prompt。原生 Pager TUI 另运行 `python3 scripts/pi-native-pty-smoke.py`。

## 8. Goal / 远程 TUI / Workflow / stock Grok 的边界

- `pi-grok-adapter` 212 个原生 Rust 单测已通过，含 Goal、Remote TUI、命令、会话隔离、RPC 工具投影等；仍需在**新一轮原生 CI** 确保同一 SHA 的全套执行结果。
- `GoalHost::load` 因没有 session 限制且已无正式调用方被删除；生产统一走 `load_for_session(session_id)`，没有改 Goal 数据协议。
- 禁止把旧 Goal 状态加载回其他 Session；Fork/Resume/Cancel/Session Restart 必须保持会话身份边界。
- Remote TUI、Dashboard、Fork、Subagents、diff/renderer 是保护清单；需要运行测试，不允许通过禁用功能换“瘦身”。
- **Workflow 不拆。** Pi Adapter 使用 Shell 的 Workflow/Session/Persistence 类型，当前是已接受的依赖耦合。不要另写 JS/Rust 第二套工作流。
- **Pi Eval MCP 与 stock Grok MCP 不同。** Pi Eval v2 daemon/extension 必须保留；移除的仅 stock Grok MCP 的 Pi-only transport 依赖。
- Stock 默认版 Shell check 已通过；但最近修复后仍应把 stock 构建列为回归闸门，防止新 feature 边界误删原版能力。

## 9. 已验证的命令：按成本从低到高执行

**A. 工作树与 HEAD**

```bash
cd /Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116
git status -sb
git log -5 --oneline
git rev-parse HEAD
```

**B. 原生轻量回归**

```bash
node --test extensions/pi-grok-herdr/lifecycle.test.mjs
python3 scripts/pi-native-pty-smoke.py
python3 scripts/pi-herdr-pty-smoke.py
```

**C. Pi Adapter 全量 Rust 单测**

```bash
cargo test --locked --lib -p pi-grok-adapter \
  --no-default-features \
  --target aarch64-apple-darwin -j 6
```

**D. Pi-only Cargo tree（不要漏 feature）**

```bash
cargo tree --locked -p xai-grok-pager-bin \
  --no-default-features --features release-dist,sandbox-enforce \
  --target aarch64-apple-darwin -e normal,build

cargo tree --locked -p xai-grok-pager-bin \
  --features stock-mcp \
  --target aarch64-apple-darwin -e normal,build
```

**E. 原生可执行的已验收 build 形式**

```bash
export GROK_PI_VERSION=0.0.0-slim-local
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_RELEASE_DIST_CODEGEN_UNITS=16
export CARGO_PROFILE_RELEASE_DIST_LTO=off
export CARGO_PROFILE_RELEASE_DIST_DEBUG=0
export CARGO_PROFILE_RELEASE_DIST_STRIP=none

cargo build --locked --profile release-dist \
  -p xai-grok-pager-bin --bin grok-pi \
  --no-default-features --features release-dist,sandbox-enforce \
  --target aarch64-apple-darwin -j 6

target/aarch64-apple-darwin/release-dist/grok-pi --version
target/aarch64-apple-darwin/release-dist/grok-pi --print-capabilities
```

注意：上面 release profile override **不是**仓库默认的 `release-dist` 属性（默认 `thin LTO/CGU=1/debug=1`）。

**F. 从真实 CI 查看问题**

```bash
gh run view 43981067595 --repo Dwsy/grok-pi-tui --log-failed
gh run view 43978491852 --repo Dwsy/grok-pi-tui
gh run list --repo Dwsy/grok-pi-tui --workflow pi-slim-ci.yml --limit 6
```

## 10. Mac 已安装版本与回滚安全

当前测试机：

- 安装路径：`/Users/dengwenyu/.local/bin/grok-pi`。
- `pig` 指向它的软链接；不需要两个文件分别安装。
- `grok-pi --version`：`grok-pi 0.0.0-slim-local [beta]`，属于本机**试用标签**，**不代表 GitHub 正式 Beta 版本号**。
- 安装文件与 `target/aarch64-apple-darwin/release-dist/grok-pi` 的 SHA-256 均为：
  `c2904991c477e96ce9436b2405aa219fc1345112aaf54214bc1ebbb5ca547196`。
- 原版正式 Beta 备份：`~/.local/bin/grok-pi.backup-20261116-0.1.14-beta.4`。
- 上一版 Pi-only 备份：`~/.local/bin/grok-pi.backup-pi-slim-before-5d165ae2-20261117T000248`。
- 当前安装的二进制**尚未包含独立 `pi-slim` 优化 profile 的构建结果**（实验没有生成完整 bin）。
- 更新时：**先在临时路径复制+校验 SHA、Mach-O arm64、CLI 版本/能力，再备份目标文件，最后同目录原子替换**；不要覆盖用户已有备份、不要强制关闭正在运行的 grok-pi / Herdr Pane。除非用户明确要求，不要仅为测试而安装未经验证的实验构建。

## 11. 关键证据、日志和参考文件

**当前最新技术报告**：

- `docs/issues/架构/20261116-grok-pi-S09-集中验证与架构裁决.md`：S09 每一步实测、历史证据、CI 门槛、Herdr 修复与 benchmark 方法（`1–15`）。
- `docs/issues/架构/20261116-grok-pi-Pi-only瘦身-整体复盘与下一Agent交接-HANDOFF.md`：较早 S01–S08/架构裁决/过程复盘；**HEAD 和状态已过期，只作历史上下文**。
- `docs/issues/架构/20261116-grok-pi-Pi-only编译闭包拆除路线图.md`、`20261116-grok-pi-逐模块编译瘦身.md`、`20261116-grok-pi-原版GrokMCP拆除进度报告.md`：以前的任务分解和模块结论。
- `docs/usage/grok-pi-herdr.md`、`docs/usage/grok-pi-herdr.zh-CN.md`：真实 Hook authority vs fallback、Welcome 与 session 区别、Socket 诊断。
- `docs/usage/pi-slim-candidate.zh-CN.md`：候选安装包工作流及安全安装说明。

**脚本**：

| 路径 | 用途 |
|---|---|
| `scripts/pi-cargo-hotspots.py` | 解析 Cargo timing HTML，打印逐 crate frontend/codegen |
| `scripts/pi-native-pty-smoke.py` | 原生 Pager ↔ 模拟 Pi RPC 端到端 PTY |
| `scripts/pi-herdr-pty-smoke.py` | 真实 Pi RPC ↔ 内置 Herdr bridge ↔ 隔离本地 Socket |
| `extensions/pi-grok-herdr/lifecycle.test.mjs` | 生命周期序列、快速连发、背压行为测试 |
| `crates/codegen/pi-grok-adapter/tests/mock_pi_rpc.py` | Mock JSONL RPC 响应 |

**本机近期日志（`/tmp` 下可能被清理；不是持久产物）**：

- `/tmp/pi-slim-ci-43981067595-failed.log`：本次原生 ARM64 Shell test SIGABRT（已拉取）。
- `/tmp/pi-slim-release-dist-8ae6d654.log`：67m33s 完整 ARM64 release build。
- `/tmp/pi-slim-adapter-all-212.log` 与 `/tmp/pi-slim-adapter-all-goal-cleanup.log`：真实 212/212 Rust tests。
- `/tmp/pi-slim-herdr-rapid-red.log` / `...green.log`：快速状态丢失 Red/Green。
- `/tmp/pi-herdr-backlog-red.log` / `/tmp/pi-herdr-backlog-green-{1,2,3}.log`：断连积压 Red/Green。
- `/tmp/pi-slim-tools-opt1-build.log`：独立 profile 构建 30m20s。
- `target/cargo-timings/cargo-timing-20261116T115706944Z-2e1ebf7872888d2d.html`：旧全 Pi 冷构建。
- `target/cargo-timings/cargo-timing-20261116T232626361Z-2e1ebf7872888d2d.html`：新 Tools-only profile 构建。

## 12. 下一轮工作顺序（实操任务树）

### P0 — 原生 Shell 测试栈溢出，阻断候选包（先做）

1. 在当前 ARM64 本机用精确 `--exact` 过滤复现问题用例，查巨大局部栈对象 / 递归路径 / Tokio paused-time/park 逻辑。
2. 只修真实根因。**不要通过跳过测试、空桩、盲目放大栈、删除 auth retry 规则来伪造 CI 成功**。
3. 复验该用例与相邻用例；Shell `--lib` 全量实跑；Stock Shell check；Pi Adapter 212 单测。
4. 提交推送后重新派发原生 `macos-26` CI；将新 SHA 和最终状态更新交接文档。

**通过标准**：同一个修复 SHA 的原生 CI 第 12 步真正执行并成功，不能仅凭 Intel check 通过。

### P1 — 全原生 ARM64 产包闭环

1. 修 P0 后完成 `release-dist` 链接、CLI smoke、PTY Pi RPC、Adapter 212、共享契约、Diff/Render 等全部步骤。
2. 确认 Actions artifact 实际存在（文件结构/大小、SHA-256、Git SHA 与目标一致）。
3. 在 Mac 本地校验下载产物的 arm64 格式和功能；对比本机验证版。
4. 仅在全部通过后才决定是否原子替换用户安装的 `pig`；不触碰 Herdr / 已运行的会话。
5. 正式 Beta（单独 tag / release）需要另走原版发布流程，不能把 CI artifact 当正式发行。

### P1 — Herdr 真正完整接管

1. 在受 Herdr 管理的上下文中新开专用临时会话，运行新版根 Pi 会话；禁止改动现有用户 Pane。
2. 验 `session_start` 后 `idle`、新 turn `working`、批准等待 `blocked`、回落 `idle`，同时核对 `agent explain --json` 的 `full_lifecycle_hook_authority`。
3. 若不通过，优先查：Welcome 未进入根会话、旧版进程/扩展、`HERDR_ENV`/Socket/pane id、上报 seq 与 session identity、Herdr acceptance，不修改 Herdr 源码。
4. 写实测证据：接受/不接受的具体状态，不凭 UI 的 idle 猜测。

### P2 — 使用 Profile 数据决定是否省编译

1. 将实验 `pi-slim` profile 编译**完整** `grok-pi`，记录总 wall-clock、unit count、peak RSS、实际二进制大小。
2. 同机/同目标/同依赖缓存策略与对照 profile 做可比较的 A/B；至少多次热构建和一次合理控制的冷构建。
3. 跑 Pi JSONL RPC、Pager TUI、Herdr、Adapter/Goal/Remote TUI；测启动时延/交互响应，不应只追编译速度。
4. 无明显净收益或明显性能回退就不切换默认配置；成功则通过单独提交修改 Pi-only CI，保留 stock release profile。

### P2 — 如仍值得，才做 Tools/Shell/Pager 实质模块瘦身

- 先画所有调用方，再选稳定、高价值的共享契约边界；工具类型、Workflow、Session/GUI 应有单一所有者。
- 以前的调查：Pager 源码约 159 个文件直接引用 Shell、80 个引用 Tools；Voice 功能横跨 Pager 63 个源码文件，贸然禁用只会增加很多 cfg 和维护复杂度。
- 不要拆 Workflow、不删除 Mermaid 正常渲染、不因包数好看而断掉真实依赖。
- 优先跑 `cargo --timings` 定位真正耗时，不要靠主观猜测。

### P3 — 产品级发布与交付

- 候选包从仅内部试用升级为版本化发行前，需要固定版本号、功能清单、跨 Session/故障注入回归、正式发行说明及发布流程评估。
- 用户最终明确 macOS **只要 Apple Silicon ARM64**；不继续构建 Intel 安装包。Intel GitHub runner 只可作为交叉编译验证手段。
- 保留安全回滚包与安装路径，不擅自改用户全局 Pi 配置或权限。

## 13. 交接前/接手后验收清单

- [x] Pi-only 917 vs Stock 1016 依赖包闭包已测。
- [x] Stock Shell 默认编译路径曾成功。
- [x] 本机 ARM64 release-dist 可执行链接成功。
- [x] 本机原生 Pager + Mock Pi RPC PTY 通过。
- [x] 本机真实 Pi CLI + Herdr 隔离 Unix Socket 会话/idle 通过。
- [x] 本机 Herdr Node 快速状态和背压测试通过。
- [x] 本机 Pi Adapter 212/212 Rust 单测通过。
- [x] Intel→ARM64 CI `#43978491852` SUCCESS。
- [x] `pi-slim` Tools-only profile 构建成功并有真实 Cargo timing。
- [x] 测量脚本、候选包流程、Herdr 修复与文档均提交远端。
- [ ] 原生 ARM64 CI Shell **实际全量单测**通过（当前被上述 stack overflow 阻断）。
- [ ] 当前 SHA 原生 `release-dist` 全套 CI 成功且 Actions 候选包真正上传。
- [ ] 新版在真实 Herdr Pane 中达到完整 hook authority。
- [ ] `pi-slim` profile 完整二进制 A/B 性能及功能验收。
- [ ] 经过真实用户试用后的产品级 Beta Release。

---

## 14. 一句话总结和新的 Agent 第一条指令

**主线已从“原版 Grok MCP 清理”转为“原生 ARM64 产品验收 + Tools Codegen 定量优化”**。Pi-only 闭包、原生 TUI/RPC、Herdr 桥接、Adapter 212 单测已有证据。现在阻断实际候选包的最小问题是 **原生 ARM64 Shell Auth Retry 单元测试栈溢出**；先解决它并完成端到端产包，然后做真实 Herdr Pane 验收，最后再决定是否启用已测出较低 Tools codegen 的 opt-in profile。

> **给下一位 Agent 的起步命令**：进入隔离工作树，确认 `git status`，读取 `docs/issues/架构/20261116-grok-pi-S09-集中验证与架构裁决.md`，用第 4 节的精确命令复现 **`api_5xx_during_park_does_not_unpark_or_redispatch`**，只针对这个真实阻断做修复和验证；不要再先开一轮 stock MCP 条件编译大清扫。
