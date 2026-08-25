/**
 * grok-pi RPC compatibility facade (no Pi source edits).
 *
 * 1. Opt-in: present Remote TUI host as `tui` mode to third-party extensions
 *    (`PI_GROK_EXTENSION_TUI_COMPAT=1`).
 * 2. Always (when this extension loads): capture ExtensionRunner, snapshot
 *    extension `getArgumentCompletions("")` results, and enrich `get_commands`
 *    RPC stdout so Pager can render arg dropdowns (e.g. /gapp list|open|…).
 *
 * Pi stays in JSONL RPC. All patches are runtime host-module hooks.
 *
 * IMPORTANT: Do NOT reassign ESM named exports like `writeRawStdout` — Node
 * freezes them (`Cannot redefine property`). Intercept by wrapping
 * `process.stdout.write` then re-running `takeOverStdout()` so rpc-mode's
 * private raw writer points at our wrap.
 */

import { basename, dirname } from "node:path";
import { pathToFileURL } from "node:url";
import { realpathSync } from "node:fs";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

type ArgCompletion = { value: string; label: string; description?: string };

type ExtensionCommand = {
  invocationName: string;
  name?: string;
  getArgumentCompletions?: (
    argumentPrefix: string,
  ) => ArgCompletion[] | null | Promise<ArgCompletion[] | null>;
};

type ExtensionRunnerLike = {
  setUIContext: (uiContext: unknown, mode?: string) => void;
  getRegisteredCommands: () => ExtensionCommand[];
  getCommand?: (name: string) => ExtensionCommand | undefined;
};

type ExtensionRunnerConstructor = {
  prototype: ExtensionRunnerLike & {
    __piGrokTuiModeFacade?: boolean;
    __piGrokRunnerCapture?: boolean;
    __piGrokGetCommandsCapture?: boolean;
  };
};

type OutputGuardModule = {
  takeOverStdout?: () => void;
  restoreStdout?: () => void;
  isStdoutTakenOver?: () => boolean;
};

const PROCESS_MARK = "__piGrokGetCommandsStdoutWrap" as const;
const QUEUE_STATUS_KEY = "__pi_grok_queue_enqueue__" as const;

/** invocationName → empty-prefix completions (sync providers + settled async). */
const completionCache = new Map<string, ArgCompletion[]>();

function hostUrl(relativePath: string): string {
  const entryDir = dirname(realpathSync(process.argv[1]!));
  if (
    basename(entryDir) === "bundle" &&
    (relativePath === "core/extensions/runner.js" || relativePath.startsWith("modes/interactive/components/"))
  ) {
    return new URL("index.js", pathToFileURL(`${entryDir}/`)).href;
  }
  const hostDistDir = basename(entryDir) === "bundle" ? dirname(entryDir) : entryDir;
  return new URL(relativePath, pathToFileURL(`${hostDistDir}/`)).href;
}

function normalizeCompletions(items: ArgCompletion[]): ArgCompletion[] {
  return items.map((item) => ({
    value: item.value,
    label: item.label,
    ...(item.description ? { description: item.description } : {}),
  }));
}

/** Snapshot completions for one command; sync fills cache immediately. */
function snapshotCommandCompletions(command: ExtensionCommand): void {
  const getCompletions = command.getArgumentCompletions;
  if (!getCompletions) return;
  const name = command.invocationName;
  try {
    const result = getCompletions("");
    if (result && typeof (result as Promise<unknown>).then === "function") {
      void (result as Promise<ArgCompletion[] | null>)
        .then((items) => {
          if (Array.isArray(items) && items.length > 0) {
            completionCache.set(name, normalizeCompletions(items));
          }
        })
        .catch(() => {});
      return;
    }
    if (Array.isArray(result) && result.length > 0) {
      completionCache.set(name, normalizeCompletions(result));
    }
  } catch {
    // Completions are best-effort.
  }
}

function snapshotAllCompletions(runner: ExtensionRunnerLike): void {
  try {
    for (const command of runner.getRegisteredCommands()) {
      snapshotCommandCompletions(command);
    }
  } catch {
    // ignore
  }
}

async function loadExtensionRunnerPrototype(): Promise<
  | (ExtensionRunnerLike & {
      __piGrokTuiModeFacade?: boolean;
      __piGrokRunnerCapture?: boolean;
      __piGrokGetCommandsCapture?: boolean;
    })
  | null
> {
  const module = (await import(hostUrl("core/extensions/runner.js"))) as {
    ExtensionRunner?: ExtensionRunnerConstructor;
  };
  return module.ExtensionRunner?.prototype ?? null;
}

