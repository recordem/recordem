#!/usr/bin/env bun
// recordem: the Recordem API from a terminal, for people and for agents. Every command is one or two calls of
// @recordem/sdk; --json prints what the API answered. Exit codes: 0 ok, 1 failed, 2 wrong usage, 3 not signed in (or
// the token lacks a scope), 4 not found.
import { clock, createClient, noNotes, notesText, transcriptText, type Client, type MeetingDetail, type Notes, type Recording } from "@recordem/sdk";
import { AuthError, login, logout, server, token } from "./session";

const HELP = `recordem — meetings, transcripts and notes from Recordem

Usage: recordem <command> [args] [--json] [--server <api url>]

  login                       sign in (opens a browser, or paste the URL it ends on)
  logout                      forget this server's tokens
  whoami                      who you are signed in as, and in which workspace
  meetings                    meetings, newest first   [--from <date>] [--to <date>] [--limit n] [--cursor c]
  meeting <meeting id>        a meeting, its recordings and their notes
  search <words…>             find what was said or noted, or meetings by title or attendee
                              [--from <date>] [--to <date>] [--meeting <id>] [--limit n] [--cursor c]
                              "quoted words" match as a phrase, -word excludes, or between words matches either
  transcript <id>             a recording's transcript (or every recording of a meeting), "[m:ss] Name: text"
                              [--since m:ss] [--until m:ss]
  notes <id>                  summary, action items and topics of a recording (or of each recording of a meeting):
                              read these first; the transcript is for quotes and detail
  audio <recording id>        a short-lived URL of the recording's audio   [--kind mix|mic_hq]

Dates: 2026-10-20, 2026-10-20T14:00:00-03:00, today, yesterday, or 7d (7 days ago).
Server: --server, else RECORDEM_URL, else the last one signed in to. RECORDEM_TOKEN, when set, is used as the token.`;

class Exit extends Error {
  constructor(message: string, readonly code: number) { super(message); }
}

function parse(argv: string[]) {
  const args: string[] = [];
  const flags: Record<string, string | true> = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) { args.push(a); continue; }
    const [k, v] = a.slice(2).split("=", 2);
    if (v !== undefined) flags[k] = v;
    else if (["json", "help"].includes(k) || argv[i + 1] === undefined || argv[i + 1].startsWith("--")) flags[k] = true;
    else flags[k] = argv[++i];
  }
  return { args, flags };
}

/** A date or datetime as the API takes it (ISO with offset); a bare date is local midnight. */
function when(s: string | true | undefined): string | undefined {
  if (s === undefined) return undefined;
  if (s === true) throw new Exit("a date is missing", 2);
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).toISOString();
  if (s === "today") return day(new Date());
  if (s === "yesterday") return day(new Date(Date.now() - 86_400_000));
  const ago = s.match(/^(\d+)d$/);
  if (ago) return day(new Date(Date.now() - Number(ago[1]) * 86_400_000));
  if (/^\d{4}-\d{2}-\d{2}$/.test(s)) { const [y, m, d] = s.split("-").map(Number); return new Date(y, m - 1, d).toISOString(); }
  const t = new Date(s);
  if (Number.isNaN(t.getTime())) throw new Exit(`not a date: ${s}`, 2);
  return t.toISOString();
}

/** m:ss or h:mm:ss → ms */
function offset(s: string | true | undefined): number | undefined {
  if (s === undefined) return undefined;
  const parts = String(s).split(":").map(Number);
  if (s === true || parts.some(Number.isNaN)) throw new Exit(`not a time in the recording (m:ss): ${s}`, 2);
  return parts.reduce((a, p) => a * 60 + p, 0) * 1000;
}

const local = (iso: string) => new Date(iso).toLocaleString("sv-SE", { dateStyle: "short", timeStyle: "short" });
const limit = (s: string | true | undefined) => (s === undefined ? undefined : Number(s));
const text = (s: string | true | undefined) => (typeof s === "string" ? s : undefined);

