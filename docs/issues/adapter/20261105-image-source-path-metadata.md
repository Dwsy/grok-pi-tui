# 图片附件原始路径元信息（issue #12）

- 日期：2026-11-05
- 来源：https://github.com/Dwsy/grok-pi-tui/issues/12
- 接力会话：`01a1a9ae-6206-7095-941e-706a9259a51d`
- 状态：已完成；验证范围与既有失败见下文

## 目标

粘贴图片文件路径后，继续发送图片字节，并将原始路径作为元信息附加到 Pi 收到的用户提示词。用户明确要求简单附加路径，不增加 UI。

## 修复前证据

- `PastedImage.source_path` 已保留用户粘贴的路径，包括用户可见的符号链接名。
- `build_content_blocks_with_prefixes_and_caps_ref` 将提示词中的 `[Image #N: <path>]` 改为 `[Image #N]`，ACP 图片 URI 优先使用耐久会话副本，不能用 URI 代替原始路径。
- `prompt_to_pi` 仅保留 ACP 图片的 Base64 和 MIME，丢弃 URI 与元信息。
- 原会话没有写入实现。工作区有其他任务的未提交改动，必须保留。

## 实现边界

1. ACP 图片 `_meta["xai.dev/imageSourcePath"]` 单独携带原始路径；保留现有耐久 URI、图片字节、显示编号、预览和占位符行为。
2. Pi 适配器在用户文本后附加 `<image_metadata>`，逐张记录 `[Image #N] path: <JSON 字符串>`。优先原始路径，无原始路径时可使用已有 `file://` 耐久路径；无本地路径则不生成路径。
3. 正常发送、排队与插话复用 `prompt_to_pi`；不修改 Pi 源码、JSONL 会话或全局安装。
4. 仅为 `prompt_images.rs` 声明新的精确原生集成 seam，保留基线哈希和其他校验。

## 验收

- [x] 原始路径优先于耐久副本，空格、中文和符号链接路径保持原样。
- [x] 多图编号、只有图片、剪贴板耐久路径和无路径附件行为正确；图片数据保持不变。
- [x] 原生图片构造与 Pi 适配器定向测试通过；相关 build/check 完成。
- [x] 原生架构校验、差异/格式检查完成；既有失败单独记录。

## 验证记录

- PASS：`./scripts/cargo-shared.sh test -p pi-grok-adapter --lib prompt_bridge::tests -- --nocapture`，7 passed / 0 failed，exit 0。覆盖原始路径优先、中文/引号、真实显示编号、图片字节/MIME 保留、剪贴板本地 URI 回退、图片顺序、无路径与纯文本/纯图片提示词。
- PASS：基线清单比较确认 `baselineFiles` 哈希及其他字段均保留，仅新增 `prompt_images.rs` 的一个精确 `allowedModifiedFiles` 项；验证器 Python 语法有效。
- 架构校验：13 passed / 8 failed，exit 1；失败项集合与接力源会话 JSONL L153 的既有结果完全一致。包括上传树/渲染器历史基线、slash/动态命令、扩展 UI、运行时事件、语义及对话静态匹配。没有放宽这些检查。
- PASS：`./scripts/cargo-shared.sh test -p xai-grok-pager-render --lib prompt_images::tests -- --quiet`，156 passed / 0 failed，exit 0。包括真实持久化后的原始路径与耐久 URI 分离、符号链接原名、恢复占位符路径和全部原生图片既有用例。首次依赖编译耗时 46m54s，外层命令超时；缓存复跑在正常退出后确认上述结果。
- PASS：`./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi`，exit 0；`./target/debug/grok-pi --version`，exit 0，输出 `grok-pi 0.1.12-beta.1+dirty [stable]`。
- PASS：两处 Rust 源码 `rustfmt --check --edition 2024`、`git diff --check`、测试后源码哈希一致性核对；逐文件对比接力前快照确认仅叠加本任务改动，保留其他未提交修改。
- 未运行真实模型/终端交互验收，未替换全局安装或发布版本。
