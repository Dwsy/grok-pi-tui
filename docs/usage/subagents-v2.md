# Subagents V2 team collaboration

Subagents V2 is the opt-in team collaboration layer for grok-pi's built-in Pi subagents. It adds stable agent paths, peer messaging, nested spawning, reusable child-session context, and external team presets without replacing the existing V1 subagent tools or Rhai workflows.

V2 is **off by default**. Keep it off if you only need `spawn_subagent`, `/subagents`, wait/cancel/history, or parent-to-child messaging.

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
2. Run `/subagent-teams` to inspect discovered presets.
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
- `FAILED` — the last run failed. It is not silently reactivated.
- `CANCELLED` — the last run was interrupted. It is not silently reactivated.

The root is shown as `ROOT`.

## Tool semantics

| Tool | Semantics |
|---|---|
| `spawn_team_agent` | Creates an asynchronous direct child with a stable `/root/...` path. Children receive the same V2 control-plane tools and may spawn nested children. |
| `spawn_team` | Loads one external team preset and creates the full roster before any member starts. Partial startup is rolled back if a later member cannot be created. |
| `team_send_message` | Sends a `MESSAGE`. It is queue-only: an idle completed child receives the semantic message in its session without starting a new turn. A running child receives it through Pi's steer queue. |
| `team_followup_task` | Sends a `NEW_TASK`. Running recipients queue it as follow-up work. Concurrency-queued recipients defer it until their already-queued run finishes. `IDLE` recipients are reactivated in the same child session with a fresh V1 run ID. |
| `team_wait` | Waits for coordinator activity without busy polling. Default 120 s; accepted range 1 s–600 s. Spawn, message, completion and interruption can wake the wait early. |
| `team_list` | Lists stable paths, roles and current statuses. |
| `team_interrupt` | Cancels queued or running work. Queued work is removed before execution; already-terminal work returns its stable terminal status. |

### Automatic final answers

When a child run ends, its final text is sent to its parent as `FINAL_ANSWER`.

- If the parent is running, the result is queued as follow-up context.
- If the parent is `IDLE`, V2 reactivates the same parent child session so the result is not dropped.
- If the parent is the root session, the result is delivered through the root extension message channel.
- A failed/cancelled parent is not silently revived; delivery failure is recorded on the child run.

This makes nested delegation usable without parent-side JSONL polling.

## Concurrency and queueing

V1 and V2 share the subagent runtime's background concurrency limit: **4 active background runs** per root process. Additional work is queued.

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
      "max_turns": 12
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
- member `task`: optional template. Supported variables are `{{task}}`, `{{team}}`, `{{agent_path}}`, and `{{parent_path}}`.

Malformed preset files fail closed. They do not prevent unrelated presets from loading, but a malformed higher-priority file shadows an inherited preset with the same filename stem instead of silently falling back. For example, an invalid project `implementation.json` hides the bundled `implementation` preset until the project file is fixed or removed. `/subagent-teams` lists accepted definitions; an unexpectedly absent preset can therefore indicate a malformed higher-scope shadow.

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
tools: ["read", "grep", "find", "ls"]
models: ["openai/gpt-5.6"]
extensions: []
skills: []
max_turns: 10
---

Review architecture and correctness. Do not edit files. Return evidence with file paths.
```

The Markdown definition controls the system prompt, business tools, up to three models, extensions, skills and max turns. V2 control-plane tools are injected separately so a read-only reviewer can still communicate with the team without gaining edit/write capability.

Use `/subagents` for the native agent-definition management surface.

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

The shared background concurrency limit is four. `team_list` will show `QUEUED`; wait for activity or interrupt work you no longer need.


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
