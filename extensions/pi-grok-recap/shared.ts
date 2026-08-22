/** Shared constants and arg types for the pi-grok-recap bridge. */

export const BRIDGE_TYPE = "pi-grok-recap/v1";
export const COMMAND = "__pi_grok_recap";
export const AUTO_MIN_TURNS = 3;
export const AUTO_MIN_IDLE_MS = 3 * 60 * 1000;
export const MAX_RECENT_TURNS = 6;
export const MAX_RECAP_CONTEXT_CHARS = 12_000;
export const MAX_MESSAGE_CHARS = 2_000;
export const MAX_EARLIER_SUMMARY_CHARS = 3_000;

/** Prefer LR when the terminal is wide enough to fit horizontal flowcharts. */
export const MERMAID_LR_MIN_COLS = 110;

export type RecapArgs = {
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
