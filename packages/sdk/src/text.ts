// Transcripts and notes as plain text, for terminals and language models: a transcript is one line per segment, its
// time from the start of the recording and who said it.
import type { Notes, Recording, Segment, Transcript } from "./index";

/** 0:42, 12:05, 1:02:09 */
export const clock = (ms: number) => {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  return `${h ? `${h}:${String(m).padStart(2, "0")}` : m}:${String(r).padStart(2, "0")}`;
};

/** The voice's name when it has one; else "Recorder" for the mic (whoever ran the recorder) and "Speaker N" for the
 * other side's voice "system:N" ("Others" when diarization couldn't tell). */
export function speaker(s: Pick<Segment, "channel" | "speaker">, t: Pick<Transcript, "speakers">) {
  const named = t.speakers.find((n) => n.label === s.speaker);
  if (named) return named.name;
  if (s.channel === "mic") return "Recorder";
  const n = s.speaker?.split(":")[1];
  return n ? `Speaker ${n}` : "Others";
}

/** "[12:05] Ana Souza: text", for the segments within [fromMs, toMs) of the recording. */
export function transcriptText(t: Pick<Transcript, "segments" | "speakers">, fromMs = 0, toMs = Infinity) {
  return t.segments
    .filter((s) => s.end_ms > fromMs && s.start_ms < toMs)
    .map((s) => `[${clock(s.start_ms)}] ${speaker(s, t)}: ${s.text}`)
    .join("\n");
}

/** Why a recording has no notes (getNotes answered 404): the call goes on, it is being transcribed, or they are not
 * written yet (a minute after the final transcript; never on a server without a language model). */
export function noNotes(r: Pick<Recording, "ended_at" | "transcript">) {
  if (!r.ended_at) return "no notes yet: still recording";
  if (r.transcript !== "final") return "no notes yet: the transcript is still being finished";
  return "no notes yet: they are written shortly after the final transcript (never, when the server has no language model)";
}

/** Notes as text: the summary, then action items and topics as lists. */
export function notesText(n: Pick<Notes, "summary" | "action_items" | "topics">) {
  return [
    n.summary,
    n.action_items.length ? `Action items:\n${n.action_items.map((a) => `- ${a.text}${a.owner ? ` (${a.owner})` : ""}`).join("\n")}` : "No action items.",
    ...n.topics.map((t) => `${t.title}:\n${t.points.map((p) => `- ${p}`).join("\n")}`),
  ].join("\n\n");
}
