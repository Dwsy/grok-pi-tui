# Subagents V2 Team 协作

Subagents V2 是 grok-pi 内置 Pi 子代理上的**可选团队协作层**。它增加稳定 agent path、peer messaging、nested spawn、可复用 child-session 上下文和外置 team preset，同时不替代现有 V1 子代理工具，也不替代 Rhai Workflow。

V2 **默认关闭**。如果只需要 `spawn_subagent`、`/subagents`、wait/cancel/history 或主→子消息，保持 V2 关闭即可。

## 启用与回滚

宿主的 **Pi subagents** 功能必须开启。该功能默认开启，可在 **F2 → Agent → Pi subagents** 控制；修改后需要完整重启进程。

启动新进程时开启 V2：

```bash
PI_GROK_SUBAGENTS_V2=1 grok-pi
```

本地构建：

```bash
PI_GROK_SUBAGENTS_V2=1 ./target/debug/grok-pi
```

V2 是进程级开关，不会热加载到已经运行的 grok-pi。

需要立即回滚时，下次启动不要设置 `PI_GROK_SUBAGENTS_V2=1`。V1 继续可用，V1 持久化 schema 不需要迁移。

## 快速使用

1. 开启 V2 并启动 grok-pi。
2. 执行 `/subagent-teams` 查看发现到的 preset。
3. 让 root agent 用 `spawn_team` 启动 `research`、`implementation`、`review` 等 preset，或用 `spawn_team_agent` 单独创建协作者。
4. 用 `team_list` 查看 team tree 和状态。
5. 只需要把事实/上下文交给另一个 agent、但不想唤醒 idle agent 时，用 `team_send_message`。
6. 需要 recipient 真正继续工作时，用 `team_followup_task`。
7. 等待团队活动时优先用 `team_wait`，不要 busy-poll。
8. 不再需要的 queued/running 工作用 `team_interrupt` 停止。

## Identity 模型

V2 明确分离**团队身份**与 **V1 单次运行身份**：

| Identity | 稳定性 | 用途 |
|---|---|---|
| `/root/...` agent path | 在当前 root Pi session / extension runtime 内稳定 | V2 寻址与团队拓扑 |
| Pi child session | completed → follow-up 之间复用 | 保留 agent 对话与上下文 |
| V1 subagent UUID | 每次重新激活生成新 ID | Pager 原生 lifecycle、Tasks、history、cancel 记账 |

例如 `/root/reviewer` 完成后进入 `IDLE`。后续 `team_followup_task` 仍使用 `/root/reviewer`，也继续复用原来的 Pi child session 历史，但会创建新的 V1 run UUID。原因是原生 Pager 对已经完成的 task/subagent ID 有 tombstone，不能把同一个 run ID 复活。

Agent path segment 只允许小写字母、数字和下划线，`root` 保留：

```text
/root
/root/researcher
/root/researcher/verifier_2
```

给 sibling 发消息时使用绝对 path。相对 target 表示发送者自己的直接 child。创建、fork、切换 root Pi session 或 reload extensions 都会替换 extension runtime，因此 V2 path/team topology 不跨这个边界保留。

## 状态模型

`team_list` 会显示：

- `QUEUED`：等待共享后台并发额度。
- `RUNNING`：child turn 正在运行。
- `IDLE`：上一轮成功完成，可用 `team_followup_task` 重新激活。
- `FAILED`：上一轮失败，不会被静默自动复活。
- `CANCELLED`：上一轮被中断，不会被静默自动复活。

Root 显示为 `ROOT`。

## Tool 语义

| Tool | 语义 |
|---|---|
| `spawn_team_agent` | 异步创建一个稳定 `/root/...` path 的直接 child。child 同样获得 V2 control-plane tools，因此可以 nested spawn。 |
| `spawn_team` | 从外置 preset 启动整组成员。完整 roster 注册成功后才统一开跑；后续 member 创建失败时会回滚此前已创建成员，避免半个 team 残留。 |
| `team_send_message` | 发送 `MESSAGE`。它是 queue-only：对 `IDLE` child 只把语义消息写入其 session，不启动新 turn；running child 走 Pi steer queue。 |
| `team_followup_task` | 发送 `NEW_TASK`。running recipient 排入 follow-up；还在 concurrency queue 的 recipient 会延后到已排队任务完成后再执行；`IDLE` recipient 在同一个 child session 上重新激活，但使用新的 V1 run ID。 |
| `team_wait` | 等待 coordinator activity，不 busy-poll。默认 120 秒，可用范围 1–600 秒；spawn/message/completion/interrupt 都可提前唤醒。 |
| `team_list` | 列出稳定 path、role 和当前状态。 |
| `team_interrupt` | 中断 queued 或 running 工作。queued work 会在真正执行前从队列移除；已终止 agent 返回稳定终态。 |

