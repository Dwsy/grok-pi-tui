/**
 * Pi Tree File Rollback — checkpoint extension.
 *
 * Wraps Pi builtin write/edit tools via their public Operations seam to
 * capture before/after byte snapshots inside the mutation queue critical
 * section. Maintains a durable WAL journal with content-addressed blob
 * storage, binds mutations to session tree entries, and serves
 * preview/execute requests from the grok-pi adapter via a control-directory
 * bridge.
 *
 * Injected only when F2 "Pi tree file rollback" is enabled.
 * Does NOT modify Pi source.
 *
 * Module map (all materialized by the Rust injector `rollback_extension.rs`):
 * - `shared.ts`   — adapter-contract constants/types + session state
 * - `store.ts`    — blob store, WAL journal I/O, secure-write helpers
 * - `journal.ts`  — journal init, mutation capture, tree-entry binding
 * - `rollback.ts` — plan computation and transactional execution
 * - `bridge.ts`   — control-directory request/response bridge
 * - `index.ts`    — entry point: tool wrappers, hooks, hidden commands
 */

import { constants } from "node:fs";
import {
  access as fsAccess,
  mkdir as fsMkdir,
  readFile as fsReadFile,
  writeFile as fsWriteFile,
} from "node:fs/promises";
import { join } from "node:path";

import {
  createEditToolDefinition,
  createWriteToolDefinition,
} from "@earendil-works/pi-coding-agent";

import { captureMutation, bindTreeEntries, initJournal } from "./journal.ts";
import { cleanStaleBridgeFiles, pollBridgeRequests } from "./bridge.ts";
import { computeRollbackPlan, executeRollback, getBranchEntryIds } from "./rollback.ts";
import { ensureDir } from "./store.ts";
import { BRIDGE_POLL_MS } from "./shared.ts";
import type { JournalHeader } from "./shared.ts";
import { state, toolCallStorage } from "./shared.ts";

