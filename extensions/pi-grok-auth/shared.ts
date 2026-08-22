import type { Component, TUI } from "@earendil-works/pi-tui";

export type AuthType = "oauth" | "api_key";

export type AuthMethod = {
	name?: string;
	loginLabel?: string;
	login?: unknown;
};

export type ProviderOption = {
	id: string;
	name: string;
	authType: AuthType;
	method?: AuthMethod;
	status?: { type: AuthType; source?: string };
};

export type ModelRuntimeLike = {
	getProviders: () => Array<{
		id: string;
		name: string;
		auth?: { oauth?: AuthMethod; apiKey?: AuthMethod };
	}>;
	getProvider?: (id: string) => { name?: string } | undefined;
	getProviderAuthStatus: (id: string) => {
		configured?: boolean;
		source?: string;
		label?: string;
	};
	isUsingOAuth?: (id: string) => boolean;
	listCredentials:
		| (() => Promise<Array<{ providerId: string; type: AuthType }>>)
		| (() => Array<{ providerId: string; type: AuthType }>);
	login: (
		providerId: string,
		method: AuthType,
		interaction: {
			signal?: AbortSignal;
			prompt: (prompt: AuthPrompt) => Promise<string>;
			notify: (event: AuthNotify) => void;
		},
	) => Promise<unknown>;
	logout: (providerId: string) => Promise<void>;
	getAvailable?: () => Promise<unknown> | unknown;
	refresh?: (options?: unknown) => Promise<unknown>;
};

export type AuthPrompt = { signal?: AbortSignal } &
	(
		| { type: "text"; message: string; placeholder?: string }
		| { type: "secret"; message: string; placeholder?: string }
		| { type: "manual_code"; message: string; placeholder?: string }
		| { type: "select"; message: string; options: Array<{ id: string; label: string }> }
	);

export type AuthNotify =
	| { type: "auth_url"; url: string; instructions?: string }
	| { type: "device_code"; userCode?: string; verificationUri?: string; [key: string]: unknown }
	| { type: "info"; message: string; links?: unknown[] }
	| { type: "progress"; message: string };

export type ModelRegistryLike = {
	runtime?: ModelRuntimeLike;
	getProviderDisplayName?: (id: string) => string;
	refresh?: () => unknown;
};

export interface OAuthSelectorConstructor {
	new (
		mode: "login" | "logout",
		providers: ProviderOption[],
		onSelect: (providerId: string, authType: AuthType) => void,
		onCancel: () => void,
		initialSearchInput?: string,
	): Component;
}

export type LoginDialog = Component & {
	signal: AbortSignal;
	showAuth(url: string, instructions?: string): void;
	showDeviceCode(info: unknown): void;
	showPrompt(message: string, placeholder?: string): Promise<string>;
	showManualInput(prompt: string): Promise<string>;
	showProgress(message: string): void;
	showWaiting(message: string): void;
	showInfo?(message: string, links?: unknown[], showCloseHint?: boolean): void;
	showDetails?(lines: string[]): void;
};

export interface LoginDialogConstructor {
	new (
		tui: TUI,
		providerId: string,
		onComplete: (success: boolean, message?: string) => void,
		providerNameOverride?: string,
		titleOverride?: string,
	): LoginDialog;
}

export type OverlayHandle = {
	hide: () => void;
	show?: () => void;
	setVisible?: (visible: boolean) => void;
};

export type AuthTui = TUI & {
	showOverlay?: (component: Component) => OverlayHandle;
	setFocus?: (component: Component | null) => void;
};

export interface ExtensionSelectorConstructor {
	new (
		title: string,
		options: string[],
		onSelect: (option: string) => void,
		onCancel: () => void,
	): Component;
}

export type LoadedAuthComponents = {
	OAuthSelectorComponent: OAuthSelectorConstructor;
	LoginDialogComponent: LoginDialogConstructor;
	ExtensionSelectorComponent: ExtensionSelectorConstructor;
};
