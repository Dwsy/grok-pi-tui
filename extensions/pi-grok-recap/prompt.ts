import { MERMAID_LR_MIN_COLS } from "./shared.ts";

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

export function recapInstruction(
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
