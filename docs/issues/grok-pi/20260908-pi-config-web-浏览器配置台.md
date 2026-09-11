# `/pi-config web` / `/pi-models web`：Pi 扩展托管的浏览器配置台

日期：2026-09-08
状态：已实现（源码与真实 `server.ts` 临时实例回归通过；待下一次正常构建后确认实际注入页）

## 目标

把 Pager 原生的 `/pi-config`（资源管理）与 `/pi-models`（providers/models）两个模态，
在浏览器里提供一份等价、可编辑的版本。约束：

- 功能本身**全部用 Node/TypeScript 实现**，放在 `extensions/` 下，作为 Pi 扩展运行；
  Rust 侧不渲染、不编辑任何配置。
- 随机端口（`listen(0)`），仅回环绑定，每次启动生成一次性 token。
- 保留原生模态：不带参数时行为完全不变。

## 2026-09-09 UI/UX 重构阶段

### 目标

- 保留现有模型、资源、F2、settings.json 全部能力与安全边界，不改配置所有权。
- 重做页面信息架构：让“模型”“资源”“主机设置”“原始设置”各自有明确主任务、上下文与危险操作层级。
- 去掉当前单文件里状态、API、渲染、表单、i18n、样式互相缠绕的结构；在不引入前端框架/依赖的前提下拆出清晰职责。
- 改善响应式、键盘可达性、焦点、空状态、错误/保存反馈与编辑确认。
- 以真实 `/pi-config web` 页面做浏览器回归，覆盖窄屏与主要编辑路径。

### 验收标准

- [x] `/pi-models web`：provider 搜索/选择、模型查看、切换当前、设默认、新增/编辑/删除语义保持可用。
- [x] `/pi-config web`：extensions 增删、skills/prompts/themes 浏览与过滤保持可用。
- [x] F2 设置：catalog 分区、标量编辑、restart 提示、只读嵌套表保持可用。
- [x] settings.json：常用开关与原始 JSON 编辑、校验、保存状态保持可用。
- [x] 页面在窄宽度下不横向溢出；语义控件、可见焦点、键盘操作可用。
- [x] 前端结构不再把全部 CSS/HTML/JS 逻辑堆在一个 1250 行文件里；注入器完整物化新资源。
- [x] 不运行 Cargo；使用针对性静态检查、TypeScript/Node 检查和真实浏览器交互验证本次改动。

### 2026-09-09 验证记录

- 前端拆为 `index.html`、`styles.css`、`app.js`、`models.js`、`resources.js`、`host.js`、`settings.js`，并以 `ui-config.json` / `i18n.json` 作为外部配置与文案 SSOT；页面职责不再堆在单文件。
- 所有 JS 源片段通过 `node --check`；现有 Bun 对扩展入口完成 TypeScript 转译；服务端完整组装 web 源文件后无未解析 marker。
- `rustfmt --check` 与限定范围 `git diff --check` 通过；本次未运行 Cargo，也未安装缺失的 `@types/node`。直接 `tsc` 因本机缺少该类型包只能停在环境依赖，改用已有 Bun 做无下载转译验证。
- 使用真实 `server.ts` + 内存配置依赖启动临时 loopback 实例：未带 token 的 `/api/state` 为 401，带 token 为 200；最终 HTML 含完整 UI 且无组装 marker。
- `agent-browser` 回归通过：新增 Provider（2→3）、切换当前模型到 `xai/grok-4`、把 OpenAI 设为默认、extension 删除/新增（33→32→33）、F2 bool 写入、Settings 快捷开关写入、非法 JSON 错误/dirty 状态、390px 宽度无横向溢出；浏览器错误列表为空。
- 由于 Rust 侧通过 `include_str!` 烘焙这些源文件，当前已经运行的旧二进制不会热更新到新 UI；按本次“不运行 Cargo”的约束，实际注入页留到下一次正常构建/重启后确认。

