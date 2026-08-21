# 更新日志（中文）

**grok-pi**（在 Grok Build 生产级 TUI 中运行 Pi Agent Core）的版本说明。

- 英文完整版（含历史版本）：[CHANGELOG.MD](CHANGELOG.MD)
- 格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)

---

## [Unreleased]

### 新增

- Eval Bridge v2 新增语言选择器：`[ui].pi_eval_v2_language = "js" | "py" | "all"`（默认 `js`，需重启）；Python 与 JavaScript 共用同一套 host-RPC、skills、completion、store/load 与 task 契约。
- Eval v2 后台任务与 Bash 对齐：支持显式后台、达到共享最大等待阈值后自动由前台转后台、`get_task_output` / `wait_tasks` / `kill_task`、模型输出限长与完整输出临时文件。

### 修复

- Recap 与 `/btw` 桥接流量不再泄漏进 agent loop context：两个扩展的摘要、delta 和答案一律通过 `appendEntry` 写入 custom entry（不再产生 custom message），adapter 解析对应的 `entry_appended` 事件，并由 `context` hook 把旧会话中 sendMessage 时代的遗留条目从 LLM context 中剔除。
- `tools.describe(name)` 在 JavaScript REPL 中会深层展示嵌套 tool schema，不再把 `properties` 折叠为 `[Object]`。
- Bash `timeout: 0` 现在统一表示“不设置超时”，与 Eval v2 的 escape hatch 语义一致；嵌套 `tool.bash(...)` 不再因 0 值触发校验失败。
- Eval v2 前台任务自动转后台后会立即补充新的前台 kernel，后续 cell 不再被已转后台任务阻塞。

### 变更

- Eval v2 `agent()` 明确为 blocking leaf；`background=true` 会快速失败，并发 leaf agent 通过 `parallel([...])` 执行。
- Eval v2 任务状态复用增强 Bash 相同的 Pager 原生 task channel 与统一等待/输出限制。

## [0.1.0] - 2026-08-21

范围：`v0.0.18` → `v0.1.0`（2026-08-18 → 2026-08-21）。Tag `v0.1` 与 `v0.1.0` 指向同一发布提交。

### 亮点

- **Pi 内置工具编排** — F2 与 resource policy 统一协调 Bash、Eval、Eval v2、Todo 及相关注入工具，不再依赖零散扩展准入。
- **Eval Bridge v2 基础能力** — 落地 host-RPC 设计、bundle 验证 harness、Bash/Eval 托管任务 runtime 与原生 Eval 展示改进。
- **可交互子代理** — Pager 子代理行补充生命周期投影与交互式 session 行为，同时继续由 Pi 拥有执行语义。
- **文件与 Skill 工作流增强** — 扩展文件搜索/预览、Ctrl-L 行内查看 `SKILL.md`、prompt/paste 处理及相关 Pi 文件交互。

### 新增

- Eval Bridge v2 设计记录、runtime 模块、demo/验证 harness、host-tool bridge、任务管理、prompt helpers 与文档。
- Pi 内置 Bash/Eval/Todo/tool policy 的 F2 控制与启动接线。
- 原生 Todo extension 与 grok-pi config skill 支持。
- 交互式子代理 session 表面、生命周期 metadata 与 background/idle-barrier 处理。
- Ctrl-L 行内 `SKILL.md` 预览以及更多文件搜索/粘贴交互路径。

### 修复

- Pi Bash 扩展注入现在会完整物化模块依赖闭包，避免 `Cannot find module './eval-tasks.ts'` 一类启动失败。
- Remote TUI 导航键不再通过 bridge 重复发送长按事件。
- Session/subagent 投影、prompt dispatch 与工具渲染针对扩展后的 Pi runtime 表面进一步加固。

### 变更

- Bash 与 Eval 卡片采用更清晰的 Pager 原生格式和状态展示。
- Session-info usage 与多处 settings/modal 表面随新工具 runtime 控制一起打磨。
- 仓库指南补充 Pi RPC bootstrap / extension failure 的权威诊断流程。

## [0.0.18] - 2026-08-18

范围：`v0.0.17` → `v0.0.18`（2026-08-16 → 2026-08-18）。

### 新增

- 扩展 adapter cache metrics 与 session 信息，供 Context / Usage 原生表面使用。
- 补充 GoalHost 状态与 notification 处理，使原生 goal/status 投影更完整。
- 新增 tag 监控相关 release automation。

### 修复

- 加固 Pager ↔ adapter 边界上的 Context/Usage modal、cache graph 交互与 session-event 展示。
- 调整 user/session event block 与 scrollback state，以适配更新后的 session metadata 投影。

### 变更

- Context cache graph、usage modal、ACP session metadata 与 goal/status plumbing 同步扩展，保持 Pager 原生表面与 Pi 所有的 session state 对齐。

## [0.0.17] - 2026-08-16

