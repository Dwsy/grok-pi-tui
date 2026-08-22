/**
 * Conversation-context projection for /btw side questions.
 *
 * Flattens the active session branch into a labeled transcript, dropping
 * trailing incomplete assistant tool runs so mid-turn snapshots never leak
 * dangling tool calls into the side-question prompt.
 */

import { MAX_CONTEXT_CHARS, MAX_MESSAGE_CHARS } from "./shared.ts";

export function truncateText(text: string, maxChars: number): string {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, maxChars).trimEnd()}…`;
}

function messageText(message: Record<string, unknown>): string {
	const content = message.content;
	if (typeof content === "string")
		return truncateText(content, MAX_MESSAGE_CHARS);
	if (!Array.isArray(content)) return "";
	const parts: string[] = [];
	for (const block of content) {
		if (!block || typeof block !== "object") continue;
		const item = block as Record<string, unknown>;
		if (item.type === "text" && typeof item.text === "string")
			parts.push(item.text);
		if (item.type === "toolCall" && typeof item.name === "string") {
			parts.push(`[tool: ${item.name}]`);
		}
		if (item.type === "toolResult") {
			const text =
				typeof item.text === "string"
					? item.text
					: typeof item.content === "string"
						? item.content
						: "";
			if (text) parts.push(`[tool result]: ${truncateText(text, 800)}`);
		}
	}
	return truncateText(parts.join("\n"), MAX_MESSAGE_CHARS);
}

/** Drop trailing incomplete assistant tool runs (mid-turn snapshot safety). */
function stripIncompleteTail(
	branch: Array<Record<string, unknown>>,
): Array<Record<string, unknown>> {
	const out = branch.slice();
	while (out.length > 0) {
		const last = out[out.length - 1];
		if (
			last.type === "message" &&
			last.message &&
			typeof last.message === "object"
		) {
			const msg = last.message as Record<string, unknown>;
			const role = msg.role;
			if (role === "toolResult") {
				out.pop();
				continue;
			}
			if (role === "assistant") {
				const content = msg.content;
				const hasToolCall =
					Array.isArray(content) &&
					content.some(
						(b) =>
							b &&
							typeof b === "object" &&
							(b as Record<string, unknown>).type === "toolCall",
					);
				if (hasToolCall) {
					out.pop();
					continue;
				}
			}
		}
		break;
	}
	return out;
}

export function buildSideContext(branch: Array<Record<string, unknown>>): string {
	const lines: string[] = [];
	const cleaned = stripIncompleteTail(branch);
	for (const entry of cleaned) {
		if (entry.type === "compaction") {
			const summary = truncateText(String(entry.summary ?? ""), 2_000);
			if (summary) lines.push(`[Earlier summary]: ${summary}`);
			continue;
		}
		if (
			entry.type !== "message" ||
			!entry.message ||
			typeof entry.message !== "object"
		) {
			continue;
		}
		const message = entry.message as Record<string, unknown>;
		const role = message.role;
		if (
			role !== "user" &&
			role !== "assistant" &&
			role !== "toolResult" &&
			role !== "system"
		) {
			continue;
		}
		const text = messageText(message);
		if (!text) continue;
		const label =
			role === "user"
				? "User"
				: role === "assistant"
					? "Assistant"
					: role === "system"
						? "System"
						: "Tool result";
		lines.push(`[${label}]: ${text}`);
	}
	const context = lines.join("\n\n");
	if (context.length <= MAX_CONTEXT_CHARS) return context;
	const tail = context.slice(-MAX_CONTEXT_CHARS);
	const firstBoundary = tail.indexOf("\n\n");
	return firstBoundary >= 0 ? tail.slice(firstBoundary + 2) : tail;
}

export function sideQuestionInstruction(question: string): string {
	return [
		"<system-reminder>",
		"This is a side question from the user.",
		"You must answer this question directly in a single response.",
		"",
		"IMPORTANT CONTEXT:",
		"- You are a separate, lightweight agent spawned to answer this one question",
		"- The main agent is NOT interrupted - it continues working independently in the background",
		"- You share the conversation context but are a completely separate instance",
		'- Do NOT reference being interrupted or what you were "previously doing" - that framing is incorrect',
		"",
		"CRITICAL CONSTRAINTS:",
		"- You have NO tools available - you cannot read files, run commands, search, or take any actions",
		"- This is a one-off response - there will be no follow-up turns",
		"- You can ONLY provide information based on what you already know from the conversation context",
		'- NEVER say things like "Let me try...", "I\'ll now...", "Let me check...", or promise to take any action',
		"- If you don't know the answer, say so - do not offer to look it up or investigate",
		"",
		"Simply answer the question with the information you have.",
		"</system-reminder>",
		"",
		question,
	].join("\n");
}
