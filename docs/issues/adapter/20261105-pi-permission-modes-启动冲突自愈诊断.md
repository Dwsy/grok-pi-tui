# Issue #11 — pi-permission-modes Bash registration and bootstrap self-heal

Status: implemented; focused Rust tests 8/8 passed (2026-11-05). Full cargo check was interrupted during unrelated dependency compilation.

## Reproduced root cause

- `pi-permission-modes` registers a sandboxed `bash` tool in `src/index.ts`.
- Grok-Pi enhanced Bash registers a separate `bash` tool in `extensions/pi-grok-bash/index.ts` while `PI_GROK_BASH=1` (default).
- Pi 1.4.0 and 1.5.0 treat the duplicate tool as a fatal extension-loading diagnostic **before** RPC starts. Both extension load orders were reproduced with a real `pi-permission-modes` source and a registration-only synthetic Grok Bash.
- The previous self-heal reported an upgrade/restart message instead of preserving the source error, and may have disabled the permission extension while bisecting.

## Change scope

`crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/extension_self_heal.rs`:

1. Preserve the initial `PiRpc` error, including its buffered child stderr; report the product-isolated stderr-log path.
2. Parse the `Tool "<name>" conflicts with ...` diagnostic before extension bisection. Treat duplicate tool ownership as **fail-closed**. Never silently remove a permission provider to recover startup.
3. For `bash` and `pi-permission-modes`, advise `PI_GROK_BASH=0 grok-pi` or `[ui] pi_bash = false` in `~/.grok-pi/config.toml`. Grok-Pi currently has no separate permission manager. This preserves Pi extension responsibility.
4. Other extension startup failures still use the existing no-extension probe, bisection, relaunch, and telemetry behavior.
5. The optional Grok-Pi update prompt defaults to **No**; no implicit upgrade on Enter, and tool registration conflicts do not offer an upgrade.
6. Include the original error when no extensions can be isolated or the relaunch fails.

## Safety and acceptance

- A successful `get_state` alone does not prove all command execution is sandboxed. Grok Bash, Eval, subagents and background tools require separate policy coverage review.
- Use the locally installed Pi 1.4.0 and 1.5.0 and a temporary `PI_CODING_AGENT_DIR` for reproducible, no-model startup checks. Verify the error parser with a Pi 1.4.0-style stderr fixture.
- Run `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi extension_self_heal::tests` and `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi`.
- Preserve all pre-existing uncommitted work, particularly unrelated self-heal crash telemetry edits.

## Follow-up outside this change

Add an explicit pre-launch Bash tool-owner resolver based on `PiResourceCatalog` so the user can retain a known permission extension without manually disabling Grok-Pi Bash; require full security acceptance first.