/** The API's answer, or an Exit with the right code. */
async function ok<T>(p: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  const { data, error, response } = await p;
  if (data !== undefined && response.ok) return data;
  const msg = (error as { message?: string } | undefined)?.message ?? response.statusText;
  if (response.status === 401) throw new Exit(`not signed in: ${msg} (run \`recordem login\`)`, 3);
  if (response.status === 403) throw new Exit(`not allowed: ${msg}`, 3);
  if (response.status === 404) throw new Exit(msg, 4);
  throw new Exit(`${response.status}: ${msg}`, 1);
}

/** A recording by its id, or the recordings of a meeting by its id. */
async function recordings(api: Client, id: string): Promise<{ meeting?: MeetingDetail; recordings: Recording[] }> {
  const r = await api.GET("/v1/recordings/{id}", { params: { path: { id } } });
  if (r.data) return { recordings: [r.data] };
  if (r.response.status !== 404) await ok(Promise.resolve(r));   // throws: not signed in, not allowed…
  const meeting = await ok(api.GET("/v1/meetings/{id}", { params: { path: { id } } })).catch((e) => {
    throw e instanceof Exit && e.code === 4 ? new Exit(`no recording or meeting ${id} in this workspace`, 4) : e;
  });
  return { meeting, recordings: meeting.recordings };
}

/** Each recording with its notes, null when it has none (yet). */
async function withNotes(api: Client, rs: Recording[]): Promise<(Recording & { notes: Notes | null })[]> {
  return Promise.all(rs.map(async (r) => {
    const n = await api.GET("/v1/recordings/{id}/notes", { params: { path: { id: r.id } } });
    if (!n.data && n.response.status !== 404) await ok(Promise.resolve(n));
    return { ...r, notes: n.data ?? null };
  }));
}

