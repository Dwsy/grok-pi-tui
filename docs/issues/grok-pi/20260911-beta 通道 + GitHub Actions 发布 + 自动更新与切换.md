---
id: "2026-09-11-grok-pi-beta-channel-github-actions-auto-update"
title: "grok-pi beta 通道 + GitHub Actions 发布 + 自动更新与切换"
status: "done"
created: "2026-09-11"
updated: "2026-09-11"
category: "grok-pi"
tags: ["workhub", "grok-pi", "release", "updater", "github-actions", "channel", "beta"]
---

# Issue: grok-pi beta 通道 + GitHub Actions 发布 + 自动更新与切换

## 用途

本文件既是一份**可直接交接给新会话的任务提示词**，也是实现前的设计记录（AGENTS.md 要求复杂改动先有 `docs/issues/` 记录）。

---

## 任务提示词（可整段粘贴给执行者）

> 你在 `/Users/dengwenyu/Dev/AI/pi-grok-build`（grok-pi 项目，`origin = Dwsy/grok-pi`）工作。
>
> **目标**：让 grok-pi 支持 **beta 版本通道**，并打通 **GitHub Actions 发布** → **自动更新** → **通道切换** 的完整链路；默认行为必须保持 stable（现有用户不受影响）。
>
> **要实现的四件事**
> 1. **beta 版本**：允许发布 `vX.Y.Z-beta.N`（以及可选的 `-alpha.N`）预发布版本，与 stable 版本并存。
> 2. **GitHub Actions**：`.github/workflows/release.yml` 支持按 tag 自动判别通道，prerelease tag 产出 **GitHub prerelease**（同样 6 个平台产物 + `install.sh`/`install.ps1`）；stable tag 行为不变。
> 3. **自动更新**：grok-pi 的后台更新检查与 `grok-pi update` 按**当前配置的通道**解析目标版本；beta 用户能看到 beta，stable 用户**永远不**会被升到 prerelease。
> 4. **切换**：新增通道切换入口（如 `grok-pi update --channel beta|stable`），持久化到 grok-pi 自己的配置，并让 `grok-pi --version` 显示通道。
>
> **必须先读的现状事实（已核实，不要重复猜测）**
> - 发布流水线：`.github/workflows/release.yml` 只在 `v*` tag 触发；`validate-changelog` 用 `scripts/extract-changelog-section.py --strict` 要求 CHANGELOG.MD 存在与版本号精确匹配的小节；`release` job 用 `softprops/action-gh-release@v2` 且**没有** `prerelease` 设置。
> - grok-pi CLI：`crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs:268-288` 的 `Command::Update` 只有 `check / json / force / version`，**没有通道参数**。
> - 更新选项：`crates/codegen/xai-grok-update/src/pi_update.rs:249-258` 的 `PiUpdateOptions` **没有通道字段**；`fetch_pi_latest_version()`（同文件 `:19,:34`）写死 GitHub `releases/latest`（该语义天然排除 prerelease）。
> - 安装脚本：`install.sh:71-85` 的 `VERSION` 只支持 `latest | vX.Y.Z | X.Y.Z`；`pi_update.rs:341-359` 的安装同样走 `releases/latest`。
> - 后台自动更新：`grok-pi.rs:1160-1175` 打开 Pi 专属更新检查（GitHub Releases only），`GROK_PI_NO_AUTO_UPDATE=1` 可关闭；相关 API 为 `xai_grok_update::{check_pi_update_background, install_pi_update, fetch_pi_latest_version}`。
> - **不要复用 stock Grok 的通道机制**：`xai-grok-update/src/version.rs` 里的 `UpdateConfig.channel`、`channel_label/channel_name`、`CLI_BASE_URLS`（`https://x.ai/cli`、`xai-org-shared/grok-build`、npm `@xai-official/grok`）是上游 Grok 的后端，grok-pi 的更新链是独立的 GitHub-Releases-only。可以借鉴其**设计**（配置里存通道、通道感知解析、`--version` 带 `[channel]` 标签、`--alpha|--stable` 互斥 flag），但不要接上游 endpoint。
> - Semver 陷阱：prerelease 排序低于同版本正式版（`1.2.0 > 1.2.0-beta.1`），所以 stable 用户不会"升"到 beta；`build.rs:96-97` 与 `pi_update.rs:222-223` 已有相关注释，改动时保持该不变式。
> - 可复用的测试先例：`crates/codegen/xai-grok-pager-bin/tests/update_never_blocked_by_config.rs` 用本地 HTTP server 充当"通道指针"，照此模式给通道解析写 hermetic 测试。
>
> **设计决策（先确认，或按推荐执行并在 PR/提交信息里写明）**
> - 通道集合：推荐 `stable`（默认）| `beta`；`alpha` 是否要做请与用户确认。
> - 通道语义：stable = GitHub `releases/latest`；beta = 在 `/releases` 列表中取**最新的、tag 含 `-beta.` 的 prerelease**；必须用纯函数做 tag→channel 判别 + 版本选择，便于单测。
> - 通道持久化位置：**只写 grok-pi 自己的配置**（`~/.grok-pi/config.toml`，产品隔离，禁止写 `~/.grok`）。建议新增 `[update].channel = "stable"|"beta"`；若已有合适的 grok-pi 配置读取 seam 则复用之。
> - 切换入口：`grok-pi update --channel <c>` 持久化；同时保留 `--alpha/--beta/--stable` 之类的互斥 flag 可选（对齐上游 UX）。切换后 `--check --json` 输出要带 `channel`。
> - 后台自动更新：`check_pi_update_background` 必须读配置通道；`GROK_PI_NO_AUTO_UPDATE` 语义不变。
>
> **验收标准**
> 1. stable tag（`vX.Y.Z`）走现有路径，产出**非** prerelease 的 GitHub release，行为与现在完全一致。
> 2. `vX.Y.Z-beta.N` tag 触发同一套 6 平台构建，产出 **prerelease** release，且附带 `install.sh`/`install.ps1`。
> 3. CHANGELOG 校验对 prerelease tag 不再直接失败：要么允许 `X.Y.Z-beta.N` 小节，要么回退到 `X.Y.Z` 小节（二选一，需在文档里写清规则并加自测）。
> 4. `grok-pi update --check` 与 `--check --json` 按通道返回正确目标版本；`--json` 含 `channel`。
> 5. `grok-pi update --channel beta` 后能安装最新 beta；`--channel stable` 能切回并只解析 stable。
> 6. `grok-pi --version` 输出体现当前通道（对齐上游 ` [alpha]` 风格；见 `xai-grok-pager-bin/src/main.rs:1844,2671`）。
> 7. 纯函数覆盖：tag→channel 判别、prerelease 过滤、`stable 1.2.0 > 1.2.0-beta.1`、beta 用户能看到更高 stable。
> 8. 未配置通道的现有用户默认 stable，**不会**被自动带到 beta。
>
> **验证命令**
> ```bash
> # 说明：按本项目用户偏好，运行 Rust 命令前先征得同意
> ./scripts/cargo-shared.sh test -p xai-grok-update
> ./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
> ./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi
> rustfmt --edition 2024 --check <改动文件>   # 无法跑 cargo 时的静态语法/格式校验
> python3 scripts/extract-changelog-section.py --self-test
> python3 scripts/extract-changelog-section.py 1.2.0-beta.1 CHANGELOG.MD --strict   # 验证 prerelease 规则
> git diff --check
> ```
> GitHub Actions 无法本地执行：至少为 tag→通道 映射、workflow 的分支逻辑写单测/脚本自测，并在 PR 描述里给出一次真实 prerelease tag 的验证记录（或 `act`/dry-run 结论）。
>
> **约束（硬性）**
> - 产品隔离：任何状态只进 `~/.grok-pi`，不读不写 `~/.grok`（见 AGENTS.md「Product state isolation」）。
> - 不接上游 x.ai/GCS/npm 通道 endpoint；grok-pi 更新保持 GitHub-Releases-only。
> - 不弱化 verifier；如需动 `pi-grok-adapter/scripts/verify_native_grok.py` 的 baseline/allowed-seam，必须显式说明理由。
> - 提交聚焦：不要 stage 无关的工作区改动（当前工作区已有无关的 `web_config_extension.rs` 与 `extensions/pi-grok-web-config/*` 改动，必须保持不动）。
> - 若新增面向用户的能力，优先落在最小 Rust seam；不要为此引入新的大子系统。
>
> **非目标**
> - 统一 stock Grok 的通道机制 / enterprise 通道 / 服务端用户 cohort 下发。
> - 每次提交自动发 beta（除非用户明确要求）：beta 只由显式 tag 或 `workflow_dispatch` 触发。
>
> **交付物**
> - 代码 + 单测 + workflow 改动 + 安装脚本通道支持 + `README.md`/`docs/README.zh-CN.md` 的 `grok-pi update` 文档更新。
> - 完成后把本 Issue 的 `status` 与「Status 更新日志」更新，并列出实际运行过的验证命令与结果。

