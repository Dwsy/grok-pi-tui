import { existsSync, readFileSync, realpathSync } from "node:fs";
import { randomUUID } from "node:crypto";
import path from "node:path";
import { pathToFileURL } from "node:url";

import * as ImportedPi from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export type BridgeExecutionMode = "parallel" | "sequential";

export type EvalToolMetadata = {
	name: string;
	description?: string;
	schema?: unknown;
	guidelines?: string[];
	executionMode: BridgeExecutionMode;
	source?: unknown;
};

type ToolResult = {
	content: Array<{ type: string; text?: string; [key: string]: unknown }>;
	details?: unknown;
	isError?: boolean;
	terminate?: boolean;
	addedToolNames?: string[];
};

type RegisteredToolLike = {
	definition: {
		name: string;
		description?: string;
		parameters?: unknown;
		executionMode?: BridgeExecutionMode;
		prepareArguments?: (args: Record<string, unknown>) => Record<string, unknown>;
		execute: (...args: unknown[]) => Promise<ToolResult> | ToolResult;
	};
	sourceInfo?: unknown;
};

type WrappedToolLike = {
	name: string;
	executionMode?: BridgeExecutionMode;
	prepareArguments?: (args: Record<string, unknown>) => Record<string, unknown>;
	execute: (
		toolCallId: string,
		args: Record<string, unknown>,
		signal: AbortSignal,
		onUpdate: (partial: ToolResult) => void,
	) => Promise<ToolResult>;
};

type RunnerLike = {
	getActiveTools(): string[];
	createContext(): { cwd: string } & Record<string, unknown>;
	emit(event: Record<string, unknown>): Promise<unknown>;
	emitToolCall(event: Record<string, unknown>): Promise<{ block?: boolean; reason?: string } | undefined>;
	emitToolResult(event: Record<string, unknown>): Promise<{
		content?: ToolResult["content"];
		details?: unknown;
		isError?: boolean;
	} | undefined>;
};

type RunnerConstructor = {
	prototype: RunnerLike & {
		getAllRegisteredTools(): RegisteredToolLike[];
		[HUB_SYMBOL]?: CaptureHub;
	};
};

type PiRuntimeModule = typeof ImportedPi & {
	ExtensionRunner?: RunnerConstructor;
	wrapRegisteredTool?: (registeredTool: RegisteredToolLike, runner: RunnerLike) => WrappedToolLike;
};

type CaptureListener = (tools: RegisteredToolLike[], runner: RunnerLike) => void;
type CaptureHub = { listeners: Set<CaptureListener> };

const HUB_SYMBOL = Symbol.for("pi-grok.eval-tool-capture.v1");
const ANCHOR_SYMBOL = Symbol.for("pi-grok.eval-tool-anchor.v1");
const CORE_TOOL_NAMES = new Set(["read", "bash", "edit", "write", "grep", "find", "ls"]);

function captureHub(Runner: RunnerConstructor): CaptureHub {
	const prototype = Runner.prototype;
	const existing = prototype[HUB_SYMBOL];
	if (existing) return existing;
	const original = prototype.getAllRegisteredTools;
	if (typeof original !== "function") {
		throw new Error("Eval v2 could not observe ExtensionRunner.getAllRegisteredTools");
	}
	const hub: CaptureHub = { listeners: new Set() };
	Object.defineProperty(prototype, HUB_SYMBOL, {
		value: hub,
		configurable: false,
		enumerable: false,
		writable: false,
	});
	prototype.getAllRegisteredTools = function getPiGrokVisibleTools() {
		const tools = original.call(this);
		for (const listener of [...hub.listeners]) listener(tools, this);
		return tools;
	};
	return hub;
}

