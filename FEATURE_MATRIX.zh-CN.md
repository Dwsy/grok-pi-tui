# Grok Native TUI × Pi 功能矩阵


**最小 Pi 版本：0.80.10**（系统 `pi` / `@earendil-works/pi-coding-agent`）。`pi-main` 为可选 git 子模块，非运行时必需。

状态定义：**原生**＝由 Grok Pager 组件实现；**适配**＝Pi 语义转换后进入 Grok 原生组件；**边界**＝Pi RPC 未暴露或与 Grok 产品后端绑定，刻意不实现。

## 终端与显示

| 功能 | 状态 | 实现 |
|---|---|---|
| Terminal init/restore | 原生 | Grok `init_terminal` / `restore_terminal` |
| Fullscreen / alternate screen | 原生 | Grok screen mode；启动时选择 |
| Minimal / scrollback-native | 原生 | `xai-grok-pager-minimal`；启动时选择 |
| Welcome / minimal logo | 原生+适配 | 默认进 Welcome（与 stock `grok` 一致）；`ExternalUiProfile.logo` 注入 π block art（行宽 pad 防居中错位）；仅 `grok-pi -c/--continue` 跳过 Welcome 直接 Resume |
| Welcome 菜单（Pi） | 原生+适配 | Resume/Ctrl+S ≡ `/resume`（Pi catalog）；隐藏 New worktree；Changelog 打开 `https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD` |
| Welcome session 预热（Pi） | 适配 | 进入 Welcome 即后台 `new_session`；首字输入 attach 预热 agent，避免冷启动 “Starting session…” |
| grok-pi 产品教程（`/tutorial`） | 原生+适配 | 复用上游 `TutorialState`、`ModalWindow`、picker、Markdown/doc viewer、命令别名（`/tour`、`/onboarding`）与键鼠路由；grok-pi composition 注入 18 主题 `TutorialProfile`，覆盖原生终端/输入；Pi 多 Provider 模型、thinking、工具、context、session/tree；review/rollback/Plan；extensions、Remote TUI、Skills、Prompt Templates、Packages、theme 与资源 Trust；后台任务、subagent/dashboard、可选自动化、export/update、状态隔离与 diagnostics。正文明确标注默认开启、F2 可选、重启、实验性和边界。stock Grok 保持默认正文；minimal 模式仍保持门控。 |
| 更新检查/安装 | 适配 | **仅 GitHub** `Dwsy/grok-pi` releases JSON + install.sh/ps1；`grok-pi update` / `--check` / Welcome **Ctrl+U**；`GROK_PI_NO_AUTO_UPDATE=1` 关后台检查 |
| Agent Dashboard | 原生+适配 | 原生 `/dashboard` · Ctrl+\\ · 列表/peek/dispatch；Pi 的单 session RPC host 同时仅保留一个 live AgentView，turn 忙时阻止二次 dispatch，已完成 session 经 `pi/session/list` → `pi/ui/session_catalog` 回到 dormant roster；不接 Grok leader FleetView |
| Prompt editing | 原生 | PromptWidget |
| 设置面板（F2） | 原生（grok-pi 自有surface） | grok-pi 自带分页设置面板（`views/pi_settings`），入口为 **F2**、`/settings` 与命令面板：每个设置分类一个原生 `ModalWindow` Tab，Tab 内左侧 section 侧边栏（非活动 section 整行变暗），焦点行说明固定在底部 3 行区块，`/` 跨 Tab 搜索。键位：`↑/↓/j/k` 移动行、`←/→/h/l` 切 Tab、`Tab` 进入 section 焦点、`Space`/`Enter` 修改、`g/G` 首尾、`d` 重置为默认（y/n 确认）。与上游共用 `crate::settings` 注册表，设置项、默认值与 `Action::Set*` 完全一致。上游 `views/settings_modal` 一行未改，仍可通过 `Action::OpenSettings` 打开。 |
| Multiline / Vim mode | 原生 | Grok slash/settings |
| Theme / timestamps / mouse | 原生+适配 | Grok appearance/input；Pi 主题 JSON 经 `theme::pi` 映射为 Grok `Theme`，`/theme` 可选 `pi:<name>`；内置实验性 `pi:transparent`（暗色）与 `pi:transparent-light`（浅色）将主画布交给终端默认背景（用于终端透明度/毛玻璃），同时保留选中态、代码、diff 与工具表面的实色；F2 可控制 OSC 9;4 terminal-tab progress，默认关闭 |
| Markdown / code blocks | 原生+适配 | Pi text/reasoning → ACP chunks → `xai-grok-markdown` |
| Tool cards | 原生+适配 | Pi tool events → ACP ToolCall；`read`/`bash`/`edit`/`write`/`grep`/`find`/`ls` 投影到原生卡 |
| Todo / plan list | 原生+适配 | Pi `@juicesharp/rpiv-todo` 的 `todo` tool `details.tasks` → ACP `Plan` → 原生 TodoPane/badge；scrollback 抑制 `todo` 卡 |
| Plan mode | 原生+适配 | Pi 仅暴露 Normal ↔ Plan：`/plan-mode` 与 Ctrl+Shift+T 用于切换，`/plan` 用于进入；Shift+Tab 保持 thinking level 切换。Pager 原生 Plan 开关 → adapter 负责的 `Inactive/Pending/Active/ExitPending` 状态机；full/sparse system-reminder 前缀；session 私有 `.plan.md` sidecar；注入 Pi `tool_call` gate 阻止 `edit`/`write`/`bash`（仅放行计划文件）；Pi `exit_plan_mode` 打开原生 `x.ai/exit_plan_mode` 审批，并持久化 `.plan-mode.json` 状态 |
| Goal 模式（`/goal`） | 适配（MVP legacy） | F2 `[ui].pi_goal` **默认关闭**（需重启）。注入扩展：`/goal` + `update_goal` + control 文件；adapter GoalHost 发原生 `GoalUpdated`（状态条 / detail）。Active 时 `agent_settled` follow-up 续跑。**不含** shell 完整 multi-agent classifier/planner/strategist（后续切片）。 |
| Loop 定时（`/loop`） | 适配（MVP） | F2 `[ui].pi_loop` **默认关闭**（需重启）。注入扩展：`/loop` + `scheduler_create/delete/list` + 进程内 timer；adapter bridge → 原生 `ScheduledTask*`（tasks pane）。仅 session（无 durable / loop subagent）。 |
| Diff rendering | 原生+适配 | edit-like metadata 进入 Grok 原生 tool/diff pipeline。grok-pi 提供 external-only F2 开关 **Side-by-side edit diffs**（默认关闭）：开启且宽度足够时，展开 EditTool 与普通全屏 viewer 并排显示 old/new，并显示 `-`/`+` 标记；关闭或窄布局使用原生 unified renderer，code-review 保持 unified 双 gutter |
| Images | 原生+适配 | Pi image blocks → ACP ImageContent；具体终端显示取决于 Grok/terminal 能力 |
| Scroll / find / copy / transcript / export | 原生 | Grok Pager |

