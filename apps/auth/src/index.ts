// The identity provider's HTTP server: better-auth under /api/auth, its discovery documents, and the few pages an
// OAuth flow needs (sign in, pick a workspace, consent). Pages are server-rendered forms: no client bundle.
import { oauthProviderAuthServerMetadata, oauthProviderOpenIdConfigMetadata } from "@better-auth/oauth-provider";
import { SCOPES } from "@recordem/contract";
import { Hono, type Context } from "hono";
import { cors } from "hono/cors";
import { AUTH_URL, auth, pool } from "./auth";
import { ensureFirstParty } from "./first-party";
import { INTERNAL_PORT, internal } from "./internal";
import { ensureMachineTables } from "./machine";
import { page } from "./page";

const app = new Hono();

// The portal exchanges codes and refreshes tokens from the browser.
const ORIGINS = (process.env.WEB_ORIGINS ?? "http://localhost:39412").split(",");
app.use("/api/auth/oauth2/*", cors({ origin: ORIGINS, allowHeaders: ["content-type", "authorization"], maxAge: 600 }));

app.on(["GET", "POST"], "/api/auth/*", (c) => auth.handler(c.req.raw));
app.get("/.well-known/openid-configuration", (c) => oauthProviderOpenIdConfigMetadata(auth)(c.req.raw));
app.get("/.well-known/oauth-authorization-server", (c) => oauthProviderAuthServerMetadata(auth)(c.req.raw));
// The same, where RFC 8414 puts them for an issuer with a path (/api/auth): MCP clients look there.
app.get("/.well-known/oauth-authorization-server/api/auth", (c) => oauthProviderAuthServerMetadata(auth)(c.req.raw));
app.get("/.well-known/openid-configuration/api/auth", (c) => oauthProviderOpenIdConfigMetadata(auth)(c.req.raw));
app.get("/healthz", (c) => c.text("ok"));

