# @recordem/sdk

TypeScript client of the [Recordem](https://github.com/recordem/recordem) API: meetings, recordings, transcripts,
notes, search and webhooks. One typed method per API operation, generated from its OpenAPI document; no dependencies.
Runs in Node 18+, Bun, Deno, browsers and edge runtimes.

```bash
npm i @recordem/sdk
```

## A backend that records its users' calls

Your backend holds an API key (create one in the portal under Settings → API keys). It creates each recording and hands
the desktop app an upload token, good for that one recording only. The desktop app ([Desktop SDK](https://github.com/recordem/recordem))
records and sends it; your backend hears back by webhook and reads the result.

```ts
import { createClient, verifyWebhook } from "@recordem/sdk";

const recordem = createClient({ baseUrl: "https://api.recordem.dev", token: process.env.RECORDEM_API_KEY! });

// 1. An interview is about to start: create its recording, get an upload token for the desktop app.
const rec = await recordem.recordings.create({
  id: crypto.randomUUID(),                   // yours to pick: a retry with the same id creates nothing new
  started_at: new Date().toISOString(),
  title: "Interview: Ana Souza",
  ref: "interview_123",                      // your own id, sent back on every webhook
  metadata: { candidate: "c_77" },
  user: "recruiter@acme.com",                // who records, in your terms
  upload: true,
});
// → send rec.upload.token to the desktop app

// 2. Webhooks (POST /v1/webhooks once, with the events you want): check each delivery, then act on it.
app.post("/recordem/webhook", async (req) => {
  const event = await verifyWebhook(process.env.RECORDEM_WEBHOOK_SECRET!, await req.text(), req.headers.get("recordem-signature"));
  if (event.type === "recording.uploaded") {
    const t = await recordem.recordings.getTranscript(event.recording_id!);
    // t.segments: [{ start_ms, end_ms, channel, speaker, text }…], t.speakers: names of the voices
  }
  if (event.type === "notes.ready") {
    const notes = await recordem.recordings.getNotes(event.recording_id!);   // summary, action items, topics
  }
  if (event.type === "recording.failed") { /* event.data.reason, event.data.ref */ }
});
```

Events of a recording, in order: `recording.created`, `recording.started` (the desktop app sent its first part),
`transcript.live`… , `recording.ended`, `transcript.final`, `track.ready`, `recording.uploaded` (the final transcript
and the audio are in), `notes.ready`; or `recording.failed`. `recording.started`, `recording.uploaded` and
`recording.failed` carry your `ref`, `metadata` and `user`.

## Calls and errors

Methods follow the API's paths: `recordem.meetings.list({ from, limit })`, `recordem.meetings.get(id)`,
`recordem.recordings.getTranscript(id)`, `recordem.recordings.putTrack(id, "mix", bytes)`, `recordem.webhooks.create(…)`,
`recordem.search({ q: "pricing" })`, `recordem.me()`. Path parameters come first, then the body or the query. Each one
resolves to the result itself; when the API says no, it throws a `RecordemError` with `status`, `code` and `message`:

```ts
import { RecordemError } from "@recordem/sdk";

const notes = await recordem.recordings.getNotes(id).catch((e) => {
  if (e instanceof RecordemError && e.status === 404) return null;   // not written yet
  throw e;
});
```

## Short-lived tokens instead of an API key

Create a client_credentials client in the portal (Settings → Machine clients) and let the SDK trade it for 1 h tokens:

```ts
import { clientCredentials, createClient } from "@recordem/sdk";

const recordem = createClient({
  baseUrl: "https://api.recordem.dev",
  token: clientCredentials({
    tokenUrl: "https://auth.recordem.dev/api/auth/oauth2/token",
    clientId: process.env.RECORDEM_CLIENT_ID!,
    clientSecret: process.env.RECORDEM_CLIENT_SECRET!,
    resource: "https://api.recordem.dev",
  }),
});
```

## A person's token

Apps acting for a signed-in person (OAuth 2.1 with PKCE against the server's identity provider) pass a function that
returns their current access token: `createClient({ baseUrl, token: async () => session.accessToken })`. Live events
of the workspace: `recordem.stream({ recordingId, onEvent, onOpen, signal })`.

## License

MIT
