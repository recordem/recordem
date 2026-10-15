# Participants, speakers and video (plan)

Goal: name the voices of a call ("system:1" → "Ana Souza") and optionally keep per-participant video, still with no
bot in the call.

## Approach
  
  - **Who is in the call, who is speaking**: read from the meeting app's Accessibility tree, one profile per app
    (Meet in Chrome, Zoom, Teams…). No bot joins the call, and the browser is never modified.
  - **Naming the voices**: diarization turns crossed with who the app showed speaking; an LLM settles the voices the
    overlap can't, with typed evidence (`self_introduction`, `addressed_and_answers`, `elimination`,
    `vocative_exclusion`, `platform_attribution`). Only certain names are suggested; the user confirms.
  - **Video (opt-in)**: ScreenCaptureKit of the meeting window, each participant's tile cropped by its frame.
  
  ## Modules

Each one has one job, a small data interface, and is testable on Linux except where it touches macOS APIs.

```
crates/ax          macOS: snapshot an app's Accessibility tree into AxNode (role, texts, DOM classes, frame, children).
                   AxNode is plain serde data: snapshots are saved as JSON fixtures.
crates/scrape      portable: a selector language (JSON) and per-platform profiles (JSON files) that turn an AxNode
                   into {participants: [{name, speaking, muted, frame}], self_name, title}. Pure functions.
crates/timeline    portable: scrape samples → speaking intervals per name; diarization turns × intervals → a
                   suggested name per voice, with the overlap that justifies it.
crates/video       macOS: ScreenCaptureKit stream of the meeting window; crops each participant's frame from it.
crates/probe       CLI over all of the above: dump, watch, video. The PoC; the app links the same crates later.
```

Profiles are data, not code, so a Meet or Zoom redesign is a JSON change. Later the server can serve them
(`GET /v1/recorder/profiles`, contract first) so recorders pick up fixes without a release.

Fixtures: every profile is tested against real AX snapshots taken with `probe dump` during test calls
(`crates/scrape/fixtures/<platform>/*.json`). A redesign shows up as a failing fixture, not a silent wrong name.

## Into the product (after the PoC)

1. Recorder writes `speakers.jsonl` (`{t_ms, participants}`) next to `mic.wav`/`system.wav`.
2. After the call, `timeline` maps each `system:N` to a name by overlap; the local user is `mic`.
3. Contract: participants of a recording and a name per speaker label, with `source` (`platform`, `llm`, `user`)
   and `confidence`. Worker: the LLM tie-break (the evidence types above) for voices the overlap can't settle.
4. Video, opt-in: window recording plus per-participant crops, uploaded as extra tracks.

## Status

- [x] Meet in Chrome (pt-BR): tile = DOM class `oZRSLe`, name in `XEazBc`, speaking = `kssMZb` on the audio indicator
      (and `FTMc0c` = silent), measured over a scripted 40 s call; fixtures in `crates/scrape/fixtures/meet-chrome/`.
      The classes are Google's obfuscated names: expect them to change, which is what the fixtures will catch.
- [x] `timeline` on that call (diarization simulated by the script's phases): both voices named, shares 98–100%
- [x] `probe video` on that call: 3840×1960 window, each tile cropped to its own mp4 (`crops.sh`, ffmpeg)
- [x] Fallbacks: profiles list tile rules in order of trust. Meet's second rule is structural (a group in the page's
      `main` landmark with exactly one name in it), so a class rename still finds people and names; speaking has no
      non-class signal in Meet, which is what the health monitor is for.
- [x] Health: every scrape says `healthy` / `degraded` / `broken` with its issues (`no_participants`,
      `fallback_tiles`, `not_in_roster`, `truncated`); `Monitor` adds `speaking_signal_lost` (the other side audible
      30 s, nobody shown speaking), asks for a fresh reader after 10 s broken, and reports each issue once per call
      (profile id + revision) — what the server needs to know a profile broke.
- [x] Versions: profiles carry `revision` and `app_versions {min, max}`; a server document (`{"profiles": […]}`)
      replaces older revisions and adds version-specific ones, which are tried first. Recorder reads the app's
      `CFBundleShortVersionString`.
- [x] Roster: Meet's People panel (when open) gives every name, who is "(Você)/(You)", muted; tile names missing
      from it are flagged; people without a tile are listed without a crop.
- [x] Languages/layouts in Zoom: role description in 17 languages, immersive view (`AXUnknown` descriptors), the
      "Talking:" banner (synthetic tests).
- [x] Cost: the snapshot reads only the call window and skips the browser's tabs/toolbar/menus. Chrome with no call:
      1202 elements in 150 ms → 3 elements in 0.7 ms; with a call open, 925 elements in 155 ms → 155 in 12 ms.
- [x] Meet layouts, live (auto, mosaic, spotlight, sidebar, presenting, someone else presenting), 50 snapshots
      over 150 s: 43 with everyone, 3 while the layout dialog was open (modal: Chrome hides the page; reported as
      `obstructed`, never as broken), 4 after one person left. While presenting, Meet adds a floating window with
      the tiles: windows are merged. Presentation tiles ("(apresentação)") are not people.
- [x] Where the call is:
      - another tab: Chrome opens Meet's picture-in-picture window; it is read and recorded instead (segments).
      - picture-in-picture closed: only the tab strip (read outside the call window) shows the call → `not_visible`;
        the recorded window shows another page → blacked out in the video, no crops.
      - another Space, or minimised: Accessibility lists windows of the active Space only; the window server lists
        them all (`CGWindowListCopyWindowInfo`, titles need Screen Recording) → `not_visible`, but the window still
        shows the call and ScreenCaptureKit still records it: video kept, crops hold their last place.
      - the recording follows the call: a new segment (file) when the call shows in another window or the recorded
        one closes (picture-in-picture does when the person returns to the tab).
      Live: 80 s, tab → PiP → PiP closed → other Space → back: 5 segments, names throughout except while unreadable.
- [ ] Meet in English: only the roster words and "(You)" differ; untested live
- [ ] Zoom (no real fixture yet), Teams, Slack huddle: need the apps and a call
- [ ] Server side: `GET /v1/recorder/profiles` and a health report route (contract first)
- [ ] First seconds of a capture sometimes scrape nothing (seen once, 7.7 s): to explain
- [ ] Video only sees what the window shows: hidden/minimised windows and off-screen tiles give no crop

Testing on a Mac over ssh: Accessibility works; ScreenCaptureKit only runs in the GUI session (start the probe
through Terminal.app, `open -a Terminal script.command`), and a CLI must touch CoreGraphics first.
