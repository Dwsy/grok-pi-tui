# 研究报告：后台子代理不显示于 Grok TUI

> 项目：pi-grok-build · 分支 `main` · HEAD `9fe9a4fc`
> 角色：pi-grok-zero · 日期：2026-08-19
> 状态：**诊断完成（静态证据充分），修复待高级模型实施**

---

## 1. 问题现象（用户报告 + 复现）

调用 `spawn_subagent`（扩展已注册的 tool）发起后台子代理，在 Grok TUI 中：

- ✅ 前台（`background=false`）：子代理面板 / SubagentBlock **正常显示**
- ❌ 后台（`background=true`）：**完全没有子代理痕迹**，scrollback 里只有一条工具调用卡片，没有独立 SubagentBlock / 子代理视图 / 进度

用户复选确认：「完全没有子代理痕迹」「只有 tool 卡片」「前台能显示/后台不能」。

---

## 2. 涉及文件与版本

| 层 | 文件 | 关键符号 |
|---|---|---|
| 扩展（发包方） | `extensions/pi-grok-subagents/index.ts` | `BRIDGE_TYPE="pi-grok-subagent/v1"`, `emit()`, `createRecord()`, `scheduleBackground()` |
| Adapter | `crates/codegen/pi-grok-adapter/src/subagent_projection.rs` | `parse_bridge_message` → `BridgeOperation::ParentTaskMetadata` / `ParentLifecycle` |
| Adapter 发送 | `crates/codegen/pi-grok-adapter/src/pi_adapter/notifications.rs` | `ParentTaskMetadata` → `ToolCallUpdate(raw_input)` |
| Pager 监听 | `crates/codegen/xai-grok-pager/src/acp/tracker.rs` | `handle_tool_call`(L1154), `handle_tool_call_update`(L1225), `is_task_tool`(L2497), `is_task_tool_id` |
| Pager 事件 | `crates/codegen/xai-grok-pager/src/app/acp_handler/session_notification.rs` | `SubagentSpawned` handler (L345), `SubagentProgress`, `SubagentFinished` |
| 工具识别 | `crates/codegen/xai-grok-tools/src/implementations/grok_build/task/mod.rs` | `is_task_tool_id`(L186) |
| 测试 | `tracker_tests.rs`, `src/app/acp_handler/tests/subagents.rs` | `background_task_clears_subagent_wait`, `replayed_subagent_finished...` |

---

## 3. 完整调用链（指认的"应该工作"路径）

```
spawn_subagent (ext) background=true
  └─ createRecord()  →  创建 AgentSession(child)、record
       └─ emit("spawned", {background:true, parentToolCallId, ...})
            └─ pi.appendEntry(BRIDGE_TYPE, envelope)   ← 关键：写入 Pi 会话流
                 └─ adapter 订阅消费
                      parse_bridge_message(kind="spawned")
                        ├─ BridgeOperation::ParentTaskMetadata{tool_call_id, raw_input:{variant:"Task", task_id, run_in_background:true}}
                        └─ BridgeOperation::ParentLifecycle(subagent_spawned 通知)
                      notifications.rs 按序 send_update / send_ext_notification
   ──► Pager
        handle_tool_call("spawn_subagent")          → is_task_tool=true → 抑制卡片 + (前台才建 blocking_wait)
        handle_tool_call_update(raw_input)          → task_tool_background[subagent_id]=true  (L1250)
        SubagentSpawned handler                       → task_tool_background.remove(subagent_id)
                                                       → is_background=true
                                                       → SubagentBlock::started(..., is_background)  ← 面板应展示
```

---

## 4. 逐环节验证结果（T=通过，?=可疑，X=硬缺口）

| # | 环节 | 判定 | 证据 |
|---|---|---|---|
| 1 | `spawn_subagent` 被识别为 task tool | ✅ | `is_task_tool_id` 匹配 `"spawn_subagent"`（task/mod.rs:186-191）；`is_task_tool` 用它（tracker.rs:2497-2502） |
| 2 | 工具卡片被抑制（不双显） | ✅ | `handle_tool_call` 对 task 加入 `suppressed_tools`（tracker.rs:1193） |
| 3 | adapter 写入后台标志 | ⚠️ 部分 | 写的是 **`raw_input.run_in_background`**，见 §5 |
| 4 | Pager 读取后台标志 #1 | ✅ | `handle_tool_call_update`→`task_tool_background`（tracker.rs:1250），pinned by `background_task_clears_subagent_wait` 测试 |
| 5 | Pager 读取后台标志 #2 | ❌ **X** | `handle_tool_call`（tracker.rs:1173）读 `tc.meta.subagentBackground` —— **adapter 从不写此字段**（见 §5），恒为 `None` → `is_background != Some(true)` → **误建前台 blocking_wait** |
| 6 | SubagentSpawned 面板 | ⚠️ 依赖 #4 | `SubagentSpawned` 从 `task_tool_background.remove` 取标志（session_notification.rs:421），机制本该工作 |

---

## 5. 根因候选（按可能性排序）

### 候选 A（最可疑）：`meta.subagentBackground` 从未被写入 —— 后台标志双读取源不对称

**硬证据：**
```
$ rg -c "subagentBackground" crates/codegen/xai-grok-pager/src/acp/tracker.rs        → 1 （只读）
$ rg -n "subagentBackground" crates/codegen/pi-grok-adapter/src                        → NONE （从不写）
```

- **读取点**：`tracker.rs:1173` 在 `handle_tool_call` 里判断是否建 `blocking_waits`：
  ```rust
  let is_background = tc.meta.as_ref()
      .and_then(|m| m.get("subagentBackground"))
      .and_then(Value::as_bool);
  if is_background != Some(true) {
      self.blocking_waits.insert(/* 前台阻塞 */);
  }
  ```
