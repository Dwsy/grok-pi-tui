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
const EXTENSIONS_MODAL_COMMAND = "__pi_extensions_modal";

function collectLiveMcpServers(pi: ExtensionAPI) {
  const api = pi as unknown as {
    getAllTools?: () => Array<{ name?: string; description?: string }>;
  };
  const tools = typeof api.getAllTools === "function" ? api.getAllTools() : [];
  const byServer = new Map<string, { id: string; name: string; endpoint: string; transport: string; tools: Array<{ name: string; description: string }> }>();
  for (const t of tools) {
    const rawName = String(t?.name ?? "");
    const match = /^mcp__([^_]+)__(.+)$/.exec(rawName);
    if (!match) continue;
    const server = match[1]!;
    const toolName = match[2]!;
    let rec = byServer.get(server);
    if (!rec) {
      rec = { id: server, name: server, endpoint: "pi-mcp", transport: "mcp", tools: [] };
      byServer.set(server, rec);
    }
    rec.tools.push({ name: toolName, description: String(t?.description ?? "") });
  }
  return [...byServer.values()].sort((a, b) => a.name.localeCompare(b.name));
}

export default function configExtension(pi: ExtensionAPI): void {
  const makeEmitMutation = (ctx: ExtensionCommandContext) => {
    const ui = ctx.ui as unknown as {
      setStatus?: (key: string, text?: string) => void;
    };
    return (payload: PiConfigMutationPayload) => {
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
  };

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

      const emitMutation = makeEmitMutation(ctx);

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

  pi.registerCommand(EXTENSIONS_MODAL_COMMAND, {
    description: "Internal grok-pi bridge for the bordered TS Extensions / Plugins modal",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const [{ loadPiConfigSnapshot }, { createExtensionsModalTui, mapRustTabName }] =
        await Promise.all([
          import("./config-data.ts"),
          import("./extensions-modal-tui.ts"),
        ]);
      const raw = String(args ?? "").trim();
      const sep = raw.indexOf("|");
      const tabRaw = sep >= 0 ? raw.slice(0, sep) : "plugins";
      const snapshotPath = sep >= 0 ? raw.slice(sep + 1).trim() : raw;
      const snapshot = loadPiConfigSnapshot(snapshotPath);
      if (!snapshot) {
        ctx.ui.notify("Extensions snapshot unavailable", "warning");
        return;
      }

      const emitMutation = makeEmitMutation(ctx);
      const mcpServers = collectLiveMcpServers(pi);
      const initialTab = mapRustTabName(tabRaw);

      await ctx.ui.custom<void>(
        (tui, theme, _kb, done) =>
          createExtensionsModalTui(
            tui as { requestRender: () => void },
            theme as TuiTheme,
            {
              snapshot,
              mcpServers,
              initialTab,
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
            maxHeight: "90%",
            anchor: "center",
          },
        },
      );
    },
  });
}
