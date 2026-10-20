// Every route of the API. apps/api implements them; nothing else is served under /v1.
import { createRoute, z } from "@hono/zod-openapi";
import {
  ContextRequest, CreatedRecording, CreatedWebhook, CreateRecording, CreateWebhook, ErrorBody, FinalTranscript, LiveAck, LiveBatch, Match, Me, Meeting, MeetingDetail, NameSpeaker, Notes, Page,
  ReaderProfiles, Recording, SearchHit, StreamEvent, Transcript, Track, TrackUrl, UpdateMeeting, MoveRecording, WebhookEndpoint,
} from "./schemas";
import type { Scope } from "./scopes";

const json = <T extends z.ZodTypeAny>(schema: T, description: string) => ({ content: { "application/json": { schema } }, description });
const errors = { 401: json(ErrorBody, "Missing or invalid token"), 403: json(ErrorBody, "Token lacks a required scope") };
const notFound = { 404: json(ErrorBody, "Not found in this workspace") };
const body = <T extends z.ZodTypeAny>(schema: T) => ({ body: { content: { "application/json": { schema } }, required: true } });
const id = z.object({ id: z.string().uuid() });
/** Scopes a route requires, read by the API's auth middleware. */
const needs = (...scopes: Scope[]) => ({ security: [{ oauth2: scopes }], "x-scopes": scopes, "x-upload": false });
/** A route a recorder sends a recording on: the scopes above, or an upload token for the recording in the path. */
const sends = (...scopes: Scope[]) => ({ ...needs(...scopes), "x-upload": true });

export const getMe = createRoute({
  method: "get", path: "/v1/me", operationId: "getMe", tags: ["identity"],
  summary: "Who the token is", ...needs(),
  responses: { 200: json(Me, "The token's subject, workspace and scopes"), ...errors },
});

export const listMeetings = createRoute({
  method: "get", path: "/v1/meetings", operationId: "listMeetings", tags: ["meetings"],
  summary: "Meetings of the workspace, newest first", ...needs("meetings:read"),
  request: {
    query: z.object({
      limit: z.coerce.number().int().min(1).max(200).default(50),
      cursor: z.string().optional().openapi({ description: "`next` from the previous page" }),
      from: z.string().datetime({ offset: true }).optional().openapi({ description: "Meetings starting at or after" }),
      to: z.string().datetime({ offset: true }).optional().openapi({ description: "Meetings starting before" }),
    }),
  },
  responses: { 200: json(Page(Meeting), "A page of meetings"), ...errors },
});

export const search = createRoute({
  method: "get", path: "/v1/search", operationId: "search", tags: ["meetings"],
  summary: "Find what was said or noted, and meetings by title or by who was in them",
  description: "Full-text search over the workspace's meeting notes, transcripts (live and final), meeting titles and attendees, best match first: titles, then notes, then transcript lines, then attendees. Words match whole, in any language, case-insensitive; accents count (\"reunião\" ≠ \"reuniao\"). A transcript (or a recording's notes) matches when it has every word; within it, each line (or part of the notes) with any of them is a hit, those with all of them first. Hits are lines and parts, not meetings: several can come from one meeting.",
  ...needs("meetings:read", "transcripts:read"),
  request: {
    query: z.object({
      q: z.string().trim().min(1).max(500).openapi({ description: 'Words to find. "Quoted words" match as a phrase, -word excludes, `or` between words matches either', example: "publishing real-time" }),
      limit: z.coerce.number().int().min(1).max(100).default(20),
      cursor: z.string().optional().openapi({ description: "`next` from the previous page" }),
      from: z.string().datetime({ offset: true }).optional().openapi({ description: "Only meetings starting at or after" }),
      to: z.string().datetime({ offset: true }).optional().openapi({ description: "Only meetings starting before" }),
      meeting_id: z.string().uuid().optional().openapi({ description: "Only within this meeting" }),
    }),
  },
  responses: { 200: json(Page(SearchHit), "A page of hits, best first"), ...errors },
});

export const getMeeting = createRoute({
  method: "get", path: "/v1/meetings/{id}", operationId: "getMeeting", tags: ["meetings"],
  summary: "A meeting with its recordings", ...needs("meetings:read"),
  request: { params: id },
  responses: { 200: json(MeetingDetail, "The meeting"), ...errors, ...notFound },
});

export const meetingContext = createRoute({
  method: "post", path: "/v1/meetings/context", operationId: "meetingContext", tags: ["recorder"],
  summary: "What a call that just started could be", description: "The recorder asks this when a meeting app takes the microphone. Meetings with the same call link near that time come first, then the caller's meetings at that time.",
  ...needs("meetings:read"),
  request: body(ContextRequest),
  responses: { 200: json(z.object({ matches: z.array(Match) }), "Candidate meetings, best first"), ...errors },
});

