# 上游合并逐文件深度分析 — [37949780]（只读预演，未执行合并）

- **范围**：`07b2f7144fd5c5c9d3dd1966937a87852d2dbdb8..37949780c144e37df692e3d669051a21fec24f20`（8 个 `Synced from monorepo` 提交，2998 文件 +341,620/−228,519）
- **目标**：在真实合并前逐文件判定「怎么合才不破坏 grok-pi 的功能」。
- **性质**：本目录全部内容为只读分析产物（`git merge-tree` / 三方 `git merge-file` 预览），仓库未做任何合并、未改任何源码。
- **逐文件结论**：`B1.jsonl`(35) `B2.jsonl`(27) `B3.jsonl`(51) `B4a.jsonl`(31) `B4b.jsonl`(59) `B5.jsonl`(23) `B6.jsonl`(38)，共 **264 个文件**（171 个文本冲突 + 95 个自动合并但语义重叠，去重后 264）。材料可用 `regenerate_materials.py` 一键重建。

## 1. 结论（TL;DR）

**本次同步可以合并，但绝不能机械合并。** merge-tree 预演产生 **171 个文本冲突**（占双侧重叠 310 文件的 55%，上次同步为 0），另有 **58 个文件自动合并但双方改了同一区域**、37 个间距 ≤3 行。逐文件分析后：**21 个 P0**（机械合并必破坏 fork 功能）、81 个 P1、91 个 P2、71 个 P3。

最危险的不是冲突本身，而是四类**不报冲突的静默破坏**：

1. **上游删除了 fork 的 workflow 插件点**：`workflow/backend.rs`（`WorkflowAgentBackend` trait）与 `workflow/external.rs`（`ExternalWorkflowRuntime`）被上游删除，而 fork 侧**未修改**这两个文件 → 普通 merge 会**无冲突地自动删除**它们，`pi-grok-adapter` 随即编译失败；更隐蔽的是 `manager.rs`（自动合并，间距 3 行）若回退会让 workflow 重新走上游 subagent 通道而非 Pi。
2. **可编译但行为丢失**：`session_notification.rs`、`dispatch/session/lifecycle.rs`、`load.rs`、`modes.rs` 若取上游版都能通过编译，但会分别静默丢失 Pi 桥后台位修复、`/new` 替换语义、Pi 会话搜索路由、Normal↔Plan 二态限制。
3. **同名 API 语义分叉**：`Effect::Compact`（fork `custom_instructions`→Pi vs 上游 `user_context`→shell）必须双字段并存，否则 Pi 的压缩指令静默丢失；`SourceFilter::External`（fork 含 pi 源 vs 上游仅 foreign）、`close/dismiss/clear_block_viewer` 三种关闭语义、Esc 取消 vs Esc 提示、Shift+Tab vs Ctrl+Shift+T，都是"取错边不报错"的坑。
4. **`.grok-pi` 项目隔离接缝散布在约 14 个文件**（paths/hooks/sandbox/plugins/checkpoint/discovery/agent 等），机械取上游会把它们回退成硬编码 `.grok`，违反产品隔离不变量且无编译错误。

另有约 **3020 个上游独改文件可直取**（含全部新 crate：`xai-grok-login`、`xai-grok-feedback`、`xai-grok-otel`、`xai-grok-image`、`xai-grok-gboom`、bot-relay 生成绑定），无 fork 风险。

## 2. 方法与口径

- **文本冲突**：`git merge-tree --write-tree HEAD upstream/main`（权威三方预演，171 个冲突文件、578 个冲突块），每个冲突用 `git merge-file -p --diff3` 重建冲突标记后逐块判读。
- **语义重叠**：对 310 个双侧修改文件，把 fork 与上游的 hunk 映射到 base 坐标系求最小间距：gap=0（58 个，同区域重叠）≤3（37 个）≤15（32 个）>15（21 个）。
- **跨 crate 依赖**：对 fork 专属代码（`pi-grok-adapter`、`grok-pi` bin、extensions）逐一 grep 上游改名/删除的符号，并核对上游 tip 的定义位置。
- **verdict 语义**：`take-theirs/take-ours/union/manual-blend/keep-deleted/regenerate`；risk：P0=机械合并破坏 fork 功能，P1=需人工精细处理，P2=机械规则可解，P3=平凡。

