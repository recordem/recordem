// Every route of the API. apps/api implements them; nothing else is served under /v1.
import { createRoute, z } from "@hono/zod-openapi";
import {
  ContextRequest, CreateRecording, ErrorBody, FinalTranscript, LiveAck, LiveBatch, Match, Me, Meeting, MeetingDetail, NameSpeaker, Page,
  Recording, StreamEvent, Transcript, Track, TrackUrl, UpdateMeeting, MoveRecording,
} from "./schemas";
import type { Scope } from "./scopes";

const json = <T extends z.ZodTypeAny>(schema: T, description: string) => ({ content: { "application/json": { schema } }, description });
const errors = { 401: json(ErrorBody, "Missing or invalid token"), 403: json(ErrorBody, "Token lacks a required scope") };
const notFound = { 404: json(ErrorBody, "Not found in this workspace") };
const body = <T extends z.ZodTypeAny>(schema: T) => ({ body: { content: { "application/json": { schema } }, required: true } });
const id = z.object({ id: z.string().uuid() });
/** Scopes a route requires, read by the API's auth middleware. */
const needs = (...scopes: Scope[]) => ({ security: [{ oauth2: scopes }], "x-scopes": scopes });

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
  summary: "Start a recording", description: "Idempotent by `id`. Without `meeting_id`, a meeting is created from the call's link, app and title.",
  ...needs("meetings:write"),
  request: body(CreateRecording),
  responses: { 200: json(Recording, "The recording"), ...errors, ...notFound },
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
  ...needs("meetings:write"),
  request: { params: id, ...body(LiveBatch) },
  responses: { 200: json(LiveAck, "How many segments are held"), ...errors, ...notFound },
});

export const putTranscript = createRoute({
  method: "put", path: "/v1/recordings/{id}/transcript", operationId: "putTranscript", tags: ["recorder"],
  summary: "Deliver the final transcript", description: "Replaces the live draft and ends the recording.",
  ...needs("meetings:write"),
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
  summary: "Name a voice of the transcript, by hand", description: "Sets `source: user`; a final transcript delivered again keeps it.",
  ...needs("meetings:write"),
  request: { params: id, ...body(NameSpeaker) },
  responses: { 200: json(Transcript, "The transcript"), ...errors, ...notFound },
});

const trackParams = z.object({ id: z.string().uuid(), kind: z.enum(["mix", "mic_hq"]) });
const audio = { "audio/mp4": { schema: z.string().openapi({ format: "binary" }) }, "audio/ogg": { schema: z.string().openapi({ format: "binary" }) }, "audio/flac": { schema: z.string().openapi({ format: "binary" }) } };

export const putTrack = createRoute({
  method: "put", path: "/v1/recordings/{id}/tracks/{kind}", operationId: "putTrack", tags: ["recorder"],
  summary: "Upload an audio track", description: "`mix`: both channels for playback (AAC). `mic_hq`: the recorder's own microphone at 48 kHz (Opus or FLAC).",
  ...needs("meetings:write"),
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