async function installRunnerHooks(): Promise<void> {
  const prototype = await loadExtensionRunnerPrototype();
  if (!prototype) {
    throw new Error("Pi ExtensionRunner is unavailable for grok-pi RPC compatibility");
  }

  const tuiCompat = process.env.PI_GROK_EXTENSION_TUI_COMPAT === "1";

  // Capture runner on setUIContext (+ optional rpc→tui rewrite).
  if (tuiCompat && !prototype.__piGrokTuiModeFacade) {
    const original = prototype.setUIContext;
    prototype.setUIContext = function setUIContext(
      this: ExtensionRunnerLike,
      uiContext: unknown,
      mode = "print",
    ): void {
      original.call(this, uiContext, mode === "rpc" ? "tui" : mode);
      snapshotAllCompletions(this);
    };
    prototype.__piGrokTuiModeFacade = true;
    prototype.__piGrokRunnerCapture = true;
  } else if (!prototype.__piGrokRunnerCapture) {
    const previous = prototype.setUIContext;
    prototype.setUIContext = function setUIContext(
      this: ExtensionRunnerLike,
      uiContext: unknown,
      mode = "print",
    ): void {
      previous.call(this, uiContext, mode);
      snapshotAllCompletions(this);
    };
    prototype.__piGrokRunnerCapture = true;
  }

  // get_commands always calls getRegisteredCommands first — snapshot there so
  // stdout enrich can apply on the same turn (sync providers).
  if (!prototype.__piGrokGetCommandsCapture) {
    const originalGet = prototype.getRegisteredCommands;
    prototype.getRegisteredCommands = function getRegisteredCommands(
      this: ExtensionRunnerLike,
    ): ExtensionCommand[] {
      const commands = originalGet.call(this);
      for (const command of commands) {
        snapshotCommandCompletions(command);
      }
      return commands;
    };
    prototype.__piGrokGetCommandsCapture = true;
  }
}

function isGetCommandsSuccessLine(obj: unknown): obj is {
  type: "response";
  command: "get_commands";
  success: true;
  data?: { commands?: Array<Record<string, unknown>> };
  id?: string;
} {
  if (!obj || typeof obj !== "object") return false;
  const row = obj as Record<string, unknown>;
  return row.type === "response" && row.command === "get_commands" && row.success === true;
}

function enrichGetCommandsLine(line: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return line;
  }
  if (!isGetCommandsSuccessLine(parsed)) return line;
  const commands = parsed.data?.commands;
  if (!Array.isArray(commands) || commands.length === 0 || completionCache.size === 0) {
    return line;
  }

  let changed = false;
  const next = commands.map((command) => {
    if (command.argumentCompletions != null) return command;
    if (command.source !== "extension") return command;
    const name = typeof command.name === "string" ? command.name : "";
    if (!name) return command;
    const argumentCompletions = completionCache.get(name);
    if (!argumentCompletions) return command;
    changed = true;
    return { ...command, argumentCompletions };
  });
  if (!changed) return line;
  return JSON.stringify({
    ...parsed,
    data: { ...(parsed.data ?? {}), commands: next },
  });
}

function maybeEnrichStdoutText(text: string): string {
  if (!text.includes('"get_commands"') || !text.includes('"success":true')) {
    return text;
  }
  const lines = text.split("\n");
  return lines.map((line) => (line ? enrichGetCommandsLine(line) : line)).join("\n");
}

/**
 * Intercept JSONL RPC output without redefining ESM exports.
 *
 * rpc-mode already called takeOverStdout(): its writeRawStdout uses a private
 * bound process.stdout.write. We restore, wrap that write, then take over again
 * so the bound raw writer is our wrapper.
 */
async function installGetCommandsStdoutIntercept(): Promise<void> {
  const proc = process as NodeJS.Process & { [PROCESS_MARK]?: boolean };
  if (proc[PROCESS_MARK]) return;

  let mod: OutputGuardModule;
  try {
    mod = (await import(hostUrl("core/output-guard.js"))) as OutputGuardModule;
  } catch {
    return;
  }
  if (typeof mod.takeOverStdout !== "function" || typeof mod.restoreStdout !== "function") {
    return;
  }

  try {
    if (mod.isStdoutTakenOver?.()) {
      mod.restoreStdout();
    }

    const previous = process.stdout.write.bind(process.stdout) as typeof process.stdout.write;
    process.stdout.write = ((
      chunk: string | Uint8Array,
      encodingOrCallback?: BufferEncoding | ((error?: Error | null) => void),
      callback?: (error?: Error | null) => void,
    ): boolean => {
      let text: string;
      if (typeof chunk === "string") {
        text = chunk;
      } else {
        text = Buffer.from(chunk).toString(
          typeof encodingOrCallback === "string" ? encodingOrCallback : "utf8",
        );
      }
      const enriched = maybeEnrichStdoutText(text);

      if (typeof encodingOrCallback === "function") {
        return previous(enriched, encodingOrCallback);
      }
      if (typeof callback === "function") {
        return previous(enriched, encodingOrCallback as BufferEncoding, callback);
      }
      if (typeof encodingOrCallback === "string") {
        return previous(enriched, encodingOrCallback);
      }
      return previous(enriched);
    }) as typeof process.stdout.write;

    mod.takeOverStdout();
    proc[PROCESS_MARK] = true;
  } catch (err) {
    // Never block Pi startup if intercept fails.
    try {
      if (!mod.isStdoutTakenOver?.()) {
        mod.takeOverStdout();
      }
    } catch {
      // ignore
    }
    console.error("[pi-grok-rpc-compat] stdout intercept failed:", err);
  }
}

export default async function (pi: ExtensionAPI): Promise<void> {
  await installRunnerHooks();
  await installGetCommandsStdoutIntercept();

  // User extensions such as loop.ts call sendUserMessage(), which re-enters
  // AgentSession.prompt with source="extension". Capture it before any later
  // extension handler, hand it to the Rust adapter, and mark it handled so it
  // does not enter Pi's private follow-up queue. The adapter owns ordering and
  // only sends the message back through source="rpc" when it is promoted.
  pi.on("input", (event, ctx) => {
    if (event.source !== "extension") return;
    ctx.ui.setStatus(
      QUEUE_STATUS_KEY,
      JSON.stringify({
        text: event.text,
        images: event.images ?? [],
        streamingBehavior: event.streamingBehavior ?? "followUp",
      }),
    );
    return { action: "handled" } as const;
  });
}