## 3. 全量分类（上游 3340 个变更路径）

| 类别 | 数量 | 处理 |
|---|---:|---|
| A 上游独改（fork 未动） | 3020 | 直接取上游；新 crate 无冲突 |
| C 文本冲突（双方都改） | 159 | 逐文件结论见 B1–B6 jsonl |
| CR 文本冲突（经上游 rename） | 12 | 冲突落在 rename 后新路径，需移植 fork 改动 |
| B1 自动合并·同区域重叠（gap=0） | 58 | 自动合并能过，但需按 jsonl 核对双方逻辑 |
| B2 自动合并·间距≤3 行 | 37 | 同上，重点 manager.rs(workflow, gap=3) |
| B3/B4 间距 4–15 / >15 行 | 53 | 低风险，合并后抽查 |
| B0 二进制/脚本（npm bin、install 脚本） | 3 | 取上游，肉眼过一遍 fork 改动 |

## 4. P0 清单（21 个，机械合并必坏）

Pager ACP/集成心跳（11）：
`app/event_loop.rs`（上游 +1633/−1032 全重写且零 external 概念，fork 19 处门控须逐块重放）· `app/acp_handler/mod.rs`、`routing.rs`、`session_notification.rs`、`subagent_activity.rs`、`interactions.rs`（上游重构通知/attempt 体系撞 fork 嵌套子代与 Pi 桥修复）· `app/dispatch/dashboard.rs`（fork 单 Pi 会话宿主护栏 vs 上游 +1001/−732 重写）· `modes.rs`（fork external 仅 Normal↔Plan 二态 vs 上游 4 态环重写）· `prompt.rs`（fork active_child_sid/interject 嵌入上游重写的提交流）· `session/lifecycle.rs`、`session/load.rs`（fork /new 替换与 External→PiSessionSearch 路由）。

符号级破坏（6）：
`scrollback/entry.rs`（上游删 `hook_data` 字段，fork 7 文件仍用）· `slash/command.rs`（上游删 `HandledNoOp`，fork dispatch 两处仍匹配）· `app/effects/mod.rs`（Compact 双字段 + fork Pi RPC 臂）· `app/dispatch/turn.rs`（上游 `SubagentInfo`→`attempt.*` 重构撞 fork `descendant_subagent_owner_mut`）· `views/block_viewer.rs` + `views/block_viewer/mod.rs`（上游目录化重构 + 遥测，fork Kitty/并排 diff 能力需整体移植）。

视图双轨重造（3）：
`views/modal.rs`（fork SubagentHistory/通知列表 vs 上游删其依赖导入）· `views/timeline.rs`（同一侧栏两套并行重造：fork marker 轨道 vs 上游 TimelineRail，取上游丢 compaction 标记）· `acp/mod.rs`（上游拆 spawn 模块引入 AgentEndpoint，fork `AcpConnection::external()`/`ui_profile` 接缝必须重挂）。

Shell（1）：
`xai-grok-shell/src/session/workflow/host_service.rs`（fork `spawn_and_await` 路由 vs 上游 spawn 加参；配套第 5 节插件点恢复）。

## 5. 四个必须作为"一揽子工程"处理的合并专项

### 5.1 workflow 插件点恢复（Pi 拥有 workflow 运行 —— 架构不变量）
- 上游删除 `session/workflow/backend.rs`、`external.rs`（fork 未改 → merge 自动删除）；**必须显式恢复**（`git checkout 07b2f714 -- <两文件>`）。
- 同步保住：`workflow/mod.rs` 的 `pub use backend::{…}` / `pub use external::{…}`（自动合并后核对）、`session/mod.rs` 的 `pub mod workflow`（上游是 `pub(crate)`，B6 已标记勿回退）、`manager.rs` 的 `WorkflowAgentBackend` 字段与 `request_cancel_run_children`、`host_service.rs` 的 `spawn_and_await` 路径、`acp_session_impl/spawn.rs` 的 backend 实参（与上游 `active_work` 新参并存）。
- 上游 `SubagentRequest` 新增 `spawn_root` 且 backend 引用移至 `xai-grok-tools` task 模块 → fork `backend.rs` 构造处需补字段。
- 外部依赖方 `pi-grok-adapter`（`pi_workflow_backend.rs`、`workflow_host.rs`）只经这层 trait 对接，接口不回退则无需改 adapter。

