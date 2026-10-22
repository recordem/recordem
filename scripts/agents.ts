// End-to-end check of the agent surface: search (notes, transcript lines, titles; isolated per workspace), the CLI
// (signing in through its loopback redirect, then reading) and the MCP server (its OAuth challenge, then tools).
//   bun scripts/agents.ts           (auth, api and mcp running; reads AUTH_URL / API_URL / MCP_URL / DATABASE_URL)
// Notes are written by apps/worker with a language model; here they are put in the database directly.
import { connect, notes } from "../packages/db/src/index";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const AUTH = process.env.AUTH_URL ?? "http://localhost:39410";
const API = process.env.API_URL ?? "http://localhost:39411";
const MCP = process.env.MCP_URL ?? "http://localhost:39414";
const REDIRECT = "http://127.0.0.1:39497/callback";
// The published build, run by Node: what `npm i -g @recordem/cli` installs.
const CLI = join(import.meta.dir, "../packages/cli/dist/recordem.js");
await Bun.$`bun run --cwd ${join(import.meta.dir, "../packages/cli")} build`.quiet();

const step = (name: string, ok: boolean, detail?: unknown) => { console.log(`${ok ? "✓" : "✗"} ${name}`, ok ? "" : detail ?? ""); if (!ok) process.exit(1); };
const form = (o: Record<string, string>) => ({ method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(o) });
const b64url = (b: ArrayBuffer | Uint8Array) => Buffer.from(b as ArrayBuffer).toString("base64url");

/** A browser: a cookie jar, following the identity provider's pages by hand. */
function browser() {
  const jar = new Map<string, string>();
  const go = async (url: string, init: RequestInit = {}) => {
    const res = await fetch(new URL(url, AUTH), { ...init, redirect: "manual", headers: { ...init.headers, cookie: [...jar].map(([k, v]) => `${k}=${v}`).join("; ") } });
    for (const c of res.headers.getSetCookie()) { const [kv] = c.split(";"); const i = kv.indexOf("="); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
    return res;
  };
  /** From an authorize URL to where the identity provider sends the browser back, signing up on the way if asked. */
  const authorize = async (url: string, email: string) => {
    let loc = url;
    for (let i = 0; i < 10; i++) {
      const page = new URL(loc, AUTH);
      if (!page.href.startsWith(AUTH)) return page;
      const res = page.pathname === "/sign-in" ? await go(page.href, form({ email, password: "agents-password-1", create: "on", name: "Ana Souza" }))
        : page.pathname === "/consent" ? await go(page.href, form({ accept: "yes" }))
        : await go(page.href);
      loc = res.headers.get("location") ?? "";
      if (!loc) throw new Error(`stuck at ${page.pathname}: ${res.status}`);
    }
    throw new Error("too many redirects");
  };
  return { authorize };
}

/** A token for `resource` with `scope`, through a client of the test's own. */
async function token(b: ReturnType<typeof browser>, email: string, scope: string, resource: string) {
  const reg = await (await fetch(`${AUTH}/api/auth/oauth2/register`, { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ client_name: "agents test", redirect_uris: [REDIRECT], application_type: "native", token_endpoint_auth_method: "none", grant_types: ["authorization_code"] }) })).json() as { client_id: string };
  const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
  const challenge = b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)));
  const back = await b.authorize(`${AUTH}/api/auth/oauth2/authorize?${new URLSearchParams({ response_type: "code", client_id: reg.client_id, redirect_uri: REDIRECT,
    state: "s", scope, code_challenge: challenge, code_challenge_method: "S256", resource })}`, email);
  const t = await (await fetch(`${AUTH}/api/auth/oauth2/token`, form({ grant_type: "authorization_code", code: back.searchParams.get("code") ?? "", redirect_uri: REDIRECT,
    client_id: reg.client_id, code_verifier: verifier, resource }))).json() as { access_token: string };
  return t.access_token;
}

const home = mkdtempSync(join(tmpdir(), "recordem-cli-"));
const cli = (...args: string[]) => Bun.spawn(["node", CLI, ...args], { env: { ...process.env, XDG_CONFIG_HOME: home, RECORDEM_URL: API, RECORDEM_TOKEN: "" }, stdout: "pipe", stderr: "pipe", stdin: "ignore" });
const run = async (...args: string[]) => { const p = cli(...args); const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]); return { out, err, code }; };

