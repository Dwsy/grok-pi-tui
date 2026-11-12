# Subagents V2 team collaboration

Subagents V2 is the opt-in team collaboration layer for grok-pi's built-in Pi subagents. It adds stable agent paths, peer messaging, nested spawning, reusable child-session context, and external team presets. V2 **replaces** the V1 single-agent tool surface rather than extending it; the V1 runtime and commands stay available.

V2 is **off by default**. Keep it off if you only need `spawn_subagent`, `/subagents`, wait/cancel/history, or parent-to-child messaging.

## Multitask with skills

With Pi subagents and V2 enabled, invoke:

```text
/skill:multitask Implement CSV export and independently review it.
```

The bundled skill selects a strategy for this task tree. Each explicitly authorized child can split independent work within its assigned depth and quota. It starts children asynchronously, continues local work, and ends its turn when dependencies are pending. A child uses `team_report(state="waiting", summary="...")` before ending that turn; `team_list` then shows `WAITING_RESULT`, and a nested result resumes the same child session. Root simply ends its current turn and receives the normal asynchronous result.

Reporting alone does not release a slot. Do not park a coordinator on `team_wait` for queued descendants or set a positive `spawn_team.timeout_ms` in this mode. `waiting` requires a pending child of the reporter; a parent decision uses `blocked` with a question. A dropped child still wakes a parent already waiting for its result, and quiet reports never hide runtime failure/cancellation.

V1 does not advertise this skill. Loading it at root does not preload child resources: copy its compact contract into every handoff. Default skill guidance caps depth at two child layers and allocates eight new identities across the whole task; these are model instructions. The existing runtime enforces 25 concurrent background runs across the tree. Agents share the same cwd, so allocate one writer per file.

For a source build, rebuild grok-pi after changing bundled skills. The skill does not install a session-wide switch, convert the busy input queue, enforce hard tree budgets, isolate worktrees, cascade cancellation, or restore a live team across process restarts. The [design/research record](../issues/adapter/20261102-subagents-v2-multitask.md) separates this implementation from the proposed runtime hardening.

## Enable V2

The normal **Pi subagents** feature must be enabled. It is on by default and can be controlled with **F2 → Agent → Pi subagents**; changing that host feature requires a full process restart.

Start a new process with V2 enabled:

```bash
PI_GROK_SUBAGENTS_V2=1 grok-pi
```

For a local build:

```bash
PI_GROK_SUBAGENTS_V2=1 ./target/debug/grok-pi
```

V2 is process-scoped. It is not hot-loaded into an already running grok-pi process.

To roll back immediately, start the next process without `PI_GROK_SUBAGENTS_V2=1`. V1 remains available and its persisted schema is unchanged.

## Quick start

1. Enable V2 and start grok-pi.
2. Run `/subagent-teams` to configure presets, per-member models, and thinking levels.
3. Ask the root agent to use `spawn_team` with a preset such as `research`, `implementation`, or `review`, or use `spawn_team_agent` for a single named collaborator.
4. Use `team_list` to inspect the team tree and statuses.
5. Use `team_send_message` for information that should enter another agent's context without waking an idle agent.
6. Use `team_followup_task` when the recipient must perform more work.
7. Use `team_wait` instead of polling when waiting for team activity.
8. Use `team_interrupt` to stop queued or running work.

## Identity model

V2 deliberately separates **team identity** from **V1 run identity**:

| Identity | Stability | Purpose |
|---|---|---|
| `/root/...` agent path | Stable for the current root Pi session / extension runtime | V2 addressing and in-memory team topology |
| Pi child session | Reused across completed → follow-up cycles | Preserves the agent's conversation/context |
| V1 subagent UUID | New for each reactivated run | Native Pager lifecycle, Tasks, history and cancellation accounting |

For example, `/root/reviewer` can finish and become `IDLE`. A later `team_followup_task` keeps the same `/root/reviewer` path and the same Pi child session history, but creates a fresh V1 run UUID. This is required because completed native task IDs are tombstoned and must not be resurrected.

Agent paths use lowercase letters, digits and underscores. `root` is reserved. Examples:

```text
/root
/root/researcher
/root/researcher/verifier_2
```

