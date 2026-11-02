---
name: multi-agent-proactive
description: 为 grok-pi 顶层父会话的 MultiAgentMode::Proactive / Ultra 提供主动委派与协调规范，适配 V1 spawn_subagent 与 V2 team 工具。仅父级编排器因该模式主动 fan-out；子代理不会因为父级启用 Proactive/Ultra 而继承主动模式。用于用户要求主动并行、多代理协作，或需要决定何时 spawn / message / follow-up / wait / report 时。
---

# Multi-Agent Proactive for grok-pi

## 核心契约

Proactive / Ultra 是**父级编排策略**，不是会沿 agent tree 传播的能力标记。

- 顶层父会话可以主动寻找真正独立、能缩短关键路径或提高质量的工作并委派。
- V1/V2 child 只执行收到的任务；**不得因为父级处于 Proactive/Ultra 就继续主动 fan-out**。
- V2 child 即使拥有 `spawn_team_agent` 等 team tools，也只在自己的任务或父级消息**明确要求进一步协调**时使用；工具可用不等于主动模式继承。
- agent definition 显式配置 `skills` / `extensions` 属于明确 opt-in，不属于继承；仍服从该 definition 的边界。
- 用户明确要求单 agent、不并行或限制修改范围时，立即遵守。

一句话：**父级主动派，子级专注做；除非任务明确要求，child 不再主动派下一层。**

## 先选对工具面

只使用当前实际可见的工具，不猜测未启用能力。

### V1：独立 child session

| 目的 | 工具 |
|---|---|
| 创建独立子任务 | `spawn_subagent` |
| 给运行中 child 补信息 | `send_message_to_subagent` |
| 等待/读取后台结果 | `get_command_or_subagent_output` |
| 查看当前 child | `list_subagents` |
| 取消 child | `kill_command_or_subagent` |

V1 适合彼此独立的研究、实现、验证、审查。默认 child 是叶子任务，不把多代理编排责任继续往下传。

### V2：有通信需求的 team tree

| 目的 | 工具 |
|---|---|
| 创建命名 team child | `spawn_team_agent` |
| 给运行中 agent 发语义消息 | `team_send_message`（连续多条合并成一条 steer） |
| 给已有 agent 新任务并触发 turn | `team_followup_task`（对排队中 agent 会插队） |
| 等待 team activity | `team_wait` |
| 查看 team tree | `team_list`（含静默时长与待决问题） |
| 中断运行中 agent | `team_interrupt` |
| 声明本轮结果 | `team_report`（`ready`/`blocked`/`failed`/`dropped`；只有 `dropped` 不唤醒 parent） |
| 启动预设团队（root 可见时） | `spawn_team`（可加 `members` 子集、`model`/`thinking_level`/`max_turns`/`capability_mode` 统一覆盖、`instructions`、`timeout_ms`） |

V2 适合 agent 之间需要消息、follow-up、稳定 `/root/...` 身份或预设团队的场景。V1 与 V2 工具面互斥：V2 开启时不注册 V1 单代理工具，因此不要在本会话里寻找 `spawn_subagent`。若 V2 工具未出现，就使用 V1；不要为了“更高级”而绕过实际工具目录。

## 第一原则：委派必须有收益

每次新任务或新阶段只快速检查四件事：

1. **是否真独立**：子任务能否在很少共享细节的情况下闭环？
2. **是否缩短关键路径**：派出去后，父级是否还能立刻推进另一项有价值的工作？
3. **是否提高质量**：是否值得用独立调查、验证或审查降低高风险假设？
4. **协调成本是否可控**：会不会读同一批文件、争抢同一热点写入或需要频繁同步？

没有明显收益就自己做。Proactive 不是“必须 spawn”；Ultra 也不是“填满所有并发槽位”。

## 选择 V1 还是 V2

按最小够用原则：

