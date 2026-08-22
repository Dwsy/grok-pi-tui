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
 *
 * Module map: shared.ts (constants/types), args.ts (arg parsing),
 * prompt.ts (instruction text), clean.ts (response sanitizing),
 * session.ts (branch scanning/context budget), model.ts (model chain).
 */
import { complete, type Message } from "@earendil-works/pi-ai/compat";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { parseArgs } from "./args.ts";
import { cleanRecapMarkdown, cleanRecapText } from "./clean.ts";
import { resolveModel, modelChain } from "./model.ts";
import { recapInstruction } from "./prompt.ts";
import {
	AUTO_MIN_IDLE_MS,
	AUTO_MIN_TURNS,
	BRIDGE_TYPE,
	COMMAND,
} from "./shared.ts";
import {
	buildRecapContext,
	countMainTurns,
	lastCompletedTurnAt,
	lastSuccessfulRecapTurnCount,
} from "./session.ts";

export default function (pi: ExtensionAPI) {
	// appendEntry keeps bridge traffic out of the LLM loop entirely while
	// staying durable for auto-recap dedup — same pattern as pi-grok-subagents.
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