### 持久化边界

`/root/...` 路径与 team topology 只在当前 grok-pi 进程生命周期内稳定。Child session JSONL 仍是持久化历史存储；在新进程中恢复 parent session 可以查看 child history，但不会重建上一进程的 V2 team tree、路径寻址关系或 pending work。

### FINAL_ANSWER 自动回传

Child run 结束后，最终文本会作为 `FINAL_ANSWER` 自动交给 parent。

- parent 正在运行：结果进入 follow-up 上下文。
- parent 已 `IDLE`：V2 会在同一个 parent child session 上重新激活，避免 nested child 结果丢失。
- parent 是 root：通过 root extension message channel 投递。
- parent 已 `FAILED`/`CANCELLED`：不会静默复活；delivery failure 会记录在 child run 上。

因此 nested delegation 不需要 parent 轮询 JSONL 才能拿到结果。

## 并发与队列

V1/V2 共用 subagent runtime 的后台并发上限：**每个 root 进程最多 4 个 active background run**。额外工作进入队列。

队列支持安全取消：queued run 被 cancel 后会在执行前移除。对一个仍在 concurrency queue 中的 agent 发送 `team_followup_task` 时，新任务不会绕过并发上限抢跑，而是作为 pending team work 等当前已排队 run 完成后再执行。

单个 team preset 最多 **8 个成员**。`spawn_team` 会先注册完整 roster 再启动成员，避免早启动的 agent 给尚未注册的 sibling 发消息失败。

## Team preset

Team preset 是 JSON 文件，发现顺序如下；同名定义由后加载的更高优先级 scope 覆盖：

1. bundled：`extensions/pi-grok-subagents/teams/*.json`
2. global：`~/.grok-pi/teams/*.json`（或 `$GROK_HOME/teams/*.json`）
3. project：`<repo>/.grok-pi/teams/*.json`（或 `$GROK_PROJECT_DIR/teams/*.json`）

因此优先级是 **project > global > bundled**。

内置 preset：`research`、`implementation`、`review`。

### Team JSON contract

```json
{
  "name": "implementation",
  "description": "Implementation plus review",
  "enabled": true,
  "instructions": "Share concrete file paths and verify each other's claims.",
  "members": [
    {
      "name": "implementer",
      "agent": "general-purpose",
      "description": "Primary implementer",
      "task": "Implement the objective: {{task}}",
      "model": "openai/gpt-5.6",
      "max_turns": 12
    },
    {
      "name": "reviewer",
      "agent": "explore",
      "task": "Review {{task}}. The implementer is {{parent_path}}/implementer."
    }
  ]
}
```

规则：

- `name` 可省略，默认取文件名；允许字母、数字、`_`、`-`，匹配时不区分大小写。
- `enabled` 默认 `true`。高优先级 scope 可以用同名 `enabled: false` 禁用低优先级 preset。
- enabled team 必须有 `members`，数量 1–8。
- member `name` 必填、team 内唯一，只允许小写字母/数字/下划线，并直接成为 path segment。
- member `agent` 是外置 agent definition 名，默认 `general-purpose`。
- member `model` 可选；如果 agent definition 有 model allowlist，则必须在 allowlist 中。
- member `max_turns` 是可选非负整数；agent definition 自己的 `max_turns` 优先级更高。
- member `task` 支持 `{{task}}`、`{{team}}`、`{{agent_path}}`、`{{parent_path}}`。

Malformed preset 采用 fail-closed 语义：它不会阻断其它无关 preset 加载，但高优先级 scope 中损坏的文件会按同名文件 stem 遮蔽低优先级 preset，而不是静默 fallback。例如 project `implementation.json` 无效时，bundled `implementation` 会保持隐藏，直到该 project 文件被修复或删除。`/subagent-teams` 只列出已接受定义；preset 意外缺失也可能表示被 malformed 高优先级文件遮蔽。

## Agent definition

Team JSON 负责拓扑，不负责业务工具权限。Agent profile 继续使用 Markdown：

- project：`<repo>/.grok-pi/agents/*.md`
- global：`~/.grok-pi/agents/*.md`

Project definition 覆盖 global，包括 `enabled: false`。

示例：

```markdown
---
description: Read-only architecture reviewer
enabled: true
tools: ["read", "grep", "find", "ls"]
models: ["openai/gpt-5.6"]
extensions: []
skills: []
max_turns: 10
---

Review architecture and correctness. Do not edit files. Return evidence with file paths.
```

Markdown definition 控制 system prompt、业务 tools、最多 3 个 models、extensions、skills 和 max turns。V2 control-plane tools 独立注入，所以 read-only reviewer 仍能参与 team 通信，但不会因此获得 edit/write 权限。

