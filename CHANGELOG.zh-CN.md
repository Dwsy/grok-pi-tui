# 更新日志（中文）

**grok-pi**（在 Grok Build 生产级 TUI 中运行 Pi Agent Core）的版本说明。

- 英文完整版（含历史版本）：[CHANGELOG.MD](CHANGELOG.MD)
- 格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)

---

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

## 更早版本

`0.0.8` 及更早的完整英文条目见 [CHANGELOG.MD](CHANGELOG.MD)。
