# Decisions

<!-- pdca:gen -->
| ID | Option | Label | Status | Decided |
| --- | --- | --- | --- | --- |
| D1 | split | Split into slices | decided | 2026-07-21 |
| D2 | continue | Continue same cycle | decided | 2026-07-21 |
<!-- /pdca:gen -->

## Decision Log

| ID | Phase | Question | Status | Selected | Rationale | Decided |
|----|-------|----------|--------|----------|-----------|---------|
| D1 | D1 | Which route should the next implementation take? | open | — | — | — |
| D2 | D2 | After Check, should this cycle close, pivot, or continue? | open | — | — | — |

## D1 — Implementation Route

**Question:** Which route should the next implementation take?

| Option | Label | Next Phase | Criteria |
|--------|-------|------------|----------|
| direct | Direct implementation | P2 | Scope is clear, blast radius is small, and validation path is known. |
| research | Research first | P0 | Domain facts, dependencies, or ownership are still unclear. |
| split | Split into slices | P1 | The work is valid but too broad for one safe change. |

**Pre-assessment (from P0 research):**
- Scope is well-defined (5 layers, clear file targets)
- Blast radius is contained (pi-grok-adapter only + one Pi extension file)
- Validation path known (`cargo test -p pi-grok-adapter` + `./verify.sh`)
- BUT: 5 layers is too broad for one safe change → **recommend `split`**
- Proposed slices: (1) state machine + prompt injection, (2) tool gate extension, (3) exit flow + approval, (4) persistence + projection

## D2 — Close / Continue / Pivot

**Question:** After Check, should this cycle close, pivot, or continue?

| Option | Label | Next Phase | Criteria |
|--------|-------|------------|----------|
| close | Close cycle | P3 | Acceptance evidence is complete and no important risk remains. |
| continue | Continue same cycle | P1 | Core direction is right but more execution is required. |
| pivot | Pivot plan | P0 | Validation changed the problem or invalidated current assumptions. |
