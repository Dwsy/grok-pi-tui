# 本地未 Push 变更汇总（2026-08-23）

> 基线：`origin/main`（当前 `main` 跟踪分支）
> 范围：已提交但未 push 的 21 个 commit + 当前 index 中尚未提交的 staged 变更。
> 本文只描述 Git 可验证事实；不把未执行的验证写成已通过。

## 1. 总览

- 当前分支：`main`，相对 `origin/main` **ahead 21**。
- 已提交未 push：**178 个文件发生变化**；Git 汇总：` 178 files changed, 15809 insertions(+), 8022 deletions(-)`。
- 当前未提交 staged：**10 个文件**；Git 汇总：`10 files changed, 789 insertions(+), 44 deletions(-)`。
- 误加入的 `pi-session-*.html` 已从 index 移除，并新增根目录 `.gitignore` 规则 `/pi-session-*.html` 防止再次误提交。

## 2. 已提交但未 Push：21 个 commit

- `a8f27a16` feat(eval): expand v2 runtime and task support
- `b185c98a` docs: sync Chinese Eval documentation
- `35ebd911` fix(bridge): keep recap/btw traffic out of the agent loop context
- `7052ce46` fix(adapter): resolve PSM session DB path cross-platform
- `1fb3d07a` fix(adapter): finish cross-platform PSM resume/search paths
- `55e904d5` refactor(extensions): split bundled Pi bridges into modules
- `955d96ea` feat(subagents): add opt-in V2 team collaboration
- `7821f92d` feat(eval): harden v2 tool bridge and display outputs
- `be3c4f77` fix(adapter): hold steer rows until safe-point dispatch
- `dc5c3f00` feat(settings): add live Eval v2 display mode
- `53a62678` feat(cli): add leader management commands
- `ca3ab4b3` fix(models): resolve provider ids containing slashes
- `70d9c237` fix(pager): render large paste choice as prompt dropdown
- `2b776128` fix(pager): cancel blocking question cards with turns
- `249535f0` feat(timeline): show stable message timestamps and hover dates
- `80046f5f` docs: sync subagents queue and Eval display behavior
- `3f53b7f6` chore: apply rustfmt to touched Rust files
- `cde1af6e` feat(pi-grok-todo): split into configurable v1/v2 with cross-version migration
- `ec7c75fb` feat(settings): drive Pi host features through HostFeatureManifest
- `cd931c3a` feat(grok-pi): manifest-driven startup wiring and Pi subagent transport
- `2fbc3fbc` feat(pager): keep write/edit hover popups open while hovered, scroll inside


### 2.1 主要变更主题

1. **Eval Bridge v2 与任务运行时**：扩展 v2 eval/tool bridge、后台任务与输出展示，增加 live Eval v2 display mode，并同步中英文文档。
2. **Agent loop / bridge 隔离与队列安全**：recap/btw 流量不再污染 agent context；steer 行延迟到 safe-point 分发。
3. **跨平台 Pi Session Manager 路径**：修复 PSM session DB、resume/search 路径解析。
4. **Bundled Pi extensions 模块化**：auth、btw、loop、recap、remote-tui、rollback、shortcut-manager 从大单文件拆成职责模块，并补 tsconfig/类型辅助。
5. **Subagents V2 team collaboration**：稳定 `/root/...` agent path、team preset、peer messaging、nested spawn、wait/list/interrupt、V2 tests 与中英文使用文档。
6. **Pager / Adapter 交互修复**：provider id 含 `/` 的模型解析、大粘贴 prompt dropdown、阻塞 question card 随 turn 取消、timeline 稳定时间戳/hover 日期。
7. **Todo V1/V2**：todo extension 拆分可配置 V1/V2，并加入跨版本 migration/compat tests。
8. **HostFeatureManifest**：F2/Pi host feature 的定义、配置读写、startup env 与 extension wiring 收敛到 manifest；随后 grok-pi 启动逻辑进一步改为 manifest-driven，并新增 Pi subagent transport。
9. **Review/hover UX 前置工作**：最新 commit 改善 write/edit hover popup 的驻留与内部滚动。

### 2.2 已提交未 Push 的文件清单

