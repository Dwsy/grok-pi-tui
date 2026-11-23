/**
 * pi-grok-review — Lightweight standalone TS extension for `/review-session`
 * and `/review-message`.
 *
 * Replaces the sprawling Rust review keymap with a clean, bordered two-pane TUI
 * that shares `file-data.ts` (Nerd Font icons & file preview) from
 * `pi-grok-mentions`.
 *
 * Works in both `grok-pi` (via hidden bridge commands invoked by the Pager) and
 * standalone `pi` (via `/review-session` and `/review-message`).
 */

import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import {
  aggregateSessionFiles,
  extractSessionReviewTurns,
  loadGitWorkingTreeChanges,
} from "./review-data.ts";
import { createReviewTui, type ReviewLaunchMode, type ReviewTuiResult } from "./review-tui.ts";

function resolveSessionJsonlPath(ctx: ExtensionCommandContext): string {
  const sm = (ctx as { sessionManager?: { getSessionFile?: () => string | undefined } })
    .sessionManager;
  return sm?.getSessionFile?.() ?? "";
}

async function openReviewOverlay(
  pi: ExtensionAPI,
  ctx: ExtensionCommandContext,
  mode: ReviewLaunchMode,
): Promise<void> {
  const cwd = (ctx as { cwd?: string }).cwd || process.cwd();
  const sessionFile = resolveSessionJsonlPath(ctx);
  const turns = extractSessionReviewTurns(sessionFile, cwd);
  let files = aggregateSessionFiles(turns);

  if (mode === "session" && files.length === 0) {
    files = loadGitWorkingTreeChanges(cwd);
  }
  if (mode === "message" && turns.length === 0) {
    ctx.ui.notify("No edited messages to review in this session", "info");
    return;
  }
  if (mode === "session" && files.length === 0) {
    ctx.ui.notify("No file changes to review in this session", "info");
    return;
  }

  const result = await ctx.ui.custom<ReviewTuiResult | undefined>(
    (tui, theme, _kb, done) =>
      createReviewTui(
        tui as { requestRender: () => void },
        theme as TuiTheme,
        {
          rootDir: cwd,
          mode,
          files,
          turns,
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

  if (!result || !result.formattedFeedback) return;

  // Send collected review feedback back to the agent cleanly.
  try {
    pi.sendUserMessage(result.formattedFeedback);
  } catch {
    ctx.ui.notify(result.formattedFeedback, "info");
  }
}

export default function reviewExtension(pi: ExtensionAPI): void {
  const registerPair = (name: string, mode: ReviewLaunchMode, description: string) => {
    pi.registerCommand(name, {
      description,
      handler: async (_args: string, ctx: ExtensionCommandContext) => {
        await openReviewOverlay(pi, ctx, mode);
      },
    });
  };

  // Public commands (standalone Pi & direct invocation)
  registerPair("review-session", "session", "Review file changes made in this session");
  registerPair("review-message", "message", "Review file changes grouped by message turn");

  // Hidden bridge commands invoked by Rust Pager (`dispatch/review.rs`)
  registerPair("__pi_review_session", "session", "Internal grok-pi bridge for /review-session");
  registerPair("__pi_review_message", "message", "Internal grok-pi bridge for /review-message");
}
