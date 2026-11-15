# macOS beta 构建优化：跨 tag 缓存与资源并发

日期：2026-11-15
状态：实施中；尚未经过新 GitHub Actions run 验证

## 已确认的事实

- beta.4 的 Intel 构建 24m04.7s，Cargo timing 显示 `Fresh units=0`、`Dirty units=1400`，没有命中编译缓存。
- macOS ARM64 与 Intel 两个 Job 都在 `macos-15` 的 Apple Silicon runner 上执行，前者原生、后者交叉编译。
- runner `ncpu=3`，仓库 `.cargo/config.toml` 固定 `jobs=8`，Cargo 报告峰值 concurrency=10。这是资源竞争风险，**不能直接推出**降低 jobs 一定更快。
- 原 Release 工作流刻意关闭 cache，仓库 Actions cache 条目为 0。GitHub cache 按 ref 隔离：beta.3 tag 的 cache 不可直接被 beta.4 tag 读取；**默认分支 main 的 cache 可以被 tag 工作流读取**。

## 最小改动方案

1. 单独的 `main` 预热工作流使用 GitHub-hosted macOS runner，仅编译 ARM64 并写入 main 范围的 Rust dependency cache。
2. release 在对应目标上仅**读取** main 缓存，绝不在 tag 中写 cache。缓存 miss 不影响发布。
3. 预热与 ARM64 release 使用相同 Rust toolchain、target、profile、Cargo flags；不使用第三方持久化服务，不 cache 最终发布二进制。
4. 同步修复现存的 `macos-cargo-jobs-lab.yml` 旧 runner/toolchain，使其能做公平的 3 vs 8 并发实验；生产 jobs 暂不改，等数据。
5. 只允许 `workflow_dispatch` 在 main 上预热，不自动在每次 push 编译两轮；先通过缓存大小和实测编译耗时证明收益，再决定是否自动预热。

## 验收

- [x] 发布 workflow 与预热 workflow 的 cache key、编译器、target/profile 参数一致（静态校验通过）
- [x] Release tags 只 restore，main 才 save，避免跨 tag 冗余缓存
- [x] 核验 YAML 和静态断言，不在本地运行 Cargo
- [ ] 新的 main 预热 run 成功保存 cache，并核验 tag release 命中、总耗时和 cache 大小
- [ ] 3 jobs 与 8 jobs 的同源实验后再决定生产默认并发

## 后续裁决：独立 macOS 构建只保留 ARM64

2026-11-15：后续 beta tag 默认矩阵改为唯一 `macos-arm64`，缓存预热取消 Intel
矩阵，macOS compile lab 仅保留 ARM64 变体。稳定版的六平台发布清单不变，
手动 `targets` 仍可覆盖默认。原有运行中的 beta.4 和预热 run 使用其各自
已固定的 workflow，不随本次修改变化。

## 风险

首次预热仍是冷构建；如果发布在预热结束之前触发，本次 release 仍可能 miss。缓存受 GitHub 配额和有效期影响；构建行为绝不能依赖缓存存在。当前正在运行的 beta.4 不会因为修改 main 中的 workflow 而加速。
