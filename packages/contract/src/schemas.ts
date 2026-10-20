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

export const SpeakerName = z.object({
  label: z.string().max(40).openapi({ description: 'A voice of the transcript: "system:1", "system:2", …', example: "system:1" }),
  name: z.string().trim().min(1).max(200),
  source: z.enum(["platform", "llm", "user"]).openapi({ description: "platform: the call app showed them speaking; llm: inferred from what was said; user: set by hand" }),
  confidence: z.number().min(0).max(1).nullish().openapi({ description: "How sure the source is, 0–1; none for user" }),
}).openapi("SpeakerName");

export const Participant = z.object({
  name: z.string().max(200),
  speaking: z.boolean(),
  muted: z.boolean().nullish(),
  self: z.boolean().optional().openapi({ description: "Whoever runs the recorder" }),
}).openapi("Participant");

export const Participants = z.object({
  t_ms: z.number().int().min(0).openapi({ description: "When, from the start of the recording" }),
  visible: z.boolean().openapi({ description: "The call app could be read; false: people are the last ones seen, nobody is known to speak" }),
  people: z.array(Participant).max(500),
}).openapi("Participants");

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

/** What to capture, chosen by whoever creates the recording (an integrator's backend, not its users); the recorder obeys. */
export const RecordingConfig = z.object({
  languages: z.array(z.string().max(8)).max(5).optional().openapi({ description: 'Spoken in the call, e.g. ["pt", "en"]; each turn is transcribed in one of them' }),
  live: z.boolean().optional().openapi({ description: "Send the transcript while the call goes on (transcript.live); default true" }),
  tracks: z.array(z.enum(["mix", "mic_hq"])).max(2).optional().openapi({ description: "Audio kept after the call; default the recorder's setting" }),
  speakers: z.boolean().optional().openapi({ description: "Read who is in the call and who speaks from the call app; default true" }),
}).openapi("RecordingConfig");

export const CreateRecording = z.object({
  id: z.string().uuid().openapi({ description: "Chosen by the caller, so a retry creates nothing new" }),
  meeting_id: z.string().uuid().nullish().openapi({ description: "From a context match; omitted → a new meeting" }),
  started_at: datetime(),
  url: z.string().max(2000).nullish(),
  app: z.string().max(40).nullish(),
  title: z.string().max(400).nullish(),
  ref: z.string().max(200).nullish().openapi({ description: "The caller's own id for it (an interview, a ticket…), kept as given" }),
  metadata: z.record(z.string().max(40), z.string().max(500)).optional().openapi({ description: "Up to 20 key/values kept with the recording" }),
  config: RecordingConfig.optional(),
  upload: z.boolean().optional().openapi({ description: "Also return an upload token: for a backend that hands the recording to a desktop app (the Desktop SDK), which then sends it with that token only" }),
}).openapi("CreateRecording");

export const Upload = z.object({
  token: z.string().openapi({ description: "Bearer token for this recording only: its live transcript, transcript and audio (scope recordings:upload)" }),
  expires_at: datetime().openapi({ description: "6 h after it was made; ask again (same id) for a longer call" }),
}).openapi("Upload");

export const Track = z.object({
  kind: z.enum(["mix", "mic_hq"]), content_type: z.string(), bytes: z.number().int(),
}).openapi("Track");

export const Recording = z.object({
  id: z.string().uuid(), meeting_id: z.string().uuid(), owner: z.string(),
  started_at: datetime(), ended_at: datetime().nullable(),
  transcript: z.enum(["none", "live", "final"]),
  tracks: z.array(Track),
  participants: Participants.optional().openapi({ description: "Who the call app showed in the call, last time the recorder read it; absent when it couldn't" }),
  attendees: z.array(z.string()).openapi({ description: "Everyone the call app listed at some point of the recording, in the order they appeared" }),
  ref: z.string().nullable().optional().openapi({ description: "The creator's own id for it, as given" }),
  metadata: z.record(z.string(), z.string()).optional(),
  config: RecordingConfig.optional().openapi({ description: "What the creator asked to capture" }),
}).openapi("Recording");

export const CreatedRecording = Recording.extend({
  upload: Upload.optional().openapi({ description: "When asked for with `upload`" }),
}).openapi("CreatedRecording");

export const LiveBatch = z.object({
  from: z.number().int().min(0).openapi({ description: "Index of the first segment in this batch" }),
  segments: z.array(Segment).max(2_000),
  participants: Participants.optional().openapi({ description: "Who is in the call now; sent when it changes" }),
  reader: z.array(z.object({
    t_ms: z.number().int().min(0), profile: z.string().max(60), revision: z.number().int(), app_version: z.string().max(40).nullish(),
    event: z.record(z.string(), z.unknown()).openapi({ description: 'e.g. {"event": "report", "issue": "no_participants"}' }),
  })).max(100).optional().openapi({ description: "What went wrong reading the call app (a reader profile that stopped matching), since the last batch taken" }),
  speakers: z.array(SpeakerName).max(200).optional().openapi({ description: "Names for the live voices so far (who the call app showed speaking during each line); sent when they change. Names set by hand are kept" }),
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
  speakers: z.array(SpeakerName).max(200).optional().openapi({ description: "Names for the voices; names set by hand are kept, and entries with source user here are ignored" }),
}).openapi("FinalTranscript");