// 1. the CLI signs in: it prints the URL, the browser goes through sign-up and consent and lands on its loopback port
const ana = browser();
const email = `agents+${Date.now()}@recordem.dev`;
const login = cli("login");
let printed = "";
const errs = login.stderr.pipeThrough(new TextDecoderStream()).getReader();
while (!printed.includes("/oauth2/authorize")) { const { value, done } = await errs.read(); if (done) break; printed += value; }
const url = printed.match(/https?:\/\/\S+\/oauth2\/authorize\S+/)?.[0] ?? "";
step("cli login prints the sign-in URL", !!url, printed);
const landed = await ana.authorize(url, email);
step("the browser is sent back to the CLI's loopback port", landed.hostname === "127.0.0.1" && !!landed.searchParams.get("code"), landed.href);
await fetch(landed);
step("cli login finishes", (await login.exited) === 0);
let r = await run("whoami", "--json");
const me = JSON.parse(r.out || "{}") as { workspace?: string; scopes?: string[] };
step("cli whoami: the CLI's scopes, no credentials", r.code === 0 && !!me.workspace && me.scopes?.includes("meetings:write") && !me.scopes?.includes("credentials:manage"), r);

// 2. a recording with a transcript and notes (written as the recorder and the worker would)
const write = await token(ana, email, "openid meetings:read meetings:write transcripts:read", API);
const call = (method: string, path: string, body?: unknown, t = write) => fetch(`${API}${path}`, {
  method, headers: { authorization: `Bearer ${t}`, ...(body === undefined ? {} : { "content-type": "application/json" }) }, body: body === undefined ? undefined : JSON.stringify(body) });
const recId = crypto.randomUUID();
const started = new Date(Date.now() - 3600_000).toISOString();
const rec = await (await call("POST", "/v1/recordings", { id: recId, started_at: started, title: "Quarterly roadmap review" })).json() as { meeting_id: string };
const seg = (s: number, speaker: string, text: string) => ({ start_ms: s * 1000, end_ms: s * 1000 + 4000, channel: speaker === "mic" ? "mic" : "system", speaker, text });
const res = await call("PUT", `/v1/recordings/${recId}/transcript`, { ended_at: new Date().toISOString(), language: "en", segments: [
  seg(1, "system:1", "We moved the embeddings from OpenAI to Bedrock last week."),
  seg(65, "mic", "Then the indexing pipeline can run locally."),
  seg(130, "system:1", "O orçamento do trimestre fecha na sexta."),
], speakers: [{ label: "system:1", name: "Dave Newman", source: "platform", confidence: 0.9 }, { label: "mic", name: "Ana Souza", source: "platform" }] });
step("a final transcript", res.ok);
const db = connect(process.env.DATABASE_URL ?? "postgres://recordem:recordem@localhost:39420/recordem");
await db.insert(notes).values({ recordingId: recId, workspaceId: me.workspace!, model: "test", language: "en",
  summary: "The team reviewed the roadmap: embeddings now run on Bedrock.",
  actionItems: [{ text: "Publish the indexing pipeline PRs", owner: "Dave Newman" }],
  topics: [{ title: "Search infrastructure", points: ["OpenSearch talks to Bedrock for text embeddings"] }] });
const liveId = crypto.randomUUID();
await call("POST", "/v1/recordings", { id: liveId, meeting_id: rec.meeting_id, started_at: new Date().toISOString() });

// 3. search
const search = async (q: string, t = write, extra = "") => (await (await call("GET", `/v1/search?q=${encodeURIComponent(q)}${extra}`, undefined, t)).json()) as {
  items: { field: string; recording_id: string | null; meeting: { id: string }; segment: { speaker_name: string | null; start_ms: number } | null; notes: { part: string; text: string } | null }[]; next: string | null };
let s = await search("bedrock");
step("search: notes before transcript lines", s.items[0]?.field === "notes" && s.items.some((h) => h.field === "transcript" && h.segment?.speaker_name === "Dave Newman"), s.items);
s = await search("pipeline");
step("search: an action item, with its owner", s.items.some((h) => h.notes?.part === "action_item" && h.notes.text.includes("(Dave Newman)")), s.items);
step("search: the mic line, named", s.items.some((h) => h.field === "transcript" && h.segment?.speaker_name === "Ana Souza" && h.segment.start_ms === 65_000), s.items);
s = await search("roadmap");
step("search: the meeting title first", s.items[0]?.field === "title" && s.items[0].meeting.id === rec.meeting_id, s.items);
s = await search("orçamento");
step("search: accents and Portuguese", s.items.some((h) => h.field === "transcript"), s.items);
s = await search("bedrock", write, `&meeting_id=${crypto.randomUUID()}`);
step("search: within another meeting, nothing", s.items.length === 0);
s = await search("bedrock -embeddings");
step("search: -word excludes", s.items.length === 0, s.items);
s = await search("bedrock", write, "&limit=1");
step("search: pages", s.items.length === 1 && s.next === "1");