async function main(argv: string[]) {
  const { args, flags } = parse(argv);
  const [cmd, ...rest] = args;
  const json = flags.json === true;
  const out = (data: unknown, human: () => string) => console.log(json ? JSON.stringify(data, null, 2) : human());
  if (!cmd || flags.help || cmd === "help") return console.log(HELP);
  const base = await server(text(flags.server));
  if (cmd === "login") {
    await login(base, (s) => console.error(s));
    return console.error(`Signed in to ${base}.`);
  }
  if (cmd === "logout") return void (await logout(base), console.error(`Signed out of ${base}.`));
  const api = createClient({ baseUrl: base, token: () => token(base) });
  const need = (what: string) => rest[0] ?? (() => { throw new Exit(`${cmd} needs ${what}; see recordem --help`, 2); })();

  switch (cmd) {
    case "whoami": {
      const me = await ok(api.GET("/v1/me"));
      return out(me, () => `${me.subject} in workspace ${me.workspace}\nscopes: ${me.scopes.join(" ")}\nserver: ${base}`);
    }
    case "meetings": {
      const page = await ok(api.GET("/v1/meetings", { params: { query: { from: when(flags.from), to: when(flags.to), limit: limit(flags.limit), cursor: text(flags.cursor) } } }));
      return out(page, () => [
        ...page.items.map((m) => `${local(m.starts_at)}  ${m.title ?? "(untitled)"}  ${m.id}`),
        page.items.length ? "" : "No meetings.",
        page.next ? `more: --cursor '${page.next}'` : "",
      ].filter(Boolean).join("\n"));
    }
    case "meeting": {
      const m = await ok(api.GET("/v1/meetings/{id}", { params: { path: { id: need("a meeting id") } } }));
      const rs = await withNotes(api, m.recordings);
      return out({ ...m, recordings: rs }, () => [
        `${m.title ?? "(untitled)"}  ${local(m.starts_at)}${m.ends_at ? ` – ${local(m.ends_at)}` : ""}${m.call_key ? `  ${m.call_key}` : ""}  ${m.id}`,
        ...rs.map((r) => [
          `\nrecording ${r.id}  ${local(r.started_at)}  transcript: ${r.transcript}`,
          r.attendees.length ? `with ${r.attendees.join(", ")}` : "",
          r.notes ? `\n${notesText(r.notes)}` : noNotes(r),
        ].filter(Boolean).join("\n")),
        m.recordings.length ? "" : "No recordings.",
      ].filter(Boolean).join("\n"));
    }
    case "search": {
      if (!rest.length) throw new Exit("search needs words to find; see recordem --help", 2);
      const page = await ok(api.GET("/v1/search", { params: { query: {
        q: rest.join(" "), from: when(flags.from), to: when(flags.to), meeting_id: text(flags.meeting), limit: limit(flags.limit), cursor: text(flags.cursor),
      } } }));
      return out(page, () => [
        ...page.items.map((h) => {
          const where = `${h.meeting.title ?? "(untitled)"} · ${local(h.meeting.starts_at)}`;
          const what = h.segment ? `[${clock(h.segment.start_ms)}] ${h.segment.speaker_name ?? h.segment.speaker ?? "?"}: ${h.segment.text}`
            : h.notes ? `notes, ${h.notes.part.replace("_", " ")}${h.notes.topic && h.notes.topic !== h.notes.text ? ` "${h.notes.topic}"` : ""}: ${h.notes.text}`
            : `(${h.field} match)`;
          return `${where}\n  ${what}\n  meeting ${h.meeting.id}${h.recording_id ? `  recording ${h.recording_id}` : ""}`;
        }),
        page.items.length ? "" : "Nothing found.",
        page.next ? `more: --cursor ${page.next}` : "",
      ].filter(Boolean).join("\n"));
    }
    case "transcript": {
      const { meeting, recordings: rs } = await recordings(api, need("a recording or meeting id"));
      const since = offset(flags.since), until = offset(flags.until);
      const ts = await Promise.all(rs.filter((r) => r.transcript !== "none").map(async (r) => ({
        recording: r, transcript: await ok(api.GET("/v1/recordings/{id}/transcript", { params: { path: { id: r.id } } })),
      })));
      if (!ts.length) throw new Exit("no transcript yet", 4);
      return out(meeting ? ts : ts[0].transcript, () => ts.map(({ recording: r, transcript: t }) =>
        `# ${meeting?.title ?? "recording"} ${r.id}, started ${local(r.started_at)} (${t.status}); [m:ss] is from that start\n${transcriptText(t, since, until)}`,
      ).join("\n\n"));
    }
    case "notes": {
      const { recordings: rs } = await recordings(api, need("a recording or meeting id"));
      const all = await withNotes(api, rs);
      if (!all.some((r) => r.notes)) throw new Exit(all.map((r) => (all.length > 1 ? `recording ${r.id}: ${noNotes(r)}` : noNotes(r))).join("\n") || "no recordings", 4);
      if (all.length === 1) return out(all[0].notes, () => notesText(all[0].notes!));
      return out(all.map((r) => ({ recording_id: r.id, notes: r.notes })), () => all.map((r) =>
        `# recording ${r.id}, ${local(r.started_at)}\n${r.notes ? notesText(r.notes) : noNotes(r)}`).join("\n\n"));
    }
    case "audio": {
      const kind = text(flags.kind) ?? "mix";
      if (kind !== "mix" && kind !== "mic_hq") throw new Exit("--kind is mix or mic_hq", 2);
      const u = await ok(api.GET("/v1/recordings/{id}/tracks/{kind}/url", { params: { path: { id: need("a recording id"), kind } } }));
      return out(u, () => u.url);
    }
    default:
      throw new Exit(`unknown command: ${cmd}; see recordem --help`, 2);
  }
}

try {
  await main(process.argv.slice(2));
} catch (e) {
  const code = e instanceof Exit ? e.code : e instanceof AuthError ? 3 : 1;
  console.error(`recordem: ${(e as Error).message}`);
  process.exit(code);
}
