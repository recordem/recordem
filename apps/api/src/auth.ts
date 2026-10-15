// Access tokens: JWTs issued by the identity provider, verified here against its JWKS. Any OAuth 2.1 server that
// publishes discovery metadata and puts the workspace in a claim would do; nothing here is better-auth specific.
import { CLAIMS } from "@recordem/contract";
import type { MiddlewareHandler } from "hono";
import { HTTPException } from "hono/http-exception";
import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";

export type Actor = { subject: string; workspace: string; scopes: Set<string>; client: string | null };
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

export function authenticate(opts: { authUrl: string; audience: string }): MiddlewareHandler<Env> {
  return async (c, next) => {
    const token = c.req.header("authorization")?.match(/^Bearer (.+)$/i)?.[1];
    if (!token) throw fail(401, "invalid_token", "a Bearer access token is required");
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
    });
    await next();
  };
}

/** Fails unless the token holds every scope the route declares in the contract (`x-scopes`). */
export function requireScopes(scopes: readonly string[]): MiddlewareHandler<Env> {
  return async (c, next) => {
    const missing = scopes.filter((s) => !c.get("actor").scopes.has(s));
    if (missing.length) throw fail(403, "insufficient_scope", `missing scope: ${missing.join(" ")}`);
    await next();
  };
}
