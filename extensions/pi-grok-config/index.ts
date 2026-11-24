/**
 * pi-grok-config — Standalone TS renderer for `/pi-config` resource management.
 *
 * Rust (`PiResourceCatalog`) owns discovery, trust checks, and `settings.json`
 * persistence; this extension simply renders the Rust JSON snapshot and posts
 * mutations back over `__pi_grok_mention_insert__` (`kind: "pi_config_mutate"`).
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import type { PiConfigMutationPayload } from "./config-data.ts";

/** Must match `EXTENSION_MENTION_INSERT_STATUS_KEY` in pi-grok-adapter. */
const INSERT_STATUS_KEY = "__pi_grok_mention_insert__";
const CONFIG_COMMAND = "__pi_config_manager";

export default function configExtension(pi: ExtensionAPI): void {
  pi.registerCommand(CONFIG_COMMAND, {
    description: "Internal grok-pi bridge for the bordered TS /pi-config resource manager",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const [{ loadPiConfigSnapshot }, { createPiConfigTui }] = await Promise.all([
        import("./config-data.ts"),
        import("./config-tui.ts"),
      ]);
      const snapshotPath = String(args ?? "").trim();
      const snapshot = loadPiConfigSnapshot(snapshotPath);
      if (!snapshot) {
        ctx.ui.notify("Pi resource catalog snapshot unavailable", "warning");
        return;
      }

      const ui = ctx.ui as unknown as {
        setStatus?: (key: string, text?: string) => void;
      };
      const emitMutation = (payload: PiConfigMutationPayload) => {
        try {
          ui.setStatus?.(
            INSERT_STATUS_KEY,
            JSON.stringify({
              kind: "pi_config_mutate",
              id: payload.path,
              title: payload.resourceType,
              insertText: JSON.stringify(payload),
              nonce: Date.now(),
            }),
          );
          ui.setStatus?.(INSERT_STATUS_KEY, undefined);
        } catch {
          // RPC transport closed; ignore.
        }
      };

      await ctx.ui.custom<void>(
        (tui, theme, _kb, done) =>
          createPiConfigTui(
            tui as { requestRender: () => void },
            theme as TuiTheme,
            {
              snapshot,
              onMutate: emitMutation,
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            () => done(undefined),
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );
    },
  });
}
