---
title: S12 继续瘦身：将 Shell/Pager 10 个 Stock-only 依赖及 wrap/trace CLI 实现门控至 stock-agent
category: 架构
status: completed
created: 2026-11-24
---

# 背景

继 `e08f51db` 将 `bm25`、`moka`、`kanal` 移入 `stock-agent` 后，静态依赖扫描显示 `xai-grok-shell` 与 `xai-grok-pager` 中仍残留 10 个仅被 Stock Grok 路径使用的无条件依赖（其中 `tokio-retry` 与 `signal-hook` 在整个工作区无其他 Pi 消费者，门控后可直接从 `grok-pi` 完整闭包中移除），且 `xai-grok-pager` 的 `wrap_cmd` / `pty_wrap` 与 `trace_cmd` 运行时实现仍随 Pi profile 无条件编译。

# 改造内容

1. **`xai-grok-shell/Cargo.toml`（5 项零源码改动的已门控依赖降为 optional）**：
   - `tokio-retry`（仅 `acp_session_impl/auth_retry.rs`，**彻底退出 `grok-pi` 编译闭包**）
   - `backon`（仅 `acp_session_impl/recap.rs`）
   - `webbrowser`（仅 `agent/app.rs`）
   - `nucleo`（仅 `extensions/suggest/file_provider.rs`）
   - `xai-computer-hub-sdk`（仅 `acp_session.rs` / `acp_session_impl/spawn.rs` / `leader/server.rs` / `acp_session_tests/support.rs`）
   - 全部设为 `optional = true` 并加入 `stock-agent` feature（`default` 保持不变，`--locked` 零 `Cargo.lock` 改动）。
2. **`xai-grok-pager`（5 项依赖降为 optional + 门控 `wrap`/`trace` Stock CLI 实现）**：
   - `signal-hook`（仅 `pty_wrap.rs`，**彻底退出 `grok-pi` 编译闭包**）、`portable-pty`（仅 `pty_wrap.rs`）、`backon`/`flate2`/`tar`（仅 `trace_cmd.rs`）设为 `optional = true` 并加入 `stock-agent`；
   - `lib.rs` 中将 `pty_wrap`、`wrap_cmd`、`wrap_filter`、`wrap_restore` 门控至 `#[cfg(feature = "stock-agent")]`（保留 `wrap_clipboard_image` 供现有测试与 UI 使用）；
   - `trace_cmd.rs` 保留无条件 `TraceArgs` 供 `cli.rs` 解析，将 650+ 行归档/上传运行时拆至 `#[cfg(feature = "stock-agent")] mod imp` (`trace_cmd_imp.rs`)；
   - `tests/signal_errno_preservation.rs` 增加 `feature = "stock-agent"` 条件；`xai-grok-pager-bin/src/main.rs` 对 `Command::Wrap` 与 `Command::Trace` 补齐 `#[cfg(feature = "stock-agent")]` 守卫。
3. **CI 闭环守卫 (`.github/workflows/pi-slim-linux-checks.yml`)**：
   - 将 `tokio-retry` 与 `signal-hook` 加入 Pi 负向闭包断言与 Stock 正向保留断言。

# 验收清单
- [x] `xai-grok-shell/Cargo.toml` 与 `xai-grok-pager/Cargo.toml` 通过 Python `tomllib` 语法校验，零 `Cargo.lock` 变更
- [x] 所有被门控依赖在非 `stock-agent` 源码与测试中零残留引用
- [x] `.github/workflows/pi-slim-linux-checks.yml` 补齐 `tokio-retry` 与 `signal-hook` 双向断言
- [x] `git diff --check` 通过，交由 CI `--locked` 验证双 profile 编译与闭包
