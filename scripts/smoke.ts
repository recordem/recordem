// End-to-end check, as the recorder would do it: a new user signs up inside an OAuth flow (authorization code +
// PKCE, public client), consents, gets a token, then records a call: context, recording, live batches, final
// transcript, audio — and reads it all back.
//   bun scripts/smoke.ts            (auth and api running; reads AUTH_URL / API_URL from the environment)
const AUTH = process.env.AUTH_URL ?? "http://localhost:39410";
const API = process.env.API_URL ?? "http://localhost:39411";
const REDIRECT = "http://127.0.0.1:39499/callback";

const jar = new Map<string, string>();
const cookie = () => [...jar].map(([k, v]) => `${k}=${v}`).join("; ");
async function go(url: string, init: RequestInit = {}) {
  const res = await fetch(url, { ...init, redirect: "manual", headers: { ...init.headers, cookie: cookie() } });
  for (const c of res.headers.getSetCookie()) { const [kv] = c.split(";"); const i = kv.indexOf("="); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return res;
}
const form = (o: Record<string, string>) => ({ method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(o) });
const step = (name: string, ok: boolean, detail?: unknown) => { console.log(`${ok ? "✓" : "✗"} ${name}`, detail ?? ""); if (!ok) process.exit(1); };
const b64url = (b: ArrayBuffer | Uint8Array) => Buffer.from(b as ArrayBuffer).toString("base64url");

// 1. a public client registers itself (what the CLI or an MCP client does once)
const reg = await fetch(`${AUTH}/api/auth/oauth2/register`, { method: "POST", headers: { "content-type": "application/json" },
  body: JSON.stringify({ client_name: "smoke", redirect_uris: [REDIRECT], application_type: "native", token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"] }) });
const client = await reg.json() as { client_id: string };
step("register public client", reg.ok, reg.ok ? client.client_id : client);

// 2. authorize → sign-in page (no session yet)
const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
const challenge = b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)));
const q = new URLSearchParams({ response_type: "code", client_id: client.client_id, redirect_uri: REDIRECT, state: "s1",
  scope: "openid profile offline_access meetings:read meetings:write transcripts:read audio:read webhooks:manage", code_challenge: challenge, code_challenge_method: "S256", resource: API });
let res = await go(`${AUTH}/api/auth/oauth2/authorize?${q}`);
let loc = res.headers.get("location") ?? "";
step("authorize redirects to sign-in", loc.includes("/sign-in"), loc.slice(0, 80));

// 3. sign up on that page; the flow continues by itself, to consent
const email = `smoke+${Date.now()}@recordem.dev`;
res = await go(new URL(loc, AUTH).href.replace("/sign-in", "/sign-in"), form({ email, password: "smoke-password-1", create: "on", name: "Smoke" }));
loc = res.headers.get("location") ?? "";
while (loc && !loc.includes("/consent") && !loc.startsWith(REDIRECT) && !loc.includes("error=")) { res = await go(new URL(loc, AUTH).href); loc = res.headers.get("location") ?? ""; }
step("sign-up continues to consent", loc.includes("/consent"), loc.slice(0, 100));

// 4. consent → back to the client with a code
res = await go(new URL(loc, AUTH).href.replace(/^(.*?)\/consent/, `${AUTH}/consent`), form({ accept: "yes" }));
loc = res.headers.get("location") ?? "";
const code = new URL(loc, AUTH).searchParams.get("code");
step("consent returns a code", !!code && loc.startsWith(REDIRECT), loc.slice(0, 100));

// 5. code → tokens
const tok = await fetch(`${AUTH}/api/auth/oauth2/token`, form({ grant_type: "authorization_code", code: code!, redirect_uri: REDIRECT,
  client_id: client.client_id, code_verifier: verifier, resource: API }));
const tokens = await tok.json() as { access_token: string; refresh_token?: string };
step("token exchange", tok.ok && !!tokens.access_token, tok.ok ? `refresh: ${!!tokens.refresh_token}` : tokens);
const claims = JSON.parse(Buffer.from(tokens.access_token.split(".")[1], "base64url").toString());
step("token carries the workspace", typeof claims.workspace === "string", { aud: claims.aud, workspace: claims.workspace, scope: claims.scope });

// 6. the API
const auth = { headers: { authorization: `Bearer ${tokens.access_token}` } };
const me = await fetch(`${API}/v1/me`, auth);
step("GET /v1/me", me.ok, await me.json());
const list = await fetch(`${API}/v1/meetings`, auth);
step("GET /v1/meetings", list.ok, await list.json());
const none = await fetch(`${API}/v1/me`);
step("no token → 401", none.status === 401);

// 7. the recorder's flow
const call = (method: string, path: string, body?: unknown) => fetch(`${API}${path}`, {
  method, headers: { ...auth.headers, ...(body === undefined ? {} : { "content-type": "application/json" }) },
  body: body === undefined ? undefined : JSON.stringify(body) });
const url = "https://meet.google.com/abc-defg-hij";
const startedAt = new Date().toISOString();
let r = await call("POST", "/v1/meetings/context", { url, app: "google_meet" });
step("context: nothing yet", r.ok && (await r.json()).matches.length === 0);

const recId = crypto.randomUUID();
r = await call("POST", "/v1/recordings", { id: recId, started_at: startedAt, url, app: "google_meet", title: "Weekly sync" });
const rec = await r.json() as { meeting_id: string; transcript: string };
step("create recording (and its meeting)", r.ok && rec.transcript === "none", rec);
r = await call("POST", "/v1/recordings", { id: recId, started_at: startedAt, url });
step("create again is the same recording", r.ok && (await r.json()).meeting_id === rec.meeting_id);

r = await call("POST", "/v1/meetings/context", { url: "abc-defg-hij - Google Meet", app: "google_meet" });
const ctx = await r.json() as { matches: { meeting_id: string; matched_by: string }[] };
step("context: matched by the call code in the window title", ctx.matches[0]?.meeting_id === rec.meeting_id && ctx.matches[0]?.matched_by === "link", ctx.matches);

const seg = (i: number, channel: "mic" | "system", text: string) => ({ start_ms: i * 1000, end_ms: i * 1000 + 900, channel, speaker: channel === "mic" ? "mic" : "system:1", text });
// a live view: the stream filtered to this recording, read in the background
const events: { type: string; data: { from?: number } }[] = [];
const sse = await fetch(`${API}/v1/stream?recording_id=${recId}`, auth);
step("open SSE stream", sse.ok && (sse.headers.get("content-type") ?? "").startsWith("text/event-stream"));
const reader = sse.body!.pipeThrough(new TextDecoderStream()).getReader();
const ready = (async () => {
  let buf = "";
  for (;;) {
    const { value, done } = await reader.read(); if (done) return;
    buf += value;
    for (let i; (i = buf.indexOf("\n\n")) >= 0; buf = buf.slice(i + 2)) {
      const data = buf.slice(0, i).split("\n").find((l) => l.startsWith("data: "))?.slice(6);
      if (data && data !== "{}") events.push(JSON.parse(data));
    }
  }
})();
await Bun.sleep(150);
r = await call("POST", `/v1/recordings/${recId}/live`, { from: 0, segments: [seg(0, "mic", "Hi"), seg(1, "system", "Hello")] });
step("live batch", (await r.json()).n === 2);
r = await call("POST", `/v1/recordings/${recId}/live`, { from: 5, segments: [seg(5, "mic", "lost before")] });
step("live batch after a gap → resend from 2", (await r.json()).n === 2);
const people = { t_ms: 1500, visible: true, people: [{ name: "Ana Souza", speaking: true }, { name: "Bruno Lima", speaking: false, self: true }] };
r = await call("POST", `/v1/recordings/${recId}/live`, { from: 2, segments: [], participants: people });
r = await call("GET", `/v1/recordings/${recId}`);
step("who is in the call, held on the recording", ((await r.json()) as { participants?: typeof people }).participants?.people[0]?.name === "Ana Souza");

r = await call("PUT", `/v1/recordings/${recId}/transcript`, { ended_at: new Date().toISOString(), language: "en", model: "whisper large-v3-turbo",
  segments: [seg(1, "system", "Hello there"), seg(0, "mic", "Hi")],
  speakers: [{ label: "system:1", name: "Ana Souza", source: "platform", confidence: 0.97 }, { label: "system:2", name: "Carla Dias", source: "platform", confidence: 0.8 }] });
const fin = await r.json() as { status: string; segments: { text: string }[]; speakers: { label: string; name: string; source: string }[] };
step("final transcript replaces the draft, sorted", fin.status === "final" && fin.segments[0].text === "Hi", fin.segments.map((x) => x.text));
step("voices named", fin.speakers.length === 2 && fin.speakers[0].name === "Ana Souza", fin.speakers);
r = await call("PATCH", `/v1/recordings/${recId}/transcript/speakers`, { label: "system:1", name: "Ana Mendes" });
step("name a voice by hand", r.ok);
r = await call("PUT", `/v1/recordings/${recId}/transcript`, { ended_at: new Date().toISOString(), segments: [seg(0, "mic", "Hi"), seg(1, "system", "Hello there")],
  speakers: [{ label: "system:1", name: "Ana Souza", source: "platform" }] });
const again = (await r.json()) as typeof fin;
step("a name set by hand survives the transcript delivered again", again.speakers.find((x) => x.label === "system:1")?.name === "Ana Mendes", again.speakers);
r = await call("PUT", `/v1/recordings/${recId}/transcript`, { ended_at: new Date().toISOString(), language: "en", model: "whisper large-v3-turbo",
  segments: [seg(1, "system", "Hello there"), seg(0, "mic", "Hi")] });

await Bun.sleep(200);
step("SSE delivered live and final events", events.some((e) => e.type === "transcript.live" && e.data.from === 0) && events.some((e) => e.type === "transcript.final")
  && events.some((e) => e.type === "participants.updated") && events.some((e) => e.type === "transcript.speakers"), events.map((e) => e.type));
const bytes = new Uint8Array(4096).map((_, i) => i % 251);
r = await fetch(`${API}/v1/recordings/${recId}/tracks/mic_hq`, { method: "PUT", headers: { ...auth.headers, "content-type": "audio/ogg" }, body: bytes });
step("upload mic_hq track", r.ok && (await r.json()).bytes === bytes.length);
r = await fetch(`${API}/v1/recordings/${recId}/tracks/mic_hq`, { ...auth, redirect: "follow" });
const back = new Uint8Array(await r.arrayBuffer());
step("download it back", r.ok && back.length === bytes.length && back.every((b, i) => b === bytes[i]));

const tu = await (await call("GET", `/v1/recordings/${recId}/tracks/mic_hq/url`)).json() as { url: string };
r = await fetch(tu.url, { headers: { range: "bytes=100-199" } });
const part = new Uint8Array(await r.arrayBuffer());
step("signed track URL serves a byte range, no token", r.status === 206 && part.length === 100 && part[0] === bytes[100], r.headers.get("content-range"));
step("tampered URL is refused", (await fetch(tu.url.replace(/sig=(.)/, (_, c) => `sig=${c === "x" ? "y" : "x"}`))).status === 403);
step("SSE got track.ready", events.some((e) => e.type === "track.ready"));
r = await call("POST", `/v1/recordings/${recId}/stop`);
step("stop after the end is a no-op", r.ok);
await reader.cancel(); void ready;
r = await call("PATCH", `/v1/meetings/${rec.meeting_id}`, { title: "Weekly sync, renamed" });
step("rename the meeting", r.ok && (await r.json()).title === "Weekly sync, renamed");
r = await call("GET", `/v1/meetings/${rec.meeting_id}`);
const m = await r.json() as { call_key: string; ends_at: string | null; recordings: { transcript: string; tracks: unknown[] }[] };
step("meeting with its recording", m.call_key === "meet:abc-defg-hij" && !!m.ends_at && m.recordings[0]?.transcript === "final" && m.recordings[0].tracks.length === 1, m);

// moving: a second recording of the same meeting goes to a meeting of its own; the first meeting keeps the other one
const rec2 = crypto.randomUUID();
await call("POST", "/v1/recordings", { id: rec2, meeting_id: rec.meeting_id, started_at: new Date().toISOString() });
r = await call("POST", `/v1/recordings/${rec2}/move`, { meeting_id: null });
const mv = await r.json() as { meeting_id: string };
step("move a recording to its own meeting", r.ok && mv.meeting_id !== rec.meeting_id);
const left = await (await call("GET", `/v1/meetings/${rec.meeting_id}`)).json() as { recordings: unknown[] };
step("the first meeting keeps its recording", left.recordings.length === 1);
r = await call("POST", `/v1/recordings/${rec2}/move`, { meeting_id: rec.meeting_id });
step("move back: the emptied meeting is gone", r.ok && (await call("GET", `/v1/meetings/${mv.meeting_id}`)).status === 404);

// deleting: a recording of its own meeting takes the meeting along; a meeting goes with its recordings and audio
const rec3 = crypto.randomUUID();
const m3 = await (await call("POST", "/v1/recordings", { id: rec3, started_at: new Date().toISOString(), title: "To delete" })).json() as { meeting_id: string };
await fetch(`${API}/v1/recordings/${rec3}/tracks/mix`, { method: "PUT", headers: { ...auth.headers, "content-type": "audio/mp4" }, body: new Uint8Array(64) });
r = await call("DELETE", `/v1/recordings/${rec3}`);
step("delete a recording", r.status === 204 && (await call("GET", `/v1/recordings/${rec3}`)).status === 404);
step("its emptied meeting is gone", (await call("GET", `/v1/meetings/${m3.meeting_id}`)).status === 404);

// 8. another person, in their own workspace, sees none of it
jar.clear();
let res2 = await go(`${AUTH}/api/auth/oauth2/authorize?${q}`);
res2 = await go(new URL(res2.headers.get("location")!, AUTH).href, form({ email: `other+${Date.now()}@recordem.dev`, password: "smoke-password-2", create: "on" }));
res2 = await go(new URL(res2.headers.get("location")!, AUTH).href.replace(/^(.*?)\/consent/, `${AUTH}/consent`), form({ accept: "yes" }));
const code2 = new URL(res2.headers.get("location")!, AUTH).searchParams.get("code")!;
const tok2 = await (await fetch(`${AUTH}/api/auth/oauth2/token`, form({ grant_type: "authorization_code", code: code2, redirect_uri: REDIRECT,
  client_id: client.client_id, code_verifier: verifier, resource: API }))).json() as { access_token: string };
const other = { headers: { authorization: `Bearer ${tok2.access_token}` } };
step("other workspace: recording is 404", (await fetch(`${API}/v1/recordings/${recId}`, other)).status === 404);
step("other workspace: meeting is 404", (await fetch(`${API}/v1/meetings/${rec.meeting_id}`, other)).status === 404);
step("other workspace: list is empty", ((await (await fetch(`${API}/v1/meetings`, other)).json()) as { items: unknown[] }).items.length === 0);
const steal = await fetch(`${API}/v1/recordings`, { method: "POST", headers: { ...other.headers, "content-type": "application/json" },
  body: JSON.stringify({ id: recId, started_at: startedAt }) });
step("other workspace: can't take the recording id", steal.status === 403);
step("other workspace: can't delete it", (await fetch(`${API}/v1/recordings/${recId}`, { method: "DELETE", ...other })).status === 404);

// 9. the owner deletes the whole meeting: recordings, transcript and audio go with it
r = await call("DELETE", `/v1/meetings/${rec.meeting_id}`);
step("delete the meeting", r.status === 204);
step("its recording is gone", (await call("GET", `/v1/recordings/${recId}`)).status === 404);
step("its audio link no longer serves", (await fetch(tu.url)).status === 404);

// 10. the Desktop SDK flow: a backend creates the recording with an upload token and a webhook; a desktop app sends
// that recording with the token alone, which opens nothing else; the webhook arrives signed.
const got: { body: string; sig: string }[] = [];
const hookServer = Bun.serve({ port: 39498, hostname: "127.0.0.1", fetch: async (req) => {
  got.push({ body: await req.text(), sig: req.headers.get("recordem-signature") ?? "" });
  return new Response("ok");
} });
r = await call("POST", "/v1/webhooks", { url: "http://127.0.0.1:39498/hook", events: ["recording.created", "transcript.live"] });
const hook = await r.json() as { id: string; secret: string };
step("create a webhook (secret shown once)", r.status === 201 && hook.secret.startsWith("whsec_"));
step("list webhooks hides the secret", !JSON.stringify(await (await call("GET", "/v1/webhooks")).json()).includes(hook.secret));

const sdkRec = crypto.randomUUID();
r = await call("POST", "/v1/recordings", { id: sdkRec, started_at: new Date().toISOString(), title: "Interview", ref: "interview-42",
  metadata: { candidate: "c-7" }, config: { languages: ["pt", "en"], live: true }, upload: true });
const made = await r.json() as { ref: string; metadata: Record<string, string>; config: { languages: string[] }; upload: { token: string; expires_at: string } };
step("create with ref, metadata, config and an upload token", r.ok && made.ref === "interview-42" && made.metadata.candidate === "c-7" && !!made.upload?.token, { ref: made.ref, expires_at: made.upload?.expires_at });
const up = { authorization: `Bearer ${made.upload.token}`, "content-type": "application/json" };
r = await fetch(`${API}/v1/recordings/${sdkRec}/live`, { method: "POST", headers: up, body: JSON.stringify({ from: 0, segments: [{ start_ms: 0, end_ms: 900, channel: "mic", speaker: "mic", text: "Olá" }] }) });
step("upload token sends its recording", r.ok, await r.json());
r = await fetch(`${API}/v1/recordings/${sdkRec}/live`, { method: "POST", headers: up, body: JSON.stringify({ from: 1, segments: [],
  reader: [{ t_ms: 5000, profile: "meet-chrome", revision: 6, app_version: "141.0", event: { event: "report", issue: "no_participants" } }] }) });
step("a reader profile issue is taken", r.ok);
const dump = Bun.gzipSync(new TextEncoder().encode(JSON.stringify({ role: "AXWindow", children: [] })));
r = await fetch(`${API}/v1/recordings/${sdkRec}/reader-dumps/5000-bad.json.gz`, { method: "PUT", headers: { ...up, "content-type": "application/gzip" }, body: dump });
step("a reader dump is kept", r.status === 204);
r = await fetch(`${API}/v1/recordings/${sdkRec}/reader-dumps/../../x`, { method: "PUT", headers: { ...up, "content-type": "application/gzip" }, body: dump });
step("a dump name is only <t_ms>-good|bad.json.gz", r.status === 400 || r.status === 404);
step("upload token reads its own recording's state", (await fetch(`${API}/v1/recordings/${sdkRec}`, { headers: up })).ok);
step("upload token reads no transcript", (await fetch(`${API}/v1/recordings/${sdkRec}/transcript`, { headers: up })).status === 403);
step("upload token lists no meetings", (await fetch(`${API}/v1/meetings`, { headers: up })).status === 403);
const otherRec = crypto.randomUUID();
await call("POST", "/v1/recordings", { id: otherRec, started_at: new Date().toISOString() });
step("upload token sends no other recording", (await fetch(`${API}/v1/recordings/${otherRec}/live`, { method: "POST", headers: up, body: JSON.stringify({ from: 0, segments: [] }) })).status === 403);
step("upload token creates nothing", (await fetch(`${API}/v1/recordings`, { method: "POST", headers: up, body: JSON.stringify({ id: crypto.randomUUID(), started_at: new Date().toISOString() }) })).status === 403);

for (let i = 0; i < 50 && got.length < 2; i++) await Bun.sleep(100);
const verify = (g: { body: string; sig: string }) => {
  const [, t, v1] = g.sig.match(/^t=(\d+),v1=([0-9a-f]+)$/) ?? [];
  return !!t && new Bun.CryptoHasher("sha256", hook.secret).update(`${t}.${g.body}`).digest("hex") === v1;
};
const types = got.map((g) => (JSON.parse(g.body) as { type: string; recording_id: string }));
step("webhook: recording.created and transcript.live arrive, signed", got.length >= 2 && got.every(verify)
  && types.some((e) => e.type === "recording.created" && e.recording_id === sdkRec) && types.some((e) => e.type === "transcript.live"), types.map((e) => e.type));
r = await call("DELETE", `/v1/webhooks/${hook.id}`);
step("delete the webhook", r.status === 204);
await call("DELETE", `/v1/recordings/${sdkRec}`);
await call("DELETE", `/v1/recordings/${otherRec}`);
hookServer.stop();