const bruno = browser();
const other = await token(bruno, `other+${Date.now()}@recordem.dev`, "openid meetings:read transcripts:read", API);
s = await search("bedrock", other);
step("search: another workspace finds nothing", s.items.length === 0, s.items);

// 4. the CLI reads it
r = await run("search", "bedrock");
step("cli search", r.code === 0 && r.out.includes("notes, summary") && r.out.includes("Dave Newman: We moved the embeddings"), r);
r = await run("meeting", rec.meeting_id);
step("cli meeting: notes, and why the other recording has none", r.code === 0 && r.out.includes("Publish the indexing pipeline PRs (Dave Newman)") && r.out.includes("no notes yet: still recording"), r.out);
r = await run("transcript", recId);
step("cli transcript: [m:ss] Name: text", r.code === 0 && r.out.includes("[1:05] Ana Souza: Then the indexing pipeline") && r.out.includes("[2:10] Dave Newman: O orçamento"), r.out);
r = await run("transcript", recId, "--since", "1:00", "--until", "2:00");
step("cli transcript: a time range", r.out.includes("[1:05]") && !r.out.includes("[0:01]") && !r.out.includes("[2:10]"), r.out);
r = await run("notes", recId, "--json");
step("cli notes --json", r.code === 0 && JSON.parse(r.out).summary.includes("Bedrock"));
r = await run("notes", liveId);
step("cli notes: none yet is exit 4, with why", r.code === 4 && r.err.includes("still recording"), r);
r = await run("meeting", crypto.randomUUID());
step("cli: not found is exit 4", r.code === 4);

// 5. the MCP server
let m = await fetch(`${MCP}/mcp`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
const challenge = m.headers.get("www-authenticate") ?? "";
step("mcp: no token → 401 pointing at its metadata", m.status === 401 && challenge.includes("resource_metadata="), challenge);
const meta = await (await fetch(challenge.match(/resource_metadata="([^"]+)"/)![1])).json() as { resource: string; authorization_servers: string[] };
step("mcp: protected resource metadata names the identity provider", meta.authorization_servers[0]?.startsWith(AUTH), meta);
const mcpToken = await token(ana, email, "openid meetings:read transcripts:read", meta.resource);
const rpc = async (method: string, params: unknown, t = mcpToken) => {
  const res = await fetch(`${MCP}/mcp`, { method: "POST", headers: { authorization: `Bearer ${t}`, "content-type": "application/json", accept: "application/json, text/event-stream" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  return { status: res.status, body: await res.json().catch(() => null) as { result?: { tools?: { name: string }[]; content?: { text: string }[]; instructions?: string } } | null };
};
let x = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "agents test", version: "0" } });
step("mcp: initialize, with instructions", x.status === 200 && !!x.body?.result?.instructions?.includes("Read notes first"), x);
x = await rpc("tools/list", {});
step("mcp: the tools", ["search_transcripts", "list_meetings", "get_meeting", "get_transcript", "get_notes"].every((n) => x.body?.result?.tools?.some((t) => t.name === n)), x.body);
const tool = async (name: string, args: unknown) => (await rpc("tools/call", { name, arguments: args })).body?.result?.content?.[0]?.text ?? "";
let out = await tool("search_transcripts", { query: "bedrock" });
step("mcp: search_transcripts", out.includes("notes, summary") && out.includes(`recording_id=${recId}`), out);
out = await tool("get_meeting", { meeting_id: rec.meeting_id });
step("mcp: get_meeting with notes", out.includes("Publish the indexing pipeline PRs") && out.includes("no notes yet: still recording"), out);
out = await tool("get_transcript", { recording_id: recId, start: 60, max_lines: 1 });
step("mcp: get_transcript pages", out.includes("[1:05] Ana Souza") && !out.includes("[2:10]") && out.includes("next start: 69"), out);
out = await tool("get_notes", { recording_id: liveId });
step("mcp: get_notes says why there are none", out.includes("still recording"), out);
x = await rpc("tools/list", {}, write);
step("mcp: a token for the API alone is refused", x.status === 401, x.status);

// cleanup
await call("DELETE", `/v1/meetings/${rec.meeting_id}`);
await db.$client.end();
rmSync(home, { recursive: true, force: true });
console.log("agents: all good");
