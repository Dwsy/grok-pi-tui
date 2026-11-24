---
id: 20261124-omp-shell-on-demand-download-tui
title: OMP Shell 独立构建解耦与 F2 按需下载 TUI 弹窗
status: done
created: 2026-11-24
area: pager
---

# 背景与目标

`grok-pi-shell-host`（OMP Shell 后端）目前在 `.github/workflows/release.yml` 中会被捆绑进 `grok-pi` macOS ARM64 beta 发布包，导致主发布包体积膨胀；同时在 F2 设置中开启 `OMP Shell backend`（`[ui].pi_omp_shell`）时，即使本地未安装 `grok-pi-shell-host` 也会直接写入 `true`，缺少引导下载与校验流程。

目标：
1. **发布包彻底解耦**：`release.yml` 不再下载、构建或打包 `grok-pi-shell-host`；`grok-pi-shell-host` 仅通过独立 workflow `.github/workflows/omp-shell-host.yml` 发布到 `omp-shell-host-latest` pre-release（同时提供稳定别名 `grok-pi-shell-host-macos-arm64.tar.gz` 与 tree-hash 版本资产）。
2. **默认关闭、按需下载**：`[ui].pi_omp_shell` 默认保持 `false`。用户在 F2 中切换开启时：
   - 若本地已存在可执行 `grok-pi-shell-host`（按 `PI_GROK_OMP_SHELL_HOST` → `${GROK_HOME:-~/.grok-pi}/bin/grok-pi-shell-host` → `PATH` 检测），立即开启并持久化。
   - 若本地未安装，保持 `pi_omp_shell = false`，弹出居中 TUI 下载弹窗展示实时进度（查询发布 → 流式下载字节与百分比进度条 → 解压 → `--handshake` 校验 → 原子安装至 `~/.grok-pi/bin/grok-pi-shell-host`），支持 `Esc` 取消与失败后 `Enter` 重试。
   - **仅当下载、解压与 `--handshake` 校验全部成功后**，才将 `pi_omp_shell` 置为 `true` 并持久化到 `config.toml`，同时刷新 F2 设置显示。

# 变更范围

- `.github/workflows/release.yml`：移除 `Reuse or build standalone OMP Shell Host` 步骤与 `Package Unix binary` 中的 `grok-pi-shell-host` 捆绑逻辑。
- `.github/workflows/omp-shell-host.yml`：同步上传稳定别名 `grok-pi-shell-host-macos-arm64.tar.gz`，更新说明文字。
- `crates/codegen/xai-grok-shell/src/util/omp_shell_install.rs`（及 `util/mod.rs`）：零新依赖的按需检测与流式下载安装器（复用 `reqwest` + `futures-util` + 系统 `tar` + `--handshake` 校验）。
- `crates/codegen/xai-grok-pager/src/views/omp_shell_install_modal.rs`（及 `views/mod.rs`）：居中圆角边框下载进度 TUI 弹窗，复用 `views::progress_bar::render_progress_bar`。
- `crates/codegen/xai-grok-pager/src/app/`：`AppView` 顶层弹窗接入、`view_tick_demand()` 动画刷新、`Effect::InstallOmpShellHost` / `TaskResult::OmpShellInstallDone` 与 `set_pi_omp_shell` 按需触发逻辑。

# 验证方式

- 不运行本地 `cargo`（遵循仓库规范），逐文件静态核对默认与 `--no-default-features` 双 profile 的类型、feature gate、枚举分支完整性。
- 校验 `Cargo.lock` 零变更与 `git diff --check` 无空白/语法问题。
