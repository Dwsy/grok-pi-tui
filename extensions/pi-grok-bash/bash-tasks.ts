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

import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";

import {
	MAX_OUTPUT_BYTES,
	MAX_TIMEOUT_SECONDS,
	formatTaskOutput,
	killChildProcess,
	truncateTaskOutput,
} from "./shared.ts";

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
export const ORPHANED_SIGNAL = "session_restart";
const MAX_TASK_IDS = 20;
/**
 * Sequential session-scoped task IDs (`bash-1`, `bash-2`, …). Background tasks
 * live only in this process's task Map, so an ordinal is unique without the
 * token cost of a 36-char UUID in every model-visible result.
 */
let nextTaskOrdinal = 1;

export type BashParams = {
	command: string;
	timeout?: number;
	is_background?: boolean;
	/** Short UI label (Pager reads this via adapter → description). */
	task_name: string;
};

export type BackgroundTask = {
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
	autoBackgroundHandle?: ReturnType<typeof setTimeout>;
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

export type BashControl = {
	sync: () => void;
	close: () => void;
};

function taskState(task: BackgroundTask): string {
	if (!task.completed) return "running";
	if (task.explicitlyKilled) return "cancelled";
	return task.exitCode === 0 && !task.signal ? "completed" : "failed";
}

function boundedTaskOutput(task: BackgroundTask) {
	return truncateTaskOutput(task.output.toString("utf8"), task.truncated);
}

function taskSnapshot(task: BackgroundTask) {
	const bounded = boundedTaskOutput(task);
	return {
		task_id: task.taskId,
		command: task.command,
		display_command: task.command,
		cwd: task.cwd,
		start_time: systemTime(task.startedAt),
		end_time: task.endedAt === undefined ? undefined : systemTime(task.endedAt),
		output: bounded.output,
		output_file: task.outputFile,
		truncated: bounded.truncated,
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

export function taskResult(task: BackgroundTask) {
	const ended = task.endedAt === undefined ? undefined : new Date(task.endedAt).toISOString();
	const bounded = boundedTaskOutput(task);
	return {
		task_id: task.taskId,
		command: task.command,
		status: taskState(task),
		exit_code: task.exitCode,
		started: new Date(task.startedAt).toISOString(),
		ended,
		duration_secs: ((task.endedAt ?? Date.now()) - task.startedAt) / 1000,
		output: formatTaskOutput(bounded.output, bounded.truncated, task.outputFile),
		output_file: task.outputFile,
		truncated: bounded.truncated,
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

export function killProcessTree(task: BackgroundTask) {
	killChildProcess(task.child);
}

export function waitForCompletion(task: BackgroundTask, timeoutMs: number | undefined, signal: AbortSignal | undefined) {
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
	const modelOutput = formatTaskOutput(snapshot.output, snapshot.truncated, snapshot.output_file);
	const content = snapshot.explicitly_killed
		? `Background Bash task cancelled: ${task.command}`
		: failed
			? `Background Bash task failed: ${task.command}\n\n${modelOutput || "(no output)"}\n\nExit code: ${snapshot.exit_code ?? "none"}${snapshot.signal ? `; signal: ${snapshot.signal}` : ""}`
			: `Background Bash task completed: ${task.command}\n\n${modelOutput || "(no output)"}\n\nExit code: ${snapshot.exit_code ?? "none"}`;
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
	if (task.autoBackgroundHandle) clearTimeout(task.autoBackgroundHandle);
	task.log.end(() => {
		if (task.backgrounded) {
			publishTerminalState(task);
			try {
				// kill_task already returns the cancellation result; do not inject a
				// user message between the kill call and its tool result.
				if (!task.explicitlyKilled) emitCompleted(pi, task);
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
	if (!Number.isFinite(timeout) || timeout < 0 || timeout > MAX_TIMEOUT_SECONDS) {
		throw new Error(`Invalid timeout: must be between 0 and ${MAX_TIMEOUT_SECONDS} seconds`);
	}
}

export async function startTask(
	pi: ExtensionAPI,
	params: {
		toolCallId: string;
		command: string;
		description?: string;
		cwd: string;
		timeout?: number;
		autoBackgroundMs?: number;
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
		taskId: `bash-${nextTaskOrdinal++}`,
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
		if (!task.backgrounded) params.onData?.(chunk);
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
	if (!params.backgrounded && params.autoBackgroundMs !== undefined) {
		task.autoBackgroundHandle = setTimeout(() => task.promote?.(), params.autoBackgroundMs);
	}
	return task;
}

export function createBashControl(tasks: Map<string, BackgroundTask>): BashControl {
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

export function ensureTaskIds(taskIds: string[]) {
	const ids = [...new Set(taskIds.map((id) => id.trim()).filter(Boolean))];
	if (ids.length === 0) throw new Error("task_ids must contain at least one task ID");
	if (ids.length > MAX_TASK_IDS) throw new Error(`task_ids may contain at most ${MAX_TASK_IDS} IDs`);
	return ids;
}

export function jsonContent(value: unknown) {
	return [{ type: "text" as const, text: JSON.stringify(value, null, 2) }];
}

