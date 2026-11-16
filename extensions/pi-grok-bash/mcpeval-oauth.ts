/**
 * mcpeval OAuth 2.1 / MCP authorization.
 *
 * The resident daemon is a loopback resource server. Pi can keep using the
 * daemon bootstrap key, while external MCP clients use dynamic registration
 * plus PKCE. Provider contracts are from @modelcontextprotocol/sdk 1.30.x;
 * the HTTP adapter is native Node so the embedded extension needs no Express.
 */
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import path from "node:path";
import type { Response } from "express";
import type { OAuthServerProvider, AuthorizationParams } from "@modelcontextprotocol/sdk/server/auth/provider.js";
import type { OAuthRegisteredClientsStore } from "@modelcontextprotocol/sdk/server/auth/clients.js";
import type { AuthInfo } from "@modelcontextprotocol/sdk/server/auth/types.js";
import type {
  OAuthClientInformationFull,
  OAuthTokenRevocationRequest,
  OAuthTokens,
} from "@modelcontextprotocol/sdk/shared/auth.js";

const ACCESS_TOKEN_TTL_MS = 60 * 60 * 1000;
const REFRESH_TOKEN_TTL_MS = 30 * 24 * 60 * 60 * 1000;
const AUTHORIZATION_CODE_TTL_MS = 5 * 60 * 1000;
const CLIENT_SECRET_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_BODY_BYTES = 1024 * 1024;

type OAuthClientInput = Omit<OAuthClientInformationFull, "client_id" | "client_id_issued_at">;
type CodeRecord = {
  clientId: string;
  challenge: string;
  redirectUri: string;
  scopes: string[];
  resource?: string;
  expiresAt: number;
};
type TokenRecord = {
  clientId: string;
  scopes: string[];
  resource?: string;
  expiresAt: number;
  tokenType: "access" | "refresh";
};
type PersistedOAuthState = {
  clients: Record<string, OAuthClientInformationFull>;
  codes: Record<string, CodeRecord>;
  tokens: Record<string, TokenRecord>;
};

export type McpevalOAuthOptions = {
  issuerUrl: URL;
  resourceServerUrl?: URL;
  statePath?: string;
  scopes?: string[];
  accessTokenTtlSeconds?: number;
};

export type McpevalOAuthProvider = OAuthServerProvider & OAuthRegisteredClientsStore & {
  ready(): Promise<void>;
  persist(): Promise<void>;
  metadata(): Record<string, unknown>;
  protectedResourceMetadata(): Record<string, unknown>;
};

function base64Url(value: Buffer): string {
  return value.toString("base64").replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_");
}
function randomToken(bytes = 32): string { return base64Url(randomBytes(bytes)); }
function sha256Base64Url(value: string): string { return base64Url(createHash("sha256").update(value).digest()); }
function equalSecret(a: string, b: string): boolean {
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
}
function asScopes(value: unknown, fallback: string[]): string[] {
  const raw = Array.isArray(value) ? value : typeof value === "string" ? value.split(" ") : fallback;
  return [...new Set(raw.filter((item): item is string => typeof item === "string" && item.length > 0))];
}
function urlString(value: URL | string | undefined): string | undefined {
  return value ? value instanceof URL ? value.toString() : value : undefined;
}
function localRedirectAllowed(uri: string): boolean {
  try {
    const parsed = new URL(uri);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;
    return parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost" ||
      parsed.hostname === "[::1]" || parsed.protocol === "https:";
  } catch { return false; }
}

class McpevalOAuthProviderImpl implements McpevalOAuthProvider {
  private readonly clients = new Map<string, OAuthClientInformationFull>();
  private readonly codes = new Map<string, CodeRecord>();
  private readonly tokens = new Map<string, TokenRecord>();
  private loaded = false;
  private writeChain: Promise<void> = Promise.resolve();
  private readonly statePath?: string;
  private readonly issuerUrl: URL;
  private readonly resourceServerUrl: URL;
  private readonly scopes: string[];
  private readonly accessTokenTtlMs: number;

