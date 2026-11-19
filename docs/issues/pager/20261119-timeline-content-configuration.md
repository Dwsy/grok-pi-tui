---
id: "2026-11-19-timeline-content-configuration"
title: "Timeline 内容可配置"
status: "in-progress"
created: "2026-11-19"
updated: "2026-11-19"
category: "Pager"
tags: ["pager", "timeline", "pi-extension", "remote-tui"]
---

# Timeline 内容可配置

## 目标

Timeline 默认显示用户输入和上下文压缩。用户可以在 Pi 扩展提供的 TUI 面板里勾选内置工具项，或手动输入任意 Pi 工具注册名。保存后，已加载历史和实时调用同时应用新规则。

Rust Pager 提供时间线候选条目、过滤、导航锚点和终端绘制。TS 扩展负责配置交互和工具展示，可以在扩展内为工具或回合摘要新增格式化器，返回 ANSI 交由 Pager 绘制。

## 配置交互

`/timeline` 控制侧栏显隐。新增 `/timeline-config` 打开内容配置面板。全屏侧栏使用现有宽度和最少刻度限制，条目不足时使用普通滚动条。

| 项目 | 默认 | 操作 |
| --- | --- | --- |
| 用户输入 | 开 | 点击整行切换 |
| 上下文压缩 | 开 | 点击整行切换 |
| `edit` / `write` | 关 | 启用后将同轮文件改动计入用户输入的 hover 卡片 |
| 自定义工具 | 空 | 点击「添加工具」并输入注册名，支持点击删除 |

输入框用 `pi.getAllTools()` 提供候选，并接受尚未加载或只出现在历史中的注册名。名称去掉首尾空白后按原名精确匹配。同名只保留一项，手输 `edit` 等价于启用内置的 `edit` 行。

面板用 `ctx.ui.custom` 和 Pi TUI 的 `SettingsList`、输入组件。Remote TUI 用最后一次绘制的矩形做命中测试，将组件局部坐标、点击和滚轮转给 Pi。点击整行触发一次切换，输入框支持点选聚焦和候选确认，滚轮滚动命中的面板区域。所有操作都可用键盘完成，`Esc` 放弃草稿。激活刻度时，Pager 直接把视口移到对应条目。

[Remote TUI 鼠标事件契约](../adapter/20261029-Remote-TUI鼠标事件对齐.md) · [Pi 设置列表](../../../pi-main/packages/tui/src/components/settings-list.ts)

## 配置和导航

配置存入 `$GROK_HOME/config.toml` 的用户级 `[ui.timeline_contents]`。Rust 负责读取、校验和持久化，TS 面板维护草稿。点击保存后，Rust 写入并应用整组规则，再返回结果。失败时保留已生效的规则和面板草稿，方便重试。缺省字段回落到默认值，`tools = []` 表示停用所有工具刻度。

```toml
[ui.timeline_contents]
user_prompts = true
compactions = true
tools = ["edit", "write", "mcp__repo__search"]
```

Pager 从当前分支的 scrollback 生成刻度。实时投影和历史回放在工具条目上保留 Pi 原始注册名和调用 ID，并映射到 Pager 的 `EntryId`。开始和结束更新同一个调用刻度，合并的工具卡片提供实际跳转锚点。每帧的绘制、hover 和上下箭头共享同一份命中映射。点击先校验映射归属，再按 `EntryId` 跳转。切分支时替换条目集合，单轮视图的跳转定位到目标条目所属轮次。`/jump` 和会话 review 由 turn 列表提供。

[刻度与 turn 列表](../../../crates/codegen/xai-grok-pager/src/scrollback/state/timeline.rs) · [实时工具投影](../../../crates/codegen/pi-grok-adapter/src/pi_adapter/tools.rs) · [历史工具投影](../../../crates/codegen/pi-grok-adapter/src/pi_adapter/replay.rs)

## TS 格式化与 Rust 绘制

扩展维护按工具注册名查询的格式化器，子代理和 Team 操作各有显示样式。扩展可用同一个接口注册其他工具。无匹配格式化器时，Pager 显示工具名和状态。

```ts
type TimelineRenderer = (entry: {
  toolName: string
  status: "running" | "success" | "failed" | "cancelled"
  path?: string
}, view: {
  tickColumns: number
  previewColumns: number
}) => {
  tickAnsi?: string
  previewAnsi?: readonly string[]
}
```

用户输入的格式化器可为最终回复开头及文件变更行添加样式。Pager 决定时间戳、选中样式、边框、点击热区和可用列宽。ANSI 只接受 SGR 样式，转换成 Ratatui 文本后按终端显示宽度裁剪。每行样式独立，非法内容或超限响应回落到原生预览。

Timeline 消息通过现有扩展桥传递，使用独立的内容配置和格式化消息；Remote TUI 通道承载配置面板的交互。Adapter 只转发消息，Pager 持有条目、配置和绘制。配置保存携带请求 ID，返回保存结果和实际生效的规则。格式化请求带会话、分支代次、条目版本、可用宽度和主题版本。只为可见刻度异步批量请求，等待期间直接绘制原生刻度和预览。缓存按条目版本和渲染尺寸等输入复用，切会话、切分支或重载后丢弃迟到响应。

