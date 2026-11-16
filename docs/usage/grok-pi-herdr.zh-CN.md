# grok-pi 的 Herdr 集成

`grok-pi` 现在内置一个默认开启的无界面 Herdr 生命周期桥接。不需要单独安装 Pi 集成；当进程不在 Herdr 中运行时，它会静默不做任何事。

桥接通过 Herdr 本地 socket 上报原生 Pi 会话标识，以及权威的 `working`、`blocked`、`idle` 状态。它不会创建第二套终端 UI，也不会修改用户目录中的 Pi 或 Herdr 文件。

## 安装与启动 Herdr

1. 安装 Herdr：

   ```bash
   curl -fsSL https://herdr.dev/install.sh | sh
   ```

2. 启动 Herdr：

   ```bash
   herdr
   ```

3. 创建或选择 workspace，并为项目打开一个 pane。

4. 在该 pane 内运行 `grok-pi`：

   ```bash
   cd /path/to/project
   grok-pi
   ```

   如果环境中已经有 `HERDR_ENV=1`，说明当前就在 Herdr 内，不要再嵌套启动一个 Herdr。

   如果启动后还停在 grok-pi 的 Welcome 页面，请先创建或选择一个 Pi
   会话。Welcome 背后的预热进程不等于已启动的根交互会话，不会报告
   `session/idle` 生命周期。

5. 可在另一个 shell 或 Herdr 命令 pane 中验证：

   ```bash
   herdr agent get "$HERDR_PANE_ID"
   ```

   Agent 应识别为 `pi`；执行回合时显示 `working`，根交互会话稳定后显示 `idle`。

## 在 F2 中开启或关闭

打开 **F2 → Agent → Pi Herdr integration**。

- 默认值是 `on`，但仅在 Herdr 环境中实际发送报告。
- 如需关闭内置桥接，可改为 `off`。
- 修改后需要完整重启 `grok-pi` 进程。

等价配置为：

```toml
[ui]
pi_herdr = true
```

改为 `false` 可以关闭；删除该键会恢复默认开启。`grok-pi --no-extensions` 也会在当前进程中跳过它以及所有其他内置桥接扩展。

## 与 Herdr 官方 Pi 集成的关系

Herdr 可通过 `herdr integration install pi` 安装面向 stock Pi 的托管扩展，但 `grok-pi` 不依赖它。内置桥接开启时，宿主只会跳过自动发现的 Herdr 托管 Pi 文件，避免两个扩展争用同一个权威 `herdr:pi` 生命周期来源；用户显式传入的 `--extension` 不会被修改。

系统不会删除或重写全局集成，因此在 `grok-pi` 之外运行 stock `pi` 时，仍可继续使用 Herdr 的托管集成。

## 排查

- 查看 Herdr 与集成状态：

  ```bash
  herdr --version
  herdr integration status
  ```

- 确认 pane 环境包含 `HERDR_ENV=1`、`HERDR_SOCKET_PATH`、`HERDR_PANE_ID`。
- 修改 F2 设置后重启 `grok-pi`。
- 如果使用了 `--no-extensions`，去掉该参数后重试。
- 桥接采用失败关闭策略：本地 socket 不存在或不可用时，`grok-pi` 会继续正常运行，只是不再上报生命周期。
- 查看 `~/.grok-pi/logs/pi-rpc-stderr.log` 中的 `[pi-grok-herdr]` 错误；Bridge 会校验 Herdr Socket API 的 JSON 应答，错误会有限重试。
- 使用 `herdr pane process-info --pane <PANE_ID>` 检查前台进程，并运行 `herdr agent explain <PANE_ID> --json`。只有 `screen_detection_skipped=true` 且原因为 `full_lifecycle_hook_authority`，才代表 Header 已由生命周期接管。Socket 成功回复不保证 Herdr 内部已接纳该状态。
- `herdr agent list` 显示 `pi/idle` 也可能只是 `default_known_agent_idle_fallback` 屏幕检测回退；请以同一 Pane 的 `agent explain --json` 判定权威接管。
- 本地 ARM64 已编译完成时，可执行 `python3 scripts/pi-herdr-pty-smoke.py`，
  通过**真实 Pi RPC + 隔离模拟 Socket**检验会话及 `idle` 上报。运行
  `node --test extensions/pi-grok-herdr/lifecycle.test.mjs` 可检验
  `idle → working → blocked → working → idle`、快速连续状态及断连背压。
  这些测试不等于真实 Herdr Pane 的权威接管验收。
- 正常运行时 Socket 报告按 FIFO 保留完整状态变化；只有本地 Socket
  堵塞导致队列超过 128 条，才退化为保留最新会话身份和之后的最新状态，
  避免断连时内存增长无界。