  public constructor(options: McpevalOAuthOptions) {
    this.statePath = options.statePath;
    this.issuerUrl = new URL(options.issuerUrl);
    this.resourceServerUrl = new URL(options.resourceServerUrl ?? options.issuerUrl);
    this.scopes = [...new Set(options.scopes ?? ["mcpeval"])];
    this.accessTokenTtlMs = Math.max(60, options.accessTokenTtlSeconds ?? ACCESS_TOKEN_TTL_MS / 1000) * 1000;
  }

  public async ready(): Promise<void> {
    if (this.loaded) return;
    this.loaded = true;
    if (!this.statePath) return;
    try {
      const parsed = JSON.parse(await readFile(this.statePath, "utf8")) as Partial<PersistedOAuthState>;
      for (const [key, value] of Object.entries(parsed.clients ?? {})) this.clients.set(key, value);
      const now = Date.now();
      for (const [key, value] of Object.entries(parsed.codes ?? {})) if (value.expiresAt > now) this.codes.set(key, value);
      for (const [key, value] of Object.entries(parsed.tokens ?? {})) if (value.expiresAt > now) this.tokens.set(key, value);
    } catch (error: any) {
      if (error?.code !== "ENOENT") console.warn("[mcpeval/oauth] ignoring unreadable state:", error?.message ?? error);
    }
  }

  public async persist(): Promise<void> {
    if (!this.statePath) return;
    await this.ready();
    const snapshot: PersistedOAuthState = {
      clients: Object.fromEntries(this.clients),
      codes: Object.fromEntries(this.codes),
      tokens: Object.fromEntries(this.tokens),
    };
    this.writeChain = this.writeChain.then(async () => {
      await mkdir(path.dirname(this.statePath!), { recursive: true });
      const temporary = this.statePath! + ".tmp-" + process.pid;
      await writeFile(temporary, JSON.stringify(snapshot), { encoding: "utf8", mode: 0o600 });
      await chmod(temporary, 0o600).catch(() => {});
      await rename(temporary, this.statePath!);
      await chmod(this.statePath!, 0o600).catch(() => {});
    });
    return this.writeChain;
  }

  public get clientsStore(): OAuthRegisteredClientsStore { return this; }

  public async getClient(clientId: string): Promise<OAuthClientInformationFull | undefined> {
    await this.ready();
    const client = this.clients.get(clientId);
    if (!client) return undefined;
    if (client.client_secret_expires_at && client.client_secret_expires_at < Math.floor(Date.now() / 1000)) {
      return { ...client, client_secret: undefined };
    }
    return client;
  }

  public async registerClient(input: OAuthClientInput): Promise<OAuthClientInformationFull> {
    await this.ready();
    const redirectUris = [...new Set((input.redirect_uris ?? []).map(String))];
    if (redirectUris.length === 0 || redirectUris.length > 8 || redirectUris.some(uri => !localRedirectAllowed(uri))) {
      throw new Error("mcpeval OAuth clients must register one to eight loopback or HTTPS redirect URIs");
    }
    const clientId = "mcpeval-" + randomToken(18);
    const secret = randomToken(32);
    const now = Math.floor(Date.now() / 1000);
    const client: OAuthClientInformationFull = {
      ...input,
      redirect_uris: redirectUris.map(uri => new URL(uri)),
      client_id: clientId,
      client_id_issued_at: now,
      client_secret: secret,
      client_secret_expires_at: now + CLIENT_SECRET_TTL_SECONDS,
      token_endpoint_auth_method: input.token_endpoint_auth_method ?? "client_secret_post",
      grant_types: input.grant_types ?? ["authorization_code", "refresh_token"],
      response_types: input.response_types ?? ["code"],
    };
    this.clients.set(clientId, client);
    await this.persist();
    return client;
  }