范围：`v0.0.16` → `v0.0.17`（2026-08-15 → 2026-08-16）。

### 修复

- Linux ARM64 release 改为在 arm64 runner 上原生编译，不再从共享 16 GB x64 runner 交叉编译。此前大型 `release-dist` profile（`codegen-units=1` + thin LTO）会因 OOM 以 exit 143 终止，导致 `v0.0.16` 缺少 Linux ARM64 asset。

## [0.0.16] - 2026-08-15

范围：`v0.0.15` → `v0.0.16`（2026-08-09 → 2026-08-15）。

### 亮点

- **Pi 运行时自愈** — Pi RPC 子进程意外退出或心跳卡死后，grok-pi 可自动恢复、重新挂接当前 session，并收敛原本会悬空的后台 Bash 状态。
- **grok-pi 原生设置面板** — F2 改为打开分栏、可搜索的 grok-pi 设置面板，继续复用上游同一套 settings registry 与 Action。
- **更稳的上游整合** — 同步 Grok Build 至 `e5fd481`，同时恢复合并时丢失的 Pi-Grok 测试适配、Edit coalescing 与 `/fork` 选择器输入接缝。

### 新增

- 按类别/分区组织的 F2 标签页设置面板，支持跨标签搜索与固定描述区。
- 大幅扩充内置 Pi theme 目录并刷新主题资源。

### 修复

- Pi RPC 意外退出与 heartbeat 卡死进入有上限的自动恢复；主动 teardown 仍不会误触发恢复。
- 后台 Bash 终态通过独立状态通道投影并去重，RPC 丢失后完成/孤儿任务不再永久保持运行动画。
- cancel idle probe 在 `await` 后重新确认 ownership，旧取消快照不再覆盖刚启动的 Goal successor turn。
- 恢复 `/fork` 选择器键盘与鼠标输入路由，包括 Enter/Esc、`j`/`k`、Ctrl+C 取消逃生口与 Ctrl+Q 放行。
- 恢复上游抽离测试模块中的 grok-pi 适配，并重新接上 Edit coalescing 调用点。
- 适配本轮上游同步带来的 theme、event loop、settings 与 workflow API 变化。

### 变更

- Grok Build 同步至 `e5fd481`，继续保留已声明的 Pi-Grok 窄接缝与产品隔离规则。

## [0.0.15] - 2026-08-09

范围：`v0.0.14` → `v0.0.15`（2026-07-31 → 2026-08-09）。

### 亮点

- **持久 BTW 历史** — 成功的 `/btw` 答案保存为 Pi 自有 custom entry，`/btw-history` 无需再次调用模型即可把 active branch 投影到原生 Pager scrollback。
- **更易读的用户消息** — grok-pi 用户消息默认使用 Markdown 渲染，F2 可即时切换，同时支持持久化 prompt cursor 设置。
- **更安全的开发构建** — 共享 Cargo 输出加入受控并行、过期增量缓存维护和可配置剩余空间门禁。

### 新增

- Active branch BTW 历史回放，包含问答去重、时间、request identity 与实际使用模型信息。
- external-only `[ui].pi_user_markdown`，默认开启；关闭后恢复经典可折叠纯文本。
- 持久化 prompt cursor 预设，或经过校验的单列自定义字符。
- Cargo 磁盘门禁与维护脚本，统一用于 build、verify、绑定生成和 stop hook 检查。

### 修复

- 取消流程抑制残留队列 continuation 事件，并等待连续稳定 idle 后再结束 Pager 状态。
- BTW 与 recap 请求跨 ACP 边界时保留配置的模型链和扩展参数。
- Session preview 滚动持久保存已钳制的底部偏移，鼠标滚轮后不再回弹。
- Pi skill discovery 继续归 Pi 所有，skill 设置与 `/reload` 无需重启 grok-pi 即可生效。

### 变更

- 整合 Grok Build `a422116`，同时保留已声明的 Pi-Grok 接缝。
- 项目文档与验证命令统一改用带门禁的共享 Cargo wrapper。

### 修复（2026-08-09）

- Pi RPC 子进程改由独立 exit-coordinator 通道持有，`kill()` 不再因 `Child::wait()` 阻塞关停。
- 每次 Pi bootstrap 都受 60 秒 deadline 约束，扩展启动卡死不再挂起 grok-pi；启动自愈已写入 `--help`。
- CLI 工具解析支持 `--tools=/--exclude-tools=`，且仅在无显式工具覆盖时才注入 tools 扩展。
- 恢复 review 弹窗的键/鼠标/粘贴路由（上游合并曾丢失 `handle_review_key`），Esc/q 关闭与点击聚焦恢复可用。

### 变更（2026-08-09）

- 默认关闭增量编译，共享 Cargo target 硬上限 64 GiB（`CARGO_TARGET_MAX_GIB`）；maintenance 先清遗留 incremental 缓存，超限再回退 `cargo clean`。磁盘门禁实时执行上限，单次 Cargo 调用无法撑爆共享 target。

