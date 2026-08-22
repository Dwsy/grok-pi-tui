import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import {
	findLoginProviderOptions,
	loginProviders,
	oauthLabelFor,
} from "./providers.ts";
import {
	ensurePiTheme,
	loadComponents,
	openCustom,
	prepareUi,
	resolveRuntime,
} from "./runtime.ts";
import type {
	AuthPrompt,
	AuthTui,
	LoginDialog,
	ModelRegistryLike,
	ProviderOption,
} from "./shared.ts";
import type { AuthType, OverlayHandle } from "./shared.ts";

export function registerLoginCommand(pi: ExtensionAPI): void {
	pi.registerCommand("login", {
		description: "Log in to a Pi model provider",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			try {
				if (!(await prepareUi(ctx, "login"))) return;

				const runtime = resolveRuntime(ctx);
				const registry = ctx.modelRegistry as unknown as ModelRegistryLike;
				const { OAuthSelectorComponent, LoginDialogComponent, ExtensionSelectorComponent } =
					await loadComponents();

				await runtime.getAvailable?.();

				const selectOption = async (
					tui: AuthTui,
					dialog: LoginDialog,
					prompt: Extract<AuthPrompt, { type: "select" }>,
				): Promise<string> => {
					const labels = prompt.options.map((option) => option.label);
					// Nested openCustom tears down LoginDialog (and its callback listener UI).
					// Overlay on the same remote-tui host, matching interactive-mode.
					if (typeof tui.showOverlay !== "function") {
						throw new Error("Remote TUI showOverlay unavailable for login select");
					}
					return new Promise<string>((resolve, reject) => {
						let settled = false;
						let overlay: OverlayHandle | undefined;
						const finish = (fn: () => void) => {
							if (settled) return;
							settled = true;
							try {
								overlay?.hide();
							} catch {
								/* ignore */
							}
							tui.setFocus?.(dialog);
							fn();
						};
						const selector = new ExtensionSelectorComponent(
							prompt.message,
							labels,
							(optionLabel) => {
								const id = prompt.options.find((option) => option.label === optionLabel)?.id;
								if (id) finish(() => resolve(id));
								else finish(() => reject(new Error("Login cancelled")));
							},
							() => finish(() => reject(new Error("Login cancelled"))),
						);
						overlay = tui.showOverlay!(selector);
						tui.setFocus?.(selector);
					});
				};

				const showAuthPrompt = async (
					tui: AuthTui,
					dialog: LoginDialog,
					prompt: AuthPrompt,
				): Promise<string> => {
					let response: Promise<string>;
					if (prompt.type === "select") {
						response = selectOption(tui, dialog, prompt);
					} else if (prompt.type === "manual_code") {
						response = dialog.showManualInput(prompt.message);
					} else {
						response = dialog.showPrompt(prompt.message, prompt.placeholder);
					}
					// Race prompt.signal so callback-server wins can cancel manual paste.
					if (!prompt.signal) return response;
					if (prompt.signal.aborted) throw new Error("Login cancelled");
					const signal = prompt.signal;
					let onAbort: (() => void) | undefined;
					const aborted = new Promise<string>((_resolve, reject) => {
						onAbort = () => reject(new Error("Login cancelled"));
						signal.addEventListener("abort", onAbort, { once: true });
					});
					try {
						return await Promise.race([response, aborted]);
					} finally {
						if (onAbort) signal.removeEventListener("abort", onAbort);
					}
				};

				const authenticate = async (provider: ProviderOption) => {
					await ensurePiTheme();
					const ensure = (
						globalThis as typeof globalThis & {
							__piGrokEnsureRemoteTuiHost?: (ui: ExtensionCommandContext["ui"]) => void;
						}
					).__piGrokEnsureRemoteTuiHost;
					if (typeof ensure === "function") ensure(ctx.ui);

					// Ambient/non-login methods: info only (matches interactive-mode).
					if (provider.authType === "api_key" && provider.method && !provider.method.login) {
						const { ran } = await openCustom<void>(ctx, (tui, _theme, _kb, done) => {
							const dialog = new LoginDialogComponent(
								tui,
								provider.id,
								() => done(undefined),
								provider.name,
								`${provider.name} setup`,
							);
							dialog.showInfo?.(
								`${provider.method?.name ?? "Authentication"} is configured outside pi.`,
								[],
								true,
							);
							return dialog;
						});
						if (!ran) {
							ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						}
						return;
					}

					let loginError: string | undefined;
					const { ran, value: success } = await openCustom<boolean>(ctx, (tui, _theme, _kb, done) => {
						const authTui = tui as AuthTui;
						const dialog = new LoginDialogComponent(
							tui,
							provider.id,
							() => done(false),
							provider.name,
						);

						void (async () => {
							try {
								await runtime.login(provider.id, provider.authType, {
									signal: dialog.signal,
									prompt: (prompt) => showAuthPrompt(authTui, dialog, prompt),
									notify: (event) => {
										if (event.type === "auth_url") {
											dialog.showAuth(event.url, event.instructions);
										} else if (event.type === "device_code") {
											dialog.showDeviceCode(event);
											dialog.showWaiting("Waiting for authentication...");
										} else if (event.type === "info") {
											dialog.showInfo?.(event.message, event.links);
										} else if (event.type === "progress") {
											dialog.showProgress(event.message);
										}
									},
								});
								await registry.refresh?.();
								await runtime.getAvailable?.();
								done(true);
							} catch (error: unknown) {
								const message = error instanceof Error ? error.message : String(error);
								if (message !== "Login cancelled") loginError = message;
								done(false);
							}
						})();

						return dialog;
					});

					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						return;
					}

					// Defer notify one tick so Pager finishes processing the widget
					// teardown frame before rendering the toast (same-frame toasts get swallowed).
					await new Promise<void>((resolve) => setTimeout(resolve, 60));
					if (loginError) {
						ctx.ui.notify(`Login failed: ${loginError}`, "error");
					} else if (success) {
						const message =
							provider.authType === "oauth"
								? `Logged in to ${provider.name}`
								: `Saved API key for ${provider.name}`;
						ctx.ui.notify(message, "info");
					}
				};

				const showProviderSelector = async (
					authType: AuthType | undefined,
					initialSearch?: string,
					onCancelBackToAuthType = false,
				) => {
					const providers = loginProviders(runtime, authType);
					if (providers.length === 0) {
						const message =
							authType === "oauth"
								? "No subscription providers available."
								: authType === "api_key"
									? "No API key providers available."
									: "No login providers available.";
						ctx.ui.notify(message, "warning");
						return;
					}

					const { ran } = await openCustom<void>(ctx, (_tui, _theme, _kb, done) => {
						return new OAuthSelectorComponent(
							"login",
							providers,
							(providerId, selectedAuthType) => {
								done(undefined);
								const provider = providers.find(
									(candidate) => candidate.id === providerId && candidate.authType === selectedAuthType,
								);
								if (provider) void authenticate(provider);
							},
							() => {
								done(undefined);
								if (onCancelBackToAuthType) {
									void showAuthTypeSelector();
								}
							},
							initialSearch,
						);
					});
					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
					}
				};

				const showAuthTypeSelector = async (scopedProviders?: ProviderOption[]) => {
					const subscriptionLabel = oauthLabelFor(scopedProviders);
					const apiKeyLabel = "Sign in with an API key";
					const available = scopedProviders
						? new Set(scopedProviders.map((provider) => provider.authType))
						: new Set<AuthType>(["oauth", "api_key"]);

					const options: string[] = [];
					if (available.has("oauth")) options.push(subscriptionLabel);
					if (available.has("api_key")) options.push(apiKeyLabel);

					if (options.length === 0) {
						ctx.ui.notify("No login methods available.", "warning");
						return;
					}

					// Single method for a scoped provider → go straight to login.
					if (scopedProviders && options.length === 1) {
						const only = scopedProviders[0];
						if (only) await authenticate(only);
						return;
					}

					const title = scopedProviders?.[0]
						? `Select authentication method for ${scopedProviders[0].name}:`
						: "Select authentication method:";

					const { ran, value } = await openCustom<string | undefined>(
						ctx,
						(_tui, _theme, _kb, done) =>
							new ExtensionSelectorComponent(title, options, done, () => done(undefined)),
					);
					if (!ran) {
						ctx.ui.notify("/login: Remote TUI custom() unavailable. Try /remote-tui.", "error");
						return;
					}
					if (!value) return;

					const authType: AuthType = value === subscriptionLabel ? "oauth" : "api_key";
					if (scopedProviders) {
						const provider = scopedProviders.find((candidate) => candidate.authType === authType);
						if (provider) await authenticate(provider);
						return;
					}
					await showProviderSelector(authType, undefined, true);
				};

				// --- match interactive-mode handleLoginCommand ---
				const providerRef = args.trim();
				if (!providerRef) {
					await showAuthTypeSelector();
					return;
				}

				const matches = findLoginProviderOptions(runtime, providerRef);
				if (matches.length === 1) {
					await authenticate(matches[0]!);
					return;
				}
				if (matches.length > 1) {
					const providerIds = new Set(matches.map((provider) => provider.id));
					if (providerIds.size === 1) {
						// Same provider, multiple auth methods → method picker.
						await showAuthTypeSelector(matches);
						return;
					}
				}
				if (matches.length === 0) {
					ctx.ui.notify(`No login provider matching "${providerRef}"`, "warning");
					return;
				}
				// Ambiguous ref across providers → provider list pre-filtered by search.
				await showProviderSelector(undefined, providerRef, false);
			} catch (error: unknown) {
				ctx.ui.notify(`/login failed: ${error instanceof Error ? error.message : String(error)}`, "error");
			}
		},
	});
}
