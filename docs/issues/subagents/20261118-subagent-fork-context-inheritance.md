---
id: "2026-11-18-subagent-fork-context-inheritance"
title: "Subagent parent-context fork mode for V1 & V2 (root & child initiators)"
status: "done"
created: "2026-11-18"
updated: "2026-11-18"
category: "subagents"
tags: ["subagents", "fork", "v1", "v2", "context-inheritance"]
---

# Issue: Subagent Parent-Context `fork` Mode for V1 & V2 (`root` & `child` Initiators)

## Goal

Add an opt-in `fork?: boolean` spawn parameter mode to both V1 (`spawn_subagent`) and V2 (`spawn_team_agent`, `spawn_team`) subagents so either `root` or an existing `child` subagent can spawn an isolated child `AgentSession` that preserves the initiating parent's live conversation branch, skills, context composition (`AGENTS.md`, prompts, system prompt sections such as `<reply_style>`), extensions/plugins, and tool loadout.

## Architecture & Pi Core Alignment

1. **Shared-JSONL Multi-Leaf Branching (`fork.ts` — `SharedBranchSessionManager` & `createForkedChildSessionManager`)**:
   - Real Pi `ctx.sessionManager` is the host's live `SessionManager` instance and all branch/append mutations are synchronous. `SharedBranchSessionManager` wraps the shared backing `SessionManager` (unwrapping through nested forks so `root -> forkedChild -> forkedGrandchild` all share the single `.jsonl` file and `byId`/`fileEntries` tree) while isolating each forked subagent's own `cursorLeafId` and unique `branchSessionId` (`<sessionId>-fork-<8hex>`) via atomic swap-and-restore on every read/write.
   - Falls back cleanly to a standalone copy (`createStandaloneForkedSessionManager`) only when called with a partial non-mutable mock `SessionManager` lacking `.branch` / `.appendMessage`.
   - Under the shared fork point (`forkOriginLeafId`), each child anchors with `pi-grok-fork-branch/v1`, closes any trailing in-flight unresolved `toolCall`s with synthetic `toolResult`s before `startLeafId`, uses branch-local `appendContextEdit(ancestorForkContextId, null)` to omit any ancestor `pi-grok-fork-context/v1` envelope on nested forks without affecting the parent, and attaches a `/tree` bookmark label (`fork: <desc>` or `fork <agentPath>: <desc>`) on its first user turn so `/tree` (`ctx.navigateTree`) can inspect and jump directly into any forked subagent branch.
   - `bridge.ts` (`persistedRunBranch`) also scopes live/running `endLeafId === null` inspection inside a shared `.jsonl` linearly from `startLeafId` while skipping nested `pi-grok-fork-branch/v1` descendant subtrees.
2. **Skills, Context Composition & `SystemMessage` Fidelity**:
   - Captures `NormalizedBuildSystemPromptOptions` on `before_agent_start` in both `root` (`index.ts`) and every child `AgentSession` (`createPromptCaptureExtensionFactory`), with transcript fallback via `getCurrentSystemMessage()` / `getCurrentTools()`.
   - Populates the forked child's `DefaultResourceLoader` with `skillsOverride`, `agentsFilesOverride`, `promptsOverride`, `systemPromptOverride`, and `appendSystemPromptOverride`, plus `createForkContextExtensionFactory` to restore `sections` (e.g. `reply_style`), `promptGuidelines`, and `forceSystemPrompt` — keeping Pi's section-diffing (`diffSystemPromptSections`) and prompt-cache prefix intact instead of overwriting with a monolithic `customPrompt`.
   - Delivers child role / V2 `usageHint` / `READ_ONLY_SUBAGENT_PROMPT` once via a non-displayed `pi-grok-fork-context/v1` custom message (`display: false`) on the first turn.
3. **Plugins / Extensions & Tool Loadout**:
   - Inherits `settingsManager.setProjectTrusted(...)`, CLI `--extension` / `--no-extensions` flags, discovered non-bridge extension paths (`tool.sourceInfo.path` / `command.sourceInfo.path` / parent `resourceLoader`), builtin extensions (`builtin:codemode`, `builtin:tool-search`, `builtin:mcp`), and `DEFAULT_SUBAGENT_EXTENSIONS`.
   - Filters out only internal `grok-pi` host-singleton bridges (`isHostSingletonBridgeExtension`, `hasHostSingletonConflict`) so root sockets, panes, bash registry, goal state, and plan-mode controls are never corrupted.
   - Preserves parent active tool order while stripping host-root-only tools (`HOST_ROOT_ONLY_TOOLS`) and stripping `edit` / `write` when `capability_mode: "read-only"`.
   - Shares the live `ModelRuntime` across parent and child sessions.
4. **Root & Child Initiation in Both V1 and V2**:
   - V1 (`tools-v1.ts`): extracts `buildV1AgentTools(runtime, getParentRecord)` so V1 children also receive V1 control tools (including `spawn_subagent({ fork: true })`) with `bridgeParentSessionId` routing already supported by Rust `subagent_projection.rs`.
   - V2 (`v2.ts`): adds `fork?: boolean` to `spawn_team_agent` (root & child) and `spawn_team` (root).
5. **Rust Extension Injector Mirror**:
   - `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/subagent_extension.rs` materializes `fork.ts` and keeps `./`-free source compliance for the injector unit test scanner.

## Verification Checklist

- [x] `fork.test.ts` offline real-SDK suite (6/6: host-bridge filter & CLI arg parser, `SharedBranchSessionManager` shared-JSONL multi-leaf + `/tree` labels, standalone mock fallback compaction/toolCall closure, V1 root→forked child→forked grandchild sharing `root.sessionFile` + live `navigateTree` jump + isolated child→forked grandchild, V2 root/child `spawn_team_agent(fork=true)` sharing `root.sessionFile` + `team_followup_task` reactivation + `spawn_team(fork=true, capability_mode="read-only")`, turn-1 fork with bg/model/thinking/read-only overrides + `builtin:codemode`)
- [x] Existing `index.test.ts`, `v2.test.ts`, `runtime.test.ts`, `delivery.test.ts` on Pi 1.5.2 (89/89 total passing with `fork.test.ts`)
- [x] `extensions/pi-grok-plan-mode/index.test.mjs` (6/6 passing: Plan Mode blocks mutating `fork: true`, allows `fork: true` with `capability_mode: "read-only"`)
- [x] Live `pi -ne --mode rpc` boot check (V1 & V2 exit 0, empty stderr)
- [x] `rustfmt --edition 2024 --check` on `subagent_extension.rs` + raw `./` scanner simulation
- [x] `verify_native_grok.py` baseline comparison (0 unallowlisted added files; 0 regressions attributable to this change — only pre-existing baseline failures + user's concurrent `notifications/{config,mod}.rs` seam diff)
- [x] `git diff --check`
- [ ] Cargo builds/tests (`cargo test -p xai-grok-pager-bin --bin grok-pi` etc.) skipped per `AGENTS.md` explicit-user-request rule
