/**
 * pi-grok-tree — Standalone TS `/tree` session branch explorer overlay.
 *
 * Replaces the heavy Rust SessionTree modal UI in `grok-pi` while keeping
 * Rust in charge of the authoritative `Action::NavigateSessionTree` RPC and
 * transcript reload (`__pi_grok_mention_insert__` -> `kind: "tree_navigate"`).
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import { buildFlattenedSessionTree, parseSessionTreeJsonl } from "./tree-data.ts";
import { createSessionTreeTui, type SessionTreeActionResult } from "./tree-tui.ts";

/** Must match `EXTENSION_MENTION_INSERT_STATUS_KEY` in pi-grok-adapter. */
const INSERT_STATUS_KEY = "__pi_grok_mention_insert__";
const TREE_COMMAND = "__pi_session_tree";

function isGrokPiHost(): boolean {
  return process.env.PI_GROK === "1";
}

function resolveSessionMeta(ctx: ExtensionCommandContext): { file: string; leafId?: string } {
  const sm = (
    ctx as {
      sessionManager?: {
        getSessionFile?: () => string | undefined;
        getLeafId?: () => string | undefined;
      };
    }
  ).sessionManager;
  return {
    file: sm?.getSessionFile?.() ?? "",
    leafId: sm?.getLeafId?.(),
  };
}

export default function treeExtension(pi: ExtensionAPI): void {
  pi.registerCommand(TREE_COMMAND, {
    description: "Internal grok-pi bridge for the bordered TS /tree session explorer",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const { file, leafId } = resolveSessionMeta(ctx);
      const parsed = parseSessionTreeJsonl(file, leafId);
      const nodes = buildFlattenedSessionTree(parsed.entries, parsed.leafId);

      if (nodes.length === 0) {
        ctx.ui.notify("No session tree entries to display yet", "info");
        return;
      }

      const initialQuery = String(args ?? "").trim();
      const result = await ctx.ui.custom<SessionTreeActionResult | undefined>(
        (tui, theme, _kb, done) =>
          createSessionTreeTui(
            tui as { requestRender: () => void },
            theme as TuiTheme,
            {
              nodes,
              initialQuery,
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
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

      if (!result) return;

      if (isGrokPiHost()) {
        const ui = ctx.ui as unknown as {
          setStatus?: (key: string, text?: string) => void;
        };
        try {
          ui.setStatus?.(
            INSERT_STATUS_KEY,
            JSON.stringify({
              kind: "tree_navigate",
              id: result.entryId,
              title: result.summarize ? "summarize" : "",
              insertText: "",
              nonce: Date.now(),
            }),
          );
          ui.setStatus?.(INSERT_STATUS_KEY, undefined);
        } catch {
          // RPC transport closed; ignore.
        }
      } else {
        await ctx.navigateTree(result.entryId, { summarize: result.summarize });
      }
    },
  });
}
