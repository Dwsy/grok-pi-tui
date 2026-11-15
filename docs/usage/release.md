# 发布 grok-pi

发布流程在 [`.github/workflows/release.yml`](../../.github/workflows/release.yml)，
平台清单在 [`scripts/release-targets.json`](../../scripts/release-targets.json)。

## 发布步骤

1. 在 `CHANGELOG.MD` 的 `## [Unreleased]` 下新增版本段落，格式必须是
   `## [X.Y.Z] - YYYY-MM-DD` 或 `## [X.Y.Z-beta.N] - YYYY-MM-DD`。
   CI 会用 `scripts/extract-changelog-section.py --strict` 校验，缺段落直接失败。
   本地先自检：

   ```bash
   python3 scripts/extract-changelog-section.py --self-test
   python3 scripts/extract-changelog-section.py 0.1.14-beta.4 CHANGELOG.MD --strict -o /tmp/notes.md
   ```

2. 提交并推送代码，然后打 tag：

   ```bash
   git tag v0.1.14-beta.4
   git push origin v0.1.14-beta.4
   ```

   **beta tag push 自动只构建 macOS ARM64 与 Intel** 并发布 prerelease；
   正式版 tag push 仍构建全部六个平台。
   `-beta.N` 后缀会被 GitHub 标记为 prerelease，不会移动 `releases/latest`。

> `workflow_dispatch` 仍可通过 `targets` 自定义平台。发布全新 beta 最简单的方式
> 是直接推 tag，不再需要取消自动触发的六平台构建。

## 手动指定 macOS ARM64（本地单人 beta）

默认 beta 自动构建两个 macOS 架构；若只需一台 Apple Silicon Mac 测试，可手动指定单一目标。

```bash
gh workflow run release.yml \
  -f tag=v0.1.14-beta.4 \
  -f targets=macos-arm64
```

或者在 GitHub Actions 页面手动 Dispatch workflow，填：

| 输入 | 值 |
|---|---|
| `tag` | 已存在的 tag，如 `v0.1.14-beta.4` |
| `targets` | `macos-arm64` |

> `tag` 必须已经存在并推到远端 —— 工作流会 `checkout` 该 tag，
> 所以**先推 tag，再 dispatch**。
>
> 手动重跑某个已存在的 beta tag 时，可用 `targets` 明确覆盖默认矩阵；
> 避免在自动 tag 构建仍运行时重复触发同一版本的发布。

`targets` 留空 = beta 为 macOS 两个平台，stable 为全部六个平台。多个用逗号分隔：

```bash
-f targets=macos-arm64,macos-x86_64
```

### 可用的 target key

| key | 平台 | runner |
|---|---|---|
| `macos-arm64` | macOS ARM64（`aarch64-apple-darwin`） | `macos-15` |
| `macos-x86_64` | macOS Intel（交叉编译） | `macos-15` |
| `linux-x86_64` | Linux x64 | `ubuntu-24.04` |
| `linux-aarch64` | Linux ARM64（原生 arm runner） | `ubuntu-24.04-arm` |
| `windows-x86_64` | Windows x64 | `windows-latest` |
| `windows-aarch64` | Windows ARM64（原生 arm runner） | `windows-11-vs2026-arm` |

未知 key 会被 `prepare-matrix` 拒绝并列出所有合法值，不会静默跳过。

## macOS 构建缓存（跨 beta tag 复用）

GitHub Actions 的 tag cache 不能跨版本复用；Release 现在只**读取**缓存、不在 tag
上保存。需要时先在 `main` 运行一次独立预热，它会为 ARM64 和 Intel 两个目标
分别编译并保存缓存，后续 beta tag 可读取 `main` 的缓存：

```bash
gh workflow run macos-release-cache-warm.yml --repo Dwsy/grok-pi-tui --ref main
gh run list --repo Dwsy/grok-pi-tui --workflow macos-release-cache-warm.yml --limit 3
gh cache list --repo Dwsy/grok-pi-tui
```

- **预热必须先完成，再推新 tag**。首次预热仍是冷构建；缓存 miss 时发布会自动冷构建，不影响正确性。预热不会自动在每次 `main` push 运行，防止双倍 CI 消耗。
- 两个目标的 cache key 按 target、Rust toolchain、Cargo manifest/lockfile 和 release profile 区分。会缓存依赖及可复用的 workspace crates，但不缓存最终用户安装包。
- 缓存存在大小和淘汰限制。若缓存体积、下载/上传耗时反而高于节省的编译时间，停止预热或关闭缓存即可；不能以 `gh cache list` 有记录就推断命中。
- macOS release 目前仍使用 `jobs=8`（Runner 只有 3 CPU）；专用 `macos-cargo-jobs-lab.yml` 可同源比较 3/8 jobs。**在实测前不调整正式发布的并发数。**
- 改动只对新 tag 生效；已经启动的 beta.4 不受影响。

## 工作原理

- `prepare-matrix` job 读取 `scripts/release-targets.json`，beta 默认只保留两个
  macOS target，stable 保留全部六个平台；显式 `targets` 输入覆盖默认，输出
  `matrix=<json>` 到 `$GITHUB_OUTPUT`。
- `build` job 用 `matrix: ${{ fromJSON(needs.prepare-matrix.outputs.matrix) }}`
  拿到动态矩阵；平台定义不再硬编码在 workflow 里。
- `validate-changelog` 校验版本段落，`release` job 把 `grok-pi-*` 产物连同
  `install.sh` / `install.ps1` 一起发布。
- release 里只包含实际构建出的 asset 与安装脚本，不宣称未构建的平台。
  beta 默认为两份 macOS archive；正式版走 tag push 全平台。
