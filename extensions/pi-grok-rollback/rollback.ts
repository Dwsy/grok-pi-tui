/**
 * Rollback planning and execution: diff journal mutations against current
 * file state, detect external modifications, and restore/delete atomically
 * with a transaction record.
 */

import { randomUUID } from "node:crypto";
import { readFile as fsReadFile, rename, unlink, writeFile as fsWriteFile } from "node:fs/promises";
import {
  appendJournal,
  getMutations,
  parseBlobRef,
  readBlob,
  readJournalRecords,
  sha256,
} from "./store.ts";
import type { MutationRecord, RollbackPlan, RollbackTransaction } from "./shared.ts";

export function getBranchEntryIds(entries: any[]): Set<string> {
  const ids = new Set<string>();
  let current = entries.find((e: any) => e.isCurrent || e.isLeaf);
  while (current) {
    ids.add(current.id);
    current = entries.find((e: any) => e.id === current.parentId);
  }
  return ids;
}

export async function computeRollbackPlan(
  targetEntryId: string,
  branchEntryIds: Set<string>,
): Promise<RollbackPlan> {
  const records = await readJournalRecords();
  const mutations = getMutations(records);

  if (!branchEntryIds.has(targetEntryId)) {
    return { eligible: false, paths: [], conflicts: [`target entry ${targetEntryId} not on active branch`] };
  }

  // Mutations whose treeEntryId is a strict descendant of target on this branch
  const afterTarget = mutations.filter((m) => {
    if (!m.treeEntryId) return false;
    return branchEntryIds.has(m.treeEntryId) && m.treeEntryId !== targetEntryId;
  });

  // Group by canonicalPath: first mutation gives target state (before), last gives expected current (after)
  const pathFirst = new Map<string, MutationRecord>();
  const pathLast = new Map<string, MutationRecord>();
  for (const m of afterTarget) {
    if (!pathFirst.has(m.canonicalPath)) pathFirst.set(m.canonicalPath, m);
    pathLast.set(m.canonicalPath, m);
  }

  const paths: RollbackPlan["paths"] = [];
  const conflicts: string[] = [];

  for (const [canonicalPath, lastMutation] of pathLast) {
    const firstMutation = pathFirst.get(canonicalPath)!;
    const targetState = firstMutation.before;
    const expectedCurrentState = lastMutation.after;

    // Read current file
    let currentDigest: string | null = null;
    let currentExists = false;
    try {
      const data = await fsReadFile(canonicalPath);
      currentExists = true;
      currentDigest = sha256(data);
    } catch {
      currentExists = false;
    }

    // Verify current state matches expected
    const expectedHash = parseBlobRef(expectedCurrentState);
    if (expectedCurrentState === "absent") {
      if (currentExists) {
        conflicts.push(`${canonicalPath}: expected absent but file exists (external modification)`);
        continue;
      }
      paths.push({ canonicalPath, action: "noop", currentDigest: null, targetDigest: null });
    } else if (expectedHash) {
      if (!currentExists) {
        conflicts.push(`${canonicalPath}: expected content but file missing (external deletion)`);
        continue;
      }
      if (currentDigest !== expectedHash) {
        conflicts.push(`${canonicalPath}: content mismatch (external modification)`);
        continue;
      }
    }

    // Determine action
    const targetHash = parseBlobRef(targetState);
    if (targetState === "absent") {
      paths.push({
        canonicalPath,
        action: currentExists ? "delete" : "noop",
        currentDigest,
        targetDigest: null,
      });
    } else if (targetHash) {
      if (currentExists && currentDigest === targetHash) {
        paths.push({ canonicalPath, action: "noop", currentDigest, targetDigest: targetHash });
      } else {
        paths.push({ canonicalPath, action: "restore", currentDigest, targetDigest: targetHash });
      }
    }
  }

  return {
    eligible: conflicts.length === 0 && paths.some((p) => p.action !== "noop"),
    paths,
    conflicts,
  };
}

export async function executeRollback(
  plan: RollbackPlan,
  targetEntryId: string,
  sourceLeafId: string,
): Promise<string> {
  const transactionId = randomUUID();
  const activePaths = plan.paths.filter((p) => p.action !== "noop");

  const tx: RollbackTransaction = {
    transactionId,
    targetEntryId,
    sourceLeafId,
    plannedPaths: activePaths.map((p) => p.canonicalPath),
    state: "prepared",
    createdAt: new Date().toISOString(),
  };
  await appendJournal(tx);

  try {
    for (const p of activePaths) {
      if (p.action === "delete") {
        await unlink(p.canonicalPath).catch(() => {});
      } else if (p.action === "restore" && p.targetDigest) {
        const data = await readBlob(p.targetDigest);
        if (!data) throw new Error(`missing blob for ${p.canonicalPath}`);
        const tmp = `${p.canonicalPath}.rollback-tmp.${randomUUID()}`;
        await fsWriteFile(tmp, data);
        await rename(tmp, p.canonicalPath);
      }
    }
    tx.state = "committed";
    await appendJournal(tx);
  } catch (err) {
    tx.state = "failed";
    await appendJournal(tx);
    throw err;
  }

  return transactionId;
}
