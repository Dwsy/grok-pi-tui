---
id: "2026-10-30-Remote-TUI-fake-cursor-APC归一化"
title: "修复 Remote TUI fake-cursor APC 标记泄漏"
status: "draft"
created: "2026-10-30"
updated: "2026-10-30"
category: "adapter"
tags: ["workhub", "pr", "remote-tui", "bugfix"]
---

# 修复 Remote TUI fake-cursor APC 标记泄漏

Pi 1.4.0 的 Input/Editor 会输出 fake-cursor APC 标记；原帧投影仅清除硬件光标 marker，导致 Pager 将 `_pi:fc` / `_pi:/fc` 显示为正文。两条活动投影路径现将 fake-cursor 边界转换为 ANSI SGR 7/27，复用现有 Pager 光标高亮。

## 变更

- `extensions/pi-grok-remote-tui/host.ts` 与 `extensions/pi-grok-rust-tui-bridge/index.ts` 使用本地字面量常量并按顺序归一化三个 marker；不增加依赖、公共 API 或生产模块。
- 主路径回归覆盖多个光标格、已有 ANSI 以及 marker 清除；bridge 独立回归捕获实际 frame 通知。两套测试复用原有 Pi mock 导出，避免 Bun 进程级 module mock 在合并执行时相互覆盖；该 fixture 不进入生产扩展 bundle。
- 保留工作树中此前存在的鼠标/transport 及其他功能改动。Pi 源码、Pager theme 与归档扩展未修改。

## 关联 Issue

- [Remote TUI 帧投影](../../issues/adapter/20260719-Remote-TUI-实验轨-RPC-custom-帧投影.md)
- [输入所有权 follow-up](../../issues/20260921-remote-tui-input-ownership.md)

## 验证

- 修复前：两条路径均因 `pi:fc` 泄漏失败；修复后：remote host 14/14，bridge 1/1，合并运行 15/15，通过。
- 系统 Pi 1.4.0 隔离 RPC：真实 Input 与 Editor 各验证 Enter 提交、Escape 退出，共 4/4；覆盖字母、大写 Z 和左方向键，帧均无 APC 载荷且保留反显。未调用模型或写入用户配置。
- Rust bin 测试目标 96/96，通过，包含 Remote TUI 依赖闭包 materialization 与 bridge 嵌入契约；生产 Rust check 通过。
- `./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi` 退出 0，新二进制已包含修复。
- 新二进制 + 系统 Pi 1.4.0 的隔离 PTY：Input/Editor × Enter/Escape，共 4/4，通过。包含字母、大写 Z、左方向键及重复开关组件；终端输出无 fake-cursor 标记。探针复用内嵌 host，未单独加载工作树 remote-tui 源码。
- `git diff --check`：本次涉及路径通过；Pi 子模块 Git 状态干净。
- `./build.sh` 退出 2：可选 pi-main 的依赖产物与源码类型不匹配（缺少 TuiMainScreen、setCapabilityOverrides、AgentToolCallOutcome 等导出），发生在 Rust 构建之前。未修改 Pi 源码，直接 Rust 构建及系统 Pi 1.4.0 验收已完成。
- 原截图中的第三方 Compaction model 弹窗未单独手测；以上是本次验证边界。

## 风险与回滚

范围限于完整 Pi cursor marker 的帧投影替换；已有 ANSI 保持原样。若需要回滚，只撤销本次两条生产路径的常量与替换链及对应测试，保留此前的未提交改动；不要整文件 restore。
