# Welcome 预热会话的 Pi 命令目录

## 问题与证据

用户报告：在原生 Welcome 页等待 Pi RPC 完成后开始输入，Pi 注册的技能不出现在补全；启动期间立即输入则正常。Pi 通过 `get_commands` 暴露 `/skill:<name>` 命令。

Fresh startup 先创建隐藏的 `welcome_prewarm_agent`，后台 attach 完成后发送进程级 `x.ai/commands/update`，但 Pager 在 Welcome 时只检查 `home_session_agent`，未播种预热 agent。Pi `session/new` 内的会话级命令更新在 `NewSessionResponse` 之前发出，此时 Pager 尚未绑定 session ID；Pager 的后续 `x.ai/commands/list` 恢复请求又未被 Pi adapter 实现。于是等待路径可能保留空目录，并在进入会话时覆盖 Welcome 的补全目录。

## 修复计划

- [x] 进程级目录只播种 Welcome 的预热 agent 或现有 pre-session 目标，保留已有会话级目录。
- [x] Pi adapter 为已绑定的当前会话提供现有命令目录恢复接口，保留 Pi 的命令来源和工作流规则。
- [x] 覆盖等待初始化、提前输入、会话级目录优先及命令目录恢复；运行定向验证并审查 diff。

## 验证

- 隔离临时配置下启动本机 Pi 1.5.0 RPC，`get_commands` 返回 `skill:welcome-smoke` 且 `source=skill`。
- 对本次修改的 Rust 文件执行 `rustfmt --check`；`git diff --check` 通过。源码一致性声明集精确匹配（66 项）；未放宽目录级规则。
- `CARGO_MAINTENANCE=0 CARGO_BUILD_JOBS=2 ./scripts/cargo-shared.sh test -p pi-grok-adapter commands_list_returns_current_pi_catalog_and_rejects_stale_sessions -- --nocapture`：通过，1 passed。
- `./scripts/cargo-shared.sh test -p xai-grok-pager --lib commands_update -- --nocapture`：通过，8 passed，包括 Welcome 初始化前后两种时序。
- `./scripts/cargo-shared.sh test -p xai-grok-pager --lib available_commands_refreshed -- --nocapture`：通过，3 passed，包括过期请求、失败与有效空列表。
- 未进行运行中真实终端的端到端手测，也未替换已安装的可执行程序。

保留工作区内既有未提交修改；不修改 Pi 上游源码。
