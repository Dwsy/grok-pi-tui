# Edit 卡片展开卡死：正则后端回归

- 日期：2026-11-03
- 状态：根因配置已修正，485 项 Markdown 测试通过；应用二进制未重建
- 接力来源：Pi 会话 `01a1a1a5-b7e9-779c-b23e-14aa57178295`
- 复现来源：Pi 会话 `01a1a198-94de-779c-b23e-14a891d59b60`

## 用户问题

已完成的 README Edit 在内联展开和 Enter 打开的 BlockViewer 中都会卡住。用户要求修复此前正常行为发生回退的关键原因，不能仅以异步处理掩盖慢路径，也不要重复执行无关的编译检查。

## 已证明的配置回归

上游提交 `bc7f02eddd3d84085849dc19ed216f11c23b0571`（2026-08-28）将工作区的 `syntect = "5.3"` 改为仅启用 `default-fancy`，关闭了原先默认的 Oniguruma 后端。最近的 `8f5b4cda` 和 `ddf239f4` 没有修改 Edit hunk 高亮算法，dev `opt-level=0` 也不是近期变化。

固定 syntect 5.3.0、two-face 0.4.5、Grok Day 主题、dev 未优化构建，以及用户会话中相同的三组 README 编辑，只改变 syntect 的后端 feature：

| 原始调用行 | 当前 fancy 后端 | 恢复原配置 | 高亮行数 | 样式差异 |
|---|---:|---:|---:|---:|
| 129，英文 README | 10.354 s | 1.085 s | 37 | 0 |
| 137，中文 README | 6.652 s | 0.951 s | 34 | 0 |
| 139，中文 README | 5.897 s | 0.986 s | 34 | 0 |
| 合计（含运行开销） | 22.904 s | 3.026 s | 105 | 0 |

逐行比较分段文本、前景 RGB 和字体标志，105 行全部一致。`/usr/bin/sample` 的慢路径栈为 `HighlightLines::highlight_line → ParseState::search → Regex::new → fancy_regex::RegexBuilder::build`。该输入会触发 Markdown 动态正则的反复构建；换回原后端能直接降低这项开销。

这里定位的是这个具体输入的配置性能回归，不声称用户最后一次正常使用的版本一定早于该上游提交。

## 最终修改

- 将根 `Cargo.toml` 的 syntect 依赖恢复为原配置 `syntect = "5.3"`，重新启用默认 Oniguruma；保留 two-face 原有配置，使语法数据和旧版本对照一致。
- `Cargo.lock` 只增加所需的 `onig` / `onig_sys`，不升级其他依赖。
- 撤回前一轮异步绕行。5 个相关 Pager 源码文件均先校验为本任务上一轮的确切哈希，再恢复为任务开始前的内容；未覆盖用户的其他修改。保留原生同步语法配色和已有后台增强行为。
- 在 Markdown crate 新增真实 README 链接的性能回归，断言 old/new 文本原样、高亮分段仍然存在，以及两次完整高亮不超过 1 秒。
- 源码身份清单只登记这个新增测试文件，撤回前一轮给 worker 增加的修改例外。

## 验证状态

- 后端 A/B：两次运行均 exit 0；105 行文本、RGB、字体和分段完全一致；总耗时 22.904 s → 3.026 s。
- 当前工作区：484 项 Markdown 库测试 + 1 项新增集成回归全部通过，0 失败，最终 Cargo 命令 exit 0。新增用例直接执行测得 old/new 两次高亮共 299 ms，保持实际语法分段。
- 最终 rustfmt、`git diff --check`、源码例外清单一致性与 lockfile 变更范围检查通过。命令为 `./scripts/cargo-shared.sh test -p xai-grok-markdown --lib --test edit_highlight_regression -- --test-threads=1 --quiet`。
- 前一轮的 64 项测试对应已撤回的异步方案，不能作为最终方案的验收结果。
- 未运行全量工作区构建，未替换正在运行的 grok-pi，也未做新二进制的终端交互验收。历史 JSONL 未改写，未提交或推送。
