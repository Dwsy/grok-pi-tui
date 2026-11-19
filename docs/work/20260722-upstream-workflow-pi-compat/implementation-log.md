# Implementation Log

Chronological deliverables. Add a dated section after every Do/Check slice.

## 2026-07-22 — PDCA scaffold + architecture Plan (P0)

- **Scope:** Frame full upstream Workflow compatibility for grok-pi (no code yet)
- **Files:**
  - `work/20260722-upstream-workflow-pi-compat/task.md`
  - `work/20260722-upstream-workflow-pi-compat/decisions.md`
  - `work/20260722-upstream-workflow-pi-compat/pdca.md`
  - `work/20260722-upstream-workflow-pi-compat/prompt.md`
  - `work/20260722-upstream-workflow-pi-compat/implementation-log.md`
  - `work/20260722-upstream-workflow-pi-compat/state.json`
- **Behavior:**
  - Locked architecture: **xai-workflow + shell orchestration + Pi Spawn backend**
  - Rejected: TS Rhai reimplementation; dual shell process; adapter TUI
  - Slices S1–S5 defined; acceptance A1–A8 written
- **Verify:** Research-only (read engine/host_service/registry/workflow_ingest/subagents); no cargo yet
- **Open:** (resolved below)

## 2026-07-22 — D1=split, enter P1

- **Scope:** Close planning decision; enter first tracer phase
- **Files:** state/history via `update-pdca-state.mjs` (auto-refresh gen regions)
- **Behavior:**
  - D1 selected **`split`** (S1–S5)
  - phase **P1 / do**; next = S0 issue + S1 shell SpawnBackend
- **Verify:** n/a (plan decision only)
- **Open:** user authorize → write `docs/issues/` then S1 code

## 2026-07-22 — S1–S4 foundation implementation

- **Scope:** Upstream-compatible workflow spawn seam for grok-pi
- **Files:**
  - `crates/codegen/xai-grok-shell/src/session/workflow/backend.rs` (new)
  - `crates/codegen/xai-grok-shell/src/session/workflow/external.rs` (new)
  - `crates/codegen/xai-grok-shell/src/session/workflow/{host_service,manager,mod}.rs`
  - `crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs`
  - `extensions/pi-grok-workflows/`
  - `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/workflow_extension.rs`
  - `crates/codegen/pi-grok-adapter/src/pi_workflow_backend.rs`
  - `docs/issues/架构/20260722-上游Workflow完整兼容-Pi-Spawn后端.md`
  - `FEATURE_MATRIX.md` / `.zh-CN.md`
- **Behavior:**
  - Pluggable `WorkflowAgentBackend` (Grok SubagentEvent default + Mock + Pi file-bridge)
  - `ExternalWorkflowRuntime` reuses shell manager
  - Pi extension only spawns children (no Rhai)
  - grok-pi injects extension with `PI_GROK_WORKFLOWS=1`
- **Verify:**
  - `cargo test -p xai-grok-shell --lib session::workflow::manager::tests::mock_backend_runs_agent_call_in_rhai` → ok
  - `cargo test -p xai-grok-shell --lib session::workflow::external` → ok
  - `cargo test -p pi-grok-adapter --lib pi_workflow_backend` → ok
  - `cargo check -p xai-grok-pager-bin --bin grok-pi` → ok
- **Open:** adapter session-lifecycle Runtime + live `workflow_updated` + deep-research handtest

## 2026-07-22 — Session host wiring (residual closed)

- **Scope:** Mount ExternalWorkflowRuntime in pi-grok-adapter; project workflow_updated
- **Files:**
  - `crates/codegen/pi-grok-adapter/src/workflow_host.rs`
  - `crates/codegen/pi-grok-adapter/src/pi_workflow_backend.rs` (channel-based bridge)
  - `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` (host + ext methods + bridge worker)
  - shell `workflow/{notify,external,mod,store}.rs` pub exports
- **Behavior:**
  - ACP `x.ai/workflow/launch|pause|stop`
  - `pi-grok-workflow/v1` tool_request → host launch
  - Poller emits `x.ai/session_notification` with WorkflowUpdated
  - Spawn via `__pi_workflow_spawn` file protocol on LocalSet
- **Verify:**
  - `cargo test -p pi-grok-adapter --lib` → 100 ok
  - `cargo check -p xai-grok-pager-bin --bin grok-pi` → ok
  - shell workflow tests 74 ok / 1 pre-existing symlink fail
- **Open:** optional live deep-research handtest only