### 2026-09-09 JSON 配置与日夜模式

- `ui-config.json` 统一声明页面标题键、资源类型与分页大小、快捷 Settings 项、语言 storage key，以及主题 storage key / 模式顺序；JS 只消费该配置，不再内嵌这些项目常量。
- `i18n.json` 作为中英文文案唯一来源；旧 `i18n.js` 已移除。快捷 Settings 标题/说明、主题文案和校验错误也全部使用 i18n key。
- `server.ts` 启动时解析并校验两个 JSON 必须是对象，再 `JSON.stringify` 后安全内联到单页脚本；仍不增加任何静态资源 HTTP 路由，浏览器拿到的仍是一份 HTML。
- 主题支持 `system` / `light` / `dark` 三态，存入 `piWebTheme`。`system` 跟随 `prefers-color-scheme`；显式 Day/Night 通过 `data-theme` 覆盖系统主题，并沿用同一套 CSS token。
- `agent-browser` + 真实 `server.ts` 临时实例回归：最终 HTML 同时含 `UI_CONFIG` / `I18N` 且 marker=0；资源页按 JSON 的 24 条分页；System→Day→Night 切换后分别得到无 `data-theme` / `light` / `dark`，Day 背景 `#f5f6f8`、Night 背景 `#111318`；Night reload 后保持；切换中文后主题文案变为“主题 · 夜间”，6 个快捷 Settings 文案均来自 JSON；390px 无横向溢出且浏览器错误为空。

## 架构

```text
/pi-config web  ─┐
                 ├─ Pager slash 命令转发（DirectPiCommand）
/pi-models web ─┘
                 └─ Pi 扩展 pi-grok-web-config（node:http + 单页 HTML）
                      ├─ GET  /               单页 UI（注入 token）
                      ├─ GET  /api/state      models.json / settings.json / 资源清单 / 鉴权状态
                      ├─ PUT  /api/models     写 models.json + Pi reload
                      ├─ PUT  /api/settings   写 settings.json + Pi reload
                      ├─ POST /api/use-model  pi.setModel()
                      ├─ POST /api/reload     ctx.reload()
                      └─ POST /api/shutdown   关闭服务器
```

### 各层职责

| 层 | 位置 | 职责 |
|---|---|---|
| 命令转发 | `xai-grok-pager/src/slash/commands/pi_config.rs`、`pi_models.rs` | 仅识别 `web` 参数并 `DirectPiCommand` 给 Pi；无参数仍打开原生模态。补 `suggest_args` 让下拉可见 `web` |
| 注入器 | `xai-grok-pager-bin/src/bin/grok_pi/web_config_extension.rs` | `include_str!` 全部模块到临时目录，写入 `ui.html`，导出 `PI_GROK_WEB_CONFIG_UI` |
| 接线 | `grok-pi.rs` | bridge 扩展启用时创建、追加 `--extension`、下发 UI 路径环境变量 |
| 扩展 | `extensions/pi-grok-web-config/` | 注册 `pi-config-web` / `pi-models-web`；起服务；读写配置；触发 reload |

### 为什么命令名是 `pi-config-web` 而不是 `pi-config`

`pi-config` / `pi-models` 在 `PI_GROK_NATIVE_COMMANDS` 里是 Pager 内建命令，
Pi 侧注册同名命令会被内建注册表遮蔽。因此扩展使用独立命令名，由内建命令把 `web`
参数转发过去。

## 安全边界

- 绑定 `127.0.0.1`（可用 `--host` 覆盖），端口随机（`--port` 可固定）。
- 每次启动生成 24 字节随机 token，写进页面并通过 `x-pi-token` / `token` 校验；
  跨站页面无法读取（无 CORS），可防 DNS rebinding / CSRF 驱动写配置。
- 校验 `Host` 头，只允许 localhost / 127.0.0.1 / ::1。
- 写入前校验 JSON 结构（models.json 必须有 `providers` 对象、每个 model 有字符串 `id`），
  非法内容直接 500，不会落盘；写文件采用 tmp + rename 原子替换。