---

## 已核实的现状证据（附录）

| 关注点 | 位置 | 现状 |
|---|---|---|
| 发布触发 | `.github/workflows/release.yml:3-7` | 仅 `v*` tag + `workflow_dispatch` |
| 通道矩阵 | 同上 `:42-90` | 只有平台矩阵（macOS ARM64/Intel、Linux x64/ARM64、Windows x64/ARM64），无通道维度 |
| prerelease 标记 | 同上 `:277-284` | `softprops/action-gh-release@v2` 未设置 `prerelease` |
| CHANGELOG 强校验 | 同上 `:13-36` | `extract-changelog-section.py --strict` 要求精确版本小节 |
| grok-pi 更新 CLI | `xai-grok-pager-bin/src/bin/grok-pi.rs:268-288` | 仅 `check/json/force/version` |
| 更新选项结构 | `xai-grok-update/src/pi_update.rs:249-258` | 无 channel |
| 最新版本解析 | `xai-grok-update/src/pi_update.rs:19,34` | GitHub `releases/latest` |
| 后台自动更新 | `xai-grok-pager-bin/src/bin/grok-pi.rs:1160-1175` | GitHub-only，`GROK_PI_NO_AUTO_UPDATE` 可关 |
| 安装脚本版本 | `install.sh:71-85`、`pi_update.rs:341-359` | 只支持 latest/固定版本 |
| stock 通道（勿复用） | `xai-grok-update/src/version.rs:56-81,97` | `UpdateConfig.channel`、`x.ai/cli`、`xai-org-shared/grok-build`、npm `@xai-official/grok` |
| stock 通道开关 UX（可借鉴） | `xai-grok-pager-bin/src/main.rs:2438-2449,1844,2671` | `--alpha/--stable/--enterprise`、` [alpha]` 版本标签 |
| semver prerelease 陷阱 | `xai-grok-pager-bin/build.rs:96-97`、`pi_update.rs:222-223` | 已有 `-dirty` / prerelease 排序说明 |
| hermetic 更新测试先例 | `xai-grok-pager-bin/tests/update_never_blocked_by_config.rs` | 本地 server 充当通道指针 |