/** Calls a better-auth endpoint as the browser would (its cookies), and passes back the cookies it sets. */
async function call(c: Context, path: string, body: unknown) {
  const res = await auth.handler(new Request(`${AUTH_URL}/api/auth${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: c.req.header("cookie") ?? "", origin: AUTH_URL },
    body: JSON.stringify(body),
  }));
  for (const v of res.headers.getSetCookie()) c.header("set-cookie", v, { append: true });
  const data = (await res.json().catch(() => ({}))) as Record<string, any>;
  return { ok: res.ok, data };
}

/** Where a flow goes next: the URL better-auth answered with, or the page itself again with an error. */
const next = (c: Context, r: { ok: boolean; data: Record<string, any> }, back: string) =>
  r.ok && typeof r.data.url === "string" ? c.redirect(r.data.url)
  : r.ok ? c.redirect(back)
  : c.redirect(`${back}${back.includes("?") ? "&" : "?"}error=${encodeURIComponent(r.data.message ?? r.data.error_description ?? "failed")}`);

const query = (c: Context) => new URL(c.req.url).search.slice(1);

app.get("/sign-in", (c) => {
  const q = query(c);
  return c.html(page("Sign in", `
    <form method="post" action="/sign-in?${q}">
      <label>Email <input name="email" type="email" required autofocus></label>
      <label>Password <input name="password" type="password" required minlength="8"></label>
      <label class="row"><input name="create" type="checkbox"> Create an account</label>
      <label>Name <input name="name" placeholder="only when creating"></label>
      <button>Continue</button>
    </form>`, c.req.query("error")));
});

app.post("/sign-in", async (c) => {
  const f = await c.req.parseBody();
  const oauth_query = query(c) || undefined;
  const r = f.create
    ? await call(c, "/sign-up/email", { email: f.email, password: f.password, name: f.name || String(f.email).split("@")[0], oauth_query })
    : await call(c, "/sign-in/email", { email: f.email, password: f.password, oauth_query });
  return next(c, r, `/sign-in?${query(c)}`);
});

app.get("/select-workspace", async (c) => {
  const orgs = (await auth.api.listOrganizations({ headers: c.req.raw.headers })) ?? [];
  return c.html(page("Pick a workspace", `
    <form method="post" action="/select-workspace?${query(c)}">
      ${orgs.map((o: { id: string; name: string }, i: number) => `<label class="row"><input type="radio" name="workspace" value="${o.id}" ${i ? "" : "checked"}> ${esc(o.name)}</label>`).join("")}
      <button>Continue</button>
    </form>`, c.req.query("error")));
});

app.post("/select-workspace", async (c) => {
  const f = await c.req.parseBody();
  const set = await call(c, "/organization/set-active", { organizationId: f.workspace });
  if (!set.ok) return next(c, set, `/select-workspace?${query(c)}`);
  return next(c, await call(c, "/oauth2/continue", { postLogin: true, oauth_query: query(c) }), `/select-workspace?${query(c)}`);
});

/** What each scope lets an app do, in its user's words. `openid` and `profile` read as one line. */
const CONSENT: Record<string, string> = {
  profile: "Know your name and email",
  "meetings:read": "See your meetings and recordings",
  "meetings:write": "Rename, edit and record meetings",
  "transcripts:read": "Read transcripts and notes",
  "audio:read": "Download the audio of recordings",
  "calendars:read": "See your connected calendars",
  "claims:write": "Link meetings to its own records",
  "webhooks:manage": "Get notified when meetings change",
  "credentials:manage": "Create and revoke the workspace's keys",
  offline_access: "Stay signed in until you sign out",
};

app.get("/consent", async (c) => {
  const asked = new Set((c.req.query("scope") ?? "").split(" ").filter(Boolean));
  if (asked.has("openid")) asked.add("profile");
  const lines = Object.keys(CONSENT).filter((s) => asked.delete(s)).map((s) => CONSENT[s]!);
  lines.push(...[...asked].filter((s) => s !== "openid"));   // a scope this page has no words for: as it is
  // The name the client registered with (e.g. "Recordem CLI"), not its id; who is signing in, and to which workspace.
  const id = c.req.query("client_id");
  const name = id && (await pool.query<{ name: string | null }>(`SELECT name FROM "oauthClient" WHERE "clientId" = $1`, [id])).rows[0]?.name;
  const s = await auth.api.getSession({ headers: c.req.raw.headers });
  const ws = s?.session.activeOrganizationId
    ? ((await auth.api.listOrganizations({ headers: c.req.raw.headers })) ?? []).find((o: { id: string }) => o.id === s.session.activeOrganizationId)?.name
    : undefined;
  return c.html(page("Allow access", `
    <p class="lead"><b>${esc(name || "An app")}</b> wants to use your Recordem account${ws ? ` in <b>${esc(ws)}</b>` : ""}.</p>
    ${s ? `<p class="muted small">Signed in as ${esc(s.user.email)}</p>` : ""}
    <p class="muted">It will be able to:</p>
    <ul class="perms">${lines.map((l) => `<li>${esc(l)}</li>`).join("")}</ul>
    <form method="post" action="/consent?${query(c)}" class="buttons">
      <button name="accept" value="no" class="ghost">Deny</button><button name="accept" value="yes">Allow</button>
    </form>
    <p class="muted small">Only allow apps you trust.</p>`, c.req.query("error")));
});

app.post("/consent", async (c) => {
  const f = await c.req.parseBody();
  return next(c, await call(c, "/oauth2/consent", { accept: f.accept === "yes", oauth_query: query(c) }), `/consent?${query(c)}`);
});

const esc = (s: string) => s.replace(/[&<>"]/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[ch]!);

await ensureMachineTables();
await ensureFirstParty(pool, { id: "recordem-web", name: "Recordem", type: "web", redirectUris: ORIGINS.map((o) => `${o}/callback`) });
await ensureFirstParty(pool, { id: "recordem-recorder", name: "Recordem recorder", type: "native", redirectUris: ["recordem://oauth", "recordem-dev://oauth"] });

// The api's own door (upload tokens): another port, so the public route to this server never reaches it.
Bun.serve({ port: INTERNAL_PORT, hostname: process.env.AUTH_INTERNAL_HOST ?? "127.0.0.1", fetch: internal.fetch });

const port = Number(process.env.AUTH_PORT ?? 39410);
console.log(`auth on :${port} (issuer ${AUTH_URL}), internal on :${INTERNAL_PORT}`);
export default { port, hostname: "0.0.0.0", fetch: app.fetch };