### 5.2 上游删除的 fork 在用公共符号（回植清单）
| 符号 | 上游动作 | fork 使用点 | 处理 |
|---|---|---|---|
| `hook_data`（scrollback Entry） | 删字段 | tracker/mouse/state/renderer/agent.rs/entry_renderer.rs | 拒删（B3 entry.rs P0） |
| `should_show_model_fingerprint` / `is_coding_model_slug` | 删函数 | pager `effects/mod.rs:5694`、`slash_exec.rs:321` | 回植（B6 acp_types.rs） |
| `CommandResult::HandledNoOp` | 删变体 | `dispatch/prompt.rs:651`、`dashboard.rs:1553` | 保留变体或同步改 dispatch（二选一，跨文件联动） |
| `xai_grok_shell::auth::AuthManager` | 整体迁 `xai-grok-login` | pager `acp/mod.rs` 构造器 `new_without_startup_diagnostics` | 调用点改 `xai_grok_login`，免诊断开关在 login/manager.rs 重挂 |
| `jump_to_turn` | 上游仍保留，fork 改 `jump_to_entry` | 上游 router/mouse/timeline 3 处调用 | 留薄包装防断 |
| recap 链路（`recap_model`/`side_model_chain`/`summary_body`） | 上游 setters/ui 已无 recap | notes.rs、settings、adapter `recap_bridge` | fork 全链保留（上游 session_event 仍用 recap_summary，可共存） |

### 5.3 同名语义分叉（取错边不报错）
- **Effect::Compact 双字段**：fork `custom_instructions`（→Pi，adapter `pi_adapter/session.rs:674` 消费）+ 上游 `user_context`（→上游 shell）。涉及 actions.rs、effects/mod.rs、dispatch/queue.rs、dispatch/tests/prompt.rs。
- **SourceFilter::External**：fork 含 pi 源（session/load.rs、session_picker.rs 取 fork 口径）；上游测试按 foreign-only 写，勿采纳。
- **block viewer 关闭三态**：fork `close_block_viewer`（清 Kitty 覆盖层）vs 上游 `dismiss_block_viewer`（存 resume）/`clear_block_viewer`。统一映射：上游语义 + 补 Kitty 清理。涉及 mode_switch、viewer.rs、interactions.rs、mouse.rs、app_view.rs、block_viewer×2。
- **Esc 策略**：fork `ESC_CANCELS_TURN=true`（可配置 cancel_turn_key）vs 上游"Esc 永不取消只提示"。actions/defaults.rs、event_loop.rs、app_view_tests 取 fork。
- **Shift+Tab**：fork 重映射为平台 Plan 弦（Ctrl+Shift+T）且 Shift+Tab=CycleThinkingLevel(thinking)；上游新增 Shift+Tab cycle-mode 测试须按 fork 键位改写（dashboard/state(_tests)、dispatch/tests/dashboard、plan_nudge）。
- **SubagentInfo → attempt.*/attempt_id**：上游数据模型改名，fork 的嵌套子代树（`descendant_view_for_live_update_mut`、`from_subagent_tree`、`subagent_tree` 行）必须映射到新字段而非回退扁平行。

