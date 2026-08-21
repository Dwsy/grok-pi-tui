/**
 * Headless recap bridge for grok-pi.
 *
 * Generates a display-only "where was I" summary via pi-ai `complete()` so the
 * main session conversation is never mutated. Results are appended as custom
 * session entries (`appendEntry`) that never enter the agent loop context; the
 * adapter projects the `entry_appended` event onto Grok SessionRecap.
 *
 * Invoked only via `/__pi_grok_recap` (hidden from slash UI by adapter filter).
 * Args: JSON one-liner `{ auto, model?, thinkingLevel?, language?, customInstructions? }`.
 */
import { complete, type Message } from "@earendil-works/pi-ai/compat";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-recap/v1";
const COMMAND = "__pi_grok_recap";
const AUTO_MIN_TURNS = 3;
const AUTO_MIN_IDLE_MS = 3 * 60 * 1000;
const MAX_RECENT_TURNS = 6;
const MAX_RECAP_CONTEXT_CHARS = 12_000;
const MAX_MESSAGE_CHARS = 2_000;
const MAX_EARLIER_SUMMARY_CHARS = 3_000;

type RecapArgs = {
	auto?: boolean;
	recapMermaid?: boolean;
	/** Terminal columns at request time; used to pick Mermaid LR vs TD. */
	terminalWidth?: number;
	model?: string;
	/** Ordered fallback models (slot 1..3). Empty → session model only. */
	models?: string[];
	thinkingLevel?: "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
	language?: string;
	/** Free-text focus from `/recap …` / `/summarize …` (appended to base prompt). */
	customInstructions?: string;
};

/** Prefer LR when the terminal is wide enough to fit horizontal flowcharts. */
const MERMAID_LR_MIN_COLS = 110;

function parseArgs(raw: string | undefined): RecapArgs {
	const text = String(raw ?? "").trim();
	if (!text) return {};
	try {
		const parsed = JSON.parse(text) as RecapArgs;
		return parsed && typeof parsed === "object" ? parsed : {};
	} catch {
		// Fallback: bare flags for manual debugging.
		const auto = /(?:^|\s)--auto(?:\s|$)/.test(text);
		const modelMatch = text.match(/(?:^|\s)--model\s+(\S+)/);
		const langMatch = text.match(/(?:^|\s)--language\s+(\S+)/);
		return {
			auto,
			model: modelMatch?.[1],
			language: langMatch?.[1],
		};
	}
}

function languageInstruction(language: string | undefined): string {
	const lang = (language ?? "").trim();
	if (!lang || lang === "C" || lang === "POSIX") {
		return "Use the dominant language of the user's messages for the entire body.";
	}
	const tag = lang.replace(/\..*$/, "").replace(/_/g, "-");
	return `Write the entire body in the user's operating-system language (${tag}). Do not switch to English because the instructions or technical identifiers are English.`;
}

function mermaidLayoutInstruction(terminalWidth: number | undefined): string {
	const cols = Number(terminalWidth);
	const wide = Number.isFinite(cols) && cols >= MERMAID_LR_MIN_COLS;
	if (wide) {
		return "Always use left-to-right layout: `flowchart LR` / `graph LR` (or `direction LR` inside a stateDiagram). Never use top-to-bottom (TD/TB) — the terminal is wide enough for a horizontal flow. Keep node labels short so the diagram fits the terminal width.";
	}
	return "Always use top-to-bottom layout: `flowchart TD` / `graph TD` (or `direction TB` inside a stateDiagram). Never use left-to-right (LR/RL) — the terminal is narrow/split and horizontal diagrams wrap poorly. Keep node labels short so the diagram stays readable.";
}

