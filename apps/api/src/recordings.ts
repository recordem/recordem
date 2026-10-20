// Meetings and recordings: what the recorder delivers (context → recording → live batches → final transcript →
// audio) and how anyone with the right scope reads it back. Every query is scoped to the token's workspace.
import type { OpenAPIHono } from "@hono/zod-openapi";
import { routes } from "@recordem/contract";
import { meetings, notes, recordings, tracks, transcripts, type Db, type Segment, type SpeakerName } from "@recordem/db";
import { and, asc, eq, inArray, isNull, lte, sql } from "drizzle-orm";
import { HTTPException } from "hono/http-exception";
import { streamSSE } from "hono/streaming";
import type { Actor, Env } from "./auth";
import { callKey } from "./callkey";
import { listen, publish } from "./events";
import { publicOrigin, storage } from "./storage";
import { uploadToken } from "./upload";
import { meetingView, notesView, recordingRefs, recordingView, trackView, transcriptView } from "./views";

const notFound = (what: string) => new HTTPException(404, { res: Response.json({ error: "not_found", message: `${what} not found` }, { status: 404 }) });
const badRequest = (message: string) => new HTTPException(400, { res: Response.json({ error: "bad_request", message }, { status: 400 }) });
const forbidden = (message: string) => new HTTPException(403, { res: Response.json({ error: "forbidden", message }, { status: 403 }) });

const SLACK_MS = 20 * 60_000;            // a call may start a bit early or run over its slot
const NEAR_MS = 12 * 3600_000;           // a call link is matched to meetings within half a day

