// Signing in and staying signed in. The CLI is a public OAuth client: it registers itself with the server's identity
// provider once (dynamic registration, like an MCP client), then runs authorization code + PKCE with a loopback
// redirect on a random port. On a headless box the browser is elsewhere and can't reach that port: pasting the URL
// it ended on works too. Tokens live in ~/.config/recordem/credentials.json (0600), per server.
import { spawn } from "node:child_process";
import { chmod, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

export const SCOPES = "openid profile offline_access meetings:read meetings:write transcripts:read audio:read";

const DIR = join(process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"), "recordem");
const FILE = join(DIR, "credentials.json");

type Tokens = { access_token: string; refresh_token?: string; expires_at: number };
/** `scope`: what the client registered for; a CLI asking for more registers again. */
type Server = { client_id: string; scope?: string; token_url: string; tokens?: Tokens };
type Store = { current?: string; servers: Record<string, Server> };

export class AuthError extends Error {}

async function load(): Promise<Store> {
  try {
    return JSON.parse(await readFile(FILE, "utf8")) as Store;
  } catch {
    return { servers: {} };
  }
}

async function save(s: Store) {
  await mkdir(DIR, { recursive: true, mode: 0o700 });
  await writeFile(FILE, JSON.stringify(s, null, 2), { mode: 0o600 });
  await chmod(FILE, 0o600);
}

/** The API to talk to: --server, else RECORDEM_URL, else the one signed in to last. */
export async function server(flag?: string) {
  const url = flag ?? process.env.RECORDEM_URL ?? (await load()).current;
  if (!url) throw new AuthError("no server: pass --server <api url> or set RECORDEM_URL (e.g. https://api.recordem.dev)");
  return url.replace(/\/+$/, "");
}

/** The servers signed in to before, and the one commands use when nothing else picks one. */
export async function known() {
  const s = await load();
  return { current: s.current, servers: Object.entries(s.servers).map(([url, v]) => ({ url, signed_in: !!v.tokens })) };
}

/** Makes `url` the current server; one never signed in to needs `recordem login --server` instead. */
export async function use(url: string) {
  const s = await load();
  const u = url.replace(/\/+$/, "");
  if (!s.servers[u]?.tokens) throw new AuthError(`not signed in to ${u}: run \`recordem login --server ${u}\``);
  s.current = u;
  await save(s);
  return u;
}

/** Where the API's identity provider is, from the API's own openapi.json. */
async function discover(api: string) {
  const doc = await (await fetch(`${api}/v1/openapi.json`)).json() as {
    components: { securitySchemes: { oauth2: { flows: { authorizationCode: { authorizationUrl: string; tokenUrl: string } } } } };
  };
  const { authorizationUrl, tokenUrl } = doc.components.securitySchemes.oauth2.flows.authorizationCode;
  const meta = await (await fetch(`${new URL(authorizationUrl).origin}/.well-known/oauth-authorization-server`)).json() as { registration_endpoint?: string };
  return { authorizationUrl, tokenUrl, registration: meta.registration_endpoint };
}

const b64url = (b: ArrayBuffer | Uint8Array) => Buffer.from(b as ArrayBuffer).toString("base64url");
const form = (o: Record<string, string>) => ({ method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(o) });

async function tokenRequest(tokenUrl: string, body: Record<string, string>): Promise<Tokens> {
  const res = await fetch(tokenUrl, form(body));
  const t = await res.json().catch(() => ({})) as { access_token?: string; refresh_token?: string; expires_in?: number; error_description?: string; error?: string };
  if (!res.ok || !t.access_token) throw new AuthError(`sign-in failed: ${t.error_description ?? t.error ?? res.status}`);
  return { access_token: t.access_token, refresh_token: t.refresh_token, expires_at: Date.now() + (t.expires_in ?? 3600) * 1000 };
}

/** The first line typed (or pasted) on stdin, or never when stdin isn't a terminal. */
function pasted(): { line: Promise<string>; close: () => void } {
  if (!process.stdin.isTTY) return { line: new Promise(() => {}), close: () => {} };
  const rl = createInterface({ input: process.stdin });
  return { line: new Promise((r) => rl.once("line", r)), close: () => rl.close() };
}

export async function login(api: string, log: (s: string) => void) {
  const store = await load();
  const d = await discover(api);
  const callback = Promise.withResolvers<URL>();
  const http = createServer((req, res) => {
    const u = new URL(req.url ?? "/", "http://127.0.0.1");
    if (u.pathname !== "/callback") return void res.writeHead(404).end("not found");
    callback.resolve(u);
    res.end("Signed in to Recordem. You can close this tab.");
  });
  await new Promise<void>((r) => http.listen(0, "127.0.0.1", r));
  const redirect = `http://127.0.0.1:${(http.address() as AddressInfo).port}/callback`;
  let client = store.servers[api]?.scope === SCOPES ? store.servers[api].client_id : undefined;
  if (!client) {
    if (!d.registration) throw new AuthError("this server doesn't let clients register themselves");
    const res = await fetch(d.registration, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({
      client_name: "Recordem CLI", redirect_uris: ["http://127.0.0.1/callback"], application_type: "native",
      token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"], scope: SCOPES,
    }) });
    const c = await res.json().catch(() => ({})) as { client_id?: string; error_description?: string };
    if (!res.ok || !c.client_id) throw new AuthError(`client registration failed: ${c.error_description ?? res.status}`);
    client = c.client_id;
  }
  const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
  const state = b64url(crypto.getRandomValues(new Uint8Array(16)));
  const url = `${d.authorizationUrl}?${new URLSearchParams({
    response_type: "code", client_id: client, redirect_uri: redirect, scope: SCOPES, state, resource: api,
    code_challenge: b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))), code_challenge_method: "S256",
  })}`;
  log(`Open this URL to sign in:\n\n  ${url}\n\nIf the browser is on another machine, it ends on a page that doesn't load: paste that page's URL here.`);
  spawn(process.platform === "darwin" ? "open" : "xdg-open", [url], { stdio: "ignore" }).on("error", () => {}).unref();
  const input = pasted();
  const back = await Promise.race([callback.promise, input.line.then((l) => new URL(l.trim()))]).finally(() => { input.close(); http.close(); });
  if (back.searchParams.get("state") !== state) throw new AuthError(`sign-in failed: ${back.searchParams.get("error_description") ?? back.searchParams.get("error") ?? "the URL is not from this sign-in"}`);
  const tokens = await tokenRequest(d.tokenUrl, {
    grant_type: "authorization_code", code: back.searchParams.get("code") ?? "", redirect_uri: redirect, client_id: client, code_verifier: verifier, resource: api,
  });
  store.servers[api] = { client_id: client, scope: SCOPES, token_url: d.tokenUrl, tokens };
  store.current = api;
  await save(store);
}

export async function logout(api: string) {
  const store = await load();
  if (store.servers[api]) delete store.servers[api].tokens;
  await save(store);
}

/** A valid access token for `api`: RECORDEM_TOKEN when set (an agent's or CI's own), else the stored one, refreshed
 * a minute before it expires. */
export async function token(api: string): Promise<string> {
  if (process.env.RECORDEM_TOKEN) return process.env.RECORDEM_TOKEN;
  const store = await load();
  const s = store.servers[api];
  if (!s?.tokens) throw new AuthError(`not signed in to ${api}: run \`recordem login\``);
  if (s.tokens.expires_at - 60_000 > Date.now()) return s.tokens.access_token;
  if (!s.tokens.refresh_token) throw new AuthError("the session expired: run `recordem login`");
  const refresh = s.tokens.refresh_token;
  const fresh = await tokenRequest(s.token_url, { grant_type: "refresh_token", refresh_token: refresh, client_id: s.client_id, resource: api })
    .catch(() => { throw new AuthError("the session expired: run `recordem login`"); });
  s.tokens = { ...fresh, refresh_token: fresh.refresh_token ?? refresh };
  await save(store);
  return s.tokens.access_token;
}
