# Handoff — 修复 remote-tui 帧投影泄漏 Pi fake-cursor APC 标记

生成时间: 2026-10-30
仓库: `pi-grok-build`（grok-pi，`origin/main` 集成分支）
下一 session 目标: 查漏补缺并落地修复（代码 + 测试 + 文档 + 端到端验收）

---

## 0. 一句话结论（已验证，无需重查）

grok-pi 的 remote-tui 帧投影只剥离了 `CURSOR_MARKER`，没有处理 Pi 新增的 **fake cursor APC 标记**
（`ESC _ pi:fc BEL` / `ESC _ pi:/fc BEL`）。标记透传到 Pager 后，`ansi-to-tui` 只吃掉 ESC，
剩下的可打印载荷 `_pi:fc` / `_pi:/fc` 被当正文渲染 —— 于是 `ctx.ui.custom()` 组件里每个
`Input` / `Editor` 的光标格都显示成 `_pi:fc … _pi:/fc`（用户截图红箭头处：
`> _pi:fc _pi:/fc`，即"Search model"搜索框的空光标位置）。

用户原始诉求 = 确认「远程 TUI 是否正确渲染」。答案：**渲染链本身正常，这是光标标记泄漏，问题仍在**。

---

## 1. 已核实的证据链（不要重复调查）

| 环节 | 位置 | 事实 |
|---|---|---|
| 标记定义 | pi-tui `src/tui.ts:196-200`（已安装 1.4.0 的 `dist/tui.js:69`；子模块 `pi-main/packages/tui/src/tui.ts:196`） | `CURSOR_MARKER=ESC_pi:c BEL`、`FAKE_CURSOR_START=ESC_pi:fc BEL`、`FAKE_CURSOR_END=ESC_pi:/fc BEL` |
| 谁发标记 | `pi-main/packages/tui/src/components/input.ts:433,491`（`editor.ts:591,596` 同族） | `Input` 的光标格**无条件**用 `renderFakeCursor()` 包裹，与是否 focused 无关 |
| 投影点（主路径） | `extensions/pi-grok-remote-tui/host.ts:238`（HEAD 是 `:135`） | 只有 `.replaceAll(CURSOR_MARKER, "")` |
| 投影点（第二路径） | `extensions/pi-grok-rust-tui-bridge/index.ts:138` | 同缺陷；`PI_GROK_RUST_TUI_BRIDGE` 默认 **off**（`grok-pi.rs:503`） |
| 投影点（归档副本） | `extensions/_archived/pi-grok-remote-tui/index.ts:253` | 同缺陷，勿改，仅记录 |
| 渲染端 | `crates/codegen/xai-grok-pager/src/views/agent_status.rs:81` `ansi_remote_tui_line` | 依赖 `ansi-to-tui 7.0.0`；其 `any_escape_sequence()` 只消费 `ESC[` / `ESC]`，`ESC _` 仅吃掉 ESC → 载荷成正文 |
| 渲染端已有能力 | 同上，`ESC[7m`→`ESC[48;5;0m`、`ESC[27m`→`ESC[49m`，再映射为 theme `bg_highlight` | **修好后光标格自然高亮，Rust 侧不必改** |
| 嵌入方式 | `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/remote_tui_extension.rs:73-74`（`include_str!("host.ts")`），断言在 `:113`/`:128` | 改 `host.ts` 必须重新 `./build.sh` 并跑 cargo 侧嵌入测试 |

复现探针（本轮实跑，Node + 已安装 pi-tui）：

```text
raw      : ["> \u001b_pi:fc\u0007 \u001b_pi:/fc\u0007" + 空格padding]   # new Input().render(40)
current  : 同上（现状：只剥 CURSOR_MARKER，fake cursor 全保留）
proposed : "> \u001b[7m \u001b[27m" + 空格padding                      # 建议行为
```

基线测试（本轮实跑 `bun test extensions/pi-grok-remote-tui/`）：**14 pass / 0 fail**。
注意 `index.test.ts:461` 的 `custom host removes Pi hardware cursor markers from projected frames`
只断言 `not.toContain("pi:c")`，所以这个缺陷在绿测下依然存在 —— 这是"查漏补缺"的重点。

---

## 2. 待办（建议按序，各自可验证）

### T1 — 修复投影归一化（核心）
`extensions/pi-grok-remote-tui/host.ts` 的 `pushFrame()`（`:237-238`）按 Pi `resolveFakeCursors()`
语义归一化，而不是简单删除（删除会让光标不可见）：

```ts
const lines = component.render(frameWidth).map((line) =>
  String(line)
    .replaceAll(CURSOR_MARKER, "")
    .replaceAll("\x1b_pi:fc\x07", "\x1b[7m")   // = FAKE_CURSOR_START → 反显
    .replaceAll("\x1b_pi:/fc\x07", "\x1b[27m"), // = FAKE_CURSOR_END
);
```

约束：
- `FAKE_CURSOR_START/END` **未从 `@earendil-works/pi-tui` 公开导出**（`pi-main/packages/tui/src/index.ts`
  只导出 `CURSOR_MARKER`、`renderFakeCursor`）。写模块级字面量常量并加一句 "why" 注释，不要从包内私路径深导入。
- 顺序要紧：先剥 `CURSOR_MARKER`，再替换 fake cursor（Pi 的 `FOCUSED_FAKE_CURSOR` 语义是先 marker 后 fake）。
- 保持 `map`/`replaceAll` 现有风格，不要引入新依赖或抽象层（KISS）。

