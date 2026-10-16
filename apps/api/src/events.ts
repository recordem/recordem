// Realtime: every change worth showing live is published on the workspace's Redis channel; each API process keeps
// one subscriber per workspace that has listeners and fans the events out to its SSE connections. Each event also
// goes on the `events` queue, where apps/worker sends it to the workspace's webhook endpoints.
import { Queue } from "bullmq";
import IORedis from "ioredis";
import { RedisClient } from "bun";

export type StreamEvent = {
  id: string; type: "recording.created" | "recording.ended" | "recording.stop_requested" | "recording.deleted" | "meeting.deleted" | "transcript.live" | "transcript.final" | "transcript.speakers" | "participants.updated" | "track.ready";
  at: string; meeting_id: string; recording_id: string | null; data: Record<string, unknown>;
};
type Listener = (e: StreamEvent) => void;

const url = process.env.REDIS_URL ?? "redis://localhost:39421";
const pub = new RedisClient(url);
const sub = new RedisClient(url);
const listeners = new Map<string, Set<Listener>>();
const channel = (workspace: string) => `recordem:ws:${workspace}`;
const queue = new Queue("events", { connection: new IORedis(url, { maxRetriesPerRequest: null }) });

export async function publish(workspace: string, e: Omit<StreamEvent, "id" | "at">) {
  const event: StreamEvent = { id: crypto.randomUUID(), at: new Date().toISOString(), ...e };
  await pub.publish(channel(workspace), JSON.stringify(event)).catch((err) => console.error("publish:", err));
  await queue.add("event", { workspace, event }, { removeOnComplete: 1000, removeOnFail: 5000 }).catch((err) => console.error("queue:", err));
}

/** Listens to a workspace's events; returns the function that stops listening. */
export async function listen(workspace: string, fn: Listener): Promise<() => void> {
  let set = listeners.get(workspace);
  if (!set) {
    listeners.set(workspace, (set = new Set()));
    await sub.subscribe(channel(workspace), (message: string) => {
      const e = JSON.parse(message) as StreamEvent;
      for (const l of listeners.get(workspace) ?? []) l(e);
    });
  }
  set.add(fn);
  let done = false;
  return () => {
    if (done) return;
    done = true;
    set.delete(fn);
    if (!set.size && listeners.get(workspace) === set) {
      listeners.delete(workspace);
      sub.unsubscribe(channel(workspace)).catch((err) => console.error("unsubscribe:", err));
    }
  };
}
