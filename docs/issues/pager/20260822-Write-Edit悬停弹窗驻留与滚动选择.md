---
id: "2026-08-22-Write-Edit 悬停弹窗驻留与滚动选择"
title: "Write/Edit 悬停弹窗：鼠标驻留不消失 + 弹窗内滚动"
status: "done"
created: "2026-08-22"
updated: "2026-08-22"
category: "pager"
tags: ["pager", "mouse", "hover-popup", "ux"]
---

# Issue: Write/Edit 悬停弹窗：鼠标驻留不消失 + 弹窗内滚动

## Goal

`[ui].write_edit_hover_popups` 弹窗（collapsed `write`/`edit` 工具行悬停 300ms 后展开的
"Write/Edit details" 浮层）当前一碰就消失、滚轮穿透。目标：

1. 鼠标从工具行移到弹窗本体上时，弹窗**保持打开**（不重置 dwell、不清滚动位）。
2. 弹窗本体上滚轮**只滚动弹窗内容**，带边界钳制（到底/到顶后不再动）。
3. 弹窗本体上点击**不穿透**到下层行（不会误触发展开/查看器/拖拽）。

> 范围简化：原计划的弹窗内拖拽划选复制（Phase 5）按用户要求裁剪，本期不做；
> 需要完整选择能力时可展开行（点击/Ctrl+O）。

## 背景/问题

弹窗可见性是每帧从 `hovered_entry` 推导的，没有任何"关闭动作"；以下条件任一失效即消失：

| # | 症状 | 根因 |
|---|---|---|
| 1 | 鼠标移上弹窗即消失 | `Moved` 分支按光标下的 hit-test 重算 `hovered_entry`；弹窗覆盖在其他内容之上 → hover 变化 → 重置 popup 状态。`src/app/mouse.rs:983-1050`（赋值 `:1045-1050`） |
| 2 | 渲染侧同样只认行矩形 | 渲染函数要求鼠标仍在**条目行矩形内**才绘制，弹窗本体在行外 → 不画。`src/views/agent.rs:744-750` |
| 3 | 滚轮穿透滚动 scrollback | `handle_write_edit_hover_popup_scroll` 的拦截区域只有条目行矩形（`:560`），弹窗本体上的滚轮落到通用分支滚动列表 → 布局在静止鼠标下移动 → 弹窗被顶走。`src/app/agent_view/panes.rs:527-578,787-800`；且偏移量无内容长度钳制（上限仅 u16::MAX，`:567-576`） |
| 4 | 点击穿透 | 弹窗上的 Down 落入下层行的 click 级联（切换展开/打开 viewer/武装拖拽）。`src/app/mouse.rs:38+` |
| 5 | 无划选 | 内容直接画进 buffer（`agent.rs:838-844`），无文本模型、无高亮、无命中映射 |

关键事实：渲染调用点有 300ms dwell 门控 `write_edit_hover_popup_ready()`
（`panes.rs:495-500`），慢速 tick 补帧（`panes.rs:502-525` → `app_view.rs:6918-6925`）。
弹窗几何完全由渲染函数内部计算后丢弃——状态里没有 rect 可供输入侧命中。

## 方案设计

### Phase 1: 渲染导出弹窗帧（rect + 内容元数据）

仿照 `line_viewer.last_popup_area` 先例（`views/file_search/line_viewer.rs:696`，
写入 `:1671`，消费 `app/agent_view/viewer.rs:404`）：

- 新增结构 `WriteEditPopupFrame { area: Rect, inner: Rect, total_lines: usize }`
  （`area`=含边框外框，用于命中；`inner`=正文区，用于划选映射；`total_lines`=展开全文行数，用于钳制）。
- `AgentView` 新增字段 `write_edit_hover_popup_frame: Option<WriteEditPopupFrame>`
  （`mod.rs:1031` 附近，`session.rs:162-165` 初始化 None）。
- `render_write_edit_hover_popup` 返回 `Option<WriteEditPopupFrame>`；
  调用点每帧无条件赋值返回值（未绘制 = None，自动清除陈旧 rect）。`render.rs:2191-2201`。

### Phase 2: 驻留保持（hover persistence）

`mouse.rs` Moved 分支：

```
in_popup = frame.area.contains(pos)
      && hovered_entry 仍解析为 collapsed Edit 条目   // 防陈旧 rect
if in_popup && self.hovered_entry.is_some() {
    new_hover 保持不变;  // 不重算、不触发 hover_changed、不动 dwell/scroll
}
```

- 鼠标离开弹窗回到原行：hit-test 得同 idx，无变化，dwell 不重启 → 继续显示。
- 鼠标离开弹窗到别的行：正常 tooltip 切换语义。

### Phase 3: 点击吸收

Down(Left) 命中 `frame.area` 时短路 click 级联（返回 Changed），不再触达下层行；
该位置同时作为 Phase 5 的划选锚点。Up 无需特殊处理（Down 未武装任何 pending/drag）。
注意 `left_mouse_down` 标记与既有双击检测的一致性。

### Phase 4: 滚轮路由与钳制

扩展 `handle_write_edit_hover_popup_scroll`（`panes.rs:527-578`）：

- 命中区域从"条目行矩形"扩为 `entry_area ∪ frame.area`。
- 用 `frame.total_lines − body_budget`（body 高度取 `frame.inner.height`）钳制
  `write_edit_hover_popup_scroll`，替换现有 u16::MAX 饱和增长 → 到底后滚轮静默吸收。
