/**
 * Default-on Pi session export/share for grok-pi.
 *
 * - `/export-html` → Pi host exportSessionToHtml (or branch JSONL)
 * - `/pi-share` → private gh gist + pi.dev viewer
 *
 * Grok `/export` remains Markdown transcript.
 */

import * as path from "node:path";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { spawn, spawnSync } from "node:child_process";
import * as os from "node:os";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

function hostUrl(relativePath: string): string {
	const entryDir = path.dirname(realpathSync(process.argv[1]!));
	if (path.basename(entryDir) === "bundle" && relativePath === "core/session-manager.js") {
		return new URL("index.js", pathToFileURL(entryDir).href + "/").href;
	}
	const hostDistDir = path.basename(entryDir) === "bundle" ? path.dirname(entryDir) : entryDir;
	return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

/** First path token after a slash command; supports single/double quotes. */
function pathCommandArgument(args: string): string | undefined {
	const argsString = args.trimStart();
	if (!argsString) return undefined;

	const firstChar = argsString[0];
	if (firstChar === '"' || firstChar === "'") {
		const closingQuoteIndex = argsString.indexOf(firstChar, 1);
		if (closingQuoteIndex < 0) return undefined;
		return argsString.slice(1, closingQuoteIndex);
	}

	const firstWhitespaceIndex = argsString.search(/\s/);
	if (firstWhitespaceIndex < 0) return argsString;
	return argsString.slice(0, firstWhitespaceIndex);
}

function expandUserPath(outputPath: string, cwd: string): string {
	const expanded =
		outputPath.startsWith("~/") || outputPath === "~"
			? path.join(os.homedir(), outputPath.slice(1))
			: outputPath;
	return path.isAbsolute(expanded) ? expanded : path.resolve(cwd, expanded);
}

type ExportSessionToHtml = (
	sm: ExtensionCommandContext["sessionManager"],
	state?: unknown,
	options?: { outputPath?: string } | string,
) => Promise<string>;

type SessionHeader = {
	type: "session";
	version: number;
	id: string;
	timestamp: string;
	cwd: string;
};

function exportBranchToJsonl(
	sessionManager: ExtensionCommandContext["sessionManager"],
	outputPath: string | undefined,
	currentSessionVersion: number,
): string {
	const filePath = expandUserPath(
		outputPath ?? `session-${new Date().toISOString().replace(/[:.]/g, "-")}.jsonl`,
		process.cwd(),
	);
	const dir = path.dirname(filePath);
	if (!existsSync(dir)) {
		mkdirSync(dir, { recursive: true });
	}

	const header: SessionHeader = {
		type: "session",
		version: currentSessionVersion,
		id: sessionManager.getSessionId(),
		timestamp: new Date().toISOString(),
		cwd: sessionManager.getCwd(),
	};

	const branchEntries = sessionManager.getBranch();
	const lines = [JSON.stringify(header)];
	let prevId: string | null = null;
	for (const entry of branchEntries) {
		const linear = { ...entry, parentId: prevId };
		lines.push(JSON.stringify(linear));
		prevId = entry.id;
	}

	writeFileSync(filePath, `${lines.join("\n")}\n`);
	return filePath;
}

async function runGhGistCreate(tmpFile: string): Promise<{ stdout: string; stderr: string; code: number | null }> {
	return await new Promise((resolve) => {
		const proc = spawn("gh", ["gist", "create", "--public=false", tmpFile]);
		let stdout = "";
		let stderr = "";
		proc.stdout?.on("data", (data) => {
			stdout += data.toString();
		});
		proc.stderr?.on("data", (data) => {
			stderr += data.toString();
		});
		proc.on("close", (code) => resolve({ stdout, stderr, code }));
		proc.on("error", (error) => resolve({ stdout, stderr: error.message, code: 1 }));
	});
}

export default async function piGrokExport(pi: ExtensionAPI) {
	if (process.env.PI_GROK !== "1") {
		return;
	}

	const [{ exportSessionToHtml }, { getShareViewerUrl }, { CURRENT_SESSION_VERSION }] = await Promise.all([
		import(hostUrl("core/export-html/index.js")) as Promise<{ exportSessionToHtml: ExportSessionToHtml }>,
		import(hostUrl("config.js")) as Promise<{ getShareViewerUrl: (gistId: string) => string }>,
		import(hostUrl("core/session-manager.js")) as Promise<{ CURRENT_SESSION_VERSION: number }>,
	]);

	pi.registerCommand("export-html", {
		description: "Export the current Pi session as HTML (or .jsonl if path ends with .jsonl)",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			const outputPath = pathCommandArgument(args);
			try {
				if (outputPath?.endsWith(".jsonl")) {
					const filePath = exportBranchToJsonl(ctx.sessionManager, outputPath, CURRENT_SESSION_VERSION);
					ctx.ui.notify(`Session exported to: ${filePath}`, "info");
					return;
				}

				const resolvedOutput = outputPath ? expandUserPath(outputPath, ctx.cwd) : undefined;
				const filePath = await exportSessionToHtml(ctx.sessionManager, undefined, resolvedOutput);
				ctx.ui.notify(`Session exported to: ${filePath}`, "info");
			} catch (error: unknown) {
				ctx.ui.notify(
					`Failed to export session: ${error instanceof Error ? error.message : String(error)}`,
					"error",
				);
			}
		},
	});

	pi.registerCommand("pi-share", {
		description: "Share the current Pi session via private GitHub gist",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			try {
				const authResult = spawnSync("gh", ["auth", "status"], { encoding: "utf-8" });
				if (authResult.error) {
					ctx.ui.notify("GitHub CLI (gh) is not installed. Install it from https://cli.github.com/", "error");
					return;
				}
				if (authResult.status !== 0) {
					ctx.ui.notify("GitHub CLI is not logged in. Run 'gh auth login' first.", "error");
					return;
				}
			} catch {
				ctx.ui.notify("GitHub CLI (gh) is not installed. Install it from https://cli.github.com/", "error");
				return;
			}

			// pi.dev's gist viewer loads the canonical `session.html` filename.
			// Isolate each invocation in its own temp directory to avoid races while
			// preserving that exact basename inside the gist.
			const tmpDir = mkdtempSync(path.join(os.tmpdir(), "pi-grok-share-"));
			const tmpFile = path.join(tmpDir, "session.html");
			try {
				await exportSessionToHtml(ctx.sessionManager, undefined, tmpFile);
			} catch (error: unknown) {
				ctx.ui.notify(
					`Failed to export session: ${error instanceof Error ? error.message : String(error)}`,
					"error",
				);
				return;
			}

			ctx.ui.notify("Creating private gist...", "info");
			try {
				const result = await runGhGistCreate(tmpFile);
				if (result.code !== 0) {
					const errorMsg = result.stderr?.trim() || "Unknown error";
					ctx.ui.notify(`Failed to create gist: ${errorMsg}`, "error");
					return;
				}

				const gistUrl = result.stdout?.trim();
				const gistId = gistUrl?.split("/").pop();
				if (!gistId) {
					ctx.ui.notify("Failed to parse gist ID from gh output", "error");
					return;
				}

				const previewUrl = getShareViewerUrl(gistId);
				ctx.ui.notify(`Share URL: ${previewUrl}\nGist: ${gistUrl}`, "info");
			} catch (error: unknown) {
				ctx.ui.notify(
					`Failed to create gist: ${error instanceof Error ? error.message : String(error)}`,
					"error",
				);
			} finally {
				try {
					rmSync(tmpDir, { recursive: true, force: true });
				} catch {
					// Ignore cleanup errors
				}
			}
		},
	});
}
