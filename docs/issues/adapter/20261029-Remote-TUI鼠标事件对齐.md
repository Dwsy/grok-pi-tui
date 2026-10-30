# Remote TUI 鼠标事件与 Pi 全屏 TUI 对齐

## 问题与范围

当前 Remote TUI 的输入所有权逻辑只转发键盘和粘贴。`AppView::handle_input_at_with_paste_provenance` 在 Remote TUI 会话期间吞掉 `Event::Mouse`，没有把鼠标事件交给 Pi 组件。因此 Pi 全屏 TUI 中依赖 `Component.handleMouse()` 的选择器、列表、输入框、鼠标区域和可交互容器，在 Remote TUI 中无法获得等价交互。

Pi `@earendil-works/pi-tui` 的全屏实现提供归一化事件：`press`、`release`、`move`、`drag`、`click`、`wheel`；事件携带零基坐标、组件局部坐标与屏幕坐标、按钮、Shift/Alt/Ctrl、滚轮逻辑行数和连续点击次数。对齐必须复用该公开组件契约，不修改 Pi 源码，也不引入第二套 TUI。

## 目标

- 将 Pager 捕获的鼠标事件安全、按序地送到当前 Remote TUI 组件。
- 使用当前实际渲染区域计算局部坐标；支持 inline frame 和 `ctx.ui.custom(..., { overlay: true })` overlay。
- 在 Pi 侧调用真实组件的 `handleMouse()`，遵循 focus/capture 结果，并在按下/释放且未拖动时合成 click，保留双击/连续点击语义。
- 支持左/中/右键、按下/释放、移动/拖动、垂直滚轮和 Shift/Alt/Ctrl 修饰键；事件不在组件区域且没有捕获指针时，应继续由 Pager 原有鼠标逻辑处理。
- 不改变既有键盘、粘贴、组件焦点、Escape 取消、会话 ID 和过期输入保护。
- 鼠标移动可能高频产生事件；传输只读取新增 JSONL 字节，并在同一批连续 hover move 中合并到最后一个位置，避免拖动期间重复扫描整个会话文件。

## 验收标准

1. Remote TUI 的 inline 与 overlay 两种布局都使用当前渲染矩形进行命中测试和坐标转换；坐标为零基，`screenX/screenY` 保留终端坐标，`x/y` 相对组件原点，`width/height` 对应组件边界。
2. Pi 组件可收到 `press`、`release`、`move`、`drag`、`click`、`wheel`；按钮、修饰键、滚轮方向/逻辑行数及点击次数不会被丢失或错误映射。
3. 组件返回的 mouse capture/focus/render 语义得到遵守；拖动期间及释放事件发送给捕获的组件；普通点击与双击可以触发组件原生行为。
4. 组件区域之外的鼠标输入不会无条件吞掉，且不意外编辑隐藏的 composer；组件关闭、重载或 ID 变化后，不会把迟到事件发给新组件。
5. 增加确定性回归测试覆盖事件转换、坐标、按钮/修饰键、滚轮、点击/双击、拖动捕获、区域外输入和 stale session。
6. 更新 `docs/FEATURE_MATRIX.md` 与本 issue 的实现/验证记录。执行相关 TypeScript 测试和最窄的 Rust 测试/check，并如实记录任何既有构建阻塞。

## 实施顺序

1. 确定 Pager 对 inline/overlay 的真实绘制矩形，并定义统一的鼠标事件 wire shape。
2. 在 Pager 输入优先级中只转发命中 Remote TUI 或被组件捕获的鼠标事件；扩展 Remote TUI transport 与 Pi 组件 host。
3. 按 Pi TUI 的 `handleMouse()` 结果处理捕获、焦点、点击和渲染；保持默认 keyfile transport 与可选 RPC bridge 的既有边界。
4. 增加回归测试、运行验证、审查 diff，最后更新 feature matrix。

## 非目标与边界

- 不修改 `pi-main` 的 Pi TUI 实现；Pi 是鼠标事件语义的参照，不是要 fork 的代码。
- 不让 headless `pi-grok-adapter` 负责绘制或直接读取终端输入。
- 不在组件矩形之外抢占 Pager 原有鼠标行为；Pi TUI 未提供的水平滚轮等行为不伪造为垂直滚动。
- 若本地 PTY 无法覆盖真实终端的鼠标协议差异，应把它作为单独的手工验收边界明确记录。

## 实现记录

