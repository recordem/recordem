// Webhook endpoints: where a workspace's events are POSTed. The api keeps the list; apps/worker delivers, signed with
// each endpoint's secret, retrying for 3 days before turning the endpoint off.
import type { OpenAPIHono } from "@hono/zod-openapi";
import { routes, type StreamEvent } from "@recordem/contract";
import type { z } from "@hono/zod-openapi";
import { webhooks, type Db } from "@recordem/db";
import { and, desc, eq } from "drizzle-orm";
import { HTTPException } from "hono/http-exception";
import type { Env } from "./auth";

const view = (w: typeof webhooks.$inferSelect) => ({
  id: w.id, url: w.url, events: w.events as z.infer<typeof StreamEvent>["type"][], created_at: w.createdAt.toISOString(), disabled_at: w.disabledAt?.toISOString() ?? null,
});

/** https, or http to this machine (development). Where a host points is checked again by the worker at delivery. */
function allowed(url: string) {
  const u = new URL(url);
  return u.protocol === "https:" || (u.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname));
}

export function mountWebhooks(app: OpenAPIHono<Env>, db: Db) {
  app.openapi(routes.createWebhook, async (c) => {
    const a = c.get("actor");
    const b = c.req.valid("json");
    if (!allowed(b.url)) throw new HTTPException(400, { res: Response.json({ error: "bad_request", message: "the url must be https" }, { status: 400 }) });
    const secret = `whsec_${Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url")}`;
    const [w] = await db.insert(webhooks).values({ workspaceId: a.workspace, url: b.url, events: [...new Set(b.events)], secret, createdBy: a.subject }).returning();
    return c.json({ ...view(w), secret }, 201);
  });

  app.openapi(routes.listWebhooks, async (c) => {
    const rows = await db.select().from(webhooks).where(eq(webhooks.workspaceId, c.get("actor").workspace)).orderBy(desc(webhooks.createdAt));
    return c.json({ items: rows.map(view) }, 200);
  });

  app.openapi(routes.deleteWebhook, async (c) => {
    const [w] = await db.delete(webhooks).where(and(eq(webhooks.id, c.req.valid("param").id), eq(webhooks.workspaceId, c.get("actor").workspace))).returning({ id: webhooks.id });
    if (!w) throw new HTTPException(404, { res: Response.json({ error: "not_found", message: "webhook not found" }, { status: 404 }) });
    return c.body(null, 204);
  });
}