## [0.0.14] - 2026-07-31

范围：`v0.0.13` → `v0.0.14`（2026-07-30 → 2026-07-31）。

### 亮点

- **Pi 原生交互打磨** — 支持运行时模型映射、强化 ask-user 响应、Context 会话成本、工作流目录稳定性、可配置 Thinking 边框及提示词光标。
- **Remote TUI 对齐** — `ctx.ui.custom()` 默认继续以内联方式呈现，并支持 Pager 原生 overlay、尺寸/位置元数据以及 Kitty 重复/释放按键序列。
- **EditTool review** — external-only F2 开关可在宽终端并排显示 EditTool diff，窄终端自动回退 unified；code review 保持 unified 双 gutter 布局。

### 新增

- EditTool 并排渲染：old/new 两列、`-`/`+` 标记、全屏 viewer、patch-copy 保留，以及默认关闭的 **Side-by-side edit diffs** 设置。
- Remote TUI overlay 定位、宽高约束、锚点、偏移和重复/释放输入转发。
- 可配置 Thinking 边框颜色和提示词光标外观。
- Context 信息中的会话成本和运行时 Pi 模型映射。

### 修复

- Pi `ask_user` 响应处理，以及失效/重复的会话 handler。
- 目录 reload 期间的工作流可见性，以及意外准入 `pi-open-tui` renderer。
- Native verifier 对声明的 EditTool renderer 接缝进行准确计数。

### 变更

- 同步 Grok Build 至 `dd04f39`，同时保留 Pi-Grok 的原生 Pager 接缝。

### 说明

- EditTool 并排 diff 由 Pager 所有，仅 external-only、进程内生效且默认关闭；窄终端和 code-review 表面继续使用原生 unified renderer。

## [0.0.13] - 2026-07-30

范围：`v0.0.12` → `v0.0.13`（2026-07-28 → 2026-07-30）。

### 新增

- **Q&A 桌面通知** — 已启用的原生 `ask_user_question` 在 grok-pi 失焦时抵达，Pager 会尽力发送原生桌面通知。F2 → Agent → **Q&A desktop notifications** 可即时控制，默认开启，且不影响 Q&A 工具准入开关。

### 修复

- **外部 ACP 启动噪声** — Pager 所有的认证管理器不再为 grok-pi 产品隔离的 external profile 记录预期缺失的 Grok 认证文件诊断。

---

## [0.0.12] - 2026-07-28

范围：`v0.0.11` → `v0.0.12`（2026-07-25 → 2026-07-28）。

### 亮点

- **原生 Pi 模型管理中心** — 在 Pager 弹窗内管理 `models.json`，保存后热更新 Pi，无需重启会话。
- **产品导览与 Herdr** — grok-pi 专属 18 篇导览，以及可选启用的原生 Herdr 生命周期桥接。
- **更安全的产品边界** — 仅当 recap 桥接实际加载时才声明该能力；未加载的桥接命令会明确报错。

### 新增

- **`/pi-models`**（别名：`/model-config`、`/models-config`）：原生 Provider → Model → Detail 三栏管理 Pi `models.json`，支持搜索、新建/克隆/编辑/删除、校验、外部修改冲突检测、备份与恢复。保存复用 Pi 官方 reload；激活模型仍走 typed ACP `session/set_model`。
- **grok-pi 教程 profile**：`/tutorial`、`/tour`、`/onboarding` 现在提供 18 篇产品专属内容，覆盖 Pager 原生表面、Pi 能力、可选桥接及边界，不再复用 stock Grok 文案。
- **Herdr 生命周期集成**：F2 中可控制、需重启的 **Pi Herdr integration** 注入宿主拥有的扩展，上报根 Pi 会话身份及 working/blocked/idle 状态；在 Herdr 外无副作用，`[ui].pi_herdr = false` 可关闭。
- **子代理会话隔离**：子代理 session 文件创建在父 session 目录下的 `subagent/` 树中。

### 修复

- **Recap 与桥接命令** — 仅当注入扩展存在时声明 session recap；拒绝调用未加载的桥接命令，并阻止并发 recap 请求。
- **Thinking 流式渲染** — 剥离完整 ANSI 控制序列，并跨 chunk 保留未完成序列，避免终端转义码泄漏到 Thinking 文本或 Rust fence 中。
- **启动噪声** — 不再向 stderr 打印成功的 Pi host 版本检查。

### 变更

- 依照“先 changelog、后隔离同步”的流程整合 Grok Build `47348d1`；保留 Pi-Grok 窄接缝，并为 linked worktree 复用 Cargo target。
- README、功能矩阵、架构记录及中英文 Herdr 使用指南同步说明新产品表面与可选启用策略。

### 说明

