# PDCA Loop

<!-- pdca:gen -->
> **status:** completed  ·  **phase:** P3  ·  **pdca:** act
> **validation:** passed  ·  **open decisions:** none
> **next:** Optional: handtest deep-research in grok-pi; no blocking work.
<!-- /pdca:gen -->

## Cycle 1

| Stage | Intent | Current Evidence |
| --- | --- | --- |
| Plan | Frame objective, constraints, acceptance, and decision criteria. | Research complete (session 2026-07-22): External path lacks SessionActor; `xai-workflow` is pub host-channel; shell workflow is pub(crate); Spawn hard-bound to Grok SubagentEvent; Pager `workflow_ingest` ready; subagents prove Pi child pattern. Architecture fixed: upstream engine + shell seam + Pi spawn backend only. Acceptance A1–A8 written in task.md. |
| Do | Execute the smallest tracer path that can produce evidence. | S1–S4 foundation landed: backend trait, ExternalRuntime, pi-grok-workflows inject, PiWorkflowAgentBackend. Tests green. |
| Check | Compare evidence against acceptance and risks. | Narrow tests + grok-pi check pass. A1–A4 partially met at unit level; A5 live UI + deep-research e2e residual. |
| Act | Close, continue, or pivot based on decision D2. | TBD |

## Operating Rule

Do not move linearly by habit. At each decision phase, choose an option and record rationale in `state.json.decision_points` through the state update script.

## Markdown Sync (agents)

After each Do/Check slice: update `implementation-log.md`, this table's evidence column, and run `update-pdca-state.mjs`. It refreshes generated status strips automatically; use `bun run render` only to adopt an older folder or recover after an interrupted write. Generated regions are derived from `state.json` and must never be hand-edited.