### T2 — 同步第二投影路径
`extensions/pi-grok-rust-tui-bridge/index.ts:138` 同样处理。该扩展默认关闭，但属于同一契约；
可抽到 `shared.ts` 的小工具函数（`normalizeFrameLine`）两处共用 —— 若两处共用会破坏
`remote_tui_extension.rs` 的模块 materialization 断言，需同步那里的模块列表/测试。

### T3 — 补回归测试
改 `extensions/pi-grok-remote-tui/index.test.ts:461` 那条测试（或新增一条）：
- fixture 换成含 fake cursor 的行，例如 `"before\x1b_pi:c\x07\x1b_pi:fc\x07 \x1b_pi:/fc\x07after"`；
- 断言投影后既不含 `pi:c`，也不含 `pi:fc` / `pi:/fc`；
- 断言光标格变成 `\x1b[7m`（或至少"被反显包裹"），避免未来用"删除"方式回归。

### T4 — 嵌入与编译验证（因 `include_str!`）
- `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi`（含嵌入模块 materialization 测试）
- `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi`
- 需要真机验证时再 `./build.sh`（会重建 grok-pi 内嵌扩展）

### T5 — 文档（workhub 是 `docs/` 唯一写者）
- 更新 `docs/issues/adapter/20260719-Remote-TUI-实验轨-RPC-custom-帧投影.md`：
  现有验收里「Pi 硬件光标 APC marker 在 frame 投影前剥离；`Type something.` 不再显示 `_pi:c`」
  已被 pi-tui 新增 fake-cursor 族打破，需要改写为覆盖三个标记，并补一条本次修复记录。
- `docs/issues/20260921-remote-tui-input-ownership.md` 的 Follow-up 段可追加一行证据指向本修复。
- 提交/记录走 workhub CLI（`~/.agents/skills/workhub/lib.ts`），不要手改表格。

### T6 — 端到端验收（推荐，非阻塞）
1. `PI_GROK_REMOTE_TUI=1 ./run-local.sh`（或 `./build.sh` 后用 `./run-local.sh`）；
2. 调出第三方 `ctx.ui.custom()` 组件：`/remote-tui` 探针，或
   `pi-custom-compaction` 的设置弹窗（F2/命令 → "Compaction model" 搜索框）；
3. 期望：搜索框光标格显示为高亮块，**屏幕上不出现 `_pi:fc` / `_pi:/fc`**；
4. 键控（字母 / Shift / 方向键 / Enter / Esc）行为不回归（参见 20260921 那份 issue 的验收清单）。

---

## 3. 明确不要做

- 不要改 Pi 源码（`pi-main/` 只读参考），不要改 `pi-tui`。
- 不要动 `ansi_remote_tui_line` 的 theme 映射（`ESC[7m` 高亮是修好后的正确落点）。
  可选兜底（先不做，除非发现第三个泄漏源）：让 `ansi_remote_tui_line` 把 APC（`ESC _ … BEL|ST`）
  当零宽消费；这属防御性扩展，违反 YAGNI 就不要提前加。
- 不要顺手"修"弹窗里列表列被截断（`gprivid er-chat (115-c/gprivid`）。这看起来是组件自身按
  `frameWidth` 布局 + `(1/690)` 分页的正常行为，未证实是 bug；如要动，先单独取证再开 issue。
- 不要跑全量 `cargo test`（lib 测试有约 110 个既有编译错误基线，见 `docs/VERIFICATION.md`）。
- 不要 `git restore .` / `reset --hard`；删除文件用 `trash`。

---

## 4. 工作区状态（重要）

工作区**不干净**，且 `extensions/pi-grok-remote-tui/` 已有未提交改动
（`host.ts`、`index.test.ts`、`shared.ts`、`transport.ts`、`transport.test.ts`，另有 `pi-grok-goal/index.ts`）。
`host.ts` 当前 238 行版本是未提交版本，HEAD 版本对应 135 行。
开工前先 `git status --short extensions/` 与 `git diff extensions/pi-grok-remote-tui/host.ts`，
只做本任务相关的最小增量，不要回滚他人未提交的工作。

---

## 5. 参考（按路径引用，勿复制内容）

- `extensions/pi-grok-remote-tui/host.ts` — `pushFrame()` 归一化点
- `extensions/pi-grok-remote-tui/index.test.ts:461` — 现有（覆盖不足的）回归测试
- `crates/codegen/xai-grok-pager/src/views/agent_status.rs:81` — ANSI→theme 映射
- `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/remote_tui_extension.rs` — 内嵌模块表与断言
- `docs/issues/adapter/20260719-Remote-TUI-实验轨-RPC-custom-帧投影.md` — 协议与非目标
- `docs/issues/20260921-remote-tui-input-ownership.md` — 输入所有权、PTY 验收方法
- `docs/issues/adapter/20261029-Remote-TUI鼠标事件对齐.md` — 同族投影改动先例
- `AGENTS.md`（仓库根）— 构建/验证与"扩展单独改动不跑 Cargo"的边界规则
- `docs/VERIFICATION.md` — 已知阻塞，不要在报告里把它们算作本次回归

---

## 6. Suggested skills

- `workhub` — 更新 `docs/` 里的 issue/进度（SSOT；先用 `search` 再动手，用户否决过的方案必须先查 `rejected.md`）
- `pi-extensions` — Pi 扩展与 `ctx.ui.custom()` / pi-tui 组件语义的权威参考
- `how-to-code` — 实现与测试的最小增量纪律
- `simplify`（收尾可选）— 只对本次新增代码做等价简化，不扩大到周边
