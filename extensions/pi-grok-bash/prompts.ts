import type { EvalV2LanguageSelection, EvalVersion } from "./eval.ts";

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
	"Eval Bridge v2 host calls must be awaited. When data needed for an ongoing computation is available through an active session tool, call it inside eval with tool.<name>(...) instead of doing a top-level tool round trip. Discover tools synchronously with tools.list()/search()/describe(name); tool.<name>.schema/.description expose the same per-tool metadata snapshot. In JavaScript Object.keys(tool) is also available; in Python use tool.keys(). Use skills.list()/search()/describe() to discover Pi-loaded model-invokable skills, then await skills.read(name) to read only an admitted skill file.";
const EVAL_V2_CONCURRENCY_GUIDELINE =
	"In Eval Bridge v2, use parallel([...]) for independent async operations and pipeline(items, ...stages) for staged fan-out/fan-in work. Set is_background=true for long-running isolated Eval work; manage the returned task_id with get_task_output, wait_tasks, or kill_task. Do not invoke eval recursively through tool.eval.";
const EVAL_V2_AGENT_GUIDELINE =
	"Eval Bridge v2 agent(prompt, options) is a blocking leaf wrapper around the active spawn_subagent tool. background=true is rejected; use parallel([() => agent(...), ...]) for concurrent leaf agents. Use agent only when spawn_subagent is active.";
const EVAL_V2_COMPLETION_GUIDELINE =
	"In Eval Bridge v2, completion(prompt, options) is a one-shot model call with no session history and no tools. Use it for cheap classification, extraction, synthesis, or local subproblems that do not need an agent loop.";

function evalV2LanguageLabel(selection: EvalV2LanguageSelection) {
	if (selection === "js") return "JavaScript";
	if (selection === "py") return "Python";
	return "Python or JavaScript";
}

function buildEvalV2Prompts(completionAvailable: boolean, selection: EvalV2LanguageSelection): EvalPromptBundle {
	const languageLabel = evalV2LanguageLabel(selection);
	let completionHelper = "";
	let completionDescription = "";
	let completionSnippet = "";
	const promptGuidelines = [
		`Prefer eval over bash for non-shell computation: calculations, parsing or transforming JSON/text/data, collection analysis, quick algorithms/experiments, and multi-step ${languageLabel}. Each cell has a fresh lexical scope; use store(key, value)/load(key) only for JSON-serializable state that must cross cells.`,
		"Eval v2 cells do not inherit prior cell-local bindings, so reuse local names freely. Persist only the minimal data needed by later cells with store/load; reset clears that stored state.",
		`Do not use bash with inline interpreters, heredocs, or temporary scripts for ordinary ${languageLabel} computation that eval can perform; reserve bash for shell-native filesystem/process/git/build/package/pipeline work.`,
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
			`Code to run verbatim in an isolated Eval Bridge v2 ${languageLabel} cell. Top-level await is supported; cross-cell data persistence is explicit through store/load; host helpers include tool/tools, skills, parallel, pipeline, and agent${completionHelper}.`,
		description:
			`Run a ${languageLabel} computation in Eval Bridge v2 with an isolated lexical scope per cell. ` +
			EVAL_COMMON_DESCRIPTION +
			"Eval Bridge v2 can call active session tools from inside a cell with await tool.<name>(...); discover active tools with tools.list()/search()/describe(), inspect schema/description with tools.describe(name) or tool.<name>.schema/.description, and use Object.keys(tool) in JavaScript or tool.keys() in Python. Discover loaded model-invokable skills with skills.list()/search()/describe() and read a trusted skill with await skills.read(name). Persist JSON-serializable data explicitly across cells with store(key, value) and load(key). Helpers parallel(...), pipeline(items, ...stages), and agent(prompt, options) are also available. Long-running foreground Eval may automatically become a background task; is_background=true starts one immediately. Manage task_id with get_task_output/wait_tasks/kill_task. Always await host calls. " +
			completionDescription +
			EVAL_BASH_BOUNDARY +
			"Values saved with store/load survive across foreground cells until reset, timeout, abort, process failure, cwd change, automatic background promotion, or session shutdown.",
		promptSnippet:
			`Eval Bridge v2: isolated ${languageLabel} cells with explicit store/load persistence, awaited host-tool calls, parallel/pipeline, agent${completionSnippet}, and managed background tasks.`,
		promptGuidelines,
	};
}

export function buildEvalPrompts(
	evalVersion: EvalVersion,
	completionAvailable: boolean,
	evalV2Language: EvalV2LanguageSelection = "js",
): EvalPromptBundle {
	if (evalVersion === "v1") return EVAL_V1_PROMPTS;
	return buildEvalV2Prompts(completionAvailable, evalV2Language);
}

const BASH_TASK_NAME_DESCRIPTION =
	"Always set task_name: a short human-readable UI title (3–8 words) in the user's language " +
	"(match the language of their messages). Required for every call — foreground and background. " +
	"This is what the terminal UI shows instead of the raw shell, especially for long/complex commands. " +
	"Never annotate command with # comments — put the label in task_name only.";
const BASH_BACKGROUND_WAIT_GUIDELINE =
	"A long-running foreground Bash call may be automatically backgrounded and return a task_id. When wait_tasks/get_task_output returns a running task at the configured max-wait cap, continue the agent loop and call it again if the result is still needed.";

export function buildBashPrompts(
	evalVersion: EvalVersion,
	nativeDescription: string,
	nativeGuidelines: readonly string[] = [],
	evalV2Language: EvalV2LanguageSelection = "js",
): ToolPromptBundle {
	let evalPreference =
		"Use bash for shell-native filesystem/process/git/build/package/pipeline work; prefer eval for Python/JavaScript calculations, parsing, data transforms, and stateful experiments. ";
	let promptSnippet =
		"Run shell-native commands; prefer eval for Python/JavaScript computation. Always pass task_name in user's language.";
	let evalGuideline =
		"Use bash for shell-native filesystem/process/git/build/package/pipeline work. Prefer eval for Python/JavaScript calculations, parsing/data transforms, quick experiments, or stateful multi-step computation.";

	if (evalVersion === "v2") {
		const languageLabel = evalV2LanguageLabel(evalV2Language);
		evalPreference =
			`Use bash for shell-native filesystem/process/git/build/package/pipeline work; prefer eval for ${languageLabel} calculations, parsing, data transforms, and multi-step work with explicit store/load persistence. `;
		promptSnippet =
			`Run shell-native commands; prefer eval for ${languageLabel} computation. Always pass task_name in user's language.`;
		evalGuideline =
			`Use bash for shell-native filesystem/process/git/build/package/pipeline work. Prefer eval for ${languageLabel} calculations, parsing/data transforms, quick experiments, or multi-step computation with explicit store/load persistence.`;
	}

	return {
		description: `${nativeDescription} ${evalPreference}${BASH_TASK_NAME_DESCRIPTION}`,
		promptSnippet,
		promptGuidelines: [...nativeGuidelines, evalGuideline, BASH_BACKGROUND_WAIT_GUIDELINE],
	};
}
