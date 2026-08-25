import * as path from "node:path";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai";
import type { Component, TUI } from "@earendil-works/pi-tui";

type SessionListProgress = (loaded: number, total: number) => void;

type SessionInfo = {
	path: string;
	name?: string;
};

interface ModelSelectorConstructor {
	new (
		tui: TUI,
		currentModel: Model<any> | undefined,
		settingsManager: { setDefaultModelAndProvider(provider: string, modelId: string): void },
		modelRegistry: ExtensionCommandContext["modelRegistry"],
		scopedModels: readonly [],
		onSelect: (model: Model<any>) => void,
		onCancel: () => void,
		initialSearchInput?: string,
	): Component;
}

interface SessionSelectorConstructor {
	new (
		currentSessionsLoader: (onProgress?: SessionListProgress) => Promise<SessionInfo[]>,
		allSessionsLoader: (onProgress?: SessionListProgress) => Promise<SessionInfo[]>,
		onSelect: (sessionPath: string) => void,
		onCancel: () => void,
		onExit: () => void,
		requestRender: () => void,
		options: { showRenameHint: boolean },
		currentSessionFilePath?: string,
	): Component;
}

interface SessionManagerStatic {
	list(cwd: string, sessionDir: string | undefined, onProgress?: SessionListProgress): Promise<SessionInfo[]>;
	listAll(sessionDir?: string, onProgress?: SessionListProgress): Promise<SessionInfo[]>;
}

interface SettingsManagerStatic {
	create(cwd: string): { setDefaultModelAndProvider(provider: string, modelId: string): void };
}

function hostUrl(relativePath: string): string {
	const entryDir = path.dirname(realpathSync(process.argv[1]!));
	if (
		path.basename(entryDir) === "bundle" &&
		(relativePath.startsWith("modes/interactive/components/") ||
			relativePath === "core/session-manager.js" ||
			relativePath === "core/settings-manager.js")
	) {
		return new URL("index.js", pathToFileURL(entryDir).href + "/").href;
	}
	const hostDistDir = path.basename(entryDir) === "bundle" ? path.dirname(entryDir) : entryDir;
	return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

function remoteTuiAvailable(): boolean {
	return process.env.PI_GROK_REMOTE_TUI === "1";
}

function unavailableCommand(name: string, reason: string) {
	return {
		description: `[experimental] Pi native /${name} (${reason})`,
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			ctx.ui.notify(`/${name} is not exposed by Pi RPC: ${reason}`, "warning");
		},
	};
}

export default async function piGrokNativeCommands(pi: ExtensionAPI) {
	if (process.env.PI_GROK !== "1") {
		return;
	}

	const [{ ModelSelectorComponent }, { SessionSelectorComponent, SessionManager, SettingsManager }] = await Promise.all([
		import(hostUrl("modes/interactive/components/model-selector.js")) as Promise<{
			ModelSelectorComponent: ModelSelectorConstructor;
		}>,
		Promise.all([
			import(hostUrl("modes/interactive/components/session-selector.js")),
			import(hostUrl("core/session-manager.js")),
			import(hostUrl("core/settings-manager.js")),
		]).then(([selector, manager, settings]) => ({
			SessionSelectorComponent: selector.SessionSelectorComponent as SessionSelectorConstructor,
			SessionManager: manager.SessionManager as SessionManagerStatic,
			SettingsManager: settings.SettingsManager as SettingsManagerStatic,
		})),
	]);

	pi.registerCommand("pi-model", {
		description: "[experimental] Open Pi's native model selector",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-model requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}

			await ctx.ui.custom<void>((tui, _theme, _keybindings, done) => {
				const selector = new ModelSelectorComponent(
					tui,
					ctx.model,
					SettingsManager.create(ctx.cwd),
					ctx.modelRegistry,
					[],
					(model) => {
						done(undefined);
						void pi.setModel(model);
					},
					() => done(undefined),
					args.trim() || undefined,
				);
				return selector;
			});
		},
	});

	pi.registerCommand("pi-resume", {
		description: "[experimental] Open Pi's native session selector",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			if (!remoteTuiAvailable()) {
				ctx.ui.notify("/pi-resume requires PI_GROK_REMOTE_TUI=1", "warning");
				return;
			}

			const switchSession = ctx.switchSession.bind(ctx);
			const sessionManager = ctx.sessionManager;
			await ctx.ui.custom<void>((_tui, _theme, _keybindings, done) => {
				const currentSessionsLoader = (onProgress?: (loaded: number, total: number) => void) =>
					SessionManager.list(sessionManager.getCwd(), sessionManager.getSessionDir(), onProgress);
				const allSessionsLoader = (onProgress?: (loaded: number, total: number) => void) =>
					SessionManager.listAll(sessionManager.getSessionDir(), onProgress);
				const selector = new SessionSelectorComponent(
					currentSessionsLoader,
					allSessionsLoader,
					(sessionPath) => {
						done(undefined);
						void switchSession(sessionPath);
					},
					() => done(undefined),
					() => done(undefined),
					() => {},
					{ showRenameHint: false },
					sessionManager.getSessionFile(),
				);
				return selector;
			});
		},
	});

	pi.registerCommand("pi-reload", {
		description: "Reload Pi resources using Pi's native reload lifecycle",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			await ctx.reload();
		},
	});

	// /login /logout → default-on pi-grok-auth; /export-html /pi-share → default-on pi-grok-export.

	pi.registerCommand("pi-import", unavailableCommand("import", "Pi RPC has no import-session operation"));
}
