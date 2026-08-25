import * as path from "node:path";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";

import type {
	ExtensionSelectorConstructor,
	LoadedAuthComponents,
	LoginDialogConstructor,
	ModelRegistryLike,
	ModelRuntimeLike,
	OAuthSelectorConstructor,
} from "./shared.ts";

function hostUrl(relativePath: string): string {
	const entryDir = path.dirname(realpathSync(process.argv[1]!));
	if (path.basename(entryDir) === "bundle" && relativePath.startsWith("modes/interactive/components/")) {
		return new URL("index.js", pathToFileURL(entryDir).href + "/").href;
	}
	const hostDistDir = path.basename(entryDir) === "bundle" ? path.dirname(entryDir) : entryDir;
	return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

function ensureRemoteTuiHost(ui: ExtensionCommandContext["ui"]): void {
	const ensure = (
		globalThis as typeof globalThis & {
			__piGrokEnsureRemoteTuiHost?: (ui: ExtensionCommandContext["ui"]) => void;
		}
	).__piGrokEnsureRemoteTuiHost;
	if (typeof ensure === "function") ensure(ui);
}

export async function ensurePiTheme(): Promise<void> {
	const mod = (await import(hostUrl("modes/interactive/theme/theme.js"))) as {
		theme?: { name?: string };
		initTheme?: (name?: string, enableWatcher?: boolean) => void;
	};
	try {
		void mod.theme?.name;
	} catch {
		mod.initTheme?.(undefined, false);
		void mod.theme?.name;
	}
}

export function resolveRuntime(ctx: ExtensionCommandContext): ModelRuntimeLike {
	const registry = ctx.modelRegistry as unknown as ModelRegistryLike | undefined;
	const runtime = registry?.runtime;
	if (!runtime || typeof runtime.login !== "function" || typeof runtime.getProviders !== "function") {
		throw new Error(
			"Pi ModelRuntime unavailable on ctx.modelRegistry.runtime. " +
				"grok-pi requires Pi >= 0.84.3 (system `pi`).",
		);
	}
	return runtime;
}

export async function loadComponents(): Promise<LoadedAuthComponents> {
	const [oauth, login, selector] = await Promise.all([
		import(hostUrl("modes/interactive/components/oauth-selector.js")) as Promise<{
			OAuthSelectorComponent: OAuthSelectorConstructor;
		}>,
		import(hostUrl("modes/interactive/components/login-dialog.js")) as Promise<{
			LoginDialogComponent: LoginDialogConstructor;
		}>,
		import(hostUrl("modes/interactive/components/extension-selector.js")) as Promise<{
			ExtensionSelectorComponent: ExtensionSelectorConstructor;
		}>,
	]);
	return {
		OAuthSelectorComponent: oauth.OAuthSelectorComponent,
		LoginDialogComponent: login.LoginDialogComponent,
		ExtensionSelectorComponent: selector.ExtensionSelectorComponent,
	};
}

export async function openCustom<T>(
	ctx: ExtensionCommandContext,
	factory: (tui: TUI, theme: unknown, kb: unknown, done: (value: T) => void) => Component,
): Promise<{ ran: boolean; value: T | undefined }> {
	let ran = false;
	const value = await ctx.ui.custom<T>((tui, theme, kb, done) => {
		ran = true;
		return factory(tui as TUI, theme, kb, done);
	});
	return { ran, value };
}

export async function prepareUi(ctx: ExtensionCommandContext, command: string): Promise<boolean> {
	if (process.env.PI_GROK_REMOTE_TUI !== "1") {
		ctx.ui.notify(
			`/${command} needs PI_GROK_REMOTE_TUI=1 (Remote TUI). Restart grok-pi without PI_GROK_REMOTE_TUI=0.`,
			"error",
		);
		return false;
	}
	ensureRemoteTuiHost(ctx.ui);
	try {
		await ensurePiTheme();
	} catch (error: unknown) {
		ctx.ui.notify(
			`/${command}: theme init failed: ${error instanceof Error ? error.message : String(error)}`,
			"error",
		);
		return false;
	}
	return true;
}
