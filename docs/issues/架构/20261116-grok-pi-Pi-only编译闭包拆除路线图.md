# grok-pi Pi-only 编译闭包拆除路线图

> 2026-11-16。执行分支：`refactor/pi-slim-20261116`；独立 worktree：
> `/Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116`。
> 目标不是让“单个源文件快一点”，而是从 `grok-pi` 的 **Cargo 可达依赖图** 中移除没有使用的原版 Grok 后端。
> 总原则：大胆拆边界，不搞平行 TUI、不复制 Agent Core、不靠复杂的兼容适配层。

## 0. 产品边界（不允许回归）

| 责任 | 唯一归属 | 必须保持 |
| --- | --- | --- |
| Agent loop、模型/Provider、工具执行、扩展、压缩、会话 | **Pi**（系统安装，通过 RPC） | Pi RPC ↔ ACP，错误/重连/会话恢复 |
| 所有可见终端 UI | **原生 Grok Pager** | 对话、Slash、QuestionView、Diff、滚动、主题、树、Dashboard |
| 远程代理与编排 | grok-pi + Pi 扩展 | Herdr 默认集成、Remote TUI、Subagents、Goal |
| 本地状态 | grok-pi | `~/.grok-pi`、`.grok-pi`，不得回退到 stock `~/.grok` |
| 原版 Grok 行为 | stock Grok 原入口 | 不能因瘦身而删除 stock 原有能力，需用清晰可验证的特性边界 |
| 安全 | 显式 opt-in / enforcement | `sandbox-enforce` 不得随 `--no-default-features` 被意外关闭 |

**禁止**：再引入第二套 Agent 会话/工具、再造 TUI；为避免调整代码而“占位”保留巨大依赖；将 UI runtime flag 冒充真正的编译期解耦。可以跨多个相关 Cargo crate **批量修改**，但每个新抽象必须有明确所有者。

## 1. 基线、证据、衡量方式

