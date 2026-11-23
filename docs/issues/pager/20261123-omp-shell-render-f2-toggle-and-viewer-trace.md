---
title: OMP Shell 渲染重构（参考 oh-my-pi）、F2 配置开关与回车预览弹窗 Trace 展示
category: pager
status: completed
created: 2026-11-23
---

# 问题背景

开启 OMP Shell (`grok-pi-shell-host`) 后存在三项体验缺陷：

1. **滚动区 Trace 渲染丑且高度重复**：
   - `tools/grok-pi-shell-host/src/trace.rs` 保留了全部 AST 容器节点（如 `and_or`、单阶段 `pipeline`），`xai-grok-pager` 的 `ExecuteToolCallBlock::push_shell_trace_lines` 直接逐条平铺渲染，导致单条命令也会重复打印两次（连同 `$ command` 标题共三次），复合命令重复打印整条脚本与各子阶段；
   - 样式为原始 `trace (N steps, Xms)` + 素色文本 + 噪点 `(0ms)`，缺少 `oh-my-pi` (`packages/tui/src/tools/shell-viz-render.ts`) 的紧凑视觉层次、状态 gutter（`×` / `▶` / `⊘` / 空白）与语法着色。
2. **F2 设置无法开关 OMP Shell**：
   - 目前只能靠手动设置环境变量 `PI_GROK_BASH_BACKEND=omp` 启用，F2 设置面板和 `config.toml` (`[ui]`) 中没有对应的持久化开关。
3. **按回车进入预览弹窗 (`BlockViewer`) 没有 Trace**：
   - `BlockViewerPane::for_execute` 与 `build_execute_items` 只渲染 `exec.output`，完全忽略 `exec.shell_trace`。

# 目标与方案

## 1. F2 设置与 `config.toml` 持久化开关 (`[ui].pi_omp_shell`)
- 在 `xai-grok-shared` (`UiConfig::pi_omp_shell: bool`，默认 `false`)、`xai-grok-shell` (`set_pi_omp_shell`)、`xai-grok-pager`（F2 设置定义/布局/注册表/dispatch/effect）新增 `pi_omp_shell` 开关（位于 Agent -> Pi features，紧邻 `pi_bash`）。
- 在 `grok-pi.rs` 启动注入环境时：若用户未显式设置 `PI_GROK_BASH_BACKEND` 环境变量，且 `[ui].pi_omp_shell = true`，自动注入 `PI_GROK_BASH_BACKEND=omp`；若用户显式设置了环境变量则环境变量优先。

## 2. 参考 `oh-my-pi` 重构 OMP Trace 渲染（去重 + 紧凑语法高亮）
- 新增共享的 `shell_trace` 规范化与渲染逻辑（供 `ExecuteToolCallBlock` 和 `BlockViewerPane` 复用）：
  - **AST 包装节点去重**：自动展开纯语法包装节点（有子节点的 `and_or`、单阶段 `pipeline`、与唯一子节点命令完全相同的外层节点、`word` 等空/冗余节点）；多阶段 `pipeline` 直接展开为子阶段并在第 2+ 阶段前附加 `| ` 管道符；重新归零缩进深度。
  - **单步命令静默**：滚动区中仅当规范化后步骤数 `> 1` 时展示 Trace，避免单条简单命令与 `$ command` 标题重复。
  - **OMP 风格渲染**：参照 `oh-my-pi/packages/tui/src/tools/shell-viz-render.ts`，使用紧凑 1 字符状态 gutter（失败 `×` 红、运行 `▶` 主题强调、跳过 `⊘` 淡化、成功留白）、轻量 Shell 语法着色（命令名/关键字/字符串/操作符/参数）、双空格分隔的淡化元信息（仅在有值时展示 `exit N`、`×runs`、`bg`、格式化耗时 `12ms` / `1.2s`，省略无意义 `0ms`）。

## 3. 回车预览弹窗 (`BlockViewerPane::for_execute`) 完整展示 Trace
- 在 `BlockViewerPane::for_execute` 与实时同步/主题切换刷新路径中，当 `exec.shell_trace` 存在时，在输出顶部优先展示完整 OMP 风格 Trace（不截断 6 条上限，且单步命令也可在弹窗中查看耗时与状态），其下紧接 `Output` 区域。
- 修复 `last_generation` 变更检测，确保 `shell_trace` 晚于 stdout 到达时弹窗能自动刷新。

# 验收清单
- [x] F2 设置面板 Agent -> Pi features 中可见 `OMP Shell backend` (`pi_omp_shell`) 开关并持久化到 `config.toml`
- [x] `grok-pi.rs` 在未显式覆盖 `PI_GROK_BASH_BACKEND` 时按 `[ui].pi_omp_shell` 注入 `omp`
- [x] 滚动区单命令不再出现重复 trace；复合/管道命令按 OMP 风格去重紧凑渲染
- [x] 回车进入 `BlockViewer` 可见完整结构化 Trace 与 Output，且流式/完成更新正确同步
- [x] `git diff --check` 通过，所有受影响 Rust 单元测试代码与静态类型检查一致（遵循禁止本地运行 cargo 约束）