1. 一个独立闭环、结果回父级即可 → V1。
2. 多个独立闭环、无需互相通信 → 多个 V1 background child。
3. agent 之间必须互相发消息、追任务或形成稳定层级 → V2。
4. 已有 team preset 正好匹配重复协作结构 → `spawn_team`。

不要用 V2 模拟一个本来只需 V1 的叶子任务。

## 委派单位：单一闭环

每个 child 都应收到自包含、单一交付物、可验收的任务书。至少包含：

```text
背景: [用户目标；父级当前正在做什么]
仓库: [绝对路径；必要时补充技术栈]
已知: [已经确认的事实、关键文件/函数、已排除假设]

任务: [唯一闭环目标和完成标准]

边界:
- [允许修改的模块/文件；只读任务明确写不要改文件]
- [禁止事项，例如不要 commit、不要加依赖]
- [和其他 agent 的责任边界]
- 不要因为父级处于 Proactive/Ultra 而继续主动创建子代理；只有本任务明确要求进一步协调时才使用 team spawn。

验证:
- [具体命令或证据标准]

汇报:
- 结束前调用 team_report 声明 state：ready（可交付）/ blocked（需父级决定，必附 question）/ failed / dropped（父级无事可做）
- summary 一句话说清结果，不是工作汇报
- 结论/根因
- 修改文件（如有）
- 验证结果关键输出
- blocker 或反证
```

任务书必须让 child 在零额外解释的情况下开工。不要依赖“它继承了我的上下文/模式”。

## V1 标准循环

### Spawn

优先 1–3 个最高价值的独立工作流。`spawn_subagent` 的 `prompt` 必须自包含；需要并行时使用 `background: true`。

可按角色选择：

- `explore`：只读调查/诊断
- `plan`：方案与风险梳理
- `general-purpose`：需要完整执行能力的闭环

仅在确有必要时覆盖 `model`、`capability_mode`、`max_turns`。

### 并行推进

```text
spawn_subagent(research, background=true)
spawn_subagent(verification, background=true)
→ 父级继续 implementation
```

不要：

```text
spawn_subagent(research)
→ 立刻等待
→ spawn_subagent(verification)
→ 再等待
```

除非第二步真的依赖第一步。

### 中途修正

运行中的 V1 child 需要新事实或纠偏时，用 `send_message_to_subagent`：

- `follow_up`：当前 turn 后继续
- `steer`：立即打断并改方向

不要为高度相关的补充问题重复 spawn 新 child。

### 汇合

只有下一关键步骤必须依赖结果、且父级没有别的本地工作时，才调用 `get_command_or_subagent_output` 等待。使用合理的长 `timeout_ms`，不要 busy polling。

## V2 标准循环

### Spawn

父级用 `spawn_team_agent` 创建稳定路径 child，例如：

```text
/root/research
/root/implementation
/root/review
```

只在通信/层级本身有价值时用 V2。

预设结构正好匹配时用 `spawn_team`：

```text
spawn_team(team="research", task="定位 X 崩溃的根因")
spawn_team(team="research", task="...", members=["researcher","synthesizer"])
```

- `members` 子集保留预设声明顺序，synthesizer 仍在其生产者之后启动。但成员任务模板可能引用未启动的 agent，选子集时要在 `task` 或 `instructions` 里说明协作范围。
- `model` / `thinking_level` / `max_turns` / `capability_mode` 是**本次调用**的统一覆盖，不是 per-member；成员的长期差异写进预设 JSON 或用 `/subagent-teams` 编辑。
- 默认不传 `timeout_ms`：立即返回，最终答案异步回传。只有确实需要同步收集时才传。

### 并行推进

V2 下 root 的正确做法是**派完就结束 turn**，而不是停下来等：

```text
spawn_team(team="research", task="...")
→ 继续本地不依赖它的工作
→ turn 结束
→ child 的 FINAL_ANSWER 重新唤醒 root
```

不要在 root 调 `team_wait` 停车——它对 root 直接返回，写了也不等。也不要为同一个目标重复 spawn。

### Message vs Follow-up

