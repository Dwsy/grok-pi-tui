import {
	BRIDGE_TYPE,
	MAX_EARLIER_SUMMARY_CHARS,
	MAX_MESSAGE_CHARS,
	MAX_RECAP_CONTEXT_CHARS,
	MAX_RECENT_TURNS,
} from "./shared.ts";

function truncateText(text: string, maxChars: number): string {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, maxChars).trimEnd()}…`;
}

function messageText(message: Record<string, unknown>): string {
	const content = message.content;
	if (typeof content === "string") return truncateText(content, MAX_MESSAGE_CHARS);
	if (!Array.isArray(content)) return "";
	const parts: string[] = [];
	for (const block of content) {
		if (!block || typeof block !== "object") continue;
		const item = block as Record<string, unknown>;
		if (item.type === "text" && typeof item.text === "string") parts.push(item.text);
		if (item.type === "toolCall" && typeof item.name === "string") parts.push(`[tool: ${item.name}]`);
	}
	return truncateText(parts.join("\n"), MAX_MESSAGE_CHARS);
}

export function countMainTurns(branch: Array<Record<string, unknown>>): number {
	return branch.filter((entry) => {
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") return false;
		return (entry.message as Record<string, unknown>).role === "user";
	}).length;
}

export function lastCompletedTurnAt(branch: Array<Record<string, unknown>>): number | undefined {
	for (let index = branch.length - 1; index >= 0; index--) {
		const entry = branch[index];
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") continue;
		if ((entry.message as Record<string, unknown>).role !== "assistant") continue;
		const timestamp = Date.parse(String(entry.timestamp ?? ""));
		if (Number.isFinite(timestamp)) return timestamp;
	}
	return undefined;
}

export function lastSuccessfulRecapTurnCount(
	branch: Array<Record<string, unknown>>,
): number | undefined {
	let userTurns = 0;
	let lastSuccessful: number | undefined;
	for (const entry of branch) {
		if (entry.type === "message" && entry.message && typeof entry.message === "object") {
			if ((entry.message as Record<string, unknown>).role === "user") userTurns++;
			continue;
		}
		if (entry.customType !== BRIDGE_TYPE) continue;
		// appendEntry entries carry `data`; sendMessage-era custom_message
		// entries carried `details`. Accept both for session continuity.
		const details =
			entry.type === "custom"
				? entry.data
				: entry.type === "custom_message"
					? entry.details
					: undefined;
		if (details && typeof details === "object" && (details as Record<string, unknown>).ok === true) {
			lastSuccessful = userTurns;
		}
	}
	return lastSuccessful;
}

export function buildRecapContext(branch: Array<Record<string, unknown>>): string {
	const lines: string[] = [];
	let selectedTurns = 0;
	let earliestSelectedIndex = branch.length;
	for (let index = branch.length - 1; index >= 0; index--) {
		const entry = branch[index];
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") continue;
		const message = entry.message as Record<string, unknown>;
		const role = message.role;
		if (role !== "user" && role !== "assistant" && role !== "toolResult") continue;
		if (role === "user" && selectedTurns >= MAX_RECENT_TURNS) break;
		const text = messageText(message);
		if (!text) continue;
		const label = role === "user" ? "User" : role === "assistant" ? "Assistant" : "Tool result";
		lines.push(`[${label}]: ${text}`);
		earliestSelectedIndex = index;
		if (role === "user") selectedTurns++;
	}
	lines.reverse();

	for (let index = earliestSelectedIndex - 1; index >= 0; index--) {
		const entry = branch[index];
		if (entry.type !== "compaction") continue;
		const summary = truncateText(String(entry.summary ?? ""), MAX_EARLIER_SUMMARY_CHARS);
		if (summary) lines.unshift(`[Earlier summary]: ${summary}`);
		break;
	}

	const context = lines.join("\n\n");
	if (context.length <= MAX_RECAP_CONTEXT_CHARS) return context;
	const tail = context.slice(-MAX_RECAP_CONTEXT_CHARS);
	const firstBoundary = tail.indexOf("\n\n");
	return firstBoundary >= 0 ? tail.slice(firstBoundary + 2) : tail;
}