- 模型管理中心刻意不伪造 enabled/disabled 状态：模型可用性和认证仍归 Pi 所有。
- Herdr 与 recap 桥接的扩展准入设置变更后，需要完全重启才能生效。

---

## [0.0.11] - 2026-07-25

范围：`v0.0.10` → `v0.0.11`（2026-07-25）。

### 修复

- **发布完整性** — 纳入 bash run-display 集成所需的本地 Pager appearance、settings、router 与 renderer 源码，确保所有发布目标能从 tag checkout 完整编译。

## [0.0.10] - 2026-07-25

范围：`v0.0.9` → `v0.0.10`（2026-07-24 → 2026-07-25）。

### 修复

- **会话替换崩溃** — shortcut-manager 不再在会话重载、fork 或切换后，通过延时回调保留失效的 Pi extension context。
- **Pi RPC 诊断** — 完整子进程 stderr 追加写入 `$GROK_HOME/logs/pi-rpc-stderr.log`，终端错误表面窄时仍保留未裁切的 Node stack trace。

## [0.0.9] - 2026-07-24

范围：`v0.0.8` → `v0.0.9`（2026-07-22 → 2026-07-24）。

### 亮点

- **透明主题波浪 accent 恢复** — 工具运行 / Thinking 左侧 `┃` 呼吸动画在 `pi:transparent` 等主题下不再冻成静态色
- **会话表面** — Context 缓存图、`/review-session` / `/review-message`、会话树地图
- **原生桥接（F2，多数默认关）** — 原生问答 QuestionView、`/btw`、`/loop` 调度
- **Adapter 对齐** — 每条 ACP 通知打 `promptId`；bash/Execute 中途 `output_delta` 流式输出
- **上游** — 合并 Grok Build `a5727c5` 并保留 Pi-Grok 窄接缝；合并后丢失接缝已回补
- **Windows / 多架构安装** — 可靠解析 Pi host shim；安装与 Release 覆盖 macOS / Linux / Windows 的 x86_64 + aarch64

### 新增

#### Context、Review、树

- Context 弹窗 **缓存图**（F2 `[ui].pi_cache_graph`，默认 **开**）：adapter 从 Pi `get_entries` 投影 `cacheMetrics`；视图 `0/1/2/3`，`s` 排序，`e` 导出，`r` 刷新 — 不走 `ctx.ui.custom`
- **`/review-session`**、**`/review-message`**：原生 Pager 审查弹窗（文件列表 + BlockViewer diff）；F2 `review_file_tree` 默认 **关**；弹窗内 `t` 切换树形
- 会话 **树地图** 表面，便于分支方位（与既有 Session Tree 导航并存）

#### 扩展桥接（F2 / 注入，多为可选）

- **原生问答** — F2 `[ui].pi_ask_user_question`（默认 **关**，需重启）：`ask_user_question` → `x.ai/ask_user_question` → 原生 QuestionView；控制目录回写答案。冲突包见 `assets/native_feature_conflicts.toml`（可用 `$GROK_HOME` / 项目目录覆盖）
- **`/btw`** — F2 `pi_btw`（默认 **关**）：旁路提问经 adapter `x.ai/btw` + `pi-grok-btw`（不映射 juicesharp 覆盖层）
- **`/loop` 调度** — F2 `[ui].pi_loop`（默认 **关**，需重启）：`scheduler_create` / `delete` / `list` → 原生 `ScheduledTask*` / tasks pane；仅会话内（无持久 loop 子代理）
- Slash **`getArgumentCompletions`** 桥接：扩展命令（如 `/gapp`）可填充 Grok 参数下拉；`/model` 补全与 Pi `provider/id` 对齐
- 实验性 **rust-tui bridge**（本 tag 仅注释清理）；shortcut-manager / remote-tui 快照归档至 `extensions/_archived/`

#### Adapter / 队列 / 工具流式

- 每条 live ACP **`SessionNotification._meta` 打上客户端 `promptId`**，Pager 的 prompt-id gate 与 turn 铬条与 stock Grok shell 一致
- 主 `session/prompt` 时 **固定 `runningPromptId`**（`QueueMirror::set_running`）；在首个 Pi 事件前再广播，便于队列 adoption
- Pi 递增全文 **`partialResult` → `BashOutput.output_delta`**，Run/bash 卡片中途流式刷新，而非仅结束时跳变

#### 资源、遥测、网站

- 项目级 **resource policy** 与崩溃自愈报告路径
- **`tools/ext-crash-telemetry`**：扩展崩溃上报 CLI + Cloudflare Worker + dashboard（可选运维工具）
- 网站：**静态导出** 部署 GitHub Pages；`basePath` 下 `/docs` 链接可用；中英文档字典扩充

#### 平台

- Windows：将裸 `pi` / `pi.cmd` 解析为绝对路径（PATH + pi-node/npm）；经 `cmd.exe` 拉起 `.cmd`；版本探测后回写 `args.pi_bin`
- 安装与 Release：macOS / Linux / Windows × x86_64 + aarch64

