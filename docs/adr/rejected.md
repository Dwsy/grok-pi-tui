# Rejected Proposals

被否决的方案。一行一条。提交新方案前先搜关键词，避免重复提议。

| 日期 | 方案 | 理由 |
| --- | --- | --- |
| 2026-11-05 | 扩展崩溃上报前用 [Y/n] 提示让用户选择 | 提示只在 TTY 下出现，后台 attach/CI 崩溃因此零上报；默认键已是 yes，提示不提供真实选择权。改为自动上报 + GROK_PI_NO_TELEMETRY=1 关闭 |
| 2026-11-08 | 首版即实现 Pi session appendEntry 持久化与 active-branch replay 的 L3 多文件改造 | 用户要求极简方案：bash 命令前缀/env 注入 + envtool 管理项目级 .pi 文件并由 agent 自主管理；session 级能力渐进增强，不在首版展开 |
| 2026-11-08 | 在 pi-grok-bash 中引入第三方 dotenv npm 包或转写其源码来解析 .env | 用户确认优先使用 Node 内置 util.parseEnv；避免新增 npm 依赖与第三方代码 |