## Agent 与流式语义

| Pi 功能 | 状态 | 映射 |
|---|---|---|
| Prompt | 适配 | ACP prompt → Pi `prompt` |
| Mid-turn send now | 适配 | 输入框 send-now → Pi `steer`。适配器隔离队列中的待执行行可按版本原子移除并提升为 `steer`，也支持编辑后立即发送；仅存在于 Pi 外部队列的行仍只读，因为 stock RPC 无法原子删除。 |
| Follow-up queue | 适配 | turn 运行中来自客户端的 prompt，以及 extension 的 `sendUserMessage(..., { deliverAs: "followUp" })`，先进入 adapter 自有隔离队列，真正出队时才以 RPC source 发送给 Pi。绕过拦截的 Pi `queue_update` 消息进入独立外部镜像通道。 |
| Abort | 适配+边界 | ACP cancel 同步清空 adapter 自有的客户端/扩展队列并完成等待者；取消屏障期间 extension 新产生的续跑消息会被丢弃，随后以 fire-and-forget 通知 Pi `abort`（Bash 用 `abort_bash`）。`get_state` settle 探针只在 Pi 真正空闲后恢复调度。stock Pi 外部队列仍受 0.81.1 无 `clear_queue` 的 RPC 边界限制。 |
| Text stream | 适配 | `message_update` → AgentMessageChunk |
| Thinking/reasoning stream | 适配 | `message_update` → AgentThoughtChunk |
| Tool start/update/end | 适配 | ACP ToolCall/ToolCallUpdate |
| Pi Bash 后台任务 / Send to Background | 原生+适配 | `grok-pi` 私有 Bash extension 持有前台与初始后台 Bash 子进程；前台仍复用 Pi `createBashToolDefinition` 的输出/渲染语义。Pager 原生 Send to Background 经 `x.ai/terminal/background` 以受控临时控制文件按 `toolCallId` 转交**同一**子进程，随后投影到既有 `x.ai/task_*` 卡片；原生任务卡 kill 经同一控制通道走 `x.ai/task/kill`（`op:kill` + 已发布 `runningTaskIds`）；`is_background` + `description`、`get_task_output` / `wait_tasks` / `kill_task` 保持可用。任务终态经私有 `__pi_grok_bash_task__` 状态通道带外发布,不受流式、ESC 取消与队列清空影响;对话侧 bridge 消息仍负责唤醒模型。适配器镜像任务生命周期,两条通道的同一终态只投影一次;Pi 子进程退出时对残留任务按 `signal: session_restart` 对账(行离开 running 过滤器,不新增失败块)。 |
| Pi 子代理 | 原生+适配 | F2 `[ui].pi_subagents` 默认开、需重启。开启时，内置 `pi-grok-subagents` extension 拥有 Pi child `AgentSession`；版本化 bridge 投影到原生 `SubagentBlock`、Tasks Pane、child `AgentView` 与 `x.ai/subagent/cancel`。`/subagents` 管理产品隔离的项目/全局 Markdown 覆盖，`/subagent-message` 与 `send_message_to_subagent` 可向运行中的 child 发送 follow-up/steer。关闭后不注入桥接，并清空继承的 activation metadata。模型驱动的手工端到端验收待执行。 |
| Workflow（Rhai / `/workflow`） | 上游引擎 + Pi Spawn 接缝 | **会话宿主 + slash 表面：** 复用 `xai-workflow` + `ExternalWorkflowRuntime`；adapter `x.ai/workflow/{launch,pause,stop}` + `x.ai/workflows/list` + `workflow_updated`；注入 `/workflow`、`/workflows`、`/create-workflow`（及命名脚本）；隐藏 `__pi_workflow_*` 桥命令；Pager 本地处理 + F2 门控。deep-research 实机手测仍建议。`/create-workflow` 为 PassThrough 用户提示（非 Pi skill）。项目脚本目录默认 `<repo>/.grok-pi/workflows`。 |
| Prompt completion | 适配 | 正常完成仍以 Pi `agent_settled` 为屏障。adapter 提升的客户端行保留 ACP waiter；extension/Pi 所有的运行发送 `x.ai/session/prompt_complete`。若 prompt 被 input handler 吞掉、没有 `agent_start`/`agent_settled`，idle 探针会主动收敛，避免幽灵 “Waiting…”。 |
| Retry | 适配 | Grok native sticky status/toast |
| RPC 连接韧性 | 适配 | Pi RPC 子进程意外退出触发带退避的自动重启(风暴护栏:5 分钟内最多 3 次恢复)、重新 bootstrap,并通过 `switch_session` 回接崩溃前的会话文件;主动拆除(探针、respawn)带 `intentional` 标记,不触发恢复。`get_state` 心跳看门狗(15 秒节奏;连续 4 次超时 ≈ 持续无响应 2 分钟)会把"活着但卡死"的子进程 kill 进同一条恢复路径(`PI_GROK_RPC_WATCHDOG=0` 停用)。崩溃时在途 prompt/队列行按取消处理,不重放。 |
| Compaction | 原生+适配 | `/compact [instructions]` → Pi `compact`；Pi `compaction_*` → 原生 CompactionStarted/Completed/Failed/Cancelled scrollback blocks + sticky status |
| Session recap (`/recap` + auto away) | 适配 | initialize `meta.sessionRecap`；`x.ai/recap` → 注入 extension `__pi_grok_recap`（`complete` 侧调用，不写会话历史）→ custom `pi-grok-recap/v1` → `SessionRecap`。仅使用 F2 显式配置的 `recap_model`，不回退当前会话模型；auto：≥3 turn、最后完成 turn ≥3 分钟、终端失焦期间后台生成、成功后无新 turn 不重复；manual：有 user turn即可；可选 `/recap [focus]` / `/summarize [focus]` 将 `customInstructions` 注入 recap 提示词（追加，同 `/compact`）；输入限最近 6 turn/12k 字符；正文语言优先 macOS `AppleLanguages`，再回退 locale |
| BTW 历史（`/btw` + `/btw-history`） | 适配 | 成功的 Pi BTW 答案通过 `appendEntry("pi-grok-btw/history/v1", …)` 写入不进入上下文的自定义 entity，保存问题、答案、时间、request id 和实际使用模型。adapter 在加载/树切换时从 `get_entries` 重建 active branch，并投影到原生 `BtwBlock` scrollback；动态提供的 `/btw-history` 刷新并查看同一批记录，不触发模型调用。 |
| Queue pane / count | 适配+边界 | adapter 自有的客户端/扩展待执行行支持真实 remove、clear、edit、reorder、interject；稳定 id/version 保持 Pager reconcile 与原始展示文本。绕过拦截的 Pi `queue_update` 仍进入只读外部通道；只有 follow-up 出队推进 `runningPromptId`，steering 保持当前 turn。队列出队模式仍可经 `pi/queue/mode` 设置（`one-at-a-time` / `all`）。 |
| Context bar used tokens | 适配 | Pi `contextUsage` / message usage → ACP `_meta.totalTokens` → 右上角 bar |
| Context click / `/context` | 原生+适配 | Grok `x.ai/session/info` → Pi stats + messages + `__pi_context_breakdown` + 可选 `cacheMetrics`（`get_entries`，对齐 pi-cache-graph）→ 原生 `ModalWindow`（`ContextInfoBlock` + `0/1/2/3/s` 视图、`e` 导出、`r` 刷新）；F2 `[ui].pi_cache_graph` 默认开；运行中即时刷新、不写 scrollback |
| 用户消息 Markdown（grok-pi） | 原生 | F2 `[ui].pi_user_markdown` 默认开；grok-pi 用户 prompt 走 agent markdown 渲染（展开、保留用户前缀/背景）；关闭恢复可折叠纯文本 `UserPromptBlock`；即时生效 |

