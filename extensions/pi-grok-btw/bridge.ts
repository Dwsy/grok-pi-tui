/**
 * /btw command execution: arg parsing, bridge emission, and the streaming
 * model-chain loop.
 *
 * Live bridge traffic must never reach the LLM: sendMessage would push custom
 * messages into agent.state.messages when idle (convertToLlm maps them onto
 * user messages) or steer the parent mid-turn when streaming. appendEntry
 * keeps deltas/answers out of the loop entirely — same pattern as
 * pi-grok-subagents live traffic.
 */

import { type Message, streamSimple } from "@earendil-works/pi-ai/compat";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";
import { buildSideContext, sideQuestionInstruction } from "./context.ts";
import { modelChain, resolveModel } from "./models.ts";
import {
	BRIDGE_TYPE,
	HISTORY_ENTRY_TYPE,
	type BtwArgs,
	type BtwHistoryEntry,
} from "./shared.ts";

export function parseArgs(raw: string | undefined): BtwArgs {
	const text = String(raw ?? "").trim();
	if (!text) return {};
	try {
		const parsed = JSON.parse(text) as BtwArgs;
		return parsed && typeof parsed === "object" ? parsed : {};
	} catch {
		return { question: text };
	}
}

type BridgePayload = {
	ok: boolean;
	phase?: "delta" | "complete";
	delta?: string;
	answer?: string;
	error?: string;
	modelUsed?: string;
};

function emit(pi: ExtensionAPI, requestId: string, payload: BridgePayload) {
	pi.appendEntry(BRIDGE_TYPE, {
		version: 1,
		requestId,
		...payload,
	});
}

export async function handleBtwCommand(
	pi: ExtensionAPI,
	args: string | undefined,
	ctx: ExtensionCommandContext,
): Promise<void> {
	const parsed = parseArgs(args);
	const requestId = String(parsed.requestId ?? "").trim() || `btw-${Date.now()}`;
	const question = String(parsed.question ?? "").trim();
	if (!question) {
		emit(pi, requestId, { ok: false, error: "Empty side question" });
		return;
	}

	try {
		const branch = ctx.sessionManager.getBranch() as unknown as Array<
			Record<string, unknown>
		>;
		const conversation = buildSideContext(branch);
		const chain = modelChain(
			parsed,
			ctx.model as { provider?: string; id?: string } | undefined,
		);
		if (chain.length === 0) {
			emit(pi, requestId, {
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
						emit(pi, requestId, {
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
				emit(pi, requestId, {
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

		emit(pi, requestId, {
			ok: false,
			error: `All /btw models failed. Reconfigure F2 btw models. (${errors.join("; ")})`,
		});
	} catch (e) {
		emit(pi, requestId, {
			ok: false,
			error: e instanceof Error ? e.message : String(e),
		});
	}
}
