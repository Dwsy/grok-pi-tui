# grok-pi Plan Mode Integration

<!-- pdca:gen -->
> **status:** in_progress  ·  **phase:** D2  ·  **pdca:** act
> **validation:** pending  ·  **open decisions:** none
> **next:** Run the four-step native Pager plan-mode walkthrough and resolve the independent pi-main workspace build issue
<!-- /pdca:gen -->

## Objective

Implement plan mode lifecycle in pi-grok-adapter: ACP mode broadcast, prompt-prefix reminder injection, Pi extension beforeToolCall gate, exit_plan_mode approval flow, and plan file management — matching Grok native PlanModeTracker semantics.

## Scope

### In Scope (5 layers)

1. **ACP Protocol Layer** — `initialize()` broadcasts `session_modes: [default, plan]`; `set_session_mode()` tracks state transitions
2. **Prompt Injection Layer** — plan-active prompts get `<system-reminder>` prefix (full/sparse alternation); activation/reentry/exit templates
3. **Tool Gate Layer** — Pi extension `beforeToolCall` hook blocks write tools (edit, write, bash, apply_patch) except plan file writes
4. **Exit Flow Layer** — `exit_plan_mode` tool registered via Pi extension → ACP `x.ai/exit_plan_mode` permission request → user approval → state transition
5. **Plan File Layer** — `plan.md` in Pi session dir; auto-created on activation; content projected to `SessionUpdate::Plan`

### Architecture Constraint

- **Adapter is the sole owner** of plan mode state (Pi RPC has no mode concept; Pager only renders)
- **No Pi core modification** — all control via RPC commands + extension hooks
- **No Pager modification** — standard ACP `SessionUpdate::Plan` + permission request protocol

### State Machine (mirrors Grok PlanModeTracker)

```
Inactive → Pending → Active → ExitPending → Inactive
              ↑                    |
              └── (re-entry) ──────┘
```

| Transition | Trigger |
|---|---|
| Inactive→Pending | `set_session_mode("plan")` |
| Pending→Active | First `prompt()` while Pending (inject activation reminder) |
| Active→ExitPending | `set_session_mode("default")` while turn in-flight |
| ExitPending→Inactive | `agent_settled` (inject exit reminder next prompt) |
| Active→Inactive | `exit_plan_mode` approved by user |
| ExitPending→Active | `set_session_mode("plan")` re-entry (cancel deferred exit) |

## Out Of Scope

- Pager UI changes (mode selector, approval dialog) — uses existing ACP protocol surfaces
- Pi core agent-loop modification
- Plan file content parsing / structured plan extraction
- Multi-session plan sharing
- Cursor harness template variants (`is_cursor_harness()` always false in Pi path)

## Acceptance

- [ ] `initialize()` response includes `session_modes` with `plan` entry; Pager shows mode selector
- [ ] `set_session_mode("plan")` → next prompt injects full activation reminder with plan_path
- [ ] Subsequent prompts alternate full/sparse reminders (even=full, odd=sparse)
- [ ] Write tools (edit, write, bash) blocked during Active; model receives rejection reason mentioning plan file path
- [ ] Plan file write (edit to `plan.md`) allowed during Active
- [ ] `exit_plan_mode` tool call → ACP permission request → user approve → Active→Inactive + exit reminder injected
- [ ] `exit_plan_mode` tool call → user reject → stays Active
- [ ] `set_session_mode("default")` mid-turn → ExitPending → after agent_settled → Inactive + exit reminder
- [ ] Re-entry: second activation uses reentry template (mentions existing plan file)
- [ ] State survives adapter restart (persisted to Pi session custom entry or adapter state file)
- [ ] `cargo test -p pi-grok-adapter` passes with new plan_mode tests
- [ ] `./verify.sh` passes (no regressions)

## SSOT / Links

- Grok PlanModeTracker: `crates/codegen/xai-grok-shell/src/session/plan_mode.rs`
- Grok session_mode handler: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/session_mode.rs`
- Grok edit gate: `crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs:166`
- Pi beforeToolCall: `pi-main/packages/agent/src/agent-loop.ts:621`
- Pi RPC types: `pi-main/packages/coding-agent/src/modes/rpc/rpc-types.ts`
- Adapter entry: `crates/codegen/pi-grok-adapter/src/pi_adapter.rs`
- Adapter todo_bridge (existing Plan projection): `crates/codegen/pi-grok-adapter/src/todo_bridge.rs`
- Alignment doc: `NATIVE_GROK_TUI_ALIGNMENT.md`

## Key Design Decisions (from research)

| Decision | Choice | Rationale |
|---|---|---|
| Reminder injection point | Prompt prefix (before user message) | Pi RPC has no `set_system_prompt` command |
| Tool interception | Pi extension `beforeToolCall` hook | Only pre-execution gate available; adapter already injects extensions |
| Plan file location | Pi session dir + `plan.md` | Mirrors Grok's `~/.grok/sessions/<cwd>/<id>/plan.md` |
| Exit trigger | Extension-registered `exit_plan_mode` tool | Closest to Grok native; model calls tool → adapter intercepts → ACP approval |
| Mid-turn toggle | Simplified: effective next prompt | Pi has no drain-point concept; acceptable degradation |
| Full/Sparse alternation | Preserved from Grok | Token economics; validated design |
| State persistence | Adapter state file in Pi session dir | Pi restart recovers via `get_state` + custom entry |
