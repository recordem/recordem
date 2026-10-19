// Background work, off the request path. Each event the api publishes comes on the `events` queue:
// - webhooks: it becomes one delivery per endpoint of the workspace that wants that type, POSTed signed and retried for
//   3 days. An endpoint still failing after that is turned off (the api lists it with disabled_at).
// - notes: a final transcript is read by a language model (notes.ts), when one is configured (llm.ts).
import { connect, meetings, notes, recordings, transcripts, webhooks } from "@recordem/db";
import { Queue, Worker, type Job } from "bullmq";
import { and, eq, isNull } from "drizzle-orm";
import IORedis from "ioredis";
import { createHmac } from "node:crypto";
import { lookup } from "node:dns/promises";
import { isIP } from "node:net";
import * as llm from "./llm";
import { takeNotes } from "./notes";

const env = (k: string, d?: string) => process.env[k] ?? d ?? (() => { throw new Error(`${k} is not set`); })();
const db = connect(env("DATABASE_URL"));
const connection = new IORedis(env("REDIS_URL", "redis://localhost:39421"), { maxRetriesPerRequest: null });

type Event = { id: string; type: string; at: string; meeting_id: string; recording_id: string | null; data: unknown };

// After the first try: 1 min, 5 min, 30 min, 2 h, 6 h, 12 h, 24 h, 24 h — about 3 days.
const DELAYS = [60, 300, 1800, 7200, 21600, 43200, 86400, 86400].map((s) => s * 1000);
const GIVE_UP_MS = 3 * 86400_000;

const deliveries = new Queue("deliveries", { connection });
const noteTaking = new Queue("notes", { connection });

new Worker<{ workspace: string; event: Event }>("events", async (job) => {
  const { workspace, event } = job.data;
  if (event.type === "transcript.final" && event.recording_id && llm.enabled) {
    await noteTaking.add("notes", { workspace, recording: event.recording_id }, {
      jobId: `notes.${event.id}`, attempts: 3, backoff: { type: "exponential", delay: 60_000 }, removeOnComplete: 1000, removeOnFail: 5000,
    });
  }
  const hooks = await db.select({ id: webhooks.id, events: webhooks.events }).from(webhooks)
    .where(and(eq(webhooks.workspaceId, workspace), isNull(webhooks.disabledAt)));
  for (const h of hooks.filter((h) => h.events.includes(event.type))) {
    await deliveries.add("deliver", { webhook: h.id, event }, {
      jobId: `${event.id}.${h.id}`, attempts: DELAYS.length + 1, backoff: { type: "custom" }, removeOnComplete: 1000, removeOnFail: 5000,
    });
  }
}, { connection, concurrency: 10 });

/** An address on this network (the cluster, the LAN, this machine): a webhook is never sent there, unless allowed. */
function private_(ip: string): boolean {
  if (isIP(ip) === 6) return ip === "::1" || /^f[cd]/i.test(ip) || /^fe80/i.test(ip) || ip.startsWith("::ffff:") && private_(ip.slice(7));
  const [a, b] = ip.split(".").map(Number);
  return a === 10 || a === 127 || a === 0 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127);
}
const ALLOW_PRIVATE = env("WEBHOOK_ALLOW_PRIVATE", "") === "1";

async function deliver(job: Job<{ webhook: string; event: Event }>) {
  const [h] = await db.select().from(webhooks).where(eq(webhooks.id, job.data.webhook));
  if (!h || h.disabledAt) return;   // removed or turned off since
  const url = new URL(h.url);
  if (!ALLOW_PRIVATE) {
    const { address } = await lookup(url.hostname);
    if (private_(address)) throw new Error(`${url.hostname} is on a private network`);
  }
  const body = JSON.stringify(job.data.event);
  const t = Math.floor(Date.now() / 1000);
  const signature = createHmac("sha256", h.secret).update(`${t}.${body}`).digest("hex");
  const res = await fetch(h.url, {
    method: "POST", body, redirect: "manual", signal: AbortSignal.timeout(10_000),
    headers: {
      "content-type": "application/json", "user-agent": "Recordem-Webhooks/1",
      "recordem-signature": `t=${t},v1=${signature}`, "recordem-event-id": job.data.event.id, "recordem-event-type": job.data.event.type,
    },
  });
  if (res.status < 200 || res.status >= 300) throw new Error(`${h.url} answered ${res.status}`);
  if (h.failingSince) await db.update(webhooks).set({ failingSince: null }).where(eq(webhooks.id, h.id));
}

