// End-to-end check of what a company embedding Recordem does (the Desktop SDK plan): a workspace admin creates an API
// key and a client_credentials client; the company's backend, with the key, creates a recording with an upload token;
// a desktop app sends the whole recording with that token alone (live transcript, participants, final transcript,
// audio, or a failure); the backend hears every step by webhook, signed, and reads the result back. Revoked keys,
// machines managing credentials and other workspaces get nothing.
//   bun scripts/sdk.ts            (auth, api and worker running; reads AUTH_URL / API_URL from the environment)
import { clientCredentials, createClient, RecordemError, verifyWebhook, type StreamEvent } from "../packages/sdk/src/index";

const AUTH = process.env.AUTH_URL ?? "http://localhost:39410";
const API = process.env.API_URL ?? "http://localhost:39411";
const REDIRECT = "http://127.0.0.1:39497/callback";
const HOOK_PORT = 39496;

const step = (name: string, ok: boolean, detail?: unknown) => { console.log(`${ok ? "✓" : "✗"} ${name}`, detail ?? ""); if (!ok) process.exit(1); };
const form = (o: Record<string, string>) => ({ method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(o) });
/** The API's status for a call: 200 when it went through, else the error's. */
const status = (p: Promise<unknown>) => p.then(() => 200, (e) => { if (e instanceof RecordemError) return e.status; throw e; });
const b64url = (b: ArrayBuffer | Uint8Array) => Buffer.from(b as ArrayBuffer).toString("base64url");

/** A new person signs up inside an OAuth flow and authorizes a public client: their access token, in their own
 * (personal) workspace, of which they are the owner. */
async function person(scope: string) {
  const jar = new Map<string, string>();
  const go = async (url: string, init: RequestInit = {}) => {
    const res = await fetch(url, { ...init, redirect: "manual", headers: { ...init.headers, cookie: [...jar].map(([k, v]) => `${k}=${v}`).join("; ") } });
    for (const c of res.headers.getSetCookie()) { const [kv] = c.split(";"); const i = kv.indexOf("="); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
    return res;
  };
  const reg = await (await fetch(`${AUTH}/api/auth/oauth2/register`, { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ client_name: "sdk smoke", redirect_uris: [REDIRECT], application_type: "native", token_endpoint_auth_method: "none", grant_types: ["authorization_code"] }) })).json() as { client_id: string };
  const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
  const q = new URLSearchParams({ response_type: "code", client_id: reg.client_id, redirect_uri: REDIRECT, state: "s", scope,
    code_challenge: b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))), code_challenge_method: "S256", resource: API });
  let res = await go(`${AUTH}/api/auth/oauth2/authorize?${q}`);
  res = await go(new URL(res.headers.get("location")!, AUTH).href, form({ email: `sdk+${crypto.randomUUID().slice(0, 8)}@recordem.dev`, password: "sdk-password-1", create: "on", name: "Admin" }));
  let loc = res.headers.get("location") ?? "";
  while (loc && !loc.includes("/consent") && !loc.startsWith(REDIRECT)) { res = await go(new URL(loc, AUTH).href); loc = res.headers.get("location") ?? ""; }
  if (loc.includes("/consent")) { res = await go(new URL(loc, AUTH).href.replace(/^(.*?)\/consent/, `${AUTH}/consent`), form({ accept: "yes" })); loc = res.headers.get("location") ?? ""; }
  const code = new URL(loc).searchParams.get("code")!;
  const t = await (await fetch(`${AUTH}/api/auth/oauth2/token`, form({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: reg.client_id, code_verifier: verifier, resource: API }))).json() as { access_token: string };
  return t.access_token;
}

const MACHINE = ["meetings:read", "meetings:write", "transcripts:read", "audio:read", "webhooks:manage"] as const;

// 1. a person, owner of their workspace, makes the machine credentials
const adminToken = await person("openid profile meetings:read credentials:manage");
const admin = createClient({ baseUrl: API, token: adminToken });
let me = await admin.me();
step("the admin is a person", me.machine === false, { workspace: me.workspace });
const key = await admin.apiKeys.create({ name: "acme backend", scopes: [...MACHINE] });
step("admin creates an API key (shown once)", key.key.startsWith("rk_live_") && key.prefix === key.key.slice(0, 12), { id: key.id, prefix: key.prefix });
const listed = await admin.apiKeys.list();
step("listing shows the key without its secret", listed.items.some((k) => k.id === key.id) && !JSON.stringify(listed).includes(key.key));
step("a key can't hold credentials:manage", await status(admin.apiKeys.create({ name: "x", scopes: ["credentials:manage" as never] })) === 400);