- **共有后台任务的本地大写来源**（上游 stock Grok）会在 tool call 的 meta 里打 `subagentBackground`。但 **pi-grok-adapter 走的是私有 bridge**，它只注入 `raw_input.run_in_background`（subagent_projection.rs:118），**从不注入 `meta.subagentBackground`**。
- 后果：后台任务到达 `handle_tool_call` 时 `meta.subagentBackground=None` → 被当作 **前台阻塞** 处理，父轮次持有一个永不消除的 `blocking_wait` 视角。子代理无法作为独立后台面板呈现。

> 说明：这解释了「只有 tool 卡片、无面板」的表象一部分；但严格说面板创建在 `SubagentSpawned`，其 `is_background` 取自 map（候选 C 依赖），与 `handle_tool_call` 的 `blocking_waits` 是两套。仍需确认最终落到哪一步。

### 候选 B：`ParentLifecycle` 通知顺序 / 消息体字段缺失

- `subagent_spawned` 通知 `update` 里**不含 `background` 字段**（subagent_projection.rs:94-110 构造）。上游 stock 通知是否有该字段、Pager `SubagentSpawned` 是否要求它在消息头，需对上游 `xai-org/grok-build` 的 `subagent_spawned` 消费端核对。
- 若上游要求通知体携带后台标志而桥接遗漏，Pager 端可能因缺字段走默认（前台）路径。

### 候选 C：运行时顺序/并发 —— `task_tool_background` 填充晚于 `SubagentSpawned` 消费

- `ParentTaskMetadata`（ToolCallUpdate）与 `ParentLifecycle`（subagent_spawned）在 notifications.rs **同一 for 循环、相同 sequence 依次 await**，顺序应保序。
- 但二者分属不同通道：`ToolCallUpdate` 走 `send_update`（ACP），`subagent_spawned` 走 `send_ext_notification`（x.ai/session/update）。若两条通道到达 Pager 的合并/分派存在乱序，`SubagentSpawned` 先于 map 填充执行，`task_tool_background.remove` 返回 `false` → `is_background=false` → **按前台渲染**，后台面板错位/缺失。
- 实测旁证：后台子代理在多轮测试中反复「0 turns / 0 tools」空转、`get_command_or_subagent_output` 只回状态壳 —— 子代理本体可能实际未执行（桥接在 spawn 后即断）。

### 候选 D（次要）：后台子代理本身未真正执行，事件流在 spawn 后终止

- 实测多轮后台 spawn 均「2s 完成、0 turns、0 tools」；前台同名调用正常。若 `scheduleBackground()`→`run()`→`session.prompt()` 后台路径未真正推进（详见扩展 `scheduleBackground`/`createAgentSession`），则只有 `"spawned"` 事件、无后续 `progress`/`child_update`/`finished`，面板只 spawn 一瞬便成孤儿，不会持续显示。

---

## 6. 修复方向（供实施参考，非唯一解）

1. **候选 A 修复**：在 adapter 的 `ParentTaskMetadata` 之外，或直接构造 tool call 时，为工具调用注入 `meta.subagentBackground = background`（与上游 stock 语义对齐）。这样 `tracker.rs:1173` 读取正确，后台不再误建 blocking_wait。
   - 需谨慎：`meta` 与 `raw_input` 是两个字段；上游在 tool call 构造时写入，adapter 走 `ToolCallUpdate` 增量更新时 `meta` 可能不可篡改 → 需确认 ACP `ToolCallUpdateFields` 是否支持 `meta`，否则改用候选 C 的时序修正或其他既有后台标志通道（如 `task_tool_background` map 后续同步）。

2. **候选 B 修复**：在 `subagent_spawned` 通知体补充 `background` 字段，并核对上游 `SubagentSpawned` 消费端对该字段的期望。

3. **候选 C 修复**：保证 ACP 通道（ToolCallUpdate）先于扩展通知通道（x.ai/session/update）到达，或将后台标志改由 `SubagentSpawned` 通知体直接携带（不依赖 map 时序）。

4. **候选 D 修复**：独立于渲染，先在扩展层定位 `scheduleBackground` 后台执行为何空转（0 turns/0 tools），确保子代理本体真实运行。

---

## 7. 建议的验证顺序（修复后）

1. `./build.sh` → `cargo check -p xai-grok-pager-bin --bin grok-pi`（编译门禁）
2. 现有单测：`tracker_tests.rs::background_task_clears_subagent_wait`、`subagents.rs::*`（确保后台语义回归通过）
3. 补充：一个新测试，模拟 `spawn_subagent background=true` 全链路，断言 `SubagentBlock` 以 `is_background=true` 生成、父轮次无 `blocking_wait`
4. 运行时手测：`spawn_subagent({background:true})` → 确认 TUI 出现后台 SubagentBlock、父轮次不挂起、`get_command_or_subagent_output` 能取回

---

## 8. 未决待核对（需要高级模型在真实环境确认）

- `meta.subagentBackground` 在上游 stock Grok 是否确为 tool call 的标准后台标志；adapter 是否有现成方式写 tool call 的 meta。
- `x.ai/session/update` 的 `subagent_spawned` 上游消费端是否要求 `background` 字段。
- 后台子代理 0-turn 空转的直接原因（扩展 `scheduleBackground` vs `createAgentSession` vs 资源加载器）。
- 是否有 pi-semver 版本差异导致 `tracker.rs:1173` 的 `meta.subagentBackground` 在过去被填充、最近上游改版后 adapter 未跟上。

---

*报告完 · 移交高级模型实施修复*