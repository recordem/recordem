// What every variant of the landing page says: links, code samples, and the call the hero plays back.
import { useEffect, useState } from "react";

export const GITHUB = "https://github.com/recordem/recordem";
export const DOWNLOAD = `${GITHUB}/releases/latest`;
export const APP = "https://app.recordem.dev";

export const BACKEND = `import { createClient, verifyWebhook } from "@recordem/sdk";

const recordem = createClient({
  baseUrl: "https://api.recordem.dev",
  token: process.env.RECORDEM_API_KEY,
});

// An interview is about to start: create its recording,
// and hand the desktop app a token good for that one only.
const rec = await recordem.recordings.create({
  id: crypto.randomUUID(),
  started_at: new Date().toISOString(),
  ref: "interview_123",
  upload: true,
});

// Later, a webhook: the transcript is in, each line with its speaker.
const event = await verifyWebhook(secret, body, signature);
if (event.type === "recording.uploaded") {
  const t = await recordem.recordings.getTranscript(event.recording_id);
}`;

export const DESKTOP = `import sdk from "@recordem/desktop-sdk";

await sdk.init({
  apiUrl: "https://api.recordem.dev",
  dataDir: app.getPath("userData"),
});

// Meet, Zoom or Webex started, in the browser or the app.
sdk.addEventListener("meeting-detected", async ({ window }) => {
  const { uploadToken } = await yourBackend.createRecording();
  await sdk.startRecording({ windowId: window.id, uploadToken });
});

sdk.addEventListener("realtime-event", ({ event, data }) => {
  if (event === "transcript.data") console.log(\`\${data.name}: \${data.text}\`);
});`;

export const AGENT = `# Any MCP client (Claude, ChatGPT, your editor): add it, sign in
https://mcp.recordem.dev/mcp

# Or the command line
npm i -g @recordem/cli
recordem login --server https://api.recordem.dev
recordem search "pricing decision"
recordem notes 4f1c2e9a-…`;

export const SELF_HOST = `git clone https://github.com/recordem/recordem
cd recordem && cp .env.example .env
docker compose up -d && bun install
bun run db:migrate && bun run dev`;

export const NOTES = [
  { voice: "marcus" as Voice, who: "Marcus", text: "writes the release notes tonight" },
  { voice: "ana" as Voice, who: "Ana", text: "flags anything that slips, by noon" },
];

export const PLATFORMS = ["Google Meet", "Zoom", "Webex"];

export type Voice = "you" | "ana" | "marcus";
export const VOICES: { id: Voice; name: string }[] = [
  { id: "you", name: "You" },
  { id: "ana", name: "Ana Souza" },
  { id: "marcus", name: "Marcus Lee" },
];

// Seconds of the loop.
export const LINES: { voice: Voice; start: number; end: number; text: string }[] = [
  { voice: "you", start: 0.4, end: 2.6, text: "So the export ships Thursday?" },
  { voice: "ana", start: 3.0, end: 6.4, text: "Thursday on the web. The desktop build needs one more day." },
  { voice: "marcus", start: 6.9, end: 9.0, text: "I can write the release notes tonight." },
  { voice: "you", start: 9.4, end: 11.8, text: "Perfect. Ana, can you flag anything that slips?" },
  { voice: "ana", start: 12.2, end: 14.6, text: "Will do. I'll post in the channel by noon." },
];
export const CALL_END = 15.4;
export const NOTES_AT = 16.4;
export const LOOP = 23;
const START_CLOCK = 12 * 60 + 4; // the call is 12:04 in

export const clock = (s: number) => {
  const t = Math.floor(START_CLOCK + s);
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, "0")}`;
};

// Seconds into the loop. Starts at the finished state (what the server renders), then plays while the element
// with id `elId` is on screen. Reduced motion keeps the finished state.
export function useCallTime(elId: string) {
  const [t, setT] = useState(LOOP - 0.01); // the finished state, as the server rendered it
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let raf = 0, visible = true;
    const t0 = performance.now() - 3000; // start a line in, so the box never opens empty
    const io = new IntersectionObserver(([e]) => { visible = e.isIntersecting; });
    const el = document.getElementById(elId);
    if (el) io.observe(el);
    const tick = (now: number) => {
      if (visible) setT(((now - t0) / 1000) % LOOP);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => { cancelAnimationFrame(raf); io.disconnect(); };
  }, []);
  return t;
}

