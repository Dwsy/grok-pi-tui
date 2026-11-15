# Tool call density 上游追溯

日期：2026-11-13
结论：**上游公开源码里不存在引入该功能的提交，无法 cherry-pick。** 该功能只存在于 xAI 内部 monorepo 构建的发布二进制中（本机 `grok 1.0.84 (fe120b472e27)`）。

## 一、功能标识

从本机二进制 1.0.84 提取到的权威定义（strings）：

- `/density` 命令：`Set tool call density, from Detailed to Compact`
- 配置键：`[ui] tool_call_density`
- 枚举值：`detailed` / `compact-shells` / `compact-ungrouped` / `compact-grouped` / `compact-all-grouped`
- 内嵌文档表格（与截图一致）：

| Label | Level | Edits | Shell commands | Grouping |
|---|---|---|---|---|
| Detailed | `detailed` | Diff shown | Output preview shown | Only reads, searches, and other tools fold |
| Diff focus | `compact-shells` | Diff shown | One-line row | Only reads, searches, and other tools fold |
| Balanced | `compact-ungrouped` | One-line `+N/-M` row | One-line row | Only reads, searches, and other tools fold |
| Grouped | `compact-grouped` | One-line row | One-line row | Runs of edits, of commands, and of other tools each fold separately |
| Compact（默认） | `compact-all-grouped` | One-line row | One-line row | Every run of tool calls folds into one row |

- 别名：`verbose` → `detailed`，`minimal` → `compact-all-grouped`
- 旧配置迁移：`[ui] collapsed_edit_blocks = true` 或 `[scrollback.blocks.edit] expanded_by_default = false` → `compact-ungrouped`；否则 → `compact-shells`。`GROK_COLLAPSED_EDIT_BLOCKS` / `GROK_GROUP_TOOL_VERBS` 同样按此映射。
- 归属文件（二进制内 panic/event 路径）：`crates/codegen/xai-grok-pager/src/app/dispatch/settings/setters.rs`（1.0.84 中约 1139 行）、`settings/defs.rs`、`slash` 层。

## 二、证据：公开上游没有该提交

1. **上游仓库只有 main 一个分支，且停在 2026-09-29。**
   `git fetch upstream --tags --force` 后：`upstream/main = 2bdd1d6a`（2026-09-29，`Synced from monorepo`），共 51 个提交；`git ls-remote --heads/--tags upstream` 只有 `refs/heads/main`，无 tag；GitHub API 的 releases 为空。
2. **上游树内无该功能。**
   `git grep -n density upstream/main` 共 16 处，全部是已有的 `compact_mode`（“more content density”）、代码注释与测试关键词，没有 `ToolCallDensity` / `tool_call_density` / `/density` / `Diff focus` / `compact-shells`。
3. **本地全部 ref（含 tag、备份、同步分支）无引入提交。**
   `git log --all -S'/density'`、`-S'ToolCallDensity'`、`-S'compact grouped'`、`-S'edits and shells'`、`-G'tool.call.density' -i`、`--grep density -i` 均为 0 条。
4. **GitHub 代码搜索无结果。**
   `gh search code '"Diff focus"' --owner xai-org`、`tool_call_density --owner xai-org`、`ToolCallDensity --owner xai-org` 均为 0 条；全站搜索只命中无关项目（kimi-code、nao 等）中同名的自有实现。xai-org 组织内其他仓库（grok-prompts、grok-build-plugin-cc、plugin-marketplace 等）也都没有。
5. **功能确实在发布二进制里，但源码提交不可达。**
   - `~/.grok/bin/grok` → `grok-1.0.84-macos-aarch64`，`grok --version` 输出 `grok 1.0.84 (fe120b472e27)`，含上述全部字符串与内嵌文档。
   - `git cat-file -t fe120b472e27` 失败：该 revision 在任何本地 ref 都不存在，公开 remote 也无法获取。
   - 版本边界：`grok-1.0.81-macos-aarch64`（2026-11-11）已含 `tool_call_density`；2026-07 的 `grok-macos-aarch64` 不含。说明它进入内部 1.0.x 线后从未以源码形式发布。

