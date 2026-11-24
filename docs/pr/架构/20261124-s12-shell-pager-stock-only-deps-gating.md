---
title: S12 继续瘦身：将 Shell/Pager 10 个 Stock-only 依赖及 wrap/trace CLI 实现门控至 stock-agent
category: 架构
status: merged
issue: docs/issues/架构/20261124-s12-shell-pager-stock-only-deps-gating.md
created: 2026-11-24
---

# 变更摘要

- **`xai-grok-shell/Cargo.toml`**：将 `tokio-retry`、`backon`、`webbrowser`、`nucleo`、`xai-computer-hub-sdk` 5 个仅在 `stock-agent` 模块中使用的依赖改为 `optional = true`，由 `stock-agent` 启用。
- **`xai-grok-pager`**：将 `signal-hook`、`portable-pty`、`backon`、`flate2`、`tar` 5 个依赖改为 `optional = true` 并挂入 `stock-agent`；门控 `pty_wrap` / `wrap_cmd` / `wrap_filter` / `wrap_restore` 与 `trace_cmd` 的归档/上传实现（拆至 `trace_cmd_imp.rs`，`TraceArgs` 保持无条件供 `cli.rs` 使用）；更新 `tests/signal_errno_preservation.rs` 与 `xai-grok-pager-bin/src/main.rs`。
- **`.github/workflows/pi-slim-linux-checks.yml`**：新增 `tokio-retry` 与 `signal-hook` 的 Pi 负向闭包断言和 Stock 正向保留断言。

# 验证
- Python `tomllib` 校验修改后的两个 `Cargo.toml` 通过；
- `git diff --check` 通过；
- 遵循仓库约束未在本地运行 `cargo`，双 profile `--locked` 编译与 `cargo tree` 闭包断言交由 CI 实证。
