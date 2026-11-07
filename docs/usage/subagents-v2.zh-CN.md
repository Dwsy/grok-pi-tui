# Subagents V2 Team 协作

Subagents V2 是 grok-pi 内置 Pi 子代理上的**可选团队协作层**。它增加稳定 agent path、peer messaging、nested spawn、可复用 child-session 上下文和外置 team preset。V2 会**替换** V1 的单代理工具面，而不是在其之上叠加；V1 runtime 与命令仍然保留。

V2 **默认关闭**。如果只需要 `spawn_subagent`、`/subagents`、wait/cancel/history 或主→子消息，保持 V2 关闭即可；开启后 `spawn_subagent` 等 5 个 V1 工具不再注册（见下文「V2 模式下不注册 V1 工具」）。

## Multitask 技能模式

开启 Pi subagents 与 V2 后调用：

```text
/skill:multitask 实现 CSV 导出，并安排独立审查。
```

技能只对本次任务树选择工作策略。每个 child 经任务书明确授权后，可以在分配的深度和额度内拆分独立工作；派发后继续本地执行。到依赖点且没有本地工作时，child 调用 `team_report(state="waiting", summary="...")` 并结束 turn；`team_list` 显示 `WAITING_RESULT`，下级结果恢复原 child session。root 直接结束当前 turn，等待正常异步结果唤醒。

report 本身不释放槽，必须结束 turn；不要用 `team_wait` 占槽等排队后代，也不要给 `spawn_team.timeout_ms` 传正值。waiting 要求自己的下级仍待完成；需父级决定时使用 blocked + question。child 的 dropped 会恢复已经 waiting 的 parent；waiting/dropped 不会掩盖运行失败或取消。

V1 不暴露该技能。root 读技能不等于 child 已加载，派发时须明确复制精简契约。默认两层 child、全树最多新建八个 identity 是模型软约定；runtime 仍硬限全树四个并发后台 run。所有 Agent 共用 cwd，每个文件应指定唯一 writer。

源码构建修改内置技能后需重新构建 grok-pi。当前能力不包含 session-wide 开关、忙碌输入队列转并行、硬树预算、worktree 隔离、取消级联或跨进程恢复。参见 [设计调研记录](../issues/adapter/20261102-subagents-v2-multitask.md) 中的现有实现与后续硬化方案。

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
2. 执行 `/subagent-teams` 配置 preset、成员模型与思考强度。
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
- `WAITING_DECISION`：上一轮以 `blocked` report 结束。问题保留在 `team_list` 里，消息仍能送达，`team_followup_task` 可像 `IDLE` 一样重新激活它。
- `FAILED`：上一轮失败，不会被静默自动复活。
- `CANCELLED`：上一轮被中断，不会被静默自动复活。

每行还带静默时长（`quiet 42s`）：距它上次收发 team 消息的时间。`RUNNING` agent 静默过久是“该去看”而不是“继续等”的信号。

Root 显示为 `ROOT`。

## Tool 语义

| Tool | 语义 |
|---|---|
| `spawn_team_agent` | 异步创建一个稳定 `/root/...` path 的直接 child。child 同样获得 V2 control-plane tools，因此可以 nested spawn。支持 `agent_type`、`description`、`model`、`max_turns`、`thinking_level`、`capability_mode`。 |
| `spawn_team` | 从外置 preset 启动选定成员。完整 roster 注册成功后才统一开跑；后续 member 创建失败，或启动前收到取消，都会回滚已创建成员，避免半个 team 残留。除 `team`、`task` 外还支持 `members`、`model`、`thinking_level`、`max_turns`、`capability_mode`、`instructions`、`timeout_ms`。 |
| `team_send_message` | 缓冲 `MESSAGE`，不启动 idle recipient 的 turn。同一 sender 到同一 target 的连续消息合并成一条 steer（编号列出，重复文本去重），在 sender 下一次 team 动作前或 turn 结束时发出。 |
| `team_followup_task` | 发送 `NEW_TASK`。running recipient 排入 follow-up；还在 concurrency queue 的 recipient 会被提到共享队列最前再执行它的待跑任务，父级明确要求的工作不必排在无关 launch 后面；`IDLE` 与 `WAITING_DECISION` recipient 在同一个 child session 上重新激活，使用新的 V1 run ID。 |
| `team_wait` | 等待 coordinator activity，不 busy-poll。默认 120 秒，可用范围 1–600 秒；spawn/message/completion/interrupt 都可提前唤醒。 |
| `team_list` | 列出稳定 path、role、当前状态、最近的 report 或待决问题，以及静默时长。会先 flush 缓冲消息，使显示与已发送内容一致。 |
| `team_interrupt` | 中断 queued 或 running 工作。queued work 会在真正执行前从队列移除；已终止 agent 返回稳定终态。 |
| `team_report` | 声明本轮结束方式，让 parent 只在必须行动时被唤醒。仅 child 拥有；root 没有 parent。 |

