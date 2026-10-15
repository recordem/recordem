// The api database. Workspaces and users live in the identity provider: here they are only ids (text, as issued).
import { index, integer, jsonb, pgTable, primaryKey, text, timestamp, uuid } from "drizzle-orm/pg-core";

export const meetings = pgTable("meetings", {
  id: uuid("id").primaryKey().defaultRandom(),
  workspaceId: text("workspace_id").notNull(),
  title: text("title"),
  callKey: text("call_key"),                 // meet:abc-defg-hij, zoom:…, teams:… — how recordings and events meet
  app: text("app"),                          // google_meet | zoom | microsoft_teams | …
  startsAt: timestamp("starts_at", { withTimezone: true }).notNull(),
  endsAt: timestamp("ends_at", { withTimezone: true }),
  createdBy: text("created_by").notNull(),   // subject of the token that created it
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
}, (t) => [
  index("meetings_workspace_starts").on(t.workspaceId, t.startsAt.desc(), t.id.desc()),
  index("meetings_workspace_call").on(t.workspaceId, t.callKey),
]);

/** One capture of a meeting by one person's recorder. The recorder picks the id, so a retry is the same recording. */
export const recordings = pgTable("recordings", {
  id: uuid("id").primaryKey(),
  workspaceId: text("workspace_id").notNull(),
  meetingId: uuid("meeting_id").notNull().references(() => meetings.id, { onDelete: "cascade" }),
  owner: text("owner").notNull(),            // subject who recorded; only they write to it
  client: text("client"),                    // OAuth client that created it (the recorder, an integration…)
  startedAt: timestamp("started_at", { withTimezone: true }).notNull(),
  endedAt: timestamp("ended_at", { withTimezone: true }),
  stopRequestedAt: timestamp("stop_requested_at", { withTimezone: true }),   // "stop" from the portal, read by the recorder
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
}, (t) => [index("recordings_meeting").on(t.meetingId)]);

export type Segment = {
  start_ms: number; end_ms: number;
  channel: "mic" | "system";                 // mic = whoever runs the recorder, system = everyone else
  speaker?: string | null;                   // "mic", "system:1", "system:2", … by first appearance
  language?: string | null;
  text: string;
};

/** The transcript of a recording: `live` while the call goes on (appended in batches), then `final`, which replaces it. */
export const transcripts = pgTable("transcripts", {
  recordingId: uuid("recording_id").primaryKey().references(() => recordings.id, { onDelete: "cascade" }),
  workspaceId: text("workspace_id").notNull(),
  status: text("status", { enum: ["live", "final"] }).notNull(),
  language: text("language"),
  model: text("model"),
  segments: jsonb("segments").$type<Segment[]>().notNull().default([]),
  updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
});

/** An audio file of a recording, in storage under `key`. */
export const tracks = pgTable("tracks", {
  recordingId: uuid("recording_id").notNull().references(() => recordings.id, { onDelete: "cascade" }),
  kind: text("kind", { enum: ["mix", "mic_hq"] }).notNull(),   // mix: both channels, for playback; mic_hq: the mic at 48 kHz
  workspaceId: text("workspace_id").notNull(),
  key: text("key").notNull(),
  contentType: text("content_type").notNull(),
  bytes: integer("bytes").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
}, (t) => [primaryKey({ columns: [t.recordingId, t.kind] })]);
