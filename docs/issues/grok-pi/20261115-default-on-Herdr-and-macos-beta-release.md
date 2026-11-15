---
id: "2026-11-15-default-on-herdr-macos-beta-release"
title: "Default-on Herdr and macOS-only beta distribution"
status: "publishing"
created: "2026-11-15"
updated: "2026-11-15"
category: "grok-pi"
tags: ["herdr", "release", "github-actions", "macos"]
---

# Herdr 默认启用与 macOS beta 发布

## 约束

- 默认启用 Herdr 桥接，但仅在具备 `HERDR_ENV`、Socket 和 pane ID 的 Herdr pane 内发送报告；已有显式 `pi_herdr=false` 仍能禁用。
- 发布新 beta tag 只构建 macOS ARM64 和 Intel 两个包；正式版保留六个平台。GitHub Actions 负责 Rust 构建，本地不运行 Cargo。
- 当前主工作树的本地修改按功能分组提交，保留其他 worktree 的状态，不改 Herdr 源码。

## 验收清单

- [x] Rust config 默认值、serde 缺省、host manifest/F2 元信息一致为启用
- [x] 文档与用户配置说明一致，Node 生命周期测试通过（4/4）
- [x] beta tag 构建矩阵仅 macOS 两项，changelog 提取可通过 strict
- [x] 本地改动按主题分组提交；远端发布仍待验收
- [ ] 确认主工作树干净且已推送
- [ ] 推送新 beta tag 并检查对应 GitHub Actions 的实际结果及 release assets

## 风险

Herdr API 返回成功并不证明 Header 已获得 full-lifecycle authority；该项需要新发行版在实际 Herdr pane 中单独验收。原有项目 `AGENTS.md` 禁止本地未授权 Cargo 构建。
