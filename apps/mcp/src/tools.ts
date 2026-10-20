// The tools, and what an agent needs to know to use them well. Answers are compact text rather than JSON: transcripts
// are long, and every id a next call needs is printed next to what it names.
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { clock, noNotes, notesText, transcriptText, type Client } from "@recordem/sdk";
import { z } from "zod";

const INSTRUCTIONS = `Recordem keeps the user's meetings: who was in them, what was said (transcripts with speaker names), and notes written from each transcript once the call ends (summary, action items with owners, topics).

How to use these tools:
- Read notes first: get_meeting includes each recording's notes, get_notes gives one recording's. Go to the transcript only for exact quotes, who said what, or detail the notes leave out.
- To find where something was discussed, use search_transcripts with a few distinctive words; it matches notes and transcript lines (each hit says which), plus meeting titles and attendees. To read around a transcript hit, use get_transcript with its recording_id, starting a minute or two before the hit's time.
- To find a meeting by date, use list_meetings with from/to; by title or a participant's name, search_transcripts also matches meeting titles and attendees.
- A meeting has one or more recordings (one per person who recorded it). Transcripts and notes belong to a recording: get_meeting lists them.
- A call still recording, or just ended, has no notes yet (get_meeting says why): read its transcript instead.
- Times: dates are ISO 8601 in UTC; convert to the user's time zone when you show them. A transcript line's [m:ss] is from the start of its recording (started_at), not a time of day.
- Speaker names come from the call app or were inferred; "Recorder" is whoever recorded, "Speaker N" a voice nobody named. They can be wrong: say so when it matters.
- When you quote or rely on a transcript, cite the meeting title, date and [m:ss].`;

const datetime = z.string().datetime({ offset: true });
const read = { readOnlyHint: true, openWorldHint: false } as const;
const text = (s: string) => ({ content: [{ type: "text" as const, text: s }] });
const fail = (s: string) => ({ content: [{ type: "text" as const, text: s }], isError: true });

/** The API's error as a tool error the agent can act on. */
const why = (r: { error?: unknown; response: Response }) =>
  fail(`${r.response.status}: ${(r.error as { message?: string } | undefined)?.message ?? r.response.statusText}`);

