# grok-pi × Herdr 生命周期集成：证据与修复

> 日期：2026-11-15｜范围：仅 grok-pi 内置 Pi Extension｜Herdr 源码不变
> 状态：扩展已改、Node 回归通过；新 beta.4 **新 pane 实机接管已通过**，现存旧 pane 的恢复仍待解决；Herdr 源码保持不变

## 2026-11-15 复核：必须保持 Herdr 源码不变

先前尝试在 Herdr 的 `src/app/api/panes.rs` 补进程检测属于错误的修复边界，**已完整撤销**；Herdr 是只读的外部依赖，今后不通过修改、重编译或发布 Herdr 来解决此问题。

在**不改变已安装 Herdr 0.9.5、也不运行本地 Cargo** 的条件下，独立 Herdr pane 中的现场对照：

| 场景 | Herdr 会话引用 | `full_lifecycle_hook_authority` |
| --- | --- | --- |
| 已运行的 `wNB:pE`，18:22 启动，内置旧扩展（5,165 bytes） | ID | 否 |
| 已运行的 `wNB:pG`，23:17 启动，内置当前扩展（6,357 bytes） | ID | 否 |
| 新目录 `pig --no-extensions` | 文件绝对路径 | **是** |
| 新目录直接 `pig` | 文件绝对路径 | **是** |
| `bw-platform` 中新 pane `pig --no-extensions` | 文件绝对路径 | **是** |
| `bw-platform` 中新 pane 直接 `pig` | 文件绝对路径 | **是** |
| 新目录原生 `pi --no-extensions --extension ~/.pi/agent/extensions/herdr-agent-state.ts` | 文件绝对路径 | **是** |

所有测试 pane 在读取 `herdr agent explain <PANE> --json` 后已关闭，未触及现有工作会话。成功的判断是 `screen_detection_skipped=true` 且 `screen_detection_skip_reason=full_lifecycle_hook_authority`，不是 Socket API 的通用 `ok`。

**裁决：当前发行版在新 pane 里已能完成官方生命周期接管，不能把失败归因为必须修改 Herdr 内部进程检测。** 旧 pane 与新 pane 的会话 ID/路径、启动时序和可能存在的保留状态不同；仅凭这些对照不能证明 ID 类型本身是根因。曾尝试通过公共 API 在旧 pane 上报告其真实文件路径并发送 `resume` 与 `idle`，仍未接管，表明这不是简单替换字段就能修好。

最小可靠恢复：先保留原会话内容，在 Herdr **新 pane** 启动当前 `pig`，确认 `agent explain` 达标后，再按需从 Grok/Pi 恢复已有会话。**不要**强制关闭正在工作的旧 pane；也不要为绕过官方安全门控改用伪造的 custom source 或修改 Herdr。

后续如果要增强 grok-pi 自愈，应先在独立测试 pane 复现从 ID 到路径/启动后状态失效的最小用例，并证明仅修改本项目代码就能恢复；未经验证不引入后台定时探测、进程名伪装或修改 Herdr 宿主逻辑。

## 核心结论

Herdr 已收到 grok-pi 的 session 身份，但没有获得完整生命周期 authority。旧报告推断是 Pi RPC 进程名 `pi-rpc`；**这一推断已被运行现场证伪**：`herdr pane process-info --pane wNB:pE` 显示当前前台进程组存在 `argv0=pi` 的 Node 子进程。因此不设置 `process.title`，也不改 Herdr 的安全门控。

真正未证实的是状态报告是否到达、到达次序以及 Herdr 内部是否接受。原代码 `socket.on("data") => true` 无条件把任意返回当作成功；`agent_start` 的 session report 与 state report 还会并发。

## 现场事实

- Herdr 0.9.5；grok-pi 0.1.14-beta.3；`[ui].pi_herdr=true`。
- grok-pi pane `wNB:pE`：有 `herdr:pi` session 身份，`screen_detection_skipped=false`，`fallback_reason=default_known_agent_idle_fallback`。
- stock Pi pane：`screen_detection_skipped=true` 且 `screen_detection_skip_reason=full_lifecycle_hook_authority`。
- `pane process-info` 显示 grok-pi 的 foreground PGID=28841，其中一个 Node 子进程 PID=28889、`argv0=pi`。此证据不能单独证明 Herdr 内部 `detected_agent` 一定等于 Pi。
- Herdr `src/terminal/state.rs` 的 full lifecycle route 还要求有效进程、已锚定的会话、非 stale/exit suppression 以及可接受的 seq；Socket API 成功 JSON 不保证事件已进入 authority。

## 已落地的最小修复

在 `extensions/pi-grok-herdr/index.ts`：

1. Session 和 state 共用串行 FIFO；每条报告入队时固定 session ref 和单调递增 seq，确保 `session_start` 和 `agent_start` 都先送 session 再送 state。
2. 解析 newline-delimited JSON 应答，校验请求 ID、`result` 存在且无 `error`。错误/断连重试一次（保留相同 ID 和 seq），失败只输出一次诊断到 Pi RPC stderr。
3. 不阻塞 Pi 的事件处理；相邻未发出的状态可以合并，但不能越过 session 报告；新会话重置旧的 blocked 深度。
4. 保留 `ctx.hasUI` RPC 入口、`herdr:pi` / `pi` 配对、原有 idle/working/blocked 映射和不主动 release 的设计。

## 早期验证（此段为修复提交时的历史记录）

- `node --test extensions/pi-grok-herdr/index.test.mjs`：4/4，通过。覆盖非 Herdr no-op，session→state 和 working/blocked/idle，`agent_start` 与 session replacement 有序，延迟应答阻塞后续请求，API error 和拆包响应。
- `node --check extensions/pi-grok-herdr/index.test.mjs` 与 `git diff --check`：通过。
- 修复提交时没有运行 Cargo、重建或重启运行中的 grok-pi（项目 AGENTS.md 限制）。**后续 beta.4 的新 pane 验收结果见本文顶部复核章节**；两个现存异常 pane 仍不能视为已恢复。

## 新 pane 验收门槛

已用现有 beta.4 在**全新的独立 Herdr pane** 完成启动时的 authority 验收；不能用已经运行的旧进程判断新代码。日常复验收集：

```sh
herdr pane process-info --pane <PANE_ID>
herdr agent explain <PANE_ID> --json
herdr agent list
```

启动时已看到 `agent=pi`、`herdr:pi` session ref、`screen_detection_skipped=true`、`screen_detection_skip_reason=full_lifecycle_hook_authority`；**Working → Idle、Blocked → 恢复仍需真实交互验收**。若 Socket 已确认 `result` 但 authority 仍不成立，在本项目内检查启动时序、session ref、seq 和桥接注入；**不得把 API 成功误读为 Herdr 已接纳。**

## 部署与回滚

Rust binary 以 `include_str!` 嵌入这份 TypeScript，因此更改源码不会影响已启动 pane；需要后续授权构建并启动新进程。回滚只需恢复扩展对应提交并重建，既不迁移会话数据也不修改 Herdr。
