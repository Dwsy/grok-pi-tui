# Herdr integration for grok-pi

`grok-pi` includes a default-on, headless Herdr lifecycle bridge. It needs no separate Pi integration install and is a silent no-op when the process is not running inside Herdr.

The bridge reports the native Pi session identity plus authoritative `working`, `blocked`, and `idle` lifecycle state through Herdr's local socket. It does not add another terminal UI and does not modify Pi or Herdr files in your home directory.

## Set up Herdr

1. Install Herdr:

   ```bash
   curl -fsSL https://herdr.dev/install.sh | sh
   ```

2. Start Herdr:

   ```bash
   herdr
   ```

3. Create or select a workspace and open a pane for the project.

4. Run `grok-pi` inside that pane:

   ```bash
   cd /path/to/project
   grok-pi
   ```

   When `HERDR_ENV=1` is already present, you are inside Herdr. Do not start a nested Herdr session.

   If grok-pi opens its Welcome screen, **enter or create a Pi session first**.
   The Herdr bridge reports the root interactive session's lifecycle; a
   prewarmed Pi process behind Welcome is not an active interactive session,
   so Welcome alone does not emit a session/idle report.

5. Verify the active pane from another shell or Herdr command pane:

   ```bash
   herdr agent get "$HERDR_PANE_ID"
   ```

   The detected agent should be `pi`; while a turn is running Herdr should show `working`, and after the root interactive session settles it should show `idle`.

## Enable or disable it

Open **F2 → Agent → Pi Herdr integration**.

- `on` is the default; it reports only inside Herdr.
- Set it to `off` to disable the built-in bridge.
- Restart the whole `grok-pi` process after changing it.

The equivalent config is:

```toml
[ui]
pi_herdr = true
```

Set it to `false` to disable it; removing the key restores the default `true`. `grok-pi --no-extensions` also skips this and every other bundled bridge extension for that process.

## Interaction with Herdr's stock Pi integration

Herdr can install a managed stock-Pi extension with `herdr integration install pi`. `grok-pi` does not require it. When the built-in bridge is active, the host skips only the auto-discovered Herdr-managed Pi file so two extensions cannot compete for the same authoritative `herdr:pi` lifecycle source. Explicit user `--extension` arguments remain untouched.

No global integration is removed or rewritten, so stock `pi` can continue using Herdr's managed integration outside `grok-pi`.

## Troubleshooting

- Check Herdr and its integrations:

  ```bash
  herdr --version
  herdr integration status
  ```

- Confirm the pane environment contains `HERDR_ENV=1`, `HERDR_SOCKET_PATH`, and `HERDR_PANE_ID`.
- Restart `grok-pi` after changing the F2 setting.
- If all bundled extensions were disabled with `--no-extensions`, start without that flag.
- The bridge fails closed: if the local socket is missing or unavailable, `grok-pi` continues normally without lifecycle reporting.
- Check `~/.grok-pi/logs/pi-rpc-stderr.log` for `[pi-grok-herdr]` socket/API failures. The bridge validates JSON responses and retries a bounded number of times.
- Run `herdr pane process-info --pane <PANE_ID>` and `herdr agent explain <PANE_ID> --json`. Full Header ownership requires `screen_detection_skipped=true` with `full_lifecycle_hook_authority`; a successful socket response alone does not prove Herdr accepted the lifecycle state.
- `herdr agent list` can identify Pi and display `idle` even when the
  detailed explanation still reports `default_known_agent_idle_fallback`.
  That means Herdr's screen-detection fallback is active, **not** that the
  built-in bridge has full lifecycle authority. Compare the same pane's
  `herdr agent explain <PANE_ID> --json` after an actual root Pi turn.
- For an offline local transport check on an ARM64 Mac with Pi installed, run
  `python3 scripts/pi-herdr-pty-smoke.py` from a repository build tree after
  producing `target/aarch64-apple-darwin/release-dist/grok-pi`. This launches
  real Pi JSONL RPC in an isolated temporary session and verifies the built-in
  bridge's session + idle messages against a **fake local Herdr Socket**. It
  neither changes Herdr nor contacts a model provider. It does **not** prove
  that real Herdr accepted working/blocked/idle states in an actual pane.
- To test the extension's state transitions without starting Pi or a model,
  run `node --test extensions/pi-grok-herdr/lifecycle.test.mjs` with Node 24.
  This executes the real extension against simulated lifecycle callbacks and
  a local Unix Socket, checking the full
  `idle → working → blocked → working → idle` sequence, ownership and
  monotonic event sequence. It is also part of the
  Pi-only CI.
- The lifecycle events must not be coalesced into a final `idle` during
  fast turns: the test also emits `working → blocked → working → idle`
  without awaiting socket replies and verifies every state was reported.
