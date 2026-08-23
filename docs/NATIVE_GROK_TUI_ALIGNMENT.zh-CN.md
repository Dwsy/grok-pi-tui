# Native Grok TUI 对齐说明

## 验收结论

当前入口不是自绘 Ratatui 壳。`grok-pi` 位于 Grok 的生产 binary package `xai-grok-pager-bin`，并调用 `xai_grok_pager::app::run_external`。适配器只产生 ACP 请求/通知；所有终端表面由 Grok Pager 创建。

## 复用的 Grok 生产组件

| 能力 | 原生实现位置 | Pi 接入方式 |
|---|---|---|
| Terminal 生命周期 | `xai-grok-pager/src/app/mod.rs` | `run_external` 进入同一 init/writer/event-loop/restore 路径 |
| 全屏/minimal/inline | `xai-grok-pager` + `xai-grok-pager-minimal` | 使用相同 screen mode resolver 与 IoC hook |
| 输入编辑器 | `views/prompt_widget` | Pi prompt 与 `set_editor_text` 进入 PromptWidget |
| 斜杠补全 | `slash` + `views/completion_dropdown` | Pi `get_commands` 转 ACP AvailableCommand 后合并 |
| Markdown/代码 | `xai-grok-markdown` | AgentMessageChunk/AgentThoughtChunk 进入原生 scrollback pipeline |
| 工具与 diff | `acp/tracker`、原生 RenderBlock/EditToolCallBlock | Pi tool lifecycle 转 ACP ToolCall/ToolCallUpdate；external-only F2 开关（默认关闭）在宽度足够时委托 sibling 并排布局 renderer，关闭或窄布局继续使用原生 unified renderer |
| 问答弹层 | `views/question_view` | select/confirm/input/editor 转 `x.ai/ask_user_question` |
| 状态与通知 | 原生 toast/sticky surface | notify/setStatus/setWidget 转窄 ACP notification |
| 滚动与 transcript | 原生 scrollback/transcript | 历史和实时事件均作为 ACP SessionUpdate |
| 模型选择 | 原生 model selector | Pi models/thinking levels 转 SessionModelState |

## 保持不变的证据

验证清单对上传的 Grok 源码建立 SHA-256 baseline：

- renderer/input/Markdown 文件在声明接缝之外保持逐字节一致；EditTool hook、viewer 元数据、模块注册和 sibling 布局 renderer 均精确列入 verifier；
- verifier 从当前 manifest 动态计算未改文件数量，不依赖硬编码总数；
- 允许修改集合保持显式且语义化：workspace/ACP/App/dispatch/slash 接缝，以及窄范围 EditTool 设置、hook、viewer 与 renderer 接缝；
- `pi-grok-adapter` 中不存在 Ratatui/Crossterm、Terminal、Frame、Widget、draw、event::read 或 raw-mode 调用；
- `grok-pi.rs` 中不存在任何直接绘制或输入循环。

## 为什么仍需修改少量 Grok Pager 文件

ACP 标准没有覆盖 Pi 的全部 UI/命令语义，因此需要窄接缝：

1. `UiProfile::External`：关闭 Grok.com 产品能力，但不改变渲染器。
2. `AcpConnection::external`：让现有 Pager 接受外部 ACP channel。
3. `run_external`：复用生产 terminal/event-loop，跳过 Grok Agent 启动和登录。
4. Pi UI notification handlers：把 fire-and-forget 状态映射到原生 toast/banner/title/editor。
5. QuestionView hints：复用原生 freeform editor，并支持 Pi timeout 撤销。
6. slash profile：只选择对 Pi 有意义且在外部 ACP 组合下可完整工作的现有 Grok 命令；Pi 动态命令仍由原生 registry 管理。
7. `/compact <instructions>`：把 Grok 原生命令中的可选文本传到 Pi `customInstructions`。
8. screen-mode 边界：Grok 原生 minimal/fullscreen renderer 保留，但原版 slash re-exec 会重建 Grok 自有 `--resume` argv，无法携带 `grok-pi` 的 Pi 启动参数，因此仅保留启动选项，不暴露失效的 `/minimal`、`/fullscreen`。
9. 教程内容 profile：stock Grok 保留默认 onboarding；grok-pi 只注入 18 个静态产品能力主题，继续复用原生 `TutorialState`、modal、picker、Markdown renderer 与输入路由。

这些接缝没有新建第二套 TUI 或 scrollback pipeline，也没有复制 PromptWidget、QuestionView、Markdown 或 tool-card 基础设施。可选 sibling EditTool 布局 renderer 复用原生 diff 数据、高亮 helper、输出行、unified 回退、选择与全屏 viewer 行为。

## 不做的事情

- 不重新实现 Grok TUI；
- 不复制 Pi-TUI；
- 不增加适配器专属命令面板；
- 不用字符画模拟 toast、widget 或 modal；
- 不把 Grok 登录、云 session、usage、plugin、voice 等产品功能错误地暴露给 Pi；
- 不伪造 Pi RPC 没有暴露的 extension component factory。
