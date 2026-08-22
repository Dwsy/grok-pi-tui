/**
 * Experimental Remote TUI — no Pi source patches.
 *
 * Enabled by default from grok-pi (child gets PI_GROK_REMOTE_TUI=1).
 * Disable host process with PI_GROK_REMOTE_TUI=0.
 * 1. On session_start, monkey-patch ctx.ui.custom to run factories in-process.
 * 2. Project frames via existing ctx.ui.setWidget("remote_tui", lines).
 * 3. Keys arrive through a temp keyfile written by the adapter (not Pi RPC).
 *
 * Usage: /remote-tui
 *
 * Demo: multi-select list → Enter applies native surfaces
 * (header/footer widgets, status, title, editor text).
 *
 * Module map (all materialized by the Rust injector `remote_tui_extension.rs`):
 * - `shared.ts`    — adapter/Pager-contract constants and wire types
 * - `env.ts`       — host detection + Pi theme bootstrap
 * - `layout.ts`    — Pi custom() layout/viewport mirroring
 * - `transport.ts` — keyfile transport and adapter meta
 * - `host.ts`      — custom() component host patch
 * - `demo.ts`      — capability-lab selector and surface application
 * - `index.ts`     — entry point: session_start hook + /remote-tui command
 */
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { ensureRemoteTuiHost, installCustomPatch } from "./host.ts";
import { shouldInstallRemoteHost } from "./env.ts";
import type { RemoteTuiDemoUi } from "./shared.ts";
import { applyDemoCapabilities, createDemoSelector } from "./demo.ts";

// Re-exported for the bun test suite and external consumers; the split keeps
// ./index.ts as the single public import surface.
export { DEMO_ITEMS, applyDemoCapabilities, createDemoSelector } from "./demo.ts";
export type { DemoKey } from "./demo.ts";

export default function (pi: ExtensionAPI) {
  pi.on("session_start", (_event, ctx) => {
    ensureRemoteTuiHost(ctx.ui as Parameters<typeof installCustomPatch>[0]);
  });

  pi.registerCommand("remote-tui", {
    description: "[experimental] Remote TUI capability lab with selectable widgets",
    handler: async (_args: string, ctx: ExtensionCommandContext) => {
      // grok-pi RPC only: re-bind host after uiContext swaps. Native TUI: leave custom alone.
      ensureRemoteTuiHost(ctx.ui as Parameters<typeof installCustomPatch>[0]);

      const started = Date.now();
      let factoryRan = false;
      const openDemo = () =>
        ctx.ui.custom<string | undefined>((tui, theme, _kb, done) => {
          factoryRan = true;
          return createDemoSelector(
            tui as { requestRender: () => void },
            theme as {
              fg: (color: string, text: string) => string;
              bold?: (text: string) => string;
            },
            done,
            (keys) => applyDemoCapabilities(ctx.ui as RemoteTuiDemoUi, keys),
          );
        });

      const result = await openDemo();

      const elapsed = Date.now() - started;
      if (result === undefined && !factoryRan) {
        if (shouldInstallRemoteHost()) {
          installCustomPatch(ctx.ui as Parameters<typeof installCustomPatch>[0]);
          const retry = await openDemo();
          if (retry !== undefined || factoryRan) {
            if (retry === undefined) ctx.ui.notify("Remote TUI demo closed", "info");
            else ctx.ui.notify(`Remote TUI demo applied: ${retry}`, "info");
            return;
          }
          ctx.ui.notify(
            "Remote TUI host patch failed: custom() stub still active (rebuild grok-pi)",
            "error",
          );
          return;
        }
        ctx.ui.notify(
          "custom() unavailable (RPC without remote host). Run under grok-pi or native Pi TUI.",
          "error",
        );
      } else if (result === undefined && elapsed < 80) {
        ctx.ui.notify("Remote TUI cancelled immediately", "warning");
      } else if (result === undefined) {
        ctx.ui.notify("Remote TUI demo closed", "info");
      } else {
        ctx.ui.notify(`Remote TUI demo applied: ${result}`, "info");
      }
    },
  });
}
