/** English copy for /docs/* page bodies (sidebar labels stay in root dict.docs). */

export const docsEn = {
  home: {
    intro:
      "Everything you need to install, configure, and master grok-pi — Pi's agent core inside Grok Build's native terminal UI. Docs track",
    versionNote: "(workflows, isolation, self-heal, export).",
    quickStart: "Quick start",
    cards: [
      {
        title: "Installation",
        desc: "One-line install for macOS, Linux, and Windows.",
        href: "/docs/installation/",
      },
      {
        title: "Configuration",
        desc: "Env vars, F2 gates, isolation homes, and CLI flags.",
        href: "/docs/configuration/",
      },
      {
        title: "Architecture",
        desc: "Three boundaries, zero hacks. How the bridge works.",
        href: "/docs/architecture/",
      },
      {
        title: "Commands",
        desc: "Native slash, Pi session ops, workflows, plan, and more.",
        href: "/docs/commands/",
      },
      {
        title: "Features",
        desc: "Resource manager, self-heal, bash, sub-agents, recap.",
        href: "/docs/features/",
      },
      {
        title: "Extensions",
        desc: "Bundled bridges + recommended juicesharp todo/ask.",
        href: "/docs/extensions/",
      },
      {
        title: "Migration",
        desc: "From stock Grok Build or from interactive Pi.",
        href: "/docs/migration/",
      },
    ],
    labels: {
      install: "Install grok-pi",
      ensurePi: "Ensure Pi ≥ 0.84.3",
      run: "Run",
    },
  },

  installation: {
    introLead: "Single platform binary. Runtime dependency: Pi ≥",
    introMid: ". Current release line:",
    introEnd: ".",
    sections: {
      macos: "macOS / Linux",
      windows: "Windows",
      prereq: "Prerequisites",
      firstRun: "First run",
      verify: "Verify",
      updating: "Updating",
      crash: "When an extension crashes startup",
      source: "Build from source",
    },
    macosBody:
      "Detects platform, installs to ~/.local/bin, and creates a pi-grok alias symlink.",
    installLabel: "One-line install",
    overrideInstall: "Override install dir with",
    powershellLabel: "PowerShell install",
    installPiLabel: "Install Pi",
    prereqBody:
      "Node.js ≥ 22.19.0 recommended for Pi package installs. grok-pi spawns pi (override with --pi-bin / PI_BIN).",
    newSession: "New session",
    continueSession: "Continue last session",
    resumePartial: "Resume by partial UUID",
    migrationHint: "Coming from stock Grok or interactive Pi? See",
    migrationLink: "Migration",
    checkVersion: "Check version",
    expectVersion: "Expect grok-pi 0.0.8 or newer.",
    updateCheck: "Check",
    updateInstall: "Install latest",
    updateNote:
      "Background checks try GitHub Releases, then the official scoped npm metadata, then the JSP proxy. Disable with GROK_PI_NO_AUTO_UPDATE=1.",
    crashBody:
      "grok-pi self-heals: binary-search the extension list, print the culprit, relaunch without it. Manual escape:",
    noExtLabel: "No extensions",
    sourceBody:
      "Rust 1.92.0 (see rust-toolchain.toml), Node.js 22.19.0+, npm, system Pi.",
    buildLabel: "Build",
    runLocalLabel: "Run locally",
  },

  configuration: {
    intro:
      "Env vars, CLI flags, F2 feature gates, and product-isolated state trees. Stable bridges default on; experimental surfaces are opt-in.",
    isolationTitle: "Product state isolation",
    isolationLead:
      "grok-pi does not share stock Grok's config roots by default:",
    table: {
      layer: "Layer",
      stock: "stock Grok",
      grokPi: "grok-pi default",
      userHome: "User home",
      projectTree: "Project tree",
    },
    isolationPi:
      "Pi agent state stays under ~/.pi/agent (or --session-dir). No dual-scan of stock trees. Migrate UI prefs with:",
    migrateNote:
      "Empty target + legacy data → one-shot auto-migrate with .migrated-from-legacy marker. Workflows are not auto-copied; place Rhai scripts under ~/.grok-pi/workflows or <repo>/.grok-pi/workflows.",
    envTitle: "Environment variables",
    envCols: { variable: "Variable", default: "Default", purpose: "Purpose" },
    envVars: [
      {
        name: "GROK_HOME",
        default: "~/.grok-pi",
        desc: "User state root (isolated from stock Grok ~/.grok)",
      },
      {
        name: "GROK_PROJECT_DIR",
        default: ".grok-pi",
        desc: "Project config/workflows/hooks dirname under the repo",
      },
      {
        name: "GROK_LEGACY_HOME",
        default: "~/.grok",
        desc: "Source tree for migrate-home / auto-migrate",
      },
      {
        name: "PI_GROK_REMOTE_TUI",
        default: "1",
        desc: "Host Pi ctx.ui.custom through Grok Pager (Remote TUI)",
      },
      {
        name: "PI_GROK_BASH",
        default: "1",
        desc: "Grok-owned Bash + Send to Background",
      },
      {
        name: "PI_GROK_NATIVE_COMMANDS",
        default: "0",
        desc: "Experimental /pi-* native selectors",
      },
      {
        name: "PI_GROK_EXCLUDE_TOOLS",
        default: "unset",
        desc: "Comma-separated built-in tools to exclude",
      },
      {
        name: "GROK_PI_NO_AUTO_UPDATE",
        default: "unset",
        desc: "Disable background GitHub update checks",
      },
      {
        name: "GROK_PI_INSTALL_DIR",
        default: "~/.local/bin",
        desc: "Custom install path for install.sh",
      },
      { name: "PI_BIN", default: "pi", desc: "Pi binary used by the host" },
      {
        name: "PI_CODING_AGENT_SESSION_DIR",
        default: "Pi default",
        desc: "Override Pi session root (same as Pi)",
      },
    ],
    flagsTitle: "CLI flags & subcommands",
    flagsLead:
      "First-class Pi flags are forwarded by the host. Extra args after -- still pass through.",
    passFlagsLabel: "Pass Pi flags",
    flagCols: { flag: "Flag", desc: "Description" },
    flags: [
      { flag: "--pi-cwd <path>", desc: "Project directory for the Pi child" },
      { flag: "--pi-bin <path>", desc: "Pi executable path" },
      {
        flag: "--continue / -c",
        desc: "Continue the previous session (skips Welcome)",
      },
      { flag: "--session <id>", desc: "Resume session (partial UUID OK)" },
      { flag: "--session-dir <path>", desc: "Custom Pi session directory" },
      { flag: "--fork", desc: "Fork semantics at startup (Pi flag)" },
      { flag: "--no-session", desc: "Do not open/persist a session file" },
      { flag: "--name <name>", desc: "Name the session" },
      { flag: "--provider / --model", desc: "Provider and model selection" },
      { flag: "--thinking <level>", desc: "Thinking effort" },
      {
        flag: "--system-prompt / --append-system-prompt",
        desc: "Prompt overrides",
      },
      { flag: "--extension <path>", desc: "Extra Pi extension paths" },
      {
        flag: "--no-extensions / -ne",
        desc: "Disable injected bridge + user extensions",
      },
      {
        flag: "--no-skills / --no-context-files",
        desc: "Disable Pi skill / AGENTS discovery",
      },
      {
        flag: "--tools / --exclude-tools / --no-tools / -nt",
        desc: "Tool allow/deny lists",
      },
      { flag: "--no-builtin-tools / -nbt", desc: "Drop Pi built-in tools" },
      { flag: "--approve / --no-approve", desc: "Trust / approval gates" },
      { flag: "--offline", desc: "Disable network where Pi honors it" },
      {
        flag: "migrate-home",
        desc: "CLI subcommand: copy allowlisted files from legacy ~/.grok",
      },
      { flag: "update [--check]", desc: "Install or check GitHub Releases" },
    ],
    f2Title: "F2 feature gates",
    f2Lead:
      "Open F2 (settings). Pi-only rows are external_only — hidden unless you are on the grok-pi / external profile.",
    f2Cols: { setting: "Setting", default: "Default", notes: "Notes" },
    f2Gates: [
      {
        key: "[ui].pi_workflows",
        def: "off",
        note: "Rhai workflows (/workflow…); restart required",
      },
      {
        key: "[ui].pi_goal",
        def: "off",
        note: "Goal mode MVP (/goal); restart required",
      },
      {
        key: "[ui].pi_tree_file_rollback",
        def: "off",
        note: "SessionTree r/R file rollback; restart",
      },
      {
        key: "recap_mermaid",
        def: "off",
        note: "Render Mermaid in /recap bodies",
      },
      {
        key: "remote_tui_footer",
        def: "varies",
        note: "Remote TUI footer lab surface",
      },
    ],
    resourcesTitle: "Pi resource manager",
    resourcesBody:
      "/pi-config (alias /pi-resources) or F2 → Pi resources opens a Rust-native two-pane manager: extensions, skills, prompts, themes across global and trusted-project scopes. Reads Pi settings.json / trust.json. Does not run pi install/remove/update — use the Pi CLI for package lifecycle, then refresh or /reload.",
    resourcesBullets: [
      "Filters: All / Enabled / Disabled; policy view (a); refresh (r); Tab = scope",
      "Source identities: GitHub / npm / local paths (not generic tags)",
      "Admission policy can block noisy sources (e.g. custom header/footer, pi-tool-display)",
    ],
    selfHealTitle: "Extension self-heal",
    selfHealBody:
      "If a Pi extension crashes RPC bootstrap, grok-pi runs a VS Code-style binary search over --extension paths, names the culprit, and relaunches without it so you are not stuck. Escape hatch: grok-pi -ne.",
    themesTitle: "Themes",
    themesBody:
      "/theme pi:<name> maps Pi theme JSON into Grok Theme. Built-ins for terminal opacity:",
    themeDark: "pi:transparent — dark",
    themeLight: "pi:transparent-light — light",
  },

  architecture: {
    intro:
      "Composition, not a fork. Three boundaries. Pi source is not modified for the bridge. The adapter stays headless. Grok Pager is the only TUI.",
    layersTitle: "The three layers",
    layers: [
      {
        name: "Grok Pager",
        role: "Terminal lifecycle, input, rendering, dialogs, scrollback",
        details: [
          "Owns the terminal — init, restore, alternate screen, minimal mode",
          "PromptWidget, slash completion, QuestionView, toasts, diffs",
          "Native SessionPicker, model selector, SessionTree, Tasks Pane",
          "F2 settings (external_only rows for Pi-only gates)",
          "Upstream workflow engine surfaces when Pi workflows are enabled",
        ],
      },
      {
        name: "pi-grok-adapter",
        role: "Headless JSONL RPC ↔ ACP bridge",
        details: [
          "No terminal — no Ratatui, Crossterm, or raw-mode",
          "Tool / stream / queue / session catalog projection",
          "WorkflowHost, GoalHost, plan-mode tracker (adapter-owned state)",
          "x.ai/* ACP methods for bash background, subagent, workflow, recap",
          "Never invents a second TUI",
        ],
      },
      {
        name: "Pi Agent Core",
        role: "Agent loop, models, providers, tools, extensions, sessions",
        details: [
          "Always started in --mode rpc (system pi ≥ 0.84.3)",
          "Local JSONL sessions; trust, settings, package lifecycle",
          "Extension ecosystem + skills + prompts",
          "Sub-agent child AgentSession; compaction; model providers",
          "Source not modified for the bridge — inject via extension API",
        ],
      },
    ],
    runtimeTitle: "Runtime design (0.0.8)",
    runtime: [
      {
        title: "Product isolation",
        body: "Default homes: ~/.grok-pi and <repo>/.grok-pi. No dual-scan of stock ~/.grok. Pi agent state remains under ~/.pi/agent (or --session-dir).",
      },
      {
        title: "Extension self-heal",
        body: "Bootstrap failure → binary-search --extension list → name culprit → relaunch without it. Escape: grok-pi -ne.",
      },
      {
        title: "Resource manager (Rust)",
        body: "/pi-config two-pane UI reads Pi settings/trust; admission policy at spawn. Package install/remove stays on the Pi CLI.",
      },
      {
        title: "Workflows & goal (opt-in)",
        body: "F2 pi_workflows / pi_goal default off; restart injects extensions. Scripts under .grok-pi/workflows.",
      },
    ],
    fieldMap: "Field map:",
    featureMatrix: "Feature matrix",
    extensions: "Extensions",
    seamsTitle: "Integration seams",
    seamsLead:
      "ACP does not cover every Pi UI/command. Narrow Pager seams (illustrative — verify against current source-identity baseline):",
    seams: [
      {
        file: "UiProfile::External",
        desc: "Disables Grok.com product surfaces; keeps Pager renderer",
      },
      {
        file: "AcpConnection::external",
        desc: "Pager accepts an external ACP channel from the adapter",
      },
      {
        file: "run_external",
        desc: "Production terminal/event-loop without Grok Agent startup",
      },
      {
        file: "UI notification handlers",
        desc: "Status → toast / banner / title / editor",
      },
      {
        file: "QuestionView + Remote TUI",
        desc: "Native freeform + experimental ctx.ui.custom host",
      },
      {
        file: "Slash profile",
        desc: "Retained Grok commands + ACP catalog for Pi/dynamic",
      },
      {
        file: "Plan / queue / tasks",
        desc: "Native plan toggle, queue pane, background task cards",
      },
      {
        file: "Session tree / jump",
        desc: "Pi navigateTree + turn jump; not Grok destructive Rewind",
      },
      {
        file: "Voice dictation",
        desc: "Narrow Pager-owned /voice; does not own Pi models",
      },
    ],
    invariantsTitle: "Invariants",
    invariants: [
      "Grok Pager is the only visible TUI",
      "Pi owns agent loop, sessions, models, extensions",
      "adapter has no Ratatui / Crossterm / raw-mode",
      "do not patch Pi source to extend RPC — extension API first",
      "project trust is Pi-owned; Grok does not re-adjudicate",
    ],
  },

  commands: {
    intro:
      "grok-pi keeps Grok Pager slash surfaces and maps Pi capabilities through the ACP command catalog. Extension / prompt / skill commands from Pi appear dynamically.",
    cols: { command: "Command", description: "Description" },
    retainedTitle: "Retained Grok native",
    nativeCommands: [
      { cmd: "exit", desc: "Exit grok-pi" },
      { cmd: "help", desc: "Show available commands" },
      {
        cmd: "hotkeys",
        desc: "Keyboard shortcuts modal (aliases: shortcuts, keys; same as Ctrl+.)",
      },
      { cmd: "new", desc: "Start a new session" },
      { cmd: "compact", desc: "Compact context; optional custom instructions" },
      { cmd: "model", desc: "Open model selector (Pi catalog)" },
      { cmd: "effort", desc: "Set thinking effort level" },
      { cmd: "rename", desc: "Rename the current session" },
      { cmd: "resume", desc: "Session picker (Pi JSONL catalog)" },
      {
        cmd: "session-info",
        desc: "Session stats + context snapshot (alias: session)",
      },
      { cmd: "dashboard", desc: "Agent dashboard (also Ctrl+\\)" },
      { cmd: "copy", desc: "Copy last response to clipboard" },
      { cmd: "find", desc: "Search in scrollback" },
      { cmd: "transcript", desc: "Export transcript" },
      { cmd: "export", desc: "Export as Markdown (Grok transcript)" },
      { cmd: "expand", desc: "Expand collapsed tool output" },
      { cmd: "queue", desc: "Show steering / follow-up queue" },
      { cmd: "notify", desc: "View in-process notifications" },
      { cmd: "multiline", desc: "Toggle multiline input" },
      { cmd: "compact-mode", desc: "Toggle compact display" },
      { cmd: "vim-mode", desc: "Toggle Vim keybindings" },
      { cmd: "theme", desc: "Switch theme (supports pi:<name>)" },
      { cmd: "timestamps", desc: "Toggle message timestamps" },
      { cmd: "toggle-mouse-reporting", desc: "Toggle mouse support" },
    ],
    sessionTitle: "Session & tree",
    sessionCommands: [
      {
        cmd: "/jump",
        desc: "Turn picker with timeline previews; restore viewport",
      },
      {
        cmd: "/fork",
        desc: "Branch from a chosen user message (Pi fork + rebind)",
      },
      {
        cmd: "/clone",
        desc: "Duplicate current leaf into a new session file",
      },
      {
        cmd: "/reload",
        desc: "ctx.reload(); blocked while streaming or compacting; refreshes catalogs + Pi themes",
      },
      {
        cmd: "/review-session",
        desc: "Native code-review modal over session file edits",
      },
      {
        cmd: "/review-message",
        desc: "Turn-scoped review via jump-style overlay",
      },
      {
        cmd: "/export-html",
        desc: "Export session as HTML (or pass a .jsonl path)",
      },
      { cmd: "/pi-share", desc: "Private GitHub gist + pi.dev viewer" },
      {
        cmd: "/context",
        desc: "Live token breakdown modal (system/tools/AGENTS/skills)",
      },
      {
        cmd: "/recap",
        desc: "Session recap (alias /summarize; optional focus text). Auto when away ≥3 min",
      },
    ],
    treeNotes: [
      "Double-Esc (empty prompt) or /rewind opens SessionTree (Pi navigateTree — non-destructive branches).",
      "SessionTree: Enter navigate, filter/search/tags; with F2 pi_tree_file_rollback: r = preview, R = execute file rollback.",
    ],
    modeTitle: "Plan, goal, workflows",
    modeLead:
      "Goal and workflows are F2 opt-in (default off) and require a full quit + restart so the host can inject extensions at process start.",
    modeCommands: [
      {
        cmd: "Ctrl+Shift+T",
        desc: "Toggle plan mode (write gate + exit_plan_mode approval)",
      },
      {
        cmd: "/view-plan",
        desc: "Open session .plan.md when plan mode is active",
      },
      {
        cmd: "/goal",
        desc: "Goal mode MVP (F2 pi_goal, restart required; default off)",
      },
      {
        cmd: "/workflow",
        desc: "Launch a Rhai workflow (F2 pi_workflows, restart; default off)",
      },
      {
        cmd: "/workflows",
        desc: "List available workflows (user + project .grok-pi/workflows)",
      },
      {
        cmd: "/create-workflow",
        desc: "Pager prompt to scaffold a workflow script",
      },
    ],
    resourceTitle: "Resources, auth, voice",
    resourceCommands: [
      {
        cmd: "/pi-config",
        desc: "Rust-native Pi resource manager (alias /pi-resources; also F2 → Pi resources)",
      },
      { cmd: "/login", desc: "Authenticate with Pi (Remote TUI path)" },
      { cmd: "/logout", desc: "Clear Pi credentials (Remote TUI path)" },
      { cmd: "/voice", desc: "Voice dictation via xAI STT (Ctrl+Space / F8)" },
    ],
    dynamicTitle: "Dynamic Pi commands",
    dynamicBody:
      "Extension, prompt, and skill commands returned by Pi are not hard-coded in Rust. They enter the Grok slash dropdown via ACP; name conflicts are de-duplicated by the Grok registry. Bridge-only names like __pi_workflow_* are filtered from the catalog.",
    excludedTitle: "Deliberately excluded",
    excludedLead:
      "Grok product / cloud-store commands and startup-only renderer modes are not exposed (Pi owns sessions; switch fullscreen/minimal at launch):",
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
      "Bare /login / /logout here are Pi auth (Remote TUI), not Grok cloud login.",
  },

  features: {
    introLead: "Field-level map for",
    introSSot: "Full SSOT:",
    statusLabel: "Status:",
    deepDives: "Deep dives:",
    cols: { feature: "Feature", status: "Status", notes: "Notes" },
    status: {
      Native: "Native",
      Adapted: "Adapted",
      "Native+Adapted": "Native+Adapted",
      Boundary: "Boundary",
      Experimental: "Experimental",
    },
    sections: [
      {
        title: "Terminal & display",
        rows: [
          {
            feature: "Terminal init/restore",
            status: "Native",
            notes: "Grok init_terminal / restore_terminal",
          },
          {
            feature: "Welcome / logo",
            status: "Native+Adapted",
            notes: "Welcome default; π block art; --continue skips Welcome",
          },
          {
            feature: "Update check/install",
            status: "Adapted",
            notes: "GitHub Releases + official npm metadata + JSP proxy fallback; grok-pi update",
          },
          {
            feature: "Theme / timestamps / mouse",
            status: "Native+Adapted",
            notes: "Pi theme JSON → Grok Theme; pi:transparent*",
          },
          {
            feature: "Voice dictation",
            status: "Native+Adapted",
            notes: "/voice · Ctrl+Space/F8 · xAI STT",
          },
          {
            feature: "Markdown / tool cards / diffs",
            status: "Native+Adapted",
            notes: "ACP chunks → native Pager surfaces",
          },
          {
            feature: "Todo / plan list",
            status: "Native+Adapted",
            notes: "Built-in todo (F2 pi_todo, default on) → ACP Plan → TodoPane",
          },
          {
            feature: "Plan mode",
            status: "Native+Adapted",
            notes: "Ctrl+Shift+T · tool gate · exit_plan_mode",
          },
          {
            feature: "Goal mode",
            status: "Adapted",
            notes: "F2 pi_goal default off; /goal + agent_settled follow-up",
          },
        ],
      },
      {
        title: "Agent & streaming",
        rows: [
          {
            feature: "Prompt / steer / follow-up",
            status: "Adapted",
            notes:
              "ACP prompt; sendNow→steer; default mid-turn→followUp",
          },
          {
            feature: "Abort + queue clear",
            status: "Adapted",
            notes:
              "clear_queue then abort; queue mirror via x.ai/queue/changed",
          },
          {
            feature: "Bash + Send to Background",
            status: "Native+Adapted",
            notes: "pi-grok-bash; x.ai/terminal/background; task kill",
          },
          {
            feature: "Sub-agents",
            status: "Native+Adapted",
            notes: "pi-grok-subagents → SubagentBlock / Tasks Pane",
          },
          {
            feature: "Rhai workflows",
            status: "Native+Adapted",
            notes: "F2 pi_workflows; /workflow /workflows /create-workflow",
          },
          {
            feature: "Compaction",
            status: "Native+Adapted",
            notes: "/compact → Pi compact; native progress blocks",
          },
          {
            feature: "Session recap",
            status: "Adapted",
            notes: "/recap · auto away ≥3 min · optional Mermaid F2",
          },
          {
            feature: "Context bar + /context",
            status: "Native+Adapted",
            notes: "Live breakdown modal; not written to history",
          },
        ],
      },
      {
        title: "Model, session & commands",
        rows: [
          {
            feature: "Model catalog",
            status: "Adapted",
            notes: "get_available_models → native picker",
          },
          {
            feature: "Resume / session picker",
            status: "Adapted",
            notes: "Pi JSONL catalog; Ctrl+F full-text search",
          },
          {
            feature: "Session tree",
            status: "Adapted",
            notes: "navigateTree; non-destructive (≠ Grok Rewind)",
          },
          {
            feature: "Fork / clone",
            status: "Adapted",
            notes: "/fork /clone · rebind + session/load",
          },
          {
            feature: "Jump / review",
            status: "Native+Adapted",
            notes: "/jump turns; /review-session · /review-message",
          },
          {
            feature: "Reload",
            status: "Adapted",
            notes: "/reload blocks streaming+compacting; theme rediscover",
          },
          {
            feature: "HTML export / share",
            status: "Adapted",
            notes: "/export-html · /pi-share (default-on)",
          },
          {
            feature: "Pi resource manager",
            status: "Native+Adapted",
            notes: "Rust /pi-config two-pane; not install/remove",
          },
        ],
      },
      {
        title: "Reliability & isolation",
        rows: [
          {
            feature: "Product home isolation",
            status: "Adapted",
            notes: "~/.grok-pi + <repo>/.grok-pi; migrate-home",
          },
          {
            feature: "Extension self-heal",
            status: "Adapted",
            notes: "Bootstrap bisect on crashing --extension; -ne escape",
          },
          {
            feature: "Resource admission policy",
            status: "Adapted",
            notes: "Allow/block lists + heuristics at spawn",
          },
          {
            feature: "Remote TUI",
            status: "Experimental",
            notes: "PI_GROK_REMOTE_TUI=1; ctx.mode facade tui; no Pi fork",
          },
        ],
      },
      {
        title: "Boundaries (deliberate)",
        rows: [
          {
            feature: "Grok cloud history / usage",
            status: "Boundary",
            notes: "Pi owns local sessions",
          },
          {
            feature: "Adapter TUI / second renderer",
            status: "Boundary",
            notes: "adapter stays headless",
          },
          {
            feature: "Pi source RPC patches",
            status: "Boundary",
            notes: "prefer official extension API",
          },
          {
            feature: "rpiv-ask pure JSONL",
            status: "Boundary",
            notes: "needs Remote TUI custom host",
          },
          {
            feature: "Grok destructive Rewind",
            status: "Boundary",
            notes: "use SessionTree navigation instead",
          },
        ],
      },
    ],
  },

  extensions: {
    intro:
      "grok-pi injects thin bridge extensions into Pi so native Grok surfaces can own Bash, sub-agents, context, recap, and more — without forking Pi. On top of that, install community packages the same way you would for interactive Pi.",
    recommendedTitle: "Recommended community extensions",
    recommendedLead:
      "These are not bundled. Install once into your Pi agent home; both pi and grok-pi load them.",
    installLabel: "Install",
    quickInstallLabel: "Quick install",
    recommended: [
      {
        pkg: "npm:@juicesharp/rpiv-ask-user-question",
        title: "Structured questions → QuestionView",
        status: "Recommended · Remote TUI path",
        body: "Lets the agent ask multi-option / multi-select questions mid-turn. Interactive Pi can open a custom factory UI; under grok-pi the stable path is Remote TUI (default on), which runs the factory in-process and projects onto native Grok QuestionView-style surfaces when the bridge can host it.",
        install: "pi install npm:@juicesharp/rpiv-ask-user-question",
        notes: [
          "Keep PI_GROK_REMOTE_TUI=1 (default) so third-party custom UI can run.",
          "Pure JSONL RPC without the custom host cannot serialize factory components — that is why Remote TUI exists.",
          "If a questionnaire declines, check Remote TUI is enabled and the extension is loaded (not blocked by policy).",
        ],
      },
    ],
    bashTitle: "Bundled: enhanced Bash",
    bashLead:
      "Owns every Bash child process so the Pager can promote a live foreground command into Grok’s native background-task UI without re-running it.",
    bashPoints: [
      "Foreground bash reuses Pi createBashToolDefinition output/render semantics.",
      "Pager “Send to Background” transfers the same subprocess via x.ai/terminal/background (toolCallId control file).",
      "Native task cards: kill via x.ai/task/kill; agent tools get_task_output / wait_tasks / kill_task stay available.",
      "Supports is_background + description for model-started background shells.",
    ],
    bashGate:
      "PI_GROK_BASH=1 (default). Disable with PI_GROK_BASH=0 or --no-extensions.",
    bashTuiTitle: "What you see in the TUI",
    bashTuiSteps: [
      "Agent runs a long bash (build, test, install…).",
      "Use Pager's native Send to Background on the tool card — process keeps running; card becomes a task row.",
      "Kill, wait, or poll from the task UI or via agent tools (get_task_output, wait_tasks, kill_task).",
    ],
    subagentsTitle: "Bundled: sub-agents",
    subagentsLead:
      "Spawns a real Pi child AgentSession and projects lifecycle into native SubagentBlock, Tasks Pane, and child AgentView.",
    subagentsPoints: [
      "Profiles: general-purpose (all tools), explore / plan (safer tool sets).",
      "Capability modes: read-only, read-write, execute, all.",
      "Foreground or background (max concurrency 4 for background).",
      "Versioned bridge pi-grok-subagent/v1 → adapter → x.ai/subagent/* surfaces; cancel is first-class.",
      "Child sessions are persisted; parent resume can rebuild lifecycle from index entries.",
    ],
    subagentsGate:
      "Injected with other bridge extensions. Disabled under --no-extensions / -ne.",
    profileCols: {
      profile: "Profile",
      tools: "Tools",
      useWhen: "Use when",
    },
    profiles: [
      {
        profile: "general-purpose",
        tools: "read, bash, edit, write",
        useWhen: "Delegated implementation slices",
      },
      {
        profile: "explore",
        tools: "read, bash",
        useWhen: "Codebase investigation, diagnostics",
      },
      {
        profile: "plan",
        tools: "read, bash",
        useWhen: "Plans with risks + verification only",
      },
    ],
    catalogTitle: "Full bridge catalog",
    catalogLead:
      "Source lives under extensions/ in the repo. Host injects them at spawn; they are not Pi core patches.",
    catalogCols: {
      extension: "Extension",
      role: "Role",
      default: "Default",
    },
    catalog: [
      {
        name: "pi-grok-bash",
        role: "Bash ownership + background promote",
        default: "On",
      },
      {
        name: "pi-grok-subagents",
        role: "Child AgentSession + native task UI",
        default: "On",
      },
      {
        name: "pi-grok-context",
        role: "System/tools/AGENTS/skills breakdown for /context",
        default: "On",
      },
      {
        name: "pi-grok-recap",
        role: "Display-only session recap (no history mutation)",
        default: "On",
      },
      {
        name: "pi-grok-auth",
        role: "/login /logout via Remote TUI surfaces",
        default: "On",
      },
      {
        name: "pi-grok-export",
        role: "/export-html and /pi-share (gist)",
        default: "On",
      },
      {
        name: "pi-grok-remote-tui",
        role: "ctx.ui.custom host + frame projection",
        default: "On*",
      },
      {
        name: "pi-grok-rpc-compat",
        role: "Present mode=tui to third-party extensions",
        default: "With Remote TUI",
      },
      {
        name: "pi-grok-plan-mode",
        role: "Plan gate + exit_plan_mode approval",
        default: "On",
      },
      {
        name: "pi-grok-goal",
        role: "/goal + update_goal (F2, restart)",
        default: "Off (F2)",
      },
      {
        name: "pi-grok-rollback",
        role: "Tree file rollback snapshots",
        default: "Off (F2)",
      },
      {
        name: "pi-grok-tools",
        role: "F2 built-in tool allow/deny preference",
        default: "On",
      },
      {
        name: "pi-grok-workflows",
        role: "Pi spawn backend for Rhai workflows",
        default: "Off (F2)",
      },
      {
        name: "pi-grok-native-commands",
        role: "Experimental /pi-* selectors",
        default: "Off (env)",
      },
    ],
    catalogFoot:
      "* Remote TUI default-on; set PI_GROK_REMOTE_TUI=0 to disable. F2-gated features need a full process restart after toggle.",
    advancedTitle: "Recap, workflows, self-heal",
    advanced: [
      {
        title: "Recap (/recap)",
        body: "pi-grok-recap is display-only — does not rewrite session history. Auto when away ≥3 min (and ≥3 turns). Optional Mermaid via F2 recap_mermaid. Configure recap_model in F2 (never silently falls back to the live session model).",
      },
      {
        title: "Rhai workflows",
        body: "F2 → Pi workflows (default off, restart). Scripts under ~/.grok-pi/workflows and <repo>/.grok-pi/workflows. Slash: /workflow, /workflows, /create-workflow. Host uses upstream xai-workflow with a Pi spawn backend; __pi_workflow_* bridge cmds are hidden from the catalog.",
      },
      {
        title: "Self-heal on bad extensions",
        body: "If any injected or discovered extension kills RPC bootstrap, the host binary-searches --extension paths, prints the culprit, and relaunches without it. Manual: grok-pi -ne.",
      },
    ],
    controlTitle: "Enable / disable",
    controlGatesLabel: "Gates",
    controlNote:
      "User / project extensions still load through Pi discovery (~/.pi/agent, trusted project trees) unless blocked by /pi-config policy.",
    faqTitle: "FAQ",
    faqs: [
      {
        q: "Do I need to install pi-grok-bash myself?",
        a: "No. The composition binary injects it at runtime. Built-in Todo is also injected by default; install only community packages you still want, such as ask-user-question.",
      },
      {
        q: "Why have Todo if the host already has plan mode?",
        a: "Plan mode is a write gate + approval flow. Built-in Todo is the living checklist projected into TodoPane — complementary, not a replacement. F2 pi_todo is on by default; when it is on grok-pi blocks rpiv-todo to avoid duplicate tool registration. Turn pi_todo off and restart if you intentionally want the community provider instead.",
      },
      {
        q: "Will ask-user-question work without Remote TUI?",
        a: "Not reliably. The package depends on in-process custom UI. Keep Remote TUI on (default) for questionnaires.",
      },
      {
        q: "Can I still use arbitrary Pi extensions?",
        a: "Yes. Install with pi install … or drop packages under Pi extension paths; manage visibility in F2 / /pi-config.",
      },
    ],
  },

  migration: {
    intro:
      "Two entry paths, one host: keep Grok muscle memory or keep Pi sessions — grok-pi sits between Grok Pager and Pi agent core.",
    fromGrokTitle: "From stock Grok Build",
    fromGrokLead: "Same terminal, same shortcuts — agent runtime becomes Pi.",
    grokSteps: [
      {
        step: "1",
        title: "Install grok-pi",
        desc: "One command. The installer detects your platform and installs the binary to ~/.local/bin.",
        code: "curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh",
      },
      {
        step: "2",
        title: "Ensure Pi ≥ 0.84.3",
        desc: "grok-pi drives Pi as its agent core. Install or update Pi via npm.",
        code: "npm install --global @earendil-works/pi-coding-agent",
      },
      {
        step: "3",
        title: "Run in your project",
        desc: "cd into your project and go. Grok Pager keybindings and themes still apply; agent power comes from Pi.",
        code: "cd your-project && grok-pi",
      },
    ],
    fromPiTitle: "From interactive Pi",
    fromPiLead:
      "Keep Pi sessions, models, and extensions. Swap the front-end for Grok Pager.",
    piSteps: [
      {
        step: "1",
        title: "Install grok-pi (keep Pi)",
        desc: "You already have Pi. Add the host binary only — sessions, models, and ~/.pi/agent stay where they are.",
        code: "curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh",
      },
      {
        step: "2",
        title: "Run grok-pi instead of pi",
        desc: "Same project cwd. Same Pi session store. Different TUI (Grok Pager) in front of the same agent core.",
        code: "cd your-project && grok-pi",
      },
      {
        step: "3",
        title: "Resume existing sessions",
        desc: "Partial UUID works, same as Pi --session. Or use /resume inside the TUI.",
        code: "grok-pi --session 019f88c\ngrok-pi --continue",
      },
    ],
    keepTitle: "What you keep",
    piKeep: [
      {
        title: "Sessions stay on Pi",
        desc: "JSONL under ~/.pi/agent/sessions (or --session-dir / PI_CODING_AGENT_SESSION_DIR). No import step. /resume lists the same catalog.",
      },
      {
        title: "Models, tools, extensions",
        desc: "Pi providers, tools, skills, prompts, and extensions keep loading from Pi paths. grok-pi injects only bridge extensions for the Pager surface.",
      },
      {
        title: "Settings still apply",
        desc: "Pi settings.json and auth remain authoritative for the agent. UI chrome (F2, themes display) lives under ~/.grok-pi — isolated from stock Grok.",
      },
      {
        title: "Quit → resume command",
        desc: "On exit you get: To resume this session: grok-pi --session <uuid> — same idea as interactive pi, host binary name swapped.",
      },
    ],
    changesTitle: "What changes",
    piDiffs: [
      {
        title: "TUI is Grok Pager",
        desc: "No Pi interactive TUI. Slash completion, tool cards, diffs, and modals are native Grok surfaces mapped to Pi RPC.",
      },
      {
        title: "Some Pi-only pickers change",
        desc: "/model and /resume use Grok SessionPicker / model UI, not Pi’s TUI components. Behavior is equivalent; layout may differ.",
      },
      {
        title: "Product state dirs",
        desc: "UI prefs/workflows default to ~/.grok-pi and <repo>/.grok-pi so they never collide with stock Grok ~/.grok. Pi agent state is unchanged.",
      },
      {
        title: "Always RPC mode",
        desc: "grok-pi always starts Pi with --mode rpc. Do not expect interactive-only Pi widgets that require an in-process TUI factory.",
      },
    ],
    migrateHomeTitle: "UI home: stock Grok → ~/.grok-pi",
    migrateHomeLead:
      "From 0.0.8, user chrome defaults to ~/.grok-pi so it never collides with stock Grok. Pi sessions stay under ~/.pi/agent. Optional one-shot copy of allowlisted files:",
    migrateHomeNote:
      "Empty target + legacy data may auto-migrate once. Workflows are not copied; put Rhai scripts in ~/.grok-pi/workflows or <repo>/.grok-pi/workflows after enabling F2 Pi workflows.",
    whyTitle: "Why switch?",
    advantages: [
      {
        title: "Keep your Grok muscle memory",
        desc: "Same Pager, same slash commands, same Ctrl+key shortcuts. grok-pi adds capability, not complexity.",
      },
      {
        title: "Unlock any model",
        desc: "/model opens Pi’s full catalog — GPT-4o, Claude, Gemini, local LLMs, custom endpoints. Switch mid-session.",
      },
      {
        title: "Own your sessions",
        desc: "Local JSONL files. Fork, clone, tag, recap. No cloud dependency.",
      },
      {
        title: "Full extension ecosystem",
        desc: "Pi extensions, skills, and prompts appear as native Grok slash commands.",
      },
      {
        title: "Sub-agents & parallel work",
        desc: "Pi sub-agents project into native SubagentBlock, Tasks Pane, and child AgentView.",
      },
      {
        title: "Context visibility",
        desc: "Click the context bar or run /context for a live breakdown of tokens.",
      },
    ],
    faqTitle: "FAQ",
    faqs: [
      {
        q: "Do I lose any Grok Build features?",
        a: "grok-pi retains Grok Pager rendering, input, and navigation. Grok product-only surfaces (cloud history, usage, plugins) are replaced by Pi equivalents — local sessions, extensions, full model access.",
      },
      {
        q: "Can I keep using interactive pi?",
        a: "Yes. grok-pi is a separate binary. Run pi for Pi’s TUI, grok-pi for Grok Pager + Pi core. Sessions are shared when they use the same session dir.",
      },
      {
        q: "Can I go back to stock Grok Build?",
        a: "Yes. stock grok is untouched. Run grok for the original product; grok-pi for the bridged host.",
      },
      {
        q: "Does grok-pi modify Pi or Grok source?",
        a: "No. Pi source is not modified for the bridge. Grok renderer identity is guarded; the adapter is a headless JSONL RPC ↔ ACP bridge.",
      },
      {
        q: "What about my existing Pi sessions?",
        a: "They work as-is. grok-pi reads Pi JSONL directly. /resume shows the catalog; --session <uuid> reopens a specific one.",
      },
    ],
  },
};

/** Widen string leaves so zh can diverge from English literals. */
export type DocsDictionary = DeepStringify<typeof docsEn>;
type DeepStringify<T> = T extends string
  ? string
  : T extends readonly (infer U)[]
    ? DeepStringify<U>[]
    : T extends object
      ? { [K in keyof T]: DeepStringify<T[K]> }
      : T;
export default docsEn;