因此不存在“上游某一次提交”可供摘取；`upstream` 的公开源码快照（0.1.x/1.0.24 基线）落后发布二进制约两个月。

## 三、cherry-pick

无。未创建 `feat/tool-call-density` 分支，未改动任何代码，用户未提交的工作区改动保持原样。

## 四、若要手写实现的最小落点分析

当前 fork 基线（pager 1.0.24）已有三个独立开关：`[ui] compact_mode`（消息内边距）、`[ui] group_tool_verbs`（只折叠 read/search/list/subagent 连续行）、`[ui] collapsed_edit_blocks`（编辑块一行 diffstat）。density 是把它们统一成一个枚举，并**新增**两项当前不存在的行为：shell 一行化、编辑/命令 run 的分组折叠。

### 4.1 设置与持久化（机械改造，低风险）

| 文件 | 改动 |
|---|---|
| `crates/codegen/xai-grok-shared/src/ui_config.rs` | 新增 `tool_call_density: Option<ToolCallDensity>`、默认值与 serde 解析 |
| `crates/codegen/xai-grok-pager/src/appearance/cache.rs` | 新增进程级缓存（照 `group_tool_verbs`/`collapsed_edit_blocks` 模式） |
| `crates/codegen/xai-grok-pager/src/settings/defs.rs` | 新增 `SettingKind::Enum` 项：Appearance 分类第一条、label `Tool call density`、文案与关键词 |
| `crates/codegen/xai-grok-pager/src/settings/registry.rs` | 枚举 choices、读取/写入分支 |
| `crates/codegen/xai-grok-pager/src/settings/layout.rs` | 行序（Appearance 首行） |
| `src/app/actions.rs`、`dispatch/settings/setters.rs`、`dispatch/router.rs` | `Action::SetToolCallDensity` 及 setter（级联更新三个旧开关的运行时效果） |
| `src/views/settings_modal/state.rs`、`src/views/pi_settings/actions.rs` | 选择器状态、预览开关 |
| `crates/codegen/xai-grok-shell/src/util/config/settings_writes.rs` | `[ui] tool_call_density` 落盘 |
| `src/app/acp_handler/settings.rs` | 远程/托管配置同步分支 |

### 4.2 `/density` 命令（低风险）

- 新增 `src/slash/commands/density.rs`：无参显示 5 个标签与说明并标记当前值；带参接受 label、level、别名。
- 在 `src/slash/commands/mod.rs::builtin_commands()` 注册（参考 `compact_mode.rs`）。

### 4.3 渲染行为（主要工作量与风险）

| 文件 | 改动 |
|---|---|
| `src/scrollback/state/groups.rs` | 现有 `scan_verb_runs` 只折叠 verb 类；需按 density 增加“编辑 run”“命令 run”“全部 run”三种折叠族，并生成摘要文案（`Edited 2 files, ran 1 command +12 -4`） |
| `src/scrollback/render.rs` | 折叠行的投影与摘要渲染 |
| `src/scrollback/blocks/tool/execute.rs` | 新增全局 “compact shells” 一行行模式；今天只有按块启发式的自动折叠 |
| `src/scrollback/blocks/tool/edit.rs` | `effective_line_summary` 改由 density 驱动，而非仅 `collapsed_edit_blocks` |
| `src/scrollback/blocks/tool/codemode.rs` | 文档说明 daemon 路由下 code cell 跟随 shell 列 |

### 4.4 迁移与测试

- 旧键与两个环境变量映射到最近档位（见第一节）。
- 测试：`tests/settings_e2e.rs`、`src/app/dispatch/tests/settings.rs`、`src/views/settings_modal/tests.rs`、`src/scrollback/state/groups.rs` 单测、`scrollback/blocks/tool/snapshots` 快照。

### 4.5 规模判断

设置/slash 管线约 10 个文件、属既有模式复制；真正的功能量在 4.3：1.0.24 基线没有“命令/编辑 run 折叠”和“全局 shell 一行化”，这两块需要新写并配套快照测试。若只做 5 档枚举但复用现有三个开关，可得到一个近似版本（Detailed / Diff focus / Balanced 可用），Grouped 与 Compact 两档会退化为与 Balanced 相同的渲染。

