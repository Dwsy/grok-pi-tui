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
import type { RemoteTuiImageSpec, TuiTheme } from "../pi-grok-tui-kit/index.ts";
import type { ReviewLaunchMode, ReviewTuiResult } from "./review-tui.ts";

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
  const [
    { REMOTE_TUI_IMAGE_WIDGET_KEY },
    { aggregateSessionFiles, extractSessionReviewTurns, loadGitWorkingTreeChanges },
    { createReviewTui },
  ] = await Promise.all([
    import("../pi-grok-tui-kit/index.ts"),
    import("./review-data.ts"),
    import("./review-tui.ts"),
  ]);
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

  const isGrokPi = process.env.PI_GROK === "1";
  const patchUi = ctx.ui as unknown as {
    setWidget?: (key: string, lines?: string[]) => void;
  };
  const emitImageOverlay = (spec: RemoteTuiImageSpec | undefined) => {
    if (!isGrokPi || typeof patchUi.setWidget !== "function") return;
    try {
      patchUi.setWidget(
        REMOTE_TUI_IMAGE_WIDGET_KEY,
        spec ? [JSON.stringify(spec)] : undefined,
      );
    } catch {
      // RPC transport closed; ignore.
    }
  };

  let result: ReviewTuiResult | undefined;
  try {
    result = await ctx.ui.custom<ReviewTuiResult | undefined>(
      (tui, theme, _kb, done) =>
        createReviewTui(
          tui as { requestRender: () => void },
          theme as TuiTheme,
          {
            rootDir: cwd,
            mode,
            files,
            turns,
            onImageOverlay: isGrokPi ? emitImageOverlay : undefined,
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
          width: "90%",
          maxHeight: "90%",
          anchor: "center",
        },
      },
    );
  } finally {
    emitImageOverlay(undefined);
  }

  if (!result || !result.formattedFeedback) return;

  // Send collected review feedback back to the agent cleanly.
  try {
    pi.sendUserMessage(result.formattedFeedback);
  } catch {
    ctx.ui.notify(result.formattedFeedback, "info");
  }
}

async function openTraceOverlay(
  ctx: ExtensionCommandContext,
  options: { initialQuery?: string; initialToolCallId?: string },
): Promise<void> {
  const [{ parseSessionTraceJsonl }, { createTraceTui }] = await Promise.all([
    import("./trace-data.ts"),
    import("./trace-tui.ts"),
  ]);
  const sessionFile = resolveSessionJsonlPath(ctx);
  const entries = parseSessionTraceJsonl(sessionFile);
  if (entries.length === 0) {
    ctx.ui.notify("No JSONL trace entries in this session yet", "info");
    return;
  }

  await ctx.ui.custom<void>(
    (tui, theme, _kb, done) =>
      createTraceTui(
        tui as { requestRender: () => void },
        theme as TuiTheme,
        {
          entries,
          sessionFile,
          initialQuery: options.initialQuery,
          initialToolCallId: options.initialToolCallId,
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
        width: "90%",
        maxHeight: "90%",
        anchor: "center",
      },
    },
  );
}

async function openToolRenderOverlay(
  ctx: ExtensionCommandContext,
  toolCallId: string,
): Promise<void> {
  const [{ findToolInvocationInJsonl, createToolRenderTui }] = await Promise.all([
    import("./tool-render-tui.ts"),
  ]);
  const sessionFile = resolveSessionJsonlPath(ctx);
  const cwd = (ctx as { cwd?: string }).cwd || process.cwd();
  const invocation = findToolInvocationInJsonl(sessionFile, toolCallId);
  if (!invocation) {
    ctx.ui.notify(`Tool call ${toolCallId} not found in session JSONL`, "info");
    return;
  }
  await ctx.ui.custom<void>(
    (tui, theme, _kb, done) =>
      createToolRenderTui(
        tui as { requestRender: () => void },
        theme as TuiTheme,
        {
          invocation,
          cwd,
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
        width: "90%",
        maxHeight: "90%",
        anchor: "center",
      },
    },
  );
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
  pi.registerCommand("trace", {
    description: "Inspect full session JSONL trace (tools, messages, thinking, metadata)",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const query = String(args ?? "").trim();
      await openTraceOverlay(ctx, { initialQuery: query || undefined });
    },
  });

  // Hidden bridge commands invoked by Rust Pager (`dispatch/review.rs`, `dispatch/router.rs`)
  registerPair("__pi_review_session", "session", "Internal grok-pi bridge for /review-session");
  registerPair("__pi_review_message", "message", "Internal grok-pi bridge for /review-message");
  pi.registerCommand("__pi_trace", {
    description: "Internal grok-pi bridge for Left-arrow /trace JSONL viewer",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const callId = String(args ?? "").trim();
      await openTraceOverlay(ctx, { initialToolCallId: callId || undefined });
    },
  });
  pi.registerCommand("__pi_tool_render", {
    description: "Internal grok-pi bridge for custom Pi plugin tool renderer popup (F2 opt-in)",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const callId = String(args ?? "").trim();
      if (!callId) return;
      await openToolRenderOverlay(ctx, callId);
    },
  });
}
