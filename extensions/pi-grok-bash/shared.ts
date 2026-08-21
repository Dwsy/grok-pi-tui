import type { ChildProcess } from "node:child_process";

import { DEFAULT_MAX_BYTES, truncateTail } from "@earendil-works/pi-coding-agent";

export const MAX_OUTPUT_BYTES = DEFAULT_MAX_BYTES;
export const MAX_TIMEOUT_SECONDS = 2_147_483.647;
export const DEFAULT_MAX_WAIT_MINS = 4.5;

export function resolveMaxWaitMs(raw = process.env.PI_GROK_BASH_MAX_WAIT_MINS): number | undefined {
	const minutes = raw === undefined ? DEFAULT_MAX_WAIT_MINS : Number(raw);
	if (!Number.isFinite(minutes) || minutes <= 0) return undefined;
	return minutes * 60_000;
}

export function truncateTaskOutput(output: string, alreadyTruncated = false) {
	const truncation = truncateTail(output);
	return {
		output: truncation.content,
		truncated: alreadyTruncated || truncation.truncated,
	};
}

export function formatTaskOutput(output: string, truncated: boolean, outputFile: string) {
	if (!truncated) return output;
	return `${output}${output ? "\n\n" : ""}[Output truncated. Full output: ${outputFile}]`;
}

export function killChildProcess(child: ChildProcess) {
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
