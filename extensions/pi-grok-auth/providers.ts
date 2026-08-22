import type { ModelRuntimeLike, ProviderOption } from "./shared.ts";
import type { AuthType } from "./shared.ts";

export function loginProviders(runtime: ModelRuntimeLike, authType?: AuthType): ProviderOption[] {
	const options: ProviderOption[] = [];
	for (const provider of runtime.getProviders()) {
		const authStatus = runtime.getProviderAuthStatus(provider.id);
		const status = authStatus?.configured
			? {
					type: (runtime.isUsingOAuth?.(provider.id) ? "oauth" : "api_key") as AuthType,
					source: authStatus.label ?? authStatus.source,
				}
			: undefined;

		if ((!authType || authType === "oauth") && provider.auth?.oauth) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "oauth",
				method: provider.auth.oauth,
				status,
			});
		}
		if ((!authType || authType === "api_key") && provider.auth?.apiKey) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "api_key",
				method: provider.auth.apiKey,
				status,
			});
		}
	}
	return options.sort((a, b) => a.name.localeCompare(b.name));
}

export function findLoginProviderOptions(runtime: ModelRuntimeLike, providerRef: string): ProviderOption[] {
	const query = providerRef.trim().toLowerCase();
	if (!query) return loginProviders(runtime);
	return loginProviders(runtime).filter(
		(provider) => provider.id.toLowerCase() === query || provider.name.toLowerCase() === query,
	);
}

export async function logoutProviders(runtime: ModelRuntimeLike): Promise<ProviderOption[]> {
	const credentials = await runtime.listCredentials();
	return credentials
		.map(({ providerId, type }) => ({
			id: providerId,
			name: runtime.getProvider?.(providerId)?.name ?? providerId,
			authType: type,
			status: { type, source: "stored credential" },
		}))
		.sort((a, b) => a.name.localeCompare(b.name));
}

export function oauthLabelFor(options?: ProviderOption[]): string {
	const oauth = options?.find((provider) => provider.authType === "oauth");
	const custom = oauth?.method?.loginLabel;
	return typeof custom === "string" && custom.trim() ? custom : "Sign in with an account";
}
