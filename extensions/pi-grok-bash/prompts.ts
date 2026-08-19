import type { EvalVersion } from "./eval.ts";

export type ToolPromptBundle = {
	description: string;
	promptSnippet: string;
	promptGuidelines: string[];
};

export type EvalPromptBundle = ToolPromptBundle & {
	codeDescription: string;
};

const EVAL_COMMON_DESCRIPTION =
	"Prefer eval over bash for calculations, JSON/text/data parsing or transformation, quick experiments, and multi-step logic. ";
const EVAL_BASH_BOUNDARY =
	"Use bash instead for shell-native filesystem, process, git, build, package-manager, or pipeline commands. ";

const EVAL_V1_PROMPTS: EvalPromptBundle = {
	codeDescription: "Code to run verbatim in the selected persistent Eval v1 kernel. Top-level await is supported.",
	description:
		"Run a small, stateful Python or Node.js JavaScript computation in a persistent per-language Eval v1 kernel. " +
		EVAL_COMMON_DESCRIPTION +
		"Work incrementally: set up once, then inspect/transform/test in later cells; after an error, fix and rerun only the failing step. " +
		EVAL_BASH_BOUNDARY +
		"State survives across calls until reset, timeout, abort, process failure, cwd change, or session shutdown.",
	promptSnippet: "Prefer for calculations, parsing/data transforms, quick experiments, and stateful multi-step Python/JavaScript; reuse prior cells.",
	promptGuidelines: [
		"Prefer eval over bash for non-shell computation: calculations, parsing or transforming JSON/text/data, collection analysis, quick algorithms/experiments, and multi-step Python/JavaScript that benefits from persistent state.",
		"Use eval incrementally: import/setup once, then reuse prior variables and functions in later cells. After an error, fix only the failing step; reset only when state is stale or the kernel must restart.",
		"Do not use bash with python -c, node -e, heredocs, or temporary scripts for ordinary computation that eval can perform; reserve bash for shell-native filesystem/process/git/build/package/pipeline work.",
	],
};

const EVAL_V2_HOST_GUIDELINE =
	"Eval Bridge v2 host calls must be awaited. When data needed for an ongoing computation is available through an active session tool, call it inside eval with tool.<name>(...) instead of doing a top-level tool round trip. Discover tools synchronously with Object.keys(tool), tools.list()/search(), and tools.describe(name); tool.<name>.schema/.description expose the same per-tool metadata snapshot. Use skills.list()/search()/describe() to discover Pi-loaded model-invokable skills, then await skills.read(name) to read only an admitted skill file.";
const EVAL_V2_CONCURRENCY_GUIDELINE =
	"In Eval Bridge v2, use parallel([...]) for independent async operations and pipeline(items, ...stages) for staged fan-out/fan-in work. Set is_background=true for long-running isolated Eval work; manage the returned task_id with get_task_output, wait_tasks, or kill_task. Do not invoke eval recursively through tool.eval.";
const EVAL_V2_AGENT_GUIDELINE =
	"Eval Bridge v2 agent(prompt, options) is a convenience wrapper around the active spawn_subagent tool; pass {background:true} to return a background subagent handle, and use the active task-output tool to retrieve it. Use agent only when spawn_subagent is active.";
const EVAL_V2_COMPLETION_GUIDELINE =
	"In Eval Bridge v2, completion(prompt, options) is a one-shot model call with no session history and no tools. Use it for cheap classification, extraction, synthesis, or local subproblems that do not need an agent loop.";

