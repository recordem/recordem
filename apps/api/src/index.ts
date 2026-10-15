// The resource server. Every route comes from @recordem/contract; this file only implements them.
import { OpenAPIHono } from "@hono/zod-openapi";
import { document, routes } from "@recordem/contract";
import { connect, meetings } from "@recordem/db";
import { and, desc, eq, lt, or } from "drizzle-orm";
import { HTTPException } from "hono/http-exception";
import { authenticate, requireScopes, type Env } from "./auth";

const env = (k: string, d?: string) => process.env[k] ?? d ?? (() => { throw new Error(`${k} is not set`); })();
const AUTH_URL = env("AUTH_URL", "http://localhost:39410");
const API_URL = env("API_URL", "http://localhost:39411");
const db = connect(env("DATABASE_URL"));

const app = new OpenAPIHono<Env>({
  defaultHook: (r, c) => (r.success ? undefined : c.json({ error: "invalid_request", message: r.error.message }, 400)),
});

app.get("/healthz", (c) => c.text("ok"));
app.get("/v1/openapi.json", (c) => c.json(document(AUTH_URL, API_URL)));
app.use("/v1/*", async (c, next) => (c.req.path === "/v1/openapi.json" ? next() : authenticate({ authUrl: AUTH_URL, audience: API_URL })(c, next)));
for (const r of Object.values(routes)) app.use(r.path.replace(/{(\w+)}/g, ":$1"), requireScopes(r["x-scopes"]));

app.openapi(routes.getMe, (c) => {
  const a = c.get("actor");
  return c.json({ subject: a.subject, workspace: a.workspace, scopes: [...a.scopes], client: a.client }, 200);
});

// Cursor: "<starts_at ISO>|<id>" of the last item, matching the (workspace, starts_at desc, id desc) index.
app.openapi(routes.listMeetings, async (c) => {
  const { limit, cursor } = c.req.valid("query");
  const ws = c.get("actor").workspace;
  const [at, id] = cursor ? cursor.split("|") : [];
  const rows = await db.select().from(meetings)
    .where(and(eq(meetings.workspaceId, ws),
      at && id ? or(lt(meetings.startsAt, new Date(at)), and(eq(meetings.startsAt, new Date(at)), lt(meetings.id, id))) : undefined))
    .orderBy(desc(meetings.startsAt), desc(meetings.id)).limit(limit + 1);
  const page = rows.slice(0, limit);
  const last = page.at(-1);
  return c.json({
    items: page.map((m) => ({
      id: m.id, title: m.title, call_key: m.callKey, starts_at: m.startsAt.toISOString(),
      ends_at: m.endsAt?.toISOString() ?? null, created_at: m.createdAt.toISOString(),
    })),
    next: rows.length > limit && last ? `${last.startsAt.toISOString()}|${last.id}` : null,
  }, 200);
});

app.onError((e, c) => (e instanceof HTTPException ? e.getResponse() : (console.error(e), c.json({ error: "server_error", message: "unexpected error" }, 500))));

const port = Number(process.env.API_PORT ?? 39411);
console.log(`api on :${port} (auth ${AUTH_URL}, audience ${API_URL})`);
export default { port, hostname: "0.0.0.0", fetch: app.fetch };
