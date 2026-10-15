// Rows → the contract's shapes.
import type { meetings, recordings, tracks, transcripts } from "@recordem/db";

type Row<T extends { $inferSelect: unknown }> = T["$inferSelect"];
const iso = (d: Date | null) => d?.toISOString() ?? null;

export const meetingView = (m: Row<typeof meetings>) => ({
  id: m.id, title: m.title, call_key: m.callKey, starts_at: m.startsAt.toISOString(),
  ends_at: iso(m.endsAt), created_at: m.createdAt.toISOString(),
});

export const trackView = (t: Row<typeof tracks>) => ({ kind: t.kind, content_type: t.contentType, bytes: t.bytes });

export const recordingView = (r: Row<typeof recordings>, ts: Row<typeof tracks>[], transcript: Pick<Row<typeof transcripts>, "status"> | null) => ({
  id: r.id, meeting_id: r.meetingId, owner: r.owner, started_at: r.startedAt.toISOString(), ended_at: iso(r.endedAt),
  transcript: transcript?.status ?? ("none" as const),
  tracks: ts.map(trackView),
  participants: r.participants ?? undefined,
});

export const transcriptView = (t: Row<typeof transcripts>) => ({
  recording_id: t.recordingId, status: t.status, language: t.language, model: t.model, segments: t.segments,
  speakers: t.speakers,
});