// 2. the company's backend, with the key
const backend = createClient({ baseUrl: API, token: key.key });
me = await backend.me();
step("the key is a machine of that workspace", me.machine === true && me.subject === `key:${key.id}` && me.scopes.includes("meetings:write"), me);
step("a machine can't make credentials", await status(backend.apiKeys.list()) === 403);

const got: { body: string; sig: string }[] = [];
const hooks = Bun.serve({ port: HOOK_PORT, hostname: "127.0.0.1", fetch: async (req) => { got.push({ body: await req.text(), sig: req.headers.get("recordem-signature") ?? "" }); return new Response("ok"); } });
const LIFECYCLE = ["recording.created", "recording.started", "recording.ended", "transcript.final", "track.ready", "recording.uploaded", "recording.failed", "participants.updated"] as const;
const hook = await backend.webhooks.create({ url: `http://127.0.0.1:${HOOK_PORT}/hook`, events: [...LIFECYCLE] });
step("the backend subscribes a webhook", !!hook.secret);

const recId = crypto.randomUUID();
const rec = await backend.recordings.create({
  id: recId, started_at: new Date().toISOString(), title: "Interview: Ana Souza", ref: "interview_123", metadata: { candidate: "c_77" },
  user: "recruiter@acme.com", upload: true,
});
step("the backend creates a recording with an upload token", !!rec.upload?.token && rec.user === "recruiter@acme.com", { user: rec.user, ref: rec.ref });

// 3. the desktop app, with the upload token only
const desk = createClient({ baseUrl: API, token: rec.upload!.token });
const dm = await desk.me();
step("the upload token checks itself", dm.scopes.join() === "recordings:upload", dm);
const seg = (i: number, channel: "mic" | "system", text: string) => ({ start_ms: i * 1000, end_ms: i * 1000 + 900, channel, speaker: channel === "mic" ? "mic" : "system:1", text });
const people = { t_ms: 500, visible: true, people: [{ name: "Ana Souza", speaking: true }, { name: "Rita Recruiter", speaking: false, self: true }] };
let live = await desk.recordings.appendLive(recId, { from: 0, segments: [seg(0, "system", "Oi, tudo bem?")], participants: people,
  speakers: [{ label: "system:1", name: "Ana Souza", source: "platform", confidence: 0.9 }] });
step("live transcript, participants and speakers, with the token", live.n === 1);
live = await desk.recordings.appendLive(recId, { from: 1, segments: [seg(1, "mic", "Tudo, e você?")], ended: true });
step("the last live batch ends it", live.n === 2);
await desk.recordings.putTranscript(recId, {
  ended_at: new Date().toISOString(), language: "pt", model: "parakeet-tdt-0.6b-v3", segments: [seg(0, "system", "Oi, tudo bem?"), seg(1, "mic", "Tudo, e você?")],
  speakers: [{ label: "system:1", name: "Ana Souza", source: "platform", confidence: 0.9 }] });
step("the final transcript, with the token", true);
step("not uploaded before the audio is in", (await backend.recordings.get(recId)).uploaded_at === null);
const track = await desk.recordings.putTrack(recId, "mix", new Uint8Array(256));
step("the audio, with the token", track.bytes === 256);
step("the token reads no transcript", await status(desk.recordings.getTranscript(recId)) === 403);
step("the token lists nothing", await status(desk.meetings.list()) === 403);

// 4. the backend reads it back
const done = await backend.recordings.get(recId);
step("uploaded, with who recorded it", !!done.uploaded_at && done.user === "recruiter@acme.com" && done.transcript === "final" && done.attendees.includes("Ana Souza"), { uploaded_at: done.uploaded_at, attendees: done.attendees });
const tr = await backend.recordings.getTranscript(recId);
step("the transcript, with the platform's names", tr.segments.length === 2 && tr.speakers.some((s) => s.name === "Ana Souza"));

// 5. a desktop app that gives up
const failId = crypto.randomUUID();
const failRec = await backend.recordings.create({ id: failId, started_at: new Date().toISOString(), ref: "interview_124", upload: true });
const failDesk = createClient({ baseUrl: API, token: failRec.upload!.token });
const fr = await failDesk.recordings.reportFailure(failId, { reason: "permission_denied", message: "no screen and system audio recording permission" });
step("a failure, reported with the token, is kept", fr.failed?.reason === "permission_denied", fr.failed);
step("a failure can't be reported on another recording", await status(failDesk.recordings.reportFailure(recId, { reason: "other" })) === 403);

