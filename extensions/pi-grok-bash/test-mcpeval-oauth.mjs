#!/usr/bin/env node
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { build } from "../../pi-main/node_modules/esbuild/lib/main.js";

const root = process.cwd();
const temp = await mkdtemp(path.join(tmpdir(), "mcpeval-oauth-"));
const bundle = path.join(temp, "oauth.mjs");
let server;
try {
  await build({
    entryPoints: [path.join(root, "extensions/pi-grok-bash/mcpeval-oauth.ts")],
    bundle: true,
    platform: "node",
    format: "esm",
    outfile: bundle,
    external: ["@modelcontextprotocol/sdk/*", "express"],
  });
  const { createMcpevalOAuthProvider, handleMcpevalOAuthRequest, verifyMcpevalBearer } = await import(pathToFileURL(bundle).href);
  const statePath = path.join(temp, "oauth-state.json");
  let provider;
  server = createServer(async (req, res) => {
    try {
      if (!provider || await handleMcpevalOAuthRequest(req, res, provider, { resourcePath: "/mcp" })) return;
      res.writeHead(404).end();
    } catch (error) {
      if (!res.headersSent) res.writeHead(500).end(String(error));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  provider = createMcpevalOAuthProvider({
    issuerUrl: new URL("http://127.0.0.1:" + port),
    resourceServerUrl: new URL("http://127.0.0.1:" + port + "/mcp"),
    statePath,
    accessTokenTtlSeconds: 60,
  });
  await provider.ready();
  const base = "http://127.0.0.1:" + port;
  const metadata = await (await fetch(base + "/.well-known/oauth-authorization-server")).json();
  if (metadata.code_challenge_methods_supported?.[0] !== "S256") throw new Error("missing S256 metadata");
  const protectedMetadata = await (await fetch(base + "/.well-known/oauth-protected-resource/mcp")).json();
  if (!protectedMetadata.authorization_servers?.length) throw new Error("missing protected resource metadata");

  const redirectUri = "http://127.0.0.1:32123/callback";
  const client = await (await fetch(base + "/oauth/register", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ client_name: "mcpeval-oauth-test", redirect_uris: [redirectUri], token_endpoint_auth_method: "client_secret_post" }),
  })).json();
  if (!client.client_id || !client.client_secret) throw new Error("DCR did not issue credentials");

  const verifier = "mcpeval-pkce-verifier";
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const authUrl = new URL(base + "/oauth/authorize");
  authUrl.searchParams.set("response_type", "code");
  authUrl.searchParams.set("client_id", client.client_id);
  authUrl.searchParams.set("redirect_uri", redirectUri);
  authUrl.searchParams.set("scope", "mcpeval");
  authUrl.searchParams.set("state", "state-1");
  authUrl.searchParams.set("code_challenge", challenge);
  authUrl.searchParams.set("code_challenge_method", "S256");
  const authResponse = await fetch(authUrl, { redirect: "manual" });
  if (authResponse.status !== 302) throw new Error("authorization did not redirect: " + authResponse.status);
  const callback = new URL(authResponse.headers.get("location"));
  if (callback.searchParams.get("state") !== "state-1") throw new Error("state was not preserved");
  const code = callback.searchParams.get("code");
  if (!code) throw new Error("authorization code missing");

  const wrong = new URLSearchParams({ grant_type: "authorization_code", client_id: client.client_id, client_secret: client.client_secret, code, code_verifier: "wrong", redirect_uri: redirectUri });
  const wrongResponse = await fetch(base + "/oauth/token", { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: wrong });
  if (wrongResponse.status !== 400) throw new Error("wrong PKCE verifier was accepted");

  const tokenResponse = await fetch(base + "/oauth/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "authorization_code", client_id: client.client_id, client_secret: client.client_secret, code, code_verifier: verifier, redirect_uri: redirectUri }),
  });
  if (!tokenResponse.ok) throw new Error("token exchange failed: " + await tokenResponse.text());
  const tokens = await tokenResponse.json();
  const authInfo = await verifyMcpevalBearer(provider, "Bearer " + tokens.access_token);
  if (!authInfo || authInfo.clientId !== client.client_id) throw new Error("bearer verification failed");

  const refreshResponse = await fetch(base + "/oauth/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "refresh_token", client_id: client.client_id, client_secret: client.client_secret, refresh_token: tokens.refresh_token }),
  });
  if (!refreshResponse.ok) throw new Error("refresh token exchange failed");
  const refreshed = await refreshResponse.json();
  const revokeResponse = await fetch(base + "/oauth/revoke", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ client_id: client.client_id, client_secret: client.client_secret, token: refreshed.access_token }),
  });
  if (revokeResponse.status !== 200) throw new Error("revocation failed");
  if (await verifyMcpevalBearer(provider, "Bearer " + refreshed.access_token)) throw new Error("revoked token still verifies");

  await provider.persist();
  const persisted = JSON.parse(await readFile(statePath, "utf8"));
  if (!persisted.clients?.[client.client_id]) throw new Error("OAuth state was not persisted");
  console.log("PASS: mcpeval OAuth metadata, DCR, PKCE, bearer verification, refresh, revocation and persistence");
} finally {
  if (server) await new Promise((resolve) => server.close(resolve));
  await rm(temp, { recursive: true, force: true });
}
