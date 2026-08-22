/**
 * Model resolution and fallback-chain construction for /btw.
 *
 * The chain comes from adapter-provided args (F2 `[ui].pi_btw` model list)
 * and falls back to the session model. Refs accept `provider::id`,
 * `provider/id`, or bare `id` (resolved against the session provider).
 */

import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { BtwArgs } from "./shared.ts";

export function resolveModel(
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

export function modelChain(
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