## 数据来源（全部走 Pi 自己的路径）

- `getAgentDir()` → `models.json`、`settings.json`（支持 `PI_CODING_AGENT_DIR`）。
- 读取容忍 JSONC（剥离注释），保存为 2 空格缩进 JSON。
- 资源清单：`DefaultResourceLoader`（skills / prompts / themes）+ `settings.json` 的
  路径数组 + Pi 进程 `process.argv` 里本次实际加载的 `--extension` 等路径。
- 鉴权状态：`ctx.modelRegistry.runtime.getProviderAuthStatus()`。

## F2 设置与 i18n（第二轮）

- **F2 设置进 web**：新增 "F2 Settings" tab。两类来源一一映射：
  1. *扩展注册*：注入器把 build.rs 烘焙的 `BUNDLED_HOST_UI_SOURCES`（全部
     `extensions/*/grok-pi.json`）序列化为 `host-catalog.json`，经
     `PI_GROK_WEB_CONFIG_CATALOG` 交给扩展 → label/description/section/order/
     default/restartRequired 与原生 F2 完全一致。
  2. *grok 自带*：读取 `$GROK_HOME/config.toml`（`GROK_HOME`，缺省 `~/.grok-pi`）
     的 `[ui]` 标量表。被 catalog 覆盖的键用富信息行，未覆盖的标量键用通用编辑器；
     `[ui.*]` 嵌套表只读展示。
- **写入安全**：`PUT /api/host-ui` 只接受 bool/number/string 标量、键名白名单
  `[A-Za-z0-9_-]+`；TOML 重写按行替换/插入，仅动 `[ui]`，其余字节（注释、
  `[voice]`、其他 section）原样保留；`[ui]` 不存在时追加。
- **i18n**：UI 文案中英双语（`navigator.language` 以 `zh` 开头自动中文，
  右上角按钮手动切换并持久化到 `localStorage`）；来自配置文件的数据（provider、
  grok-pi.json 的 label）保持原样不翻译。

## 静态审查修复（第二轮）

- `models.json` / `settings.json` 解析失败时服务器返回空文档——任何保存都会把
  原文件覆盖掉。现在 UI 顶部出警告横幅，且所有保存路径（含"设为默认"、资源路径
  增删）在错误状态下拒绝写入。
- 401（丢了 token 的刷新）与启动加载失败改为整页 fatal 视图并给出可行动提示。
- 表单：Enter 提交 / Esc 关闭；清空可选字段现在会真正删除键（含 cost 全空时删除）。
- token 列格式化（200000 → 200k）；provider 列表标注 current/default；
  资源页与 F2 页各加过滤框；hash 保留深链接并响应 `hashchange`。

## 已知取舍

- 保存 `models.json` / `settings.json` 会重写文件，原文件里的注释会丢失。
- 每次保存都会触发 Pi `reload()`（对齐原生 `/pi-models` 的 live reload 语义）。
  扩展状态放在 `globalThis`，reload 后服务器与端口保持不变。
- 资源页的 extensions 列表只覆盖 `settings.json` 与 CLI 传入路径；
  Pi 自动发现但未显式配置的路径不在此列（避免二次加载用户扩展）。

## 验证

- `cargo check -p xai-grok-pager-bin --bin grok-pi`：通过。
- `web_config_extension.rs` 单测：断言入口闭包里每个模块都被物化。
- `pi_config.rs` / `pi_models.rs` 单测：`web` 参数转发、`web --no-open`、未知参数报错。
- `server.ts` 冒烟脚本：随机端口、token 注入、无 token 401、PUT 后 reload、
  非法 JSON 500、shutdown 关闭连接。
- 待人工：在 grok-pi 里执行 `/pi-models web`，浏览器打开后改一个 provider 并确认
  `models.json` 落盘、Pi 已 reload。