Use absolute paths when addressing siblings. A relative target names a direct child of the sending agent. Creating, forking, or switching the root Pi session (or reloading extensions) replaces the extension runtime, so V2 paths/team topology do not cross that boundary.

## Status model

`team_list` reports:

- `QUEUED` — waiting for the shared background concurrency budget.
- `RUNNING` — a child turn is active.
- `IDLE` — the last run completed successfully; the stable agent can be reactivated with `team_followup_task`.
- `WAITING_DECISION` — the last run completed with a `blocked` report. The question stays in `team_list`, messages still reach it, and `team_followup_task` reactivates it like an `IDLE` agent.
- `FAILED` — the last run failed. It is not silently reactivated.
- `CANCELLED` — the last run was interrupted. It is not silently reactivated.

Each line also carries the agent's quiet gap (`quiet 42s`): the time since it last sent or received team traffic. A long quiet gap on a `RUNNING` agent is the signal to inspect rather than wait.

The root is shown as `ROOT`.

## Tool semantics

| Tool | Semantics |
|---|---|
| `spawn_team_agent` | Creates an asynchronous direct child with a stable `/root/...` path. Children receive the same V2 control-plane tools and may spawn nested children. Accepts `agent_type`, `description`, `model`, `max_turns`, `thinking_level`, and `capability_mode`. |
| `spawn_team` | Loads one external team preset and creates the selected roster before any member starts. A failed member or a cancel that arrives before start rolls back every member already created. Accepts `members`, `model`, `thinking_level`, `max_turns`, `capability_mode`, `instructions`, and `timeout_ms` on top of `team` and `task`. |
| `team_send_message` | Buffers a `MESSAGE` without starting an idle recipient turn. A 25 ms merge window batches messages from one sender to one target (numbered, repeated text dropped), then flushes automatically; team actions and root turn end may flush earlier. Running recipients receive an actual steer at their next safe loop boundary. Use it for facts, corrections, or wrap-up instructions needed during current work. |
| `team_followup_task` | Sends a `NEW_TASK` after the recipient's current work; it is not an urgent correction channel. Concurrency-queued recipients are promoted to the front of the shared queue before their pending run executes, so an explicit parent request does not wait behind unrelated launches. `IDLE` and `WAITING_DECISION` recipients are reactivated in the same child session with a fresh V1 run ID. |
| `team_wait` | Waits for coordinator activity without busy polling. Default 120 s; accepted range 1 s–600 s. Spawn, message, completion and interruption can wake the wait early. |
| `team_list` | Lists stable paths, roles, current statuses, the last report or decision question, and the quiet gap. Flushes buffered messages first so the tree it shows matches what was already sent. |
| `team_interrupt` | Cancels queued or running work. Queued work is removed before execution; already-terminal work returns its stable terminal status. |
| `team_report` | Declares how the child's turn ends so the parent is woken only when it must act. Child-only: the root has no parent. |

### `spawn_team` call-level controls

| Parameter | Effect |
|---|---|
| `team`, `task` | Preset name and the shared objective substituted into each member template as `{{task}}`. |
| `members` | Optional subset of preset member names. Preset order is preserved, so a synthesizer defined last still starts after its producers. An unknown name fails before any child is created. |
| `model`, `thinking_level`, `max_turns` | Applied to every selected member, overriding that member's preset value. Omitting the parameter keeps the member's own preset value. |
| `capability_mode` | Tool access for every selected member: `read-only` or `all`. Both include Bash; `read-only` omits built-in `edit`/`write` and injects the read-only contract. Legacy `execute`/`read-write` map to `all`. Defaults to each agent definition, then `all`. |
| `instructions` | Extra team-wide instruction appended to the preset `instructions` and shared through every member's system prompt. |
| `timeout_ms` | `0` or omitted returns immediately and delivers final answers to `/root` asynchronously. A positive value waits up to 600 s and returns each member's status and output in the tool result. |

Permanent per-member differences belong in the preset JSON or the `/subagent-teams` editor. Call-level overrides apply to one run only (for example, forcing a cheaper model or a read-only roster) and are not persisted.

### V1 tools are not registered in V2 mode

