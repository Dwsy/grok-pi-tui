# Grok Native TUI × Pi 验证报告

验证日期：2026-07-17
交付版本：`pi-grok-native-v4.0.0`

## 结论

当前交付已通过生产构建、适配器单元测试，以及原生 Grok 架构与 Pi 协议契约验证。入口确实使用 Grok Build 的生产 Pager，而不是独立 Ratatui/fallback/字符画前端。

尚不能宣称**全部**验证全绿：`verify.sh` 的 Rust 语法阶段依赖未声明的 Python 包 `tree_sitter` / `tree_sitter_rust`，当前环境因缺少该依赖停止；两条 Pager focused lib test 还会在既有跨 crate `#[cfg(test)]` helper 配置上失败（与 Pi adapter 逻辑无关）。尚未新增 `grok-pi` 的 PTY 端到端 smoke test。

## 2026-07-18 子代理适配增量

已新增内置 `pi-grok-subagents` extension：它用官方 Pi extension API 创建、追踪、取消并持久化 child `AgentSession`，通过 `pi-grok-subagent/v1` custom-message bridge 交给 adapter。adapter 仅验证/去重并投影到 Pager 已消费的 `x.ai/session/update` 和带 child session ID 的 ACP `SessionNotification`；Pager 本体继续复用原有 SubagentBlock、Tasks Pane、child AgentView 与取消 UI。

| 验证层 | 结果 | 说明 |
|---|---:|---|
| Pi custom-message bridge probe | PASS | RPC JSONL `message_start`/`message_end` 均保留 `customType`、`display:false` 与结构化 `details`。 |
| Tempfile extension load | PASS | 将 extension 复制到独立 tempfile 后以 `pi --mode rpc --extension <temp>.ts` 加载，隐藏 cancel command 出现在 command catalog。 |
| Adapter unit tests | PASS | `cargo test -p pi-grok-adapter`：53 项通过。 |
| `grok-pi` binary unit tests | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`：7 项通过。 |
| `grok-pi` check | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` 成功；仅既有 `PiModel.reasoning` dead-code warning。 |
| Pager child-route lib test | BLOCKED | 聚焦测试编译被既有无关 Pager test 配置错误阻断：缺 `set_voice_mode_enabled_for_test`、layout 参数漂移、`ActiveModal: Debug`、`AppView` 初始化字段漂移。 |
| 带真实模型的原生 TUI E2E | PENDING | 尚未手工验证 spawn/progress/child view/finish/cancel/resume/replay；不得将静态通过视为运行时验收。 |

## 已执行结果

| 验证层 | 结果 | 说明 |
|---|---:|---|
| 原生 Grok 架构审计 | PASS | `grok-pi` 位于 `xai-grok-pager-bin`，进入 `xai_grok_pager::app::run_external` |
| 自绘/fallback 排除 | PASS | adapter 为 library-only，无 Ratatui/Crossterm/terminal loop；旧 `pi-grok-tui` 不存在 |
| Grok 原生源码完整性 | PASS | 原始树中 2696 个文件保持 SHA-256 一致；仅 19 个声明的组合/ACP/状态/命令接缝变化 |
| Renderer/Input/Markdown 完整性 | PASS | 283 个核心文件与上传的 Grok 源码逐字节一致 |
| Pi RPC 命令契约 | PASS | 适配器使用的 13 个 RPC 命令均存在于包内 Pi `rpc-types.ts` |
| Pi 事件契约 | PASS | 映射的 20 类 lifecycle/stream/tool/queue/compaction/retry/UI 事件均可在 Pi 源码中定位 |
| Extension UI | PASS | Pi RPC 暴露的 9 个方法均有原生 Grok UI 路由 |
| Mock JSONL RPC | PASS | 27 条交互覆盖 bootstrap、history、commands、stream、tool、UI response 与 `agent_settled` |
| Rust tree-sitter 解析 | BLOCKED | `verify.sh` 未声明并预检 `tree_sitter` / `tree_sitter_rust` 依赖，当前环境缺失该模块 |
| Shell 脚本语法 | PASS | `build.sh`、`run-local.sh`、`run-installed.sh`、`verify.sh` 通过 `bash -n` |
| 补丁可应用性 | PASS | 对上传的原始 Grok 树执行 `patch --dry-run -p1`，29 个源码/manifest 文件全部可应用 |
| `cargo check` | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` 成功；仅 adapter 中 1 条既有 dead-code warning |
| Adapter Rust 单元测试 | PASS | `cargo test -p pi-grok-adapter`：17 项通过 |
| `grok-pi` binary 单元测试 | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`：1 项通过 |
| Pager focused lib tests | BLOCKED | 依赖 `xai-grok-pager-render` 的 `#[cfg(test)]` helper；测试依赖未启用 test-support feature，编译报错 |
| 本地 Pi npm build | PASS | `npm run build` 已在 Node.js `v24.15.0` 环境成功执行 |

