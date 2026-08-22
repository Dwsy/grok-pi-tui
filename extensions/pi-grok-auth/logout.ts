import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import { logoutProviders } from "./providers.ts";
import { loadComponents, openCustom, prepareUi, resolveRuntime } from "./runtime.ts";
import type { ModelRegistryLike } from "./shared.ts";

export function registerLogoutCommand(pi: ExtensionAPI): void {
	pi.registerCommand("logout", {
		description: "Remove a stored Pi provider credential",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			try {
				if (!(await prepareUi(ctx, "logout"))) return;

				const runtime = resolveRuntime(ctx);
				const registry = ctx.modelRegistry as unknown as ModelRegistryLike;
				const { OAuthSelectorComponent } = await loadComponents();
				const providers = await logoutProviders(runtime);

				if (providers.length === 0) {
					ctx.ui.notify(
						"No credentials saved by /login. Environment variables and models.json are unchanged.",
						"info",
					);
					return;
				}

				const { ran } = await openCustom<void>(ctx, (_tui, _theme, _kb, done) => {
					return new OAuthSelectorComponent(
						"logout",
						providers,
						(providerId) => {
							const provider = providers.find((candidate) => candidate.id === providerId);
							if (!provider) return;
							void (async () => {
								try {
									await runtime.logout(providerId);
									await registry.refresh?.();
									done(undefined);
									ctx.ui.notify(`Logged out of ${provider.name}`, "info");
								} catch (error: unknown) {
									done(undefined);
									ctx.ui.notify(
										`Logout failed: ${error instanceof Error ? error.message : String(error)}`,
										"error",
									);
								}
							})();
						},
						() => done(undefined),
					);
				});

				if (!ran) {
					ctx.ui.notify("/logout: Remote TUI custom() unavailable. Try /remote-tui.", "error");
				}
			} catch (error: unknown) {
				ctx.ui.notify(`/logout failed: ${error instanceof Error ? error.message : String(error)}`, "error");
			}
		},
	});
}
