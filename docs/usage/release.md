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
   python3 scripts/extract-changelog-section.py 0.1.14-beta.3 CHANGELOG.MD --strict -o /tmp/notes.md
   ```

2. 提交并推送代码，然后打 tag：

   ```bash
   git tag v0.1.14-beta.3
   git push origin v0.1.14-beta.3
   ```

   tag push 会**自动构建全部六个平台**并发布 prerelease。
   `-beta.N` 后缀会被 GitHub 标记为 prerelease，不会移动 `releases/latest`。

## 只构建 macOS ARM64（本地单人 beta）

适合「就我一个人在测试」的迭代，省掉另外五个 runner 的时间和额度。

```bash
gh workflow run release.yml \
  -f tag=v0.1.14-beta.3 \
  -f targets=macos-arm64
```

或者在 GitHub Actions 页面手动 Dispatch workflow，填：

| 输入 | 值 |
|---|---|
| `tag` | 已存在的 tag，如 `v0.1.14-beta.3` |
| `targets` | `macos-arm64` |

> `tag` 必须已经存在并推到远端 —— 工作流会 `checkout` 该 tag，
> 所以**先推 tag，再 dispatch**。tag push 本身会触发一次全平台构建，
> 若只想跑单平台，可以在推 tag 后立刻取消那次自动运行，再手动 dispatch。

`targets` 留空 = 全部六个平台（与 tag push 行为一致）。多个用逗号分隔：

```bash
-f targets=macos-arm64,macos-x86_64
```

### 可用的 target key

| key | 平台 | runner |
|---|---|---|
| `macos-arm64` | macOS ARM64（`aarch64-apple-darwin`） | `macos-15` |
| `macos-x86_64` | macOS Intel（交叉编译） | `macos-15` |
| `linux-x86_64` | Linux x64 | `ubuntu-latest` |
| `linux-aarch64` | Linux ARM64（原生 arm runner） | `ubuntu-24.04-arm` |
| `windows-x86_64` | Windows x64 | `windows-latest` |
| `windows-aarch64` | Windows ARM64（原生 arm runner） | `windows-11-vs2026-arm` |

未知 key 会被 `prepare-matrix` 拒绝并列出所有合法值，不会静默跳过。

## 工作原理

- `prepare-matrix` job 读取 `scripts/release-targets.json`，按 `targets` 输入
  过滤，输出 `matrix=<json>` 到 `$GITHUB_OUTPUT`。
- `build` job 用 `matrix: ${{ fromJSON(needs.prepare-matrix.outputs.matrix) }}`
  拿到动态矩阵；平台定义不再硬编码在 workflow 里。
- `validate-changelog` 校验版本段落，`release` job 把 `grok-pi-*` 产物连同
  `install.sh` / `install.ps1` 一起发布。
- 只构建一个平台时，release 里也就只有那一个 asset + 安装脚本。
  这是**有意为之**：beta 通道仅供本地测试，正式版请走 tag push 全平台。
