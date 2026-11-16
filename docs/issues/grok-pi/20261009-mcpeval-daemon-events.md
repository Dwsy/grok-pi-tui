# mcpeval resident daemon + event mode

- Date: 2026-10-09
- Branch: main
- Scope: `extensions/pi-grok-bash/eval-pi-mcp.ts`, `extensions/pi-grok-bash/mcpeval-*.ts`, `extensions/pi-grok-bash/index.ts`
- Constraint: do not use Cargo; Eval remains in the live Pi process.
- Reference architecture: https://github.com/Xuanwo/agenvo (relay registry, connector registration, bounded event delivery, and resync semantics).

## Background

The old integration opened one secret MCP HTTP listener per Pi session. That makes discovery, reconnect, event delivery, and Herdr wakeups depend on a particular Pi process. The requested topology is one resident `mcpevald` daemon plus short-lived Pi registrations:

`MCP client -> loopback daemon -> private Unix socket -> live Pi Eval`.

A daemon is transport/registry infrastructure only. It must not start a second Pi or a second Eval runtime. A Pi shutdown unregisters its binding; the daemon stays available for later Pi sessions.

## Contract

- [x] Versioned `mcpeval/1` newline-delimited Unix-socket protocol.
- [x] Daemon state, lock, and socket under `$GROK_HOME/eval-pi-mcp` (0700/0600).
- [x] Loopback-only authenticated Streamable HTTP MCP endpoint.
- [x] Key-protected local dashboard (`/`) and JSON status endpoint (`/api/status`) for human/Herdr inspection.
- [x] MCP OAuth 2.1 provider using the official SDK 1.30 provider contracts: metadata, DCR, S256 PKCE, bearer verification, refresh and revocation; bootstrap key remains compatible.
- [x] Pi registration returns a unique binding ID and daemon URL; reconnect discovers current registrations.
- [x] `discover`, `search`, `get_context`, `get_desc`, and `execute_eval` forward into the registered Pi.
- [x] A Pi receives cancellation frames and aborts its local Eval request.
- [x] Daemon keeps running after Pi unregisters; a detached first-Pi startup path is available for Herdr/launchd style supervisors.
- [x] Event mode: `runtime.changed` payloads with event ID, timestamp, binding/instance ID, and registration epoch.
- [x] Event filters by service/thread/native type.
- [x] Bounded event queues: 64 per subscription and 256 global; overflow emits `agenvo.resync_required`.
- [x] Duplicate event IDs are ignored; consumers use current context/output after a resync or reconnect.
- [x] Portable MCP event tools: `events_subscribe`, `events_poll`, `events_unsubscribe`.
- [x] Agenvo-shaped `events/list`, `events/subscribe`, `events/unsubscribe` handlers are attempted when an SDK accepts custom schemas; current official SDK falls back to the portable tools.
- [x] Existing per-Pi `startEvalPiMcp` export remains as a compatibility API for focused regression tests; the Pi extension's production path uses registration/discovery only.

## Startup and lifecycle

- [x] `PI_GROK_EVAL_MCP=1` + Eval v2-only starts/ensures the resident daemon, then registers the current Pi.
- [x] A later Pi reads the same daemon descriptor and registers over the Unix socket.
- [x] Daemon child is detached and ignores Pi session shutdown.
- [x] Herdr can wake/start Pi; Pi registration reconnects to the resident daemon.
- [ ] Add a dedicated launchd/Herdr service definition if the deployment wants daemon startup independent of the first Pi. The current implementation's first-Pi path is the fallback.

## Validation

- `node extensions/pi-grok-bash/test-eval-pi-mcp.mjs`
- `node extensions/pi-grok-bash/test-mcpeval-daemon.mjs` (daemon, dashboard and OAuth discovery)
- `node extensions/pi-grok-bash/test-mcpeval-oauth.mjs` (OAuth provider flow)
- Node/esbuild bundle check for `extensions/pi-grok-bash/index.ts` and all split mcpeval modules
- Embedded extension materialization assertions for the split module list
- Auto-start smoke test through `ensureMcpevalDaemon`
- No Cargo command was used.