### 5.4 `.grok-pi` 项目隔离接缝（约 14 文件，禁止回退 `.grok`）
`config/paths.rs`（`project_config_dir*`，公共前置；`grok_home` 上游改由新 crate `xai-dirs` 再导出）· `xai-grok-agent/discovery.rs` · `plugins/discovery.rs` · `shell/config/watcher.rs` · `shell/util/hooks.rs`（与上游 `classify_grok_hook_source` 合成）· `sandbox/profiles.rs`（与 `SANDBOX_CONFIG_FILENAME` 合成）· `tools/types/compat.rs`（strum 重构中回植）· `tools/…/workflow/mod.rs` 描述 · `workspace/folder_trust.rs`（用上游 `crate::util::path_present_or_uncertain` 包 fork 路径）· `workspace/permission/resolution.rs`（上游 −1234 重构中回植 `find_project_grok_configs`）· `workspace/session/checkpoint_store.rs` · `workspace/discovery.rs`（上游签名加 `project_trusted`，fork 调用点补实参）· `views/import_claude_modal.rs`（P1，取 fork `project_config_dir`）· `shared/ui_config.rs`（`PiBuiltinTools` fork 专有，`agent/config.rs` re-export 勿丢）。

## 6. 跨文件联动组（必须同一次提交内一起解）

1. **Compact 双字段**：actions.rs ↔ effects/mod.rs ↔ dispatch/queue.rs ↔ dispatch/tests/prompt.rs ↔（消费方）pi-grok-adapter。
2. **block viewer 三态**：mode_switch.rs ↔ agent_view/viewer.rs ↔ acp_handler/interactions.rs ↔ mouse.rs ↔ app_view.rs ↔ block_viewer(.rs/mod.rs)。
3. **`only_unused_home_or_empty`**：app_view.rs 必须采纳上游新增方法，否则 foreign_sessions.rs 的上游 hunk 编译失败。
4. **workflow 插件点**：见 5.1（backend/external 恢复 + mod.rs×2 + manager + host_service + spawn + adapter）。
5. **auth→login**：login/manager.rs 重挂开关 ↔ pager acp/mod.rs 调用路径。
6. **input/ 模块迁移**：key.rs、keyboard_normalizer.rs → `xai-grok-pager-render/src/input/`，fork `is_force_attachment_paste_key`（被 interactions.rs:372、prompt.rs:708 调用）与 `mac_option_glyph_to_letter`（extension_shortcuts.rs:279）随迁并改导入路径；上游新版 rescue_key 带 `os_modifier_rescue_suppressed()` 门控需保留。
7. **SessionInfoResponse 新字段**：`agent/handlers/session.rs` 补 4 个 None ↔ `acp_types.rs` 新增 Option 字段，缺一编译失败。
8. **dispatch/mod.rs re-export**：上游 `abandon_unused_home_session`/`maybe_create_home_session` 来自其 lifecycle.rs 改动，必须随上游实现一起进来。
9. **settings 注册表**：`defs.rs`/`registry.rs`/`settings_writes.rs`/`ui_config.rs` 四处按键并集（fork pi:* 键 + 上游 export_copy/terminal 主题；上游删 `scheduler_background_loops` 需确认 fork 测试同步）。
10. **modal 签名**：上游 `filter_palette_entries` 等改参，modals.rs 15 处 fork 调用点逐块适配。

## 7. 测试族战略决策点（需要用户拍板）

上游把 `xai-grok-pager/tests/pty_e2e/` 整体迁入新 crate `xai-grok-pager-pty-harness/tests/pty_e2e/`（约 50 个条目），fork 曾在 a8906d9c 决定保留在旧路径（Cargo.toml 注释 "modules remain under tests/pty_e2e/"）。两批代理给出相反策略：

- **方案 A（推荐）**：接受上游迁移，把 fork 的行为性测试改动移植到新位置（`common.rs` 的 `wait_for_turn_idle`/`wait_until_stable`/排空 helper、Shift+Tab→thinking 断言、screen-mode 持久化裁剪、minimal sticky 用例、`minimal/mod.rs` 模块清单并集）。理由：旧路径每次同步都会再打一架；上游 pty-harness 仍在演进。
- **方案 B**：拒绝 rename，把上游内容改动映射回旧路径（fork Cargo.toml `[[test]]` 保持）。工作量小，但后续同步持续冲突。
- 无论哪个方案：`edit_merge_parallel/sequential_pty.rs` 维持 fork 删除（行为已移植为 `acp/tracker_tests.rs` 单测）；`minimal_cli_screen_mode_does_not_persist.rs` 维持删除（与 fork 持久化行为相反）；`basename_path_demo_pty.rs` 保 fork 版（上游真删未迁移）；`acp/tracker_tests.rs` 双方 +274/+777 行扩编必须并集（ACP seam 保障网）。

