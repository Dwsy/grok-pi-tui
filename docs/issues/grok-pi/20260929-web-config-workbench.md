# Web 配置工作台重构

日期：2026-09-29
状态：实现完成；随 v0.1.10-beta.9 与 Eval MCP、Remote TUI PR #10 一并发布

## 用户目标与范围

用户确认建设完整配置工作台：模型、资源、F2、Pi 设置；完整中英界面，改善搜索、编辑、保存体验。

## 检查证据

- 现有 web 为无依赖单页，由 server.ts 组装；Rust include_str! 注入已有资源。
- 已有 i18n.json，但 F2 catalog 元数据未翻译；模型编辑未暴露 headers / compat 等高级字段。
- 仅 extensions 路径可增删，其他资源只读；快捷设置会自动提交原始 JSON 草稿。
- 保留 Pi 配置所有权、回环服务、token / Host 检查和注入文件闭包。无需新增运行时依赖。

## 实施计划

1. 建立清晰的导航、全局搜索、会话/默认模型摘要和响应式工作区。
2. 扩展模型高级编辑与复制，资源四类路径管理，F2 双语搜索/恢复默认，Pi 分组表单与安全草稿。
3. 加强输入校验、写入反馈、失败保留、未保存提醒；真实服务和浏览器验证，更新说明。

## 验证

- `bun test extensions/pi-grok-web-config/tests/config-store.test.ts`：5 pass / 0 fail（前端注入组装、i18n 对齐、模型校验与未知字段保留、JSONC、TOML）。
- `bun extensions/pi-grok-web-config/tests/browser-regression.ts`：使用 `tests/fixture.ts` 启动仅内存的真实回环 HTTP 服务，验证中英切换、分组 Pi/F2 设置草稿及显式保存、冲突拦截、Provider/Model 克隆、资源路径、全局搜索、窄屏溢出及暗色主题。浏览器命令使用独立 session，并显式重开原 token URL 验证持久偏好。
- 仅在合成内存配置与临时浏览器会话上执行写入；无用户真实 settings/models/config 修改。`extensions/pi-grok-web-config/.impeccable/design.json` 与 `DESIGN.md` 为设计记录；`.impeccable/review/` 截图及本地 fixture URL 是临时验证产物，不进入发行源码。
