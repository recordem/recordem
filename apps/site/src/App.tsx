// The landing page (recordem.dev). One bold element, the hero: the call itself, drawn edge to edge as audio bars
// colored by who is speaking, with what they said underneath. Everything after it stays quiet.
import { Architecture } from "./Architecture";
import { CodeBlock, CodeTabs } from "./Code";
import { NoBot } from "./NoBot";
import {
  AGENT, APP, BACKEND, CALL_END, clock, DESKTOP, DOWNLOAD, GITHUB, LINES, NOTES, NOTES_AT, PLATFORMS, SELF_HOST,
  useCallTime, VOICES, type Voice,
} from "./content";
import "./styles.css";

function Wordmark() {
  return <a className="wordmark" href="/" aria-label="Recordem home"><span className="dot" aria-hidden="true" />recordem</a>;
}

// The bars of the call: one every BAR seconds, each with the voice speaking at that moment (or none) and a height
// that only depends on its index, so the server and the browser draw the same thing.
const BARS = 150;
const BAR = CALL_END / BARS;
const noise = (i: number) => {
  const x = Math.sin(i * 12.9898 + 78.233) * 43758.5453;
  return x - Math.floor(x);
};
const WAVE = Array.from({ length: BARS }, (_, i) => {
  const at = i * BAR + BAR / 2;
  const line = LINES.find((l) => at >= l.start && at < l.end);
  if (!line) return { at, voice: null as Voice | null, h: 0.05 + noise(i) * 0.05 };
  const edge = Math.min(at - line.start, line.end - at); // words swell in and out
  const env = Math.min(1, edge / 0.35);
  return { at, voice: line.voice as Voice | null, h: (0.25 + 0.75 * (0.35 + 0.65 * noise(i))) * (0.4 + 0.6 * env) };
});

function Wave({ t, height = 160, playhead = true }: { t: number; height?: number; playhead?: boolean }) {
  const w = 1600, gap = w / BARS, bw = gap * 0.56;
  return (
    <svg className="wave" viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none" aria-hidden="true" style={{ height }}>
      {WAVE.map((b, i) => {
        const bh = Math.max(3, b.h * height * 0.92);
        return (
          <rect key={i} x={i * gap + (gap - bw) / 2} y={(height - bh) / 2} width={bw} height={bh} rx={bw / 2}
            className={`${b.voice ? `b-${b.voice}` : "b-quiet"}${b.at > t ? " later" : ""}`} />
        );
      })}
      {playhead && t < CALL_END && <rect className="head" x={(t / CALL_END) * w - 1.5} y={0} width={3} height={height} />}
    </svg>
  );
}

// The band in miniature: the whole call, already said.
function MiniWave({ className = "" }: { className?: string }) {
  return <div className={`mini ${className}`}><Wave t={CALL_END} height={28} playhead={false} /></div>;
}

function Caption({ t }: { t: number }) {
  const ended = t >= CALL_END;
  if (ended && t >= NOTES_AT) {
    return (
      <div className="caption notes" aria-live="off">
        <span className="cap-who">Action items</span>
        <span className="cap-said">
          {NOTES.map((n, i) => <span key={n.who}>{i > 0 && " · "}<b className={`v-${n.voice}`}>{n.who}</b> {n.text}</span>)}
        </span>
      </div>
    );
  }
  const line = [...LINES].reverse().find((l) => l.start <= t) ?? LINES[0];
  const words = line.text.split(" ");
  const shown = t >= line.end ? words.length : Math.max(1, Math.ceil(((t - line.start) / (line.end - line.start)) * words.length));
  return (
    <div className="caption" aria-live="off">
      <span className={`cap-who v-${line.voice}`}>{VOICES.find((v) => v.id === line.voice)!.name}</span>
      <span className="cap-said">{words.slice(0, shown).join(" ")}</span>
    </div>
  );
}

function Hero() {
  const t = useCallTime("wave");
  const ended = t >= CALL_END;
  return (
    <section className="hero">
      <div className="hero-copy wrap">
        <h1>Every call, remembered.<br />No bot in the room.</h1>
        <p className="lede">
          Recordem quietly records your meetings on your Mac and remembers who said what.
          Come back to them later, ask your agent, or build on them.
        </p>
        <div className="ctas">
          <a className="btn" href={DOWNLOAD}>Download for Mac</a>
          <a className="btn ghost" href={`${GITHUB}#readme`}>Read the docs</a>
        </div>
        <p className="fine">Free for solo use. Open source. Apple silicon, macOS 14.4 or later.</p>
      </div>

      <figure className="band" id="wave" aria-label="A call being recorded: the audio, colored by who is speaking, and what they said">
        <div className="band-meta wrap">
          <span className={ended ? "rec off" : "rec"} aria-hidden="true" />
          <b>Design review</b>
          <span className="muted">{ended ? "Saved" : "Recording"}</span>
          <span className="legend">
            {VOICES.map((v) => <span key={v.id} className={`v-${v.id}`}><i />{v.name}</span>)}
          </span>
          <span className="clock">{clock(Math.min(t, CALL_END))}</span>
        </div>
        <Wave t={t} />
        <div className="wrap"><Caption t={t} /></div>
      </figure>

      <p className="platforms wrap">Works with {PLATFORMS.slice(0, -1).join(", ")} and {PLATFORMS.at(-1)}, in the browser or their apps.</p>
    </section>
  );
}

