// The TypeScript client of the Recordem API: a typed method per operation (src/client.ts, generated from openapi.json),
// plus the event stream. A method returns the result itself and throws a RecordemError when the API says no.
//   const recordem = createClient({ baseUrl, token: async () => accessToken });
//   const { items } = await recordem.meetings.list({ limit: 20 });
import { caller } from "./call";
import { operationsOf } from "./client";
import type { components, operations, paths } from "./schema";

export type { components, operations, paths };
export type Schemas = components["schemas"];
export type Meeting = Schemas["Meeting"];
export type MeetingDetail = Schemas["MeetingDetail"];
export type Recording = Schemas["Recording"];
export type Transcript = Schemas["Transcript"];
export type Segment = Schemas["Segment"];
export type SpeakerName = Schemas["SpeakerName"];
export type Participants = Schemas["Participants"];
export type StreamEvent = Schemas["StreamEvent"];
export type Notes = Schemas["Notes"];
export type SearchHit = Schemas["SearchHit"];
export type CreatedRecording = Schemas["CreatedRecording"];
export type Upload = Schemas["Upload"];
export type ApiKey = Schemas["ApiKey"];
export { clock, noNotes, notesText, speaker, transcriptText } from "./text";
export { clientCredentials, verifyWebhook, WebhookError } from "./backend";
export { RecordemError } from "./call";

export type Options = {
  baseUrl: string;
  /** An API key (`rk_live_…`), or a function returning a valid access token, called before every request (refresh
   * there; `clientCredentials()` makes one). */
  token: string | (() => Promise<string> | string);
  fetch?: typeof fetch;
};

export function createClient(opts: Options) {
  const token = typeof opts.token === "string" ? () => opts.token as string : opts.token;
  const o = { ...opts, token };
  return { ...operationsOf(caller(opts.baseUrl.replace(/\/$/, ""), token, opts.fetch)), stream: (s: StreamOptions) => stream(o, s) };
}
export type Client = ReturnType<typeof createClient>;

export type StreamOptions = {
  recordingId?: string;
  onEvent: (e: StreamEvent) => void;
  /** Called once the stream is open, and again after every reconnect: re-read the current state there. */
  onOpen?: () => void;
  signal?: AbortSignal;
};

/** Follows GET /v1/stream (SSE over fetch, so the token goes in a header), reconnecting with backoff until aborted. */
async function stream(opts: Options & { token: () => Promise<string> | string }, o: StreamOptions) {
  const q = o.recordingId ? `?recording_id=${o.recordingId}` : "";
  for (let attempt = 0; !o.signal?.aborted; attempt++) {
    try {
      const res = await (opts.fetch ?? fetch)(`${opts.baseUrl}/v1/stream${q}`, {
        headers: { authorization: `Bearer ${await opts.token()}`, accept: "text/event-stream" }, signal: o.signal,
      });
      if (!res.ok || !res.body) throw new Error(`stream: ${res.status}`);
      attempt = 0;
      const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
      let buf = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += value;
        for (let i; (i = buf.indexOf("\n\n")) >= 0; buf = buf.slice(i + 2)) {
          const lines = buf.slice(0, i).split("\n");
          const event = lines.find((l) => l.startsWith("event: "))?.slice(7);
          const data = lines.find((l) => l.startsWith("data: "))?.slice(6);
          if (event === "ready") o.onOpen?.();
          else if (data) o.onEvent(JSON.parse(data) as StreamEvent);
        }
      }
    } catch (e) {
      if (o.signal?.aborted) return;
      console.warn("recordem stream:", (e as Error).message);
    }
    await new Promise((r) => setTimeout(r, Math.min(30_000, 500 * 2 ** attempt)));
  }
}