[原生刻度布局](../../../crates/codegen/xai-grok-pager/src/views/timeline.rs) · [现有 ANSI 文本转换](../../../crates/codegen/xai-grok-pager/src/views/agent_status.rs#L81) · [原生 TUI 分层约定](../../NATIVE_GROK_TUI_ALIGNMENT.md)

## 内置刻度与预览扩展

- 保留 `edit` 和 `write` 独立开关，加入「子代理交互」和「Team 子代理交互」两组。每组保存对应的实际 Pi 工具注册名，手工配置的部分成员单独显示为可移除项。
- 用户输入刻度的 hover 保留输入标题，并显示同轮最终可见回复的开头。Rust 根据 scrollback 的轮次和工具边界提取文本，TS 负责格式化输入、回复和压缩摘要。还没有最终回复时，预览只显示输入。
- 子代理和 Team 刻度显示操作、目标与调用状态。预览传输只带必要的目标名、路径及状态字段，消息正文和逐条工具输出不进入格式化请求。

## 用户回合里的文件改动摘要

启用 `edit` 或 `write` 后，同轮完成的文件改动汇总到用户输入的 hover 卡片。独立文件刻度退役。卡片最多显示一行用户输入、两行 Agent 最终回复、一个汇总行和两个文件行。编辑使用原生 diff 的增删行数，写入统计写出的行数。多个调用按文件合并，标题保留总文件数，失败或取消的调用不计入。

## 切树与重绘的增量更新

- Pi 扩展调用官方 `ctx.navigateTree`，切换完成后直接返回实际叶节点和编辑器文本。已缓存该叶节点时，Adapter 跳过切换后的 `get_entries` 请求。新路径只向上走到缓存的最近共同祖先。摘要、新节点或旧宿主使用现有读取流程。带状态的历史继续走完整 ACP 回放，普通文本分支保留完整的公共用户轮次。
- Pager 缓存过滤后的刻度。普通重绘复用快照，结构或工具条目变化时只更新最早受影响的条目之后的部分。文本分支重放保留的前缀也保留刻度和已完成的格式化结果，过期分支回包按代次丢弃。
- 用户输入的最终回复只在 hover 时读取。工具和压缩继续按可见窗口请求格式化。

## 验收

- [ ] 缺省配置下，开启侧栏只显示用户输入和压缩。通过鼠标启用 `edit` 并输入一个自定义工具名。保存后，在用户回合 hover 中看到已完成的编辑摘要，在侧栏看到自定义工具刻度。新调用按同一规则显示，重启后配置可恢复。
- [ ] 同一个调用从运行中转为成功或失败时，刻度数量保持一致。切分支后只保留新分支的匹配条目。过滤、滚动、缩放后，hover、点击和上下箭头定位同一帧对应的条目。
- [ ] 在 TS 内新增一个格式化器后，对应工具的刻度或预览样式随之变化。格式化超时、非法 ANSI 或旧分支迟到响应时，Pager 仍能提供可点击的原生刻度和预览。
- [ ] 在支持鼠标上报的真实终端完整走通「打开面板 → 点击启用 → 点输入框 → 添加工具 → 保存 → 点击刻度」。检查滚轮范围、一次点击只触发一次、`Esc` 放弃草稿和纯键盘流程。
- [ ] 配置写入失败时面板保留草稿并显示失败原因。恢复默认后当前会话回到用户输入与压缩两类刻度。

## 实施记录

- Pager 从用户级 TOML 加载内容规则，通过现有配置锁和原子替换保存。写入成功后立即应用整组规则，失败时回传错误并保留草稿。
- 实时与回放使用 Pi 注册名及调用 ID。合并卡片保留原调用的刻度，共用现存卡片锚点。原生隐藏的状态工具和 Eval 调用使用邻近的可用 scrollback 锚点。Codemode 内的调用使用所属卡片的锚点。
- `/timeline-config` 由默认启用的 Remote TUI 扩展注册。配置和格式化使用独立消息，配置面板复用 Pi 组件及 Remote TUI 的输入转发。
- Pager 以同一帧的几何和条目快照绘制、预览和跳转。可见工具按批请求格式化，按会话、分支、主题、工具内容和尺寸校验及缓存；超时或非法响应使用原生内容。

## 验证记录

- Remote TUI Bun 测试 14 项通过。扩展使用已安装的 Pi 1.5.2 SDK 做定向类型检查，结果通过。使用本地 Pi 源码目录进行的完整类型检查仍受既有依赖和类型错误阻塞。
- 用真实 Pi 1.5.2 RPC 进程和模拟的 Pager 回包验证了命令发现、格式化批次、鼠标点选、输入框焦点、滚轮、键盘保存、Esc 放弃、保存失败保留草稿及恢复默认。
- 独立 rustfmt 可以解析改动的 Rust 文件，`git diff --check` 通过。架构校验已登记 Timeline 接缝，完整校验仍有既有的基线及协议断言失败。
- 本次未执行 Cargo。Rust 构建、Rust 回归用例和真实终端端到端验收待完成。


## 内置功能与增量更新验证

- Pi 1.5.2 SDK 定向类型检查和 Remote TUI 的 18 项测试通过，覆盖同轮回复和文件变更摘要、限行、无回复及压缩预览、两组工具格式化和终端列宽。
- 隔离 Pi RPC 测试覆盖两组开关、部分选中转整组选中和保存。树桥接测试覆盖已知兄弟叶、用户节点定位到父节点及回到空根。
- 已添加 Rust 回归用例，覆盖同轮回复边界、刻度快照复用、保留完整公共轮次和已知叶节点复用。Cargo 与真实终端验收待运行。