#### 上游

- 合并 Grok Build **`a5727c5`**；写入 `docs/upstream/UPSTREAM_CHANGELOG.md`；验证后更新 AGENTS `base`
- 合并后 **窄接缝回补**（render / effects / shortcuts / shell ops 等）

### 修复

#### 透明主题波浪 accent（用户可见回归）

- **根因：** 透明 / 终端原生主题将 `Theme.bg_base` 设为 `Color::Reset`。运行中 accent 调用 `blend_color(bg, accent, wave_brightness)`；旧实现对 `Reset` 返回 `None`，调用方 `unwrap_or(accent)` → **每帧同一实色**（主观「完全没有呼吸」）
- **修复：** `blend_color` 仅在插值时将 `Reset` 映射为合成深色 canvas `(0x12, 0x12, 0x18)`（页面仍透明，不强制铺不透明底）。命名 ANSI 色仍不可 blend
- **回归测试：** `test_blend_color_reset_base_keeps_wave`
- **附带：** `EntryRenderer` 在 `entry.is_running` 时，即使 block `accent()` 为 `None`（Collapsed 默认）也强制 `accent_running` 动画

#### 其他

- Resume：全文搜索、fork 树、预览模式、快捷键提示
- `a5727c5` 整合后的接缝回补
- GH Pages `basePath` 下文档链接
- rust-tui-bridge 注释噪声清理

### 变更

- FEATURE_MATRIX / README（中英）与 session tree、review、queue、问答、btw、loop、cache graph、notify 行为对齐
- 多行 info 通知优先 **scrollback `SystemMessage`**（对齐 Pi `showStatus`，避免仅 toast 丢失）
- 文档启动路径简化为 **`grok-pi` / `pi-grok`**
- `.gitignore`：本地 fabric mesh 运行态
- 上游流程：先 changelog，再隔离 merge + 窄接缝 reapply

### 说明

- 依赖注入扩展的 F2（**ask-user / btw / loop / workflows / goal**）开关后需 **完全退出并重启**
- 透明主题：波浪仅用合成 canvas 做明度调制，UI 仍保持宿主透明
- 排查笔记（可选）：`docs/investigation/breathing-animation-debug.md`
- 自 **0.0.8** 升级：无额外迁移；透明主题用户无需换主题即可恢复呼吸
- GitHub Release 说明默认仍从 **0.0.6** 起累计章节（`scripts/extract-changelog-section.py`）

---

## [0.0.8] - 2026-07-22

范围：`v0.0.7` → `v0.0.8`（2026-07-21 → 2026-07-22）。

### 新增

#### 产品状态隔离

- 默认用户目录改为 **`~/.grok-pi`**（`$GROK_HOME`），不再与 stock Grok 的 `~/.grok` 共用。
- 默认项目配置树改为 **`<repo>/.grok-pi`**（`$GROK_PROJECT_DIR`）；统一通过 `xai_grok_config::project_config_dirname()` / `project_config_dir` 解析。
- 启动时在任何库通过 `OnceLock` 固定 `grok_home()` 前注入 home 与项目目录名。
- **`grok-pi migrate-home`**：从旧 `~/.grok`（或 `$GROK_LEGACY_HOME`）复制 allowlist 文件，支持 `--status` / `--dry-run` / `--force` / `--from` / `--into` / 可选 `--include-auth`。
- 目标 home 为空且 legacy 有数据时安全执行一次 **自动迁移**，并写入 `.migrated-from-legacy` 标记。
- 默认**不双扫描** stock `~/.grok` / `<repo>/.grok`，保持真正的产品隔离。
- 无环境变量的单元测试仍保持 stock `.grok` 默认值，避免破坏上游风格测试。

#### Pi Workflows（Rhai，F2 默认关闭）

- 复用上游 **`xai-workflow`** 引擎与 shell orchestration；**SpawnBackend** 可插拔（默认 `Grok`，grok-pi 使用 `Pi` bridge）。
- Adapter 提供 `WorkflowHost`、`pi_workflow_backend`、ACP `x.ai/workflow/{launch,pause,stop}`、`x.ai/workflows/list` 与 `workflow_updated` 通知。
- 注入 extension + slash：`/workflow`、`/workflows`、`/create-workflow`（及命名脚本）；`__pi_workflow_*` bridge command 从目录中过滤。
- F2 **`[ui].pi_workflows`** 默认**关闭**，开启后需重启才能注入 extension。
- 项目 workflow 位于 `<repo>/.grok-pi/workflows`，用户 workflow 位于 `~/.grok-pi/workflows`。
- `/create-workflow` 是 Pager PassThrough 用户 prompt，不是 Pi skill。

#### Goal 模式（F2 默认关闭）