function hostPackageRoot(): string | undefined {
	const cliPath = process.argv[1];
	if (!cliPath) return undefined;
	let directory: string;
	try {
		directory = path.dirname(realpathSync(cliPath));
	} catch {
		return undefined;
	}
	while (directory !== path.dirname(directory)) {
		const manifestPath = path.join(directory, "package.json");
		if (existsSync(manifestPath)) {
			try {
				const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as { name?: string };
				if (manifest.name === "@earendil-works/pi-coding-agent") return directory;
			} catch {
				// Keep walking; a malformed unrelated manifest is not our host package.
			}
		}
		directory = path.dirname(directory);
	}
	return undefined;
}

async function runtimeModules(): Promise<PiRuntimeModule[]> {
	const modules: PiRuntimeModule[] = [ImportedPi as PiRuntimeModule];
	const roots = new Set(
		[process.env.PI_PACKAGE_DIR, hostPackageRoot()].filter(
			(root): root is string => typeof root === "string" && root.length > 0,
		),
	);
	for (const root of roots) {
		try {
			const loaded = (await import(pathToFileURL(path.join(root, "dist", "index.js")).href)) as PiRuntimeModule;
			if (!modules.some((candidate) => candidate.ExtensionRunner === loaded.ExtensionRunner)) modules.push(loaded);
		} catch {
			// The statically imported module remains a valid candidate.
		}
	}
	return modules;
}

function textFromContent(content: ToolResult["content"]): string {
	return content
		.filter((item) => item.type === "text" && typeof item.text === "string")
		.map((item) => item.text as string)
		.join("\n");
}

function abortError(signal: AbortSignal): Error {
	return signal.reason instanceof Error ? signal.reason : new Error(signal.reason === undefined ? "aborted" : String(signal.reason));
}

function throwIfAborted(signal: AbortSignal) {
	if (signal.aborted) throw abortError(signal);
}

function evalV2OnlyHostToolsEnabled(): boolean {
	return process.env.PI_GROK_EVAL_V2_ONLY === "1";
}

function jsonSafeValue(value: unknown): unknown {
	if (value === undefined) return undefined;
	try {
		return JSON.parse(JSON.stringify(value));
	} catch {
		return undefined;
	}
}

function definitionDelegatesTo(definition: object, target: object): boolean {
	let current: object | null = definition;
	while (current) {
		if (current === target) return true;
		current = Object.getPrototypeOf(current) as object | null;
	}
	return false;
}

export class EvalSessionToolBridge {
	private modules: PiRuntimeModule[] = [];
	private runner?: RunnerLike;
	private runtimeModule?: PiRuntimeModule;
	private registered = new Map<string, { registeredTool: RegisteredToolLike; wrappedTool: WrappedToolLike }>();
	private disposers: Array<() => void> = [];
	private allowedTools?: Set<string>;

	constructor(private readonly pi: ExtensionAPI) {}

	/**
	 * Restrict the host tools injected into eval cells (catalog + invoke).
	 * undefined/empty restores the default: every registered tool in eval v2-only mode.
	 */
	setAllowedTools(names: string[] | undefined) {
		this.allowedTools = names && names.length > 0 ? new Set(names) : undefined;
	}

	getAllowedTools(): string[] | undefined {
		return this.allowedTools ? [...this.allowedTools] : undefined;
	}

	private isAllowed(toolName: string): boolean {
		return !this.allowedTools || this.allowedTools.has(toolName);
	}

	async install(anchor: object | string) {
		this.modules = await runtimeModules();
		const anchorToken = typeof anchor === "string" ? undefined : {};
		if (anchorToken) {
			Object.defineProperty(anchor, ANCHOR_SYMBOL, {
				value: anchorToken,
				configurable: false,
				enumerable: false,
				writable: false,
			});
		}
		for (const module of this.modules) {
			const Runner = module.ExtensionRunner;
			if (!Runner) continue;
			const hub = captureHub(Runner);
			const listener: CaptureListener = (tools, runner) => {
				const anchored = tools.some((tool) => {
					if (typeof anchor === "string") return tool.definition.name === anchor;
					const definition = tool.definition as object & { [ANCHOR_SYMBOL]?: unknown };
					return definition[ANCHOR_SYMBOL] === anchorToken || definitionDelegatesTo(definition, anchor);
				});
				if (!anchored) return;
				this.observeRegisteredTools(tools, runner);
			};
			hub.listeners.add(listener);
			this.disposers.push(() => hub.listeners.delete(listener));
		}
	}