export function mount(app: OpenAPIHono<Env>, db: Db) {
  async function recording(a: Actor, id: string, write = false) {
    const [r] = await db.select().from(recordings).where(and(eq(recordings.id, id), eq(recordings.workspaceId, a.workspace)));
    if (!r) throw notFound("recording");
    if (write && r.owner !== a.subject) throw forbidden("only who recorded it can write to this recording");
    return r;
  }
  /** recording.started, the first time the recorder sends anything for it (a live batch, or straight the transcript). */
  async function started(r: typeof recordings.$inferSelect, hadTranscript: boolean) {
    if (!hadTranscript) await publish(r.workspaceId, { type: "recording.started", meeting_id: r.meetingId, recording_id: r.id, data: recordingRefs(r) });
  }
  /** recording.uploaded, once: when the final transcript and the mix track are both in, whichever came last. */
  async function uploaded(id: string) {
    const [done] = await db.update(recordings).set({ uploadedAt: new Date() }).where(and(
      eq(recordings.id, id), isNull(recordings.uploadedAt),
      sql`exists (select 1 from ${transcripts} where ${transcripts.recordingId} = ${id} and ${transcripts.status} = 'final')`,
      sql`exists (select 1 from ${tracks} where ${tracks.recordingId} = ${id} and ${tracks.kind} = 'mix')`,
    )).returning();
    if (done) await publish(done.workspaceId, { type: "recording.uploaded", meeting_id: done.meetingId, recording_id: done.id, data: recordingRefs(done) });
  }
  async function full(r: typeof recordings.$inferSelect) {
    const [ts, [t]] = await Promise.all([
      db.select().from(tracks).where(eq(tracks.recordingId, r.id)),
      db.select({ status: transcripts.status }).from(transcripts).where(eq(transcripts.recordingId, r.id)),
    ]);
    return recordingView(r, ts, t ?? null);
  }

  app.openapi(routes.getMeeting, async (c) => {
    const a = c.get("actor");
    const [m] = await db.select().from(meetings).where(and(eq(meetings.id, c.req.valid("param").id), eq(meetings.workspaceId, a.workspace)));
    if (!m) throw notFound("meeting");
    const rs = await db.select().from(recordings).where(eq(recordings.meetingId, m.id)).orderBy(asc(recordings.startedAt));
    return c.json({ ...meetingView(m), recordings: await Promise.all(rs.map(full)) }, 200);
  });

  app.openapi(routes.updateMeeting, async (c) => {
    const a = c.get("actor");
    const b = c.req.valid("json");
    const set = b.title === undefined ? {} : { title: b.title || null };
    const where = and(eq(meetings.id, c.req.valid("param").id), eq(meetings.workspaceId, a.workspace));
    const [m] = Object.keys(set).length
      ? await db.update(meetings).set(set).where(where).returning()
      : await db.select().from(meetings).where(where);
    if (!m) throw notFound("meeting");
    return c.json(meetingView(m), 200);
  });

  app.openapi(routes.meetingContext, async (c) => {
    const a = c.get("actor");
    const b = c.req.valid("json");
    const at = new Date(b.at ?? Date.now());
    const key = callKey(b.url) ?? callKey(b.title);
    const near = await db.select().from(meetings).where(and(
      eq(meetings.workspaceId, a.workspace),
      lte(meetings.startsAt, new Date(at.getTime() + NEAR_MS)),
      sql`coalesce(${meetings.endsAt}, ${meetings.startsAt} + interval '2 hours') >= ${new Date(at.getTime() - NEAR_MS).toISOString()}::timestamptz`,
    )).orderBy(sql`abs(extract(epoch from ${meetings.startsAt} - ${at.toISOString()}::timestamptz))`);
    // By time: a meeting of yours going on now. One that already ended (a past recording) doesn't attract a new
    // call just because it was minutes ago; the same call link still does, above.
    const now = (m: (typeof near)[number]) =>
      m.startsAt.getTime() - SLACK_MS <= at.getTime() && at.getTime() <= (m.endsAt ?? new Date(m.startsAt.getTime() + 2 * 3600_000)).getTime();
    const byLink = key ? near.filter((m) => m.callKey === key) : [];
    // A meeting with another call's link is another call, whatever the time.
    const byTime = near.filter((m) => !byLink.includes(m) && m.createdBy === a.subject && now(m) && !(key && m.callKey && m.callKey !== key));
    const match = (m: (typeof near)[number], matched_by: "link" | "time") =>
      ({ meeting_id: m.id, title: m.title, starts_at: m.startsAt.toISOString(), matched_by });
    return c.json({ matches: [...byLink.map((m) => match(m, "link")), ...byTime.map((m) => match(m, "time"))] }, 200);
  });

  /** The recording as created, with an upload token when asked (a new one each time: a long call asks again). */
  async function created(a: Actor, r: typeof recordings.$inferSelect, upload: boolean | undefined) {
    return { ...(await full(r)), ...(upload ? { upload: await uploadToken({ sub: a.subject, workspace: a.workspace, recording: r.id, client: a.client }) } : {}) };
  }

  app.openapi(routes.createRecording, async (c) => {
    const a = c.get("actor");
    const b = c.req.valid("json");
    if (Object.keys(b.metadata ?? {}).length > 20) throw badRequest("metadata holds 20 keys at most");
    const [prev] = await db.select().from(recordings).where(eq(recordings.id, b.id));
    if (prev) {
      if (prev.workspaceId !== a.workspace || prev.owner !== a.subject) throw forbidden("this recording id is taken");
      return c.json(await created(a, prev, b.upload), 200);
    }
    let meetingId = b.meeting_id ?? null;
    if (meetingId) {
      const [m] = await db.select({ id: meetings.id }).from(meetings).where(and(eq(meetings.id, meetingId), eq(meetings.workspaceId, a.workspace)));
      if (!m) throw notFound("meeting");
    } else {
      const [m] = await db.insert(meetings).values({
        workspaceId: a.workspace, title: b.title ?? null, callKey: callKey(b.url) ?? callKey(b.title), app: b.app ?? null,
        startsAt: new Date(b.started_at), createdBy: a.subject,
      }).returning({ id: meetings.id });
      meetingId = m.id;
    }
    const [r] = await db.insert(recordings).values({
      id: b.id, workspaceId: a.workspace, meetingId, owner: a.subject, client: a.client, startedAt: new Date(b.started_at),
      ref: b.ref ?? null, metadata: b.metadata ?? {}, config: b.config ?? null, user: b.user ?? null,
    }).returning();
    await publish(a.workspace, { type: "recording.created", meeting_id: meetingId, recording_id: r.id, data: {} });
    return c.json(await created(a, r, b.upload), 200);
  });

  app.openapi(routes.moveRecording, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id, true);
    const to = c.req.valid("json").meeting_id;
    const moved = await db.transaction(async (tx) => {
      let target = to;
      if (target) {
        const [m] = await tx.select({ id: meetings.id }).from(meetings).where(and(eq(meetings.id, target), eq(meetings.workspaceId, a.workspace)));
        if (!m) throw notFound("meeting");
      } else {
        const [m] = await tx.insert(meetings).values({
          workspaceId: a.workspace, startsAt: r.startedAt, endsAt: r.endedAt, createdBy: a.subject,
        }).returning({ id: meetings.id });
        target = m.id;
      }
      const [u] = await tx.update(recordings).set({ meetingId: target }).where(eq(recordings.id, r.id)).returning();
      // The meeting it left: gone if empty, else its span follows the recordings still in it.
      const left = await tx.select().from(recordings).where(eq(recordings.meetingId, r.meetingId));
      if (!left.length) await tx.delete(meetings).where(eq(meetings.id, r.meetingId));
      else await tx.update(meetings).set({
        startsAt: new Date(Math.min(...left.map((x) => x.startedAt.getTime()))),
        endsAt: left.every((x) => x.endedAt) ? new Date(Math.max(...left.map((x) => x.endedAt!.getTime()))) : null,
      }).where(eq(meetings.id, r.meetingId));
      return u;
    });
    await publish(a.workspace, { type: "recording.created", meeting_id: moved.meetingId, recording_id: moved.id, data: {} });
    return c.json(await full(moved), 200);
  });

  /** Audio of these recordings, removed from storage after their rows are gone (a failure only leaves an orphan file). */
  /** Every file of these recordings in storage: the audio tracks and any reader dumps. */
  async function dropAudio(recordingIds: string[]) {
    if (!recordingIds.length) return [];
    const audio = await db.select({ key: tracks.key }).from(tracks).where(inArray(tracks.recordingId, recordingIds));
    const rs = await db.select({ reader: recordings.reader }).from(recordings).where(inArray(recordings.id, recordingIds));
    const dumps = rs.flatMap((r) => r.reader).flatMap((x) => (x.event?.event === "dump" && typeof x.event.key === "string" ? [{ key: x.event.key }] : []));
    return [...audio, ...dumps];
  }
  const removeFiles = (keys: { key: string }[]) =>
    Promise.allSettled(keys.map((k) => storage.delete(k.key))).then((rs) => rs.forEach((r) => r.status === "rejected" && console.error("storage delete:", r.reason)));

  app.openapi(routes.deleteRecording, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id, true);
    const keys = await dropAudio([r.id]);
    const meetingGone = await db.transaction(async (tx) => {
      await tx.delete(recordings).where(eq(recordings.id, r.id));    // transcript and tracks cascade
      const left = await tx.select({ id: recordings.id }).from(recordings).where(eq(recordings.meetingId, r.meetingId));
      if (!left.length) await tx.delete(meetings).where(eq(meetings.id, r.meetingId));
      return !left.length;
    });
    await removeFiles(keys);
    await publish(a.workspace, { type: "recording.deleted", meeting_id: r.meetingId, recording_id: r.id, data: {} });
    if (meetingGone) await publish(a.workspace, { type: "meeting.deleted", meeting_id: r.meetingId, recording_id: null, data: {} });
    return c.body(null, 204);
  });

  app.openapi(routes.deleteMeeting, async (c) => {
    const a = c.get("actor");
    const [m] = await db.select().from(meetings).where(and(eq(meetings.id, c.req.valid("param").id), eq(meetings.workspaceId, a.workspace)));
    if (!m) throw notFound("meeting");
    const rs = await db.select({ id: recordings.id, owner: recordings.owner }).from(recordings).where(eq(recordings.meetingId, m.id));
    if (rs.some((r) => r.owner !== a.subject)) throw forbidden("someone else's recording is in this meeting");
    const keys = await dropAudio(rs.map((r) => r.id));
    await db.delete(meetings).where(eq(meetings.id, m.id));            // recordings, transcripts, tracks cascade
    await removeFiles(keys);
    await publish(a.workspace, { type: "meeting.deleted", meeting_id: m.id, recording_id: null, data: {} });
    return c.body(null, 204);
  });

  app.openapi(routes.getRecording, async (c) => c.json(await full(await recording(c.get("actor"), c.req.valid("param").id)), 200));

  app.openapi(routes.appendLive, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id, true);
    const b = c.req.valid("json");
    const stop = !!r.stopRequestedAt;
    const [t] = await db.select().from(transcripts).where(eq(transcripts.recordingId, r.id));
    if (t?.status === "final") return c.json({ n: b.from + b.segments.length, stop: false }, 200);
    // The first batch: an empty live transcript marks the recording started, before anything else it brings.
    if (!t) {
      const [made] = await db.insert(transcripts).values({ recordingId: r.id, workspaceId: a.workspace, status: "live", segments: [] })
        .onConflictDoNothing().returning({ id: transcripts.recordingId });
      if (made) await started(r, false);
    }
    const ev = { meeting_id: r.meetingId, recording_id: r.id };
    // A reader profile that stopped matching: kept on the recording (the last 200) and logged, to fix the profile.
    if (b.reader?.length) {
      for (const x of b.reader) console.warn(`reader: ${x.profile} r${x.revision}${x.app_version ? ` (app ${x.app_version})` : ""} ${JSON.stringify(x.event)} recording ${r.id}`);
      const all = sql`coalesce(${recordings.reader}, '[]'::jsonb) || ${JSON.stringify(b.reader)}::jsonb`;
      await db.update(recordings).set({
        reader: sql`(select coalesce(jsonb_agg(e order by i), '[]'::jsonb) from (select e, i from jsonb_array_elements(${all}) with ordinality as x(e, i) order by i desc limit 200) last)`,
      }).where(eq(recordings.id, r.id));
    }
    if (b.participants) {
      // Who is in the call now, and everyone who was at some point (the call ends with nobody listed).
      const joined = b.participants.people.map((p) => p.name).filter((n) => !r.attendees.includes(n));
      await db.update(recordings).set({ participants: b.participants, attendees: [...r.attendees, ...new Set(joined)] }).where(eq(recordings.id, r.id));
      await publish(a.workspace, { type: "participants.updated", ...ev, data: b.participants });
    }
    const held = t?.segments ?? [];
    if (b.from > held.length) return c.json({ n: held.length, stop }, 200);
    const segments: Segment[] = [...held.slice(0, b.from), ...b.segments];
    await db.insert(transcripts).values({ recordingId: r.id, workspaceId: a.workspace, status: "live", segments })
      .onConflictDoUpdate({ target: transcripts.recordingId, set: { segments, updatedAt: new Date() } });
    if (b.segments.length) await publish(a.workspace, { type: "transcript.live", ...ev, data: { from: b.from, segments: b.segments } });
    // Who speaks, as the call app showed it: names set by hand win, as in the final transcript.
    if (b.speakers) {
      const u = await db.transaction(async (tx) => {
        const [prev] = await tx.select({ speakers: transcripts.speakers }).from(transcripts).where(eq(transcripts.recordingId, r.id)).for("update");
        const byHand = (prev?.speakers ?? []).filter((s) => s.source === "user");
        const found = b.speakers!.filter((s) => s.source !== "user" && !byHand.some((h) => h.label === s.label));
        const [row] = await tx.update(transcripts).set({ speakers: [...byHand, ...found], updatedAt: new Date() })
          .where(eq(transcripts.recordingId, r.id)).returning();
        return row;
      });
      if (u) await publish(a.workspace, { type: "transcript.speakers", ...ev, data: { speakers: u.speakers } });
    }
    if (b.ended && !r.endedAt) {
      await db.update(recordings).set({ endedAt: new Date() }).where(eq(recordings.id, r.id));
      await publish(a.workspace, { type: "recording.ended", ...ev, data: {} });
    }
    return c.json({ n: segments.length, stop }, 200);
  });

  app.openapi(routes.putTranscript, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id, true);
    const b = c.req.valid("json");
    const segments = [...b.segments].sort((x, y) => x.start_ms - y.start_ms);
    // A name someone set by hand wins over what the recorder found (a retry, or the same call delivered again). Only
    // nameSpeaker sets one: the recorder can't claim `user`. Locked, so a rename in between isn't overwritten.
    const { row: t, had } = await db.transaction(async (tx) => {
      const [prev] = await tx.select({ speakers: transcripts.speakers }).from(transcripts).where(eq(transcripts.recordingId, r.id)).for("update");
      const byHand = (prev?.speakers ?? []).filter((s) => s.source === "user");
      const found = (b.speakers ?? []).filter((s) => s.source !== "user" && !byHand.some((u) => u.label === s.label));
      const values = { status: "final" as const, language: b.language ?? null, model: b.model ?? null, segments, speakers: [...byHand, ...found], updatedAt: new Date() };
      const [row] = await tx.insert(transcripts).values({ recordingId: r.id, workspaceId: a.workspace, ...values })
        .onConflictDoUpdate({ target: transcripts.recordingId, set: values }).returning();
      return { row, had: !!prev };
    });
    await started(r, had);
    const endedAt = new Date(b.ended_at);
    await db.update(recordings).set({ endedAt }).where(eq(recordings.id, r.id));
    await db.update(meetings).set({ endsAt: sql`greatest(${meetings.endsAt}, ${endedAt.toISOString()}::timestamptz)` })
      .where(eq(meetings.id, r.meetingId));
    await publish(a.workspace, { type: "transcript.final", meeting_id: r.meetingId, recording_id: r.id, data: {} });
    await uploaded(r.id);
    return c.json(transcriptView(t), 200);
  });

  app.openapi(routes.nameSpeaker, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id);
    const { label, name } = c.req.valid("json");
    const u = await db.transaction(async (tx) => {
      const [t] = await tx.select().from(transcripts).where(eq(transcripts.recordingId, r.id)).for("update");
      if (!t) throw notFound("transcript");
      if (!t.segments.some((s) => s.speaker === label)) throw notFound("speaker");
      const speakers: SpeakerName[] = [
        ...t.speakers.filter((s) => s.label !== label),
        ...(name ? [{ label, name, source: "user" as const, confidence: null }] : []),
      ];
      const [row] = await tx.update(transcripts).set({ speakers, updatedAt: new Date() }).where(eq(transcripts.recordingId, r.id)).returning();
      return row;
    });
    await publish(a.workspace, { type: "transcript.speakers", meeting_id: r.meetingId, recording_id: r.id, data: { speakers: u.speakers } });
    return c.json(transcriptView(u), 200);
  });

  app.openapi(routes.getTranscript, async (c) => {
    const r = await recording(c.get("actor"), c.req.valid("param").id);
    const [t] = await db.select().from(transcripts).where(eq(transcripts.recordingId, r.id));
    if (!t) throw notFound("transcript");
    return c.json(transcriptView(t), 200);
  });

  app.openapi(routes.getNotes, async (c) => {
    const r = await recording(c.get("actor"), c.req.valid("param").id);
    const [n] = await db.select().from(notes).where(eq(notes.recordingId, r.id));
    if (!n) throw notFound("notes");
    return c.json(notesView(n), 200);
  });

  const AUDIO = new Set(["audio/mp4", "audio/ogg", "audio/flac"]);

  app.openapi(routes.putTrack, async (c) => {
    const a = c.get("actor");
    const { id, kind } = c.req.valid("param");
    const r = await recording(a, id, true);
    const type = (c.req.header("content-type") ?? "").split(";")[0].trim();
    if (!AUDIO.has(type)) return c.json({ error: "unsupported_media_type", message: `audio/mp4, audio/ogg or audio/flac, not ${type || "nothing"}` }, 415);
    const ext = { "audio/mp4": "m4a", "audio/ogg": "ogg", "audio/flac": "flac" }[type];
    const key = `w/${a.workspace}/m/${r.meetingId}/r/${r.id}/${kind}.${ext}`;
    const bytes = await storage.put(key, await c.req.blob(), type);
    const values = { key, contentType: type, bytes, createdAt: new Date() };
    const [t] = await db.insert(tracks).values({ recordingId: r.id, kind, workspaceId: a.workspace, ...values })
      .onConflictDoUpdate({ target: [tracks.recordingId, tracks.kind], set: values }).returning();
    await publish(a.workspace, { type: "track.ready", meeting_id: r.meetingId, recording_id: r.id, data: trackView(t) });
    await uploaded(r.id);
    return c.json(trackView(t), 200);
  });

  // The recorder (a Desktop SDK app) gave up: kept, and told to webhooks, unless it got through after all.
  app.openapi(routes.reportFailure, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id, true);
    if (r.uploadedAt) return c.json(await full(r), 200);
    const b = c.req.valid("json");
    const failure = { at: new Date().toISOString(), reason: b.reason, message: b.message ?? null };
    const [u] = await db.update(recordings).set({ failure }).where(eq(recordings.id, r.id)).returning();
    await publish(a.workspace, { type: "recording.failed", meeting_id: u.meetingId, recording_id: u.id, data: { ...recordingRefs(u), reason: failure.reason, message: failure.message } });
    return c.json(await full(u), 200);
  });

  // A reader's last good and broken snapshots, when the person allows sending them: kept next to the audio and listed
  // on the recording's reader reports (event "dump"), for whoever fixes the profile.
  app.openapi(routes.putReaderDump, async (c) => {
    const a = c.get("actor");
    const { id, name } = c.req.valid("param");
    const r = await recording(a, id, true);
    const tooMany = r.reader.filter((x) => x.event?.event === "dump").length >= 6;
    const body = await c.req.blob();
    if (tooMany || body.size > 4 * 1024 * 1024) return c.json({ error: "too_large", message: "4 MB and 6 dumps per recording at most" }, 413);
    const key = `w/${a.workspace}/m/${r.meetingId}/r/${r.id}/reader/${name}`;
    await storage.put(key, body, "application/gzip");
    const entry = { t_ms: Number(name.split("-")[0]), profile: "", revision: 0, event: { event: "dump", name, key } };
    await db.update(recordings).set({ reader: sql`${recordings.reader} || ${JSON.stringify([entry])}::jsonb` }).where(eq(recordings.id, r.id));
    console.warn(`reader dump: recording ${r.id} ${name} (${body.size} bytes)`);
    return c.body(null, 204);
  });

  app.openapi(routes.getTrack, async (c) => {
    const { id, kind } = c.req.valid("param");
    const r = await recording(c.get("actor"), id);
    const [t] = await db.select().from(tracks).where(and(eq(tracks.recordingId, r.id), eq(tracks.kind, kind)));
    if (!t) throw notFound("track");
    return (await storage.serve(t.key, t.contentType)) as never;
  });

  const URL_TTL = 3600;   // long enough to listen to a whole meeting without the player losing its source

  app.openapi(routes.getTrackUrl, async (c) => {
    const { id, kind } = c.req.valid("param");
    const r = await recording(c.get("actor"), id);
    const [t] = await db.select().from(tracks).where(and(eq(tracks.recordingId, r.id), eq(tracks.kind, kind)));
    if (!t) throw notFound("track");
    return c.json({ url: storage.url(t.key, URL_TTL, publicOrigin(c.req.raw)), expires_at: new Date(Date.now() + URL_TTL * 1000).toISOString() }, 200);
  });

  app.openapi(routes.stopRecording, async (c) => {
    const a = c.get("actor");
    const r = await recording(a, c.req.valid("param").id);
    if (!r.endedAt && !r.stopRequestedAt) {
      const [u] = await db.update(recordings).set({ stopRequestedAt: new Date() }).where(eq(recordings.id, r.id)).returning();
      await publish(a.workspace, { type: "recording.stop_requested", meeting_id: r.meetingId, recording_id: r.id, data: {} });
      return c.json(await full(u), 200);
    }
    return c.json(await full(r), 200);
  });

  app.openapi(routes.stream, (c) => {
    const a = c.get("actor");
    const only = c.req.valid("query").recording_id;
    return streamSSE(c, async (s) => {
      const queue: StreamEventLike[] = [];
      let wake: (() => void) | null = null;
      const stop = await listen(a.workspace, (e) => {
        if (only && e.recording_id !== only) return;
        queue.push(e); wake?.();
      });
      s.onAbort(stop);
      await s.writeSSE({ event: "ready", data: "{}" });
      while (!s.aborted) {
        while (queue.length) { const e = queue.shift()!; await s.writeSSE({ id: e.id, event: e.type, data: JSON.stringify(e) }); }
        // A comment line every 25 s keeps proxies from closing an idle stream.
        await Promise.race([new Promise<void>((r) => (wake = r)), Bun.sleep(25_000).then(() => s.write(": ping\n\n"))]);
        wake = null;
      }
      stop();
    }) as never;
  });
}

type StreamEventLike = { id: string; type: string };
