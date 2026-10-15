import { z } from "@hono/zod-openapi";

export const ErrorBody = z.object({ error: z.string(), message: z.string() }).openapi("Error");

export const Me = z.object({
  subject: z.string().openapi({ description: "The user id, or the client id for a machine token" }),
  workspace: z.string().openapi({ description: "The workspace the token acts in" }),
  scopes: z.array(z.string()),
  client: z.string().nullable().openapi({ description: "The OAuth client that obtained the token" }),
}).openapi("Me");

export const Meeting = z.object({
  id: z.string().uuid(),
  title: z.string().nullable(),
  call_key: z.string().nullable().openapi({ description: "meet:abc-defg-hij, zoom:81234567890, teams:…", example: "meet:abc-defg-hij" }),
  starts_at: z.string().datetime({ offset: true }),
  ends_at: z.string().datetime({ offset: true }).nullable(),
  created_at: z.string().datetime({ offset: true }),
}).openapi("Meeting");

export const Page = <T extends z.ZodTypeAny>(item: T) => z.object({
  items: z.array(item),
  next: z.string().nullable().openapi({ description: "Cursor for the next page; null at the end" }),
});

const datetime = () => z.string().datetime({ offset: true });

export const Segment = z.object({
  start_ms: z.number().int().min(0), end_ms: z.number().int().min(0),
  channel: z.enum(["mic", "system"]).openapi({ description: "mic = whoever runs the recorder, system = everyone else" }),
  speaker: z.string().max(40).nullish().openapi({ description: '"mic", or "system:1", "system:2", … numbered by first appearance' }),
  language: z.string().max(8).nullish(),
  text: z.string().max(10_000),
}).openapi("Segment");

export const ContextRequest = z.object({
  at: datetime().optional().openapi({ description: "When the call started; default now" }),
  url: z.string().max(2000).nullish().openapi({ description: "The call's link, when the recorder can see it" }),
  app: z.string().max(40).nullish().openapi({ example: "google_meet" }),
  title: z.string().max(400).nullish().openapi({ description: "The meeting window's title (Meet puts the code there)" }),
}).openapi("ContextRequest");

export const Match = z.object({
  meeting_id: z.string().uuid(),
  title: z.string().nullable(),
  starts_at: datetime(),
  matched_by: z.enum(["link", "time"]).openapi({ description: "link: same call code; time: a meeting of yours now" }),
}).openapi("Match");

export const CreateRecording = z.object({
  id: z.string().uuid().openapi({ description: "Chosen by the recorder, so a retry creates nothing new" }),
  meeting_id: z.string().uuid().nullish().openapi({ description: "From a context match; omitted → a new meeting" }),
  started_at: datetime(),
  url: z.string().max(2000).nullish(),
  app: z.string().max(40).nullish(),
  title: z.string().max(400).nullish(),
}).openapi("CreateRecording");

export const Track = z.object({
  kind: z.enum(["mix", "mic_hq"]), content_type: z.string(), bytes: z.number().int(),
}).openapi("Track");

export const Recording = z.object({
  id: z.string().uuid(), meeting_id: z.string().uuid(), owner: z.string(),
  started_at: datetime(), ended_at: datetime().nullable(),
  transcript: z.enum(["none", "live", "final"]),
  tracks: z.array(Track),
}).openapi("Recording");

export const LiveBatch = z.object({
  from: z.number().int().min(0).openapi({ description: "Index of the first segment in this batch" }),
  segments: z.array(Segment).max(2_000),
  ended: z.boolean().optional().openapi({ description: "The last batch: the recording ended" }),
}).openapi("LiveBatch");

export const LiveAck = z.object({
  n: z.number().int().openapi({ description: "Segments held; a failed batch is resent from here" }),
  stop: z.boolean().openapi({ description: "Someone asked, from the portal, to stop recording" }),
}).openapi("LiveAck");

export const FinalTranscript = z.object({
  ended_at: datetime(),
  language: z.string().max(20).nullish(),
  model: z.string().max(80).nullish(),
  segments: z.array(Segment).max(20_000),
}).openapi("FinalTranscript");

export const Transcript = z.object({
  recording_id: z.string().uuid(), status: z.enum(["live", "final"]),
  language: z.string().nullable(), model: z.string().nullable(),
  segments: z.array(Segment),
}).openapi("Transcript");

export const MeetingDetail = Meeting.extend({ recordings: z.array(Recording) }).openapi("MeetingDetail");

export const TrackUrl = z.object({
  url: z.string().url().openapi({ description: "Short-lived; works in an <audio> element, no token needed" }),
  expires_at: datetime(),
}).openapi("TrackUrl");

export const StreamEvent = z.object({
  id: z.string().openapi({ description: "Event id (also the SSE id)" }),
  type: z.enum(["recording.created", "recording.ended", "recording.stop_requested", "transcript.live", "transcript.final", "track.ready"]),
  at: datetime(),
  meeting_id: z.string().uuid(),
  recording_id: z.string().uuid(),
  data: z.record(z.string(), z.unknown()).openapi({ description: "transcript.live: {from, segments}; track.ready: Track; others: {}" }),
}).openapi("StreamEvent");

export const UpdateMeeting = z.object({
  title: z.string().trim().max(200).nullable().optional().openapi({ description: "null clears it" }),
}).openapi("UpdateMeeting");

export const MoveRecording = z.object({
  meeting_id: z.string().uuid().nullable().openapi({ description: "Another meeting of the workspace; null → a meeting of its own" }),
}).openapi("MoveRecording");
