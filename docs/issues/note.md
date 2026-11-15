# 原生 Pi 会话与模型 UI 修复

## 目标

- [x] `/resume` 的会话预览中，Esc 返回会话列表而非关闭弹窗。
- [x] `/model` 选择项显示“模型名称 + 提供商”。
- [x] 成功切换模型后，用新模型的上下文窗口更新上下文最大值，保留已用 token 数。
- [x] Herdr 桥接插件默认开启（仅在 Herdr 内生效），可通过 F2 `[ui].pi_herdr` 关闭（重启生效）。
- [x] Pi 模式仅暴露 Normal/Plan；`/plan-mode` 与 Ctrl+Shift+T 切换（`/plan` 仅进入），Shift+Tab 正确显示并执行 thinking level 切换。
- [x] Dashboard 对 Pi 单 session RPC host 做串行隔离：运行中阻止二次 dispatch，新 session 创建前移除旧 live AgentView，避免用户提示词/上下文串线。
- [x] `/resume` 改用 `get_entries` 回放 active branch，渲染压缩前消息及可见 summary/custom entry；旧 host 回退 `get_messages`。
- [x] 修复 GitHub issue #2：`/pi-share` 的私有 gist 固定包含 pi.dev 所需的 `session.html`。
- [x] 为 Pi 子代理增加 F2 `[ui].pi_subagents` 开关（默认开、重启生效）；关闭后不注入 bridge，并重新放行冲突的第三方子代理包。
- [x] 优化 Pi resume/tree 分支切换：复用 `get_entries` append-log 缓存与 `since` delta，按 active leaf 线性回溯 parent chain，避免全量 bootstrap 与嵌套 tree 重载。
- [x] 更新 `pi-main` 到 upstream `4f0437e2`，并增强 `/session-info`：展示 Pi session 名称、完整消息/工具计数、cache 分解、全量 token 与总成本。
- [x] `/resume` 会话列表在相对时间后直接显示已知消息数，例如 `just now · 20 msgs`。

## 验收

- 预览模式按 Esc 后仍停留在 `/resume` 会话选择器。
- 同名模型可通过显示的提供商区分。
- 已显示上下文使用量时，切换到不同窗口大小的模型会立刻更新最大值。
- 缺少 `ui.pi_herdr` 配置时按默认值注入 Herdr 扩展；`false` 才明确关闭。
- Pi 模式切换只在 Normal/Plan 间往返；Shift+Tab 提示为 thinking，Dashboard 模式提示为 Ctrl+Shift+T。
- Dashboard 不会在一个 Pi RPC host 上保留多个可写 live AgentView，运行中的 turn 也不会被新 dispatch 重定向。
- Resume 可看到最近一次压缩之前的历史；`/pi-share` gist 文件名为 `session.html`。
- F2 关闭 Pi subagents 后，下一次启动不携带 bundled extension，`PI_GROK_SUBAGENTS=0`，且 `pi-subagents` 包不再被 native feature policy 屏蔽。
- Resume/tree 切换仅回放 active parent chain，不混入 sibling branch；同 session 后续刷新使用 `get_entries(since)`，且运行中的 Pi turn 禁止 tree navigation。
- `/session-info` 优先显示 Pi `sessionName`，并按 Pi 口径展示 total/user/assistant、tool calls/results、input/cache read/cache write/output/total token、cache hit rate 与 cost；旧 agent 缺少扩展字段时仍可反序列化并使用 compact fallback。
- `/resume` 普通 session 行在右侧显示 `时间 · N msgs`；消息数未知（0 占位）时不误报 `0 msgs`。

## 验证

- [x] `git diff --check` 通过。
- [x] 既有问题修复的定向源码契约断言通过（8 个文件、22 条断言）。
- [x] 子代理开关源码契约通过（13 个文件、32 条断言）。
- [x] `bun build extensions/pi-grok-export/index.ts` 通过。
- [x] 子代理开关涉及的 6 个可独立检查 Rust 文件通过 `rustfmt --check`；其余 7 个文件的新增相关行经临时副本验证为 rustfmt-stable。
- [x] `native_feature_conflicts.toml` 可解析，中英文能力矩阵目标行通过结构检查，且无陈旧 “always-on/no F2 switch” 文案。
- [x] Resume 分支性能源码契约通过（6 个文件、17 条断言）；确认 vendored Pi 的 `buildSessionPath` / `getBranch` 已采用 upstream 的线性 push+reverse 遍历。
- [x] `bun test` 的 Pi session-manager 分支/上下文定向套件通过（47 pass、0 fail）。
- [x] 新增 branch-cache 相关 Rust 行经临时副本验证为 rustfmt-stable；定向回归测试已加入源码。
- [x] `/session-info` 源码契约通过（8 个文件、29 条断言），所有 `SessionInfoResponse` 构造均补齐可选兼容字段。
- [x] 更新 Pi AI workspace build artifact 后，最新 upstream `AgentSession.getSessionStats` 定向测试通过（8 pass、0 fail；18 个 RPC integration case 按测试文件配置跳过）。
- [x] 会话信息涉及的 4 个可独立 Rust 文件通过 `rustfmt --check`，Pager formatter/test 新增行经临时副本验证为 rustfmt-stable；`pi-main` 保持 clean detached `4f0437e2`。
- [x] `/resume` 行消息数源码契约通过，确认 Pi catalog `messageCount` → picker `num_messages` → `时间 · N msgs` 的完整链路；`session_picker.rs` 通过定向 rustfmt 检查。
- 未运行 Cargo（用户要求，避免高内存消耗），因此新增 Rust 测试未执行，也未做 Rust 编译或运行时验证。
- 全量 `rustfmt --check` 仍被仓库既有未格式化代码阻断；未重排无关代码。