- `team_send_message`：给**正在运行**的 agent 补事实，不主动制造一个新的 idle turn。
- `team_followup_task`：已有 agent 完成一轮后，需要利用它已有上下文继续做新的相关任务。
- `team_wait`：只给 child 做 mailbox 同步屏障；root 启动后台 V2 agent 后不要用它停车，直接结束当前 turn，等待 `FINAL_ANSWER` 自动唤醒。
- `team_list`：需要确认路径、状态或责任归属时再看，不拿它做高频轮询。

### 消息要合并，不要刷屏

- 同一 sender 到同一 target 的连续 `team_send_message` 会合并成一条 steer，重复文本被丢弃。一个 turn 内要说的事一次说完，不要拆成多条。
- 合并后的消息在下一个 team 动作前或 turn 结束时发出；`team_list` 会先 flush。
- `team_followup_task` 发给排队中的 agent 时，会把它的待跑任务提到并发队列最前——父级明确要的工作不该排在无关 launch 后面。
- `team_list` 每行带 `quiet 42s`。`RUNNING` agent 静默过久是“该去看”而不是“继续等”的信号。

### 什么内容值得发

一条消息只在对方**无法自行获得**或**必须据此行动**时才发：

- 值得发：对方需要的事实（文件路径、复现条件、已排除的假设）、对方向你提出的问题的答案、他正在等的结果。
- 不值得发：进度汇报（“我在看 X”）、已经发过的内容、对方能自己读到的东西。

判断标准：删掉这条消息，对方会不会做错或做不下去？不会就不发。

### 收尾必须声明结果

child 在结束 turn 前调用 `team_report`，父级只为需要行动的结果付出 turn：

| `state` | 何时用 | 父级被唤醒 |
|---|---|---|
| `ready` | 要的结果可交付 | 是 |
| `blocked` | 需要父级做一个决定，`question` 必填 | 是 |
| `failed` | 做不成 | 是 |
| `dropped` | 无需改动、请求已被满足、或已被其他 agent 接管 | **否** |

`dropped` 不是“偷懒退出”，而是“父级无事可做”的诚实声明：它保留 summary 供 `team_list` 查看，但不消耗父级的 turn。没有产出就不要编产出。

不调 `team_report` 时，最终文本仍会无条件回传——这是旧行为，只在确实无法判断状态时使用。

### 父级如何处置 report

收到 child 的 report 后，父级必须做出处置，而不是只读一眼：

| child 报的 | 父级要做的 |
|---|---|
| `ready` | 验收 summary 与证据；需要细节再读它的 transcript |
| `blocked` | 读 `question`。能自己定就定，用 `team_followup_task` 回给同一个 child；要用户定就升级给用户 |
| `failed` | 判断是任务书问题还是能力问题；修正任务书重派，或自己接手 |
| `dropped` | 不追问。确认是否真无事可做；若是任务书给错了，重写后重派 |

`blocked` 的 question 是父级的责任，不是可忽略的状态。

### Child 的边界

V2 child 会拿到 team control tools，这是通信能力，不是 Proactive 继承。

child 默认行为：

1. 完成父级给它的任务。
2. 必要时给父级/同级发送消息。
3. 不主动 `spawn_team_agent` 扩树。
4. 只有任务文本、后续 `team_followup_task` 或用户要求明确授权进一步拆分时，才创建自己的 child。
5. 即使获准继续拆，也不得扩展原任务范围或制造写冲突。

## 状态解读

`team_list` 每行：`[STATUS] path (role) [preset] — detail — quiet Ns`

| 状态 | 含义 | 父级动作 |
|---|---|---|
| `QUEUED` | 在等并发额度 | 需要它先跑 → `team_followup_task` 会插队 |
| `RUNNING` | turn 正在跑 | 看 quiet；过长则 `team_interrupt` 或再等 |
| `IDLE` | 上一轮成功结束 | 有新工作 → `team_followup_task` |
| `WAITING_DECISION` | 上一轮 `blocked`，等父级决定 | 读 question → 给决定 |
| `FAILED` | 上一轮失败 | 不静默复活；重派或自己接手 |
| `CANCELLED` | 被中断 | 同上 |

