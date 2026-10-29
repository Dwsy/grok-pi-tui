# grok-pi startup: defer Pi RPC bootstrap behind Pager first paint

## Problem

`grok-pi` currently awaits `spawn_with_extension_self_heal()` and `PiBootstrap::load()` before calling `xai_grok_pager::app::run_external()`. With the normal extension set this leaves the user's terminal unchanged while Pi RPC and extensions initialize.

Observed by the user on the same machine:

- `pig`: about 8s before entry
- `pig -ne`: about 3s
- `grok-pi -ne --no-bridge-extensions`: about 2s

The no-extension result establishes that extension/RPC bootstrap dominates the avoidable delay.

## Goal

Enter the native Grok Pager terminal surface before Pi RPC/bootstrap finishes, then wait for Pi readiness inside the Pager-owned startup surface. Preserve Pi as the only agent core, Grok Pager as the only terminal UI, extension self-heal semantics, session selection, model/command bootstrap, and existing exit/resume behavior.

## Constraints

- Do not create a second TUI or an ASCII fallback.
- Do not fake Pi state before bootstrap completes.
- Do not remove extension self-heal or weaken its timeout/error behavior.
- Keep the external ACP adapter headless.
- Keep existing `run_external` behavior available; add the narrowest deferred-connect seam needed by grok-pi.

## Verification

1. Startup surface is entered before the deferred Pi bootstrap completes.
2. Normal `grok-pi` still reaches a working Pi session with models and commands populated after bootstrap.
3. Bootstrap failure still exits cleanly after restoring the terminal.
4. Focused Pager/bin checks pass; full verification blockers remain reported separately.

## Phase log

### 2026-10-29 — Phase 2: first paint before Pi bootstrap (deferred attach)

`run_external_deferred` already entered the terminal early but still blocked on
`ready.await` before the event loop, so the user saw a black alt-screen for the
whole Pi RPC bootstrap (up to several seconds with the normal extension set).
Phase 2 removes that wait for the fresh-start path.

Design: the ACP channel pair is created before the backend. The Pager paints
its native Welcome surface immediately; the Pi child (spawn + extension
self-heal + bootstrap + `PiAgent` wiring) attaches behind it and drains queued
requests in order. Models/commands/session identity arrive through the
existing adapter flows (`x.ai/models/update`, `AvailableCommandsUpdate`,
`pi/ui/title` from `publish_bootstrap` on the prewarm `session/new`), so no Pi
state is faked before bootstrap completes.

Changes:

- `grok-pi.rs`: `attach_pi_backend()` (spawn + self-heal + adapter wiring,
  shared by both paths) and `PiAttachSpec`. Fresh starts (`NewAuto`) build
  `AcpConnection::external` immediately (empty model/command seed) and signal
  completion through `ExternalRunReady::backend_attach`.
- `xai-grok-pager/src/app/mod.rs`: `ExternalRunReady.backend_attach` (optional
  oneshot); `run_external_deferred` drives it beside the event loop. `Err`
  cancels the connection and is returned after terminal teardown (fail fast).
  Teardown log flush is skipped (fire-and-forget instead) when no agent-side
  router is alive to answer, so a failed/incomplete attach cannot hang exit.
- `extension_self_heal.rs`: `interactive` flag gates the upgrade/crash-report
  stdin prompts. Background attach passes `false` — a live TUI must never have
  keystrokes stolen or prompts painted over.

Gap pass (same day):

- Host notifications (`x.ai/models/update` model-catalog seed at attach,
  `pi/ui/notify` self-heal notice) are published **only** on the deferred
  path. `acp_send` awaits a response from the Pager event loop, which does not
  exist yet during the blocking attach — publishing there would deadlock
  `ready.await`. `attach_pi_backend`'s `behind_live_tui` flag owns this gate
  together with the stdin-prompt gate.
- The model-catalog seed closes the empty-status-line window in every fresh
  mode and covers a Welcome held by the folder-trust question (where the
  prewarm `session/new` is deferred).
- The self-heal notice closes the observability hole where the bisection
  report is invisible (stderr is redirected while the Pager owns the
  terminal): `pi/ui/notify` with `notifyType: "info"` queues into the first
  agent scrollback.

Scope decisions (user-confirmed):

- Resume paths (`--continue` / `--session` / `--session-id` / `--fork`) keep
  the blocking attach: `MaterializedStartup::Resume` needs the real Pi session
  id before the event loop starts. Deferred resume needs a separate
  deferred-`LoadSession` design (follow-up).
- Verification ran without cargo (user constraint: disk pressure). Evidence:
  `rustfmt --edition 2024 --check` parses all touched files with zero diffs on
  the changed regions; `verify_native_grok.py` FAIL count unchanged (8, all
  pre-existing baseline drift). **Not compile-verified** — run
  `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi` and the
  focused Pager tests before release.

Follow-ups:

- Deferred attach for resume/fork (deferred `LoadSession` after attach).
- Optional: seed `AvailableCommandsUpdate` pre-session so slash autocomplete
  carries Pi skills in the short window before the first `session/new`
  completes (needs an app-level route; the update is session-scoped today).