  public async authorize(client: OAuthClientInformationFull, params: AuthorizationParams, response: Response): Promise<void> {
    await this.ready();
    if (!client.redirect_uris.some(uri => String(uri) === params.redirectUri)) throw new Error("redirect_uri is not registered");
    if (!params.codeChallenge) throw new Error("PKCE code_challenge is required");
    const code = randomToken(32);
    this.codes.set(code, {
      clientId: client.client_id,
      challenge: params.codeChallenge,
      redirectUri: params.redirectUri,
      scopes: (params.scopes ?? this.scopes).filter(scope => this.scopes.includes(scope)),
      resource: urlString(params.resource),
      expiresAt: Date.now() + AUTHORIZATION_CODE_TTL_MS,
    });
    await this.persist();
    const target = new URL(params.redirectUri);
    target.searchParams.set("code", code);
    if (params.state) target.searchParams.set("state", params.state);
    response.statusCode = 302;
    response.setHeader("Location", target.toString());
    response.end();
  }

  public async challengeForAuthorizationCode(_client: OAuthClientInformationFull, authorizationCode: string): Promise<string> {
    await this.ready();
    const code = this.codes.get(authorizationCode);
    if (!code || code.expiresAt <= Date.now()) throw new Error("authorization code is invalid or expired");
    return code.challenge;
  }

  public async exchangeAuthorizationCode(client: OAuthClientInformationFull, authorizationCode: string, codeVerifier?: string, redirectUri?: string, resource?: URL): Promise<OAuthTokens> {
    await this.ready();
    const code = this.codes.get(authorizationCode);
    if (!code || code.expiresAt <= Date.now() || code.clientId !== client.client_id) throw new Error("authorization code is invalid");
    if (!codeVerifier || !equalSecret(sha256Base64Url(codeVerifier), code.challenge)) throw new Error("PKCE verification failed");
    if (redirectUri && redirectUri !== code.redirectUri) throw new Error("redirect_uri does not match");
    this.codes.delete(authorizationCode);
    const tokens = this.issueTokens(client.client_id, code.scopes, resource?.toString() ?? code.resource);
    await this.persist();
    return tokens;
  }

  public async exchangeRefreshToken(client: OAuthClientInformationFull, refreshToken: string, scopes?: string[], resource?: URL): Promise<OAuthTokens> {
    await this.ready();
    const record = this.tokens.get(refreshToken);
    if (!record || record.tokenType !== "refresh" || record.expiresAt <= Date.now() || record.clientId !== client.client_id) throw new Error("refresh token is invalid or expired");
    this.tokens.delete(refreshToken);
    const requested = (scopes ?? record.scopes).filter(scope => record.scopes.includes(scope) && this.scopes.includes(scope));
    const tokens = this.issueTokens(client.client_id, requested, resource?.toString() ?? record.resource);
    await this.persist();
    return tokens;
  }

  public async verifyAccessToken(token: string): Promise<AuthInfo> {
    await this.ready();
    const record = this.tokens.get(token);
    if (!record || record.tokenType !== "access" || record.expiresAt <= Date.now()) throw new Error("invalid or expired access token");
    return {
      token, clientId: record.clientId, scopes: [...record.scopes],
      expiresAt: Math.floor(record.expiresAt / 1000),
      resource: record.resource ? new URL(record.resource) : undefined,
      extra: { provider: "mcpeval", issuer: this.issuerUrl.toString() },
    };
  }

  public async revokeToken(_client: OAuthClientInformationFull, request: OAuthTokenRevocationRequest): Promise<void> {
    await this.ready();
    this.tokens.delete(request.token);
    await this.persist();
  }

  public metadata(): Record<string, unknown> {
    const base = this.issuerUrl.toString().replace(/\/$/, "");
    return {
      issuer: this.issuerUrl.toString(),
      authorization_endpoint: base + "/oauth/authorize",
      token_endpoint: base + "/oauth/token",
      registration_endpoint: base + "/oauth/register",
      revocation_endpoint: base + "/oauth/revoke",
      response_types_supported: ["code"],
      response_modes_supported: ["query"],
      grant_types_supported: ["authorization_code", "refresh_token"],
      token_endpoint_auth_methods_supported: ["none", "client_secret_post"],
      scopes_supported: this.scopes,
      code_challenge_methods_supported: ["S256"],
    };
  }

  public protectedResourceMetadata(): Record<string, unknown> {
    return {
      resource: this.resourceServerUrl.toString(),
      authorization_servers: [this.issuerUrl.toString()],
      bearer_methods_supported: ["header"],
      scopes_supported: this.scopes,
      resource_name: "pi-grok mcpeval resident daemon",
    };
  }