function buildEvalV2Prompts(completionAvailable: boolean): EvalPromptBundle {
	let completionHelper = "";
	let completionDescription = "";
	let completionSnippet = "";
	const promptGuidelines = [
		"Prefer eval over bash for non-shell computation: calculations, parsing or transforming JSON/text/data, collection analysis, quick algorithms/experiments, and multi-step JavaScript. Each cell has a fresh lexical scope; use store(key, value)/load(key) only for JSON-serializable state that must cross cells.",
		"Eval v2 cells do not inherit prior let/const/function/class bindings, so reuse local names freely. Persist only the minimal data needed by later cells with store/load; reset clears that stored state.",
		"Do not use bash with node -e, heredocs, or temporary scripts for ordinary JavaScript computation that eval can perform; reserve bash for shell-native filesystem/process/git/build/package/pipeline work.",
		EVAL_V2_HOST_GUIDELINE,
	];

	if (completionAvailable) {
		completionHelper = ", plus completion";
		completionDescription = "completion(prompt, options) runs a one-shot model call with no session history or tools. ";
		completionSnippet = ", plus one-shot completion";
		promptGuidelines.push(EVAL_V2_COMPLETION_GUIDELINE);
	}

	promptGuidelines.push(EVAL_V2_CONCURRENCY_GUIDELINE, EVAL_V2_AGENT_GUIDELINE);

	return {
		codeDescription:
			`Code to run verbatim in an isolated Eval Bridge v2 JavaScript cell. Top-level await is supported; cross-cell data persistence is explicit through store/load; host helpers include tool/tools, skills, parallel, pipeline, and agent${completionHelper}.`,
		description:
			"Run a Node.js JavaScript computation in Eval Bridge v2 with an isolated lexical scope per cell. " +
			EVAL_COMMON_DESCRIPTION +
			"Eval Bridge v2 is JavaScript-only and can call active session tools from inside a cell with await tool.<name>({...}); discover active names with Object.keys(tool) or tools.list(), inspect schema/description with tools.describe(name) or tool.<name>.schema/.description. Discover loaded model-invokable skills with skills.list()/search()/describe() and read a trusted skill with await skills.read(name). Each cell may freely reuse local let/const names; persist JSON-serializable data explicitly across cells with store(key, value) and load(key). Helpers parallel(thunks), pipeline(items, ...stages), and agent(prompt, options) are also available. Set is_background=true for an isolated background Eval task managed by get_task_output/wait_tasks/kill_task. Always await host calls. " +
			completionDescription +
			EVAL_BASH_BOUNDARY +
			"Values saved with store/load survive across cells until reset, timeout, abort, process failure, cwd change, or session shutdown.",
		promptSnippet:
			`Eval Bridge v2: isolated JavaScript cells with explicit store/load persistence, awaited host-tool calls, parallel/pipeline, and agent${completionSnippet}.`,
		promptGuidelines,
	};
}

export function buildEvalPrompts(evalVersion: EvalVersion, completionAvailable: boolean): EvalPromptBundle {
	if (evalVersion === "v1") return EVAL_V1_PROMPTS;
	return buildEvalV2Prompts(completionAvailable);
}

const BASH_TASK_NAME_DESCRIPTION =
	"Always set task_name: a short human-readable UI title (3–8 words) in the user's language " +
	"(match the language of their messages). Required for every call — foreground and background. " +
	"This is what the terminal UI shows instead of the raw shell, especially for long/complex commands. " +
	"Never annotate command with # comments — put the label in task_name only.";

export function buildBashPrompts(
	evalVersion: EvalVersion,
	nativeDescription: string,
	nativeGuidelines: readonly string[] = [],
): ToolPromptBundle {
	let evalPreference =
		"Use bash for shell-native filesystem/process/git/build/package/pipeline work; prefer eval for Python/JavaScript calculations, parsing, data transforms, and stateful experiments. ";
	let promptSnippet =
		"Run shell-native commands; prefer eval for Python/JavaScript computation. Always pass task_name in user's language.";
	let evalGuideline =
		"Use bash for shell-native filesystem/process/git/build/package/pipeline work. Prefer eval for Python/JavaScript calculations, parsing/data transforms, quick experiments, or stateful multi-step computation.";

	if (evalVersion === "v2") {
		evalPreference =
			"Use bash for shell-native filesystem/process/git/build/package/pipeline work; prefer eval for JavaScript calculations, parsing, data transforms, and multi-step work with explicit store/load persistence. ";
		promptSnippet =
			"Run shell-native commands; prefer eval for JavaScript computation. Always pass task_name in user's language.";
		evalGuideline =
			"Use bash for shell-native filesystem/process/git/build/package/pipeline work. Prefer eval for JavaScript calculations, parsing/data transforms, quick experiments, or multi-step computation with explicit store/load persistence.";
	}

	return {
		description: `${nativeDescription} ${evalPreference}${BASH_TASK_NAME_DESCRIPTION}`,
		promptSnippet,
		promptGuidelines: [...nativeGuidelines, evalGuideline],
	};
}
