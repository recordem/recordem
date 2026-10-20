// Machine credentials (API keys, client_credentials clients): kept by the identity provider, managed here so every
// client goes through the contract. Only a person (not a machine, not an upload token) may ask; the identity
// provider then checks they are an owner or admin of the workspace.
import type { OpenAPIHono } from "@hono/zod-openapi";
import { routes } from "@recordem/contract";
import type { Context } from "hono";
import { HTTPException } from "hono/http-exception";
import { forgetKeys, type Env } from "./auth";

const URL_ = process.env.AUTH_INTERNAL_URL ?? "http://localhost:39413";
const SECRET = process.env.INTERNAL_SECRET ?? "";

const fail = (status: 403 | 404, error: string, message: string) => new HTTPException(status, { res: Response.json({ error, message }, { status }) });

/** Calls the identity provider's admin door as the person behind the token; its errors become ours. */
async function idp(c: Context<Env>, method: string, path: string, body?: unknown) {
  const a = c.get("actor");
  if (a.machine) throw fail(403, "forbidden", "machine credentials can't manage credentials: sign in as a person");
  const res = await fetch(`${URL_}/internal/admin/${path}`, {
    method,
    headers: { "content-type": "application/json", authorization: `Bearer ${SECRET}`, "x-workspace": a.workspace, "x-user": a.subject },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 403) throw fail(403, "forbidden", "only an owner or admin of the workspace manages its credentials");
  if (res.status === 404) throw fail(404, "not_found", "not found in this workspace");
  if (!res.ok) throw new Error(`idp ${method} ${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? null : res.json();
}

export function mountCredentials(app: OpenAPIHono<Env>) {
  app.openapi(routes.listApiKeys, async (c) => c.json(await idp(c, "GET", "keys"), 200));
  app.openapi(routes.createApiKey, async (c) => c.json(await idp(c, "POST", "keys", c.req.valid("json")), 201));
  app.openapi(routes.revokeApiKey, async (c) => {
    await idp(c, "DELETE", `keys/${encodeURIComponent(c.req.valid("param").id)}`);
    forgetKeys();
    return c.body(null, 204);
  });
  app.openapi(routes.listMachineClients, async (c) => c.json(await idp(c, "GET", "clients"), 200));
  app.openapi(routes.createMachineClient, async (c) => c.json(await idp(c, "POST", "clients", c.req.valid("json")), 201));
  app.openapi(routes.deleteMachineClient, async (c) => {
    await idp(c, "DELETE", `clients/${encodeURIComponent(c.req.valid("param").id)}`);
    return c.body(null, 204);
  });
}