  private issueTokens(clientId: string, scopes: string[], resource?: string): OAuthTokens {
    const access = randomToken(32);
    const refresh = randomToken(40);
    const expiresAt = Date.now() + this.accessTokenTtlMs;
    this.tokens.set(access, { clientId, scopes: [...scopes], resource, expiresAt, tokenType: "access" });
    this.tokens.set(refresh, { clientId, scopes: [...scopes], resource, expiresAt: Date.now() + REFRESH_TOKEN_TTL_MS, tokenType: "refresh" });
    return {
      access_token: access, token_type: "Bearer",
      expires_in: Math.floor(this.accessTokenTtlMs / 1000),
      refresh_token: refresh, scope: scopes.join(" "),
    };
  }
}

export function createMcpevalOAuthProvider(options: McpevalOAuthOptions): McpevalOAuthProvider {
  return new McpevalOAuthProviderImpl(options);
}

export async function verifyMcpevalBearer(provider: Pick<McpevalOAuthProvider, "verifyAccessToken">, authorizationHeader: string | undefined): Promise<AuthInfo | undefined> {
  const match = /^Bearer\s+(.+)$/i.exec(authorizationHeader ?? "");
  if (!match) return undefined;
  return provider.verifyAccessToken(match[1]).catch(() => undefined);
}

async function readBody(request: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of request) {
    const value = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    total += value.length;
    if (total > MAX_BODY_BYTES) throw new Error("request body too large");
    chunks.push(value);
  }
  return Buffer.concat(chunks);
}
function writeJson(response: ServerResponse, status: number, value: unknown, headers: Record<string, string> = {}): void {
  response.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", ...headers });
  response.end(JSON.stringify(value));
}
function oauthError(response: ServerResponse, status: number, error: string, description: string): void {
  writeJson(response, status, { error, error_description: description });
}
function parseBasicAuth(value: string | undefined): { id: string; secret: string } | undefined {
  if (!value?.startsWith("Basic ")) return undefined;
  try {
    const decoded = Buffer.from(value.slice(6), "base64").toString("utf8");
    const separator = decoded.indexOf(":");
    if (separator < 0) return undefined;
    return { id: decodeURIComponent(decoded.slice(0, separator)), secret: decodeURIComponent(decoded.slice(separator + 1)) };
  } catch { return undefined; }
}
function clientCredentials(request: IncomingMessage, body: URLSearchParams): { id?: string; secret?: string } {
  const basic = parseBasicAuth(typeof request.headers.authorization === "string" ? request.headers.authorization : undefined);
  return basic ?? { id: body.get("client_id") ?? undefined, secret: body.get("client_secret") ?? undefined };
}
function checkClientSecret(client: OAuthClientInformationFull, provided: string | undefined): boolean {
  const expected = client.client_secret;
  return Boolean(expected && provided && equalSecret(expected, provided));
}
async function parseRequestBody(request: IncomingMessage): Promise<{ raw: string; form?: URLSearchParams; json?: any }> {
  const raw = (await readBody(request)).toString("utf8");
  const contentType = String(request.headers["content-type"] ?? "");
  if (contentType.includes("application/json")) return { raw, json: JSON.parse(raw || "{}") };
  return { raw, form: new URLSearchParams(raw) };
}

/**
 * Handles OAuth metadata, dynamic registration, authorization, token and
 * revocation endpoints. Return true when this request was consumed.
 */