export const Transcript = z.object({
  recording_id: z.string().uuid(), status: z.enum(["live", "final"]),
  language: z.string().nullable(), model: z.string().nullable(),
  segments: z.array(Segment),
  speakers: z.array(SpeakerName).openapi({ description: "Names for the voices; a label not listed has none" }),
}).openapi("Transcript");

export const ActionItem = z.object({
  text: z.string(),
  owner: z.string().nullable().openapi({ description: "Who took it, as named in the transcript; null when nobody did" }),
}).openapi("ActionItem");

export const Topic = z.object({
  title: z.string(),
  points: z.array(z.string()).openapi({ description: "Facts, decisions and open questions, in the order discussed" }),
}).openapi("Topic");

export const Notes = z.object({
  recording_id: z.string().uuid(),
  summary: z.string(),
  action_items: z.array(ActionItem),
  topics: z.array(Topic),
  language: z.string().nullable().openapi({ description: "What they are written in: the transcript's language" }),
  model: z.string().openapi({ description: "The language model that wrote them" }),
  created_at: datetime(),
}).openapi("Notes");

export const SearchHit = z.object({
  meeting: z.object({ id: z.string().uuid(), title: z.string().nullable(), starts_at: datetime() }).openapi({ description: "The meeting the hit is in; read it whole with getMeeting" }),
  recording_id: z.string().uuid().nullable().openapi({ description: "The recording the line is from (getTranscript, getNotes); null for a hit on the meeting's title" }),
  field: z.enum(["transcript", "title", "attendees"]).openapi({ description: "What matched: a line of a transcript, the meeting's title, or someone the call app listed in it" }),
  segment: z.object({
    start_ms: z.number().int().openapi({ description: "From the start of the recording; add it to the recording's started_at for the time of day" }),
    end_ms: z.number().int(),
    speaker: z.string().nullable().openapi({ description: 'The voice label: "mic", "system:1", …' }),
    speaker_name: z.string().nullable().openapi({ description: "Who it is, from the transcript's speakers; null when the voice has no name" }),
    text: z.string(),
  }).nullable().openapi({ description: "The matching line, for field transcript" }),
  rank: z.number().openapi({ description: "Higher is a better match; only comparable within one search" }),
}).openapi("SearchHit");

export const NameSpeaker = z.object({
  label: z.string().max(40).openapi({ example: "system:1" }),
  name: z.string().trim().max(200).nullable().openapi({ description: "null or empty: the voice goes back to unnamed" }),
}).openapi("NameSpeaker");

export const MeetingDetail = Meeting.extend({ recordings: z.array(Recording) }).openapi("MeetingDetail");

export const TrackUrl = z.object({
  url: z.string().url().openapi({ description: "Short-lived; works in an <audio> element, no token needed" }),
  expires_at: datetime(),
}).openapi("TrackUrl");

export const StreamEvent = z.object({
  id: z.string().openapi({ description: "Event id (also the SSE id)" }),
  type: z.enum(["recording.created", "recording.ended", "recording.stop_requested", "recording.deleted", "meeting.deleted", "transcript.live", "transcript.final", "transcript.speakers", "participants.updated", "track.ready", "notes.ready"]),
  at: datetime(),
  meeting_id: z.string().uuid(),
  recording_id: z.string().uuid().nullable().openapi({ description: "null for meeting.deleted" }),
  data: z.record(z.string(), z.unknown()).openapi({ description: "transcript.live: {from, segments}; transcript.speakers: {speakers}; participants.updated: Participants; track.ready: Track; others: {}" }),
}).openapi("StreamEvent");

export const UpdateMeeting = z.object({
  title: z.string().trim().max(200).nullable().optional().openapi({ description: "null clears it" }),
}).openapi("UpdateMeeting");

export const MoveRecording = z.object({
  meeting_id: z.string().uuid().nullable().openapi({ description: "Another meeting of the workspace; null → a meeting of its own" }),
}).openapi("MoveRecording");

export const WebhookEndpoint = z.object({
  id: z.string().uuid(),
  url: z.string().url().max(2000).openapi({ description: "https (http only for localhost); receives POSTs of StreamEvent" }),
  events: z.array(StreamEvent.shape.type).min(1).openapi({ description: "Which event types it gets" }),
  created_at: datetime(),
  disabled_at: datetime().nullable().openapi({ description: "Turned off after its deliveries failed for 3 days" }),
}).openapi("WebhookEndpoint");

export const CreateWebhook = z.object({ url: WebhookEndpoint.shape.url, events: WebhookEndpoint.shape.events }).openapi("CreateWebhook");

export const CreatedWebhook = WebhookEndpoint.extend({
  secret: z.string().openapi({ description: "Shown once. Each delivery carries Recordem-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256 of `${t}.${raw body}` with this secret>; reject a t older than 5 minutes" }),
}).openapi("CreatedWebhook");

/** How a recorder reads a call app's window: one per app (and app version), in the format of apps/recorder's
 * crates/scrape. Data, so a redesign of Meet or Zoom is fixed on the server without a new recorder. */
export const ReaderProfiles = z.object({
  profiles: z.array(z.object({
    id: z.string().openapi({ example: "meet-chrome" }),
    revision: z.number().int().openapi({ description: "Bumped on every change; a recorder keeps the higher of its own and this" }),
    apps: z.array(z.string()).openapi({ description: "Bundle ids it reads" }),
  }).passthrough()),
}).openapi("ReaderProfiles");