```text
M	CHANGELOG.MD
M	CHANGELOG.zh-CN.md
M	FEATURE_MATRIX.md
M	FEATURE_MATRIX.zh-CN.md
M	README.md
M	README.zh-CN.md
M	VERIFICATION.md
M	crates/codegen/pi-grok-adapter/Cargo.toml
M	crates/codegen/pi-grok-adapter/src/btw_bridge.rs
M	crates/codegen/pi-grok-adapter/src/lib.rs
M	crates/codegen/pi-grok-adapter/src/model.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/agent.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/events.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/queue_runtime.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/recovery.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/replay.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/session.rs
M	crates/codegen/pi-grok-adapter/src/pi_adapter/tests.rs
M	crates/codegen/pi-grok-adapter/src/pi_rpc.rs
M	crates/codegen/pi-grok-adapter/src/psm_session_catalog.rs
M	crates/codegen/pi-grok-adapter/src/queue_bridge.rs
M	crates/codegen/pi-grok-adapter/src/recap_bridge.rs
M	crates/codegen/pi-grok-adapter/src/subagent_projection.rs
A	crates/codegen/pi-grok-adapter/src/subagent_transport.rs
M	crates/codegen/pi-grok-adapter/src/tool_projection/tests.rs
M	crates/codegen/xai-fuzzy-file-search/src/lib.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/auth_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/bash_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/btw_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/cli.rs
A	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/extension_self_heal.rs
A	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/host_feature_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/loop_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/recap_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/remote_tui_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/rollback_extension.rs
A	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/runtime_config.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/shortcut_manager_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/skills/grok-pi-config/SKILL.md
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/subagent_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/todo_extension.rs
D	crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/workflow_extension.rs
M	crates/codegen/xai-grok-pager-bin/src/main.rs
M	crates/codegen/xai-grok-pager-render/src/appearance/cache.rs
M	crates/codegen/xai-grok-pager/src/acp/mod.rs
M	crates/codegen/xai-grok-pager/src/acp/model_state.rs
M	crates/codegen/xai-grok-pager/src/acp/tracker.rs
M	crates/codegen/xai-grok-pager/src/app/actions.rs
M	crates/codegen/xai-grok-pager/src/app/agent_view/mod.rs
M	crates/codegen/xai-grok-pager/src/app/agent_view/panes.rs
M	crates/codegen/xai-grok-pager/src/app/agent_view/paste.rs
M	crates/codegen/xai-grok-pager/src/app/agent_view/render.rs
M	crates/codegen/xai-grok-pager/src/app/agent_view/session.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/router.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/settings/setters.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/settings/ui.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/tests/settings.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/tests/turn.rs
M	crates/codegen/xai-grok-pager/src/app/dispatch/turn.rs
M	crates/codegen/xai-grok-pager/src/app/effects/helpers.rs
M	crates/codegen/xai-grok-pager/src/app/event_loop.rs
M	crates/codegen/xai-grok-pager/src/app/modals.rs
M	crates/codegen/xai-grok-pager/src/app/mouse.rs
M	crates/codegen/xai-grok-pager/src/scrollback/block.rs
M	crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/eval.rs
M	crates/codegen/xai-grok-pager/src/scrollback/export.rs
M	crates/codegen/xai-grok-pager/src/scrollback/mod.rs
M	crates/codegen/xai-grok-pager/src/scrollback/render.rs
M	crates/codegen/xai-grok-pager/src/scrollback/scrollback_pane.rs
M	crates/codegen/xai-grok-pager/src/scrollback/state/timeline.rs
M	crates/codegen/xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs
M	crates/codegen/xai-grok-pager/src/settings/defs.rs
M	crates/codegen/xai-grok-pager/src/settings/layout.rs
M	crates/codegen/xai-grok-pager/src/settings/registry.rs
A	crates/codegen/xai-grok-pager/src/slash/commands/eval_display.rs
M	crates/codegen/xai-grok-pager/src/slash/commands/mod.rs
M	crates/codegen/xai-grok-pager/src/views/agent.rs
M	crates/codegen/xai-grok-pager/src/views/modal.rs
M	crates/codegen/xai-grok-pager/src/views/pi_settings/actions.rs
M	crates/codegen/xai-grok-pager/src/views/settings_modal/state.rs
M	crates/codegen/xai-grok-pager/src/views/timeline.rs
M	crates/codegen/xai-grok-pager/tests/settings_e2e.rs
M	crates/codegen/xai-grok-shared/src/ui_config.rs
A	crates/codegen/xai-grok-shell/src/host_features.rs
M	crates/codegen/xai-grok-shell/src/lib.rs
M	crates/codegen/xai-grok-shell/src/session/acp_session_tests/tool_layer_images_bridge_tests.rs
M	crates/codegen/xai-grok-shell/src/session/workflow/manager.rs
M	crates/codegen/xai-grok-shell/src/util/config/settings_writes.rs
A	docs/adr/20260822-subagents-v2-team-collaboration.md
M	"docs/issues/adapter/20260818-Eval Bridge v2 \344\270\216 RLM runtime.md"
M	docs/issues/adapter/20260818-grok-pi Todo OMP agent-loop discipline.md
A	"docs/issues/adapter/20260821-recap-btw\346\241\245\346\216\245\346\266\210\346\201\257\344\270\215\346\263\250\345\205\245agent-context.md"
A	"docs/issues/adapter/20260821-\346\213\206\345\210\206\351\253\230\344\273\267\345\200\274 Pi \346\211\251\345\261\225\346\250\241\345\235\227.md"
A	"docs/issues/adapter/20260822-grok-pi Subagents V2 Team \345\215\217\344\275\234.md"
A	"docs/issues/pager/20260822-Write-Edit\346\202\254\345\201\234\345\274\271\347\252\227\351\251\273\347\225\231\344\270\216\346\273\232\345\212\250\351\200\211\346\213\251.md"
A	docs/usage/subagents-v2.md
A	docs/usage/subagents-v2.zh-CN.md
M	extensions/pi-grok-auth/index.ts
A	extensions/pi-grok-auth/login.ts
A	extensions/pi-grok-auth/logout.ts
A	extensions/pi-grok-auth/providers.ts
A	extensions/pi-grok-auth/runtime.ts
A	extensions/pi-grok-auth/shared.ts
A	extensions/pi-grok-auth/tsconfig.json
M	extensions/pi-grok-bash/README.md
M	extensions/pi-grok-bash/bash-tasks.ts
M	extensions/pi-grok-bash/eval-tasks.ts
M	extensions/pi-grok-bash/eval.ts
M	extensions/pi-grok-bash/index.ts
M	extensions/pi-grok-bash/prompts.ts
M	extensions/pi-grok-bash/test-v2.1.mjs
M	extensions/pi-grok-bash/tool-bridge.ts
A	extensions/pi-grok-btw/bridge.ts
A	extensions/pi-grok-btw/context.ts
M	extensions/pi-grok-btw/index.ts
A	extensions/pi-grok-btw/models.ts
A	extensions/pi-grok-btw/shared.ts
A	extensions/pi-grok-btw/tsconfig.json
A	extensions/pi-grok-loop/command.ts
A	extensions/pi-grok-loop/control.ts
M	extensions/pi-grok-loop/index.ts
A	extensions/pi-grok-loop/scheduler.ts
A	extensions/pi-grok-loop/shared.ts
A	extensions/pi-grok-loop/tools.ts
A	extensions/pi-grok-loop/tsconfig.json
A	extensions/pi-grok-loop/typebox.d.ts
A	extensions/pi-grok-recap/args.ts
A	extensions/pi-grok-recap/clean.ts
M	extensions/pi-grok-recap/index.ts
A	extensions/pi-grok-recap/model.ts
A	extensions/pi-grok-recap/prompt.ts
A	extensions/pi-grok-recap/session.ts
A	extensions/pi-grok-recap/shared.ts
A	extensions/pi-grok-remote-tui/demo.ts
A	extensions/pi-grok-remote-tui/env.ts
A	extensions/pi-grok-remote-tui/host.ts
M	extensions/pi-grok-remote-tui/index.ts
A	extensions/pi-grok-remote-tui/layout.ts
A	extensions/pi-grok-remote-tui/shared.ts
A	extensions/pi-grok-remote-tui/transport.ts
A	extensions/pi-grok-remote-tui/tsconfig.json
A	extensions/pi-grok-rollback/bridge.ts
M	extensions/pi-grok-rollback/index.ts
A	extensions/pi-grok-rollback/journal.ts
A	extensions/pi-grok-rollback/rollback.ts
A	extensions/pi-grok-rollback/shared.ts
A	extensions/pi-grok-rollback/store.ts
A	extensions/pi-grok-rollback/tsconfig.json
A	extensions/pi-grok-shortcut-manager/commands.ts
A	extensions/pi-grok-shortcut-manager/config.ts
A	extensions/pi-grok-shortcut-manager/dispatch.ts
A	extensions/pi-grok-shortcut-manager/host.ts
M	extensions/pi-grok-shortcut-manager/index.ts
A	extensions/pi-grok-shortcut-manager/shared.ts
A	extensions/pi-grok-shortcut-manager/tsconfig.json
A	extensions/pi-grok-subagents/bridge.ts
A	extensions/pi-grok-subagents/config-ui.ts
A	extensions/pi-grok-subagents/definitions.ts
M	extensions/pi-grok-subagents/index.ts
A	extensions/pi-grok-subagents/runtime.test.ts
A	extensions/pi-grok-subagents/runtime.ts
A	extensions/pi-grok-subagents/shared.ts
A	extensions/pi-grok-subagents/teams.ts
A	extensions/pi-grok-subagents/teams/implementation.json
A	extensions/pi-grok-subagents/teams/research.json
A	extensions/pi-grok-subagents/teams/review.json
A	extensions/pi-grok-subagents/tools-v1.ts
M	extensions/pi-grok-subagents/tsconfig.json
A	extensions/pi-grok-subagents/v2.test.ts
A	extensions/pi-grok-subagents/v2.ts
A	extensions/pi-grok-todo/compat.test.mjs
M	extensions/pi-grok-todo/index.ts
A	extensions/pi-grok-todo/tsconfig.json
A	extensions/pi-grok-todo/typebox.d.ts
A	extensions/pi-grok-todo/v1.ts
A	extensions/pi-grok-todo/v2.ts
```

