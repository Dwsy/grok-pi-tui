/**
 * Working-message mirror for grok-pi.
 *
 * Pi's RPC mode implements `ctx.ui.setWorkingMessage()` as a no-op:
 *
 *   setWorkingMessage(_message?: string): void {
 *       // Working message not supported in RPC mode - requires TUI loader access
 *   }
 *
 * grok-pi drives Pi over JSONL RPC, so anything an extension puts there (a
 * whimsical quote, a progress line, …) is silently dropped. The Pager's turn
 * status row has its own activity labels and never sees it.
 *
 * This extension bridges the gap without touching Pi or its extensions: it
 * wraps `ctx.ui.setWorkingMessage` and republishes the text through
 * `ctx.ui.setStatus()` under a private key. The adapter translates that key to
 * `pi/ui/working_message`, and the Pager may render it in place of selected
 * `TurnActivity` labels (Thinking / ToolRunning / WritingToolCall / Waiting),
 * gated by the F2 `pi_working_message` toggle.
 *
 * The original call is always preserved, so native Pi keeps its own behavior
 * and no upstream code depends on this shim.
 *
 * Only installs in a grok-pi child (`PI_GROK=1`); a plain `pi` process is
 * untouched.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** Must match `EXTENSION_WORKING_MESSAGE_STATUS_KEY` in pi-grok-adapter. */
const STATUS_KEY = "__pi_grok_working_message__";

type PatchableUi = {
  setWorkingMessage?: (message?: string) => void;
  setStatus?: (key: string, text?: string) => void;
};

const PATCH_MARK = "__piGrokWorkingMessageMirror";

type PatchedSetWorkingMessage = ((message?: string) => void) & {
  [PATCH_MARK]?: boolean;
};

/** Track patched uiContext objects; Pi rebinds `ctx.ui` after session start. */
const patchedUIs = new WeakSet<object>();

function shouldInstall(): boolean {
  return process.env.PI_GROK === "1";
}

/**
 * Wrap `ui.setWorkingMessage` so every call also reaches `ui.setStatus`.
 *
 * Idempotent per uiContext: Pi swaps the object on reload, and the WeakSet
 * plus the function mark keep a rebound object patchable exactly once.
 */
export function installWorkingMessageMirror(ui: PatchableUi): void {
  if (!shouldInstall()) return;
  if (patchedUIs.has(ui as object)) return;

  const original = ui.setWorkingMessage?.bind(ui);
  const patch = ((message?: string) => {
    // A broken mirror must never break the extension that called us: both the
    // native setter and our republish are isolated.
    try {
      original?.(message);
    } catch {
      // Native Pi may reject an invalid message; keep forwarding our copy.
    }
    if (typeof ui.setStatus !== "function") return;
    try {
      // `undefined` clears the row, matching setWorkingMessage() semantics.
      ui.setStatus(STATUS_KEY, message);
    } catch {
      // RPC transport closed mid-turn; nothing else we can do here.
    }
  }) as PatchedSetWorkingMessage;

  patch[PATCH_MARK] = true;
  ui.setWorkingMessage = patch;
  patchedUIs.add(ui as object);
}

export default function (pi: ExtensionAPI) {
  // `session_start` fires once the uiContext is live; the command path re-runs
  // the patch because Pi may rebind `ctx.ui` on reload or resume.
  pi.on("session_start", (_event, ctx) => {
    installWorkingMessageMirror(ctx.ui as PatchableUi);
  });

  // Expose the installer for host-side re-binding (mirrors the Remote TUI
  // `__piGrokEnsureRemoteTuiHost` convention), so other grok-pi surfaces that
  // swap uiContext can restore the mirror without importing this module.
  (globalThis as typeof globalThis & {
    __piGrokEnsureWorkingMessageMirror?: typeof installWorkingMessageMirror;
  }).__piGrokEnsureWorkingMessageMirror = installWorkingMessageMirror;
}