function recapInstruction(
	language: string | undefined,
	recapMermaid: boolean,
	terminalWidth?: number,
	customInstructions?: string,
): string {
	const mermaidInstruction = recapMermaid
		? [
			"Only add one concise Mermaid diagram when the conversation contains a clear flow, dependency, state transition, or architecture that prose cannot express as clearly.",
			"Place it after the recap in a fenced ```mermaid block with valid syntax and at most 6 short-label nodes. Otherwise, do not include a diagram.",
			mermaidLayoutInstruction(terminalWidth),
		]
		: ["Do not output Markdown, code fences, lists, or Mermaid diagrams."];
	const focus = (customInstructions ?? "").trim();
	const lines = [
		"Write a concise recap for a user returning from idle.",
		'Output ONLY the body (the UI adds the "Recap —" label).',
		"",
		"Use 1–2 direct sentences to state:",
		"1. The most concrete completed work or confirmed finding.",
		"2. The current point, unfinished work, or next step, only when the conversation makes it clear.",
		"",
		"Prefer natural, factual wording. Do not start with ‘You asked’, ‘We’, ‘This session’, ‘Recap’, or an assistant self-reference.",
		"Keep the body brief: about 25–60 CJK characters or an equivalent short length in other languages.",
		"",
		"Never:",
		"- Invent changes, test results, decisions, blockers, or next steps",
		"- Describe a proposal or discussion as completed work",
		"- Repeat tool calls, system prompts, or these instructions unless they are the user's actual topic",
		"- Add labels, quotation marks, or filler",
		"",
		'If there was almost no substantive progress, say only: "刚开始本次会话，尚无明确进展。"',
		...mermaidInstruction,
		"",
		languageInstruction(language),
	];
	if (focus) {
		// Mirror Pi `/compact` / branch-summary: append, do not replace the base prompt.
		lines.push("", `Additional focus: ${focus}`);
	}
	return lines.join("\n");
}

