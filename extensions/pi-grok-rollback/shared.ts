/**
 * Shared constants, wire types, and mutable session state for the
 * Pi tree file rollback checkpoint extension.
 *
 * Journal/blob layout and bridge filenames are part of the grok-pi adapter
 * contract (`rollback_bridge.rs`) — rename only in lockstep.
 */

import { AsyncLocalStorage } from "node:async_hooks";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

export const JOURNAL_VERSION = 1;
export const BRIDGE_VERSION = 1;
export const MAX_BLOB_SIZE = 10 * 1024 * 1024; // 10 MB per blob
export const STALE_BRIDGE_MS = 60_000;
export const BRIDGE_POLL_MS = 50;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface JournalHeader {
  version: number;
  piSessionId: string;
  canonicalSessionFile?: string;
  sessionHeaderDigest?: string;
  origin: "new" | "continued" | "resumed" | "forked" | "ephemeral";
  captureBoundaryEntryId?: string;
  createdByGrokPi: true;
}

export interface MutationRecord {
  sequence: number;
  operationId: string;
  piSessionId: string;
  treeEntryId?: string;
  toolCallId: string;
  tool: "write" | "edit";
  canonicalPath: string;
  before: "absent" | string; // "absent" or "blob:<sha256hex>"
  after: "absent" | string;
  state: "prepared" | "committed" | "unbound" | "reconciled";
  toolReportedError: boolean;
  preparedAt: string;
  committedAt?: string;
}

export interface RollbackTransaction {
  transactionId: string;
  targetEntryId: string;
  sourceLeafId: string;
  plannedPaths: string[];
  state: "prepared" | "committed" | "compensating" | "failed";
  createdAt: string;
}

export interface BridgeRequest {
  version: number;
  nonce: string;
  sessionId: string;
  method: "preview" | "execute";
  params: { targetEntryId: string };
  createdAt: string;
}

export interface BridgeResponse {
  version: number;
  nonce: string;
  sessionId: string;
  method: "preview" | "execute";
  ok: boolean;
  result?: {
    eligible: boolean;
    paths: Array<{
      canonicalPath: string;
      action: "restore" | "delete" | "noop";
      currentDigest: string | null;
      targetDigest: string | null;
    }>;
    conflicts: string[];
    transactionId?: string;
  };
  error?: string;
  completedAt: string;
}

export interface RollbackPlan {
  eligible: boolean;
  paths: Array<{
    canonicalPath: string;
    action: "restore" | "delete" | "noop";
    currentDigest: string | null;
    targetDigest: string | null;
  }>;
  conflicts: string[];
}

// ---------------------------------------------------------------------------
// AsyncLocalStorage for toolCallId propagation into operations
// ---------------------------------------------------------------------------

export interface ToolCallContext {
  toolCallId: string;
  tool: "write" | "edit";
}

export const toolCallStorage = new AsyncLocalStorage<ToolCallContext>();

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** Session-scoped paths and liveness, initialized by the entry point. */
export const state = {
  stateRoot: "",
  controlDir: "",
  sessionId: "",
  sessionDir: "",
  blobDir: "",
  journalPath: "",
  headerPath: "",
  sequence: 0,
  active: false,
  extensionCwd: "",
};
