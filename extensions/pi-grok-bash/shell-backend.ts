import { type ChildProcess, spawn } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";

export type BashBackendKind = "system" | "omp";

export const OMP_HOST_PROTOCOL_VERSION = 1;
export const OMP_TRACE_SCHEMA_VERSION = 1;

export type ShellTraceNodeState = "pending" | "running" | "ok" | "fail" | "skipped";

export type ShellTraceNode = {
	id: number;
	parent_id?: number;
	depth: number;
	kind: string;
	command: string;
	state: ShellTraceNodeState;
	exit_code?: number;
	duration_ms: number;
	runs?: number;
	background?: boolean;
};

export type ShellTraceSummary = {
	version: number;
	total_duration_ms: number;
	exit_code?: number;
	truncated: boolean;
	nodes: ShellTraceNode[];
};

export type BackendCompletedEvent = {
	exitCode: number | null;
	cancelled: boolean;
	timedOut: boolean;
	workingDir?: string;
	trace?: ShellTraceSummary;
};

export type LaunchBackendParams = {
	backend?: BashBackendKind;
	command: string;
	cwd: string;
	env: NodeJS.ProcessEnv;
	timeoutSeconds?: number;
	trace?: boolean;
	onOutput: (chunk: Buffer) => void;
	onCompletedEvent?: (event: BackendCompletedEvent) => void;
	onExit: (code: number | null, signal: NodeJS.Signals | null) => void;
	onError: (error: Error) => void;
};

export function resolveBashBackend(raw = process.env.PI_GROK_BASH_BACKEND): BashBackendKind {
	const normalized = raw?.trim().toLowerCase();
	if (normalized === "omp") return "omp";
	return "system";
}

function isExecutableFile(candidate: string): boolean {
	try {
		accessSync(candidate, constants.X_OK);
		return true;
	} catch {
		return false;
	}
}

export function resolveOmpHostBinary(): string {
	const explicit = process.env.PI_GROK_OMP_SHELL_HOST?.trim();
	if (explicit) {
		if (!isExecutableFile(explicit)) {
			throw new Error(
				`PI_GROK_OMP_SHELL_HOST points to a missing or non-executable binary: ${explicit}`,
			);
		}
		return explicit;
	}

	const binaryName = process.platform === "win32" ? "grok-pi-shell-host.exe" : "grok-pi-shell-host";
	const grokHome = process.env.GROK_HOME?.trim() || path.join(homedir(), ".grok-pi");
	const homeCandidate = path.join(grokHome, "bin", binaryName);
	if (isExecutableFile(homeCandidate)) {
		return homeCandidate;
	}

	const pathEntries = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
	for (const entry of pathEntries) {
		const candidate = path.join(entry, binaryName);
		if (isExecutableFile(candidate)) {
			return candidate;
		}
	}

	throw new Error(
		`OMP bash backend selected (PI_GROK_BASH_BACKEND=omp), but ${binaryName} was not found. ` +
			`Set PI_GROK_OMP_SHELL_HOST or install ${binaryName} in ${path.join(grokHome, "bin")} or PATH.`,
	);
}

function sanitizeEnv(env: NodeJS.ProcessEnv): Record<string, string> {
	const out: Record<string, string> = {};
	for (const [key, value] of Object.entries(env)) {
		if (typeof value === "string") {
			out[key] = value;
		}
	}
	return out;
}

export function launchBackendProcess(params: LaunchBackendParams): {
	backend: BashBackendKind;
	child: ChildProcess;
} {
	const backend = params.backend ?? resolveBashBackend();
	if (backend === "system") {
		const shell = process.platform === "win32" ? "bash" : "/bin/bash";
		const child = spawn(shell, ["-c", params.command], {
			cwd: params.cwd,
			env: params.env,
			detached: process.platform !== "win32",
			stdio: ["ignore", "pipe", "pipe"],
			windowsHide: true,
		});
		child.stdout?.on("data", params.onOutput);
		child.stderr?.on("data", params.onOutput);
		child.once("error", params.onError);
		child.once("close", params.onExit);
		return { backend, child };
	}

	const hostBinary = resolveOmpHostBinary();
	const child = spawn(hostBinary, [], {
		cwd: params.cwd,
		env: params.env,
		detached: process.platform !== "win32",
		stdio: ["pipe", "pipe", "pipe"],
		windowsHide: true,
	});

	let stdoutBuffer = "";
	let completedEvent: BackendCompletedEvent | undefined;
	let sawProtocolError = false;

	const handleLine = (line: string) => {
		const trimmed = line.trim();
		if (!trimmed) return;
		try {
			const event = JSON.parse(trimmed) as {
				type?: string;
				text?: string;
				exit_code?: number | null;
				cancelled?: boolean;
				timed_out?: boolean;
				working_dir?: string | null;
				trace?: ShellTraceSummary | null;
				code?: string;
				message?: string;
			};
			if (event.type === "output" && typeof event.text === "string") {
				params.onOutput(Buffer.from(event.text, "utf8"));
				return;
			}
			if (event.type === "completed") {
				completedEvent = {
					exitCode: typeof event.exit_code === "number" ? event.exit_code : null,
					cancelled: Boolean(event.cancelled),
					timedOut: Boolean(event.timed_out),
					workingDir: typeof event.working_dir === "string" ? event.working_dir : undefined,
					trace: event.trace ?? undefined,
				};
				params.onCompletedEvent?.(completedEvent);
				return;
			}
			if (event.type === "error") {
				sawProtocolError = true;
				const msg = `[grok-pi-shell-host ${event.code ?? "error"}] ${event.message ?? "unknown error"}\n`;
				params.onOutput(Buffer.from(msg, "utf8"));
			}
		} catch {
			params.onOutput(Buffer.from(`${line}\n`, "utf8"));
		}
	};

	child.stdout?.on("data", (chunk: Buffer) => {
		stdoutBuffer += chunk.toString("utf8");
		let newlineIdx = stdoutBuffer.indexOf("\n");
		while (newlineIdx !== -1) {
			const line = stdoutBuffer.slice(0, newlineIdx);
			stdoutBuffer = stdoutBuffer.slice(newlineIdx + 1);
			handleLine(line);
			newlineIdx = stdoutBuffer.indexOf("\n");
		}
	});
	child.stderr?.on("data", params.onOutput);
	child.once("error", params.onError);
	child.once("close", (code, signal) => {
		if (stdoutBuffer.trim()) {
			handleLine(stdoutBuffer);
			stdoutBuffer = "";
		}
		if (completedEvent) {
			const effectiveCode = completedEvent.exitCode ?? code ?? (sawProtocolError ? 1 : 0);
			params.onExit(effectiveCode, signal);
			return;
		}
		params.onExit(code ?? (sawProtocolError ? 1 : null), signal);
	});

	const timeoutMs =
		params.timeoutSeconds && params.timeoutSeconds > 0
			? Math.max(1, Math.round(params.timeoutSeconds * 1000))
			: undefined;

	const requestLine = JSON.stringify({
		protocol_version: OMP_HOST_PROTOCOL_VERSION,
		command: params.command,
		cwd: params.cwd,
		env: sanitizeEnv(params.env),
		timeout_ms: timeoutMs,
		trace: params.trace ?? true,
	});
	child.stdin?.write(`${requestLine}\n`);
	child.stdin?.end();

	return { backend, child };
}