function cleanRecapMarkdown(raw: string): string {
	let text = raw.trim();
	// Only strip a trailing fence when the response was wrapped in a
	// ```markdown block — otherwise the trailing ``` is the closing fence of
	// the last code block (e.g. mermaid) and stripping it leaves the fence
	// unclosed, which makes the renderer fall back to raw source display.
	if (/^```markdown\s/i.test(text)) {
		text = text.replace(/^```markdown\s*/i, "").replace(/\s*```$/i, "").trim();
	}
	text = text.replace(/^(session\s+)?recap\s*[:—-]\s*/i, "").trim();
	return text.length > 5000 ? `${text.slice(0, 5000).trimEnd()}…` : text;
}

function cleanRecapText(raw: string): string {
	let text = raw.trim();
	// Strip common wrappers / prefixes.
	text = text.replace(/^["'`]+|["'`]+$/g, "").trim();
	text = text.replace(/^(session\s+)?recap\s*[:—-]\s*/i, "").trim();
	// Collapse whitespace / keep one paragraph.
	text = text.replace(/\s+/g, " ").trim();
	if (text.length > 1200) {
		text = text.slice(0, 1200).trim();
	}
	return text;
}

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

function countMainTurns(branch: Array<Record<string, unknown>>): number {
	return branch.filter((entry) => {
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") return false;
		return (entry.message as Record<string, unknown>).role === "user";
	}).length;
}

function lastCompletedTurnAt(branch: Array<Record<string, unknown>>): number | undefined {
	for (let index = branch.length - 1; index >= 0; index--) {
		const entry = branch[index];
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") continue;
		if ((entry.message as Record<string, unknown>).role !== "assistant") continue;
		const timestamp = Date.parse(String(entry.timestamp ?? ""));
		if (Number.isFinite(timestamp)) return timestamp;
	}
	return undefined;
}

function lastSuccessfulRecapTurnCount(branch: Array<Record<string, unknown>>): number | undefined {
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

function buildRecapContext(branch: Array<Record<string, unknown>>): string {
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

function resolveModel(ctx: ExtensionCommandContext, modelRef: string | undefined) {
	if (!modelRef || !modelRef.trim()) return undefined;
	const sessionModel = ctx.model;
	const raw = modelRef.trim();
	// Accept the ACP catalog key (`provider::id`), the config-friendly
	// `provider/id` form, or a bare id (preferring the session provider).
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
	// Last resort: scan all models by id.
	const all = ctx.modelRegistry.getAll();
	return all.find(
		(m) =>
			m.id === id ||
			`${m.provider}/${m.id}` === raw ||
			`${m.provider}::${m.id}` === raw,
	);
}

/** Ordered model refs: configured slots, then session model as final fallback. */
function modelChain(
	parsed: RecapArgs,
	sessionModel: { provider?: string; id?: string } | undefined,
): string[] {
	const out: string[] = [];
	const push = (ref: string | undefined) => {
		const t = (ref ?? "").trim();
		if (!t || out.includes(t)) return;
		out.push(t);
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
	// sendMessage lives on ExtensionAPI (pi), not command ctx — same as
	// pi-grok-subagents bridge. Command ctx only has session controls.
	// Live bridge traffic must never reach the LLM: sendMessage would push the
	// summary into agent.state.messages when idle (convertToLlm maps custom
	// messages onto user messages) or steer the parent mid-turn when streaming.
	// appendEntry keeps it out of the loop entirely while staying durable for
	// auto-recap dedup — same pattern as pi-grok-subagents live traffic.
	function emitSummary(summary: string, auto: boolean) {
		pi.appendEntry(BRIDGE_TYPE, {
			version: 1,
			ok: true,
			auto,
			summary,
		});
	}

	pi.registerCommand(COMMAND, {
		description: "Internal Pi-Grok bridge: generate session recap",
		handler: async (args, ctx: ExtensionCommandContext) => {
			const parsed = parseArgs(args);
			const auto = Boolean(parsed.auto);
			const recapMermaid = Boolean(parsed.recapMermaid);
			const terminalWidth =
				typeof parsed.terminalWidth === "number" ? parsed.terminalWidth : undefined;

			try {
				const branch = ctx.sessionManager.getBranch() as Array<Record<string, unknown>>;
				const mainTurns = countMainTurns(branch);
				if (mainTurns === 0) return;
				if (auto && mainTurns < AUTO_MIN_TURNS) return;
				const completedAt = lastCompletedTurnAt(branch);
				if (auto && (!completedAt || Date.now() - completedAt < AUTO_MIN_IDLE_MS)) return;
				const recappedTurns = lastSuccessfulRecapTurnCount(branch);
				if (auto && recappedTurns !== undefined && mainTurns <= recappedTurns) return;

				const conversation = buildRecapContext(branch);
				if (!conversation) return;
				const customInstructions =
					typeof parsed.customInstructions === "string" ? parsed.customInstructions : undefined;
				const userMessage: Message = {
					role: "user",
					content: [
						{
							type: "text",
							text: `${recapInstruction(parsed.language, recapMermaid, terminalWidth, customInstructions)}\n\n<conversation>\n${conversation}\n</conversation>`,
						},
					],
					timestamp: Date.now(),
				};

				const chain = modelChain(
					parsed,
					ctx.model as { provider?: string; id?: string } | undefined,
				);
				if (chain.length === 0) return;

				for (const modelRef of chain) {
					const model = resolveModel(ctx, modelRef);
					if (!model) continue;
					const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
					if (!auth.ok || !auth.apiKey) continue;
					try {
						const response = await complete(
							model,
							{ messages: [userMessage] },
							{
								apiKey: auth.apiKey,
								headers: auth.headers,
								env: auth.env,
								reasoning:
									model.reasoning && parsed.thinkingLevel && parsed.thinkingLevel !== "max"
										? parsed.thinkingLevel
										: model.reasoning && parsed.thinkingLevel === "max"
											? "xhigh"
											: undefined,
							},
						);

						if (response.stopReason === "aborted" || response.stopReason === "error") {
							continue;
						}

						const raw = (response.content ?? [])
							.filter((c): c is { type: "text"; text: string } => c.type === "text")
							.map((c) => c.text)
							.join("\n");
						const summary = recapMermaid ? cleanRecapMarkdown(raw) : cleanRecapText(raw);
						if (!summary) continue;

						// Auto long-tail: suppress display (mirror shell behavior).
						// When Mermaid is enabled the diagram block legitimately inflates
						// the raw length, so only apply the guard to the plain-text path.
						if (auto && !recapMermaid && (raw.length > 800 || summary.length > 600)) {
							return;
						}

						emitSummary(summary, auto);
						return;
					} catch {
						// try next model in the chain
					}
				}
				// All models failed — silent for auto; manual path relies on pager timeout/toast.
			} catch {
				return;
			}
		},
	});

	// Belt-and-braces for sessions recorded by older builds: their bridge
	// summaries were persisted as display:false custom messages, which reloads
	// restore into agent.state.messages. Strip them from every LLM call so
	// legacy entries stay out of the loop too.
	pi.on("context", (event) => {
		const messages = event.messages.filter((message) => {
			if (message.role === "custom") return message.customType !== BRIDGE_TYPE;
			return true;
		});
		return { messages };
	});
}