// 6. the webhooks: every step, signed, in order, with the backend's own ids
const want = ["recording.created", "recording.started", "participants.updated", "recording.ended", "transcript.final", "track.ready", "recording.uploaded"];
const mine = () => got.map((g) => JSON.parse(g.body) as StreamEvent).filter((e) => e.recording_id === recId);
for (let i = 0; i < 100 && (mine().length < want.length || !got.some((g) => g.body.includes("recording.failed"))); i++) await Bun.sleep(100);
const verified = await Promise.all(got.map((g) => verifyWebhook(hook.secret, g.body, g.sig).then(() => true, () => false)));
step("every delivery is signed", verified.every(Boolean), `${got.length} deliveries`);
const order = mine().sort((a, b) => a.at.localeCompare(b.at)).map((e) => e.type);
step("a recording's events, in order", JSON.stringify(order) === JSON.stringify(want), order);
const up = mine().find((e) => e.type === "recording.uploaded")!;
step("recording.uploaded carries ref, metadata and user", JSON.stringify(up.data) === JSON.stringify({ ref: "interview_123", metadata: { candidate: "c_77" }, user: "recruiter@acme.com" }), up.data);
const fe = got.map((g) => JSON.parse(g.body) as StreamEvent).find((e) => e.type === "recording.failed");
step("recording.failed carries why", fe?.recording_id === failId && fe.data.reason === "permission_denied" && fe.data.ref === "interview_124", fe?.data);
step("a failure after the upload is ignored", (await desk.recordings.reportFailure(recId, { reason: "other" })).failed === null);

// 7. client_credentials: short-lived tokens for the same backend
const mc = await admin.machineClients.create({ name: "acme worker", scopes: ["meetings:read", "transcripts:read"] });
step("admin creates a client_credentials client (secret shown once)", !!mc.client_secret && !!mc.token_endpoint, { client_id: mc.client_id, token_endpoint: mc.token_endpoint });
step("listing shows it without its secret", (await admin.machineClients.list()).items.some((c) => c.client_id === mc.client_id && !("client_secret" in c)));
const cc = createClient({ baseUrl: API, token: clientCredentials({ tokenUrl: mc.token_endpoint, clientId: mc.client_id, clientSecret: mc.client_secret, resource: API }) });
const ccMe = await cc.me();
step("its token is a machine of the workspace, with its scopes", ccMe.machine === true && ccMe.workspace === me.workspace && ccMe.scopes.join(" ") === "meetings:read transcripts:read", ccMe);
step("it reads the transcript", await status(cc.recordings.getTranscript(recId)) === 200);
step("it writes nothing it wasn't given", await status(cc.recordings.create({ id: crypto.randomUUID(), started_at: new Date().toISOString() })) === 403);
const wrong = await fetch(mc.token_endpoint, form({ grant_type: "client_credentials", client_id: mc.client_id, client_secret: "nope", resource: API }));
step("a wrong secret gets no token", wrong.status === 401 || wrong.status === 400);
const pub = await fetch(mc.token_endpoint, form({ grant_type: "client_credentials", client_id: mc.client_id, client_secret: mc.client_secret, resource: API, scope: "meetings:write" }));
step("it can't ask for more scopes", !pub.ok);

// 8. another workspace sees none of it
const otherToken = await person("openid profile meetings:read meetings:write credentials:manage");
const other = createClient({ baseUrl: API, token: otherToken });
step("other workspace: the recording is 404", await status(other.recordings.get(recId)) === 404);
step("other workspace: no keys of ours", (await other.apiKeys.list()).items.length === 0);
step("other workspace: can't revoke our key", await status(other.apiKeys.revoke(key.id)) === 404);
step("other workspace: can't delete our client", await status(other.machineClients.delete(mc.client_id)) === 404);

// 9. the backend cleans up (only who recorded deletes), then the admin revokes its key
for (const id of [recId, failId]) await backend.recordings.delete(id);
await backend.webhooks.delete(hook.id);
hooks.stop();
step("admin revokes the key", await status(admin.apiKeys.revoke(key.id)) === 200);
step("the revoked key gets 401", await status(backend.me()) === 401);
step("the key is listed as revoked", !!(await admin.apiKeys.list()).items.find((k) => k.id === key.id)?.revoked_at);
step("admin deletes the client", await status(admin.machineClients.delete(mc.client_id)) === 200);
const after = await fetch(mc.token_endpoint, form({ grant_type: "client_credentials", client_id: mc.client_id, client_secret: mc.client_secret, resource: API }));
step("a deleted client gets no token", !after.ok);
