# Side-by-side diff 默认词级高亮（2026-10-21）

## 目标

grok-pi 的 side-by-side EditTool diff 默认突出变化的单词，而不是给删改行整行着色。F2 Appearance → Tool output 中保留原有布局开关，并新增词级开关；关闭时恢复原整行样式。

## 设计与边界

- 仍使用 Pager 原生 side-by-side / unified diff 渲染，无第二套 UI；仅对并排 EditTool 及普通全屏 diff 查看器生效。
- 独立 PAGER-owned、external-only 的 F2 开关 `side_by_side_word_highlight`，进程级默认 `true`，无需重启、无 Shell 配置模式变更。现有并排开关仍默认关闭。
- 成对的 Delete/Insert 源行用 `similar::TextDiff::from_words` 获取 UTF-8 变化区间；新增/删除的无配对行突出其实际内容，空列和 padding 不着色。保留语法前景色。
- 软换行按原有单词换行边界拆分 styled spans，保证变化的单词在续行仍高亮。Equal 不着色。窄终端与 code-review 仍沿用 unified。
- 单侧超过 8 KiB 的超长行回退为整段内容背景（不着色 padding），避免在终端绘制热路径执行昂贵的词差异算法。
- 全屏查看器在 F2 粒度切换后重建编辑行；diff 复制与源行元数据不变。

## 验证

- 已添加 `side_by_side_edit::tests` 五个测试，覆盖差异词、整行兼容、软换行、中文 UTF-8、新增/删除和 Equal 行。
- 已运行 `rustfmt`、`git diff --check`，均通过。
- 曾执行 `./scripts/cargo-shared.sh test -p xai-grok-pager side_by_side_edit::tests --lib -- --nocapture`；首次冷构建耗时超过 27 分钟，持续编译工程基础 crate，测试未执行即因超时停止。**单元测试及完整 Rust 构建尚未通过验证**。后续使用热缓存重跑该命令并运行设置注册表测试。
- 不触碰已有的 Plan Mode / Pi Adapter 未提交工作。

## 复审补充（2026-10-21）

- `render_side_rows` 在词级模式下把 `ChangedWordRanges` 传入前先判空：Equal 行与未配对但内容相同的行不会走 `TextDiff`，也拿不到背景色；删除/新增的无配对行仍整段内容着色（不含 padding），符合预期。
- 软换行的 `spans_in_range` 以 `wrap_text` 的**字节**偏移对齐 `LineEditor`/`String` 呈现：`wrap_text` 用 `split_inclusive` 保留空白，拼接后的字节总数与 `raw_content` 严格相等，`spans_in_range` 因此不会切坏 UTF-8 边界。
- 关闭词级开关时，背景回到整行（含 padding）语义：`fit_side_content_line` 用同一个 `background` 填充 padding，与旧实现的观感一致。
