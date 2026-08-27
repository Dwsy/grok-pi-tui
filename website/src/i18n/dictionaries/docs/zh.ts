import type { DocsDictionary } from "./en";

const docsZh: DocsDictionary = {
  home: {
    intro:
      "安装、配置并掌握 grok-pi 所需的一切——在 Grok Build 原生终端 UI 中运行 Pi 的 Agent 核心。文档跟踪",
    versionNote: "（工作流、隔离、自愈、导出）。",
    quickStart: "快速开始",
    cards: [
      {
        title: "安装",
        desc: "macOS、Linux 与 Windows 一行安装。",
        href: "/docs/installation/",
      },
      {
        title: "配置",
        desc: "环境变量、F2 开关、隔离目录与 CLI 参数。",
        href: "/docs/configuration/",
      },
      {
        title: "架构",
        desc: "三个边界，零 hack。桥接如何工作。",
        href: "/docs/architecture/",
      },
      {
        title: "命令",
        desc: "原生斜杠、Pi 会话操作、工作流、计划模式等。",
        href: "/docs/commands/",
      },
      {
        title: "功能",
        desc: "资源管理器、自愈、Bash、子代理、回顾。",
        href: "/docs/features/",
      },
      {
        title: "扩展",
        desc: "内置桥接 + 推荐 juicesharp todo/ask。",
        href: "/docs/extensions/",
      },
      {
        title: "迁移",
        desc: "从 stock Grok Build 或 interactive Pi 迁入。",
        href: "/docs/migration/",
      },
    ],
    labels: {
      install: "安装 grok-pi",
      ensurePi: "确保 Pi ≥ 0.84.3",
      run: "运行",
    },
  },

  installation: {
    introLead: "单一平台二进制。运行时依赖：Pi ≥",
    introMid: "。当前发布线：",
    introEnd: "。",
    sections: {
      macos: "macOS / Linux",
      windows: "Windows",
      prereq: "前置条件",
      firstRun: "首次运行",
      verify: "验证",
      updating: "更新",
      crash: "扩展导致启动崩溃时",
      source: "从源码构建",
    },
    macosBody:
      "自动检测平台，安装到 ~/.local/bin，并创建 pi-grok 别名符号链接。",
    installLabel: "一行安装",
    overrideInstall: "可用以下变量覆盖安装目录",
    powershellLabel: "PowerShell 安装",
    installPiLabel: "安装 Pi",
    prereqBody:
      "安装 Pi 包建议 Node.js ≥ 22.19.0。grok-pi 会启动 pi（可用 --pi-bin / PI_BIN 覆盖）。",
    newSession: "新会话",
    continueSession: "继续上次会话",
    resumePartial: "按部分 UUID 恢复",
    migrationHint: "从 stock Grok 或 interactive Pi 过来？见",
    migrationLink: "迁移",
    checkVersion: "查看版本",
    expectVersion: "期望 grok-pi 0.0.8 或更高。",
    updateCheck: "检查更新",
    updateInstall: "安装最新版",
    updateNote:
      "后台检查依次尝试 GitHub Releases、官方 scope npm 元数据和 JSP 代理。用 GROK_PI_NO_AUTO_UPDATE=1 关闭。",
    crashBody:
      "grok-pi 会自愈：对扩展列表二分搜索、打印罪魁、剔除后重启。手动逃生：",
    noExtLabel: "禁用扩展",
    sourceBody:
      "Rust 1.92.0（见 rust-toolchain.toml）、Node.js 22.19.0+、npm、系统 Pi。",
    buildLabel: "构建",
    runLocalLabel: "本地运行",
  },

  configuration: {
    intro:
      "环境变量、CLI 参数、F2 功能门控，以及产品隔离的状态目录。稳定桥接默认开启；实验表面需显式打开。",
    isolationTitle: "产品状态隔离",
    isolationLead: "grok-pi 默认不与 stock Grok 共享配置根目录：",
    table: {
      layer: "层级",
      stock: "stock Grok",
      grokPi: "grok-pi 默认",
      userHome: "用户主目录",
      projectTree: "项目目录",
    },
    isolationPi:
      "Pi agent 状态仍在 ~/.pi/agent（或 --session-dir）。不双扫 stock 树。迁移 UI 偏好：",
    migrateNote:
      "目标为空且存在 legacy 数据 → 一次性自动迁移，并写 .migrated-from-legacy 标记。工作流不会自动复制；请把 Rhai 脚本放到 ~/.grok-pi/workflows 或 <repo>/.grok-pi/workflows。",
    envTitle: "环境变量",
    envCols: { variable: "变量", default: "默认", purpose: "用途" },
    envVars: [
      {
        name: "GROK_HOME",
        default: "~/.grok-pi",
        desc: "用户状态根（与 stock Grok ~/.grok 隔离）",
      },
      {
        name: "GROK_PROJECT_DIR",
        default: ".grok-pi",
        desc: "仓库内项目配置/工作流/hooks 目录名",
      },
      {
        name: "GROK_LEGACY_HOME",
        default: "~/.grok",
        desc: "migrate-home / 自动迁移的源目录",
      },
      {
        name: "PI_GROK_REMOTE_TUI",
        default: "1",
        desc: "经 Grok Pager 承载 Pi ctx.ui.custom（Remote TUI）",
      },
      {
        name: "PI_GROK_BASH",
        default: "1",
        desc: "Grok 拥有的 Bash + 发送到后台",
      },
      {
        name: "PI_GROK_NATIVE_COMMANDS",
        default: "0",
        desc: "实验性 /pi-* 原生选择器",
      },
      {
        name: "PI_GROK_EXCLUDE_TOOLS",
        default: "unset",
        desc: "逗号分隔的要排除的内置工具",
      },
      {
        name: "GROK_PI_NO_AUTO_UPDATE",
        default: "unset",
        desc: "禁用后台 GitHub 更新检查",
      },
      {
        name: "GROK_PI_INSTALL_DIR",
        default: "~/.local/bin",
        desc: "install.sh 自定义安装路径",
      },
      { name: "PI_BIN", default: "pi", desc: "宿主使用的 Pi 可执行文件" },
      {
        name: "PI_CODING_AGENT_SESSION_DIR",
        default: "Pi default",
        desc: "覆盖 Pi 会话根（与 Pi 相同）",
      },
    ],
    flagsTitle: "CLI 参数与子命令",
    flagsLead: "宿主转发一等 Pi 参数。-- 之后的额外参数仍会透传。",
    passFlagsLabel: "透传 Pi 参数",
    flagCols: { flag: "参数", desc: "说明" },
    flags: [
      { flag: "--pi-cwd <path>", desc: "Pi 子进程的项目目录" },
      { flag: "--pi-bin <path>", desc: "Pi 可执行文件路径" },
      {
        flag: "--continue / -c",
        desc: "继续上一会话（跳过 Welcome）",
      },
      { flag: "--session <id>", desc: "恢复会话（支持部分 UUID）" },
      { flag: "--session-dir <path>", desc: "自定义 Pi 会话目录" },
      { flag: "--fork", desc: "启动时 fork 语义（Pi 参数）" },
      { flag: "--no-session", desc: "不打开/不持久化会话文件" },
      { flag: "--name <name>", desc: "命名会话" },
      { flag: "--provider / --model", desc: "Provider 与模型选择" },
      { flag: "--thinking <level>", desc: "思考力度" },
      {
        flag: "--system-prompt / --append-system-prompt",
        desc: "系统提示覆盖",
      },
      { flag: "--extension <path>", desc: "额外 Pi 扩展路径" },
      {
        flag: "--no-extensions / -ne",
        desc: "禁用注入桥接 + 用户扩展",
      },
      {
        flag: "--no-skills / --no-context-files",
        desc: "禁用 Pi skill / AGENTS 发现",
      },
      {
        flag: "--tools / --exclude-tools / --no-tools / -nt",
        desc: "工具允许/拒绝列表",
      },
      { flag: "--no-builtin-tools / -nbt", desc: "去掉 Pi 内置工具" },
      { flag: "--approve / --no-approve", desc: "信任 / 审批门控" },
      { flag: "--offline", desc: "在 Pi 支持处禁用网络" },
      {
        flag: "migrate-home",
        desc: "子命令：从 legacy ~/.grok 复制白名单文件",
      },
      { flag: "update [--check]", desc: "安装或检查 GitHub Releases" },
    ],
    f2Title: "F2 功能门控",
    f2Lead:
      "打开 F2（设置）。仅 Pi 的行带 external_only——不在 grok-pi / external 配置时隐藏。",
    f2Cols: { setting: "设置", default: "默认", notes: "说明" },
    f2Gates: [
      {
        key: "[ui].pi_workflows",
        def: "off",
        note: "Rhai 工作流（/workflow…）；需重启",
      },
      {
        key: "[ui].pi_goal",
        def: "off",
        note: "目标模式 MVP（/goal）；需重启",
      },
      {
        key: "[ui].pi_tree_file_rollback",
        def: "off",
        note: "SessionTree r/R 文件回滚；需重启",
      },
      {
        key: "recap_mermaid",
        def: "off",
        note: "在 /recap 正文中渲染 Mermaid",
      },
      {
        key: "remote_tui_footer",
        def: "varies",
        note: "Remote TUI 页脚实验表面",
      },
    ],
    resourcesTitle: "Pi 资源管理器",
    resourcesBody:
      "/pi-config（别名 /pi-resources）或 F2 → Pi resources 打开 Rust 原生双栏管理器：全局与受信任项目范围内的扩展、技能、提示词、主题。读取 Pi settings.json / trust.json。不执行 pi install/remove/update——包生命周期请用 Pi CLI，然后刷新或 /reload。",
    resourcesBullets: [
      "过滤：All / Enabled / Disabled；策略视图（a）；刷新（r）；Tab = 作用域",
      "来源身份：GitHub / npm / 本地路径（非泛标签）",
      "准入策略可屏蔽嘈杂来源（如自定义 header/footer、pi-tool-display）",
    ],
    selfHealTitle: "扩展自愈",
    selfHealBody:
      "若 Pi 扩展拖垮 RPC 启动，grok-pi 会对 --extension 路径做 VS Code 式二分搜索、点名罪魁并剔除后重启，避免卡死。逃生：grok-pi -ne。",
    themesTitle: "主题",
    themesBody:
      "/theme pi:<name> 将 Pi 主题 JSON 映射为 Grok Theme。终端透明内置：",
    themeDark: "pi:transparent — 暗色",
    themeLight: "pi:transparent-light — 亮色",
  },

  architecture: {
    intro:
      "组合，不是 fork。三个边界。桥接不修改 Pi 源码。adapter 保持无头。Grok Pager 是唯一 TUI。",
    layersTitle: "三层结构",
    layers: [
      {
        name: "Grok Pager",
        role: "终端生命周期、输入、渲染、对话框、scrollback",
        details: [
          "拥有终端——init、restore、alternate screen、minimal mode",
          "PromptWidget、斜杠补全、QuestionView、toast、diff",
          "原生 SessionPicker、模型选择器、SessionTree、任务面板",
          "F2 设置（Pi-only 门控为 external_only 行）",
          "启用 Pi 工作流时使用上游 workflow 引擎表面",
        ],
      },
      {
        name: "pi-grok-adapter",
        role: "无头 JSONL RPC ↔ ACP 桥接",
        details: [
          "无终端——无 Ratatui、Crossterm 或 raw-mode",
          "工具 / 流 / 队列 / 会话目录投影",
          "WorkflowHost、GoalHost、计划模式跟踪（adapter 状态）",
          "x.ai/* ACP：bash 后台、子代理、工作流、recap",
          "绝不发明第二套 TUI",
        ],
      },
      {
        name: "Pi Agent Core",
        role: "Agent 循环、模型、Provider、工具、扩展、会话",
        details: [
          "始终以 --mode rpc 启动（系统 pi ≥ 0.84.3）",
          "本地 JSONL 会话；信任、设置、包生命周期",
          "扩展生态 + skills + prompts",
          "子代理子 AgentSession；压缩；模型 Provider",
          "桥接不改源码——经扩展 API 注入",
        ],
      },
    ],
    runtimeTitle: "运行时设计（0.0.8）",
    runtime: [
      {
        title: "产品隔离",
        body: "默认目录：~/.grok-pi 与 <repo>/.grok-pi。不双扫 stock ~/.grok。Pi agent 状态仍在 ~/.pi/agent（或 --session-dir）。",
      },
      {
        title: "扩展自愈",
        body: "启动失败 → 二分 --extension 列表 → 点名罪魁 → 剔除后重启。逃生：grok-pi -ne。",
      },
      {
        title: "资源管理器（Rust）",
        body: "/pi-config 双栏 UI 读 Pi settings/trust；启动时准入策略。包安装/卸载仍走 Pi CLI。",
      },
      {
        title: "工作流与目标（可选）",
        body: "F2 pi_workflows / pi_goal 默认关；重启后注入扩展。脚本在 .grok-pi/workflows。",
      },
    ],
    fieldMap: "字段映射：",
    featureMatrix: "功能矩阵",
    extensions: "扩展",
    seamsTitle: "集成接缝",
    seamsLead:
      "ACP 无法覆盖全部 Pi UI/命令。窄 Pager 接缝（示意——请对照当前 source-identity 基线）：",
    seams: [
      {
        file: "UiProfile::External",
        desc: "关闭 Grok.com 产品表面；保留 Pager 渲染器",
      },
      {
        file: "AcpConnection::external",
        desc: "Pager 接受来自 adapter 的外部 ACP 通道",
      },
      {
        file: "run_external",
        desc: "生产级终端/事件循环，不启动 Grok Agent",
      },
      {
        file: "UI notification handlers",
        desc: "状态 → toast / banner / 标题 / 编辑器",
      },
      {
        file: "QuestionView + Remote TUI",
        desc: "原生自由文本 + 实验性 ctx.ui.custom 宿主",
      },
      {
        file: "Slash profile",
        desc: "保留的 Grok 命令 + Pi/动态 ACP 目录",
      },
      {
        file: "Plan / queue / tasks",
        desc: "原生计划切换、队列面板、后台任务卡片",
      },
      {
        file: "Session tree / jump",
        desc: "Pi navigateTree + 轮次跳转；非 Grok 破坏性 Rewind",
      },
      {
        file: "Voice dictation",
        desc: "窄范围 Pager 拥有的 /voice；不拥有 Pi 模型",
      },
    ],
    invariantsTitle: "不变量",
    invariants: [
      "Grok Pager 是唯一可见 TUI",
      "Pi 拥有 agent 循环、会话、模型、扩展",
      "adapter 无 Ratatui / Crossterm / raw-mode",
      "不为扩 RPC 而改 Pi 源码——优先扩展 API",
      "项目 trust 归 Pi；Grok 不重新裁决",
    ],
  },

  commands: {
    intro:
      "grok-pi 保留 Grok Pager 斜杠表面，并通过 ACP 命令目录映射 Pi 能力。来自 Pi 的扩展 / 提示词 / skill 命令会动态出现。",
    cols: { command: "命令", description: "说明" },
    retainedTitle: "保留的 Grok 原生",
    nativeCommands: [
      { cmd: "exit", desc: "退出 grok-pi" },
      { cmd: "help", desc: "显示可用命令" },
      {
        cmd: "hotkeys",
        desc: "快捷键模态（别名：shortcuts、keys；等同 Ctrl+.）",
      },
      { cmd: "new", desc: "开始新会话" },
      { cmd: "compact", desc: "压缩上下文；可附自定义指令" },
      { cmd: "model", desc: "打开模型选择器（Pi 目录）" },
      { cmd: "effort", desc: "设置思考力度" },
      { cmd: "rename", desc: "重命名当前会话" },
      { cmd: "resume", desc: "会话选择器（Pi JSONL 目录）" },
      {
        cmd: "session-info",
        desc: "会话统计 + 上下文快照（别名：session）",
      },
      { cmd: "dashboard", desc: "Agent 仪表盘（亦 Ctrl+\\）" },
      { cmd: "copy", desc: "复制最后一条回复到剪贴板" },
      { cmd: "find", desc: "在 scrollback 中搜索" },
      { cmd: "transcript", desc: "导出 transcript" },
      { cmd: "export", desc: "导出为 Markdown（Grok transcript）" },
      { cmd: "expand", desc: "展开折叠的工具输出" },
      { cmd: "queue", desc: "显示 steering / follow-up 队列" },
      { cmd: "notify", desc: "查看进程内通知" },
      { cmd: "multiline", desc: "切换多行输入" },
      { cmd: "compact-mode", desc: "切换紧凑显示" },
      { cmd: "vim-mode", desc: "切换 Vim 键位" },
      { cmd: "theme", desc: "切换主题（支持 pi:<name>）" },
      { cmd: "timestamps", desc: "切换消息时间戳" },
      { cmd: "toggle-mouse-reporting", desc: "切换鼠标支持" },
    ],
    sessionTitle: "会话与树",
    sessionCommands: [
      {
        cmd: "/jump",
        desc: "带时间线预览的轮次选择器；恢复视口",
      },
      {
        cmd: "/fork",
        desc: "从选定用户消息分支（Pi fork + rebind）",
      },
      {
        cmd: "/clone",
        desc: "将当前叶复制到新会话文件",
      },
      {
        cmd: "/reload",
        desc: "ctx.reload()；流式或压缩中阻塞；刷新目录 + Pi 主题",
      },
      {
        cmd: "/review-session",
        desc: "对会话文件编辑的原生 code-review 模态",
      },
      {
        cmd: "/review-message",
        desc: "轮次范围审查（jump 风格浮层）",
      },
      {
        cmd: "/export-html",
        desc: "将会话导出为 HTML（或传入 .jsonl 路径）",
      },
      { cmd: "/pi-share", desc: "私有 GitHub gist + pi.dev 查看器" },
      {
        cmd: "/context",
        desc: "实时 token 分解模态（system/tools/AGENTS/skills）",
      },
      {
        cmd: "/recap",
        desc: "会话回顾（别名 /summarize；可选焦点文本）。离开 ≥3 分钟自动",
      },
    ],
    treeNotes: [
      "空提示下双击 Esc 或 /rewind 打开 SessionTree（Pi navigateTree——非破坏性分支）。",
      "SessionTree：Enter 导航，过滤/搜索/标签；开启 F2 pi_tree_file_rollback 时：r = 预览，R = 执行文件回滚。",
    ],
    modeTitle: "计划、目标、工作流",
    modeLead:
      "目标与工作流为 F2 可选（默认关），需完全退出并重启，以便宿主在进程启动时注入扩展。",
    modeCommands: [
      {
        cmd: "/plan-mode",
        desc: "切换计划模式；快捷键：Ctrl+Shift+T（Windows：Ctrl+Alt+T）",
      },
      {
        cmd: "/view-plan",
        desc: "计划模式激活时打开会话 .plan.md",
      },
      {
        cmd: "/goal",
        desc: "目标模式 MVP（F2 pi_goal，需重启；默认关）",
      },
      {
        cmd: "/workflow",
        desc: "启动 Rhai 工作流（F2 pi_workflows，需重启；默认关）",
      },
      {
        cmd: "/workflows",
        desc: "列出可用工作流（用户 + 项目 .grok-pi/workflows）",
      },
      {
        cmd: "/create-workflow",
        desc: "用 Pager 提示脚手架工作流脚本",
      },
    ],
    resourceTitle: "资源、鉴权、语音",
    resourceCommands: [
      {
        cmd: "/pi-config",
        desc: "Rust 原生 Pi 资源管理器（别名 /pi-resources；亦 F2 → Pi resources）",
      },
      { cmd: "/login", desc: "Pi 鉴权（Remote TUI 路径）" },
      { cmd: "/logout", desc: "清除 Pi 凭据（Remote TUI 路径）" },
      { cmd: "/voice", desc: "xAI STT 语音听写（Ctrl+Space / F8）" },
    ],
    dynamicTitle: "动态 Pi 命令",
    dynamicBody:
      "Pi 返回的扩展、提示词与 skill 命令不在 Rust 中硬编码。它们经 ACP 进入 Grok 斜杠下拉；名称冲突由 Grok 注册表去重。仅桥接的名字如 __pi_workflow_* 会从目录中过滤。",
    excludedTitle: "有意排除",
    excludedLead:
      "不暴露 Grok 产品/云存储命令与启动期专用渲染模式（会话归 Pi；fullscreen/minimal 在启动时切换）：",
    excludedCommands: [
      "history",
      "usage",
      "plugins",
      "mcp",
      "memory",
      "workspace",
      "share",
      "debug",
      "minimal",
      "fullscreen",
    ],
    loginNote:
      "此处裸 /login / /logout 是 Pi 鉴权（Remote TUI），不是 Grok 云登录。",
  },

  features: {
    introLead: "字段级映射，版本",
    introSSot: "完整 SSOT：",
    statusLabel: "状态：",
    deepDives: "深入：",
    cols: { feature: "功能", status: "状态", notes: "说明" },
    status: {
      Native: "原生",
      Adapted: "已适配",
      "Native+Adapted": "原生+适配",
      Boundary: "边界",
      Experimental: "实验",
    },
    sections: [
      {
        title: "终端与显示",
        rows: [
          {
            feature: "终端 init/restore",
            status: "Native",
            notes: "Grok init_terminal / restore_terminal",
          },
          {
            feature: "Welcome / logo",
            status: "Native+Adapted",
            notes: "默认 Welcome；π 块字符；--continue 跳过 Welcome",
          },
          {
            feature: "更新检查/安装",
            status: "Adapted",
            notes: "GitHub Releases + 官方 npm 元数据 + JSP 代理回退；grok-pi update",
          },
          {
            feature: "主题 / 时间戳 / 鼠标",
            status: "Native+Adapted",
            notes: "Pi 主题 JSON → Grok Theme；pi:transparent*",
          },
          {
            feature: "语音听写",
            status: "Native+Adapted",
            notes: "/voice · Ctrl+Space/F8 · xAI STT",
          },
          {
            feature: "Markdown / 工具卡片 / diff",
            status: "Native+Adapted",
            notes: "ACP 分块 → 原生 Pager 表面",
          },
          {
            feature: "Todo / 计划列表",
            status: "Native+Adapted",
            notes: "内置 todo（F2 pi_todo，默认开）→ ACP Plan → TodoPane",
          },
          {
            feature: "计划模式",
            status: "Native+Adapted",
            notes: "/plan-mode · Ctrl+Shift+T（Windows：Ctrl+Alt+T）· 工具门控 · exit_plan_mode",
          },
          {
            feature: "目标模式",
            status: "Adapted",
            notes: "F2 pi_goal 默认关；/goal + agent_settled follow-up",
          },
        ],
      },
      {
        title: "Agent 与流式",
        rows: [
          {
            feature: "Prompt / steer / follow-up",
            status: "Adapted",
            notes: "ACP prompt；sendNow→steer；默认 mid-turn→followUp",
          },
          {
            feature: "中止 + 清队列",
            status: "Adapted",
            notes: "先 clear_queue 再 abort；队列经 x.ai/queue/changed 镜像",
          },
          {
            feature: "Bash + 发送到后台",
            status: "Native+Adapted",
            notes: "pi-grok-bash；x.ai/terminal/background；task kill",
          },
          {
            feature: "子代理",
            status: "Native+Adapted",
            notes: "pi-grok-subagents → SubagentBlock / 任务面板",
          },
          {
            feature: "Rhai 工作流",
            status: "Native+Adapted",
            notes: "F2 pi_workflows；/workflow /workflows /create-workflow",
          },
          {
            feature: "压缩",
            status: "Native+Adapted",
            notes: "/compact → Pi compact；原生进度块",
          },
          {
            feature: "会话回顾",
            status: "Adapted",
            notes: "/recap · 离开 ≥3 分钟自动 · 可选 Mermaid F2",
          },
          {
            feature: "上下文栏 + /context",
            status: "Native+Adapted",
            notes: "实时分解模态；不写入历史",
          },
        ],
      },
      {
        title: "模型、会话与命令",
        rows: [
          {
            feature: "模型目录",
            status: "Adapted",
            notes: "get_available_models → 原生选择器",
          },
          {
            feature: "Resume / 会话选择器",
            status: "Adapted",
            notes: "Pi JSONL 目录；Ctrl+F 全文搜索",
          },
          {
            feature: "会话树",
            status: "Adapted",
            notes: "navigateTree；非破坏性（≠ Grok Rewind）",
          },
          {
            feature: "Fork / clone",
            status: "Adapted",
            notes: "/fork /clone · rebind + session/load",
          },
          {
            feature: "Jump / review",
            status: "Native+Adapted",
            notes: "/jump 轮次；/review-session · /review-message",
          },
          {
            feature: "Reload",
            status: "Adapted",
            notes: "/reload 在流式+压缩中阻塞；主题 rediscover",
          },
          {
            feature: "HTML 导出 / 分享",
            status: "Adapted",
            notes: "/export-html · /pi-share（默认开）",
          },
          {
            feature: "Pi 资源管理器",
            status: "Native+Adapted",
            notes: "Rust /pi-config 双栏；不含 install/remove",
          },
        ],
      },
      {
        title: "可靠性与隔离",
        rows: [
          {
            feature: "产品主目录隔离",
            status: "Adapted",
            notes: "~/.grok-pi + <repo>/.grok-pi；migrate-home",
          },
          {
            feature: "扩展自愈",
            status: "Adapted",
            notes: "崩溃的 --extension 启动二分；-ne 逃生",
          },
          {
            feature: "资源准入策略",
            status: "Adapted",
            notes: "启动时允许/阻止列表 + 启发式",
          },
          {
            feature: "Remote TUI",
            status: "Experimental",
            notes: "PI_GROK_REMOTE_TUI=1；ctx.mode facade tui；不 fork Pi",
          },
        ],
      },
      {
        title: "边界（有意）",
        rows: [
          {
            feature: "Grok 云历史 / usage",
            status: "Boundary",
            notes: "会话归 Pi 本地",
          },
          {
            feature: "Adapter TUI / 第二渲染器",
            status: "Boundary",
            notes: "adapter 保持无头",
          },
          {
            feature: "改 Pi 源码扩 RPC",
            status: "Boundary",
            notes: "优先官方扩展 API",
          },
          {
            feature: "rpiv-ask 纯 JSONL",
            status: "Boundary",
            notes: "需要 Remote TUI 自定义宿主",
          },
          {
            feature: "Grok 破坏性 Rewind",
            status: "Boundary",
            notes: "改用 SessionTree 导航",
          },
        ],
      },
    ],
  },

  extensions: {
    intro:
      "grok-pi 向 Pi 注入薄桥接扩展，让原生 Grok 表面拥有 Bash、子代理、上下文、回顾等——无需 fork Pi。之上可像 interactive Pi 一样安装社区包。",
    recommendedTitle: "推荐社区扩展",
    recommendedLead:
      "这些不内置。安装一次到 Pi agent 主目录；pi 与 grok-pi 都会加载。",
    installLabel: "安装",
    quickInstallLabel: "快速安装",
    recommended: [
      {
        pkg: "npm:@juicesharp/rpiv-ask-user-question",
        title: "结构化问题 → QuestionView",
        status: "推荐 · Remote TUI 路径",
        body: "允许 Agent 在回合中提出多选项/多选问题。interactive Pi 可开自定义 factory UI；在 grok-pi 下稳定路径是 Remote TUI（默认开），在进程内运行 factory 并在桥接可宿主时投影到原生 Grok QuestionView 风格表面。",
        install: "pi install npm:@juicesharp/rpiv-ask-user-question",
        notes: [
          "保持 PI_GROK_REMOTE_TUI=1（默认）以便第三方自定义 UI 运行。",
          "无自定义宿主的纯 JSONL RPC 无法序列化 factory 组件——这正是 Remote TUI 存在的原因。",
          "若问卷失败，检查 Remote TUI 已启用且扩展已加载（未被策略屏蔽）。",
        ],
      },
    ],
    bashTitle: "内置：增强 Bash",
    bashLead:
      "拥有每一个 Bash 子进程，使 Pager 能把前台命令提升为 Grok 原生后台任务 UI，而无需重跑。",
    bashPoints: [
      "前台 bash 复用 Pi createBashToolDefinition 的输出/渲染语义。",
      "Pager「发送到后台」经 x.ai/terminal/background（toolCallId 控制文件）转移同一子进程。",
      "原生任务卡片：经 x.ai/task/kill 终止；Agent 工具 get_task_output / wait_tasks / kill_task 仍可用。",
      "支持 is_background + description（模型启动的后台 shell）。",
    ],
    bashGate:
      "PI_GROK_BASH=1（默认）。用 PI_GROK_BASH=0 或 --no-extensions 关闭。",
    bashTuiTitle: "TUI 中你看到的",
    bashTuiSteps: [
      "Agent 运行长 bash（构建、测试、安装…）。",
      "在工具卡片上使用 Pager 原生「发送到后台」——进程继续跑；卡片变为任务行。",
      "从任务 UI 或 Agent 工具（get_task_output、wait_tasks、kill_task）终止、等待或轮询。",
    ],
    subagentsTitle: "内置：子代理",
    subagentsLead:
      "生成真实的 Pi 子 AgentSession，并将生命周期投影到原生 SubagentBlock、任务面板与子 AgentView。",
    subagentsPoints: [
      "配置：general-purpose（全部工具）、explore / plan（更安全工具集）。",
      "能力模式：read-only、read-write、execute、all。",
      "前台或后台（后台最大并发 4）。",
      "版本化桥接 pi-grok-subagent/v1 → adapter → x.ai/subagent/* 表面；取消为一等能力。",
      "子会话会持久化；父会话 resume 可从索引条目重建生命周期。",
    ],
    subagentsGate:
      "与其他桥接扩展一起注入。--no-extensions / -ne 时禁用。",
    profileCols: {
      profile: "配置",
      tools: "工具",
      useWhen: "适用场景",
    },
    profiles: [
      {
        profile: "general-purpose",
        tools: "read, bash, edit, write",
        useWhen: "委托实现切片",
      },
      {
        profile: "explore",
        tools: "read, bash",
        useWhen: "代码库调查、诊断",
      },
      {
        profile: "plan",
        tools: "read, bash",
        useWhen: "仅含风险与验证的计划",
      },
    ],
    catalogTitle: "完整桥接目录",
    catalogLead:
      "源码在仓库 extensions/。宿主在 spawn 时注入；不是 Pi 核心补丁。",
    catalogCols: {
      extension: "扩展",
      role: "职责",
      default: "默认",
    },
    catalog: [
      {
        name: "pi-grok-bash",
        role: "Bash 所有权 + 后台提升",
        default: "开",
      },
      {
        name: "pi-grok-subagents",
        role: "子 AgentSession + 原生任务 UI",
        default: "开",
      },
      {
        name: "pi-grok-context",
        role: "/context 的 system/tools/AGENTS/skills 分解",
        default: "开",
      },
      {
        name: "pi-grok-recap",
        role: "仅展示的会话回顾（不改历史）",
        default: "开",
      },
      {
        name: "pi-grok-auth",
        role: "经 Remote TUI 的 /login /logout",
        default: "开",
      },
      {
        name: "pi-grok-export",
        role: "/export-html 与 /pi-share（gist）",
        default: "开",
      },
      {
        name: "pi-grok-remote-tui",
        role: "ctx.ui.custom 宿主 + 帧投影",
        default: "开*",
      },
      {
        name: "pi-grok-rpc-compat",
        role: "向第三方扩展呈现 mode=tui",
        default: "随 Remote TUI",
      },
      {
        name: "pi-grok-plan-mode",
        role: "计划门控 + exit_plan_mode 审批",
        default: "开",
      },
      {
        name: "pi-grok-goal",
        role: "/goal + update_goal（F2，需重启）",
        default: "关（F2）",
      },
      {
        name: "pi-grok-rollback",
        role: "树文件回滚快照",
        default: "关（F2）",
      },
      {
        name: "pi-grok-tools",
        role: "F2 内置工具允许/拒绝偏好",
        default: "开",
      },
      {
        name: "pi-grok-workflows",
        role: "Rhai 工作流的 Pi spawn 后端",
        default: "关（F2）",
      },
      {
        name: "pi-grok-native-commands",
        role: "实验性 /pi-* 选择器",
        default: "关（env）",
      },
    ],
    catalogFoot:
      "* Remote TUI 默认开；设 PI_GROK_REMOTE_TUI=0 关闭。F2 门控功能切换后需完整进程重启。",
    advancedTitle: "回顾、工作流、自愈",
    advanced: [
      {
        title: "回顾（/recap）",
        body: "pi-grok-recap 仅展示——不改写会话历史。离开 ≥3 分钟（且 ≥3 轮）自动。可选 Mermaid（F2 recap_mermaid）。在 F2 配置 recap_model（绝不静默回落到当前会话模型）。",
      },
      {
        title: "Rhai 工作流",
        body: "F2 → Pi workflows（默认关，需重启）。脚本在 ~/.grok-pi/workflows 与 <repo>/.grok-pi/workflows。斜杠：/workflow、/workflows、/create-workflow。宿主用上游 xai-workflow + Pi spawn 后端；__pi_workflow_* 桥接命令从目录隐藏。",
      },
      {
        title: "坏扩展自愈",
        body: "若任一注入或发现的扩展拖垮 RPC 启动，宿主对 --extension 路径二分搜索、打印罪魁并剔除后重启。手动：grok-pi -ne。",
      },
    ],
    controlTitle: "启用 / 禁用",
    controlGatesLabel: "开关",
    controlNote:
      "用户/项目扩展仍经 Pi 发现加载（~/.pi/agent、受信任项目树），除非被 /pi-config 策略屏蔽。",
    faqTitle: "常见问题",
    faqs: [
      {
        q: "需要自己安装 pi-grok-bash 吗？",
        a: "不需要。组合二进制会在运行时注入；内置 Todo 也默认注入。只安装你仍需要的社区包，例如 ask-user-question。",
      },
      {
        q: "宿主已有计划模式，为何还需要 Todo？",
        a: "计划模式是写门控 + 审批流；内置 Todo 是投影到 TodoPane 的实时工作清单——两者互补。F2 pi_todo 默认开启；开启时 grok-pi 会屏蔽 rpiv-todo，避免重复注册同名工具。如确实要改用社区 provider，请先关闭 pi_todo 并重启。",
      },
      {
        q: "没有 Remote TUI，ask-user-question 能用吗？",
        a: "不可靠。该包依赖进程内自定义 UI。问卷请保持 Remote TUI 开启（默认）。",
      },
      {
        q: "还能用任意 Pi 扩展吗？",
        a: "可以。用 pi install … 或放到 Pi 扩展路径；在 F2 / /pi-config 管理可见性。",
      },
    ],
  },

  migration: {
    intro:
      "两条入口，同一个宿主：保留 Grok 肌肉记忆，或保留 Pi 会话——grok-pi 夹在 Grok Pager 与 Pi agent 核心之间。",
    fromGrokTitle: "从 stock Grok Build",
    fromGrokLead: "同一终端、同一快捷键——Agent 运行时换成 Pi。",
    grokSteps: [
      {
        step: "1",
        title: "安装 grok-pi",
        desc: "一行命令。安装器检测平台并把二进制装到 ~/.local/bin。",
        code: "curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh",
      },
      {
        step: "2",
        title: "确保 Pi ≥ 0.84.3",
        desc: "grok-pi 以 Pi 为 agent 核心。用 npm 安装或更新 Pi。",
        code: "npm install --global @earendil-works/pi-coding-agent",
      },
      {
        step: "3",
        title: "在项目中运行",
        desc: "cd 进项目即可。Grok Pager 键位与主题仍有效；Agent 能力来自 Pi。",
        code: "cd your-project && grok-pi",
      },
    ],
    fromPiTitle: "从 interactive Pi",
    fromPiLead: "保留 Pi 会话、模型与扩展。前端换成 Grok Pager。",
    piSteps: [
      {
        step: "1",
        title: "安装 grok-pi（保留 Pi）",
        desc: "你已有 Pi。只需加宿主二进制——会话、模型与 ~/.pi/agent 原位不动。",
        code: "curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh",
      },
      {
        step: "2",
        title: "用 grok-pi 代替 pi",
        desc: "同一项目 cwd。同一 Pi 会话存储。前面换成 Grok Pager，背后仍是同一 agent 核心。",
        code: "cd your-project && grok-pi",
      },
      {
        step: "3",
        title: "恢复已有会话",
        desc: "部分 UUID 可用，与 Pi --session 相同。或在 TUI 内用 /resume。",
        code: "grok-pi --session 019f88c\ngrok-pi --continue",
      },
    ],
    keepTitle: "你保留什么",
    piKeep: [
      {
        title: "会话仍归 Pi",
        desc: "JSONL 在 ~/.pi/agent/sessions（或 --session-dir / PI_CODING_AGENT_SESSION_DIR）。无需导入。/resume 列出同一目录。",
      },
      {
        title: "模型、工具、扩展",
        desc: "Pi providers、工具、skills、prompts 与扩展仍从 Pi 路径加载。grok-pi 仅为 Pager 表面注入桥接扩展。",
      },
      {
        title: "设置仍然生效",
        desc: "Pi settings.json 与 auth 仍是 agent 权威。UI 外壳（F2、主题显示）在 ~/.grok-pi——与 stock Grok 隔离。",
      },
      {
        title: "退出 → 恢复命令",
        desc: "退出时提示：To resume this session: grok-pi --session <uuid>——与 interactive pi 同思路，主机二进制名替换。",
      },
    ],
    changesTitle: "什么变了",
    piDiffs: [
      {
        title: "TUI 是 Grok Pager",
        desc: "没有 Pi interactive TUI。斜杠补全、工具卡片、diff 与模态是映射到 Pi RPC 的原生 Grok 表面。",
      },
      {
        title: "部分仅 Pi 的选择器会变",
        desc: "/model 与 /resume 用 Grok SessionPicker / 模型 UI，不是 Pi 的 TUI 组件。行为等价；布局可能不同。",
      },
      {
        title: "产品状态目录",
        desc: "UI 偏好/工作流默认 ~/.grok-pi 与 <repo>/.grok-pi，永不与 stock Grok ~/.grok 碰撞。Pi agent 状态不变。",
      },
      {
        title: "始终 RPC 模式",
        desc: "grok-pi 始终以 --mode rpc 启动 Pi。不要期望需要进程内 TUI factory 的仅 interactive Pi 小部件。",
      },
    ],
    migrateHomeTitle: "UI 主目录：stock Grok → ~/.grok-pi",
    migrateHomeLead:
      "从 0.0.8 起，用户 chrome 默认 ~/.grok-pi，永不与 stock Grok 碰撞。Pi 会话仍在 ~/.pi/agent。可选一次性复制白名单文件：",
    migrateHomeNote:
      "目标为空且存在 legacy 数据时可能自动迁移一次。工作流不复制；开启 F2 Pi workflows 后把 Rhai 脚本放到 ~/.grok-pi/workflows 或 <repo>/.grok-pi/workflows。",
    whyTitle: "为何切换？",
    advantages: [
      {
        title: "保留 Grok 肌肉记忆",
        desc: "同一 Pager、同一斜杠命令、同一 Ctrl+key。grok-pi 加能力，不加复杂度。",
      },
      {
        title: "解锁任意模型",
        desc: "/model 打开 Pi 完整目录——GPT-4o、Claude、Gemini、本地 LLM、自定义端点。会话中途可切换。",
      },
      {
        title: "拥有自己的会话",
        desc: "本地 JSONL。Fork、clone、标签、回顾。无云依赖。",
      },
      {
        title: "完整扩展生态",
        desc: "Pi 扩展、skills 与 prompts 以原生 Grok 斜杠命令出现。",
      },
      {
        title: "子代理与并行工作",
        desc: "Pi 子代理投影到原生 SubagentBlock、任务面板与子 AgentView。",
      },
      {
        title: "上下文可见性",
        desc: "点击上下文栏或运行 /context 查看 token 实时分解。",
      },
    ],
    faqTitle: "常见问题",
    faqs: [
      {
        q: "会丢掉 Grok Build 功能吗？",
        a: "grok-pi 保留 Grok Pager 渲染、输入与导航。Grok 仅产品表面（云历史、usage、plugins）由 Pi 等价物替代——本地会话、扩展、完整模型访问。",
      },
      {
        q: "还能继续用 interactive pi 吗？",
        a: "可以。grok-pi 是独立二进制。用 pi 跑 Pi 的 TUI，用 grok-pi 跑 Grok Pager + Pi 核心。同一 session dir 时会话共享。",
      },
      {
        q: "能回到 stock Grok Build 吗？",
        a: "可以。stock grok 未改动。用 grok 跑原产品；用 grok-pi 跑桥接宿主。",
      },
      {
        q: "grok-pi 会改 Pi 或 Grok 源码吗？",
        a: "不会。桥接不修改 Pi 源码。Grok 渲染器身份受保护；adapter 是无头 JSONL RPC ↔ ACP 桥。",
      },
      {
        q: "已有 Pi 会话怎么办？",
        a: "原样可用。grok-pi 直接读 Pi JSONL。/resume 显示目录；--session <uuid> 打开指定会话。",
      },
    ],
  },
};

export default docsZh;
