import type { RecapArgs } from "./shared.ts";

export function parseArgs(raw: string | undefined): RecapArgs {
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
