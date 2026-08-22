/**
 * Journal initialization and mutation capture/tree-entry binding.
 */

import { randomUUID } from "node:crypto";
import { readFile as fsReadFile } from "node:fs/promises";
import { join } from "node:path";
import {
  ensureDir,
  appendJournal,
  getMutations,
  getTransactions,
  isSymlink,
  readJournalRecords,
  writeBlob,
  blobRef,
  writeSecure,
} from "./store.ts";
import { JOURNAL_VERSION } from "./shared.ts";
import type { JournalHeader, MutationRecord } from "./shared.ts";
import { state } from "./shared.ts";

export async function initJournal(sid: string, origin: JournalHeader["origin"], boundaryEntryId?: string): Promise<void> {
  state.sessionId = sid;
  state.sessionDir = join(state.stateRoot, "sessions", sid);
  state.blobDir = join(state.stateRoot, "blobs");
  state.journalPath = join(state.sessionDir, "journal.jsonl");
  state.headerPath = join(state.sessionDir, "header.json");

  await ensureDir(state.sessionDir);
  await ensureDir(state.blobDir);

  let existingHeader: JournalHeader | null = null;
  try {
    existingHeader = JSON.parse(await fsReadFile(state.headerPath, "utf-8"));
  } catch {
    // no existing header
  }

  if (existingHeader && existingHeader.piSessionId === sid) {
    // Resume existing journal
    const records = await readJournalRecords();
    const mutations = getMutations(records);
    state.sequence = mutations.reduce((max, m) => Math.max(max, m.sequence), 0);
    return;
  }

  const header: JournalHeader = {
    version: JOURNAL_VERSION,
    piSessionId: sid,
    origin,
    captureBoundaryEntryId: boundaryEntryId,
    createdByGrokPi: true,
  };
  await writeSecure(state.headerPath, JSON.stringify(header, null, 2));
  await writeSecure(state.journalPath, "");
  state.sequence = 0;
}

// ---------------------------------------------------------------------------
// Mutation capture — called from within the mutation queue critical section
// ---------------------------------------------------------------------------

export async function captureMutation(
  tool: "write" | "edit",
  toolCallId: string,
  canonicalPath: string,
  beforeData: Buffer | null,
  afterData: Buffer | null,
  toolReportedError: boolean,
): Promise<void> {
  if (!state.active) return;

  // Reject symlinks at final target
  if (await isSymlink(canonicalPath)) {
    throw new Error(`rollback: refusing to checkpoint symlink target: ${canonicalPath}`);
  }

  let before: string;
  if (beforeData === null) {
    before = "absent";
  } else {
    before = blobRef(await writeBlob(beforeData));
  }

  let after: string;
  if (afterData === null) {
    after = "absent";
  } else {
    after = blobRef(await writeBlob(afterData));
  }

  // Skip if nothing changed
  if (before === after) return;

  state.sequence++;
  const record: MutationRecord = {
    sequence: state.sequence,
    operationId: randomUUID(),
    piSessionId: state.sessionId,
    toolCallId,
    tool,
    canonicalPath,
    before,
    after,
    state: "unbound",
    toolReportedError,
    preparedAt: new Date().toISOString(),
  };

  await appendJournal(record);
}

// ---------------------------------------------------------------------------
// Tree entry binding
// ---------------------------------------------------------------------------

export async function bindTreeEntries(ctx: any): Promise<void> {
  if (!state.active) return;
  try {
    const entries = ctx.sessionManager?.getEntries?.();
    if (!entries) return;

    const records = await readJournalRecords();
    const unbound = getMutations(records).filter((m) => m.state === "unbound");
    if (unbound.length === 0) return;

    const toolResultMap = new Map<string, string>();
    for (const entry of entries) {
      if (entry.type === "toolResult" && entry.toolCallId) {
        toolResultMap.set(entry.toolCallId, entry.id);
      }
    }

    let updated = false;
    for (const m of unbound) {
      const entryId = toolResultMap.get(m.toolCallId);
      if (entryId) {
        m.treeEntryId = entryId;
        m.state = "reconciled";
        updated = true;
      }
    }

    if (updated) {
      const allRecords = await readJournalRecords();
      const mutations = getMutations(allRecords).map((m) => {
        const bound = unbound.find((u) => u.operationId === m.operationId);
        return bound ?? m;
      });
      const transactions = getTransactions(allRecords);
      const all = [...mutations, ...transactions].sort(
        (a, b) => ("sequence" in a ? a.sequence : 0) - ("sequence" in b ? b.sequence : 0),
      );
      await writeSecure(state.journalPath, all.map((r) => JSON.stringify(r)).join("\n") + "\n");
    }
  } catch {
    // Best-effort
  }
}
