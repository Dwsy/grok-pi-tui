import { randomUUID } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";

import type { EvalExecution, EvalSkillMetadata, PersistentEvalKernel } from "./eval.ts";
import { formatTaskOutput, truncateTaskOutput } from "./shared.ts";

const TASK_STATUS_KEY = "__pi_grok_bash_task__";

type TaskStatusChannel = Pick<ExtensionUIContext, "setStatus">;

export type EvalBackgroundTask = {
	taskId: string;
	toolCallId: string;
	code: string;
	description?: string;
	cwd: string;
	outputFile: string;
	startedAt: number;
	endedAt?: number;
	output: string;
	truncated: boolean;
	completed: boolean;
	explicitlyKilled: boolean;
	error?: string;
	controller: AbortController;
	kernel: PersistentEvalKernel;
	waiters: Set<() => void>;
	ui?: TaskStatusChannel;
};

function systemTime(milliseconds: number) {
	return {
		secs_since_epoch: Math.floor(milliseconds / 1000),
		nanos_since_epoch: Math.floor(milliseconds % 1000) * 1_000_000,
	};
}

function boundedTaskOutput(task: EvalBackgroundTask) {
	return truncateTaskOutput(task.output, task.truncated);
}

function snapshot(task: EvalBackgroundTask) {
	const bounded = boundedTaskOutput(task);
	return {
		task_id: task.taskId,
		command: task.code,
		display_command: task.description ?? task.code,
		cwd: task.cwd,
		start_time: systemTime(task.startedAt),
		end_time: task.endedAt === undefined ? undefined : systemTime(task.endedAt),
		output: bounded.output,
		output_file: task.outputFile,
		truncated: bounded.truncated,
		exit_code: task.completed && !task.error && !task.explicitlyKilled ? 0 : undefined,
		signal: task.explicitlyKilled ? "killed" : task.error ? "eval_error" : undefined,
		completed: task.completed,
		kind: "eval",
		block_waited: false,
		explicitly_killed: task.explicitlyKilled,
		owner_session_id: undefined,
	};
}

function publish(task: EvalBackgroundTask, event: "started" | "completed") {
	if (!task.ui) return;
	const body = event === "started"
		? {
			version: 1,
			event,
			taskId: task.taskId,
			toolCallId: task.toolCallId,
			command: task.code,
			cwd: task.cwd,
			outputFile: task.outputFile,
			description: task.description,
		}
		: {
			version: 1,
			event,
			taskId: task.taskId,
			toolCallId: task.toolCallId,
			taskSnapshot: snapshot(task),
		};
	try {
		task.ui.setStatus(TASK_STATUS_KEY, JSON.stringify(body));
	} catch {
		// The model-facing task APIs remain authoritative if the UI channel detached.
	}
}

export async function startEvalBackgroundTask(params: {
	toolCallId: string;
	code: string;
	description?: string;
	cwd: string;
	timeout: number;
	ui?: TaskStatusChannel;
	kernel: PersistentEvalKernel;
	tools: Parameters<PersistentEvalKernel["execute"]>[5];
	skills: EvalSkillMetadata[];
	onSettled?: () => void;
}): Promise<EvalBackgroundTask> {
	const taskId = `eval-${randomUUID()}`;
	const directory = join(tmpdir(), "pi-grok-eval-tasks", taskId);
	await mkdir(directory, { recursive: true });
	const task: EvalBackgroundTask = {
		taskId,
		toolCallId: params.toolCallId,
		code: params.code,
		description: params.description?.trim() || undefined,
		cwd: params.cwd,
		outputFile: join(directory, "output.log"),
		startedAt: Date.now(),
		output: "",
		truncated: false,
		completed: false,
		explicitlyKilled: false,
		controller: new AbortController(),
		kernel: params.kernel,
		waiters: new Set(),
		ui: params.ui,
	};
	await writeFile(task.outputFile, "", "utf8");
	const log = createWriteStream(task.outputFile, { flags: "a" });
	publish(task, "started");

	void params.kernel
		.execute(
			params.code,
			params.cwd,
			params.timeout,
			task.controller.signal,
			false,
			params.tools,
			params.skills,
			(chunk) => log.write(chunk),
		)
		.then((result: EvalExecution) => {
			task.output = result.output;
			task.truncated = result.truncated;
		})
		.catch((error) => {
			if (!task.explicitlyKilled) task.error = error instanceof Error ? error.message : String(error);
		})
		.finally(async () => {
			task.completed = true;
			task.endedAt = Date.now();
			params.kernel.close();
			const rendered = task.error ? [task.output, task.error].filter(Boolean).join("\n") : task.output;
			task.output = rendered;
			await new Promise<void>((resolve) => log.end(resolve));
			publish(task, "completed");
			for (const waiter of task.waiters) waiter();
			task.waiters.clear();
			params.onSettled?.();
		});

	return task;
}

export function evalTaskResult(task: EvalBackgroundTask) {
	const bounded = boundedTaskOutput(task);
	return {
		task_id: task.taskId,
		command: task.code,
		status: !task.completed ? "running" : task.explicitlyKilled ? "cancelled" : task.error ? "failed" : "completed",
		exit_code: task.completed && !task.error && !task.explicitlyKilled ? 0 : undefined,
		started: new Date(task.startedAt).toISOString(),
		ended: task.endedAt === undefined ? undefined : new Date(task.endedAt).toISOString(),
		duration_secs: ((task.endedAt ?? Date.now()) - task.startedAt) / 1000,
		output: formatTaskOutput(bounded.output, bounded.truncated, task.outputFile),
		output_file: task.outputFile,
		truncated: bounded.truncated,
		raw_output_bytes: Buffer.byteLength(task.output),
		kind: "eval",
	};
}

export function waitForEvalTask(task: EvalBackgroundTask, timeoutMs: number | undefined, signal: AbortSignal | undefined) {
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
		if (signal?.aborted) return aborted();
		task.waiters.add(done);
		signal?.addEventListener("abort", aborted, { once: true });
		if (timeoutMs && timeoutMs > 0) timer = setTimeout(done, timeoutMs);
	});
}

export function killEvalTask(task: EvalBackgroundTask) {
	if (task.completed) return false;
	task.explicitlyKilled = true;
	task.controller.abort(new Error("killed"));
	return true;
}

