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
 *
 * Module map (all materialized by the Rust injector `btw_extension.rs`):
 * - `shared.ts`  — adapter-contract constants and wire types
 * - `context.ts` — branch → side-question transcript projection
 * - `models.ts`  — model ref resolution and fallback chain
 * - `bridge.ts`  — arg parsing, emission, streaming loop
 * - `index.ts`   — entry point: command registration + legacy context filter
 */
import type {
	ExtensionAPI,
	ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";
import { handleBtwCommand } from "./bridge.ts";
import { BRIDGE_TYPE, COMMAND, HISTORY_COMMAND } from "./shared.ts";

export default function (pi: ExtensionAPI) {
	pi.registerCommand(COMMAND, {
		description: "Internal Pi-Grok bridge: /btw side question",
		handler: async (args, ctx: ExtensionCommandContext) => {
			await handleBtwCommand(pi, args, ctx);
		},
	});

	// The Pager invokes this direct Pi command to ask the adapter to project
	// persisted custom entries onto the native scrollback. The handler itself
	// intentionally performs no model work.
	pi.registerCommand(HISTORY_COMMAND, {
		description: "Show saved /btw answers",
		handler: async () => {},
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
