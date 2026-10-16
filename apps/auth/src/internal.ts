// What the api asks of the identity provider, on a port of its own that only the cluster reaches (never the public
// one): upload tokens. An upload token lets one desktop app send one recording — its live transcript, transcript and
// audio — and nothing else (scope recordings:upload, claim `recording`). The api asks for it after checking that the
// caller may write that recording; here only the shared secret is checked, and the token is minted with the same keys
// as every access token, so the api verifies it like any other.
import { CLAIMS, UPLOAD_SCOPE } from "@recordem/contract";
import { Hono } from "hono";
import { timingSafeEqual } from "node:crypto";
import { API_URL, ISSUER, auth } from "./auth";

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

export const INTERNAL_PORT = Number(process.env.AUTH_INTERNAL_PORT ?? 39413);