## Model、session 与命令

| 功能 | 状态 | 说明 |
|---|---|---|
| Provider/模型目录 | 原生+适配 | Pi 负责多 Provider registry、凭据、`models.json` 本地/自定义 endpoint 与 extension `registerProvider`；`get_available_models` → Grok 原生 model selector，裸 `/model` 打开且当前模型置顶。默认开启的 Pi auth bridge 在 Remote TUI 可用时提供 `/login`/`/logout`。 |
| Thinking effort | 适配 | Pi levels → Grok effort selector；xhigh/max 做能力归一化 |
| New session | 适配 | Grok `/new` → Pi `new_session` |
| Rename | 适配 | Grok `/rename` → Pi `set_session_name` |
| Resume session catalog | 适配 | `/resume` 经无界面 adapter 读取 Pi JSONL 元数据。已命名会话显示原生 `named` 标记；展开 Pi 行可显示 CWD/会话路径、开始/更新时间、模型、消息数、已持久化的 token 总数与成本（仅在记录存在时）。目录继续按最近活动时间排序。 |
| Session info / context snapshot | 适配 | 原生 `/session-info`（别名 `/session`，对齐 Pi 命名）→ Grok `x.ai/session/info` ← 最新 Pi `get_session_stats`。Pi 风格 scrollback 现展示 session 名称/file/ID、总/user/assistant/tool call/tool result 计数、prompt/cache/output/total token、cache 命中率与写入量、总成本；下方保留 Grok runtime/auth/model/current-context。注入 breakdown 继续为 `/context` 提供 system/tool-defs/AGENTS，bridge 失败时回退 0。 |
| Session history replay | 适配 | 保留 `get_entries` append-log 缓存并用 `since` 增量刷新；active `leafId` 按 parent chain 线性 push+reverse（对齐 Pi upstream 最近优化），排除 sibling branch，tree 切换不再全量刷新 state/model/command 与嵌套 `get_tree`。回放包含压缩前消息及可见 summary/custom entry；旧 host 不支持时回退压缩后的 `get_messages`。 |
| 启动时继续上一会话 | 适配 | `grok-pi --continue` / `-c` → Pi `--continue` |
| 启动资源、提示词与会话选项 | 适配 | `grok-pi` 一等转发：模型（`--provider`/`--model`/`--models`/`--thinking`）、会话（`--session`/`--session-id`/`--session-dir`/`--fork`/`--no-session`/`--name`）、提示词（`--system-prompt`/`--append-system-prompt`）、资源（`--extension`/`--no-extensions`/`--no-skills`/`--no-context-files`）、工具（`--tools`/`--exclude-tools`/`--no-tools`/`--no-builtin-tools`）、trust/网络（`--approve`/`--no-approve`/`--offline`）；`--` 后参数仍透传。不暴露 `--resume`（Welcome/`/resume`） |
| Pi extension/prompt/skill commands | 原生+适配 | `get_commands` → Grok slash registry；`source=extension` 经私有 ACP metadata 直达 Pi command handler，不进入 Pager 本地或 Pi steering/follow-up 队列；prompt/skill 保持 prompt 语义 |
| Pi Config 资源管理 | 原生+Rust 兼容 | F2 或 `/pi-config`（别名 `/pi-resources`）→ Pi resources；Rust 读取 Pi `settings.json`/`trust.json`，管理 extensions/skills/prompts/themes 的 global 与 trusted-project 覆盖。按 Pi 自动扩展入口规则发现资源；来源树默认折叠，GitHub/npm/local 身份清晰可见，搜索仅展开命中来源。原生双栏支持树展开/折叠、搜索、键盘分页/滚动、点击与滚轮；右栏预览 package.json 关键字段与 README；切换后提示重启或 Pi `/reload`；不含 `install/remove/update`。 |
| Pi 模型管理中心 | 原生+Rust 兼容 | `/pi-models`（别名 `/model-config`、`/models-config`）打开响应式 Provider → Model → Details 原生弹窗，信息层级参考 PSM。键盘和鼠标支持搜索、新建/克隆/编辑/删除、可选布尔循环、脏状态、二次确认、空态/错误态和当前模型激活。Rust 保留 `models.json` 未知字段，并使用目录锁、外部修改冲突检测、原始字节备份、私有同目录原子替换和最近备份恢复。保存/恢复复用 Pi 官方 `/reload`（`ctx.reload()`），无需重启即可刷新运行中模型目录；可用性仍遵循 Pi 认证（`apiKey`、`/login` 或 CLI），激活走 typed ACP `session/set_model`。 |
| Grok cloud/session history picker | 边界 | 依赖 Grok session store，Pi profile 不暴露 `/history` |
| Pi session tree (`/tree`) | 适配 | 原生 `SessionTree` modal：筛选/搜索/折叠/详情/复制/标签；Enter/`Shift+Enter` 经注入 extension 调 `ctx.navigateTree`（可 summarize）；`session/load` 回放；TreeX 风格详情面板；不改 Pi 源码 |
| 会话代码审查（`/review-session`、`/review-message`） | 原生 | 参考 PSM code-review → 原生 Pager 双栏：左文件列表（flat/tree）、右 BlockViewer 预览（默认仅 changes）。F2 `review_file_tree` **默认关**并持久化；弹窗内 `t` 切换树形（按 cwd 省略前缀，折叠 Java 连续包路径为 `com.example.app`）。`/review-message` 复用 jump。不走 Pi custom UI。 |
| Pi session fork (`/fork`) | 适配 | External：与 `/jump` 同款 prompt 区 `ListOverlay`（RPC `get_fork_messages`）；选择后 RPC `fork` 生成分支 session 文件，同 agent 换绑新 `sessionId`，`session/load` 回放并把选中文案预填 prompt；非 external 仍走 Grok peer-agent `/fork` |
| Pi session clone (`/clone`) | 适配 | External：RPC `clone` 在当前 leaf 复制新 session 文件；同 agent 换绑新 `sessionId`，`session/load` 回放并清空 prompt（对齐 Pi） |
| Pi 资源重载 (`/reload`) | 适配 | External：`__pi_reload` → `ctx.reload()`；流式 **与** compaction 中禁止（对齐 Pi）；adapter 刷新命令/模型目录；Pager 重扫 Pi theme（`rediscover`）并重应用当前 `pi:*` 主题；loading/成功 toast 文案对齐 Pi；不分支 session 文件 |
| Pi HTML export / share | 适配 | Grok `/export` 仍为 Markdown transcript；默认开启 `/export-html`（Pi HTML / `.jsonl`）与 `/pi-share`（私有 gh gist 固定写入 `session.html` + pi.dev），经 `pi-grok-export` 注入，不另造 TUI |

