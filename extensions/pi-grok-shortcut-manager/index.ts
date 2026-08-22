/**
 * pi-grok-shortcut-manager
 *
 * Bridges Pi extension shortcuts into grok-pi's Remote TUI key dispatch path.
 *
 * The Rust injector materializes this module and its relative-import closure
 * as one temporary bundle; see `shortcut_manager_extension.rs`.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { registerShortcutCommand } from "./commands.ts";
import { ensureDispatchChannel, dispatchByKeyId } from "./dispatch.ts";
import {
  installGlobalIntercept,
  installRunnerCapture,
  publishRegistryToHost,
  refreshRegistry,
} from "./host.ts";
import { state } from "./shared.ts";

export default function (pi: ExtensionAPI): void {
  void installRunnerCapture();
  installGlobalIntercept();
  // Optional legacy keyfile channel (kept as fallback; primary is RPC prompt).
  ensureDispatchChannel();

  // Hidden bridge command: adapter → RPC prompt → runHandler.
  // Not for user slash UI (filtered by adapter is_bridge_command).
  pi.registerCommand("__pi_shortcut_dispatch", {
    description: "Internal: dispatch extension shortcut by KeyId",
    handler: async (args, ctx) => {
      state.latestCtx = ctx;
      refreshRegistry();
      const key = args.trim().split(/\s+/)[0] ?? "";
      if (!key) {
        ctx.ui.notify("shortcut dispatch: empty key", "warning");
        return;
      }
      if (!dispatchByKeyId(key)) {
        ctx.ui.notify(`shortcut dispatch: no handler for ${key}`, "warning");
      }
    },
  });

  // Keep ctx fresh for handler dispatch + sync list to native Rust modal.
  pi.on("session_start", (_event, ctx) => {
    state.latestCtx = ctx;
    ensureDispatchChannel();
    refreshRegistry();
    publishRegistryToHost(ctx.ui);
    // Later turn_start or bridge dispatch refreshes extensions that load after session_start.
    // Timers must not retain this ctx: Pi invalidates it on session replacement.
  });

  pi.on("turn_start", (_event, ctx) => {
    state.latestCtx = ctx;
    refreshRegistry();
    publishRegistryToHost(ctx.ui);
  });

  registerShortcutCommand(pi);
}