export const createRecording = createRoute({
  method: "post", path: "/v1/recordings", operationId: "createRecording", tags: ["recorder"],
  summary: "Start a recording", description: "Idempotent by `id`. Without `meeting_id`, a meeting is created from the call's link, app and title. With `upload`, also a token for a desktop app to send it (asked again, a fresh one).",
  ...needs("meetings:write"),
  request: body(CreateRecording),
  responses: { 200: json(CreatedRecording, "The recording"), ...errors, ...notFound },
});

export const getRecording = createRoute({
  method: "get", path: "/v1/recordings/{id}", operationId: "getRecording", tags: ["recordings"],
  summary: "A recording", ...needs("meetings:read"),
  request: { params: id },
  responses: { 200: json(Recording, "The recording"), ...errors, ...notFound },
});

export const appendLive = createRoute({
  method: "post", path: "/v1/recordings/{id}/live", operationId: "appendLive", tags: ["recorder"],
  summary: "Append live transcript segments", description: "A batch past what is held is not stored: the answer tells the recorder where to resend from. After the final transcript, batches are ignored.",
  ...sends("meetings:write"),
  request: { params: id, ...body(LiveBatch) },
  responses: { 200: json(LiveAck, "How many segments are held"), ...errors, ...notFound },
});

export const putTranscript = createRoute({
  method: "put", path: "/v1/recordings/{id}/transcript", operationId: "putTranscript", tags: ["recorder"],
  summary: "Deliver the final transcript", description: "Replaces the live draft and ends the recording.",
  ...sends("meetings:write"),
  request: { params: id, ...body(FinalTranscript) },
  responses: { 200: json(Transcript, "The stored transcript"), ...errors, ...notFound },
});

export const getTranscript = createRoute({
  method: "get", path: "/v1/recordings/{id}/transcript", operationId: "getTranscript", tags: ["recordings"],
  summary: "A recording's transcript (live or final)", ...needs("transcripts:read"),
  request: { params: id },
  responses: { 200: json(Transcript, "The transcript"), ...errors, ...notFound },
});

export const nameSpeaker = createRoute({
  method: "patch", path: "/v1/recordings/{id}/transcript/speakers", operationId: "nameSpeaker", tags: ["recordings"],
  summary: "Name a voice of the transcript, by hand", description: "Sets `source: user`; a final transcript delivered again keeps it. A label no segment has is not found.",
  ...needs("meetings:write"),
  request: { params: id, ...body(NameSpeaker) },
  responses: { 200: json(Transcript, "The transcript"), ...errors, ...notFound },
});

export const getNotes = createRoute({
  method: "get", path: "/v1/recordings/{id}/notes", operationId: "getNotes", tags: ["recordings"],
  summary: "A recording's notes: summary, action items, topics",
  description: "Written by a language model from the final transcript, which also names voices it can tell from what was said (`source: llm`). Not found until they are ready (event `notes.ready`), nor when the server has no language model configured.",
  ...needs("transcripts:read"),
  request: { params: id },
  responses: { 200: json(Notes, "The notes"), ...errors, ...notFound },
});

const trackParams = z.object({ id: z.string().uuid(), kind: z.enum(["mix", "mic_hq"]) });
const audio = { "audio/mp4": { schema: z.string().openapi({ format: "binary" }) }, "audio/ogg": { schema: z.string().openapi({ format: "binary" }) }, "audio/flac": { schema: z.string().openapi({ format: "binary" }) } };

export const putTrack = createRoute({
  method: "put", path: "/v1/recordings/{id}/tracks/{kind}", operationId: "putTrack", tags: ["recorder"],
  summary: "Upload an audio track", description: "`mix`: both channels for playback (AAC). `mic_hq`: the recorder's own microphone at 48 kHz (Opus or FLAC).",
  ...sends("meetings:write"),
  request: { params: trackParams, body: { content: audio, required: true } },
  responses: { 200: json(Track, "The stored track"), 415: json(ErrorBody, "Unsupported audio type"), ...errors, ...notFound },
});

export const getTrack = createRoute({
  method: "get", path: "/v1/recordings/{id}/tracks/{kind}", operationId: "getTrack", tags: ["recordings"],
  summary: "Download an audio track", description: "Redirects to a short-lived URL, or serves the bytes.",
  ...needs("audio:read"),
  request: { params: trackParams },
  responses: { 200: { content: audio, description: "The audio" }, 302: { description: "Short-lived download URL" }, ...errors, ...notFound },
});

export const getTrackUrl = createRoute({
  method: "get", path: "/v1/recordings/{id}/tracks/{kind}/url", operationId: "getTrackUrl", tags: ["recordings"],
  summary: "A short-lived URL for a track", description: "For players that can't send a token (an <audio> element).",
  ...needs("audio:read"),
  request: { params: trackParams },
  responses: { 200: json(TrackUrl, "The URL"), ...errors, ...notFound },
});

export const stopRecording = createRoute({
  method: "post", path: "/v1/recordings/{id}/stop", operationId: "stopRecording", tags: ["recordings"],
  summary: "Ask the recorder to stop", description: "The recorder reads it in its next live batch answer (`stop: true`).",
  ...needs("meetings:write"),
  request: { params: id },
  responses: { 200: json(Recording, "The recording"), ...errors, ...notFound },
});

