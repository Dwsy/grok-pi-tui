/**
 * Pi Bash enhancement used only by grok-pi.
 *
 * The extension owns every Bash child process. This lets the native Pager
 * promote an active foreground tool call into its existing background-task UI
 * without rerunning the command. Pager still owns all visible task surfaces.
 */
import { type ChildProcess, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
	closeSync,
	createWriteStream,
	existsSync,
	type FSWatcher,
	openSync,
	readFileSync,
	unlinkSync,
	type WriteStream,
	watch,
	writeFileSync,
} from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { StringEnum } from "@earendil-works/pi-ai";
import {
	createBashToolDefinition,
	type ExtensionAPI,
	type ExtensionContext,
	type ExtensionUIContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";

const BRIDGE_TYPE = "pi-grok-background-bash/v1";
/**
 * Out-of-band terminal-state channel. `ui.setStatus` is a synchronous
 * fire-and-forget `extension_ui_request` in RPC mode, so it reaches the adapter
 * regardless of streaming state, aborts, or a cleared follow-up queue —
 * unlike `pi.sendMessage`, which the agent may queue or drop entirely.
 */
const TASK_STATUS_KEY = "__pi_grok_bash_task__";
/**
 * Pager's marker for a task that died with a previous session lifetime. It
 * settles the row quietly instead of pushing a red "Task failed" block for a
 * teardown the user already knows about.
 */
const ORPHANED_SIGNAL = "session_restart";
const MAX_OUTPUT_BYTES = 50 * 1024;
const MAX_TASK_IDS = 20;
const MAX_TIMEOUT_SECONDS = 2_147_483.647;

type BashParams = {
	command: string;
	timeout?: number;
	is_background?: boolean;
	/** Short UI label (Pager reads this via adapter → description). */
	task_name: string;
};

type BackgroundTask = {
	taskId: string;
	toolCallId: string;
	command: string;
	description?: string;
	cwd: string;
	outputFile: string;
	startedAt: number;
	endedAt?: number;
	child: ChildProcess;
	log: WriteStream;
	output: Buffer;
	outputBytes: number;
	truncated: boolean;
	exitCode?: number;
	signal?: string;
	completed: boolean;
	backgrounded: boolean;
	explicitlyKilled: boolean;
	timedOut: boolean;
	timeoutHandle?: ReturnType<typeof setTimeout>;
	waiters: Set<() => void>;
	foregroundSettler?: (outcome: "completed" | "backgrounded") => void;
	promote?: () => void;
	stateChanged?: () => void;
	/**
	 * UI context captured at launch. The context object itself is held (not the
	 * surrounding `ctx`) so publishing after a session replacement cannot hit
	 * the stale-instance `assertActive()` guard.
	 */
	ui?: TaskStatusChannel;
};

type TaskStatusChannel = Pick<ExtensionUIContext, "setStatus">;

type BashControl = {
	sync: () => void;
	close: () => void;
};

type EvalLanguage = "py" | "js";

type EvalParams = {
	language: EvalLanguage;
	code: string;
	title?: string;
	timeout?: number;
	reset?: boolean;
};

type EvalExecution = {
	output: string;
	truncated: boolean;
};

type EvalWorkerReply = {
	id: string;
	ok: boolean;
	value?: string;
	error?: string;
};

type PendingEval = {
	id: string;
	resolve: (result: EvalExecution) => void;
	reject: (error: Error) => void;
	output: Buffer;
	truncated: boolean;
	timer?: ReturnType<typeof setTimeout>;
	signal?: AbortSignal;
	abortHandler?: () => void;
};

const JS_EVAL_WORKER = String.raw`
const fs = require("node:fs");
const repl = require("node:repl");
const readline = require("node:readline");
const util = require("node:util");
const { PassThrough } = require("node:stream");

const protocol = fs.createWriteStream(null, { fd: 3, autoClose: false });
const input = new PassThrough();
const server = repl.start({
  prompt: "",
  input,
  output: process.stdout,
  terminal: false,
  useGlobal: false,
  ignoreUndefined: true,
});
server.context.display = value => {
  console.log(util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }));
};

function reply(message) {
  protocol.write(JSON.stringify(message) + "\n");
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", line => {
  let request;
  try {
    request = JSON.parse(line);
  } catch (error) {
    reply({ id: "", ok: false, error: String(error) });
    return;
  }
  server.eval(request.code, server.context, "eval", (error, value) => {
    if (error) {
      reply({ id: request.id, ok: false, error: error.stack || error.message || String(error) });
      return;
    }
    reply({
      id: request.id,
      ok: true,
      value: value === undefined
        ? ""
        : util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }),
    });
  });
});
`;

const PYTHON_EVAL_WORKER = String.raw`
import ast
import asyncio
import inspect
import json
import os
import sys
import traceback

protocol = os.fdopen(3, "w", buffering=1)
namespace = {"__name__": "__eval__"}

def display(value):
    print(repr(value))

namespace["display"] = display

async def evaluate(code):
    tree = ast.parse(code, filename="<eval>", mode="exec")
    last_expression = None
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last_expression = tree.body.pop()
    if tree.body:
        ast.fix_missing_locations(tree)
        compiled = compile(tree, "<eval>", "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
        result = eval(compiled, namespace)
        if inspect.isawaitable(result):
            await result
    if last_expression is None:
        return None
    expression = ast.Expression(last_expression.value)
    ast.fix_missing_locations(expression)
    compiled = compile(expression, "<eval>", "eval", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    result = eval(compiled, namespace)
    if inspect.isawaitable(result):
        result = await result
    return result

async def main():
    loop = asyncio.get_running_loop()
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line:
            return
        try:
            request = json.loads(line)
            value = await evaluate(request["code"])
            protocol.write(json.dumps({
                "id": request["id"],
                "ok": True,
                "value": "" if value is None else repr(value),
            }, ensure_ascii=False) + "\n")
        except BaseException:
            request_id = request.get("id", "") if isinstance(request, dict) else ""
            protocol.write(json.dumps({
                "id": request_id,
                "ok": False,
                "error": traceback.format_exc(),
            }, ensure_ascii=False) + "\n")

asyncio.run(main())
`;

function validateEvalTimeout(timeout: number) {
	if (
		!Number.isFinite(timeout) ||
		timeout < 0 ||
		timeout > MAX_TIMEOUT_SECONDS
	) {
		throw new Error(
			`Invalid eval timeout: must be between 0 and ${MAX_TIMEOUT_SECONDS} seconds`,
		);
	}
}

class PersistentEvalKernel {
	private child?: ChildProcess;
	private input?: NonNullable<ChildProcess["stdin"]>;
	private cwd?: string;
	private protocolBuffer = "";
	private pending?: PendingEval;

	constructor(private readonly language: EvalLanguage) {}

	async execute(
		code: string,
		cwd: string,
		timeout: number,
		signal: AbortSignal | undefined,
		reset: boolean,
	): Promise<EvalExecution> {
		validateEvalTimeout(timeout);
		if (this.pending)
			throw new Error(
				`${this.language} eval kernel is already executing a cell`,
			);
		if (reset || (this.cwd !== undefined && this.cwd !== cwd))
			this.resetKernel();
		this.ensureStarted(cwd);
		if (signal?.aborted) {
			this.resetKernel();
			throw new Error("aborted");
		}

		const id = randomUUID();
		return new Promise<EvalExecution>((resolve, reject) => {
			const pending: PendingEval = {
				id,
				resolve,
				reject,
				output: Buffer.alloc(0),
				truncated: false,
				signal,
			};
			if (timeout > 0) {
				pending.timer = setTimeout(() => {
					this.resetKernel(
						new Error(
							`Eval timed out after ${timeout} seconds; ${this.language} kernel reset`,
						),
					);
				}, timeout * 1000);
			}
			if (signal) {
				pending.abortHandler = () =>
					this.resetKernel(new Error("aborted; eval kernel reset"));
				signal.addEventListener("abort", pending.abortHandler, { once: true });
			}
			this.pending = pending;
			this.input?.write(`${JSON.stringify({ id, code })}\n`, (error) => {
				if (error) this.resetKernel(error);
			});
		});
	}

	close() {
		this.resetKernel(new Error("session_shutdown"));
	}

	private ensureStarted(cwd: string) {
		if (this.child) return;
		const command =
			this.language === "js"
				? process.execPath
				: process.env.PI_GROK_PYTHON ||
					(process.platform === "win32" ? "python" : "python3");
		const args =
			this.language === "js"
				? ["-e", JS_EVAL_WORKER]
				: ["-u", "-c", PYTHON_EVAL_WORKER];
		const child = spawn(command, args, {
			cwd,
			env: process.env,
			detached: process.platform !== "win32",
			stdio: ["pipe", "pipe", "pipe", "pipe"],
			windowsHide: true,
		});
		const input = child.stdin;
		const stdout = child.stdout;
		const stderr = child.stderr;
		const protocol = child.stdio[3] as NodeJS.ReadableStream | null;
		if (!input || !stdout || !stderr || !protocol) {
			killChildProcess(child);
			throw new Error(`Failed to start ${this.language} eval kernel pipes`);
		}
		this.child = child;
		this.input = input;
		this.cwd = cwd;
		this.protocolBuffer = "";
		stdout.on("data", (chunk) => this.appendOutput(chunk));
		stderr.on("data", (chunk) => this.appendOutput(chunk));
		protocol.on("data", (chunk) => this.consumeProtocol(chunk));
		child.once("error", (error) => {
			if (this.child === child) this.resetKernel(error);
		});
		child.once("close", (code, childSignal) => {
			if (this.child !== child) return;
			const reason = `Eval kernel exited (code=${code ?? "none"}, signal=${childSignal ?? "none"})`;
			this.child = undefined;
			this.input = undefined;
			this.cwd = undefined;
			this.protocolBuffer = "";
			this.takePending()?.reject(new Error(reason));
		});
	}

	private appendOutput(chunk: Buffer | string) {
		const pending = this.pending;
		if (!pending) return;
		const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
		const joined = Buffer.concat([pending.output, bytes]);
		if (joined.length > MAX_OUTPUT_BYTES) {
			pending.output = joined.subarray(joined.length - MAX_OUTPUT_BYTES);
			pending.truncated = true;
			return;
		}
		pending.output = joined;
	}

	private consumeProtocol(chunk: Buffer | string) {
		this.protocolBuffer += Buffer.isBuffer(chunk)
			? chunk.toString("utf8")
			: chunk;
		let newline = this.protocolBuffer.indexOf("\n");
		while (newline >= 0) {
			const line = this.protocolBuffer.slice(0, newline);
			this.protocolBuffer = this.protocolBuffer.slice(newline + 1);
			if (line.trim()) this.handleReply(line);
			newline = this.protocolBuffer.indexOf("\n");
		}
	}

	private handleReply(line: string) {
		let reply: EvalWorkerReply;
		try {
			reply = JSON.parse(line) as EvalWorkerReply;
		} catch (error) {
			this.resetKernel(
				new Error(
					`Invalid ${this.language} eval protocol reply: ${String(error)}`,
				),
			);
			return;
		}
		if (!this.pending || reply.id !== this.pending.id) return;
		const pending = this.takePending();
		if (!pending) return;
		const streamed = pending.output.toString("utf8");
		const output = [streamed, reply.value ?? ""]
			.filter((part) => part.length > 0)
			.join(
				streamed && reply.value ? (streamed.endsWith("\n") ? "" : "\n") : "",
			);
		const rendered = pending.truncated
			? `[output truncated to ${MAX_OUTPUT_BYTES} bytes]\n${output}`
			: output;
		if (reply.ok) {
			pending.resolve({ output: rendered, truncated: pending.truncated });
			return;
		}
		pending.reject(
			new Error(
				[reply.error || "Eval failed", rendered].filter(Boolean).join("\n"),
			),
		);
	}

	private takePending(): PendingEval | undefined {
		const pending = this.pending;
		this.pending = undefined;
		if (!pending) return undefined;
		if (pending.timer) clearTimeout(pending.timer);
		if (pending.signal && pending.abortHandler) {
			pending.signal.removeEventListener("abort", pending.abortHandler);
		}
		return pending;
	}

	private resetKernel(error?: Error) {
		const child = this.child;
		this.child = undefined;
		this.input = undefined;
		this.cwd = undefined;
		this.protocolBuffer = "";
		const pending = this.takePending();
		if (child) killChildProcess(child);
		if (pending && error) pending.reject(error);
	}
}

function taskState(task: BackgroundTask): string {
	if (!task.completed) return "running";
	if (task.explicitlyKilled) return "cancelled";
	return task.exitCode === 0 && !task.signal ? "completed" : "failed";
}

function taskSnapshot(task: BackgroundTask) {
	return {
		task_id: task.taskId,
		command: task.command,
		display_command: task.command,
		cwd: task.cwd,
		start_time: systemTime(task.startedAt),
		end_time: task.endedAt === undefined ? undefined : systemTime(task.endedAt),
		output: task.output.toString("utf8"),
		output_file: task.outputFile,
		truncated: task.truncated,
		exit_code: task.exitCode,
		signal: task.signal,
		completed: task.completed,
		kind: "bash",
		block_waited: false,
		explicitly_killed: task.explicitlyKilled,
		owner_session_id: undefined,
	};
}

function systemTime(milliseconds: number) {
	return {
		secs_since_epoch: Math.floor(milliseconds / 1000),
		nanos_since_epoch: Math.floor(milliseconds % 1000) * 1_000_000,
	};
}

function taskResult(task: BackgroundTask) {
	const ended = task.endedAt === undefined ? undefined : new Date(task.endedAt).toISOString();
	return {
		task_id: task.taskId,
		command: task.command,
		status: taskState(task),
		exit_code: task.exitCode,
		started: new Date(task.startedAt).toISOString(),
		ended,
		duration_secs: ((task.endedAt ?? Date.now()) - task.startedAt) / 1000,
		output: task.output.toString("utf8"),
		output_file: task.outputFile,
		truncated: task.truncated,
		raw_output_bytes: task.outputBytes,
	};
}

function appendOutput(task: BackgroundTask, chunk: Buffer) {
	task.outputBytes += chunk.length;
	const joined = Buffer.concat([task.output, chunk]);
	if (joined.length > MAX_OUTPUT_BYTES) {
		task.output = joined.subarray(joined.length - MAX_OUTPUT_BYTES);
		task.truncated = true;
		return;
	}
	task.output = joined;
}

function killChildProcess(child: ChildProcess) {
	const pid = child.pid;
	if (!pid) return;
	if (process.platform !== "win32") {
		try {
			process.kill(-pid, "SIGKILL");
			return;
		} catch {
			// The process may not own a group. Fall back to its direct PID.
		}
	}
	try {
		child.kill("SIGKILL");
	} catch {
		// The close handler will establish final state when it is still alive.
	}
}

function killProcessTree(task: BackgroundTask) {
	killChildProcess(task.child);
}

function waitForCompletion(task: BackgroundTask, timeoutMs: number | undefined, signal: AbortSignal | undefined) {
	if (task.completed) return Promise.resolve();
	return new Promise<void>((resolve, reject) => {
		let timer: ReturnType<typeof setTimeout> | undefined;
		const done = () => {
			if (timer) clearTimeout(timer);
			signal?.removeEventListener("abort", aborted);
			task.waiters.delete(done);
			resolve();
		};
		const aborted = () => {
			if (timer) clearTimeout(timer);
			task.waiters.delete(done);
			reject(new Error("aborted"));
		};
		if (signal?.aborted) {
			aborted();
			return;
		}
		task.waiters.add(done);
		signal?.addEventListener("abort", aborted, { once: true });
		if (timeoutMs && timeoutMs > 0) timer = setTimeout(done, timeoutMs);
	});
}

function emitCompleted(pi: ExtensionAPI, task: BackgroundTask) {
	const snapshot = taskSnapshot(task);
	const failed = !snapshot.explicitly_killed && (snapshot.exit_code !== 0 || Boolean(snapshot.signal));
	const shouldWake = !snapshot.explicitly_killed;
	const content = snapshot.explicitly_killed
		? `Background Bash task cancelled: ${task.command}`
		: failed
			? `Background Bash task failed: ${task.command}\n\n${snapshot.output || "(no output)"}\n\nExit code: ${snapshot.exit_code ?? "none"}${snapshot.signal ? `; signal: ${snapshot.signal}` : ""}`
			: `Background Bash task completed: ${task.command}\n\n${snapshot.output || "(no output)"}\n\nExit code: ${snapshot.exit_code ?? "none"}`;
	pi.sendMessage(
		{
			customType: BRIDGE_TYPE,
			content,
			display: false,
			details: {
				version: 1,
				event: "completed",
				taskId: task.taskId,
				toolCallId: task.toolCallId,
				taskSnapshot: snapshot,
			},
		},
		shouldWake ? { triggerTurn: true, deliverAs: "followUp" } : { triggerTurn: false },
	);
}

/**
 * Publish the task's terminal state on the private status channel.
 *
 * This is what the native task UI converges on. It is deliberately independent
 * of `emitCompleted`: the bridge message is a conversation message and shares
 * the agent's queue lifetime, so it can be delayed for a whole turn or dropped
 * outright when the user aborts.
 */
function publishTerminalState(task: BackgroundTask) {
	try {
		task.ui?.setStatus(
			TASK_STATUS_KEY,
			JSON.stringify({
				version: 1,
				event: "completed",
				taskId: task.taskId,
				toolCallId: task.toolCallId,
				taskSnapshot: taskSnapshot(task),
			}),
		);
	} catch {
		// A detached UI channel only costs this one projection; the caller
		// still delivers the result to the model.
	}
}

function finishTask(pi: ExtensionAPI, task: BackgroundTask, code: number | null, signal: NodeJS.Signals | null) {
	if (task.completed) return;
	task.completed = true;
	task.endedAt = Date.now();
	task.exitCode = code ?? undefined;
	task.signal ??= signal ?? undefined;
	if (task.timeoutHandle) clearTimeout(task.timeoutHandle);
	task.log.end(() => {
		if (task.backgrounded) {
			publishTerminalState(task);
			try {
				emitCompleted(pi, task);
			} catch {
				// `pi.sendMessage` throws on a stale extension instance (session
				// replacement / reload). Waking the model is best effort; the
				// bookkeeping below must still run or the task never settles.
			}
		}
		const settleForeground = task.foregroundSettler;
		task.foregroundSettler = undefined;
		settleForeground?.("completed");
		for (const waiter of task.waiters) waiter();
		task.waiters.clear();
		task.stateChanged?.();
	});
}

function launchShell(command: string, cwd: string, env: NodeJS.ProcessEnv) {
	const shell = process.platform === "win32" ? "bash" : "/bin/bash";
	return spawn(shell, ["-c", command], {
		cwd,
		env,
		detached: process.platform !== "win32",
		stdio: ["ignore", "pipe", "pipe"],
		windowsHide: true,
	});
}

function validateTimeout(timeout: number | undefined) {
	if (timeout === undefined) return;
	if (!Number.isFinite(timeout) || timeout <= 0 || timeout > MAX_TIMEOUT_SECONDS) {
		throw new Error(`Invalid timeout: must be a finite number of seconds up to ${MAX_TIMEOUT_SECONDS}`);
	}
}

async function startTask(
	pi: ExtensionAPI,
	params: {
		toolCallId: string;
		command: string;
		description?: string;
		cwd: string;
		timeout?: number;
		backgrounded: boolean;
		env: NodeJS.ProcessEnv;
		onData?: (chunk: Buffer) => void;
		stateChanged?: () => void;
		ui?: TaskStatusChannel;
	},
): Promise<BackgroundTask> {
	validateTimeout(params.timeout);
	const directory = await mkdtemp(join(tmpdir(), "pi-grok-bash-"));
	const task: BackgroundTask = {
		taskId: `bash-${randomUUID()}`,
		toolCallId: params.toolCallId,
		command: params.command,
		description: params.description?.trim() || undefined,
		cwd: params.cwd,
		outputFile: join(directory, "output.log"),
		startedAt: Date.now(),
		child: launchShell(params.command, params.cwd, params.env),
		log: createWriteStream(join(directory, "output.log"), { flags: "a" }),
		output: Buffer.alloc(0),
		outputBytes: 0,
		truncated: false,
		completed: false,
		backgrounded: params.backgrounded,
		explicitlyKilled: false,
		timedOut: false,
		waiters: new Set(),
		stateChanged: params.stateChanged,
		ui: params.ui,
	};
	const recordOutput = (chunk: Buffer) => {
		appendOutput(task, chunk);
		task.log.write(chunk);
		params.onData?.(chunk);
	};
	task.child.stdout?.on("data", recordOutput);
	task.child.stderr?.on("data", recordOutput);
	task.log.on("error", (error) => {
		task.signal ??= `output_log_error:${error.message}`;
	});
	task.child.once("error", (error) => {
		task.signal = error.message;
		finishTask(pi, task, null, null);
	});
	task.child.once("close", (code, childSignal) => finishTask(pi, task, code, childSignal));
	if (params.timeout) {
		task.timeoutHandle = setTimeout(() => {
			task.timedOut = true;
			task.signal = "timeout";
			killProcessTree(task);
		}, params.timeout * 1000);
	}
	return task;
}

function createBashControl(tasks: Map<string, BackgroundTask>): BashControl {
	const metaPath = process.env.PI_GROK_BASH_CONTROL_META;
	if (!metaPath) return { sync: () => {}, close: () => {} };

	const controlPath = join(tmpdir(), `pi-grok-bash-control-${randomUUID()}.jsonl`);
	closeSync(openSync(controlPath, "a"));
	let offset = 0;
	const sync = () => {
		const activeToolCallIds = [...tasks.values()]
			.filter((task) => !task.completed && !task.backgrounded && task.promote)
			.map((task) => task.toolCallId);
		const runningTaskIds = [...tasks.values()]
			.filter((task) => !task.completed)
			.map((task) => task.taskId);
		try {
			writeFileSync(
				metaPath,
				JSON.stringify({ controlPath, activeToolCallIds, runningTaskIds }),
				"utf8",
			);
		} catch {
			// A failed control publication only disables Pager promotion/kill; Bash itself remains valid.
		}
	};
	const drain = () => {
		try {
			if (!existsSync(controlPath)) return;
			const content = readFileSync(controlPath, "utf8");
			if (content.length <= offset) return;
			const chunk = content.slice(offset);
			offset = content.length;
			for (const line of chunk.split("\n")) {
				if (!line.trim()) continue;
				try {
					const event = JSON.parse(line) as {
						op?: string;
						toolCallId?: string;
						taskId?: string;
					};
					if (event.op === "background" && typeof event.toolCallId === "string") {
						tasks.get(event.toolCallId)?.promote?.();
						continue;
					}
					if (event.op === "kill" && typeof event.taskId === "string") {
						const task = [...tasks.values()].find((candidate) => candidate.taskId === event.taskId);
						if (!task || task.completed) continue;
						task.explicitlyKilled = true;
						task.signal = "killed";
						killProcessTree(task);
					}
				} catch {
					// Ignore malformed events rather than affecting an active Bash process.
				}
			}
		} catch {
			// The adapter may race session shutdown; subsequent writes can retry.
		}
	};
	let watcher: FSWatcher | undefined;
	let poller: ReturnType<typeof setInterval> | undefined;
	try {
		watcher = watch(controlPath, drain);
	} catch {
		poller = setInterval(drain, 50);
	}
	sync();
	return {
		sync,
		close: () => {
			try {
				watcher?.close();
			} catch {
				// Ignore a watcher that already closed during shutdown.
			}
			if (poller) clearInterval(poller);
			try {
				if (existsSync(controlPath)) unlinkSync(controlPath);
			} catch {
				// The OS will clean the process temp directory on exit if needed.
			}
		},
	};
}

function ensureTaskIds(taskIds: string[]) {
	const ids = [...new Set(taskIds.map((id) => id.trim()).filter(Boolean))];
	if (ids.length === 0) throw new Error("task_ids must contain at least one task ID");
	if (ids.length > MAX_TASK_IDS) throw new Error(`task_ids may contain at most ${MAX_TASK_IDS} IDs`);
	return ids;
}

function jsonContent(value: unknown) {
	return [{ type: "text" as const, text: JSON.stringify(value, null, 2) }];
}

export default function (pi: ExtensionAPI) {
	const tasks = new Map<string, BackgroundTask>();
	const control = createBashControl(tasks);
	const nativeBash = createBashToolDefinition(process.cwd());
	const evalKernels: Record<EvalLanguage, PersistentEvalKernel> = {
		py: new PersistentEvalKernel("py"),
		js: new PersistentEvalKernel("js"),
	};
	const EvalParameters = Type.Object({
		language: StringEnum(["py", "js"] as const),
		code: Type.String({
			minLength: 1,
			description:
				"Code to run verbatim in the selected persistent kernel. Top-level await is supported.",
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
	});

	pi.registerTool({
		name: "eval",
		label: "Eval",
		description:
			"Run one step of Python or JavaScript in a persistent per-language kernel. " +
			"State survives across calls until reset, timeout, abort, process failure, cwd change, or session shutdown.",
		promptSnippet:
			"Run Python or JavaScript in a persistent kernel; reuse prior state and reset only when needed.",
		parameters: EvalParameters,
		async execute(
			_toolCallId,
			params: EvalParams,
			signal,
			_onUpdate,
			ctx: ExtensionContext,
		) {
			if (!params.code.trim()) throw new Error("eval code must not be empty");
			const startedAt = Date.now();
			const result = await evalKernels[params.language].execute(
				params.code,
				ctx.cwd,
				params.timeout ?? 30,
				signal,
				params.reset ?? false,
			);
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
				},
			};
		},
	});

	const BashParameters = Type.Object({
		command: Type.String({
			description:
				"Exact bash to run. No trailing comments (# ...). Put the human-readable UI label in task_name instead.",
		}),
		timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
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

	pi.registerTool({
		...nativeBash,
		parameters: BashParameters,
		description:
			`${nativeBash.description} ` +
			`Always set task_name: a short human-readable UI title (3–8 words) in the user's language ` +
			`(match the language of their messages). Required for every call — foreground and background. ` +
			`This is what the terminal UI shows instead of the raw shell, especially for long/complex commands. ` +
			`Never annotate command with # comments — put the label in task_name only.`,
		promptSnippet:
			"Run bash; always pass task_name (short UI title in user's language, fg+bg). No #comments in command.",
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
					stateChanged: control.sync,
					ui: ctx.ui,
				});
				tasks.set(task.toolCallId, task);
				control.sync();
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
							backgrounded: false,
							description: taskName,
							env: options.env ?? process.env,
							onData: options.onData,
							stateChanged: control.sync,
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
								activeTask.backgrounded = true;
								control.sync();
								settle("backgrounded");
							};
							if (options.signal?.aborted) {
								aborted();
							} else {
								options.signal?.addEventListener("abort", aborted, { once: true });
							}
							control.sync();
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
					control.sync();
				}
			}
		},
	});

	pi.registerTool({
		name: "get_task_output",
		label: "get_task_output",
		description: "Get output for one or more background bash tasks. Set timeout_ms to wait for completion; omit it to poll.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map((id) => [...tasks.values()].find((task) => task.taskId === id));
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !selected.find((task) => task?.taskId === id)) }) };
			const found = selected.filter(
				(task): task is BackgroundTask => task !== undefined,
			);
			if (params.timeout_ms && params.timeout_ms > 0) {
				await Promise.all(
					found.map((task) =>
						waitForCompletion(task, params.timeout_ms, signal),
					),
				);
			}
			const results = found.map(taskResult);
			return { content: jsonContent(results.length === 1 ? results[0] : { mode: "wait_all", results }) };
		},
	});

	pi.registerTool({
		name: "wait_tasks",
		label: "wait_tasks",
		description: "Wait for background bash tasks to finish.",
		parameters: Type.Object({
			task_ids: Type.Array(Type.String({ minLength: 1 })),
			mode: StringEnum(["wait_any", "wait_all"] as const),
			timeout_ms: Type.Optional(Type.Number({ minimum: 0 })),
		}),
		async execute(_toolCallId, params, signal) {
			const ids = ensureTaskIds(params.task_ids);
			const selected = ids.map((id) => [...tasks.values()].find((task) => task.taskId === id));
			if (selected.some((task) => !task)) return { content: jsonContent({ task_not_found: ids.filter((id) => !selected.find((task) => task?.taskId === id)) }) };
			const found = selected.filter(
				(task): task is BackgroundTask => task !== undefined,
			);
			const waits = found.map((task) =>
				waitForCompletion(task, params.timeout_ms, signal),
			);
			if (params.mode === "wait_any") await Promise.race(waits);
			else await Promise.all(waits);
			const results = found.map(taskResult);
			return { content: jsonContent({ mode: params.mode, results }) };
		},
	});

	pi.registerTool({
		name: "kill_task",
		label: "kill_task",
		description: "Terminate a running background bash task by task ID.",
		parameters: Type.Object({ task_id: Type.String({ minLength: 1 }) }),
		async execute(_toolCallId, params) {
			const task = [...tasks.values()].find((candidate) => candidate.taskId === params.task_id.trim());
			if (!task) return { content: jsonContent({ task_not_found: params.task_id }) };
			if (task.completed) return { content: jsonContent({ task_id: task.taskId, outcome: "already_exited" }) };
			task.explicitlyKilled = true;
			task.signal = "killed";
			killProcessTree(task);
			return { content: jsonContent({ task_id: task.taskId, outcome: "killed" }) };
		},
	});

	pi.on("session_shutdown", () => {
		control.close();
		for (const kernel of Object.values(evalKernels)) kernel.close();
		for (const task of tasks.values()) {
			if (task.completed) continue;
			task.explicitlyKilled = true;
			task.signal = ORPHANED_SIGNAL;
			killProcessTree(task);
		}
	});
}