	dispose() {
		for (const dispose of this.disposers.splice(0)) dispose();
		this.registered.clear();
		this.runner = undefined;
		this.runtimeModule = undefined;
	}

	/** Structural hook used by the production runner observer and focused tests. */
	observeRegisteredTools(tools: RegisteredToolLike[], runner: RunnerLike) {
		this.runner = runner;
		this.runtimeModule =
			this.modules.find((module) => module.ExtensionRunner && runner instanceof module.ExtensionRunner) ??
			(this.modules[0] ?? (ImportedPi as PiRuntimeModule));
		const wrap = this.runtimeModule.wrapRegisteredTool ?? (ImportedPi as PiRuntimeModule).wrapRegisteredTool;
		if (typeof wrap !== "function") throw new Error("Eval v2 host cannot wrap Pi extension tools");
		this.registered.clear();
		for (const registeredTool of tools) {
			this.registered.set(registeredTool.definition.name, {
				registeredTool,
				wrappedTool: wrap(registeredTool, runner),
			});
		}
	}

	catalog(): EvalToolMetadata[] {
		const active = new Set(this.pi.getActiveTools());
		const includeRegistered = evalV2OnlyHostToolsEnabled();
		return this.pi
			.getAllTools()
			.filter((tool) => (includeRegistered || active.has(tool.name)) && tool.name !== "eval" && this.isAllowed(tool.name))
			.map((tool) => {
				const captured = this.registered.get(tool.name)?.registeredTool;
				const runtimeInfo = tool as typeof tool & { executionMode?: BridgeExecutionMode };
				const executionMode =
					captured?.definition.executionMode === "parallel" || runtimeInfo.executionMode === "parallel"
						? "parallel"
						: "sequential";
				const metadata: EvalToolMetadata = {
					name: tool.name,
					executionMode,
				};
				if (tool.description) metadata.description = tool.description;
				const schema = jsonSafeValue(tool.parameters ?? captured?.definition.parameters);
				if (schema !== undefined) metadata.schema = schema;
				const guidelines = jsonSafeValue(tool.promptGuidelines);
				if (Array.isArray(guidelines)) metadata.guidelines = guidelines.filter((item): item is string => typeof item === "string");
				const source = jsonSafeValue(tool.sourceInfo ?? captured?.sourceInfo);
				if (source !== undefined) metadata.source = source;
				return metadata;
			});
	}

	executionMode(toolName: string): BridgeExecutionMode {
		if (this.registered.get(toolName)?.registeredTool.definition.executionMode === "parallel") {
			return "parallel";
		}
		const info = this.pi.getAllTools().find((tool) => tool.name === toolName) as
			| { executionMode?: BridgeExecutionMode }
			| undefined;
		return info?.executionMode === "parallel" ? "parallel" : "sequential";
	}