## 3. 当前未提交（staged）变更

### 3.1 F2 增加 Subagents V2 / Todo V2 开关

- `UiConfig` 新增 `pi_subagents_v2`、`pi_todo_v2`，默认均关闭。
- `HostFeatureManifest` 新增 `PI_SUBAGENTS_V2_SPEC`、`PI_TODO_V2_SPEC`。
- startup env 从单纯“启用时写 1”扩展为 `HostFeatureEnv { key, on, off }`，允许关闭时显式覆盖继承环境：Subagents V2 写 `0`；Todo V2 写回 `v1`。
- `grok-pi` 启动时遍历完整 manifest，为启用/关闭状态都计算 env override，避免 shell 中残留 `PI_GROK_TODO_VERSION=v2` 等变量绕过 F2 设置。
- README / FEATURE_MATRIX 中英文同步说明 F2 开关。

### 3.2 Session Review 键盘/树形交互增强

- Tree mode 增加稳定 `dir_key` 和 `collapsed_dirs`，支持目录折叠状态。
- `h` 折叠/回父目录、`l` 展开、`Enter` 切换目录；目录 glyph 使用 `▸/▾`；鼠标点击目录行切换折叠。
- 文件过滤期间强制展开目录，保证命中项可见。
- Preview 新增 `]` / `[` hunk 跳转；`.` / `,` 切换文件；`d` / `u` 半页滚动；`n` / `N` 留给搜索结果跳转。
- `?` 打开完整 keymap overlay，任意键或点击关闭。
- `BlockViewerPane` 新增 half-page scroll 与 diff hunk start/jump 支持。
- staged diff 同时增加相应 unit tests（tree fold、filter auto-expand、mouse fold、help overlay、hunk navigation）。