export function tools(api: Client) {
  const server = new McpServer({ name: "recordem", version: "0.1.0" }, { instructions: INSTRUCTIONS });

  server.registerTool("search_transcripts", {
    title: "Search meetings",
    description: "Full-text search over the user's meetings: their notes (summary, action items, topics), everything said in them, meeting titles and attendee names. Best match first: titles, then notes, then transcript lines. Each hit says where it came from (notes or transcript), with its meeting (title, date, id) and recording id; a transcript hit also has its speaker and time in the recording. Words match whole and exactly (no stemming, accents count): try variants or the other language (meetings mix English and Portuguese) when nothing comes back.",
    inputSchema: {
      query: z.string().min(1).max(500).describe('Words to find. "quoted words" match as a phrase, -word excludes a word, `or` between words matches either'),
      from: datetime.optional().describe("Only meetings starting at or after this time (ISO 8601 with offset)"),
      to: datetime.optional().describe("Only meetings starting before this time (ISO 8601 with offset)"),
      meeting_id: z.string().uuid().optional().describe("Only within this meeting"),
      limit: z.number().int().min(1).max(100).default(20).describe("Hits per page"),
      cursor: z.string().optional().describe("`next` from the previous page"),
    },
    annotations: read,
  }, async ({ query, from, to, meeting_id, limit, cursor }) => {
    const r = await api.GET("/v1/search", { params: { query: { q: query, from, to, meeting_id, limit, cursor } } });
    if (!r.data) return why(r);
    const lines = r.data.items.map((h) => {
      const m = `${h.meeting.title ?? "(untitled)"} (${h.meeting.starts_at}) meeting_id=${h.meeting.id}`;
      if (h.notes) return `- notes, ${h.notes.part.replace("_", " ")}${h.notes.topic && h.notes.topic !== h.notes.text ? ` "${h.notes.topic}"` : ""}: ${h.notes.text}\n  in ${m} recording_id=${h.recording_id}`;
      if (!h.segment) return `- ${h.field} match: ${m}`;
      return `- transcript [${clock(h.segment.start_ms)}] ${h.segment.speaker_name ?? h.segment.speaker ?? "?"}: ${h.segment.text}\n  in ${m} recording_id=${h.recording_id}`;
    });
    return text([lines.length ? lines.join("\n") : "Nothing found.", r.data.next ? `next cursor: ${r.data.next}` : ""].filter(Boolean).join("\n"));
  });

  server.registerTool("list_meetings", {
    title: "List meetings",
    description: "The user's meetings, newest first, optionally within a time range. Each with its title, start and end (UTC) and meeting_id. Use get_meeting for its recordings and attendees.",
    inputSchema: {
      from: datetime.optional().describe("Meetings starting at or after (ISO 8601 with offset)"),
      to: datetime.optional().describe("Meetings starting before (ISO 8601 with offset)"),
      limit: z.number().int().min(1).max(200).default(30),
      cursor: z.string().optional().describe("`next` from the previous page"),
    },
    annotations: read,
  }, async ({ from, to, limit, cursor }) => {
    const r = await api.GET("/v1/meetings", { params: { query: { from, to, limit, cursor } } });
    if (!r.data) return why(r);
    const lines = r.data.items.map((m) => `- ${m.starts_at}${m.ends_at ? ` – ${m.ends_at}` : ""}  ${m.title ?? "(untitled)"}  meeting_id=${m.id}`);
    return text([lines.length ? lines.join("\n") : "No meetings.", r.data.next ? `next cursor: ${r.data.next}` : ""].filter(Boolean).join("\n"));
  });

  server.registerTool("get_meeting", {
    title: "Get a meeting",
    description: "A meeting with its recordings: for each, recording_id, when it started, whether a transcript exists (none, live while the call goes on, final), everyone the call app listed, and its notes (summary, action items, topics), or why there are none yet. Use the recording_id with get_transcript for quotes and detail.",
    inputSchema: { meeting_id: z.string().uuid() },
    annotations: read,
  }, async ({ meeting_id }) => {
    const r = await api.GET("/v1/meetings/{id}", { params: { path: { id: meeting_id } } });
    if (!r.data) return why(r);
    const m = r.data;
    const notes = await Promise.all(m.recordings.map((x) => api.GET("/v1/recordings/{id}/notes", { params: { path: { id: x.id } } })));
    return text([
      `${m.title ?? "(untitled)"}  ${m.starts_at}${m.ends_at ? ` – ${m.ends_at}` : ""}${m.call_key ? `  call ${m.call_key}` : ""}  meeting_id=${m.id}`,
      ...m.recordings.map((x, i) => [
        `\n## recording_id=${x.id} started ${x.started_at}${x.ended_at ? `, ended ${x.ended_at}` : ", still recording"}; transcript: ${x.transcript}`,
        x.attendees.length ? `attendees: ${x.attendees.join(", ")}` : "",
        notes[i].data ? `notes:\n${notesText(notes[i].data)}` : noNotes(x),
      ].filter(Boolean).join("\n")),
      m.recordings.length ? "" : "No recordings.",
    ].filter(Boolean).join("\n"));
  });

  server.registerTool("get_transcript", {
    title: "Read a transcript",
    description: 'A recording\'s transcript as lines "[m:ss] Speaker: text", [m:ss] counted from the recording\'s start. Long meetings come in pages: pass start/end (seconds into the recording) to read around a search hit, or follow `next start` to read on.',
    inputSchema: {
      recording_id: z.string().uuid().describe("From get_meeting or a search hit"),
      start: z.number().min(0).default(0).describe("Seconds into the recording to start from"),
      end: z.number().min(0).optional().describe("Seconds into the recording to stop at; default: as far as max_lines allows"),
      max_lines: z.number().int().min(1).max(2000).default(400).describe("Lines per page"),
    },
    annotations: read,
  }, async ({ recording_id, start, end, max_lines }) => {
    const [rec, t] = await Promise.all([
      api.GET("/v1/recordings/{id}", { params: { path: { id: recording_id } } }),
      api.GET("/v1/recordings/{id}/transcript", { params: { path: { id: recording_id } } }),
    ]);
    if (!rec.data) return why(rec);
    if (!t.data) return why(t);
    const inRange = t.data.segments.filter((s) => s.end_ms > start * 1000 && (end === undefined || s.start_ms < end * 1000));
    const page = inRange.slice(0, max_lines);
    const more = inRange.length > page.length ? page.at(-1)!.end_ms : null;
    return text([
      `recording_id=${recording_id} started ${rec.data.started_at} (transcript ${t.data.status}${t.data.language ? `, ${t.data.language}` : ""}); ${t.data.segments.length} lines in all`,
      transcriptText({ segments: page, speakers: t.data.speakers }) || "No lines in this range.",
      more !== null ? `next start: ${Math.floor(more / 1000)}` : "",
    ].filter(Boolean).join("\n"));
  });

  server.registerTool("get_notes", {
    title: "Get meeting notes",
    description: "A recording's notes, written by a language model from its final transcript: summary, action items (with owners) and topics. Read these before the transcript. Not there while the call goes on, for a minute after it ends, or when the server has no language model: read the transcript then.",
    inputSchema: { recording_id: z.string().uuid().describe("From get_meeting or a search hit") },
    annotations: read,
  }, async ({ recording_id }) => {
    const r = await api.GET("/v1/recordings/{id}/notes", { params: { path: { id: recording_id } } });
    if (r.response.status === 404) {
      const rec = await api.GET("/v1/recordings/{id}", { params: { path: { id: recording_id } } });
      if (!rec.data) return why(rec);
      return text(`${noNotes(rec.data)}. Read the transcript instead (get_transcript).`);
    }
    if (!r.data) return why(r);
    return text(notesText(r.data));
  });

  return server;
}