## 五、遗留问题

1. 是否手写实现、实现到哪一档，需要用户确认。建议先做 4.1+4.2+“编辑一行化 remap”（约 1–2 天量级），Grouped/Compact 的折叠族另立 issue。
2. 上游源码快照（2026-09-29）与发布二进制（1.0.84, 2026-11-13）差距持续扩大；若未来 xAI 重新发布源码，应以新快照为准重新评估，而不是基于本手写版本。
3. 本机 1.0.84 的内嵌文档可作为行为规范来源；但这只是二进制字符串，不构成源码授权或 API 契约。

## 六、实现记录（进行中，2026-11-13）

状态：**未完成，未提交。** 主工作区（`main`）的 18 个未提交改动原样保留；实现在
worktree `/Users/dengwenyu/Dev/AI/pi-grok-density`，分支 `feat/tool-call-density`
（从 `e3ec2279` 新建）。

### 已完成（库代码可编译：`cargo check` 通过）

- `xai-grok-shared/src/ui_config.rs`：`ToolCallDensity` 枚举（五档、label、
  description、`parse` 支持 label/canonical/`verbose`/`minimal`、行为谓词、
  `from_legacy` 迁移）；`[ui] tool_call_density: Option<String>`；旧键改为迁移输入。
- `xai-grok-pager-render/src/appearance/cache.rs`：`load/set_tool_call_density`，
  首读时按 `tool_call_density` → `GROK_*` 环境变量 → 旧键 → 默认的顺序取值。
- 设置层：Appearance 新增首个 section “Tool calls” 与 `Tool call density` 行
  （文案/关键词/五档 choices 与截图一致）；移除 `group_tool_verbs`、
  `collapsed_edit_blocks` 两个设置行及其 Action/setter/persist 链路；新增
  `Action::SetToolCallDensity`、`set_tool_call_density`、shell
  `set_tool_call_density`、ACP 推送同步。
- `/density`：新增 `slash/commands/density.rs` 并注册；无参列出五档并标出当前值，
  带参接受 label/canonical/别名，非法参数返回 `CommandError` 文本。
- 渲染：
  - shell 在 Detailed 用输出预览（Truncated），其余档位固定一行（Collapsed）；
  - 编辑在 Detailed/Diff focus 展开 diff，其余档位一行 `+N/-M`；
  - 分组模式 `VerbOnly` / `ByFamily` / `AllRuns`，按档位折叠 verb run、编辑 run、
    命令 run 或混合 run；
  - 折叠摘要改为句首大写、后续小写、同类合并的格式
    （`Explored 2 files, 1 search`、`Edited 2 files, ran 1 command +12 -4`）；
  - 切换档位时对现有 transcript 重新 materialize（`apply_tool_call_density_flip`）。

### 未完成

1. `--all-targets` 仍不通过：大量单元/e2e 测试引用已删除的
   `Action::SetGroupToolVerbs` / `SetCollapsedEditBlocks`、旧 cache 断言与旧标签文案，
   需要逐项改写为 density 断言（`app/dispatch/tests/settings.rs`、`tests/settings_e2e.rs`、
   `app/acp_handler/tests/settings.rs`、`scrollback/render_tests.rs`、`state/groups.rs` 等）。
2. 新增测试尚未补：枚举解析/迁移、`/density` 参数、五档折叠分组单测与快照。
3. `cargo fmt --all --check`、`clippy --all-targets -D warnings`、全量 pager 测试未跑。
4. 未提交、未 push、未开 PR。

### 已知与截图的偏差（待确认）

- 未实现设置弹窗里的 live preview 面板（截图中的 preview 区）。
- 单条编辑的折叠摘要显示 `Edited 1 file`，没有还原截图里的文件名（`Edited report.py`）。
- “Ran 4 unit tests” 这类基于命令描述的摘要未实现，命令 run 统一用计数
  （`ran 2 commands`）。
- 折叠摘要的动词统一为 Explore/Edit/Run 家族；`Read 1 skill` 等个别措辞与上游可能不同。