### 3.3 build.sh 行为变化

- 删除 build 开始阶段对 `PI_BIN` 可执行文件的 fail-fast 检查；Cargo/build 流程不再在该位置提前因 Pi executable 缺失退出。

### 3.4 Pi session HTML 忽略规则

- 误暂存的 `pi-session-2026-08-22T04-34-33-816Z_01a027bf-c2d8-72f0-873e-a1b3ba205311.html` 已从 Git index 删除。
- 根 `.gitignore` 新增 `/pi-session-*.html`，只匹配仓库根目录这类 Pi session HTML 导出。

### 3.5 staged 文件清单

```text
M	FEATURE_MATRIX.md
M	FEATURE_MATRIX.zh-CN.md
M	README.md
M	README.zh-CN.md
M	build.sh
M	crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs
M	crates/codegen/xai-grok-pager/src/views/block_viewer.rs
M	crates/codegen/xai-grok-pager/src/views/review.rs
M	crates/codegen/xai-grok-shared/src/ui_config.rs
M	crates/codegen/xai-grok-shell/src/host_features.rs
```

## 4. 合并后的功能影响

- **配置面**：Host features 逐步由 manifest 成为 SSOT；V2 subagents/todo 可直接从 F2 控制并可靠覆盖继承环境。
- **Agent 能力面**：Eval v2、Subagents V2、Todo V2、team collaboration 与 subagent transport 明显扩展。
- **Pager UX**：review modal、timeline、paste、question cancellation、hover popup 等交互连续增强。
- **跨平台性**：PSM session path/resume/search 做了 Windows/Unix 路径兼容修复。
- **维护性**：多个 bundled extension 从巨型 `index.ts` 拆为职责模块；grok-pi startup wiring 也从主文件抽离。

## 5. Push / Commit 前检查点

1. staged 的 Review UX 与 F2 V2 开关仍应跑最窄相关 Rust tests/check，再做完整 build/verify（若准备正式 push）。
2. `build.sh` 移除了 Pi executable 的前置检查，需确认这是有意行为，而不是临时调试改动。
3. 21 个本地 commit 跨越 Eval、Subagents、Todo、Adapter、Pager、文档等多个主题；若目标是可审查 PR，可考虑按功能簇拆分/整理后再 push。

## 6. Git 取证命令

```bash
git status --short --branch
git log --oneline @{u}..HEAD
git diff --stat @{u}..HEAD
git diff --name-status @{u}..HEAD
git diff --cached --stat
git diff --cached --name-status
```
