# grok-pi Pi-only ARM64 候选包（非正式 Release）

Pi-only 瘦身分支 `refactor/pi-slim-20261116` 的测试产物只通过显式调用的
[Pi slim ARM64 CI](../../.github/workflows/pi-slim-ci.yml) 生成；
**不修改 tag 驱动的 `release.yml`，不覆盖稳定版/正式 Beta**。

## 生产候选包闸门

选择 `runner=macos-15`（原生 Apple Silicon），流水线会运行：

1. Pi-only / stock Grok 的依赖闭包断言；Workspace、Shell / 测试模块的编译检查。
2. ARM64 `release-dist` 二进制构建、`--version`、`--help`、能力清单的真实执行。
3. 原生 Pager + 模拟 Pi JSONL RPC 的 PTY 冒烟测试。
4. Herdr 内置扩展真实 Unix Socket 的生命周期测试（`idle/working/blocked`）。
5. 工具共享契约、原生 diff、renderer 测试。

**仅上述全部通过时**，上传 `pi-slim-macos-arm64-<full Git SHA>` Actions artifact，
其中有 `grok-pi-macos-arm64-pi-slim-<short SHA>.tar.gz`、
`.tar.gz.sha256`、`build-info.txt`。
跨架构 Intel runner 只编译、不能执行 ARM64，所以**不产出候选包**。

## 构建并获取候选包

在仓库维护者终端执行：

```bash
gh workflow run pi-slim-ci.yml \
  --repo Dwsy/grok-pi-tui \
  --ref refactor/pi-slim-20261116 \
  -f runner=macos-15

gh run list --repo Dwsy/grok-pi-tui \
  --workflow pi-slim-ci.yml --limit 3
```

流水线成功后，从该 run 的 Artifacts 下载，或执行：

```bash
gh run download <RUN_ID> --repo Dwsy/grok-pi-tui \
  --pattern 'pi-slim-macos-arm64-*' \
  --dir ./pi-slim-candidate
```

一个目录内应同时包含归档、校验文件和 `build-info.txt`。核对提交
与预期一致后，在**归档所在目录**运行：

```bash
shasum -a 256 -c *.tar.gz.sha256
tar -tzf *.tar.gz
```

解包预期只有根目录中的可执行文件 `grok-pi`。先在临时目录解包验证，
再备份并原子替换本机的 `~/.local/bin/grok-pi`：

```bash
set -euo pipefail
archives=(grok-pi-macos-arm64-pi-slim-*.tar.gz)
test "${#archives[@]}" -eq 1 && test -f "${archives[0]}"
archive="${archives[0]}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
tar -xzf "$archive" -C "$scratch"
file "$scratch/grok-pi" | grep -q 'Mach-O 64-bit executable arm64'
"$scratch/grok-pi" --version
mkdir -p "$HOME/.local/bin"
cp -p "$HOME/.local/bin/grok-pi" \
  "$HOME/.local/bin/grok-pi.backup-$(date +%Y%m%dT%H%M%S)"
install -m 755 "$scratch/grok-pi" "$HOME/.local/bin/.grok-pi-next"
mv -f "$HOME/.local/bin/.grok-pi-next" "$HOME/.local/bin/grok-pi"
"$HOME/.local/bin/grok-pi" --version
```

已有的 `pig → ~/.local/bin/grok-pi` 软链接不需要修改。
正在运行的进程不会被重启；要测试新版，启动新终端会话。

## 完成标准

候选包通过**本地真实 Pi RPC**和 Herdr Socket 冒烟后才能进入人工体验。
真实 Herdr Pane 是否达到 `full_lifecycle_hook_authority` 必须在运行
新版的 Pane 内检查，不能仅凭界面显示 `idle` 判定。
没有达到产品级别的签名、公证、正式版本号、发行说明和真实用户冒烟
之前，**不得将这个候选包视为正式 Beta Release**。