2026-11-16 参考基线：[Actions 43722291205](https://github.com/Dwsy/grok-pi-tui/actions/runs/43722291205)，ARM64 编译约 **3013.8 s、1399 个 compilation units**，未恢复 Rust 构建缓存。单个 crate 时间会**并行重叠**，不可累加为总节省时间。

真实热点及其背后的拖拽关系：

| 编译热点（基线 ARM64） | 时间（近似） | 拆除路径 |
| --- | ---: | --- |
| `xai-grok-shell` | 1612 s | 将 stock Backend 从 Pager 外部模式、Pi Adapter、bin 的可达闭包里剔除 |
| `xai-grok-pager` | 1517 s | UI 外部模式与 stock Agent 主入口按编译特性分开 |
| `pdf_oxide` | 462 s | 让 stock PDF reader 不参与 Pi 编译；先清理 `tools` 依赖边 |
| `aws-sdk-s3` | 332 s | 清掉 stock 文件/云上传功能到 Pi 的依赖边 |
| `aws-lc-sys` | 200 s | 仅在确定不再被其它 TLS/云功能引用时才从闭包消失 |
| `tikv-jemalloc-sys` | 174 s | 已用 Pi 专用 `--no-default-features` 处理 |
| `rhai` | 151 s | 拆 `pi-grok-adapter -> xai-workflow`，保留用户明确需要的 workflow |

每批次最终验收时附带四个指标：①实际 CI SHA / Run URL / 状态，②相同 profile 下 wall-clock，③ Cargo units，④ `cargo tree -e normal,build --target aarch64-apple-darwin` 的移除依赖证明。区分“**某 crate 不再直接依赖**”和“**该 crate 不再从 grok-pi 可达**”，报告两者时绝不混淆。

## 2. 目标依赖结构

```text
grok-pi (only composition binary)
  │
  ├─ pi-grok-adapter       → Pi JSONL RPC, ACP mapping, extension bridges
  │   └─ light config / typed host API (no original Grok agent runtime)
  │
  └─ xai-grok-pager [external-backend profile]
      ├─ xai-grok-pager-render  → xai-grok-image, xai-tty-utils, xai-grok-config
      ├─ native view + reducer + interaction state
      ├─ Pi-driven events and UI actions
      └─ theme / scrollback / diff / selection / modal

stock-grok (separate build feature, unchanged behavior)
  └─ xai-grok-pager [stock-agent profile]
      └─ xai-grok-shell
           ├─ xai-grok-tools → pdf_oxide ...
           ├─ Grok auth + sampler
           └─ stock cloud / workflow / other features
```

关键是消除编译期边 **而不是**仅让 Pi 在运行时不调用 stock Agent。底层小工具优先复用现有 crate，禁止为了三个 helper 新增一个“common-utils 大杂烩”包。

## 3. 大批量拆分清单（先重构，最后集中验证）

| 刀 | 具体变更 | 预期可证明的结果 | 风险 |
| --- | --- | --- | --- |
| S01 已完成 | Pi 入口显式关闭默认 jemalloc，保留 sandbox | CI 负向断言三项 jemalloc 不可达 | 低 |
| S02 已完成 | `cryptify` 变成 stock-only feature | Pi 编译闭包不含 `cryptify`；CI #43798146593 success | 低 |
| **S03 已提交待批量验收** | `xai-grok-pager-render` 去除直接 `xai-grok-tools`；四处 helper 直连已有轻量库 | Render manifest 不再直接依赖 Grok Tools | 低 |
| **S04 已修改待编译** | `xai-grok-pager-diff` 编辑详情类型移到 `xai-tool-types`，stock Tools re-export；Diff 只解析相关 wire JSON | Diff 代码与 manifest 去除 `xai-grok-tools`，保持 stock 结构化 diff 数据布局 | 中 |
| **S05 已修改待编译** | stock PDF 专用特性 `stock-pdf` 控制 `pdf_oxide` 和文件读取路径 | Pi 编译 feature 下不应含 `pdf_oxide` | 中 |
| **S06 已修改待编译** | `xai-file-utils` 的 `stock-s3` 功能与 S3 stub 分开 | Pi 编译 feature 下不应含 AWS S3 SDK / config / smithy client | 中 |
| **S07 已修改待编译** | `stock-gcs` 控制直接 GCS 客户端，移除 Shell 未使用的 GCS direct dependency | Pi 编译 feature 下不应含 `gcloud-storage`；代理上传实现保留 | 中 |
| S08a | 再审 `xai-grok-shell-terminal` 对 `xai-grok-tools` 的具体类型依赖 | 仅将无状态契约移到已有公共类型包，保持 Shell Terminal 的现有实现 | 中 |
| **S08a-1 本批已改待编译** | `TaskKind` 上移 `xai-tool-types`，Shell Terminal 直连类型，stock Tools 原路径 re-export | 持久化 background task JSON 的 `bash`/`monitor` 形状不变；仍未解除 Terminal 对 Tools 的其余引用 | 中 |
| **S08a-2 本批已改待编译** | `xai-grok-announcements` 对 Tools 的唯一 home 路径引用改为 `xai-dirs`，`xai-grok-update` 的 detach/pager-env 改为 `xai-tty-utils` | 两个 manifest 都不再直接依赖 Tools；Update 本身仍依赖 Shell | 低 |
| **S08a-3 本批已改待编译** | `xai-grok-login` 的 API key provider 上移 `xai-tool-types`，工具 401 归因契约上移 `xai-grok-auth`，进程分离直连 `xai-tty-utils` | Login 的源码和 manifest 不再直接依赖完整 Tools，原 Tools API 通过 re-export 保持可用 | 中 |
| **S08a-4 本批已改待编译** | Pi Adapter 的 Eval 配置只读直接使用 `xai-grok-config` | 去掉一处对 Shell 的磁盘配置 re-export 使用，保留远端 campaign-aware 的其它 Shell 配置调用 | 低 |
| **S08a-5 本批已改待编译** | 将 `ToolKind` 和唯一的 Claude↔Grok 别名注册表迁入 `xai-tool-types`；`xai-grok-hooks` 改为直接使用它及 `xai-tty-utils` | Hooks manifest 不再直接依赖 Tools，工具分类、匹配别名和 stock 原公共路径都保留 | 中 |
| S08b | Pager 的 `external-backend` 与 `stock-agent` **编译期**隔离；从 `lib.rs`、App reducer、Slash、Settings、session/view 逐块分层 | Pi 下 `xai-grok-pager` 编译入口不依赖 Shell 的 Agent 运行时 | 高 |
| S08c | `pi-grok-adapter` 的 Shell config / workflow host / session 枚举等从具体 Shell 实现改为精简 host 接口；必须保持 Pi 主导执行 | Adapter 不再直接链接 `xai-grok-shell`；API / workflow 行为不回归 | 高 |
| S08d | 完全去除 Pi 对 stock Tools、Shell agent loop 的编译依赖：继续裁 Grok Auth、采样器、MCP、Workflow 的真实可达路径 | `cargo tree` 证实剩余 stock Agent 后端不再可达；需要按边界小批量推进 | 高 |
| S09 | 验证与收敛：修复外部模式自测、删除无用门控/抽象、文档收敛 | 完整 Pi RPC/TUI/Herdr/Remote TUI 回归，冷/热 CI 性能可复现 | 中 |

如果某刀需要把模块一次性重写成多个新 crate，应先验证为什么不能只做一个可选依赖和一个现有类型层；**首先砍依赖边，其次才重排目录**。Stock 功能默认路径不应被无意改动。

## 4. 当前已确认的实际依赖来源

- `grok-pi` bin 本身依赖 `xai-grok-shell`、`xai-grok-login`、`xai-grok-workspace` 等，不因 Render 独立就自动变瘦。
- `xai-grok-pager` 也直接依赖 `xai-grok-shell`、`xai-grok-tools`，且 `lib.rs` 暴露 stock 模块；它是大目标。
- `pi-grok-adapter` 直接依赖 `xai-grok-shell` 和 `xai-workflow`；不能为了去掉 Rhai 破坏 Pi 主机 Workflow。
- Render 的 stock tools 调用只有四条：图像尺寸校验、scroll log home、URL opener 进程脱离、`format_bytes`；S03 已作针对性处理，图像从 `xai-grok-image` 接入。
- `xai-grok-pager-diff` 的直接 tools 依赖已改为只消费 `xai-tool-types` 中的共享编辑数据契约。Pager 和 Shell Terminal **仍**直接依赖 tools，不能把这次改善说成整个 tools 包消失。
- 本批新增的认证边界拆除使 `xai-grok-login` 不再直接依赖 Tools；类型身份通过 `xai-grok-tools::{types,attribution}` 对同一个公共类型 re-export 保持一致。Login 的其它依赖可能通过间接路径继续引用 Tools，需最终用 Cargo tree 证明闭包变化。
- `ToolKind` 的完整枚举与现有单一 Claude↔Grok 别名表共同迁移到 `xai-tool-types`，避免 Hooks 和 Agent Builder 持有两份可能漂移的工具映射；`xai-grok-tools::types::tool::ToolKind` 与 `types::claude_alias` 保留 re-export。ToolKind 的 serde、strum 以及别名表内原有测试需要批量验收。
- 本轮静态 Cargo manifest 排查后仍直接依赖 Tools 的 8 个 crate 为：`xai-grok-agent`、`xai-grok-mcp`、`xai-grok-memory`、`xai-grok-pager`、`xai-grok-shell-terminal`、`xai-grok-shell`、`xai-grok-subagent-resolution`、`xai-grok-workspace`。其中 Memory 的 `MemoryBackend`/`MemoryV2Access` 是 Login → Tools 的剩余间接路径；Terminal 还需要 `TerminalBackend`、`ToolNotificationHandle`、`BashOutput` 等接口。不要声称 Login 或 Hooks 的直接去依赖已经让整个 Pi 构建闭包不含 Tools。
- `pdf_oxide` 只在 `xai-grok-tools` 的 stock PDF 路径使用；`aws-sdk-s3` / `aws-config` / `aws-smithy-http-client` 只在 `xai-file-utils/s3.rs` 使用；`gcloud-storage` 只用于直接 GCS 客户端及 Shell 的一个未使用 manifest 声明（已移除）。
- stock Grok 组合根 `xai-grok-pager-bin` 的 `default` 与 `default-bazel` 显式启用 `stock-pdf`、`stock-s3`、`stock-gcs`；Pi 的 `--no-default-features --features release-dist,sandbox-enforce` 不启用这些功能。Pi 误调用原版文件上传分支将返回清晰错误，Pi agent 的工具由其独立进程执行。

## 5. 批量验证策略（用户决定：不要每拆一项就跑 CI）

1. **取消按提交自动 CI**：`.github/workflows/pi-slim-ci.yml` 只允许 `workflow_dispatch`，原先每小时自动续跑任务已暂停。中间改动仅静态审查、整理 Cargo.lock 和本地分组提交，先不中断开发。
2. 待一批依赖闭包拆完，再 **手动触发一次** ARM64 CI。使用 `cargo tree --locked -p xai-grok-pager-bin --no-default-features --features release-dist,sandbox-enforce --target aarch64-apple-darwin -e normal,build`，断言 jemalloc、cryptify、`pdf_oxide`、AWS SDK、`gcloud-storage` 缺席。
3. 同特性构建真正的 `grok-pi` ARM64 `release-dist` 二进制，验 `--version`、`--help`、`--print-capabilities`；并运行 Diff wire JSON 与 Render formatting 回归测试。
4. 静态阶段 **不宣称编译通过**；`--locked` 所需的 workspace Cargo.lock 节点依赖必须同步修改；跨模块 API/serde 行为也只能等真实编译和测试确认。
5. 不改 `main`、不 force-push、不随手发布 beta；所有工作只在隔离 worktree 和 `refactor/pi-slim-20261116` 中进行。
6. 缓存、runner、LTO 与版本不同造成的 wall-clock 不可混淆；最终再做同条件冷/热对照。

### 编译期大头的实际边界与执行顺序

经源文件调用审查，Pager 的 `src/` 有约 **193 个文件**直接提及 Shell 或 Tools；Pi Adapter 提及 Shell 的约六个文件，多数实际调用绑定在 **Workflow** 与启动配置上。它们不是简单的 Cargo.toml 改动，因此不采用将整个 Pager 或 Shell 标记 `optional`、再用几十个假的 `cfg` 分支遮掩的方式。

优先拆分顺序：

1. **叶子 → 上层逆依赖**：消除公告、更新、认证模块为了工具接口引入整个 Tools 的错误方向（本批已开始）。
2. **运行时契约 → Stock 行为**：继续把 Terminal Backend、ToolNotificationHandle、BashOutput 等协议转入拥有这些数据的基础层，保持现有 ACP 终端行为；工具操作本体仍留在 stock。验证序列化、取消、信号/超时。
3. **Workflow Host 提取**：当前 `xai-grok-shell/src/session/workflow` 中 registry/external/backend/notify/store 合计约 **2267 行**，并依赖 Shell 内部 SessionCommand、PersistenceMsg、Trust、SubagentCoordinator；不能直接移文件。先将这些上下文能力注入为接口/回调，host 运行与 registry 在独立库中装配，stock 和 Pi 共用单一实现，而不是 Pi 另写一套 workflow。
4. **Pager 外部模式**：逐层隔离 stock 的 auth/bootstrap/sampling/leader，而把纯 UI 状态、ACP 接口和工具卡片保留在通用 Pager。最终才让 Pi 组合根不依赖整个 Shell 实现。

切断**直接 Cargo 边**不等于整个 Pi 编译闭包已经没有 Shell/Tools。最终判断必须看 `--no-default-features --features release-dist,sandbox-enforce` 的 **真实 target-specific Cargo tree** 和 ARM64 编译测试。

## 6. 当前执行水位

- S01：`f1b14cd4` — [CI 成功 #43792742800](https://github.com/Dwsy/grok-pi-tui/actions/runs/43792742800)，Release 编译约 2848 s；不能从这一次推断绝对节省量。
- S02：`ed616ce9` — [CI 成功 #43798146593](https://github.com/Dwsy/grok-pi-tui/actions/runs/43798146593)。
- S03：提交 `d3428440`，Render 脱钩和十组字节格式测试，**尚未经新的编译验证**。
- S04–S07：跨工具类型、PDF、S3、GCS 的批量变更，Rustfmt、manifest、Cargo.lock 和负向源码引用的静态审查已通过，**未运行 Cargo 或新 CI，不能声称构建成功**。
- S08a-1–5：针对 Terminal、Announcements、Update、Login、Hooks、Adapter 的本轮共享契约拆分，**仅静态审查，不执行 Cargo 或 GitHub Actions**。

后续按批次更新提交、最终验证 Run、实际闭包变化和未消除的风险。**以可验证的证据替代 CI 次数。**
