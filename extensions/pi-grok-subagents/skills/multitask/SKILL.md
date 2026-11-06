---
name: multitask
description: 在 grok-pi Subagents V2 中按任务树启用 Multitask 异步多任务编排。仅在用户显式要求 multitask、非阻塞多代理并行，或委派任务明确携带 Multitask 契约时使用。各层 Agent 可在获分配深度和额度内拆分独立任务，派发后继续工作，依赖未到时结束 turn 并由结果唤醒。要求真实 V2 team 工具；不用于默认 Proactive/Ultra 继承、V1 或用户要求单 Agent 的任务。
---

# Multitask for Subagents V2

## 激活与范围

先确认当前工具面有 `spawn_team_agent`、`team_followup_task`、`team_report`（仅 child）、`team_list`。root 没有 team_report 是正常的。工具缺失时说明需要启用 Pi subagents + V2，不猜造工具或修改用户配置。

用 `/skill:multitask <任务>` 显式选择本次任务树的工作策略。后续无关任务不自动沿用。遵守用户的单 Agent、禁止并行、写入范围和模型约束。

将每个 Agent 当作可做本地工作的协调者；没有独立工作可委派时直接执行。不要为了“每个 Agent 都 multitask”制造下级。

保留资源隔离：child 不自动继承 root 已加载的技能、扩展或历史上下文。每次派发都明确复制下方契约并填入额度，让进一步拆分成为任务的显式授权。

## 先规划可验收闭环

快速列出任务、依赖、owner、写域和验收条件，通常只派 1–3 个有收益的闭环。

- 独立调查、不同模块实现、独立审查可以并行。
- 消费者的依赖尚未就绪就先不创建它，不启动一个只会等待的 synthesizer。
- root 为本次任务树最多分配 8 个新 agent identity；按分支分配互不重叠的剩余额度，不把全局额度重复发给每个 child。
- 默认 root 深度 0，最多两层 child；每个 coordinator 通常最多 2 个直系 child。
- 预算或深度不足时本地执行。深度和额度是本技能的软约定，runtime 目前只硬限全树 4 个后台执行槽。
- 同一目标已有路径时复用 `team_followup_task`，不重复 spawn。跟踪必需任务是否交付，不能把 idle 等同于完成目标。
- 使用定义/用户允许的模型和能力；不擅自降低模型或提高权限。

## 非阻塞循环：每层都相同

1. 用 `spawn_team_agent` 启动就绪任务。它已是异步工具，无须虚构 `background` 参数。
2. 默认用单个 `spawn_team_agent` 派发独立任务。只有核对过确实存在且成员任务/写域适合本次工作的 preset，才用 root 专有的 `spawn_team`，传真实 team 名称，省略 `timeout_ms` 或设 0，并通过 instructions 传契约。不要为了一个普通任务虚构 preset。
3. 派发后立刻做自己可推进的工作，不读取结果做忙轮询，不调用 `team_wait` 占槽等依赖。
4. 到真正依赖点且无本地工作时结束当前 turn：
   - root：简短说明仍有哪项后台任务，结束本轮。等待 FINAL_ANSWER 自动触发后续 turn，不宣称整个任务已完成。
   - child：有自己的 running/queued/WAITING_RESULT child 时，先 `team_report(state="waiting", summary="等待哪些下级及交付")`，然后结束本轮。runtime 释放本轮槽，结果会恢复同一个 child session。
5. 收到结果后核对 sender、任务、写域和证据，更新依赖，再做本地工作或派发下一项。
6. 必需依赖全部验收后才报告交付；root 最终综合，不把 child 的一句话直接当作用户结论。

`team_report` 只记录声明，**调用后仍须结束 turn**。不要调用 report 后继续 wait。waiting 不是一个新的线程，也不承诺跨进程恢复。

## 每次派发必须复制的契约

把下列文字作为任务指令填入 message；不要仅写“沿用我的模式”。传给下一级时减少 depth_left，分配独占 spawn_budget；本地保留一份剩余额度账。