### `spawn_team` 调用级参数

| 参数 | 作用 |
|---|---|
| `team`、`task` | preset 名称，以及代入各成员模板 `{{task}}` 的共享目标。 |
| `members` | 可选的成员子集。保持 preset 声明顺序，因此定义在最后的 synthesizer 仍在其生产者之后启动。出现未知成员名时，在创建任何 child 之前就失败。 |
| `model`、`thinking_level`、`max_turns` | 覆盖所有选中成员的对应值。不传则保留成员自身的 preset 值。 |
| `capability_mode` | 所有选中成员的工具权限：`read-only` 或 `all`。两者都含 Bash；`read-only` 省略内置 `edit`/`write` 并注入只读契约。旧的 `execute`/`read-write` 静默映射为 `all`。默认取 agent definition，再回退 `all`。 |
| `instructions` | 追加到 preset `instructions` 的团队级补充指令，写入每个成员的系统提示。 |
| `timeout_ms` | `0` 或省略时立即返回，最终答案异步回传 `/root`。正数最多等待 600 秒，并在工具结果里返回各成员状态与输出。 |

成员的长期差异应写入 preset JSON 或通过 `/subagent-teams` 编辑；调用级覆盖只影响本次运行（例如临时换更便宜的模型，或整组降为只读），不会持久化。

### V2 模式下不注册 V1 工具

开启 V2 是替换模型可见的工具面，而不是叠加。`spawn_subagent`、`send_message_to_subagent`、`get_command_or_subagent_output`、`kill_command_or_subagent`、`list_subagents` 都**不再注册**。原因是 `spawn_subagent` 会阻塞 root 直到 child 完成，而 team 模式要求 root 结束回合、由 `FINAL_ANSWER` 重新激活。

对应能力由 V2 覆盖：spawn 用 `spawn_team_agent`/`spawn_team`，发消息用 `team_send_message`/`team_followup_task`，等待用 `team_wait`，取消用 `team_interrupt`，查看用 `team_list`。

V1 的 **runtime 与命令**仍然保留：`/subagents`、`/subagent-message`、`/subagent-history` 继续可用，隐藏的 `__pi_grok_subagent_replay` / `__pi_grok_subagent_cancel` bridge 命令也保持注册，因为 Rust adapter 依赖它们才启用 subagent bridge。

### 持久化边界

`/root/...` 路径与 team topology 只在当前 grok-pi 进程生命周期内稳定。Child session JSONL 仍是持久化历史存储；在新进程中恢复 parent session 可以查看 child history，但不会重建上一进程的 V2 team tree、路径寻址关系或 pending work。

### 用 `team_report` 声明结果

Child 用 `team_report` 结束一轮：

| `state` | 含义 | 是否唤醒 parent |
|---|---|---|
| `ready` | 请求的结果可交付 | 是 |
| `blocked` | 需要 parent 决策，`question` 必填 | 是 |
| `failed` | 工作无法完成 | 是 |
| `dropped` | parent 无事可做 | **否** |

report 会替换 child 的最终文本：投递内容变成 `Report: <state>`、一句 `summary`，有 `question` 时追加 `Needs your decision: <question>`。不调 `team_report` 的 child 保持旧行为：最终文本无条件投递。