export default function (pi: any) {
  const enabled = process.env.PI_GROK_ROLLBACK === "1";
  if (!enabled) return;

  state.stateRoot = process.env.GROK_PI_ROLLBACK_STATE || join(process.env.HOME || "/tmp", ".grok", "pi-file-rollback");
  state.controlDir = process.env.GROK_PI_ROLLBACK_CONTROL || "";
  state.extensionCwd = process.cwd();

  let bridgeCtx: any = null;
  let bridgeTimer: ReturnType<typeof setInterval> | null = null;

  pi.on("session_start", async (_event: any, ctx: any) => {
    state.extensionCwd = ctx.cwd || process.cwd();
    const sid = ctx.sessionManager?.sessionId || ctx.sessionManager?.id || "unknown";
    const origin: JournalHeader["origin"] =
      process.env.PI_GROK_ROLLBACK_ORIGIN === "resumed" ? "resumed" : "new";

    await initJournal(sid, origin);
    state.active = true;

    // Verify write/edit are still Pi builtin (not overridden by user extension)
    const allTools = pi.getAllTools?.() ?? [];
    for (const name of ["write", "edit"]) {
      const info = allTools.find((t: any) => t.name === name);
      if (info && info.source && info.source !== "builtin" && info.source !== "pi") {
        state.active = false;
        return;
      }
    }

    // --- Write tool wrapper ---
    // Custom operations that capture before/after inside the mutation queue.
    const writeDef = createWriteToolDefinition(state.extensionCwd, {
      operations: {
        mkdir: (dir: string) => fsMkdir(dir, { recursive: true }).then(() => {}),
        writeFile: async (absolutePath: string, content: string) => {
          const tc = toolCallStorage.getStore();
          // Capture before
          let beforeData: Buffer | null = null;
          try { beforeData = await fsReadFile(absolutePath); } catch { /* absent */ }

          // Actual write
          await fsWriteFile(absolutePath, content, "utf-8");

          // Capture after and record mutation
          if (tc && state.active) {
            const afterData = Buffer.from(content, "utf-8");
            await captureMutation("write", tc.toolCallId, absolutePath, beforeData, afterData, false).catch(() => {});
          }
        },
      },
    });

    // Wrap execute to inject toolCallId via AsyncLocalStorage
    const origWriteExecute = writeDef.execute.bind(writeDef);
    writeDef.execute = async (toolCallId: string, params: any, signal: any, onUpdate: any, toolCtx: any) => {
      return toolCallStorage.run({ toolCallId, tool: "write" }, () =>
        origWriteExecute(toolCallId, params, signal, onUpdate, toolCtx),
      );
    };
    pi.registerTool(writeDef);

    // --- Edit tool wrapper ---
    let editBeforeData: Buffer | null = null;
    let editBeforePath: string | null = null;

    const editDef = createEditToolDefinition(state.extensionCwd, {
      operations: {
        access: (absolutePath: string) => fsAccess(absolutePath, constants.R_OK | constants.W_OK),
        readFile: async (absolutePath: string) => {
          const data = await fsReadFile(absolutePath);
          // Store before snapshot
          editBeforeData = data;
          editBeforePath = absolutePath;
          return data;
        },
        writeFile: async (absolutePath: string, content: string) => {
          const tc = toolCallStorage.getStore();

          // Actual write
          await fsWriteFile(absolutePath, content, "utf-8");

          // Capture mutation
          if (tc && state.active) {
            const before = editBeforePath === absolutePath ? editBeforeData : null;
            const afterData = Buffer.from(content, "utf-8");
            await captureMutation("edit", tc.toolCallId, absolutePath, before, afterData, false).catch(() => {});
          }
          editBeforeData = null;
          editBeforePath = null;
        },
      },
    });

    const origEditExecute = editDef.execute.bind(editDef);
    editDef.execute = async (toolCallId: string, params: any, signal: any, onUpdate: any, toolCtx: any) => {
      return toolCallStorage.run({ toolCallId, tool: "edit" }, () =>
        origEditExecute(toolCallId, params, signal, onUpdate, toolCtx),
      );
    };
    pi.registerTool(editDef);

    // Start bridge polling
    if (state.controlDir) {
      await ensureDir(state.controlDir);
      await cleanStaleBridgeFiles();
      bridgeCtx = ctx;
      bridgeTimer = setInterval(() => pollBridgeRequests(bridgeCtx), BRIDGE_POLL_MS);
    }
  });

  // Bind tree entries after each turn
  pi.on("turn_end", async (_event: any, ctx: any) => {
    await bindTreeEntries(ctx);
  });

  pi.on("agent_settled", async (_event: any, ctx: any) => {
    await bindTreeEntries(ctx);
  });

  // Cleanup on shutdown
  pi.on("session_shutdown", async () => {
    state.active = false;
    if (bridgeTimer) {
      clearInterval(bridgeTimer);
      bridgeTimer = null;
    }
  });

  // Hidden bridge commands for adapter
  pi.registerCommand("__pi_rollback_preview", {
    description: "Internal: preview file rollback to a tree entry",
    handler: async (args: string, ctx: any) => {
      const targetEntryId = String(args ?? "").trim();
      if (!targetEntryId) throw new Error("target entry id required");
      const entries = ctx.sessionManager?.getEntries?.() ?? [];
      const branchEntryIds = getBranchEntryIds(entries);
      const plan = await computeRollbackPlan(targetEntryId, branchEntryIds);
      ctx.ui?.toast?.(
        plan.eligible
          ? `Rollback preview: ${plan.paths.filter((p) => p.action !== "noop").length} file(s) to restore`
          : `Rollback blocked: ${plan.conflicts.join("; ") || "no eligible paths"}`,
      );
    },
  });

  pi.registerCommand("__pi_rollback_execute", {
    description: "Internal: execute file rollback to a tree entry",
    handler: async (args: string, ctx: any) => {
      const targetEntryId = String(args ?? "").trim();
      if (!targetEntryId) throw new Error("target entry id required");
      if (!ctx.isIdle?.()) throw new Error("rollback requires idle session");

      const entries = ctx.sessionManager?.getEntries?.() ?? [];
      const branchEntryIds = getBranchEntryIds(entries);
      const plan = await computeRollbackPlan(targetEntryId, branchEntryIds);
      if (!plan.eligible) {
        throw new Error(`rollback blocked: ${plan.conflicts.join("; ") || "no eligible paths"}`);
      }

      const leafId = entries.find((e: any) => e.isCurrent || e.isLeaf)?.id ?? "unknown";
      const txId = await executeRollback(plan, targetEntryId, leafId);
      const count = plan.paths.filter((p) => p.action !== "noop").length;
      ctx.ui?.toast?.(`Rolled back ${count} file(s) (tx: ${txId.slice(0, 8)})`);
    },
  });
}
