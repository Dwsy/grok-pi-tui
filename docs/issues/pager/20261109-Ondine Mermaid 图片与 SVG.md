# Mermaid 图片使用 Ondine 并支持打开 SVG

## 目标

将 Mermaid 的图片操作切换到 Ondine；在 Open Image 右侧增加 Open SVG。
终端内的 ASCII Mermaid 渲染保持独立。PNG 和 SVG 都沿用按需渲染、
会话缓存与隔离子进程的保护。

## 方案与约束

- 使用固定版本的 Ondine Rust 编译器生成 SVG，再由现有 rasterizer 生成 PNG。
- 输出适合独立查看器与现有 rasterizer 的浅色或深色 SVG。
- 在缓存键中区分 PNG 和 SVG，避免旧图片缓存和并发操作互相覆盖。
- 用户明确要求不运行 Cargo；本次只做静态检查和差异审阅，构建与运行验证留待后续。

## 进度

- [x] 定位图片按钮、渲染工作线程、子进程和 Ondine 上游接口。
- [x] 完成渲染器、缓存和按钮接入；保留原有 ASCII 渲染。
- [x] 完成静态检查并审阅变更。已添加图片和 SVG 的后续回归测试。

## 验证与边界

- Rustfmt 语法/格式检查、TOML/JSON 解析、精确源码边界集合与锁文件结构检查、
  `git diff --check` 通过。
- 全量静态架构脚本在 55 秒时限内未完成，未得到全量结果。
- 按用户要求未运行 Cargo，尚未进行编译、测试、真实按钮操作或图片视觉验收。
- 上游 Ondine 要求 Rust 1.97；源码构建首次需要获取固定的 Git 依赖。
