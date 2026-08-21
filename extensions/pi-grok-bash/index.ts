/**
 * Pi Bash enhancement used only by grok-pi.
 *
 * The extension owns every Bash child process. This lets the native Pager
 * promote an active foreground tool call into its existing background-task UI
 * without rerunning the command. Pager still owns all visible task surfaces.
 */
import { readFile } from "node:fs/promises";

import { StringEnum, Type } from "@earendil-works/pi-ai";
import {
	createBashToolDefinition,
	type ExtensionAPI,
	type ExtensionContext,
} from "@earendil-works/pi-coding-agent";

import {
	type BackgroundTask,
	type BashParams,
	ORPHANED_SIGNAL,
	createBashControl,
	ensureTaskIds,
	jsonContent,
	killProcessTree,
	startTask,
	taskResult,
	waitForCompletion,
} from "./bash-tasks.ts";
import {
	type EvalHostCallHandler,
	type EvalLanguage,
	type EvalSkillMetadata,
	type EvalParams,
	type HostCallExecutionMode,
	HostCallGate,
	PersistentEvalKernel,
	evalHostToolValue,
	resolveEvalV2LanguageSelection,
	resolveEvalVersion,
} from "./eval.ts";
import {
	type EvalBackgroundTask,
	armEvalAutoBackground,
	evalTaskResult,
	killEvalTask,
	promoteEvalTask,
	startEvalBackgroundTask,
	waitForEvalTask,
} from "./eval-tasks.ts";
import { buildBashPrompts, buildEvalPrompts } from "./prompts.ts";
import { MAX_TIMEOUT_SECONDS, resolveMaxWaitMs } from "./shared.ts";
import { EvalSessionToolBridge } from "./tool-bridge.ts";
export { EvalSessionToolBridge } from "./tool-bridge.ts";

const EVAL_V2_PARALLEL_HOST_CALL_LIMIT = 4;

function envFlagDefaultOn(name: string): boolean {
	const raw = process.env[name]?.trim().toLowerCase();
	return raw === undefined || !["0", "false", "off", "no"].includes(raw);
}

function hostToolNameEnabled(name: string): boolean {
	const configured = process.env.PI_GROK_BUILTIN_TOOLS;
	if (configured !== undefined) {
		const selected = new Set(configured.split(",").map((value) => value.trim()).filter(Boolean));
		if (!selected.has(name)) return false;
	}
	const excluded = new Set(
		(process.env.PI_GROK_EXCLUDE_TOOLS ?? "")
			.split(",")
			.map((value) => value.trim())
			.filter(Boolean),
	);
	return !excluded.has(name);
}

