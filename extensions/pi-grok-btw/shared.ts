/**
 * Shared constants and wire types for the headless /btw bridge.
 *
 * Entry/custom-type strings are part of the adapter contract
 * (`crates/codegen/pi-grok-adapter/src/btw_bridge.rs`) — rename only in lockstep.
 */

export const BRIDGE_TYPE = "pi-grok-btw/v1";
export const HISTORY_ENTRY_TYPE = "pi-grok-btw/history/v1";
export const COMMAND = "__pi_grok_btw";
export const HISTORY_COMMAND = "btw-history";
export const MAX_CONTEXT_CHARS = 48_000;
export const MAX_MESSAGE_CHARS = 4_000;

export type BtwArgs = {
	requestId?: string;
	question?: string;
	models?: string[];
	model?: string;
	thinkingLevel?: "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
};

export type BtwHistoryEntry = {
	version: 1;
	requestId: string;
	question: string;
	answer: string;
	createdAt: number;
	modelUsed?: string;
};
