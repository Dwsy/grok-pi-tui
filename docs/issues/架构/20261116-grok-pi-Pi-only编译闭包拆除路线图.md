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

**禁止**：再引入第二套 Agent 会话/工具、再造 TUI；为避免调整代码而“占位”保留巨大依赖；将 UI runtime flag 冒充真正的编译期解耦；一次提交改几十个互不关联的 Cargo crates。

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

每轮附带四个指标：①实际 CI SHA / Run URL / 状态，②相同 profile 下 wall-clock，③ Cargo units，④ `cargo tree -e normal,build --target aarch64-apple-darwin` 的移除依赖证明。区分“**某 crate 不再直接依赖**”和“**该 crate 不再从 grok-pi 可达**”，报告两者时绝不混淆。

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

## 3. 原子执行清单（每刀一 commit、每 commit 一次 ARM64 CI）

| 刀 | 具体变更 | 预期可证明的结果 | 风险 |
| --- | --- | --- | --- |
| S01 已完成 | Pi 入口显式关闭默认 jemalloc，保留 sandbox | CI 负向断言三项 jemalloc 不可达 | 低 |
| S02 进行中 CI | `cryptify` 变成 stock-only feature | Pi 编译闭包不含 `cryptify` | 低 |
| **S03 实施中** | `xai-grok-pager-render` 去除直接 `xai-grok-tools`；四处 helper 直连已有轻量库 | Render manifest 不再直接依赖 Grok Tools；UI 输出与命令安全语义不变 | 低 |
| S04 | `xai-grok-pager-diff` / `xai-grok-shell-terminal` 对 `xai-grok-tools` 的纯工具调用分流 | 多个纯 UI crate 无需引用 stock tool runtime | 低～中 |
| S05 | Pager 的 `external-backend` 与 `stock-agent` **编译期**隔离；从 `lib.rs`、App reducer、Slash、Settings、session/view 逐块分层 | Pi 下 `xai-grok-pager` 编译入口不依赖 Shell 的 Agent 运行时 | 高 |
| S06 | `pi-grok-adapter` 的 Shell config / workflow host / session 枚举等从具体 Shell 实现改为精简 host 接口；必须保持 Pi 主导执行 | Adapter 不再直接链接 `xai-grok-shell`；API / workflow 行为不回归 | 高 |
| S07 | stock `xai-grok-tools`、`pdf_oxide` 的生产依赖边全部从 Pi 构建闭包中删除，`read_file` 留在 stock 路径 | CI 负向断言 tools + pdf_oxide 真正不可达 | 高 |
| S08 | `xai-file-utils` 和 AWS/GCS 云组件、Grok Auth/sampler/voice/Mermaid 的可选边逐一审查与裁断 | 依据 `cargo tree` 实际去掉不必要的 AWS SDK / native libs，保留 Pi 所需图像/语音/绘图 | 中～高 |
| S09 | 验证与收敛：修复外部模式自测、删除无用门控/抽象、文档收敛 | 完整 Pi RPC/TUI/Herdr/Remote TUI 回归，冷/热 CI 性能可复现 | 中 |

如果某刀需要把模块一次性重写成多个新 crate，应先验证为什么不能只做一个可选依赖和一个现有类型层；**首先砍依赖边，其次才重排目录**。Stock 功能默认路径不应被无意改动。

## 4. 当前已确认的实际依赖来源

- `grok-pi` bin 本身依赖 `xai-grok-shell`、`xai-grok-login`、`xai-grok-workspace` 等，不因 Render 独立就自动变瘦。
- `xai-grok-pager` 也直接依赖 `xai-grok-shell`、`xai-grok-tools`，且 `lib.rs` 暴露 stock 模块；它是大目标。
- `pi-grok-adapter` 直接依赖 `xai-grok-shell` 和 `xai-workflow`；不能为了去掉 Rhai 破坏 Pi 主机 Workflow。
- Render 的 stock tools 调用只有四条：图像尺寸校验、scroll log home、URL opener 进程脱离、`format_bytes`；S03 已作针对性处理，图像从 `xai-grok-image` 接入。
- `xai-grok-pager-diff` 还直接依赖 tools；后续审计不应被 S03 的直接依赖变化蒙蔽。

## 5. 每次 CI 的硬门槛

1. **前序 CI completed + success 才能推下一刀**。若失败只修该刀，不能绕过去继续排队提交。
2. `cargo tree --locked -p xai-grok-pager-bin --no-default-features --features release-dist,sandbox-enforce --target aarch64-apple-darwin -e normal,build`，新增本刀应消失的负向断言；不删除之前断言。
3. 同特性构建真正的 `grok-pi` ARM64 `release-dist` 二进制，并验 `--version`、`--help`、`--print-capabilities`。
4. UI 改动需补窄而可维护的回归测试（如 Render 格式输出和图像识别），CI 不仅验证能链接，也须有可执行测试证据。
5. 不改 `main`、不 force-push、不随手发布 beta；所有工作只在隔离 worktree 和 `refactor/pi-slim-20261116` 中进行。
6. 有 CI 缓存时记录命中与否，不把不同源码版本或 runner 的耗时当作可靠加速比例；达到一条明显稳定的最小闭包后再做同 SHA 的冷/热编译对照。

## 6. 当前执行水位

- S01：`f1b14cd4` — [CI 成功 #43792742800](https://github.com/Dwsy/grok-pi-tui/actions/runs/43792742800)，Release 编译约 2848 s；不能从这一次推断绝对节省量。
- S02：`ed616ce9` — [CI #43798146593](https://github.com/Dwsy/grok-pi-tui/actions/runs/43798146593)，上次检查仍在运行。
- S03：已在隔离 worktree 实施 Render 脱钩及十组边界值回归测试，静态检查和 YAML / Cargo manifest 检查均已通过；**仅本地提交，等待 S02 CI 成功后推送并启动独立 CI**。

后续每刀在本文件追加 commit、CI Run、依赖图变化、基线对比、回归风险；不要使用另一个与代码状态脱节的“完成进度表”。
