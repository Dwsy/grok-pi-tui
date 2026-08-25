import { createWriteStream } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { join } from "node:path";

import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";

import type { EvalDisplayImage, EvalExecution, EvalSkillMetadata, PersistentEvalKernel } from "./eval.ts";
import { formatTaskOutput, truncateTaskOutput } from "./shared.ts";

const TASK_STATUS_KEY = "__pi_grok_bash_task__";
/**
 * Sequential session-scoped task IDs (`eval-1`, `eval-2`, …) keep model-visible
 * results short. The tmp directory nests under the pid because the ordinal is
 * only unique per process, while `/tmp/pi-grok-eval-tasks` is shared.
 */
let nextEvalTaskOrdinal = 1;

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
	backgrounded: boolean;
	ownsKernel: boolean;
	explicitlyKilled: boolean;
	error?: string;
	result?: EvalExecution;
	controller: AbortController;
	kernel: PersistentEvalKernel;
	waiters: Set<() => void>;
	autoBackgroundHandle?: ReturnType<typeof setTimeout>;
	foregroundSettler?: (outcome: "completed" | "backgrounded") => void;
	promote?: () => void;
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

function imageFileExtension(mimeType: string): string {
	if (mimeType.includes("jpeg") || mimeType.includes("jpg")) return "jpg";
	if (mimeType.includes("gif")) return "gif";
	if (mimeType.includes("webp")) return "webp";
	return "png";
}

/** Persist displayed images next to the output log; background tasks report file paths instead of base64. */
async function saveDisplayedImages(task: EvalBackgroundTask, images: EvalDisplayImage[]): Promise<void> {
	const directory = path.dirname(task.outputFile);
	const saved: string[] = [];
	for (const [index, image] of images.entries()) {
		const file = join(directory, `display-${index + 1}.${imageFileExtension(image.mimeType)}`);
		await writeFile(file, Buffer.from(image.data, "base64"));
		saved.push(file);
	}
	if (saved.length > 0) {
		task.output = `${task.output}\n${saved.map((file) => `[display image saved: ${file}]`).join("\n")}`;
	}
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
	backgrounded?: boolean;
	ownsKernel?: boolean;
	onSettled?: () => void;
}): Promise<EvalBackgroundTask> {
	const taskId = `eval-${nextEvalTaskOrdinal++}`;
	const directory = join(tmpdir(), "pi-grok-eval-tasks", String(process.pid), taskId);
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
		backgrounded: params.backgrounded ?? true,
		ownsKernel: params.ownsKernel ?? true,
		explicitlyKilled: false,
		controller: new AbortController(),
		kernel: params.kernel,
		waiters: new Set(),
		ui: params.ui,
	};
	await writeFile(task.outputFile, "", "utf8");
	const log = createWriteStream(task.outputFile, { flags: "a" });
	if (task.backgrounded) publish(task, "started");

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
		.then(async (result: EvalExecution) => {
			task.result = result;
			task.output = result.output;
			task.truncated = result.truncated;
			if (result.images && result.images.length > 0) await saveDisplayedImages(task, result.images);
		})
		.catch((error) => {
			if (!task.explicitlyKilled) task.error = error instanceof Error ? error.message : String(error);
		})
		.finally(async () => {
			task.completed = true;
			task.endedAt = Date.now();
			if (task.autoBackgroundHandle) clearTimeout(task.autoBackgroundHandle);
			if (task.ownsKernel) params.kernel.close();
			const rendered = task.error ? [task.output, task.error].filter(Boolean).join("\n") : task.output;
			task.output = rendered;
			await new Promise<void>((resolve) => log.end(resolve));
			if (task.backgrounded) publish(task, "completed");
			const settleForeground = task.foregroundSettler;
			task.foregroundSettler = undefined;
			settleForeground?.("completed");
			for (const waiter of task.waiters) waiter();
			task.waiters.clear();
			params.onSettled?.();
		});

	return task;
}

export function promoteEvalTask(task: EvalBackgroundTask) {
	if (task.completed || task.backgrounded) return false;
	if (task.autoBackgroundHandle) clearTimeout(task.autoBackgroundHandle);
	task.autoBackgroundHandle = undefined;
	task.backgrounded = true;
	task.ownsKernel = true;
	publish(task, "started");
	const settleForeground = task.foregroundSettler;
	task.foregroundSettler = undefined;
	settleForeground?.("backgrounded");
	return true;
}

export function armEvalAutoBackground(task: EvalBackgroundTask, timeoutMs: number | undefined) {
	if (timeoutMs === undefined || task.backgrounded || task.completed) return;
	task.autoBackgroundHandle = setTimeout(() => task.promote?.(), timeoutMs);
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

