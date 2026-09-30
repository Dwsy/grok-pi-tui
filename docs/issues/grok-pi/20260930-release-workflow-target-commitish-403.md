---
id: "2026-09-30-release-workflow-target-commitish-403"
title: "workflow_dispatch 发布已有 tag 时 Releases API 403"
status: "done"
created: "2026-09-30"
updated: "2026-09-30"
category: "grok-pi"
tags: ["grok-pi", "release", "github-actions", "workflow-dispatch", "403"]
---

# Issue: workflow_dispatch 发布已有 tag 时 Releases API 403

## 现象

GitHub Actions run `36542528585` 的六个平台构建全部成功，`Publish GitHub release` 在 `softprops/action-gh-release@v2` 创建 `v0.1.10-beta.9` Release 时返回 `403 Resource not accessible by integration`。Job 日志显示 `Contents: write` 已生效。

## 根因

手动重跑从当前 `main` 启动，但发布的是已经存在的旧 tag。workflow 先 checkout 该 tag，再把 tag commit SHA 作为 `target_commitish` 传给 Releases API。该旧 commit 与当前默认分支在 `.github/workflows/` 下存在差异；GitHub 对这种 release 创建请求要求 token 具备 Workflows 写权限，而 Actions 自带 `GITHUB_TOKEN` 无法获得该权限，因此被拒绝。

`target_commitish` 只在目标 tag 尚不存在时才需要。这里 validate/build/release 都要求 checkout 一个已存在 tag，所以应省略 `target_commitish`，让 GitHub Release 直接绑定现有 tag。

## 修复计划

1. 删除 release job 中解析 tag commit SHA 的步骤。
2. 删除 `softprops/action-gh-release` 的 `target_commitish` 输入，并写明现有 tag 约束。
3. 保留 `contents: write`、显式 `tag_name`、prerelease 判定和六平台产物逻辑不变。
4. 运行 workflow 静态检查、CHANGELOG extractor self-test 与 `git diff --check`。

## 实现与验证

- `.github/workflows/release.yml` 的手动输入说明改为“Existing release tag”，明确此路径只发布已有 tag。
- 删除 release job 的 `Resolve pinned release commit` 步骤，并删除 `softprops/action-gh-release` 的 `target_commitish` 输入。
- 保留 `contents: write`、显式 `tag_name`、beta `prerelease` 判定和全部 release assets。
- 已确认 `v0.1.10-beta.9` 的 tag commit 与当前 `main` 在 `.github/workflows/release.yml` 存在差异，与 GitHub Releases API 的 workflow-write 限制吻合。
- `python3 scripts/extract-changelog-section.py --self-test` 通过：`37 sections, head=0.1.10-beta.9`。
- `git diff --check` 通过。由于本次仅修改 GitHub Actions YAML 与问题记录，未运行 Cargo。
- 本地不能等价模拟 GitHub 发 Release 的安装 token；最终远端验收应在提交/推送修复后重新 `workflow_dispatch` 同一已有 tag。
