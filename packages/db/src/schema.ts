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
  participants: jsonb("participants").$type<Participants>(),                 // who the call app showed, last time it was read
  attendees: jsonb("attendees").$type<string[]>().notNull().default([]),      // everyone it listed at some point, in order
  ref: text("ref"),                                                           // the creator's own id for it, as given
  metadata: jsonb("metadata").$type<Record<string, string>>().notNull().default({}),
  config: jsonb("config").$type<RecordingConfig>(),                           // what the creator asked to capture
  reader: jsonb("reader").$type<ReaderReport[]>().notNull().default([]),      // reader profile issues the recorder hit
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
}, (t) => [index("recordings_meeting").on(t.meetingId), index("recordings_workspace_ref").on(t.workspaceId, t.ref)]);

export type ReaderReport = { t_ms: number; profile: string; revision: number; app_version?: string | null; event: Record<string, unknown> };
export type RecordingConfig = { languages?: string[]; live?: boolean; tracks?: ("mix" | "mic_hq")[]; speakers?: boolean };

export type Segment = {
  start_ms: number; end_ms: number;
  channel: "mic" | "system";                 // mic = whoever runs the recorder, system = everyone else
  speaker?: string | null;                   // "mic", "system:1", "system:2", … by first appearance
  language?: string | null;
  text: string;
};

export type SpeakerName = { label: string; name: string; source: "platform" | "llm" | "user"; confidence?: number | null };
export type Participants = { t_ms: number; visible: boolean; people: { name: string; speaking: boolean; muted?: boolean | null; self?: boolean }[] };

/** The transcript of a recording: `live` while the call goes on (appended in batches), then `final`, which replaces it. */
export const transcripts = pgTable("transcripts", {
  recordingId: uuid("recording_id").primaryKey().references(() => recordings.id, { onDelete: "cascade" }),
  workspaceId: text("workspace_id").notNull(),
  status: text("status", { enum: ["live", "final"] }).notNull(),
  language: text("language"),
  model: text("model"),
  segments: jsonb("segments").$type<Segment[]>().notNull().default([]),
  speakers: jsonb("speakers").$type<SpeakerName[]>().notNull().default([]),   // names for the voices ("system:1" → …)
  updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
});

export type ActionItem = { text: string; owner: string | null };
export type Topic = { title: string; points: string[] };

/** A recording's notes, written by a language model from its final transcript (apps/worker). */
export const notes = pgTable("notes", {
  recordingId: uuid("recording_id").primaryKey().references(() => recordings.id, { onDelete: "cascade" }),
  workspaceId: text("workspace_id").notNull(),
  summary: text("summary").notNull(),
  actionItems: jsonb("action_items").$type<ActionItem[]>().notNull(),
  topics: jsonb("topics").$type<Topic[]>().notNull(),
  language: text("language"),
  model: text("model").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
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

/** Where a workspace's events are POSTed (apps/worker delivers them). The secret signs each delivery. */
export const webhooks = pgTable("webhooks", {
  id: uuid("id").primaryKey().defaultRandom(),
  workspaceId: text("workspace_id").notNull(),
  url: text("url").notNull(),
  events: jsonb("events").$type<string[]>().notNull(),
  secret: text("secret").notNull(),
  createdBy: text("created_by").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  failingSince: timestamp("failing_since", { withTimezone: true }),   // first failed delivery since the last success
  disabledAt: timestamp("disabled_at", { withTimezone: true }),       // after failing for 3 days
}, (t) => [index("webhooks_workspace").on(t.workspaceId)]);
