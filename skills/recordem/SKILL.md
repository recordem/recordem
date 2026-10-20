---
name: recordem
description: Read the user's recorded meetings with the `recordem` CLI — find meetings by date, title or attendee, search what was said or decided, and read notes (summary, action items, topics) and transcripts with speaker names and timestamps. Use when the user asks about a past call or meeting, what someone said, what was decided, action items, or wants a meeting summarized, quoted or compared.
---

# Recordem

Recordem keeps the user's meetings: who was in them, what was said (a transcript per recording, with speaker names)
and notes written from each transcript after the call (summary, action items with owners, topics). The `recordem` CLI
reads them. It only reads; it never changes or deletes anything.

## Setup

Check first: `recordem whoami`. Exit code 3 means not signed in.

- Not installed: from a Recordem checkout, `bun install && cd packages/cli && bun link` (needs [Bun](https://bun.sh));
  or run `bun <checkout>/packages/cli/src/main.ts` wherever this skill says `recordem`.
- Not signed in: ask the user to run `recordem login --server <their API URL>` themselves (it opens a browser; on a
  machine without one they paste back the URL the browser ends on). Don't try to complete the sign-in for them.
- `RECORDEM_URL` picks the server, `RECORDEM_TOKEN` a token to use instead of the stored sign-in.

## Find the meeting

```bash
recordem search "embeddings bedrock"                 # notes, transcript lines, titles, attendees; best first
recordem search "roadmap" --from 7d                  # only meetings of the last 7 days
recordem search '"real time" -zoom' --meeting <id>   # a phrase, excluding a word, within one meeting
recordem meetings --from 2026-10-01 --to 2026-10-08  # by date, newest first
```

Search matches whole words exactly: no stemming, accents count. Meetings may mix languages (English and Portuguese
often): when nothing comes back, try fewer words, another form of the word, or the other language. Each hit says
where it came from (`notes, summary`, `notes, action item`, a transcript line `[m:ss] Speaker: text`, or a title or
attendee match) and prints the `meeting` and `recording` ids to use next. More pages: `--cursor <n>` as printed.

Dates: `2026-10-20`, an ISO datetime, `today`, `yesterday`, or `Nd` (N days ago). A bare date is local midnight.

## Read it: notes first, transcript for detail

```bash
recordem meeting <meeting id>        # its recordings, who was there, and each recording's notes
recordem notes <recording or meeting id>
recordem transcript <recording or meeting id> --since 3:50 --until 6:30
```

- Start with the notes (`meeting` shows them): they answer "what was decided", "what are my action items", "what
  did we discuss" in a fraction of the text.
- Read the transcript only for exact quotes, who said what, or detail the notes leave out. Read around a search hit
  with `--since`/`--until` (a minute before the hit's time) rather than the whole meeting: an hour is ~1,000 lines.
- No notes yet means the call is still being recorded, its transcript is being finished, or the notes come a minute
  later (never, on a server without a language model). The CLI says which; read the transcript then.
- A meeting can have several recordings (one per person who recorded it); `transcript`/`notes` with a meeting id read
  all of them.

Transcript lines are `[m:ss] Speaker: text`, `[m:ss]` counted from the recording's start (printed in its header), not
a time of day. Speakers are named from the call app or inferred; "Recorder" is whoever recorded, "Speaker N" a voice
nobody named. Names can be wrong: when an answer hinges on who said something, say how sure it is.

## Answering

- Cite what you rely on: meeting title, date, and `[m:ss]` for transcript lines.
- Quote sparingly and exactly; paraphrase otherwise, and say when something comes from the notes (machine-written)
  rather than the transcript.
- Show times in the user's time zone; the API's are UTC.
- `--json` on any command prints the API's own JSON, for scripting; prefer the text for reading.

## Exit codes

`0` ok · `1` failed · `2` wrong usage (see `recordem --help`) · `3` not signed in, or the token lacks a scope ·
`4` not found (or no transcript/notes yet).

## Privacy

Meetings are private conversations. Read only what the user's question needs, don't copy transcripts into files,
commits, issues or other tools unless the user asks, and leave out what other attendees said that the question
doesn't touch.
