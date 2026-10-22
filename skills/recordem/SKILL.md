---
name: recordem
description: The user's recorded meetings through the `recordem` CLI — find meetings by date, title or attendee, search what was said or decided, read notes (summary, action items, topics) and transcripts with speaker names and timestamps, and rename a meeting. Use when the user asks about a past call or meeting, what someone said, what was decided, action items, wants a meeting summarized, quoted or compared, or wants one titled.
---

# Recordem

Recordem keeps the user's meetings: who was in them, what was said (a transcript per recording, with speaker names)
and notes written from each transcript after the call (summary, action items with owners, topics). The `recordem` CLI
reads them and can give a meeting a title; it never deletes anything.

**The CLI describes itself.** Run `recordem --help` once before the first command: it lists every command and flag of
the installed version, the date formats and the exit codes. `recordem <command> --help` shows one command. This skill
is about how to use them well, not a list of them.

## Setup

Check first: `recordem whoami`. Exit code 3 means not signed in.

- Not installed: `npm i -g @recordem/cli` (Node 22+), or `npx -y @recordem/cli` wherever this says `recordem`.
- Not signed in, or "run `recordem login` again" (the sign-in predates a permission the command needs): ask the user
  to run `recordem login --server <their API URL>` themselves. It opens a browser; don't try to complete it for them.
- Several servers (e.g. dev and production): `recordem server` lists them; `--server <url>` picks one for a command.

## The usual path

```bash
recordem search "roadmap bedrock" --from 7d     # hits print the meeting and recording ids to use next
recordem meeting <meeting id>                    # who was there, and each recording's notes
recordem transcript <id> --since 3:50 --until 6:30
```

- **Notes first.** They answer "what was decided", "my action items", "what did we discuss" in a fraction of the
  text. Read the transcript only for exact quotes, who said what, or detail the notes leave out, and only around a
  hit (a minute before its time): an hour of call is ~1,000 lines.
- **Search matches whole words exactly**: no stemming, accents count. Meetings mix languages (often English and
  Portuguese): when nothing comes back, try fewer words, another form of the word, or the other language.
- **No notes yet** means the call is still recording, its transcript is being finished, or the notes come a minute
  later. The CLI says which; read the transcript meanwhile.
- A meeting can have several recordings (one per person who recorded it); a meeting id reads all of them.
- `[m:ss]` counts from the recording's start (printed in its header), not a time of day. "Recorder" is whoever
  recorded, "Speaker N" a voice nobody named. Names can be wrong: when an answer hinges on who said something, say
  how sure it is.
- Rename only when the user asks, or after proposing a title and hearing yes; say the old and new title.

## Answering

- Cite what you rely on: meeting title, date, and `[m:ss]` for transcript lines.
- Quote sparingly and exactly; paraphrase otherwise, and say when something comes from the notes (machine-written)
  rather than the transcript.
- Show times in the user's time zone. `--json` gives the API's own JSON, for scripting; the text reads better.

## Privacy

Meetings are private conversations. Read only what the question needs, don't copy transcripts into files, commits,
issues or other tools unless the user asks, and leave out what other attendees said that the question doesn't touch.