- 保留 ready 门控与 config 开关判断。

### Phase 5（已裁剪）: 弹窗内划选与复制

按用户要求不做。若后续要做，复用 block_viewer 模式
（`views/block_viewer.rs:229` TextDrag、`:234` drag_copy_text、`:1471-1542`
拖拽处理、`:1708` overlay；消费端 drain + 复制 `app/agent_view/viewer.rs:1107-1118`），
Phase 1 的 frame 已缓存 `inner` 与 `total_lines`，可直接扩展行文本缓存。

### 边界情况

- **陈旧 rect**：输入发生在两次渲染之间，rect 可能滞后一帧；Phase 2 的
  "hovered_entry 仍为 collapsed Edit"校验兜底，最坏一次闪烁。
- **流式输出推动布局**：鼠标静止但内容滚动时行为同现状（tooltip 跟随/消失），
  本期不改。
- **上下翻转放置**：弹窗放不下时翻到行上方，union 命中不受影响。
- **config 关闭 / modal / 子代理面板**：既有前置拦截顺序不变。

## 验收标准 (Acceptance Criteria)

- [x] 鼠标从 collapsed write/edit 行移入弹窗本体：弹窗保持打开、无闪烁、dwell 与滚动位不复位。
- [x] 弹窗本体滚轮只滚动弹窗内容；到顶/到底后继续滚动不再移动 scrollback。
- [x] 弹窗本体内左键点击不触发展开/Ctrl+O 语义/打开 viewer/启动 scrollback 拖拽。
- [x] 单测锁定：frame 导出、驻留保持（含陈旧 rect 防护）、wheel union+钳制、点击吸收。
- [x] `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi` 与相关 lib 测试通过。

## 关键决策

| 决策 | 理由 |
|---|---|
| 渲染导出 rect 到状态，而非输入侧重算几何 | 几何逻辑（翻转、clamp、宽度预算）单点存在于渲染函数；输入侧重算必然漂移。先例：line_viewer/review/dashboard 均如此 |
| 驻留判定加"条目仍有效"校验而非信任 rect | rect 天然滞后一帧，校验消除大部分陈旧风险且零成本 |
| 指针在弹窗上时冻结 popup_x | 否则 `mouse_col-4` 锚点会让弹窗随指针水平滑动 |
| 滚动钳制放进 handler 而非渲染 clamp | 渲染 clamp 是静默纠偏；handler 钳制让"到底后滚轮穿透 scrollback"成为不可能，交互可预期 |
| 划选复制裁剪出期 | 用户明确要求从简；需要选择能力时展开行是既有路径 |

## 实施阶段

- [x] Phase 1: frame 结构 + 字段 + 渲染返回值接线（含陈旧清除）
- [x] Phase 2: Moved 驻留保持 + 校验 + 渲染 keep-alive 区域
- [x] Phase 3: Down/Drag 吸收
- [x] Phase 4: wheel union + total_lines 钳制
- [ ] ~~Phase 5: 划选高亮 + 复制~~（裁剪，见上文）
- [x] Phase 6: 单测补齐 + check/test 验证

## Result

`render_write_edit_hover_popup` 新增 `WriteEditPopupFrame { area, inner, total_lines }`
返回值，调用点每帧赋值到 `AgentView::write_edit_hover_popup_frame`（未绘制即清除）。
鼠标侧三处接入：

- `Moved`：指针在已绘制弹窗内且 hover 目标仍为 collapsed Edit 行时保持 hover 不变；
- 渲染函数接收上一帧 rect 作为 keep-alive 区域，指针在弹窗上时冻结 x 锚点继续绘制；
- `Down(Left)` / 未武装手势的 `Drag(Left)` 在弹窗内被吸收，不触达下层行。

滚轮拦截区从"条目行矩形"扩为 行∪弹窗，并用 `total_lines - inner.height` 钳制偏移。

## Verification

- `./scripts/cargo-shared.sh test -p xai-grok-pager --lib write_edit_hover`：
  PASS，5 tests（新增 4 条交互测试 + 既有 dwell 测试）。
- `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi`：PASS。
- 全量 `--lib`：8753+ passed；71 failed 与干净 HEAD 基线**逐条一致**
  （`comm -13` 无新增），均为 VERIFICATION.md 记录的既有 blocker。
- `cargo fmt -p xai-grok-pager -- --check`：本次改动文件无差异。
- `git diff --check`：无空白错误。

### 附带修复（并行任务遗留）

工作区存在另一并行任务的半成品迁移（`SetPi*` → `Action::SetHostFeatureBool`），
生产代码已迁完但 `dispatch/tests/settings.rs` 漏改 5 处引用，导致整个 lib 测试目标
无法编译。按其新变体机械补齐（PI_HERDR / PI_SUBAGENTS / PI_TODO ×2），使测试可编译。
该文件其余逻辑未动。

## Notes

- 上游同类浮层（hook badge popup `views/agent.rs:568-694`、timeline tick card
  `views/timeline.rs:136-220`）存在同样的"一碰就没"问题；本 Issue 只改
  Write/Edit 弹窗，若模式验证顺利可另立 Issue 平移。
- 改动集中在 `xai-grok-pager` crate 内（views/agent.rs、app/mouse.rs、
  app/agent_view/{mod,session,panes,render}.rs），不触及 Pi RPC / adapter 接缝。