## Extension UI

| 方法 | 状态 | Grok 组件 |
|---|---|---|
| `notify` | 原生+适配 | warning/error → 原生 toast；显式 `info` → 仅 SystemMessage scrollback（对齐 Pi `showStatus` / chat 追加，不做 toast-only）；`/notify` 用原生可搜索 modal 查看当前进程内、按 Pi session 隔离的全部 info/warning/error 事件（不持久化） |
| `setStatus` | 原生+适配 | sticky banner/status |
| `setWidget` | 原生+适配 | persistent native banner surface |
| `setTitle` | 原生+适配 | terminal title |
| `set_editor_text` | 原生+适配 | PromptWidget |
| `select` | 原生+适配 | QuestionView option list |
| `confirm` | 原生+适配 | QuestionView Yes/No |
| `input` | 原生+适配 | QuestionView freeform PromptWidget |
| `editor` | 原生+适配 | QuestionView multiline PromptWidget |
| timeout/cancel | 适配 | Pi timeout 撤销对应 QuestionView，返回 `cancelled:true` |
| 原生 Q&A（`ask_user_question` 工具） | 适配 | F2 `[ui].pi_ask_user_question` **默认关**（需重启）。注入扩展注册工具；adapter 打开多题 `x.ai/ask_user_question` → 原生 QuestionView；control 目录回写答案。F2 `[ui].pi_ask_user_question_notifications` **默认开**，即时控制 grok-pi 失焦时 Q&A 抵达的原生桌面通知。文案：*Grok Build asks the right questions to nail the details.* 冲突包表：`assets/native_feature_conflicts.toml` — 开启时 host block 列表包（如 `@juicesharp/rpiv-ask-user-question`）；F2 描述会列出。 |
| raw terminal hook | 边界 | Pi RPC 明确不支持 |
| custom header/footer/component | 边界 | Pi RPC 明确不支持 component factory |
| Remote TUI（实验） | 实验 | `PI_GROK_REMOTE_TUI` 默认开：**不改 Pi 源码**；npm/Node Pi 通过官方 `rpc-entry.js` 启动，因此仅检查 argv 的第三方 RPC guard 看不到外层 `--mode rpc`；最先注入的兼容扩展仅在 Remote TUI host 活跃时将 `ExtensionRunner` 暴露给扩展的 `ctx.mode` 从 `rpc` 投影为 `tui`。Pi core 与 JSONL transport 仍是真实 RPC。注入 `ctx.ui.custom` host + `setWidget` 帧投影；键经 tmp keyfile；Pager ANSI 解析。裸 `/login`/`/logout` 由 `pi-grok-auth` 默认开启（resume-x 风格）；更广的 `/pi-*` 选择器仍需 `PI_GROK_NATIVE_COMMANDS` |
| 原生 feature 包冲突 | 宿主策略 | 默认：`assets/native_feature_conflicts.toml`。运行时外挂（免 rebuild）：`$GROK_HOME/native-feature-conflicts.toml` → `$GROK_PROJECT_DIR/native-feature-conflicts.toml`（包列表 union）。由 `pi_ask_user_question` / `pi_goal` / `pi_workflows` / `pi_subagents` / `pi_btw` 的 F2/bridge 状态门控；关闭功能后重新放行其冲突包。用户 `allow` 可豁免。 |
| `rpiv-btw` | 边界 | F2 `pi_btw` 开启时屏蔽；走原生 `/btw` + adapter `x.ai/btw` + `pi-grok-btw` 扩展（默认关） |

