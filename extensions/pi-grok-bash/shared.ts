import type { ChildProcess } from "node:child_process";

export const MAX_OUTPUT_BYTES = 50 * 1024;
export const MAX_TIMEOUT_SECONDS = 2_147_483.647;

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