## 8. fork 专属代码跨 crate 依赖核查（主会话独立验证）

- `pi-grok-adapter` **不依赖** `xai-grok-pager`（仅 ACP 协议耦合）→ Pager 内部改名不波及 adapter。
- adapter → `xai_grok_shell` 仅两个面：`session::persistence::PersistenceMsg`（上游枚举完好）与 `session::workflow::{WorkflowAgentBackend, ExternalWorkflowRuntime, WorkflowRunStore, workflow_session_notification_json, …}` —— 后者正是 5.1 的插件点，**唯一真正的跨 crate 断点**。
- adapter 的 `custom_instructions` 是自有 ACP wire 字段（`pi_adapter/session.rs:674`），与 5.3 的 Compact 双字段互证。
- fork 专属代码对上游改名符号（`hook_data`/`jump_to_turn`/`HandledNoOp`/`screen_mode_switch_hint`/`push_end_marker_block`/`SubagentInfo`/`descendant_view`/`close_block_viewer`）grep 全部 0 命中 —— 这些风险都在 pager crate 内部的 fork seam 文件里，已由批次分析覆盖。
- `grok-pi` bin 依赖 `xai-grok-config::paths`（`project_config_dir*`，上游改 xai-dirs 再导出，paths.rs 为 union 低风险）与 `shell::session::workflow` pub 可见性（`session/mod.rs` 勿回退 `pub(crate)`）。

## 9. 建议合并顺序（授权后执行）

1. **预备**：当前 HEAD 跑基线验证（build + adapter/pager-bin 测试绿）；隔离 worktree。
2. `git merge upstream/main` → 171 冲突。
3. **结构层**：3 个 Cargo.toml（workspace/pager/pager-bin/update）取并集 → `cargo generate-lockfile` 重生成 Cargo.lock → 新 crate（login/feedback/otel/image/gboom/pty-harness）进 members。
4. **恢复插件点**：`git checkout 07b2f714 -- crates/codegen/xai-grok-shell/src/session/workflow/{backend,external}.rs`，随后按 5.1 联动组接线。
5. **公共前置**：paths.rs/xai-dirs、`.grok-pi` 接缝 14 文件（5.4）。
6. **按域解冲突**：B6（shell/workspace/tools）→ B1/B2（pager app 核心 P0）→ B3（views/scrollback）→ B4a/B4b（其余 + input 迁移）→ B5 测试。
7. **测试族**：按第 7 节拍板执行。
8. **语义自检**：grep 审计 `external_agent` 门控数量、`project_config_dir` 无 `.grok` 回退、Compact 双字段、workflow backend 路由。
9. **验证门槛**（AGENTS.md）：`./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi` → `test -p pi-grok-adapter` → `test -p xai-grok-pager-bin --bin grok-pi` → `./build.sh` → `./verify.sh`（已知 blockers 单列）。
10. **基线维护**：source-identity verifier 允许 seam 更新（input/、search/ 迁入 pager-render、slash_meta!、block_viewer 目录化）→ 更新 `SOURCE_REV=c4ea71cf…` 与 AGENTS.md `base=37949780…`（仅在验证完成后）。

## 10. 残留不确定项

- B4/B3 批次中 gap 4–15 的 32 个文件仅做了 hunk 级判定，合并后建议 `cargo test` 兜底。
- `xai-grok-pager/npm/grok/bin/grok` 与两个 install 脚本（B0，numstat 视为二进制）未逐行比对，取上游前肉眼过一遍。
- 上游对 `manager.rs`(workflow) 的 `request_service` 新体系与 fork backend trait 的长期共存策略（上游未来可能彻底移除 trait 通道）建议在合并后的 issue 记录中注明。
