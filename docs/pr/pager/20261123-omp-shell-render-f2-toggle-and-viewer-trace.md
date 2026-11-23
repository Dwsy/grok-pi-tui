---
title: OMP Shell 渲染重构（参考 oh-my-pi）、F2 配置开关与回车预览弹窗 Trace 展示
category: pager
status: merged
issue: docs/issues/pager/20261123-omp-shell-render-f2-toggle-and-viewer-trace.md
created: 2026-11-23
---

# 变更摘要

1. **F2 设置与 `config.toml` 持久化开关 (`[ui].pi_omp_shell`)**：
   - 在 `xai-grok-shared`、`xai-grok-shell`、`xai-grok-pager` 全链路新增 `pi_omp_shell: bool`（默认 `false`），位于 F2 设置 `Agent → Pi features`（紧邻 `pi_bash`），支持持久化与失败回滚。
   - `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` 与 `runtime_config.rs` 启动时读取 `[ui].pi_omp_shell`，在未显式设置 `PI_GROK_BASH_BACKEND` 环境变量时自动注入 `PI_GROK_BASH_BACKEND=omp`。
2. **参考 `oh-my-pi` 重构 Bash + OMP Trace 渲染 (`execute.rs`)**：
   - 新增 `normalize_shell_trace_rows` 展开重复 AST 包装节点（`and_or`、单/多阶段 `pipeline`、空/`word` 及重复单子容器），管道第 2+ 阶段前置 `| `，单步简单命令不再在滚动区重复展示 Trace。
   - 参照 `oh-my-pi/packages/tui/src/tools/shell-viz-render.ts` 实现紧凑 1 字符状态 gutter（`×` / `▶` / `⊘` / 空白）、轻量 Shell 语法着色与双空格分隔的淡化元信息（仅在有值时展示 `exit N` / `×runs` / `bg` / 耗时，省略无意义 `0ms`），并将 Trace 置于 Output 上方。
3. **回车预览弹窗 (`BlockViewerPane::for_execute`) 完整展示 OMP Trace**：
   - 在 `views/block_viewer/mod.rs` 中新增 `build_execute_items_with_trace` 与 `execute_generation`，回车预览时在 `Output` 顶部完整展示结构化 Trace（含单步命令与不截断步骤），支持迟到 trace 自动刷新与主题切换重绘。

# 验证
- `git diff --check` 通过（零空白/冲突标记错误）。
- 遵循仓库约束未在本地运行 `cargo`；已对所有受影响 Rust 结构字段、穷举 match 分支与单元测试 (`execute.rs`、`block_viewer_tests.rs`、`grok-pi.rs`、`registry.rs`、`dispatch/tests/settings.rs`) 完成静态校验，交由 CI 验证编译与测试。
