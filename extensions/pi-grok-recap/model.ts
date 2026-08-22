import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { RecapArgs } from "./shared.ts";

export function resolveModel(ctx: ExtensionCommandContext, modelRef: string | undefined) {
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
export function modelChain(
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