## 本次实现决策

1. 通道只实现 `stable|beta`；未引入 `alpha`，保持最小产品面。
2. `stable` 继续使用 GitHub `releases/latest` 语义并明确拒绝 prerelease；`beta` 扫描 GitHub releases，选择 semver 最大的合法 beta/正式版，因此 beta 用户可从 `1.2.0-beta.1` 升到更高的 `1.2.0` 正式版。
3. 通道只持久化到 grok-pi 隔离配置 `~/.grok-pi/config.toml` 的 `[update].channel`，不读写 stock `~/.grok`。
4. 本次只暴露 CLI/配置，不新增 F2 设置项，也不新增 `GROK_PI_CHANNEL` 环境变量覆盖。
5. prerelease CHANGELOG 采用“精确 `X.Y.Z-beta.N` 小节”规则；现有 extractor 已支持，不增加弱化 fallback。

## Status 更新日志

### 2026-09-11 — done

- `.github/workflows/release.yml` 对 `-beta.` tag 设置 GitHub prerelease，继续复用同一套 6 平台构建与 installer assets。
- `xai-grok-update::pi_update` 新增独立 `stable|beta` 解析、`[update].channel` 持久化、channel-aware 后台检查/手动更新，并保持 stock Grok 通道后端完全隔离。
- `grok-pi update --channel beta|stable`、`--check --json` channel 字段、`grok-pi --version` channel 标签已接入；beta 安装从目标 release tag 获取 `install.sh`/`install.ps1`。
- 新增纯函数测试覆盖 tag→channel、draft/prerelease 过滤、`1.2.0 > 1.2.0-beta.1`、beta 接受更高 stable；因用户明确要求节省磁盘，本次未运行任何 Cargo 命令，测试代码已落盘但未执行。
- 文档同步：`README.md`、`docs/README.zh-CN.md`、`docs/FEATURE_MATRIX.md`。

实际执行并通过的非 Cargo 验证：

```bash
rustfmt --edition 2024 --check crates/codegen/xai-grok-update/src/pi_update.rs crates/codegen/xai-grok-update/src/lib.rs crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/cli.rs
rustfmt --edition 2024 --check --config skip_children=true crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs
python3 scripts/extract-changelog-section.py --self-test
# 另用临时 CHANGELOG 验证 1.2.0-beta.1 精确小节可被 --strict 提取
sh -n install.sh
git diff --check
```

结果：`extract-changelog-section.py --self-test` 报告 `self-test ok: 28 sections, head=0.1.9`；临时 prerelease 小节严格提取成功（56 bytes）；其余静态检查均通过。GitHub Actions 与真实 prerelease tag 发布未在本地执行。

### 2026-09-11 — review hardening

- `workflow_dispatch` 新增必填 release tag，并与 tag push 共用同一个规范化 tag；release job 允许手工触发，显式设置 `tag_name`/`target_commitish`。
- workflow 只接受 `vX.Y.Z` / `vX.Y.Z-beta.N`，拒绝未实现的 alpha/rc，避免它们被误发布成 stable GitHub release。
- stable discovery 对 GitHub/page/npm/JSP 每个来源都再次强制校验“无 prerelease”；beta 在主源语义选择失败时继续尝试 JSP fallback。
- 显式 `grok-pi update --channel stable` 现在即使有效默认已是 stable 也会落盘，可物化缺失 channel 并修复合法 TOML 中的非法 channel 值。
- 新增纯函数测试覆盖 stable source guard 与 channel 配置物化/修复。