`dropped` 是唯一静默状态。agent 停在 `IDLE`，`team_list` 保留 summary，不消耗 parent 的 turn。适用于：预设成员发现没有需要改的东西、请求已被满足、或另一个 agent 已接管。

`team_report` 只为 child 注册；root 没有 parent，工具面里不出现。

重新激活 agent 会清空上一轮的 report。新 run 不会重放旧 summary，残留的 `dropped` 也不会吞掉新结果。

### FINAL_ANSWER 自动回传

Child run 结束后，结果会作为 `FINAL_ANSWER` 自动交给 parent。

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
      "max_turns": 12,
      "thinking_level": "high"
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
- member `thinking_level` 可选，取值 `off` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`。省略时先看 agent definition 的 `thinking_level`，再看 Pi 全局设置。模型不支持该等级时会自动 clamp 到最接近的支持等级，不会让整组启动失败。
- member `task` 支持 `{{task}}`、`{{team}}`、`{{agent_path}}`、`{{parent_path}}`。

Malformed preset 采用 fail-closed 语义：它不会阻断其它无关 preset 加载，但高优先级 scope 中损坏的文件会按同名文件 stem 遮蔽低优先级 preset，而不是静默 fallback。例如 project `implementation.json` 无效时，bundled `implementation` 会保持隐藏，直到该 project 文件被修复或删除。`/subagent-teams` 只显示已接受定义；preset 意外缺失也可能表示被 malformed 高优先级文件遮蔽。

## 用 `/subagent-teams` 配置模型与思考强度

`/subagent-teams` 是完整交互式编辑器，不是只读列表：

1. 选择 `New project team` / `New global team`，或已有 preset。
2. 进入 `Members`，`Add member` 后用成员名创建（小写字母、数字、下划线）。
3. 选择成员 → `Edit settings`：
   - `Agent`：agent definition 名。
   - `Model`：首项是 **All available models (default)**，其后是所有当前可用模型 `provider/model`。选择默认时不会写入 `model`，由 agent definition / Pi 决定。
   - `Thinking level`：首项是 **Inherit Pi settings (default)**，其后只列出所选模型真正支持的等级（按 `reasoning` 与 `thinkingLevelMap` 推导）。未选模型时列出全部可用模型的等级并集。
   - `Max turns`：留空继承 agent definition，`0` 表示不限制。
4. `Save and close` 写入 project / global scope。

bundled preset（`research`、`implementation`、`review`）是只读的：编辑时会先要求选择 **Project override** 或 **Global override**，不会原地改写安装目录里的文件。

模型与思考强度的发现完全发生在 TUI 里——本扩展**不注册任何向模型暴露的模型/思考强度列表工具**，所以不会占用模型上下文。

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
tools: ["read", "bash", "grep", "find", "ls"]
models: ["openai/gpt-5.6"]
thinking_level: high
extensions: []
skills: []
max_turns: 10
---

Review architecture and correctness. Do not edit files. Return evidence with file paths.
```

Markdown definition 控制 system prompt、业务 tools、最多 3 个 models、extensions、skills、max turns、默认 `thinking_level` 与继承开关。V2 control-plane tools 独立注入，所以 read-only reviewer 仍能参与 team 通信。**只保留两种 capability**：`read-only`（精简工具集 + 注入 `READ_ONLY_CONTRACT`）与 `all`（全量内置工具）。两者都包含 Bash，definition 显式配置 `tools:` 时也会补入。旧的 `execute` / `read-write` 值静默映射为 `all`，既有 preset 与定义照常可用。自定义工具列表若同时不含 `edit` 和 `write`，按只读工具集处理。agent 的限制来自注入的 system prompt（只读契约与各 profile 自身指令），而不是隐藏 Bash；这是提示词级约束，不是 OS 级强制。

使用 `/subagents` 管理原生 agent definition，`/subagent-control` 打开统一 TUI 控制中心（状态/定义/preset/注入预览）。

### 内置 profile 的工具权限

