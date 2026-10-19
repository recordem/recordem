// A meeting's notes, from its final transcript: a summary, the action items (with who took each), the topics, and
// names for the voices nobody named yet. One call to the language model (llm.ts) reads the whole transcript.
import type { Segment, SpeakerName } from "@recordem/db";
import { ask, type Schema } from "./llm";

export type ActionItem = { text: string; owner: string | null };
export type Topic = { title: string; points: string[] };
export type Notes = { summary: string; action_items: ActionItem[]; topics: Topic[] };

/** What the notes are written from. */
export type Meeting = {
  title: string | null;
  language: string | null;
  segments: Segment[];
  speakers: SpeakerName[];
  /** Everyone the call app listed, and the person who recorded (whose voice is the mic channel). */
  participants: string[];
  me: string | null;
};

/** A voice named from what was said, and why. */
export type Guess = { label: string; name: string; evidence: "self_introduction" | "addressed_and_answers" | "only_candidate" | "none"; quote: string };
type Answer = Notes & { speakers: Guess[] };

/** How sure a name from what was said is: less than the call app showing them speaking. */
const CONFIDENCE = 0.7;

const SYSTEM = `You read the transcript of a meeting and write its notes.

- summary: what the meeting was about and what came out of it, in 3 to 6 sentences.
- action_items: things someone said they or someone else would do. owner is that person's name as it appears in the
  transcript, or null when nobody took it.
- topics: the subjects discussed, in order, each with its main points (facts, decisions, open questions).
- speakers: lines from "system:1", "system:2", … were said by voices the recording couldn't name. Name a voice (label:
  "system:N") only with clear evidence: they introduce themselves (self_introduction); they are asked something by name and
  answer right after (addressed_and_answers); or they are the only participant who could be them (only_candidate).
  The name must be one of the participants, written as listed. quote is the line that shows it. Leave a voice out
  when unsure: a wrong name is worse than none.

Use only what the transcript says. Write the summary, action items and topics in the language given.`;

const string = { type: "string" };
const SCHEMA: Schema = {
  name: "meeting_notes",
  schema: {
    type: "object", additionalProperties: false, required: ["summary", "action_items", "topics", "speakers"],
    properties: {
      summary: string,
      action_items: {
        type: "array",
        items: { type: "object", additionalProperties: false, required: ["text", "owner"], properties: { text: string, owner: { type: ["string", "null"] } } },
      },
      topics: {
        type: "array",
        items: { type: "object", additionalProperties: false, required: ["title", "points"], properties: { title: string, points: { type: "array", items: string } } },
      },
      speakers: {
        type: "array",
        items: {
          type: "object", additionalProperties: false, required: ["label", "name", "evidence", "quote"],
          properties: { label: string, name: string, evidence: { type: "string", enum: ["self_introduction", "addressed_and_answers", "only_candidate", "none"] }, quote: string },
        },
      },
    },
  },
};

/** Writes the notes, and names the voices the evidence names. `guesses`: what the model said of each unnamed voice,
 * kept or not, for the log. */
export async function takeNotes(m: Meeting): Promise<{ notes: Notes; named: SpeakerName[]; guesses: Guess[]; usage: unknown }> {
  const { answer, usage } = await ask<Answer>(SYSTEM, prompt(m), SCHEMA);
  const { speakers, ...notes } = answer;
  const unnamed = new Set(m.segments.filter((s) => s.channel === "system" && !m.speakers.some((n) => n.label === s.speaker)).map((s) => s.speaker));
  return { notes, named: accepted(speakers, m), guesses: speakers.filter((g) => unnamed.has(g.label)), usage };
}

function prompt(m: Meeting): string {
  return [
    `Title: ${m.title ?? "(none)"}`,
    `Language: ${m.language ?? "the one most spoken"}`,
    `Participants: ${m.participants.join(", ") || "(unknown)"}`,
    "",
    transcript(m),
  ].join("\n");
}

/** The transcript as "[mm:ss] Who: text", one line per turn (consecutive lines of one voice joined). */
function transcript(m: Meeting): string {
  const names = new Map(m.speakers.map((s) => [s.label, s.name]));
  const who = (s: Segment) => {
    if (s.channel === "mic") return m.me ?? "The person recording";
    if (!s.speaker) return "Someone";
    return names.get(s.speaker) ?? s.speaker;
  };
  const turns: { at: number; who: string; text: string }[] = [];
  for (const s of m.segments) {
    const w = who(s);
    const last = turns.at(-1);
    if (last?.who === w) last.text += ` ${s.text}`;
    else turns.push({ at: s.start_ms, who: w, text: s.text });
  }
  return turns.map((t) => `[${clock(t.at)}] ${t.who}: ${t.text.trim()}`).join("\n");
}

function clock(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

/** The guesses worth keeping: an unnamed voice of the transcript, a participant's name, and evidence for it. */
function accepted(guesses: Guess[], m: Meeting): SpeakerName[] {
  const named = new Set(m.speakers.map((s) => s.label));
  const voices = new Set(m.segments.filter((s) => s.channel === "system").map((s) => s.speaker));
  const others = m.participants.filter((p) => p !== m.me);
  const out = new Map<string, SpeakerName>();
  for (const g of guesses) {
    const name = participant(g.name, others);
    if (g.evidence === "none" || !name || !voices.has(g.label) || named.has(g.label)) continue;
    out.set(g.label, { label: g.label, name, source: "llm", confidence: CONFIDENCE });
  }
  return [...out.values()];
}

/** The participant a name means: written as listed, or their first name when no one else has it ("Ana" → "Ana Souza"). */
function participant(name: string, participants: string[]): string | undefined {
  const n = name.trim().toLowerCase();
  const exact = participants.find((p) => p.toLowerCase() === n);
  if (exact) return exact;
  const first = participants.filter((p) => p.toLowerCase().split(/\s+/)[0] === n);
  return first.length === 1 ? first[0] : undefined;
}
