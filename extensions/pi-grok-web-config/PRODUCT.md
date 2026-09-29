# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

grok-pi 使用者在浏览器管理当前 Pi 环境的模型、资源、F2 和日常设置。

## Product Purpose

用户已确认：完整配置工作台，中英双语，统一搜索、编辑和保存反馈。

## Operating Context

从 /pi-config web 或 /pi-models web 打开，Pi 扩展启动本地回环 HTTP 服务；当前模型与启动默认值分别管理。

## Capabilities and Constraints

现有无依赖 HTML/CSS/JavaScript；通过 server.ts 组装并由 Rust 注入。Pi 拥有 models.json 与 settings.json；主机拥有 config.toml [ui]。保留未知配置字段、鉴权和产品状态隔离；新增字段以仓库契约为准。用户自定义资源文案与标识不翻译。

## Evidence on Hand

web/、config-store.ts、shared.ts；docs/issues/grok-pi/20260908-pi-config-web-浏览器配置台.md。