| profile | capabilityMode | 可用工具 | 行为约束 |
|---|---|---|---|
| `general-purpose` | `all` | `read` `bash` `edit` `write` `grep` `find` `ls` | 可完整实施改动 |
| `read-only` capability | `read-only` | `read` `bash` `grep` `find` `ls` | 省略内置 `edit`/`write`；注入 `READ_ONLY_CONTRACT` 禁止文件和状态修改 |
| `explore` | `all`（默认） | `read` `bash` `edit` `write` `grep` `find` `ls` | 全量工具，但自身 system prompt 禁止修改 tracked files；可用 Bash 跑 `rg`/`git log`/`tree`/`wc`/`python3 -c`/`node -e` |
| `plan` | `all` | `read` `bash` `edit` `write` `grep` `find` `ls` | 全量工具，但自身 system prompt 明确「Do not edit files」 |

`execute` 与 `read-write` 不再是独立模式：`requireCapability()` 把它们静默映射为 `all`，既有 preset 与 agent 定义无需改动即可继续用；旧的持久化快照也仍可加载。

除 `read-only` 省略 `edit`/`write` 的工具级裁剪外，其余约束都是**提示词级**，不是 OS 级强制：Bash 可以执行任意命令，因此注入的 system prompt（只读契约，或各 profile 自身指令）才负责禁止文件写入、Git 状态修改、依赖安装、数据库写入及其他状态变更。需要硬隔离时必须使用外部 OS/container sandbox；本扩展本身不提供沙箱。


### 子代理默认扩展

Child session 不加载自动发现的扩展，所以 root 侧装好的工具在子代理里会缺失。以下扩展存在时会被显式加上：

| 扩展 | 提供 |
|---|---|
| `~/.pi/agent/npm/node_modules/@ff-labs/pi-fff` | FFF 模糊文件与内容搜索 |

目录不存在时静默跳过，所以未安装不构成启动错误。agent definition 的 `extensions:` 会在此基础上追加。扩展在绑定时注册的工具会叠加在 session 自身 loadout 之上而不是替换它，因此 `capability_mode` 仍然成立。

### Child 的 skills 与 AGENTS.md 继承（默认开）

Child session 默认继承 root 的全局/项目 skills 与 AGENTS.md/CLAUDE.md 上下文，definition 的显式 `skills:` 在此基础上叠加。恢复隔离：

```md
---
description: "isolated worker"
inherit_skills: false
inherit_context: false
---
```

- `inherit_skills: false` → 只加载 `skills:`（`noSkills`）。
- `inherit_context: false` → 不做 AGENTS.md 发现（`noContextFiles`）。
- 扩展保持隔离：只加载 `extensions:`（+ 已安装的默认搜索扩展）。

### Child 的 prompt 组成

Child session 走 `systemPromptOverride`，Pi 只渲染 **preamble**：默认的 `tools` 与 `rules` 段被跳过。后果：

- 工具的 `promptSnippet` 与 `promptGuidelines` **只对 root session 生效**，永远到不了 child。
- child 的全部行为说明 = `definition.systemPrompt` + V2 `usageHint` + 每个工具的 `description`。
- 默认规则（“Be concise”“Show file paths clearly”）在 child 里不存在，所以 `usageHint` 显式写了证据规则。

面向 child 的规则要写进 `usageHint` 或工具的 `description`。加 `promptGuideline` 不会改变 child 行为。

## 消息与持久化边界

V2 语义消息使用 `pi-grok-team-message/v2` custom message，并且有意进入 recipient 的模型上下文。

现有 `pi-grok-subagent/v1` bridge 继续只负责 UI/lifecycle，但全部 lifecycle 与 child update 统一走进程私有有序 socket。恢复快照写入独立 `<parent-session>.subagents.jsonl` sidecar；V1 bridge/state 不再追加到 parent Pi JSONL。

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

### `spawn_subagent` 不存在

开启 V2 时这是预期行为，见「V2 模式下不注册 V1 工具」。下一进程不带 `PI_GROK_SUBAGENTS_V2=1` 启动即可恢复 V1 工具面。

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
- Parent session 增长只来自真实对话语义；V1 UI lifecycle/state 位于 socket + sidecar，不进入 parent Pi JSONL。
