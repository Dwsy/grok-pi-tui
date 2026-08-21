/**
 * Headless /btw bridge for grok-pi.
 *
 * Single-turn side question via pi-ai `streamSimple()` — does not mutate the main
 * LLM conversation. Stream deltas and the final result are appended as custom
 * session entries (`pi-grok-btw/v1`) that never enter the agent loop context;
 * the adapter projects the `entry_appended` event onto the native review Q&A
 * panel. Successful answers are also persisted under a dedicated history
 * custom type for `/btw-history`.
 *
 * Invoked via `/__pi_grok_btw` (hidden from slash UI by adapter filter).
 * `/btw-history` is a model-free command whose projection is handled by the
 * adapter. Args JSON: `{ requestId, question, models?: string[], thinkingLevel? }`.
 */
import { type Message, streamSimple } from "@earendil-works/pi-ai/compat";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-btw/v1";
const HISTORY_ENTRY_TYPE = "pi-grok-btw/history/v1";
const COMMAND = "__pi_grok_btw";
const HISTORY_COMMAND = "btw-history";
const MAX_CONTEXT_CHARS = 48_000;
const MAX_MESSAGE_CHARS = 4_000;

type BtwArgs = {
	requestId?: string;
	question?: string;
	models?: string[];
	model?: string;
	thinkingLevel?: "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
};

type BtwHistoryEntry = {
	version: 1;
	requestId: string;
	question: string;
	answer: string;
	createdAt: number;
	modelUsed?: string;
};

function parseArgs(raw: string | undefined): BtwArgs {
	const text = String(raw ?? "").trim();
	if (!text) return {};
	try {
		const parsed = JSON.parse(text) as BtwArgs;
		return parsed && typeof parsed === "object" ? parsed : {};
	} catch {
		return { question: text };
	}
}

