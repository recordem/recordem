// The resource server. Every route comes from @recordem/contract; this file only implements them.
import { OpenAPIHono } from "@hono/zod-openapi";
import { document, routes } from "@recordem/contract";
import { connect, meetings } from "@recordem/db";
import { and, desc, eq, gte, lt, or } from "drizzle-orm";
import { cors } from "hono/cors";
import { HTTPException } from "hono/http-exception";
import { authenticate, requireScopes, type Env } from "./auth";
import { serveProfiles } from "./profiles";
import { mount } from "./recordings";
import { mountSearch } from "./search";
import { mountWebhooks } from "./webhooks";
import { files } from "./storage";
import { meetingView } from "./views";

const env = (k: string, d?: string) => process.env[k] ?? d ?? (() => { throw new Error(`${k} is not set`); })();
const AUTH_URL = env("AUTH_URL", "http://localhost:39410");
const API_URL = env("API_URL", "http://localhost:39411");
const db = connect(env("DATABASE_URL"));
// Browser clients (the portal) call the API directly with a Bearer token.
const ORIGINS = env("WEB_ORIGINS", "http://localhost:39412").split(",");

const app = new OpenAPIHono<Env>({
  defaultHook: (r, c) => (r.success ? undefined : c.json({ error: "invalid_request", message: r.error.message }, 400)),
});

app.use("*", cors({ origin: ORIGINS, allowHeaders: ["authorization", "content-type"], exposeHeaders: ["www-authenticate"], maxAge: 600 }));
app.get("/healthz", (c) => c.text("ok"));
app.get("/files/*", (c) => files(c.req.raw));
app.get("/v1/openapi.json", (c) => c.json(document(AUTH_URL, API_URL)));
app.get(routes.readerProfiles.path, (c) => serveProfiles(c.req.raw));
// Public routes (the contract marks them `x-public`) take no token; every other /v1 route does.
const PUBLIC = new Set(["/v1/openapi.json", ...Object.values(routes).filter((r) => "x-public" in r).map((r) => r.path)]);
app.use("/v1/*", async (c, next) => (PUBLIC.has(c.req.path) ? next() : authenticate({ authUrl: AUTH_URL, audience: API_URL })(c, next)));
// Each route requires the scopes the contract declares for it (per method: GET and PUT of a track differ).
for (const r of Object.values(routes).filter((r) => !("x-public" in r))) app.on(r.method.toUpperCase(), r.path.replace(/{(\w+)}/g, ":$1"), requireScopes(r["x-scopes"], r["x-upload"]));

app.openapi(routes.getMe, (c) => {
  const a = c.get("actor");
  return c.json({ subject: a.subject, workspace: a.workspace, scopes: [...a.scopes], client: a.client }, 200);
});

// Cursor: "<starts_at ISO>|<id>" of the last item, matching the (workspace, starts_at desc, id desc) index.
app.openapi(routes.listMeetings, async (c) => {
  const { limit, cursor, from, to } = c.req.valid("query");
  const ws = c.get("actor").workspace;
  const [at, id] = cursor ? cursor.split("|") : [];
  const rows = await db.select().from(meetings)
    .where(and(eq(meetings.workspaceId, ws),
      from ? gte(meetings.startsAt, new Date(from)) : undefined,
      to ? lt(meetings.startsAt, new Date(to)) : undefined,
      at && id ? or(lt(meetings.startsAt, new Date(at)), and(eq(meetings.startsAt, new Date(at)), lt(meetings.id, id))) : undefined))
    .orderBy(desc(meetings.startsAt), desc(meetings.id)).limit(limit + 1);
  const page = rows.slice(0, limit);
  const last = page.at(-1);
  return c.json({
    items: page.map(meetingView),
    next: rows.length > limit && last ? `${last.startsAt.toISOString()}|${last.id}` : null,
  }, 200);
});

mount(app, db);
mountSearch(app, db);
mountWebhooks(app, db);

app.onError((e, c) => (e instanceof HTTPException ? e.getResponse() : (console.error(e), c.json({ error: "server_error", message: "unexpected error" }, 500))));

const port = Number(process.env.API_PORT ?? 39411);
console.log(`api on :${port} (auth ${AUTH_URL}, audience ${API_URL})`);
// Audio uploads: an hour of the 48 kHz mic track is ~45 MB.
// SSE streams sit quiet between pings (25 s): Bun closes idle connections after 10 s unless told otherwise.
export default { port, hostname: "0.0.0.0", fetch: app.fetch, maxRequestBodySize: 1024 ** 3, idleTimeout: 60 };