- F2 **`[ui].pi_goal`** 默认**关闭**，开启后需重启。
- 注入 extension：`/goal` + `update_goal` 工具 + control file。
- Adapter **`GoalHost`** 状态机投影为原生 `GoalUpdated`（`goal_detail` / status bar）。
- Active goal 通过 `agent_settled` follow-up 继续执行，不等同于 shell 的完整 multi-agent classifier/planner/strategist 栈。

#### 导出 / 分享

- 默认开启 **`pi-grok-export`**：`/export-html`（Pi HTML 或 `.jsonl` 路径）与 `/pi-share`（private gh gist + pi.dev viewer）。
- Grok `/export` 继续导出 Markdown transcript，不引入第二套 TUI。

#### Website & CI

- `website/` 新增营销站与文档站（Next.js 15，中英双语 landing + docs）。
- GitHub Actions **`Website`** workflow 仅支持 `workflow_dispatch`；执行 `npm ci` + `npm run build`，上传 standalone/static/public artifacts。
- Release workflow 引入 **Swatinem/rust-cache**，加速多平台构建。

#### Session picker 与主题

- External Pi session picker 增加 **全文搜索**（`Ctrl+F`），搜索页独立于 catalog，每次切换都会重置状态。
- 透明主题 `pi:transparent` / `pi:transparent-light`：代码块使用终端默认背景（`Color::Reset`）；canvas 背景为 Reset 时用户消息增加**左侧 accent bar**。
- 更新透明主题 JSON 与 Pi theme map（`theme/pi/map.rs`）的 code/user surface 映射。
- 同步补齐 session load、jump、modal、welcome 与 overlay list 相关 Pager app/view/dispatch 接线。

#### Adapter / Queue（隔离波次前）

- `pi-grok-adapter` 更新 model/RPC/session/queue bridge：扩展 session catalog 字段、让 queue mirror 对齐 Pager optimistic dequeue，并加固后续 workflow/goal host 所需的 RPC client/session 路径。

#### 上游与工具链

- 同步 Grok Build **`3af4d5d`**（`SOURCE_REV` `0f4d7c91`）：包含新 **`xai-workflow`** crate、workspace permission/security 重构（exec-risk、auto-mode、hardened shell）、shell 工作目录迁移、doctor/terminal-setup 表面与 prompt-queue batching；完整列表见 `docs/upstream/UPSTREAM_CHANGELOG.md`。
- 新增 **`upstream-changelog`** skill（`.pi/skills/upstream-changelog/`）并写入 3af4d5d 首条结构化记录；AGENTS.md 明确“两阶段同步：先 changelog，再 merge”。
- `pi-main` 子模块推进至 earendil-works/pi `main` 的 `a5afc3f1`；拒绝子模块中的本地 dirty RPC 修改，只允许干净 pin。
- Release CI 使用 **Swatinem/rust-cache**；release notes 由 `scripts/extract-changelog-section.py` 从 `CHANGELOG.MD` 生成（strict，可选 `--since 0.0.6` 累计段 + 安装 footer），再追加 GitHub 自动 commit 列表。

#### 文档 / 法务

- 新增 queue architecture redesign 研究：`docs/issues/queue-architecture-redesign.md`、`queue-redesign-feasibility.md`（Pager-owned queue / 消除多层 reconcile，仅研究，未交付）。
- Isolation/workflow/goal Issue 归档到 `docs/issues/架构/` 与 `docs/issues/adapter/`。
- LICENSE 保留 SpaceXAI 上游 Apache-2.0，同时注明 Dwsy fork 修改版权。
- README / FEATURE_MATRIX / AGENTS 中英同步产品隔离、workflow、goal 与 export 行为。

### 变更

- Adapter model/RPC/session/queue bridge 对齐上游，并支持 workflow/goal host。
- Pager app logic、jump/session picker、views、dispatch、effects 与 `grok-pi` binary 接线加入 `pi_workflows` / `pi_goal` F2 gate 与 extension 注入顺序。
- `install.sh` / path helper 在相关路径上遵循产品隔离 home。
- Slash command registration 统一 external profile gate；`/doctor` 作为 Pager 原生 terminal diagnostics。

### 修复

- `pi-grok-bash` extension 与后台任务 kill 控制路径 `x.ai/task/kill` 对齐。

### 说明

- **`pi-main`** 源码不允许本地 dirty 修改；子模块只保留干净 pin。
- F2 **Pi workflows** / **Pi goal** 改动后需完全退出并重启，因为 extension 在进程启动时注入。
- 未提交 WIP（例如 `/review-session` polish）不属于该 tag，除非 cut tag 前已经落地。
- 从 **0.0.6** 升级时也应阅读 **[0.0.7]**（Plan mode、`/jump`、`/fork`/`/clone`/`/reload`、resource policy、Remote TUI、tree rollback、update proxy）；0.0.8 的 GitHub Release notes 默认包含两段。

---

## [0.0.7] - 2026-07-21