`quiet` 是距上次收发 team 消息的时长：`RUNNING` + 长 quiet 可能卡住；`IDLE` + 长 quiet 是正常。

## 写入纪律

所有并行 agent 可能看到同一工作目录，因此写入必须主动隔离。

优先按以下边界切：

- 模块：frontend vs backend
- 文件：agent A 改 parser，agent B 只改独立 tests
- 职责：一个只读调查，一个 writer
- 阶段：先并行调查，再指定一个 owner 写热点文件

两个 agent 必须改同一个核心文件时，不要并行写；指定一个 owner，另一个只审查或提供建议。

V2 下这一点更严格：所有 agent 共享同一个 cwd（`usageHint` 会明确告知），`spawn_team` 一次起多个成员时先确认写域不重叠，否则并行写入会互相覆盖。只读成员（`explore` / `capability_mode: read-only`）不占写域。

## 父级必须保留的责任

不要把“整个任务 + 所有决策权”原样丢给 child。父级保留：

- 用户意图和范围解释
- 子任务优先级与写边界
- 冲突处理
- child 结果综合
- 关键验证复核
- 最终用户答复

child 的 final 是证据包，不自动等于最终答案。

## 结果验收

父级至少检查：

- 是否回答任务书中的问题
- 是否越界修改
- 声称执行的验证是否有真实输出
- 路径、函数和错误是否与代码一致
- 多个 child 是否给出矛盾结论
- child 的 `team_report` state 与其证据是否一致（说 `ready` 但没验证 = 不成立）
- 是否暴露新的 blocker

高风险改动由父级亲自复验。

## 常见反模式

### 过度并行

普通 bug 拆成大量微任务。修正：恢复为少量端到端闭环。

### Spawn 后立刻 Wait

仍有本地工作却立即等待。修正：先推进不依赖 child 的工作，在真正汇合点再等。

### 重复调查

多个 agent 读同样文件。修正：任务书写清已知事实和责任域，发现重叠立即重划边界。

### 并行改同一热点

共享工作区相互覆盖。修正：单 writer owner。

### Child 继承 Proactive 继续递归 fan-out

这是本 skill 明确禁止的默认行为。修正：child 专注当前任务；只有显式委派要求才允许 V2 下钻。

### 把工具可用当成模式启用

V2 child 看得到 `spawn_team_agent` 不代表它应该主动使用。工具权限和主动编排策略是两件事。

### 用 team_report 掩盖没做完

把未完成的工作报成 `ready`。修正：`ready` 只在结果可交付时用；不确定就用 `blocked` 带 question。

### 忽视 WAITING_DECISION

child 报 `blocked` 后父级不回应，agent 一直挂着。修正：每个 blocked 都要有处置——自己决定、升级用户，或 `team_interrupt` 明确终止。

### 为同一目标重复 spawn

已经有一个 agent 在做 X，又起一个做同样的 X。修正：先用 `team_list` 看现有 agent 的职责，再用 `team_followup_task` 复用它的上下文。

## 最终检查

结束前确认：

- [ ] 只派了真正独立、有收益的工作
- [ ] 每个 child 有单一闭环任务和写边界
- [ ] V1/V2 使用了当前真实存在的工具名
- [ ] child 没有因为父级 Proactive/Ultra 自动继续 fan-out
- [ ] 没有在仍有本地工作时无意义等待
- [ ] 关键 child 结论已由父级复核
- [ ] 每个 `ready` / `blocked` / `failed` / `dropped` 都有处置，没有悬空的 `WAITING_DECISION`
- [ ] `team_list` 里没有长期 `RUNNING` + 长 quiet 的 agent 被忽略
- [ ] blocker 和未验证项明确告知用户

**能缩短关键路径或提高质量时，父级主动派；child 专注执行，消息汇合，父级验证并收敛。**
