# Recorder (macOS)

Menu-bar app that notices a call, records it on this Mac (no bot joins the call), transcribes it here and delivers
the transcript, then the audio, to a Recordem server. MIT licensed.

```
a meeting app or browser starts using the mic   (Anarlog detect: Core Audio listener)
  → POST /v1/meetings/context {at, app, title}   → meetings of yours now, or with the same call link
  → you pick one (or a new meeting) and record    (Anarlog audio-actual: mic with echo cancellation + system audio tap)
  → POST /v1/recordings {id, meeting_id?, …}      → the recording exists on the server from the start
  → mic.wav + system.wav, 16 kHz                  (mic = you, system = the others)
    + mic_hq.wav, 48 kHz, no echo cancellation    (your own voice in full quality; setting "keep my voice in HQ")
  → live: Silero VAD cuts each channel's turns, whisper.cpp transcribes them → POST /v1/recordings/{id}/live
  → after the call: pyannote (CoreML) tells the other voices apart → "system:1", "system:2", …;
    whisper.cpp large-v3-turbo (Metal) transcribes every turn, in the language each one sounds like
  → PUT /v1/recordings/{id}/transcript           → replaces the live draft
  → PUT /v1/recordings/{id}/tracks/mix           (both channels, AAC 32 kbps, ~14 MB/h)
  → PUT /v1/recordings/{id}/tracks/mic_hq        (your voice, AAC 128 kbps 48 kHz, ~58 MB/h)
```

The live transcript is a draft (one "system" voice, half the cores, since the call runs on the same Mac); the pass
after the call replaces it. "Stop recording" in the portal reaches the recorder in its next live batch answer.

**Signing in** is OAuth 2.1 + PKCE as the first-party public client `recordem-recorder`, redirect `recordem://oauth`.
A server can offer a "Connect the recorder" link: `recordem://login?api=…&auth=…&web=…`.

**Anarlog crates.** Capture, detection and diarization are Anarlog's MIT crates (`audio`, `audio-actual`, `detect`,
`pyannote-local`) as git dependencies pinned to one commit — no fork. Their workspace pins versions a git dependency
doesn't inherit, so `src-tauri/Cargo.lock` was seeded from theirs: when bumping `rev`, copy their `Cargo.lock` again
and rebuild. Never depend on anything under their `enterprise/` (commercial license).

**UI** is plain HTML/JS in `ui/` (no bundler, no package.json), outside the bun workspace.

## Dev build

```bash
cargo install tauri-cli --version "^2" --locked     # once; cmake on PATH (whisper.cpp)
apps/recorder/scripts/dev-build.sh --setup          # once per Mac: a "Recordem Dev" signing certificate
RECORDEM_API_URL=http://<server>:39411 RECORDEM_AUTH_URL=http://<server>:39410 RECORDEM_WEB_URL=https://<server>:39412 \
  apps/recorder/scripts/dev-build.sh                # build, sign, install in /Applications (not mid-recording)
```

macOS ties permissions to the app's signature. An ad-hoc signature changes with every build, so Microphone, System
Audio, Accessibility and Automation would be asked again (or silently fail) after each rebuild; the dev certificate
keeps them. It lives in a keychain of its own, so signing works over SSH too. Permissions on first run: Microphone,
System Audio Recording, notifications, and optionally Accessibility (reads the Meet window title, which carries the
call code; without it the match is by time only). Releases: `scripts/release.sh`.

## Where things live on the Mac

`~/Library/Application Support/Recordem/`: `config.json` (0600, with the sign-in tokens), `models/` (574 MB, once),
`recordings/<id>/`. A recording that wasn't sent (offline, app quit) is resumed on the next start.
