# grok-pi S10：从 Pi-only 编译闭包拆除 stock LSP 与 stock web_fetch 实现

> 日期：2026-11-18  
> 分支：`refactor/pi-slim-20261116`（隔离工作树 `/Users/dengwenyu/Dev/AI/pi-grok-build-worktrees/pi-slim-20261116`）  
> 依据数据：CI `#44198844083`（`45386652`，`aarch64-apple-darwin` `release-dist`）真实 `cargo-timing.html` 与 `Cargo.lock` 反向依赖分析。

## 1. 背景与量化依据

在 `#44198844083`（Wall-clock `1818.0s`，`1288 units`）真实 timing 中，`grok-pi`（`xai-grok-pager-bin --no-default-features --features release-dist,sandbox-enforce`）仍然编译了多组仅由原版 Grok 工具执行器使用的重型依赖，其中最独立、零跨模块逻辑复制的叶子子系统为：

1. **原版 LSP 运行时（`stock-lsp`）**：
   - `async-lsp`（`16.79s`）+ `lsp-types`（`10.31s`）及 `xai-grok-tools/src/implementations/lsp/` 14 个客户端/管理器实现模块（约 6,000+ 行）。
   - `xai-grok-pager`、`pi-grok-adapter` 与 `grok-pi.rs` 对 `implementations::lsp` 零调用；唯一外部构造点只有 `xai-grok-workspace/src/handle.rs` 与 `xai-grok-shell/src/agent/mvp_agent/agent_ops.rs`（`LspManager` / `LspBackendAdapter`）。
2. **原版 HTML `web_fetch` 抓取与转换器（`stock-web-fetch`）**：
   - `scraper`（`5.64s`）+ `htmd`（`5.14s`）及 `html5ever` / `selectors` / `cssparser` / `markup5ever*` / `xml5ever` / `string_cache*` / `web_atoms` / `tendril` / `ego-tree` 等传递依赖，以及 `web_fetch/{artifact,cache,client,http,overflow,ssrf}.rs`（约 3,252 行）。
   - `WebFetchClient` 在整个 workspace 仅由 `xai-grok-tools` 自身（`web_fetch/mod.rs` 与 `registry/types.rs`）构造和调用；外部仅使用轻量类型 `WebFetchConfig`、`WebFetchParams`、`WebFetchInput`、`DomainMatcher`、`domain_from_url`。

根据 `Cargo.lock` 与 `#44198844083` `cargo-timing.html` 统计，从 `xai-grok-tools` 的 Pi-only 闭包切断 `async-lsp`、`scraper`、`htmd` 后，预期可从 `grok-pi` 可达依赖树中移除 **27 个 crate**（在 `#44198844083` 中对应合计 `52.33s` unit 编译耗时，外加 `xai-grok-tools` 自身约 9,250+ 行代码的前端与 codegen 负载）。

## 2. 边界与设计原则

- 延续 `S05`（`stock-pdf`）、`S06`（`stock-s3`）、`S07`（`stock-gcs`）模式：
  - `xai-grok-tools` 的 `default` 与 `default-bazel` 保持 `["serde"]`，将 `async-lsp`、`scraper`、`htmd` 设为 `optional = true`，新增 `stock-lsp` 与 `stock-web-fetch` feature。
  - `xai-grok-workspace`（`default` / `default-bazel` 启用 `stock-lsp`）、`xai-grok-shell`（`default` / `default-bazel` 启用 `stock-lsp`、`stock-web-fetch`）与 `xai-grok-pager-bin`（`default` / `default-bazel` 启用 `stock-lsp`、`stock-web-fetch`）负责为原版 `xai-grok-pager` 默认开启。
  - `grok-pi` 继续使用 `--no-default-features --features release-dist,sandbox-enforce`，天然不启用 `stock-lsp` 与 `stock-web-fetch`。
- 单一所有者与零复制：
  - 将 `DiagnosticsSummary` 归位至 `lsp/types.rs`（`manager.rs` 改为 `pub use super::types::DiagnosticsSummary`），保留 `lsp/config.rs`、`LspConfig`、`LspBackend`、`LspTool`、`WebFetchConfig`、`WebFetchParams`、`WebFetchTool`、`DomainMatcher`、`WebFetchError` 无条件编译，确保 `xai-grok-agent`、`xai-grok-workspace` 权限逻辑与 `xai-grok-shell` 信任门控无需零碎 `cfg`。
  - 不触碰 `nono`（`sandbox-enforce`）、`xai-grok-mermaid`（Mermaid）、`xai-grok-voice`、单套 Workflow、Pi Eval MCP 与原版 stock Grok 默认能力。

## 3. 验收标准

1. CI `Assert stock-only dependency closure is absent from Pi` 断言 `async-lsp`、`lsp-types`、`scraper`、`htmd` 不在 `grok-pi` 的 `cargo tree -e normal,build` 闭包内。
2. 原版默认 feature（`xai-grok-pager-bin` default）仍包含 `async-lsp`、`scraper`、`htmd`，且 `xai-grok-tools` 在开启 `stock-lsp,stock-web-fetch,stock-pdf` 时编译检查通过。
3. Pi-only `xai-grok-workspace`、`xai-grok-shell`、`release-dist` 二进制构建、PTY/RPC smoke、Adapter 单测与候选包上传全部在 GitHub ARM64 CI 通过。
