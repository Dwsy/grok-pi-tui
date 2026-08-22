/**
 * Control-directory bridge: the grok-pi adapter drops request-<nonce>.json
 * files; this polls, executes preview/rollback, and writes response files.
 */

import { randomUUID } from "node:crypto";
import { chmod, readdir, rename, rm, stat, readFile as fsReadFile, writeFile as fsWriteFile } from "node:fs/promises";
import { join } from "node:path";
import { computeRollbackPlan, executeRollback, getBranchEntryIds } from "./rollback.ts";
import { BRIDGE_VERSION, STALE_BRIDGE_MS } from "./shared.ts";
import type { BridgeRequest, BridgeResponse } from "./shared.ts";
import { state } from "./shared.ts";

export async function cleanStaleBridgeFiles(): Promise<void> {
  try {
    const files = await readdir(state.controlDir);
    const now = Date.now();
    for (const f of files) {
      const p = join(state.controlDir, f);
      try {
        const s = await stat(p);
        if (now - s.mtimeMs > STALE_BRIDGE_MS) await rm(p, { force: true });
      } catch { /* ignore */ }
    }
  } catch { /* control dir may not exist */ }
}

export async function processBridgeRequest(req: BridgeRequest, ctx: any): Promise<void> {
  const responsePath = join(state.controlDir, `response-${req.nonce}.json`);
  const tmpPath = `${responsePath}.tmp.${randomUUID()}`;

  let response: BridgeResponse;
  try {
    if (req.version !== BRIDGE_VERSION) throw new Error(`unsupported bridge version: ${req.version}`);
    if (req.sessionId !== state.sessionId) throw new Error(`session mismatch: ${req.sessionId} !== ${state.sessionId}`);

    const entries = ctx.sessionManager?.getEntries?.() ?? [];
    const branchEntryIds = getBranchEntryIds(entries);

    if (req.method === "preview") {
      const plan = await computeRollbackPlan(req.params.targetEntryId, branchEntryIds);
      response = {
        version: BRIDGE_VERSION, nonce: req.nonce, sessionId: state.sessionId, method: "preview", ok: true,
        result: { eligible: plan.eligible, paths: plan.paths, conflicts: plan.conflicts },
        completedAt: new Date().toISOString(),
      };
    } else if (req.method === "execute") {
      const plan = await computeRollbackPlan(req.params.targetEntryId, branchEntryIds);
      if (!plan.eligible) {
        response = {
          version: BRIDGE_VERSION, nonce: req.nonce, sessionId: state.sessionId, method: "execute", ok: false,
          error: plan.conflicts.join("; ") || "no eligible paths",
          completedAt: new Date().toISOString(),
        };
      } else {
        const leafId = entries.find((e: any) => e.isCurrent || e.isLeaf)?.id ?? "unknown";
        const txId = await executeRollback(plan, req.params.targetEntryId, leafId);
        response = {
          version: BRIDGE_VERSION, nonce: req.nonce, sessionId: state.sessionId, method: "execute", ok: true,
          result: { eligible: true, paths: plan.paths, conflicts: [], transactionId: txId },
          completedAt: new Date().toISOString(),
        };
      }
    } else {
      throw new Error(`unknown method: ${req.method}`);
    }
  } catch (err: any) {
    response = {
      version: BRIDGE_VERSION, nonce: req.nonce, sessionId: state.sessionId, method: req.method, ok: false,
      error: err?.message ?? String(err),
      completedAt: new Date().toISOString(),
    };
  }

  await fsWriteFile(tmpPath, JSON.stringify(response, null, 2));
  await chmod(tmpPath, 0o600).catch(() => {});
  await rename(tmpPath, responsePath);
}

export async function pollBridgeRequests(ctx: any): Promise<void> {
  if (!state.controlDir) return;
  try {
    const files = await readdir(state.controlDir);
    for (const f of files) {
      if (!f.startsWith("request-") || !f.endsWith(".json")) continue;
      const reqPath = join(state.controlDir, f);
      try {
        const raw = await fsReadFile(reqPath, "utf-8");
        const req: BridgeRequest = JSON.parse(raw);
        await rm(reqPath, { force: true });
        await processBridgeRequest(req, ctx);
      } catch { /* skip malformed */ }
    }
  } catch { /* control dir may not exist */ }
}