## 斜杠命令

### 保留的 Grok 原生命令

`exit`、`help`、`hotkeys`（别名 `shortcuts`/`keys`）、`tutorial`（别名 `tour`/`onboarding`）、`new`、`compact`、`model`、`effort`、`rename`、`resume`、`session-info`（别名 `session`）、`tree`、`tree-map`、`fork`、`clone`、`reload`、`notify`、`dashboard`、`recap`、`btw`、`copy`、`find`、`jump`、`review-session`、`review-message`、`transcript`、`export`、`expand`、`queue`、`multiline`、`compact-mode`、`vim-mode`、`theme`、`timestamps`、`timeline`、`toggle-mouse-reporting`、`voice`、`doctor`、`debug`、`pi-config`、`pi-models`、`pi-shortcut-manager`。各命令仍遵循自身 visibility/capability 门控。F2 `pi_btw` 开启时，Pi extension 另外提供直接、不调用模型的 `/btw-history`。

### 动态 Pi 命令

Pi 返回的 extension、Prompt Template 和 Skill 命令不硬编码在 Rust 中。它们通过 ACP command catalog 进入 Grok 原生 slash suggestion/dropdown；名称冲突由 Grok registry 去重。内置 bridge extension 还可提供 Pi Provider 登录（`/login`、`/logout`）、Pi HTML 导出/分享（`/export-html`、`/pi-share`）以及受 F2 门控的 workflow/automation 命令。

### 刻意排除

stock Grok 产品或本地 session-store 命令——包括 Grok `/history`、Grok 账户 `/login`/`/logout`、`usage`、`plugins`、`mcp`、`memory`、`workspace`、`share`——均排除。同名 `/login`/`/logout` 可由 grok-pi 的 Pi auth extension 提供，此时认证的是 Pi 模型 Provider，而不是 Grok.com。原版 `/minimal`、`/fullscreen` re-exec 也不暴露；screen mode 应在启动时选择，以保留 Pi 进程参数。
