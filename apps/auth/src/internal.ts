// What the api asks of the identity provider, on a port of its own that only the cluster reaches (never the public
// one): upload tokens. An upload token lets one desktop app send one recording — its live transcript, transcript and
// audio — and nothing else (scope recordings:upload, claim `recording`). The api asks for it after checking that the
// caller may write that recording; here only the shared secret is checked, and the token is minted with the same keys
// as every access token, so the api verifies it like any other. And machine credentials: what an API key is, and
// managing keys and client_credentials clients for a workspace's admins (machine.ts).
import { CLAIMS, UPLOAD_SCOPE } from "@recordem/contract";
import { Hono } from "hono";
import { timingSafeEqual } from "node:crypto";
import { API_URL, AUTH_URL, ISSUER, auth } from "./auth";
import { createClient, createKey, deleteClient, isAdmin, listClients, listKeys, machineScopes, revokeKey, verifyKey } from "./machine";

const SECRET = process.env.INTERNAL_SECRET ?? "";
const UPLOAD_TTL_S = 6 * 3600;

const same = (a: string, b: string) => a.length === b.length && timingSafeEqual(Buffer.from(a), Buffer.from(b));

export const internal = new Hono();

internal.use("*", async (c, next) => {
  const token = c.req.header("authorization")?.match(/^Bearer (.+)$/i)?.[1] ?? "";
  if (!SECRET || !same(token, SECRET)) return c.json({ error: "unauthorized" }, 401);
  await next();
});

internal.post("/internal/upload-token", async (c) => {
  const b = (await c.req.json().catch(() => null)) as { sub?: unknown; workspace?: unknown; recording?: unknown; client?: unknown } | null;
  const str = (v: unknown) => (typeof v === "string" && v.length > 0 && v.length <= 200 ? v : null);
  const [sub, workspace, recording] = [str(b?.sub), str(b?.workspace), str(b?.recording)];
  if (!sub || !workspace || !recording) return c.json({ error: "bad_request", message: "sub, workspace and recording are required" }, 400);
  const exp = Math.floor(Date.now() / 1000) + UPLOAD_TTL_S;
  const { token } = await auth.api.signJWT({
    body: {
      payload: { sub, aud: API_URL, scope: UPLOAD_SCOPE, [CLAIMS.workspace]: workspace, [CLAIMS.recording]: recording, ...(str(b?.client) ? { azp: b!.client } : {}) },
      overrideOptions: { jwt: { issuer: ISSUER, audience: API_URL, expirationTime: exp } },
    },
  });
  return c.json({ token, expires_at: new Date(exp * 1000).toISOString() });
});

// Machine credentials (machine.ts). The api has already checked the caller holds credentials:manage; here, that they
// are a person who is an owner or admin of the workspace.
const str = (v: unknown, max = 200) => (typeof v === "string" && v.length > 0 && v.length <= max ? v : null);
const bad = (message: string) => Response.json({ error: "bad_request", message }, { status: 400 });
/** Where clients reach the token endpoint: the public address of better-auth's routes. */
const ISSUER_URL = `${AUTH_URL}/api/auth`;

internal.post("/internal/keys/verify", async (c) => {
  const b = (await c.req.json().catch(() => null)) as { key?: unknown } | null;
  const k = str(b?.key, 400) ? await verifyKey(b!.key as string) : null;
  return k ? c.json(k) : c.json({ error: "invalid_key" }, 404);
});

/** Every call below names the workspace and the person acting; it fails unless they administer the workspace. */
internal.use("/internal/admin/*", async (c, next) => {
  const ws = c.req.header("x-workspace") ?? "";
  const user = c.req.header("x-user") ?? "";
  if (!ws || !user || !(await isAdmin(user, ws))) return c.json({ error: "forbidden", message: "only an owner or admin of the workspace" }, 403);
  await next();
});

const admin = (c: { req: { header: (k: string) => string | undefined } }) => ({ ws: c.req.header("x-workspace")!, user: c.req.header("x-user")! });

internal.get("/internal/admin/keys", async (c) => c.json({ items: await listKeys(admin(c).ws) }));
internal.post("/internal/admin/keys", async (c) => {
  const b = (await c.req.json().catch(() => null)) as { name?: unknown; scopes?: unknown } | null;
  const name = str(b?.name, 100), scopes = machineScopes(b?.scopes);
  if (!name || !scopes) return bad("a name and machine scopes are required");
  return c.json(await createKey(admin(c).ws, name, scopes, admin(c).user), 201);
});
internal.delete("/internal/admin/keys/:id", async (c) => (await revokeKey(admin(c).ws, c.req.param("id")) ? c.body(null, 204) : c.json({ error: "not_found" }, 404)));

internal.get("/internal/admin/clients", async (c) => c.json({ items: await listClients(admin(c).ws) }));
internal.post("/internal/admin/clients", async (c) => {
  const b = (await c.req.json().catch(() => null)) as { name?: unknown; scopes?: unknown } | null;
  const name = str(b?.name, 100), scopes = machineScopes(b?.scopes);
  if (!name || !scopes) return bad("a name and machine scopes are required");
  return c.json({ ...(await createClient(admin(c).ws, name, scopes, admin(c).user)), token_endpoint: `${ISSUER_URL}/oauth2/token` }, 201);
});
internal.delete("/internal/admin/clients/:id", async (c) => (await deleteClient(admin(c).ws, c.req.param("id")) ? c.body(null, 204) : c.json({ error: "not_found" }, 404)));

export const INTERNAL_PORT = Number(process.env.AUTH_INTERNAL_PORT ?? 39413);