使用 `/subagents` 管理原生 agent definition。

## 消息与持久化边界

V2 语义消息使用 `pi-grok-team-message/v2` custom message，并且有意进入 recipient 的模型上下文。

现有 `pi-grok-subagent/v1` bridge 继续只负责 UI/lifecycle。每个 run 只产生有界的 `spawned` / `finished` lifecycle；V2 不恢复 `progress` / `child_update` 高频写 parent JSONL 的旧路径。

Pi child-session JSONL 仍是持久化对话/history 存储，但**不是**实时 team message bus。实时投递由进程内 coordinator + Pi 官方 session API 完成。

## 与 Rhai Workflow 的关系

需要长期 agent identity、peer communication、nested delegation、动态协作时，用 **Subagents V2**。

需要确定性步骤、显式分支、脚本化 control flow 时，用 **Rhai Workflow**。

两者互补。Workflow 可以委派给 subagent；team preset 不是第二套 workflow engine。

## 产品运行建议

- 在目标 model/provider 完成真实 multi-agent handtest 前，保持 V2 opt-in，不默认开启。
- Team 尽量小、角色职责明确；agent 越多，context/tool/queue 压力越大。
- 每个 member 使用窄 task template，并配 least-privilege agent definition。
- 等待团队结果优先 `team_wait`，避免反复 `team_list` polling。
- 传事实/上下文用 `team_send_message`；只有确实需要新 turn 时才用 `team_followup_task`。
- 把 `FAILED`、`CANCELLED` 当作显式终态，不假设自动恢复。
- 仓库专用行为放 project scope；global scope 只放真正可复用的默认配置。

## 排障

### `/subagent-teams` 不存在

依次检查：

1. F2 的 **Pi subagents** 已开启。
2. 启动进程时设置了 `PI_GROK_SUBAGENTS_V2=1`。
3. 修改 feature 后完整重启了进程。
4. 当前进程没有禁用 bundled bridge extensions。

### Team preset 没有出现

- 文件必须以 `.json` 结尾并且是合法 JSON。
- member name 只能使用小写字母、数字、下划线。
- enabled team 必须有 1–8 个成员。
- 检查 project/global 是否存在同名覆盖。
- 检查最终生效定义是否 `enabled: false`。

### Agent 无法启动

- 确认引用的 agent definition 已启用。
- 确认指定 model 当前存在于 Pi model registry。
- 如果 agent definition 配置了 model allowlist，确认 team member 的 model 被允许。
- 用 `team_list` 检查 path 是否已存在；同一 coordinator session 内稳定 path 唯一。

### 发了消息但没有开始工作

如果是 `team_send_message` 发给 `IDLE` agent，这是预期行为。需要新 turn 时使用 `team_followup_task`。

### 一直显示 QUEUED

共享后台并发上限是 4。用 `team_wait` 等待 activity，或 `team_interrupt` 停掉不再需要的工作。


## 可复现的本地检查

在仓库根目录执行 V2 专项自动化检查：

```bash
cd extensions/pi-grok-subagents
bun test v2.test.ts runtime.test.ts
cd ../..
node extensions/pi-grok-bash/test-v2.1.mjs
cargo test -p xai-grok-pager-bin --bin grok-pi
```

Subagents 的 Bun 测试直接覆盖 coordinator/runtime；Rust binary suite 还会验证 bundled extension materializer 能把运行时所需的 TypeScript/JSON 依赖完整复制到临时 bundle。

当前独立 TypeScript 检查不是这个 checkout 的干净 gate：未显式接入 sibling Node types 时会先停在 `TS2688`（不可见 `@types/node`）；补上该依赖路径后，extension path mapping 又会进入 `pi-main` 并命中已有 baseline diagnostics。因此本增量的可复现自动化 gate 以 Bun tests + Rust materialization/loadability tests 为准；下面的真实模型 handtest 仍是独立的发布 gate，不能被静态测试替代。日期化结果和 blocker 见 `VERIFICATION.md`。

## 发布前验收清单

V2 从 opt-in 提升发布等级前，至少确认：

- V2 coordinator 与 runtime hardening 单测通过。
- Pi 能在 V2 开启时直接加载 extension。
- Rust injector 能物化所有 TS/JSON 依赖。
- `grok-pi` 构建通过。
- V2=off 时不暴露 `/subagent-teams` 和 V2 tools。
- V2=on 时能发现 bundled presets。
- 真实模型 handtest 覆盖 root→child、child→root、sibling message、nested spawn、idle follow-up、nested FINAL_ANSWER 唤醒、interrupt、queueing。
- Parent session 增长仍只来自语义消息和有界 lifecycle，没有恢复 progress/child-delta bridge。