const Check = () => (
  <svg className="check" viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" /></svg>
);

export function App() {
  return (
    <div className="page">
      <a className="skip" href="#main">Skip to content</a>
      <header className="nav">
        <div className="wrap nav-in">
          <Wordmark />
          <nav aria-label="Main">
            <a href="#developers">Developers</a>
            <a href="#self-host">Self-host</a>
            <a href="#pricing">Pricing</a>
            <a href={GITHUB}>GitHub</a>
          </nav>
          <a className="btn ghost small" href={APP}>Sign in</a>
        </div>
      </header>

      <main id="main">
        <Hero />

        <section className="nobot wrap" aria-labelledby="nobot-h">
          <header className="intro">
            <h2 id="nobot-h">It listens from your computer, not from inside the call.</h2>
          </header>
          <NoBot />
        </section>

        <section className="engine wrap" aria-labelledby="engine-h">
          <header className="intro split">
            <h2 id="engine-h">A Rust engine does the hard part on the device.</h2>
            <p>
              Capture, speaker separation and transcription all happen on your Mac. The same engine ships inside the
              Desktop SDK, so your product gets everything the app does.
            </p>
          </header>
          <Architecture />
          <dl className="facts">
            <div><dt>Notes you shape</dt><dd>Summary, action items and topics from any OpenAI-compatible model, local or hosted.</dd></div>
            <div><dt>Never lost</dt><dd>A recording that can’t upload stays on the computer and resumes on its own.</dd></div>
            <div><dt>Your settings</dt><dd>Pick the languages, the speech models and the microphone. Change them whenever you want.</dd></div>
          </dl>
        </section>

        <section className="dev" id="developers" aria-labelledby="dev-h">
          <div className="wrap dev-in">
            <header className="intro">
              <h2 id="dev-h">Build on your meetings.</h2>
              <p>
                Recordem is one OpenAPI contract. The web app, the CLI and the MCP server use the same API you do, with
                typed clients for your backend and your desktop app.
              </p>
              <p><a className="link" href={`${GITHUB}/tree/main/packages/sdk`}>SDK reference</a></p>
            </header>
            <div className="editor">
              <CodeTabs tabs={[
                { label: "Your backend", note: "Create recordings with an API key, hear back by webhook.", code: BACKEND },
                { label: "Your desktop app", note: "Record your users’ calls from Electron or Node. Your API key never leaves your backend.", code: DESKTOP },
                { label: "Your agent", note: "Search what was said and decided, from any agent.", code: AGENT },
              ]} />
            </div>
          </div>
        </section>

        <section className="self wrap" id="self-host" aria-labelledby="self-h">
          <header className="intro">
            <h2 id="self-h">Run it on your own servers.</h2>
            <p>It needs Postgres and Redis, nothing else.</p>
            <dl className="licenses">
              <div><dt>Server</dt><dd>AGPL-3.0</dd></div>
              <div><dt>Recorder and SDKs</dt><dd>MIT, ship them inside your product</dd></div>
            </dl>
            <p><a className="link" href={GITHUB}>recordem/recordem on GitHub</a></p>
          </header>
          <div className="terminal">
            <div className="term-bar" aria-hidden="true"><span className="lights"><i /><i /><i /></span></div>
            <CodeBlock code={SELF_HOST} />
          </div>
        </section>

        <section className="pricing wrap" id="pricing" aria-labelledby="pricing-h">
          <header className="intro split">
            <h2 id="pricing-h">Free when it’s just you.</h2>
            <p>Use it on your own at no cost, or run the whole thing on your servers.</p>
          </header>
          <div className="plans">
            <div className="plan solo">
              <h3>Solo</h3>
              <p className="price">$0</p>
              <p className="plan-for">For one person, for as long as you like.</p>
              <ul>
                <li><Check />The Mac recorder</li>
                <li><Check />The web app, search and notes</li>
                <li><Check />The CLI and the MCP server</li>
              </ul>
              <a className="btn" href={DOWNLOAD}>Download for Mac</a>
            </div>
            <div className="plan">
              <h3>Self-hosted</h3>
              <p className="price">$0</p>
              <p className="plan-for">Everything, on your own servers.</p>
              <ul>
                <li><Check />The whole server, open source</li>
                <li><Check />Your data never leaves</li>
                <li><Check />Your own rules</li>
              </ul>
              <a className="btn ghost" href={GITHUB}>View on GitHub</a>
            </div>
          </div>
        </section>
      </main>

      <footer className="foot">
        <div className="wrap foot-in">
          <div>
            <Wordmark />
            <p>Meeting memory, without the bot.</p>
          </div>
          <nav aria-label="Footer">
            <a href={APP}>Sign in</a>
            <a href={GITHUB}>GitHub</a>
            <a href={`${GITHUB}/blob/main/LICENSING.md`}>License</a>
          </nav>
        </div>
      </footer>
    </div>
  );
}