export default async function (pi: ExtensionAPI) {
	const bashEnabled = envFlagDefaultOn("PI_GROK_BASH") && hostToolNameEnabled("bash");
	const maxWaitMs = resolveMaxWaitMs();
	const tasks = new Map<string, BackgroundTask>();
	const evalTasks = new Map<string, EvalBackgroundTask>();
	const control = bashEnabled ? createBashControl(tasks) : { sync: () => {}, close: () => {} };
	const publishTodoBacking = () => {
		const count = [...tasks.values()].filter((task) => task.backgrounded && !task.completed).length;
		pi.events.emit("pi-grok:todo-backing", { source: "bash", count });
	};
	const syncTaskState = () => {
		control.sync();
		publishTodoBacking();
	};
	if (bashEnabled) pi.on("session_start", publishTodoBacking);
	const nativeBash = createBashToolDefinition(process.cwd());
	const evalVersion = resolveEvalVersion();
	const evalV2Language = evalVersion === "v2" ? resolveEvalV2LanguageSelection() : "all";
	const evalV2Languages: EvalLanguage[] =
		evalV2Language === "all" ? ["js", "py"] : [evalV2Language];
	const evalV2Only = evalVersion === "v2" && process.env.PI_GROK_EVAL_V2_ONLY === "1";
	if (evalV2Only) {
		pi.on("session_start", () => pi.setActiveTools(["eval"]));
	}
	const evalToolBridge = evalVersion === "v2" ? new EvalSessionToolBridge(pi) : undefined;
	let evalSkills: EvalSkillMetadata[] = [];
	let evalSkillFiles = new Map<string, string>();
	if (evalVersion === "v2") {
		pi.on("before_agent_start", (event) => {
			const skills = (event.systemPromptOptions.skills ?? []).filter((skill) => !skill.disableModelInvocation);
			evalSkills = skills.map((skill) => ({
				name: skill.name,
				description: skill.description,
				filePath: skill.filePath,
				source: skill.sourceInfo,
			}));
			evalSkillFiles = new Map(skills.map((skill) => [skill.name, skill.filePath]));
		});
	}
	const completionAvailable = typeof (pi as ExtensionAPI & { complete?: unknown }).complete === "function";
	const evalPrompts = buildEvalPrompts(evalVersion, completionAvailable, evalV2Language);
	const evalHostCallGate = evalVersion === "v2" ? new HostCallGate(EVAL_V2_PARALLEL_HOST_CALL_LIMIT) : undefined;
	const invokeEvalHostCall: EvalHostCallHandler | undefined =
		evalVersion === "v2"
			? async (call, signal) => {
					if (!evalHostCallGate) throw new Error("eval v2 host call gate unavailable");
					if (call.method === "skill") {
						if (call.operation !== "read") throw new Error(`Unsupported Eval skill operation ${JSON.stringify(call.operation)}`);
						const filePath = evalSkillFiles.get(call.name);
						if (!filePath) throw new Error(`Skill ${JSON.stringify(call.name)} is not available in this session`);
						return readFile(filePath, "utf8");
					}
					if (call.method === "completion") {
						if (!call.prompt.trim()) throw new Error("eval v2 completion prompt must not be empty");
						const complete = (pi as ExtensionAPI & {
							complete?: (prompt: string, options: unknown, signal: AbortSignal) => Promise<unknown>;
						}).complete;
						if (typeof complete !== "function") {
							throw new Error("Eval v2 completion() is unavailable on this Pi host");
						}
						return evalHostCallGate.run("parallel", signal, () => complete.call(pi, call.prompt, call.options ?? {}, signal));
					}
					const toolName = call.tool.trim();
					if (!toolName) throw new Error("eval v2 host tool name must not be empty");
					if (toolName === "eval") throw new Error("eval v2 cannot recursively invoke the eval tool");
					if (!evalToolBridge) throw new Error("eval v2 host tool bridge unavailable");
					const executionMode: HostCallExecutionMode = evalToolBridge.executionMode(toolName);
					return evalHostCallGate.run(executionMode, signal, async () => {
						const result = await evalToolBridge.invoke(toolName, call.args ?? {}, signal);
						const value = evalHostToolValue(result.content);
						if (result.isError) throw new Error(value.text || `Tool ${toolName} failed`);
						return value;
					});
				}
			: undefined;
	let evalKernels: Partial<Record<EvalLanguage, PersistentEvalKernel>> =
		evalVersion === "v2"
			? evalV2Language === "js"
				? { js: new PersistentEvalKernel("js", evalVersion, invokeEvalHostCall) }
				: evalV2Language === "py"
					? { py: new PersistentEvalKernel("py", evalVersion, invokeEvalHostCall) }
					: {
							py: new PersistentEvalKernel("py", evalVersion, invokeEvalHostCall),
							js: new PersistentEvalKernel("js", evalVersion, invokeEvalHostCall),
						}
			: {
					py: new PersistentEvalKernel("py", evalVersion, invokeEvalHostCall),
					js: new PersistentEvalKernel("js", evalVersion, invokeEvalHostCall),
				};
	const EvalParameters = Type.Object({
		language:
			evalVersion === "v2"
				? evalV2Language === "js"
					? StringEnum(["js"] as const)
					: evalV2Language === "py"
						? StringEnum(["py"] as const)
						: StringEnum(["py", "js"] as const)
				: StringEnum(["py", "js"] as const),
		code: Type.String({
			minLength: 1,
			description: evalPrompts.codeDescription,
		}),
		title: Type.Optional(
			Type.String({
				minLength: 1,
				description: "Short transcript label for this cell",
			}),
		),
		timeout: Type.Optional(
			Type.Number({
				minimum: 0,
				maximum: MAX_TIMEOUT_SECONDS,
				description:
					"Cell timeout in seconds; defaults to 30. Set 0 to disable the timeout.",
			}),
		),
		reset: Type.Optional(
			Type.Boolean({
				description: "Destroy this language's kernel before running the cell",
			}),
		),
		...(evalVersion === "v2"
			? {
					is_background: Type.Optional(
						Type.Boolean({
							description: "Run this cell in an isolated background kernel and return a task_id",
						}),
					),
				}
			: {}),
	});

	pi.registerTool({
		name: "eval",
		label: "Eval",
		description: evalPrompts.description,
		promptSnippet: evalPrompts.promptSnippet,
		promptGuidelines: evalPrompts.promptGuidelines,
		parameters: EvalParameters,
		async execute(
			toolCallId,
			params: EvalParams,
			signal,
			_onUpdate,
			ctx: ExtensionContext,
		) {
			if (!params.code.trim()) throw new Error("eval code must not be empty");
			if (params.is_background) {
				if (evalVersion !== "v2" || !evalV2Languages.includes(params.language)) {
					throw new Error(`background eval is unavailable for language ${JSON.stringify(params.language)} in this Eval configuration`);
				}
				const kernel = new PersistentEvalKernel(params.language, "v2", invokeEvalHostCall);
				const task = await startEvalBackgroundTask({
					toolCallId,
					code: params.code,
					description: params.title,
					cwd: ctx.cwd,
					timeout: params.timeout ?? 30,
					ui: ctx.ui,
					kernel,
					tools: evalToolBridge?.catalog() ?? [],
					skills: evalSkills,
				});
				evalTasks.set(task.taskId, task);
				return {
					content: jsonContent({ task_id: task.taskId, status: "running", output_file: task.outputFile }),
					details: {
						taskId: task.taskId,
						background: true,
						command: task.code,
						cwd: task.cwd,
						outputFile: task.outputFile,
						description: task.description,
						kind: "eval",
					},
				};
			}
			const startedAt = Date.now();
			const kernel = evalKernels[params.language];
			if (!kernel) {
				throw new Error(`Eval ${evalVersion} does not support language ${JSON.stringify(params.language)}`);
			}
			let result;
			if (evalVersion === "v2") {
				if (params.reset) kernel.close();
				const task = await startEvalBackgroundTask({
					toolCallId,
					code: params.code,
					description: params.title,
					cwd: ctx.cwd,
					timeout: params.timeout ?? 30,
					ui: ctx.ui,
					kernel,
					tools: evalToolBridge?.catalog() ?? [],
					skills: evalSkills,
					backgrounded: false,
					ownsKernel: false,
				});
				const promoted = () => {
					if (task.completed || task.backgrounded) return;
					evalKernels = {
						...evalKernels,
						[params.language]: new PersistentEvalKernel(params.language, "v2", invokeEvalHostCall),
					};
					if (promoteEvalTask(task)) evalTasks.set(task.taskId, task);
				};
				task.promote = promoted;
				armEvalAutoBackground(task, maxWaitMs);
				const aborted = () => {
					if (!task.backgrounded && !task.completed) task.controller.abort(new Error("aborted"));
				};
				if (signal?.aborted) aborted();
				else signal?.addEventListener("abort", aborted, { once: true });
				await new Promise<void>((resolve) => {
					task.foregroundSettler = () => resolve();
					if (task.completed || task.backgrounded) resolve();
				});
				signal?.removeEventListener("abort", aborted);
				if (task.backgrounded) {
					return {
						content: jsonContent({ task_id: task.taskId, status: "running", output_file: task.outputFile }),
						details: {
							taskId: task.taskId,
							background: true,
							command: task.code,
							cwd: task.cwd,
							outputFile: task.outputFile,
							description: task.description,
							kind: "eval",
						},
					};
				}
				if (task.error) throw new Error(task.error);
				if (!task.result) throw new Error("Eval v2 foreground task settled without a result");
				result = task.result;
			} else {
				result = await kernel.execute(
					params.code,
					ctx.cwd,
					params.timeout ?? 30,
					signal,
					params.reset ?? false,
					evalToolBridge?.catalog() ?? [],
					evalSkills,
				);
			}
			const language = params.language === "py" ? "python" : "js";
			return {
				content: [
					{ type: "text" as const, text: result.output || "(no output)" },
				],
				details: {
					language,
					languages: [language],
					cells: [
						{
							index: 0,
							title: params.title?.trim() || undefined,
							code: params.code,
							language,
							output: result.output,
							status: "complete",
							durationMs: Date.now() - startedAt,
						},
					],
					truncated: result.truncated,
					bridgeVersion: evalVersion,
				},
			};
		},
	});
	if (evalToolBridge) await evalToolBridge.install("eval");

	const BashParameters = Type.Object({
		command: Type.String({
			description:
				"Exact bash to run. No trailing comments (# ...). Put the human-readable UI label in task_name instead.",
		}),
		timeout: Type.Optional(
			Type.Number({
				minimum: 0,
				maximum: MAX_TIMEOUT_SECONDS,
				description: "Timeout in seconds; omit or set 0 to disable the timeout",
			}),
		),
		is_background: Type.Optional(
			Type.Boolean({ description: "true = background task (returns task_id); false/omit = foreground" }),
		),
		// Required (like stock Grok `description`): Pager Execute cards prefer this over raw shell.
		task_name: Type.String({
			description:
				"Short human-readable UI title (3–8 words) for BOTH foreground and background. " +
				"Write it in the same language as the user's messages (not always English). " +
				"Especially required when the command is long, multi-pipeline, or hard to scan (>~40 chars). " +
				"Never put this label in command as a # comment.",
		}),
	});

	const bashPrompts = buildBashPrompts(
		evalVersion,
		nativeBash.description,
		nativeBash.promptGuidelines ?? [],
		evalV2Language,
	);
	if (bashEnabled) pi.registerTool({
		...nativeBash,
		parameters: BashParameters,
		description: bashPrompts.description,
		promptSnippet: bashPrompts.promptSnippet,
		promptGuidelines: bashPrompts.promptGuidelines,
		async execute(toolCallId, params: BashParams, signal, onUpdate, ctx: ExtensionContext) {
			if (signal?.aborted) throw new Error("aborted");
			const taskName = params.task_name?.trim() || undefined;
			if (params.is_background) {
				const task = await startTask(pi, {
					toolCallId,
					command: params.command,
					description: taskName,
					cwd: ctx.cwd,
					timeout: params.timeout,
					backgrounded: true,
					env: process.env,
					stateChanged: syncTaskState,
					ui: ctx.ui,
				});
				tasks.set(task.toolCallId, task);
				syncTaskState();
				return {
					content: jsonContent({ task_id: task.taskId, status: "running", output_file: task.outputFile }),
					details: {
						taskId: task.taskId,
						background: true,
						command: task.command,
						cwd: task.cwd,
						outputFile: task.outputFile,
						description: task.description,
					},
				};
			}

			let task: BackgroundTask | undefined;
			const managedBash = createBashToolDefinition(ctx.cwd, {
				operations: {
					exec: async (command, cwd, options) => {
						task = await startTask(pi, {
							toolCallId,
							command,
							cwd,
							timeout: options.timeout,
							autoBackgroundMs: maxWaitMs,
							backgrounded: false,
							description: taskName,
							env: options.env ?? process.env,
							onData: options.onData,
							stateChanged: syncTaskState,
							ui: ctx.ui,
						});
						tasks.set(toolCallId, task);
						const activeTask = task;
						return new Promise<{ exitCode: number | null }>((resolve, reject) => {
							const settle = (outcome: "completed" | "backgrounded") => {
								activeTask.foregroundSettler = undefined;
								activeTask.promote = undefined;
								options.signal?.removeEventListener("abort", aborted);
								if (outcome === "backgrounded") {
									resolve({ exitCode: 0 });
									return;
								}
								if (options.signal?.aborted) {
									reject(new Error("aborted"));
									return;
								}
								if (activeTask.timedOut) {
									reject(new Error(`timeout:${options.timeout}`));
									return;
								}
								resolve({ exitCode: activeTask.exitCode ?? null });
							};
							const aborted = () => killProcessTree(activeTask);
							activeTask.foregroundSettler = settle;
							activeTask.promote = () => {
								if (activeTask.completed || activeTask.backgrounded) return;
								if (activeTask.autoBackgroundHandle) clearTimeout(activeTask.autoBackgroundHandle);
								activeTask.autoBackgroundHandle = undefined;
								activeTask.backgrounded = true;
								syncTaskState();
								settle("backgrounded");
							};
							if (options.signal?.aborted) {
								aborted();
							} else {
								options.signal?.addEventListener("abort", aborted, { once: true });
							}
							syncTaskState();
							if (activeTask.completed) settle("completed");
						});
					},
				},
			});
			try {
				const result = await managedBash.execute(toolCallId, params, signal, onUpdate, ctx);
				if (!task?.backgrounded) return result;
				return {
					...result,
					details: {
						...result.details,
						taskId: task.taskId,
						background: true,
						command: task.command,
						cwd: task.cwd,
						outputFile: task.outputFile,
						description: task.description,
					},
				};
			} finally {
				if (task && !task.backgrounded) {
					tasks.delete(task.toolCallId);
					syncTaskState();
				}
			}
		},
	});

	type ManagedTask =
		| { kind: "bash"; task: BackgroundTask }
		| { kind: "eval"; task: EvalBackgroundTask };
	const findManagedTask = (taskId: string): ManagedTask | undefined => {
		const bashTask = [...tasks.values()].find((task) => task.taskId === taskId);
		if (bashTask) return { kind: "bash", task: bashTask };
		const evalTask = evalTasks.get(taskId);
		return evalTask ? { kind: "eval", task: evalTask } : undefined;
	};
	const managedTaskResult = (managed: ManagedTask) =>
		managed.kind === "bash" ? taskResult(managed.task) : evalTaskResult(managed.task);
	const waitManagedTask = (managed: ManagedTask, timeoutMs: number | undefined, signal: AbortSignal | undefined) =>
		managed.kind === "bash"
			? waitForCompletion(managed.task, timeoutMs, signal)
			: waitForEvalTask(managed.task, timeoutMs, signal);
	const capWaitMs = (timeoutMs: number | undefined) => {
		if (maxWaitMs === undefined) return timeoutMs;
		return timeoutMs && timeoutMs > 0 ? Math.min(timeoutMs, maxWaitMs) : maxWaitMs;
	};

	if (bashEnabled || evalVersion === "v2") pi.registerTool({
		name: "get_task_output",
		label: "get_task_output",
		description: "Get output for one or more background Bash or Eval v2 tasks. Set timeout_ms to wait for completion; omit it to poll. Configured max-wait still caps a blocking wait.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map(findManagedTask);
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !findManagedTask(id)) }) };
			const found = selected.filter((task): task is ManagedTask => task !== undefined);
			if (params.timeout_ms && params.timeout_ms > 0) {
				await Promise.all(found.map((task) => waitManagedTask(task, capWaitMs(params.timeout_ms), signal)));
			}
			const results = found.map(managedTaskResult);
			return { content: jsonContent(results.length === 1 ? results[0] : { mode: "wait_all", results }) };
		},
	});

	if (bashEnabled || evalVersion === "v2") pi.registerTool({
		name: "wait_tasks",
		label: "wait_tasks",
		description: "Wait for background Bash or Eval v2 tasks to finish. Configured max-wait returns current running state instead of blocking indefinitely.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			mode: StringEnum(["wait_any", "wait_all"] as const),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map(findManagedTask);
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !findManagedTask(id)) }) };
			const found = selected.filter((task): task is ManagedTask => task !== undefined);
			const waits = found.map((task) => waitManagedTask(task, capWaitMs(params.timeout_ms), signal));
			if (params.mode === "wait_any") await Promise.race(waits);
			else await Promise.all(waits);
			const results = found.map(managedTaskResult);
			return { content: jsonContent({ mode: params.mode, results }) };
		},
	});

	if (bashEnabled || evalVersion === "v2") pi.registerTool({
		name: "kill_task",
		label: "kill_task",
		description: "Terminate a running background Bash or Eval v2 task by task ID.",
		parameters: Type.Object({ task_id: Type.String({ minLength: 1 }) }),
		async execute(_toolCallId, params) {
			const managed = findManagedTask(params.task_id.trim());
			if (!managed) return { content: jsonContent({ task_not_found: params.task_id }) };
			if (managed.task.completed) return { content: jsonContent({ task_id: managed.task.taskId, outcome: "already_exited" }) };
			if (managed.kind === "bash") {
				managed.task.explicitlyKilled = true;
				managed.task.signal = "killed";
				killProcessTree(managed.task);
			} else {
				killEvalTask(managed.task);
			}
			return { content: jsonContent({ task_id: managed.task.taskId, outcome: "killed" }) };
		},
	});

	pi.on("session_shutdown", () => {
		control.close();
		evalToolBridge?.dispose();
		for (const kernel of Object.values(evalKernels)) kernel.close();
		for (const task of tasks.values()) {
			if (task.completed) continue;
			if (task.autoBackgroundHandle) clearTimeout(task.autoBackgroundHandle);
			task.explicitlyKilled = true;
			task.signal = ORPHANED_SIGNAL;
			killProcessTree(task);
		}
		for (const task of evalTasks.values()) {
			if (!task.completed) killEvalTask(task);
		}
	});
}