```text
Multitask 契约（本任务显式授权）：
- 本任务目标：[唯一交付物]；仓库：[cwd]；parent：[绝对 agent path]。
- 已知与输入：[关键事实/路径/依赖结果]。
- 写域：[允许修改的文件；只读任务明确禁止写入]。
- 验收：[命令或证据标准]。
- depth_left=[剩余可创建层数]；spawn_budget=[本分支可再创建的 agent identity 总数]。
- 仅在有独立闭环、有并行收益且额度足够时创建自己的 child；不得扩展任务范围。
- 复制本契约给下级，depth_left 减 1，分配不重复的剩余 spawn_budget。
- spawn_team_agent 已异步；派发后继续本地工作。不要 team_wait、轮询或 shell sleep 等下级。
- 自己的下级仍 running/queued 且没有本地工作时，team_report waiting 后结束 turn；结果会恢复本会话。waiting 不代表完成。
- 完成并验收所有必需交付后 team_report ready；需父级决定时 blocked + question；失败时 failed；确实无需父级行动才 dropped。
- report summary 必须带结论和可核查证据路径/关键验证结果；它会替换父级收到的最终文本。
- 与其他 Agent 共用 cwd；单一 writer，不覆盖他人修改；不得顺手全局格式化、切分支或安装依赖。
```

root 的第一个 child 默认 depth_left=1；其下一级 depth_left=0。派发本身消耗 1 个 identity，交给 child 的 spawn_budget 只计算它未来还能创建的额度。

父任务已显式禁止继续拆分时，将 depth_left=0、spawn_budget=0；没有额度仍可复用已有 agent，但不能借 follow-up 扩大原任务树。

## 汇报、消息与恢复

| 情况 | 动作 |
|---|---|
| 完整交付且证据已核对 | child `team_report ready` 后结束 turn |
| 本地无工作，自己的下级仍 active | child `team_report waiting` 后结束 turn；上级不被中间结果唤醒 |
| 需要父级做决定 | `blocked`，question 写清一个具体决定；parent 用 follow-up 回答 |
| 执行失败 | `failed`；parent 判断修任务书、接手或有界重试 |
| 已被满足/接管，确实没有待办 | `dropped`；不要用它表示“仍在等结果” |
| 对方正在运行，需要新事实 | 合并成一条 `team_send_message` |
| 对方 idle/WAITING_RESULT，需要执行新工作 | `team_followup_task`，保留原会话身份 |
| 需要检查责任/异常状态 | 适时 `team_list`，不用作轮询 |

waiting 仅适用于自己的 active child。依赖已完成就处理已有结果；依赖失败/取消就处置失败；等待别人做决定用 blocked。不要靠创建空 child 通过 waiting 校验。

父级要求的调查即使结论是“无需改动”，其证据仍是交付物，应报告 ready。只有父级已经知道该工作不再需要交付时才用 dropped，避免让等待最终答复的 root 收不到结果。已声明 waiting 的中间 parent 仍会收到下级 dropped 作为依赖完成事件。

旧版本若拒绝 waiting，不伪报 ready/dropped：简短报告这一能力缺口并结束当前 turn，允许现有 FINAL_ANSWER 回传中间状态；说明仍未完成。不要回退到长等待。需要继续拆分前让 parent 决定兼容策略。

## 写入与取消

所有 V2 agent 共享 cwd。按文件或模块指定唯一 writer；两个任务都要改热点文件时串行执行，让另一个只读审查。当前 spawn 不支持 workdir，不虚构 worktree 隔离。

取消时查看任务树，逐个处理本任务的 running/queued 后代；现有 team_interrupt 不会级联。不要把长期静默自动判成卡死：quiet 只是 team 通信间隔。遇到 parent 失败或不可达，保留证据并向仍可联系的上级说明，不无限重派。

## 结束前核对

- 每个 child 都有自包含任务、明确写域、验收条件和可追踪额度。
- 没有 Agent 占执行槽等自己的排队后代；waiting report 后已经结束 turn。
- 没有把 waiting/idle 当成目标完成，没有悬空 blocked。
- 根任务所有必需交付已验收，关键验证有真实输出。
- 未实现的硬预算、DAG/写锁、取消级联、跨进程恢复或忙碌输入队列转换没有被宣称为已支持。