export const stream = createRoute({
  method: "get", path: "/v1/stream", operationId: "stream", tags: ["realtime"],
  summary: "Server-Sent Events of the workspace",
  description: "Each `data:` line is a StreamEvent (`event:` is its type). Filter to one recording with `recording_id`. After a reconnect, read the current state (e.g. the transcript) again, then keep streaming.",
  ...needs("meetings:read"),
  request: { query: z.object({ recording_id: z.string().uuid().optional() }) },
  responses: { 200: { content: { "text/event-stream": { schema: StreamEvent } }, description: "The event stream" }, ...errors },
});

export const updateMeeting = createRoute({
  method: "patch", path: "/v1/meetings/{id}", operationId: "updateMeeting", tags: ["meetings"],
  summary: "Change a meeting (its title)", ...needs("meetings:write"),
  request: { params: id, ...body(UpdateMeeting) },
  responses: { 200: json(Meeting, "The meeting"), ...errors, ...notFound },
});

export const moveRecording = createRoute({
  method: "post", path: "/v1/recordings/{id}/move", operationId: "moveRecording", tags: ["recordings"],
  summary: "Move a recording to another meeting, or to one of its own",
  description: "For a recording linked to the wrong meeting. The meeting it leaves is removed when nothing else is in it.",
  ...needs("meetings:write"),
  request: { params: id, ...body(MoveRecording) },
  responses: { 200: json(Recording, "The recording, in its new meeting"), ...errors, ...notFound },
});

const deleted = { 204: { description: "Deleted, with its transcript and audio" } };

export const deleteRecording = createRoute({
  method: "delete", path: "/v1/recordings/{id}", operationId: "deleteRecording", tags: ["recordings"],
  summary: "Delete a recording", description: "Only who recorded it. Its transcript and audio go with it; a meeting left empty is removed too.",
  ...needs("meetings:write"),
  request: { params: id },
  responses: { ...deleted, ...errors, ...notFound },
});

export const deleteMeeting = createRoute({
  method: "delete", path: "/v1/meetings/{id}", operationId: "deleteMeeting", tags: ["meetings"],
  summary: "Delete a meeting", description: "With all its recordings, transcripts and audio. Only when every recording in it is yours.",
  ...needs("meetings:write"),
  request: { params: id },
  responses: { ...deleted, ...errors, ...notFound },
});

export const createWebhook = createRoute({
  method: "post", path: "/v1/webhooks", operationId: "createWebhook", tags: ["webhooks"],
  summary: "Send the workspace's events to a URL", description: "The same events as /v1/stream, POSTed one by one, signed, retried with backoff for 3 days.",
  ...needs("webhooks:manage"),
  request: body(CreateWebhook),
  responses: { 201: json(CreatedWebhook, "The endpoint, with its signing secret (shown once)"), ...errors },
});

export const listWebhooks = createRoute({
  method: "get", path: "/v1/webhooks", operationId: "listWebhooks", tags: ["webhooks"],
  summary: "The workspace's webhook endpoints", ...needs("webhooks:manage"),
  responses: { 200: json(z.object({ items: z.array(WebhookEndpoint) }), "The endpoints"), ...errors },
});

export const deleteWebhook = createRoute({
  method: "delete", path: "/v1/webhooks/{id}", operationId: "deleteWebhook", tags: ["webhooks"],
  summary: "Stop sending to an endpoint", ...needs("webhooks:manage"),
  request: { params: id },
  responses: { 204: { description: "Removed" }, ...errors, ...notFound },
});

export const readerProfiles = createRoute({
  method: "get", path: "/v1/recorder/profiles", operationId: "readerProfiles", tags: ["recorder"],
  summary: "How to read each call app", description: "Public, no token: a recorder needs it before any recording exists. Answers 304 to a matching If-None-Match.",
  "x-scopes": [], "x-upload": false, "x-public": true,
  responses: { 200: json(ReaderProfiles, "The profiles"), 304: { description: "Unchanged since the ETag sent" } },
});

export const putReaderDump = createRoute({
  method: "put", path: "/v1/recordings/{id}/reader-dumps/{name}", operationId: "putReaderDump", tags: ["recorder"],
  summary: "Send what the call app's window looked like when its reader broke",
  description: "Sent only when the person allows it: the last good snapshot of the call's stage and the one that broke (`<t_ms>-good.json.gz`, `<t_ms>-bad.json.gz`), so the reader profile can be fixed. Gzipped JSON, 4 MB at most, 6 per recording.",
  ...sends("meetings:write"),
  request: {
    params: z.object({ id: z.string().uuid(), name: z.string().regex(/^[0-9]{1,9}-(good|bad)\.json\.gz$/) }),
    body: { content: { "application/gzip": { schema: z.string().openapi({ format: "binary" }) } }, required: true },
  },
  responses: { 204: { description: "Kept" }, 413: json(ErrorBody, "Too large, or too many for this recording"), ...errors, ...notFound },
});