Enabling V2 replaces the model-facing tool surface instead of adding to it. `spawn_subagent`, `send_message_to_subagent`, `get_command_or_subagent_output`, `kill_command_or_subagent`, and `list_subagents` are **not registered**, because `spawn_subagent` blocks the root until the child finishes while team mode requires the root to end its turn and be reactivated by `FINAL_ANSWER`.

V2 covers the same jobs: `spawn_team_agent`/`spawn_team` for spawning, `team_send_message`/`team_followup_task` for messaging, `team_wait` for waiting, `team_interrupt` for cancellation, and `team_list` for inspection.

The V1 **runtime and commands** remain. `/subagents`, `/subagent-message`, and `/subagent-history` still work, and the hidden `__pi_grok_subagent_replay` / `__pi_grok_subagent_cancel` bridge commands stay registered because the Rust adapter requires them to enable the subagent bridge.

### Declaring a result with `team_report`

A child ends a turn by calling `team_report`:

| `state` | Meaning | Parent woken |
|---|---|---|
| `ready` | The requested result is deliverable | yes |
| `blocked` | A parent decision is required; `question` is mandatory | yes |
| `failed` | The work cannot be completed | yes |
| `dropped` | Nothing for the parent to act on | **no** |

The report replaces the child's final text in the delivered message: `Report: <state>`, the one-sentence `summary`, and `Needs your decision: <question>` when present. A child that skips `team_report` keeps the previous behaviour, where its final text is delivered unconditionally.

`dropped` is the only silent state. The agent stays `IDLE`, `team_list` keeps its summary, and no parent turn is spent. Use it when a preset member finds nothing to change, the request is already satisfied, or another agent superseded the work.

`team_report` is registered for children only. The root has no parent, so the tool is absent from its surface.

Reactivating an agent clears its previous report. A new run never replays the old summary, and a stale `dropped` cannot swallow the new result.

### Automatic final answers

When a child run ends, its result is sent to its parent as `FINAL_ANSWER`.

- If the parent is running (including root), the result enters its steering queue and is processed at the next safe loop boundary, without waiting for the entire tool loop to finish.
- If the parent is `IDLE`, V2 reactivates the same parent child session so the result is not dropped.
- If the parent is the root session, the result is delivered through the root extension message channel.
- A failed/cancelled parent is not silently revived; delivery failure is recorded on the child run.

This makes nested delegation usable without parent-side JSONL polling.

The merge window bounds batching, not end-to-end latency: neither steer nor a final answer aborts an already-running model request or tool. Ordinary messages omit `triggerTurn` so Pi steers active sessions but only appends to idle ones; explicitly passing `false` would bypass the active steering queue. Child provider/stream responses ending in `error` or `aborted` are recorded as `FAILED` or `CANCELLED`, even when Pi's prompt Promise resolves normally. A recovered error followed by a successful response remains completed.

## Concurrency and queueing

V1 and V2 share the subagent runtime's background concurrency limit: **25 active background runs** per root process (configurable as "Background concurrency" in `/subagents`, range 1–256, applies to new spawns). Additional work is queued.

The queue is cancellation-safe: cancelling a queued run removes it before it can start. A `team_followup_task` sent to an agent that is itself still waiting in the concurrency queue is stored as pending team work and starts only after the already-queued run completes.

A team preset may contain at most **8 members**. `spawn_team` registers the complete roster before starting members so early agents can safely address later siblings.

## Team presets

Team presets are JSON files discovered in this order, with later scopes overriding earlier definitions of the same team name:

1. bundled: `extensions/pi-grok-subagents/teams/*.json`
2. global: `~/.grok-pi/teams/*.json` (or `$GROK_HOME/teams/*.json`)
3. project: `<repo>/.grok-pi/teams/*.json` (or `$GROK_PROJECT_DIR/teams/*.json`)

Project therefore overrides global, and global overrides bundled.

Bundled presets are `research`, `implementation`, and `review`.

### Team JSON schema