export async function handleMcpevalOAuthRequest(request: IncomingMessage, response: ServerResponse, provider: McpevalOAuthProvider, options: { basePath?: string; resourcePath?: string } = {}): Promise<boolean> {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  const basePath = (options.basePath ?? "/oauth").replace(/\/$/, "");
  const resourcePath = options.resourcePath ?? "/mcp";
  if (url.pathname === "/.well-known/oauth-authorization-server" || url.pathname === basePath + "/.well-known") {
    writeJson(response, 200, provider.metadata()); return true;
  }
  if (url.pathname === "/.well-known/oauth-protected-resource" || url.pathname === "/.well-known/oauth-protected-resource" + resourcePath) {
    writeJson(response, 200, provider.protectedResourceMetadata()); return true;
  }
  if (!url.pathname.startsWith(basePath + "/")) return false;
  if (request.method === "GET" && url.pathname === basePath + "/authorize") {
    if (url.searchParams.get("response_type") !== "code") {
      oauthError(response, 400, "unsupported_response_type", "response_type=code is required"); return true;
    }
    if ((url.searchParams.get("code_challenge_method") ?? "S256") !== "S256") {
      oauthError(response, 400, "invalid_request", "only S256 PKCE is supported"); return true;
    }
    const clientId = url.searchParams.get("client_id") ?? "";
    const client = await provider.getClient(clientId);
    const redirectUri = url.searchParams.get("redirect_uri") ?? "";
    if (!client || !redirectUri || !client.redirect_uris.some(uri => String(uri) === redirectUri)) {
      oauthError(response, 400, "invalid_request", "unknown client or redirect_uri"); return true;
    }
    const requestedScopes = asScopes(url.searchParams.get("scope"), provider.metadata().scopes_supported as string[]);
    const supportedScopes = provider.metadata().scopes_supported as string[];
    if (requestedScopes.some(scope => !supportedScopes.includes(scope))) {
      oauthError(response, 400, "invalid_scope", "requested scope is not supported"); return true;
    }
    try {
      await provider.authorize(client, {
        codeChallenge: url.searchParams.get("code_challenge") ?? "",
        redirectUri,
        state: url.searchParams.get("state") ?? undefined,
        scopes: requestedScopes,
        resource: url.searchParams.get("resource") ? new URL(url.searchParams.get("resource")!) : undefined,
      }, response as unknown as Response);
    } catch (error: any) {
      oauthError(response, 400, "invalid_request", error?.message ?? "authorization failed");
    }
    return true;
  }
  if (request.method !== "POST") { oauthError(response, 405, "method_not_allowed", "POST required"); return true; }
  let parsed: { raw: string; form?: URLSearchParams; json?: any };
  try { parsed = await parseRequestBody(request); } catch (error: any) {
    oauthError(response, 400, "invalid_request", error?.message ?? "invalid body"); return true;
  }
  const body = parsed.form ?? new URLSearchParams();
  if (url.pathname === basePath + "/register") {
    try {
      const input = (parsed.json ?? Object.fromEntries(body.entries())) as OAuthClientInput;
      const client = await provider.registerClient(input);
      writeJson(response, 201, { ...client, redirect_uris: client.redirect_uris.map(String) });
    } catch (error: any) { oauthError(response, 400, "invalid_client_metadata", error?.message ?? "registration failed"); }
    return true;
  }
  if (url.pathname === basePath + "/token") {
    const grantType = body.get("grant_type");
    const creds = clientCredentials(request, body);
    const client = creds.id ? await provider.getClient(creds.id) : undefined;
    if (!client || (client.token_endpoint_auth_method !== "none" && !checkClientSecret(client, creds.secret))) {
      oauthError(response, 401, "invalid_client", "client authentication failed"); return true;
    }
    try {
      const tokens = grantType === "authorization_code"
        ? await provider.exchangeAuthorizationCode(client, body.get("code") ?? "", body.get("code_verifier") ?? undefined, body.get("redirect_uri") ?? undefined, body.get("resource") ? new URL(body.get("resource")!) : undefined)
        : grantType === "refresh_token"
          ? await provider.exchangeRefreshToken(client, body.get("refresh_token") ?? "", body.get("scope")?.split(" "))
          : (() => { throw new Error("unsupported grant_type"); })();
      writeJson(response, 200, tokens);
    } catch (error: any) { oauthError(response, 400, "invalid_grant", error?.message ?? "token exchange failed"); }
    return true;
  }
  if (url.pathname === basePath + "/revoke") {
    const creds = clientCredentials(request, body);
    const client = creds.id ? await provider.getClient(creds.id) : undefined;
    if (client) await provider.revokeToken?.(client, { token: body.get("token") ?? "", token_type_hint: body.get("token_type_hint") ?? undefined });
    response.writeHead(200, { "cache-control": "no-store" }); response.end(); return true;
  }
  return false;
}