范围：`v0.0.6` → `v0.0.7`。（该版本曾从 changelog 中遗漏，后按 Git 历史重建。）

### 新增

#### Plan 模式（原生 Pager ↔ Pi）

- Pager 原生 Plan toggle 桥接到 adapter 所有的 `Inactive` / `Pending` / `Active` / `ExitPending` 状态机。
- Session 私有 `.plan.md` sidecar 与 `.plan-mode.json` 持久化。
- Active 时注入 tool gate，除 plan file 外阻止 `edit` / `write` / `bash`。
- Pi `exit_plan_mode` 映射到原生 `x.ai/exit_plan_mode` 审批表面。
- 按模式阶段使用 full/sparse system-reminder 前缀。
- **Ctrl+Shift+T** 切换 Plan mode；向 `/view-plan` 发布 plan file path。

#### Session 导航与树

- **`/jump`**：原生 turn picker，含 timeline preview、紧凑 `HH:MM` 时间与 viewport 恢复。
- External profile 下，空输入时 **double-Esc** 与 **`/rewind`** 打开 **SessionTree**（Pi tree navigation，不是 Grok destructive rewind）。
- SessionTree 选中行高亮对齐 Pi selection 语义。
- **Tree file rollback**（F2 `pi_tree_file_rollback`，external-only，需重启）：SessionTree `r` 预览、`R` 执行；write/edit checkpoint preimage 经 `pi-grok-rollback` + adapter `pi/session/rollback_preview|execute` 完成。

#### Pi Session 分支操作

- **`/fork`**（external）：RPC `get_fork_messages` → jump 风格 `ListOverlay`；`fork` 创建分支 session file；同一 agent 重新绑定 `sessionId` + `session/load` 回放；所选文本预填 prompt。
- **`/clone`**（external）：RPC `clone` 复制当前 leaf；同一 agent 重绑并 replay，prompt 清空以对齐 Pi。
- 非 external profile 保持 Grok peer-agent `/fork` 不变。

#### Reload、Hotkeys 与 Session 别名

- **`/reload`**：`__pi_reload` → `ctx.reload()`；streaming 或 compacting 时禁止；adapter 刷新 command/model catalog；Pager 重新发现 Pi theme 并重应用当前 `pi:*` 主题；toast 文案对齐 Pi interactive。
- **`/hotkeys`**（别名 `shortcuts` / `keys`）：打开原生 ShortcutsHelp modal，与 Ctrl+. 相同。
- **`/session`** 作为 `/session-info` 别名；payload 可包含 `session_file` + message/turn/tool-call 计数。

#### Resource Policy 与 CLI

- **Pi resource admission policy**（`ResourcePolicy`）：allow/block list + heuristic（例如阻止 `pi-tool-display`、custom header/footer），在 `grok-pi` 启动时执行。
- 启动按需关闭 Pi auto-discovery（`--no-extensions/skills/prompt-templates/themes`），仅通过显式 flag 注入 policy 允许的资源。
- Catalog discovery 对齐 Pi package auto-entry 规则；package dir 只展开声明的 entry，不扫描嵌套私有模块。
- `/pi-config` modal：All/Enabled/Disabled filter、policy view（`a`）、refresh（`r`）、Tab scope、扩展快捷键；F2 在 `pi_config` 上 Enter 可打开。
- CLI tool flags：`--no-tools` / `-nt` / `--no-builtin-tools` / `-nbt`、`--exclude-tools` / `-xt`，以及 `PI_GROK_EXCLUDE_TOOLS`。
- 一等转发 Pi 的 model、session、prompt、resource、tools、trust/network flags；`--` 后参数继续透传。

#### Remote TUI、Auth 与 Recap

- Remote TUI host 使用官方 Pi **`rpc-entry.js`** + extension mode facade（extension 看到 `ctx.mode=tui`，底层 transport 仍为 RPC），**不 fork Pi 源码**。
- `pi-grok-remote-tui` 增加 multi-select capability lab（header/footer widget、status、title、editor text）与 overlay stacking/restore。
- `pi-grok-auth` 修复 nested `openCustom` 导致 LoginDialog 被拆除的问题，补齐 `showOverlay` / `showAuthPrompt` / `prompt.signal`。
- `pi-grok-recap` 可通过 F2 `recap_mermaid` 启用 **Mermaid**；markdown cleaner 保留内部 mermaid fence，recap 正文用 Markdown 渲染。

#### Settings（external-only gate）

- `SettingMeta.external_only`：仅在 grok-pi / external profile 活跃时显示对应行。
- F2 新增 `pi_tree_file_rollback`（需重启）、`recap_mermaid`、`remote_tui_footer`。
- `EXTERNAL_AGENT_ACTIVE` atomic 在启动时根据 UI profile 设置一次。

#### Timeline / Theme Render

