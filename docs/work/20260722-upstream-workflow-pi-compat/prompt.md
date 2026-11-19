# Restart Prompt

Continue `work/20260722-upstream-workflow-pi-compat/` as a PDCA goal loop.

## Read order

1. `work/20260722-upstream-workflow-pi-compat/state.json` → `next_action`, open decisions
2. `work/20260722-upstream-workflow-pi-compat/implementation-log.md` (last section)
3. `work/20260722-upstream-workflow-pi-compat/task.md` (architecture + acceptance)
4. `work/20260722-upstream-workflow-pi-compat/pdca.md`
5. `work/20260722-upstream-workflow-pi-compat/decisions.md`
6. `work/20260722-upstream-workflow-pi-compat/history/events.jsonl`

## Repo SSOT (code)

| Concern | Path |
| --- | --- |
| Engine | `crates/codegen/xai-workflow/` |
| Shell host/manager | `crates/codegen/xai-grok-shell/src/session/workflow/` |
| Builtin Rhai | `crates/codegen/xai-grok-shell/src/session/workflows/deep_research.rhai` |
| Pager ingest | `crates/codegen/xai-grok-pager/src/app/acp_handler/workflow_ingest.rs` |
| Pi child pattern | `extensions/pi-grok-subagents/index.ts` |
| Adapter | `crates/codegen/pi-grok-adapter/` |
| Invariants | `AGENTS.md`, `FEATURE_MATRIX.md` |

## Architecture lock (do not reopen without D2 pivot)

1. **Full compat = upstream `xai-workflow` + same `.rhai`** — no TS engine  
2. **Only new backend = SpawnAgent → Pi**  
3. **UI = existing Pager `workflow_updated` path**  
4. **Extension = spawn executor**, not workflow director  

## Rules

- Keep `state.json` current through `update-pdca-state.mjs`.
- Never overwrite `state.json` without archiving the previous version.
- Do not move past a decision point without recording selected option and rationale.
- Do not mark complete until acceptance evidence is verified.
- After implementation: sync `implementation-log.md`; `update-pdca-state.mjs` refreshes generated regions.
- Extensions-only changes: plugin static check + diff; do not run Cargo unless Rust/loader changed or user asks.
- Rust changes: narrowest `cargo test` / `cargo check`, read exit code, then report.

## Current Progress

<!-- pdca:gen -->
**Phase:** P3 (act)
**Status:** completed
**Next:** Optional: handtest deep-research in grok-pi; no blocking work.
<!-- /pdca:gen -->
