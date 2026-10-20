// Access tokens: JWTs issued by the identity provider, verified here against its JWKS. Any OAuth 2.1 server that
// publishes discovery metadata and puts the workspace in a claim would do; nothing here is better-auth specific.
// API keys (`rk_live_…`) are not JWTs: the identity provider says what one is (its internal port), kept here a minute.
import { API_KEY_PREFIX, CLAIMS, UPLOAD_SCOPE } from "@recordem/contract";
import { createHash } from "node:crypto";
import type { MiddlewareHandler } from "hono";
import { HTTPException } from "hono/http-exception";
import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";

/** `recording`: an upload token's one recording (createRecording `upload`); null for every other token. `machine`: an
 * API key or a client_credentials client, a backend rather than a person. */
export type Actor = { subject: string; workspace: string; scopes: Set<string>; client: string | null; recording: string | null; machine: boolean };
export type Env = { Variables: { actor: Actor } };

type Verifier = { issuer: string; jwks: ReturnType<typeof createRemoteJWKSet> };
let verifier: Promise<Verifier> | null = null;

/** The issuer and its keys, from the provider's discovery document (fetched once; jose caches and rotates keys). */
function discover(authUrl: string): Promise<Verifier> {
  verifier ??= fetch(`${authUrl}/.well-known/oauth-authorization-server`)
    .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`discovery: ${r.status}`))))
    .then((m: { issuer: string; jwks_uri: string }) => ({ issuer: m.issuer, jwks: createRemoteJWKSet(new URL(m.jwks_uri)) }))
    .catch((e) => { verifier = null; throw e; });
  return verifier;
}

const fail = (status: 401 | 403, error: string, message: string) =>
  new HTTPException(status, { res: Response.json({ error, message }, { status, headers: status === 401 ? { "www-authenticate": `Bearer error="${error}"` } : {} }) });

const INTERNAL_URL = process.env.AUTH_INTERNAL_URL ?? "http://localhost:39413";
const INTERNAL_SECRET = process.env.INTERNAL_SECRET ?? "";
const KEY_TTL_MS = 60_000;
/** Keys seen in the last minute, by their SHA-256: what they are, or null for one that isn't a live key. */
const keys = new Map<string, { at: number; key: { id: string; workspace: string; scopes: string[] } | null }>();

async function apiKey(token: string): Promise<Actor> {
  const h = createHash("sha256").update(token).digest("hex");
  let hit = keys.get(h);
  if (!hit || Date.now() - hit.at > KEY_TTL_MS) {
    const res = await fetch(`${INTERNAL_URL}/internal/keys/verify`, {
      method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${INTERNAL_SECRET}` }, body: JSON.stringify({ key: token }),
    });
    if (!res.ok && res.status !== 404) throw new Error(`api key: ${res.status}`);
    hit = { at: Date.now(), key: res.ok ? await res.json() : null };
    if (keys.size > 10_000) keys.clear();
    keys.set(h, hit);
  }
  if (!hit.key) throw fail(401, "invalid_token", "unknown or revoked API key");
  const k = hit.key;
  return { subject: `key:${k.id}`, workspace: k.workspace, scopes: new Set(k.scopes), client: `key:${k.id}`, recording: null, machine: true };
}

/** Forgets a key at once in this process (revoked here); other processes forget it within KEY_TTL_MS. */
export const forgetKeys = () => keys.clear();

export function authenticate(opts: { authUrl: string; audience: string | string[] }): MiddlewareHandler<Env> {
  return async (c, next) => {
    const token = c.req.header("authorization")?.match(/^Bearer (.+)$/i)?.[1];
    if (!token) throw fail(401, "invalid_token", "a Bearer access token is required");
    if (token.startsWith(API_KEY_PREFIX)) {
      c.set("actor", await apiKey(token));
      return next();
    }
    const { issuer, jwks } = await discover(opts.authUrl);
    let p: JWTPayload & Record<string, unknown>;
    try {
      ({ payload: p } = await jwtVerify(token, jwks, { issuer, audience: opts.audience }));
    } catch (e) {
      throw fail(401, "invalid_token", (e as Error).message);
    }
    const workspace = p[CLAIMS.workspace];
    if (typeof workspace !== "string" || !p.sub) throw fail(401, "invalid_token", "the token carries no workspace");
    c.set("actor", {
      subject: p.sub, workspace,
      scopes: new Set(typeof p.scope === "string" ? p.scope.split(" ") : []),
      client: typeof p.azp === "string" ? p.azp : typeof p.client_id === "string" ? p.client_id : null,
      recording: typeof p[CLAIMS.recording] === "string" ? (p[CLAIMS.recording] as string) : null,
      machine: p[CLAIMS.machine] === true,
    });
    await next();
  };
}

/** Fails unless the token holds every scope the route declares in the contract (`x-scopes`). An upload token holds
 * none of them: it passes only on a route marked `x-upload`, for the recording in its claim. */
export function requireScopes(scopes: readonly string[], upload = false): MiddlewareHandler<Env> {
  return async (c, next) => {
    const a = c.get("actor");
    if (a.scopes.has(UPLOAD_SCOPE)) {
      // Its own recording, on a route marked x-upload; on one with no recording in its path (getMe), only to say who it is.
      const id = c.req.param("id");
      if (!upload || !a.recording || (id !== undefined && id !== a.recording)) throw fail(403, "insufficient_scope", "an upload token only sends its own recording");
      return next();
    }
    const missing = scopes.filter((s) => !a.scopes.has(s));
    if (missing.length) throw fail(403, "insufficient_scope", `missing scope: ${missing.join(" ")}`);
    await next();
  };
}