- `AppView::remote_tui_mouse_event` 将终端事件映射为 Pi 的归一化事件；根据最后一次绘制的 inline/overlay 矩形做命中测试，保留屏幕坐标、局部坐标、按钮和修饰键。区域外事件继续走 Pager 原有输入路径；已有手势的拖动、滚轮和释放可越出矩形继续转发，命中框暂缺时的按下/释放会清理旧手势。
- Remote TUI 通过新增 `RemoteTuiMouse` ACP effect 和 `pi/ui/remote_tui/mouse` 通知传递事件；adapter 将事件按会话 ID 写入现有 JSONL transport。
- `extensions/pi-grok-remote-tui/host.ts` 调用真实组件的 `handleMouse()`，处理 focus/capture、按下—释放后的 click/连续点击计数，以及 Pi 的滚轮加速与 Alt-wheel 行数。
- JSONL transport 只读取新增字节，并将同一批连续 hover move 合并为最后一个点；拖动、按下和释放事件仍按序处理。
- inline hit box 使用 widget 列表中的精确行偏移，不再依赖渲染文本匹配，因此重复文本不会把鼠标命中区域定位到其他 widget。
- `docs/FEATURE_MATRIX.md` 已更新 Remote TUI 鼠标行为与验证边界。

## 验证记录（2026-10-29）

- `bun test extensions/pi-grok-remote-tui/index.test.ts extensions/pi-grok-remote-tui/transport.test.ts`：**14 passed, 0 failed**（59 assertions）。
- `./scripts/cargo-shared.sh test -p pi-grok-adapter`：**197 passed, 0 failed**。
- 对本次涉及的 Rust 文件执行 `rustfmt --edition 2024 --config skip_children=true --check ...`，并执行 `git diff --check`：退出码 0。
- `pi-main/node_modules/.bin/tsgo --noEmit -p extensions/pi-grok-remote-tui/tsconfig.json --module ESNext --moduleResolution Bundler`：整体检查因 `pi-main` 现有 SDK/API 类型不一致和缺失依赖而失败；按输出过滤后，本次扩展文件没有 TypeScript 错误。
- `./scripts/cargo-shared.sh check -p xai-grok-pager --lib`：**退出码 0**。初次检查发现 `app/mod.rs:888` 的 E0505（`terminal` 仍被 pinned 的 `loop_run` future 借用）；将 loop future 收进独立作用域后，生产库检查通过。剩余 1 条既有的 `non_shorthand_field_patterns` 风格警告不影响构建。
- `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi`：**退出码 0**；Pager、adapter 和最终二进制组合路径均通过生产代码检查。
- `./scripts/cargo-shared.sh test -p xai-grok-pager --lib remote_tui`：退出码 101，定向测试尚未运行，因为 Rust 在编译整个 `lib test` harness 时遇到 **49 个其他模块的编译错误**。已观察到的问题包括 subagent 字段/API、session-picker 结构体字段、settings/review 测试 API 和 scrollback 测试引用过期；还存在一条既有重复 `#[test]` 警告。当前输出未报告本次新增 Remote TUI 测试中的类型错误。
- 真实终端/PTY 的手工鼠标协议验收尚未完成；当前结果仅代表确定性测试和代码检查。

## 后续验证（2026-10-30）

- `./scripts/cargo-shared.sh test -p xai-grok-pager --lib remote_tui`：**23 passed, 0 failed**。Pager lib-test harness 现已成功编译并实际运行 Remote TUI 定向用例；此前记录的 49 个测试编译错误已通过最小 API/fixture 对齐处理。
- `remote_tui_mouse_outside_frame_falls_through_and_closed_session_drops_drag` 单独复测：**1 passed**。测试现在比较关闭前后的 `RemoteTuiMouse` effect 数量，验证关闭后的 release 不会再入队，同时保留关闭前合法 press 的断言；本次续接未修改生产鼠标路由。
- `./scripts/cargo-shared.sh check -p xai-grok-pager --lib`：**退出码 0**。仅有既存的 `non_shorthand_field_patterns` 风格警告。
- 本次涉及的 18 个 Rust 文件执行 `rustfmt --edition 2024 --config skip_children=true --check ...`，并执行 `git diff --check`：**退出码 0**。
- 包级 `cargo check -p xai-grok-pager --tests` 的广范围检查仍不作为本次通过依据：此前执行超时，并暴露独立集成测试 `registered_features_are_documented.rs` 引用缺失文档 `25-enterprise.md`、`22-environment-variables.md`。本次验证范围明确为 Pager `--lib` test harness 与 Remote TUI 定向用例。

