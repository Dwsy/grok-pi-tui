/**
 * Durable storage helpers: content-addressed blob store, WAL journal
 * append/read, and secure-write primitives.
 */

import { createHash, randomUUID } from "node:crypto";
import {
  chmod,
  lstat,
  mkdir as fsMkdir,
  readFile as fsReadFile,
  rename,
  stat,
  writeFile as fsWriteFile,
  appendFile,
} from "node:fs/promises";
import { isAbsolute, join, resolve } from "node:path";
import { MAX_BLOB_SIZE } from "./shared.ts";
import type {
  MutationRecord,
  RollbackTransaction,
} from "./shared.ts";
import { state } from "./shared.ts";

export function sha256(buf: Buffer): string {
  return createHash("sha256").update(buf).digest("hex");
}

export function blobRef(hash: string): string {
  return `blob:${hash}`;
}

export function parseBlobRef(ref: string): string | null {
  if (ref === "absent") return null;
  if (ref.startsWith("blob:")) return ref.slice(5);
  return null;
}

export async function ensureDir(dir: string): Promise<void> {
  await fsMkdir(dir, { recursive: true });
  await chmod(dir, 0o700).catch(() => {});
}

export async function writeSecure(path: string, data: Buffer | string): Promise<void> {
  await fsWriteFile(path, data);
  await chmod(path, 0o600).catch(() => {});
}

export async function appendJournal(record: MutationRecord | RollbackTransaction): Promise<void> {
  const line = JSON.stringify(record) + "\n";
  await appendFile(state.journalPath, line, "utf-8");
}

export async function readBlob(hash: string): Promise<Buffer | null> {
  try {
    return await fsReadFile(join(state.blobDir, hash));
  } catch {
    return null;
  }
}

export async function writeBlob(data: Buffer): Promise<string> {
  if (data.length > MAX_BLOB_SIZE) {
    throw new Error(`blob exceeds ${MAX_BLOB_SIZE} byte limit (${data.length})`);
  }
  const hash = sha256(data);
  const p = join(state.blobDir, hash);
  try {
    await stat(p);
    return hash; // dedup
  } catch {
    // not found
  }
  const tmp = `${p}.tmp.${randomUUID()}`;
  await fsWriteFile(tmp, data);
  await chmod(tmp, 0o600).catch(() => {});
  await rename(tmp, p);
  return hash;
}

export async function isSymlink(path: string): Promise<boolean> {
  try {
    return (await lstat(path)).isSymbolicLink();
  } catch {
    return false;
  }
}

export function canonicalize(filePath: string): string {
  return isAbsolute(filePath) ? resolve(filePath) : resolve(state.extensionCwd, filePath);
}

export async function readJournalRecords(): Promise<Array<MutationRecord | RollbackTransaction>> {
  try {
    const content = await fsReadFile(state.journalPath, "utf-8");
    return content.split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  } catch {
    return [];
  }
}

export function getMutations(records: Array<MutationRecord | RollbackTransaction>): MutationRecord[] {
  return records.filter((r): r is MutationRecord => "operationId" in r);
}

export function getTransactions(records: Array<MutationRecord | RollbackTransaction>): RollbackTransaction[] {
  return records.filter((r): r is RollbackTransaction => "transactionId" in r);
}