```json
{
  "name": "implementation",
  "description": "Implementation plus review",
  "enabled": true,
  "instructions": "Share concrete file paths and verify each other's claims.",
  "members": [
    {
      "name": "implementer",
      "agent": "general-purpose",
      "description": "Primary implementer",
      "task": "Implement the objective: {{task}}",
      "model": "openai/gpt-5.6",
      "max_turns": 12,
      "thinking_level": "high"
    },
    {
      "name": "reviewer",
      "agent": "explore",
      "task": "Review {{task}}. The implementer is {{parent_path}}/implementer."
    }
  ]
}
```

Contract:

- `name`: optional; defaults to the filename. Team names allow letters, digits, `_` and `-` and are matched case-insensitively.
- `enabled`: defaults to `true`. A higher-scope `enabled: false` definition disables a lower-scope preset of the same name.
- `members`: required for enabled teams; 1–8 members.
- member `name`: required, unique within the team, lowercase letters/digits/underscores only; becomes the path segment.
- member `agent`: external agent definition name; defaults to `general-purpose`.
- member `model`: optional Pi model key. If the referenced agent definition has a model allowlist, the value must be in it.
- member `max_turns`: optional non-negative integer. The external agent definition's `max_turns` takes precedence.
- member `thinking_level`: optional `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. Omitted falls back to the agent definition's `thinking_level`, then Pi settings. A level the selected model does not accept is clamped to the nearest supported level instead of failing the whole team startup.
- member `task`: optional template. Supported variables are `{{task}}`, `{{team}}`, `{{agent_path}}`, and `{{parent_path}}`.

Malformed preset files fail closed. They do not prevent unrelated presets from loading, but a malformed higher-priority file shadows an inherited preset with the same filename stem instead of silently falling back. For example, an invalid project `implementation.json` hides the bundled `implementation` preset until the project file is fixed or removed. `/subagent-teams` shows accepted definitions; an unexpectedly absent preset can therefore indicate a malformed higher-scope shadow.

## Configure models and thinking levels with `/subagent-teams`

`/subagent-teams` is a full interactive editor, not a read-only list:

1. Choose `New project team` / `New global team`, or an existing preset.
2. Open `Members`, then `Add member` with a member name (lowercase letters, digits, underscores).
3. Select a member → `Edit settings`:
   - `Agent`: agent definition name.
   - `Model`: the first entry is **All available models (default)**, followed by every currently available `provider/model`. Choosing the default writes no `model`, so the agent definition or Pi decides.
   - `Thinking level`: the first entry is **Inherit Pi settings (default)**; the rest are only the levels the selected model actually accepts, derived from `reasoning` and `thinkingLevelMap`. With no explicit model, the list is the union across the catalog.
   - `Max turns`: leave empty to inherit the agent definition; `0` means unlimited.
4. `Save and close` writes to project or global scope.

Bundled presets (`research`, `implementation`, `review`) are read-only: editing asks for a **Project override** or **Global override** first and never rewrites files inside the installation directory.

Model and thinking-level discovery happens entirely in the TUI. This extension registers **no model-facing tool** that lists models or thinking levels, so none of it consumes model context.

## Agent definitions

Team JSON controls topology; it does **not** define business-tool permissions. Agent profiles remain Markdown files:

- project: `<repo>/.grok-pi/agents/*.md`
- global: `~/.grok-pi/agents/*.md`

Project definitions override global definitions, including `enabled: false`.

Example:

```markdown
---
description: Read-only architecture reviewer
enabled: true
tools: ["read", "bash", "grep", "find", "ls"]
models: ["openai/gpt-5.6"]
thinking_level: high
extensions: []
skills: []
max_turns: 10
---

Review architecture and correctness. Do not edit files. Return evidence with file paths.
```

The Markdown definition controls the system prompt, business tools, up to three models, extensions, skills, max turns, the default `thinking_level`, and inheritance flags. V2 control-plane tools are injected separately so a read-only reviewer can still communicate with the team. **There are only two capability modes**: `read-only` (reduced loadout + `READ_ONLY_CONTRACT`) and `all` (every built-in tool). Both include Bash, including definition-supplied `tools:` lists. Legacy `execute` / `read-write` values are silently mapped to `all` so existing presets keep working. Custom tool lists that omit both `edit` and `write` are treated as read-only. Agents are constrained by the injected system prompt (the read-only contract, or each profile's own instructions), not by hiding Bash. This is a prompt-level rule, not an OS-level boundary.

Use `/subagents` for the native agent-definition management surface (including the background-concurrency setting) and `/subagent-control` for the unified TUI control center (status/definitions/presets/injection preview). All configuration screens are full-screen pi-tui components (projected into Pager by the Remote TUI bridge): list selection, checkbox toggles, button confirmation, text input, and scrolling work with both mouse and keyboard.

### Built-in profile tool access

| profile | capabilityMode | tools | behaviour |
|---|---|---|---|
| `general-purpose` | `all` | `read` `bash` `edit` `write` `grep` `find` `ls` | Full implementation |
| `read-only` capability | `read-only` | `read` `bash` `grep` `find` `ls` | Built-in `edit`/`write` omitted; `READ_ONLY_CONTRACT` forbids all file and state mutations |
| `explore` | `all` (default) | `read` `bash` `edit` `write` `grep` `find` `ls` | Full loadout, but its system prompt forbids modifying tracked files. Has Bash for `rg`, `git log`, `tree`, `wc`, `python3 -c`, `node -e` |
| `plan` | `all` | `read` `bash` `edit` `write` `grep` `find` `ls` | Full loadout, but its system prompt says "Do not edit files" |

`execute` and `read-write` are no longer modes: `requireCapability()` maps them to `all`, so old presets and agent definitions keep working without a separate tool contract. Persisted snapshots from older runs still load.

Beyond the `edit`/`write` tools omitted in `read-only`, all agent constraints are **prompt-level**, not OS-level enforcement: Bash can run arbitrary commands, so the injected system prompt (the read-only contract, or the profile's own instructions) is what forbids file writes, Git mutations, installs, database writes, and other state changes. For a hard boundary, use an external OS/container sandbox; this extension does not provide one.


### Default child extensions

A child session loads no auto-discovered extensions, so root-session tooling would otherwise be missing inside every subagent. These are added explicitly when present:

| Extension | Provides |
|---|---|
| `~/.pi/agent/npm/node_modules/@ff-labs/pi-fff` | FFF fuzzy file and content search |

A missing directory is skipped silently, so an uninstalled extension is not a startup error. `extensions:` in an agent definition adds more on top. When an extension registers tools while binding, those tools are kept on top of the session's own loadout instead of replacing it, so `capability_mode` still holds.

### Child skills and AGENTS.md inheritance (default on)

A child session inherits the root session's global/project skills and AGENTS.md/CLAUDE.md context by default. The definition's explicit `skills:` are merged on top. To restore isolation, set frontmatter flags:

```md
---
description: "isolated worker"
inherit_skills: false
inherit_context: false
---
```

- `inherit_skills: false` → only `skills:` load (`noSkills`).
- `inherit_context: false` → no AGENTS.md discovery (`noContextFiles`).
- Extensions stay isolated: only `extensions:` load (plus the default search extension when installed).

### Child prompt composition

A child session runs with `systemPromptOverride`, so Pi renders the **preamble only**: the default `tools` and `rules` sections are skipped. Consequences:

- A tool's `promptSnippet` and `promptGuidelines` reach the **root session only**. They never reach a child.
- A child's whole behavioural briefing is `definition.systemPrompt` + the V2 `usageHint`, plus each tool's `description`.
- The default rules ("Be concise", "Show file paths clearly") are absent in a child, which is why `usageHint` states the evidence rule explicitly.

Keep child-facing rules in `usageHint` or a tool `description`. Adding a `promptGuideline` will not change child behaviour.

## Message and persistence boundaries

V2 semantic messages use the custom message type `pi-grok-team-message/v2` and are intentionally model-visible in the recipient session.

The existing `pi-grok-subagent/v1` bridge remains UI/lifecycle-only, but all lifecycle and child updates now use one process-private ordered socket. Recovery snapshots live in a separate `<parent-session>.subagents.jsonl` sidecar; no V1 bridge/state entry is appended to the parent Pi JSONL.

Pi child-session JSONL remains the durable conversation/history store. It is not polled as a realtime team message bus; live routing and `/root/...` team topology are in-process through the coordinator and Pi's official session APIs. Resuming the parent session in a new process preserves child history but does not reconstruct the previous V2 team tree or path-addressable pending work.

## Relationship to Rhai Workflow

Use **Subagents V2** when you need long-lived agent identity, peer communication, nested delegation and adaptive collaboration.

Use **Rhai Workflow** when you need deterministic orchestration, explicit steps/branches and scriptable control flow.

They are complementary. A workflow can delegate to subagents; a team preset is not a replacement workflow engine.

## Operational guidance

For production-like use:

- Keep V2 opt-in until the target model/provider has passed a real multi-agent handtest in your environment.
- Prefer small teams with distinct roles. More agents increase context, tool and queue pressure.
- Give each member a narrow `task` template and a least-privilege agent definition.
- Prefer `team_wait` to repeated `team_list` polling.
- Use `team_send_message` for facts/context and `team_followup_task` only when another turn is required.
- Treat `FAILED` and `CANCELLED` as explicit terminal states; do not assume automatic recovery.
- Use project-scoped team/agent definitions for repository-specific behavior and global definitions only for reusable defaults.

## Troubleshooting

### `spawn_subagent` is missing

That is expected while V2 is enabled. See [V1 tools are not registered in V2 mode](#v1-tools-are-not-registered-in-v2-mode). Start the next process without `PI_GROK_SUBAGENTS_V2=1` to get the V1 tool surface back.

### `/subagent-teams` is missing

Check all of the following:

1. **Pi subagents** is enabled in F2.
2. The process was started with `PI_GROK_SUBAGENTS_V2=1`.
3. The process was restarted after changing feature state.
4. Bundled bridge extensions were not disabled for the process.

### A team preset is not listed

- Validate that the file ends in `.json` and contains valid JSON.
- Check member names: lowercase letters, digits and underscores only.
- Check that enabled teams have at least one and at most eight members.
- Check whether a project/global preset with the same `name` overrides it.
- Check whether the winning definition has `enabled: false`.

### An agent cannot start

- Confirm the referenced agent definition is enabled.
- Confirm the selected model exists in the current Pi model registry.
- If the agent definition lists models, confirm the requested team member model is allowed.
- Use `team_list` to check for an existing path collision. Stable paths are unique within one coordinator session.

### A message did not start work

That is expected for `team_send_message` to an `IDLE` agent. Use `team_followup_task` when a new turn is required.

### Work appears queued

The shared background concurrency limit is 25 by default (configurable via `/subagents`). `team_list` will show `QUEUED`; wait for activity or interrupt work you no longer need.


## Reproducible local checks

From the repository root, the V2-specific automated checks are:

```bash
cd extensions/pi-grok-subagents
bun test v2.test.ts runtime.test.ts
cd ../..
node extensions/pi-grok-bash/test-v2.1.mjs
cargo test -p xai-grok-pager-bin --bin grok-pi
```

The Subagents test command exercises the coordinator and runtime directly. The Rust binary suite also verifies that the bundled extension materializer copies every required TypeScript/JSON dependency into its temporary runtime bundle.

A standalone TypeScript check is not a clean gate in this checkout: without the sibling Node type path it stops first at `TS2688` (`@types/node` is not visible); after making that dependency explicit, the extension path mappings cross into `pi-main` and hit existing baseline diagnostics there. Treat the Bun tests plus Rust materialization/loadability tests as the reproducible automated gate for this increment, and keep the real-model handtest below as a separate release gate. See `VERIFICATION.md` for the dated result and blocker details.

## Verification checklist

Before promoting V2 from opt-in in a release, verify:

- V2 unit tests and runtime hardening tests pass.
- Pi can load the extension with V2 enabled.
- The Rust injector materializes every TS/JSON dependency.
- `grok-pi` builds successfully.
- V2-off does not expose `/subagent-teams` or V2 tools.
- V2-on discovers bundled presets.
- A real-model handtest covers root → child, child → root, sibling messaging, nested spawn, idle follow-up, nested final-answer wakeup, interrupt and queueing.
- Parent session growth comes only from real conversation semantics; V1 UI lifecycle/state lives in the socket + sidecar and does not enter the parent Pi JSONL.