机器可读报告：

- `crates/codegen/pi-grok-adapter/docs/native-grok-verification.json`
- `crates/codegen/pi-grok-adapter/docs/mock-pi-contract.json`
- `crates/codegen/pi-grok-adapter/docs/rust-syntax-verification.json`
- `verification-logs/cargo-status.json`
- `verification-logs/environment-status.json`
- `verification-logs/patch-status.json`

## 关键架构证据

### 生产 Grok Pager 入口

`crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` 只执行组合工作：

1. 启动 `pi --mode rpc`；
2. 将 Pi JSONL RPC 转为 ACP；
3. 构造 `AcpConnection::external`；
4. 调用 `xai_grok_pager::app::run_external`。

该文件不创建 Ratatui `Terminal`、`Frame` 或 Widget，也不读取 Crossterm input。

### 原生组件复用

`run_external` 继续使用 Grok 的：

- terminal init/restore 与 writer thread；
- production event loop；
- PromptWidget 与键盘输入；
- slash `CommandRegistry`、suggestion/dropdown；
- Markdown/code/diff/tool rendering；
- scrollback、find、copy、transcript、export；
- QuestionView；
- toast、sticky banner、terminal title；
- fullscreen、inline、minimal renderer。

### 修改边界

Grok 侧修改限制为：

- 增加 external ACP connection/profile；
- external backend 的产品功能 gate；
- Pi Extension UI 通知进入现有 Grok surface；
- QuestionView 增加 `initialText`/`noFreeform` 语义提示；
- 动态 Pi command 与被允许的 Grok builtin 合并；
- `/compact <instructions>` 参数透传；
- `grok-pi` composition binary。

Renderer、input engine、Markdown engine、tool renderer 和 minimal renderer 本体没有被重写。

## 在具备工具链的机器上必须执行

要求：

- Rust toolchain `1.92.0`（见 `rust-toolchain.toml`）；
- Node.js `>=22.19.0`；
- npm；
- 可安装 workspace 依赖。

执行：

```bash
./verify.sh
```

或者逐项执行：

```bash
cd grok-build-main
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
./scripts/cargo-shared.sh test -p pi-grok-adapter
./scripts/cargo-shared.sh test -p xai-grok-pager --lib \
  external_builtin_filter_accepts_aliases_and_omits_product_commands
./scripts/cargo-shared.sh test -p xai-grok-pager --lib \
  slash_compact_with_context_enqueues_command
```

再构建完整运行链路：

```bash
cd ..
./build.sh
./run-local.sh /path/to/project --no-session
```

## 运行验收清单

构建成功后，至少手工验证：

1. 画面、PromptWidget、命令下拉、Markdown 和 tool cards 与 Grok Build Pager 一致；
2. `/help` 只显示允许的 Grok 本地命令，并合并 Pi 动态命令；
3. Pi extension `notify`/`setStatus` 不再生成 fallback 文本消息；
4. `select`、`confirm`、`input`、`editor` 使用 Grok QuestionView；
5. `/model` 与 `/effort` 实际修改 Pi model/thinking level；
6. active turn 中普通提交进入 Pi follow-up，send-now 进入 steer；
7. `!command` 使用 Pi `bash` RPC并渲染为 Grok tool card；
8. `/new`、`/compact instructions`、`/rename` 生效；
9. 重启已有 Pi session 时历史、reasoning、图片和工具结果恢复；
10. minimal/fullscreen 通过启动参数选择，终端退出后正确恢复。

## Upstream sync record (98c3b24)

日期：2026-07-17  
分支：`sync/upstream-98c3b24`（尚未 merge 回 `main`）

| 项 | 结果 |
|---|---|
| 上游 tip | `98c3b24`（含 `8adf901`） |
| 策略 | 有共同祖先 `c68e39f` 的 Git merge + 接缝修复，**非**直接在 main 碰运气 merge |
| `pi-grok-adapter` tests | PASS（46） |
| `grok-pi` cargo check | PASS |
| `grok-pi` unit tests | 4/5 PASS；1 项 `--append-system-prompt` 命名漂移为 main 既有失败 |
| 架构不变量 | adapter headless；Pager 唯一 TUI；Pi 唯一 core |

已知仍独立的基础设施 blocker 见上文 `verify.sh` / Pager focused lib tests 段落。