- Timeline rail glyph（chevron、粗/细横线、active/hover tick）提供 ConHost fallback；横向 stroke 对齐 Pi 视觉。
- `/reload` 后 Pi theme registry 执行 **`rediscover()`**。

#### 更新通道

- GitHub release discovery 增加 **JSP proxy** fallback（`jsp.dwsy.link`）以规避未认证 API rate limit，失败再回退 `api.github.com`。

### 修复

- 展开 Edit 卡片：仅在 viewport 内节流 redraw；优化 sticky-header cache 与 edit gutter 高度估算。
- Live-turn sticky suppress 与 Pi 对齐 spinner。
- 版本比较忽略 dirty prerelease，改用 `+dirty` build metadata，避免本地构建误报“有更新”。
- Package extension discovery 只展开 Pi auto-entry（manifest entry）。
- Bash task kill：`x.ai/task/kill` 通过 `runningTaskIds` 校验（`KillOutcome`）到达 Pi Bash extension。
- Widget 渲染对齐与 picker `label_color` 打磨。

### 变更

- Adapter：Pi RPC stderr ring buffer diagnostics 与 rustfmt 清理。
- 精简双语 README，更新 Remote TUI 文档/流程 demo。
- 吸收该时间窗内上游 monorepo 同步（见 Git 的 `Synced from monorepo` commits）。

### 文档

- FEATURE_MATRIX 增加 `/fork`、`/clone`、`/reload`、`/hotkeys`、`/session` 行。
- 增加 Resume preview 与 search 对齐 Issue 规范。

---

## [0.0.6] - 2026-07-20

### 新增

- Adapter 增加 PSM session catalog、丰富 metadata 与 tree editor text 支持。
- Session picker 支持排序、全部展开、丰富 metadata 展示与 tree navigation。

### 修复

- Model picker 高度限制在可用 viewport 内。
- Remote TUI paste event 正确转发。
- Subagent live traffic 改用 `appendEntry`，并简化 recap emit。

## [0.0.5] - 2026-07-18

### 新增

- 为 extensions、skills、prompts、themes 提供原生 Pi resource manager。
- Context visualization bridge 进入 Pager 原生渲染。
- 改进 session recap、timeline 与 Pi model selection 集成。

### 变更

- Welcome hero card 使用 `grok-pi` 名称、产品版本与 Pi 描述。
- Release build、update check 与 update install 只使用 `GROK_PI_VERSION`。

### 修复

- 上游 Grok `GROK_VERSION` 与 workspace version 不再影响 `grok-pi` 版本显示或更新比较。

## [0.0.4] - 2026-07-17

### 新增

- 点击 Context / `/context`：adapter 基于 Pi `get_session_stats` + message estimate 实现 `x.ai/session/info`，进入原生 `ContextInfoBlock`。
- Queue pane bridge：Pi `queue_update` 完整数组 → `x.ai/queue/changed`，使 optimistic dequeue 生效。
- 通过 `GROK_PI_VERSION` / git describe 注入产品版本，`--version` 不再显示上游 `0.1.220-alpha.*`。

### 修复

- message estimate 为空时 Context breakdown 不再把整个窗口计入 Reasoning/overhead，而是回退到 Messages。
- Session-info payload 省略 null Option 字段，降低 Pager 反序列化风险。
- 修复 Welcome / dashboard / session load 路径上的 Pi catalog 连续性。

## [0.0.3] - 2026-07-17

### 变更

- Update check/install 改为**仅 GitHub**（`Dwsy/grok-pi` release JSON + install.sh/ps1）。
- 删除 npm registry fallback（未 scope 的 `grok-pi` 是外部包，scope 包当时也尚未发布）。

## [0.0.2] - 2026-07-17

### 新增

- 原生 Welcome screen + Pi π block logo（默认启动；`-c/--continue` 继续会话）。
- Agent Dashboard 适配：`/dashboard` · Ctrl+\\ · Pi session catalog → dormant roster。
- 更新检查与安装：
  - 来源：GitHub `Dwsy/grok-pi` release JSON（npm fallback 在 0.0.3 移除）。
  - CLI：`grok-pi update`、`grok-pi update --check`、`grok-pi update --to 0.0.2`。
  - Welcome **Ctrl+U** 在 quit-for-update 后运行同一安装器。
- Welcome **Changelog** 在 GitHub 打开本文件。
- Welcome **Resume session** 使用原生 SessionPicker（Pi catalog）。

### 变更

- Welcome 对 Pi 隐藏 **New worktree**（没有 Grok worktree 产品路径）。
- Canonical GitHub repo 为 `Dwsy/grok-pi`。
- Release workflow 用 git tag 注入 `GROK_VERSION`。

## [0.0.1] - 2026-07-17

### 新增

- 初始 `grok-pi` composition：Pi JSONL RPC ↔ ACP adapter + Grok Pager TUI。
- 安装脚本与多平台 GitHub release packaging。
