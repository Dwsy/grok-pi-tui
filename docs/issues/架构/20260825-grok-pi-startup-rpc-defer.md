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