	async invoke(toolName: string, args: Record<string, unknown>, signal: AbortSignal): Promise<ToolResult> {
		throwIfAborted(signal);
		const active = this.pi.getActiveTools().includes(toolName);
		const registered = this.pi.getAllTools().some((tool) => tool.name === toolName);
		if (!active && !(evalV2OnlyHostToolsEnabled() && registered && this.isAllowed(toolName))) {
			throw new Error(`Eval v2 cannot invoke inactive tool ${JSON.stringify(toolName)}`);
		}

		const nativeInvoke = (this.pi as ExtensionAPI & {
			invokeTool?: (name: string, args: Record<string, unknown>, signal: AbortSignal) => Promise<ToolResult>;
		}).invokeTool;
		if (active && typeof nativeInvoke === "function") return nativeInvoke.call(this.pi, toolName, args, signal);

		const captured = this.registered.get(toolName);
		if (captured) return this.invokeWrapped(toolName, captured.wrappedTool, args, signal);

		if (CORE_TOOL_NAMES.has(toolName)) {
			const wrapped = this.createCoreTool(toolName);
			if (wrapped) return this.invokeWrapped(toolName, wrapped, args, signal);
		}

		throw new Error(
			`Eval v2 cannot invoke ${JSON.stringify(toolName)} on this Pi host: the active tool is not an extension/core tool and ExtensionAPI.invokeTool is unavailable`,
		);
	}

	private createCoreTool(toolName: string): WrappedToolLike | undefined {
		const runner = this.runner;
		const module = this.runtimeModule ?? (ImportedPi as PiRuntimeModule);
		if (!runner) return undefined;
		const context = runner.createContext();
		const factoryName = {
			read: "createReadToolDefinition",
			bash: "createBashToolDefinition",
			edit: "createEditToolDefinition",
			write: "createWriteToolDefinition",
			grep: "createGrepToolDefinition",
			find: "createFindToolDefinition",
			ls: "createLsToolDefinition",
		}[toolName] as keyof PiRuntimeModule | undefined;
		if (!factoryName) return undefined;
		const factory = module[factoryName] as unknown;
		const wrap = module.wrapRegisteredTool ?? (ImportedPi as PiRuntimeModule).wrapRegisteredTool;
		if (typeof factory !== "function" || typeof wrap !== "function") return undefined;
		const definition = (factory as (cwd: string) => RegisteredToolLike["definition"])(context.cwd);
		return wrap({ definition }, runner);
	}

	private async invokeWrapped(
		toolName: string,
		wrappedTool: WrappedToolLike,
		inputArgs: Record<string, unknown>,
		signal: AbortSignal,
	): Promise<ToolResult> {
		const runner = this.runner;
		if (!runner) throw new Error("Eval v2 host runner is not ready");
		const prepared = wrappedTool.prepareArguments ? wrappedTool.prepareArguments(inputArgs) : inputArgs;
		const args = { ...prepared };
		const toolCallId = `eval-host-${randomUUID()}`;
		await runner.emit({ type: "tool_execution_start", toolCallId, toolName, args });
		let result: ToolResult;
		let isError = false;
		let thrown: unknown;
		let updateTail = Promise.resolve();
		try {
			const preflight = await runner.emitToolCall({
				type: "tool_call",
				toolName,
				toolCallId,
				input: args,
			});
			if (preflight?.block) throw new Error(preflight.reason || `Pi tool ${toolName} was blocked`);
			result = await wrappedTool.execute(toolCallId, args, signal, (partialResult) => {
				updateTail = updateTail
					.then(() =>
						runner.emit({
							type: "tool_execution_update",
							toolCallId,
							toolName,
							args,
							partialResult,
						}),
					)
					.then(() => undefined, () => undefined);
			});
		} catch (error) {
			thrown = error;
			isError = true;
			result = {
				content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }],
			};
		}
		await updateTail;
		throwIfAborted(signal);
		const patch = await runner.emitToolResult({
			type: "tool_result",
			toolName,
			toolCallId,
			input: args,
			content: result.content,
			details: result.details,
			isError,
		});
		if (patch) {
			result = {
				...result,
				content: patch.content ?? result.content,
				...(patch.details !== undefined ? { details: patch.details } : {}),
			};
			isError = patch.isError ?? isError;
		}
		await runner.emit({ type: "tool_execution_end", toolCallId, toolName, result, isError });
		if (isError) {
			throw new Error(textFromContent(result.content).trim() || (thrown instanceof Error ? thrown.message : `Pi tool ${toolName} failed`));
		}
		return result;
	}
}