const worker = new Worker("deliveries", deliver, {
  connection, concurrency: 20,
  settings: { backoffStrategy: (attempts: number) => DELAYS[Math.min(attempts, DELAYS.length) - 1] },
});

worker.on("failed", async (job, err) => {
  if (!job) return;
  const [h] = await db.select().from(webhooks).where(eq(webhooks.id, job.data.webhook));
  if (!h) return;
  const since = h.failingSince ?? new Date();
  const off = Date.now() - since.getTime() >= GIVE_UP_MS;
  await db.update(webhooks).set({ failingSince: since, ...(off ? { disabledAt: new Date() } : {}) }).where(eq(webhooks.id, h.id));
  console.error(`webhook ${h.id}: ${err.message}${off ? " (turned off)" : ""}`);
});

/** Publishes an event as the api does (events.ts there): live to the workspace's listeners, and to its webhooks. */
const pub = new IORedis(env("REDIS_URL", "redis://localhost:39421"));
const events = new Queue("events", { connection });
async function publish(workspace: string, e: Omit<Event, "id" | "at">) {
  const event: Event = { id: crypto.randomUUID(), at: new Date().toISOString(), ...e };
  await pub.publish(`recordem:ws:${workspace}`, JSON.stringify(event));
  await events.add("event", { workspace, event }, { removeOnComplete: 1000, removeOnFail: 5000 });
}

/** The notes of a recording's final transcript, and names for the voices nobody named that the notes could tell. */
async function noteTake(job: Job<{ workspace: string; recording: string }>) {
  const { workspace, recording: id } = job.data;
  const [row] = await db.select({ r: recordings, t: transcripts, title: meetings.title }).from(recordings)
    .innerJoin(transcripts, eq(transcripts.recordingId, recordings.id)).innerJoin(meetings, eq(meetings.id, recordings.meetingId))
    .where(and(eq(recordings.id, id), eq(recordings.workspaceId, workspace)));
  if (!row || row.t.status !== "final") return;
  const { r, t, title } = row;
  const me = t.speakers.find((s) => s.label === "mic")?.name ?? null;
  const taken = await takeNotes({ title, language: t.language, segments: t.segments, speakers: t.speakers, participants: r.attendees, me });
  for (const g of taken.guesses) {
    const kept = taken.named.find((n) => n.label === g.label);
    console.log(`notes ${id}: ${g.label} ${kept ? `is ${kept.name}` : `not named ${g.name}`} (${g.evidence}: ${g.quote})`);
  }

  const { summary, action_items: actionItems, topics } = taken.notes;
  const values = { summary, actionItems, topics, language: t.language, model: llm.MODEL, createdAt: new Date() };
  await db.insert(notes).values({ recordingId: id, workspaceId: workspace, ...values }).onConflictDoUpdate({ target: notes.recordingId, set: values });
  const ev = { meeting_id: r.meetingId, recording_id: id };
  if (taken.named.length) {
    // Only voices still unnamed: the call app's names and names set by hand (maybe since the model read it) stay.
    const speakers = await db.transaction(async (tx) => {
      const [now] = await tx.select({ speakers: transcripts.speakers }).from(transcripts).where(eq(transcripts.recordingId, id)).for("update");
      const added = taken.named.filter((n) => !now.speakers.some((s) => s.label === n.label));
      if (!added.length) return null;
      const all = [...now.speakers, ...added];
      await tx.update(transcripts).set({ speakers: all, updatedAt: new Date() }).where(eq(transcripts.recordingId, id));
      return all;
    });
    if (speakers) await publish(workspace, { type: "transcript.speakers", ...ev, data: { speakers } });
  }
  await publish(workspace, { type: "notes.ready", ...ev, data: {} });
}

new Worker("notes", noteTake, { connection, concurrency: 2 });

console.log(`worker: webhooks${llm.enabled ? `, notes (${llm.MODEL})` : " (notes off: no LLM_API_KEY)"}`);
