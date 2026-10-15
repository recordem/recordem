// End-to-end check of the first milestone, as a CLI would do it: a new user signs up inside an OAuth flow
// (authorization code + PKCE, public client), consents, gets a token, and calls the API with it.
//   bun scripts/smoke.ts            (auth and api running; reads AUTH_URL / API_URL from the environment)
const AUTH = process.env.AUTH_URL ?? "http://localhost:39410";
const API = process.env.API_URL ?? "http://localhost:39411";
const REDIRECT = "http://127.0.0.1:39499/callback";

const jar = new Map<string, string>();
const cookie = () => [...jar].map(([k, v]) => `${k}=${v}`).join("; ");
async function go(url: string, init: RequestInit = {}) {
  const res = await fetch(url, { ...init, redirect: "manual", headers: { ...init.headers, cookie: cookie() } });
  for (const c of res.headers.getSetCookie()) { const [kv] = c.split(";"); const i = kv.indexOf("="); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return res;
}
const form = (o: Record<string, string>) => ({ method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(o) });
const step = (name: string, ok: boolean, detail?: unknown) => { console.log(`${ok ? "✓" : "✗"} ${name}`, detail ?? ""); if (!ok) process.exit(1); };
const b64url = (b: ArrayBuffer | Uint8Array) => Buffer.from(b as ArrayBuffer).toString("base64url");

// 1. a public client registers itself (what the CLI or an MCP client does once)
const reg = await fetch(`${AUTH}/api/auth/oauth2/register`, { method: "POST", headers: { "content-type": "application/json" },
  body: JSON.stringify({ client_name: "smoke", redirect_uris: [REDIRECT], application_type: "native", token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"] }) });
const client = await reg.json() as { client_id: string };
step("register public client", reg.ok, reg.ok ? client.client_id : client);

// 2. authorize → sign-in page (no session yet)
const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
const challenge = b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)));
const q = new URLSearchParams({ response_type: "code", client_id: client.client_id, redirect_uri: REDIRECT, state: "s1",
  scope: "openid profile meetings:read offline_access", code_challenge: challenge, code_challenge_method: "S256", resource: API });
let res = await go(`${AUTH}/api/auth/oauth2/authorize?${q}`);
let loc = res.headers.get("location") ?? "";
step("authorize redirects to sign-in", loc.includes("/sign-in"), loc.slice(0, 80));

// 3. sign up on that page; the flow continues by itself, to consent
const email = `smoke+${Date.now()}@recordem.dev`;
res = await go(new URL(loc, AUTH).href.replace("/sign-in", "/sign-in"), form({ email, password: "smoke-password-1", create: "on", name: "Smoke" }));
loc = res.headers.get("location") ?? "";
while (loc && !loc.includes("/consent") && !loc.startsWith(REDIRECT) && !loc.includes("error=")) { res = await go(new URL(loc, AUTH).href); loc = res.headers.get("location") ?? ""; }
step("sign-up continues to consent", loc.includes("/consent"), loc.slice(0, 100));

// 4. consent → back to the client with a code
res = await go(new URL(loc, AUTH).href.replace(/^(.*?)\/consent/, `${AUTH}/consent`), form({ accept: "yes" }));
loc = res.headers.get("location") ?? "";
const code = new URL(loc, AUTH).searchParams.get("code");
step("consent returns a code", !!code && loc.startsWith(REDIRECT), loc.slice(0, 100));

// 5. code → tokens
const tok = await fetch(`${AUTH}/api/auth/oauth2/token`, form({ grant_type: "authorization_code", code: code!, redirect_uri: REDIRECT,
  client_id: client.client_id, code_verifier: verifier, resource: API }));
const tokens = await tok.json() as { access_token: string; refresh_token?: string };
step("token exchange", tok.ok && !!tokens.access_token, tok.ok ? `refresh: ${!!tokens.refresh_token}` : tokens);
const claims = JSON.parse(Buffer.from(tokens.access_token.split(".")[1], "base64url").toString());
step("token carries the workspace", typeof claims.workspace === "string", { aud: claims.aud, workspace: claims.workspace, scope: claims.scope });

// 6. the API
const auth = { headers: { authorization: `Bearer ${tokens.access_token}` } };
const me = await fetch(`${API}/v1/me`, auth);
step("GET /v1/me", me.ok, await me.json());
const list = await fetch(`${API}/v1/meetings`, auth);
step("GET /v1/meetings", list.ok, await list.json());
const none = await fetch(`${API}/v1/me`);
step("no token → 401", none.status === 401);
