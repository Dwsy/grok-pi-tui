# Decision Points

## D1: Implementation Route

Question: Which route should the next implementation take?

Options:

| id | Label | Meaning |
| --- | --- | --- |
| `direct` | Direct implementation | Single PR; scope small enough |
| `research` | Research first | Facts still missing |
| `split` | Split into slices | **Recommended:** S1→S5 seams; full compat is multi-crate |

Research already established:

- External grok-pi has **no** `SessionActor` → no stock `WorkflowManager` lifecycle
- `xai-workflow` is **public** and host-channel based → correct reuse point
- shell `session/workflow/*` is **`pub(crate)`** → needs narrow export / spawn trait
- Spawn today is hard-wired to Grok `SubagentEvent::Spawn`
- Pager already ingests `workflow_updated`
- Pi extension can own `createAgentSession` (proven by subagents) but **must not** reimplement Rhai

## D2: Cycle Outcome

Question: After Check, should this cycle close, pivot, or continue?

- Close cycle: acceptance evidence complete
- Continue same cycle: more slices remain
- Pivot plan: e.g. fork_context or ExternalRuntime approach invalidated

## D0 (architecture, closed in research)

| Topic | Choice | Rationale |
| --- | --- | --- |
| Engine | **Use `xai-workflow` only** | Full `.rhai` compatibility; no TS engine |
| Orchestration | **Reuse shell manager path via narrow seam** | Avoid logic drift |
| Spawn | **Pi backend only new code** | Pi is agent core on grok-pi |
| UI | **Native Pager `workflow_updated`** | No second dashboard |
| Extension role | **Spawn executor + slash/tool front** | Not a second workflow runtime |

## Decision Log

<!-- pdca:gen -->
| ID | Option | Label | Status | Decided |
| --- | --- | --- | --- | --- |
| D1 | split | Split into slices | decided | 2026-07-22 |
| D2 | close | Close cycle | decided | 2026-07-22 |
<!-- /pdca:gen -->