function truncateText(text: string, maxChars: number): string {
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

function buildSideContext(branch: Array<Record<string, unknown>>): string {
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

function sideQuestionInstruction(question: string): string {
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

function resolveModel(
	ctx: ExtensionCommandContext,
	modelRef: string | undefined,
) {
	if (!modelRef || !modelRef.trim()) return undefined;
	const sessionModel = ctx.model;
	const raw = modelRef.trim();
	const canonicalSeparator = raw.indexOf("::");
	const slash = raw.indexOf("/");
	let provider: string | undefined;
	let id: string;
	if (canonicalSeparator > 0) {
		provider = raw.slice(0, canonicalSeparator);
		id = raw.slice(canonicalSeparator + 2);
	} else if (slash > 0) {
		provider = raw.slice(0, slash);
		id = raw.slice(slash + 1);
	} else {
		provider = sessionModel?.provider;
		id = raw;
	}
	if (provider) {
		const found = ctx.modelRegistry.find(provider, id);
		if (found) return found;
	}
	const all = ctx.modelRegistry.getAll();
	return all.find(
		(m) =>
			m.id === id ||
			`${m.provider}/${m.id}` === raw ||
			`${m.provider}::${m.id}` === raw,
	);
}

function modelChain(
	parsed: BtwArgs,
	sessionModel: { provider?: string; id?: string } | undefined,
): string[] {
	const out: string[] = [];
	const push = (ref: string | undefined) => {
		const t = (ref ?? "").trim();
		if (!t) return;
		if (!out.includes(t)) out.push(t);
	};
	if (Array.isArray(parsed.models)) {
		for (const m of parsed.models) push(typeof m === "string" ? m : undefined);
	}
	push(parsed.model);
	if (out.length === 0 && sessionModel?.id) {
		const p = sessionModel.provider;
		push(p ? `${p}::${sessionModel.id}` : sessionModel.id);
	}
	return out;
}

export default function (pi: ExtensionAPI) {
	// Live bridge traffic must never reach the LLM: sendMessage would push
	// custom messages into agent.state.messages when idle (convertToLlm maps
	// them onto user messages) or steer the parent mid-turn when streaming.
	// appendEntry keeps deltas/answers out of the loop entirely — same pattern
	// as pi-grok-subagents live traffic.
	function emit(
		requestId: string,
		payload: {
			ok: boolean;
			phase?: "delta" | "complete";
			delta?: string;
			answer?: string;
			error?: string;
			modelUsed?: string;
		},
	) {
		pi.appendEntry(BRIDGE_TYPE, {
			version: 1,
			requestId,
			...payload,
		});
	}

	pi.registerCommand(COMMAND, {
		description: "Internal Pi-Grok bridge: /btw side question",
		handler: async (args, ctx: ExtensionCommandContext) => {
			const parsed = parseArgs(args);
			const requestId =
				String(parsed.requestId ?? "").trim() || `btw-${Date.now()}`;
			const question = String(parsed.question ?? "").trim();
			if (!question) {
				emit(requestId, { ok: false, error: "Empty side question" });
				return;
			}

			try {
				const branch = ctx.sessionManager.getBranch() as Array<
					Record<string, unknown>
				>;
				const conversation = buildSideContext(branch);
				const chain = modelChain(
					parsed,
					ctx.model as { provider?: string; id?: string } | undefined,
				);
				if (chain.length === 0) {
					emit(requestId, {
						ok: false,
						error:
							"No model available for /btw. Configure btw models in F2 or select a session model.",
					});
					return;
				}

				const userMessage: Message = {
					role: "user",
					content: [
						{
							type: "text",
							text: conversation
								? `${sideQuestionInstruction(question)}\n\n<conversation>\n${conversation}\n</conversation>`
								: sideQuestionInstruction(question),
						},
					],
					timestamp: Date.now(),
				};

				const errors: string[] = [];
				for (const modelRef of chain) {
					const model = resolveModel(ctx, modelRef);
					if (!model) {
						errors.push(`${modelRef}: not found`);
						continue;
					}
					const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
					if (!auth.ok || !auth.apiKey) {
						errors.push(`${modelRef}: no API key`);
						continue;
					}
					try {
						const stream = streamSimple(
							model,
							{ messages: [userMessage] },
							{
								apiKey: auth.apiKey,
								headers: auth.headers,
								env: auth.env,
								reasoning:
									model.reasoning &&
									parsed.thinkingLevel &&
									parsed.thinkingLevel !== "max"
										? parsed.thinkingLevel
										: model.reasoning && parsed.thinkingLevel === "max"
											? "xhigh"
											: undefined,
							},
						);
						for await (const event of stream) {
							if (event.type === "text_delta" && event.delta) {
								emit(requestId, {
									ok: true,
									phase: "delta",
									delta: event.delta,
								});
							}
						}
						const response = await stream.result();
						if (
							response.stopReason === "aborted" ||
							response.stopReason === "error"
						) {
							errors.push(`${modelRef}: ${response.stopReason}`);
							continue;
						}
						const answer = (response.content ?? [])
							.filter(
								(c): c is { type: "text"; text: string } => c.type === "text",
							)
							.map((c) => c.text)
							.join("\n")
							.trim();
						if (!answer) {
							errors.push(`${modelRef}: empty response`);
							continue;
						}
						const modelUsed = `${model.provider}::${model.id}`;
						const historyEntry: BtwHistoryEntry = {
							version: 1,
							requestId,
							question,
							answer,
							createdAt: Date.now(),
							modelUsed,
						};
						// Custom entries are durable Pi state and do not participate
						// in the main agent context. The dedicated history type keeps
						// /btw-history separate from live bridge traffic.
						pi.appendEntry(HISTORY_ENTRY_TYPE, historyEntry);
						emit(requestId, {
							ok: true,
							phase: "complete",
							answer,
							modelUsed,
						});
						return;
					} catch (e) {
						errors.push(
							`${modelRef}: ${e instanceof Error ? e.message : String(e)}`,
						);
					}
				}

				emit(requestId, {
					ok: false,
					error: `All /btw models failed. Reconfigure F2 btw models. (${errors.join("; ")})`,
				});
			} catch (e) {
				emit(requestId, {
					ok: false,
					error: e instanceof Error ? e.message : String(e),
				});
			}
		},
	});

	// The Pager invokes this direct Pi command to ask the adapter to project
	// persisted custom entries onto the native scrollback. The handler itself
	// intentionally performs no model work.
	pi.registerCommand(HISTORY_COMMAND, {
		description: "Show saved /btw answers",
		handler: () => {},
	});

	// Belt-and-braces for sessions recorded by older builds: their bridge
	// deltas/answers were persisted as display:false custom messages, which
	// reloads restore into agent.state.messages. Strip them from every LLM
	// call so legacy entries stay out of the loop too.
	pi.on("context", (event) => {
		const messages = event.messages.filter((message) => {
			if (message.role === "custom") return message.customType !== BRIDGE_TYPE;
			return true;
		});
		return { messages };
	});
}
