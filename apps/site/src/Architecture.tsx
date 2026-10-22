// How a call travels: captured and transcribed on the Mac by the Rust engine, then sent to the server, where every
// client reads it. The steps light up in order on a loop (CSS only, see .arch in styles.css); with reduced motion the
// diagram stands still and reads the same.

function Node({ step, title, note, className = "" }: { step: number; title: string; note: string; className?: string }) {
  return (
    <div className={`arch-node ${className}`} style={{ "--step": step } as React.CSSProperties}>
      <b>{title}</b>
      <span>{note}</span>
    </div>
  );
}

function Flow({ step, label, className = "" }: { step: number; label?: string; className?: string }) {
  return (
    <div className={`arch-flow ${className}`} style={{ "--step": step } as React.CSSProperties} aria-hidden="true">
      <i />
      {label && <em>{label}</em>}
    </div>
  );
}

// The engine at work: the call's sound as bars, colored by who is speaking, becoming lines of transcript. The bar
// heights come from the index only, so the server and the browser draw the same thing.
const BARS = Array.from({ length: 26 }, (_, i) => ({
  voice: i < 8 ? "you" : i < 18 ? "ana" : "marcus",
  h: 0.3 + 0.7 * Math.abs(Math.sin(i * 1.7) * Math.cos(i * 0.6)),
}));
const SAID = [
  { voice: "you", who: "You", text: "So the export ships Thursday?" },
  { voice: "ana", who: "Ana Souza", text: "Thursday on the web, Friday on desktop." },
  { voice: "marcus", who: "Marcus Lee", text: "I’ll write the release notes." },
];

function Voice() {
  return (
    <div className="arch-voice" style={{ "--step": 1 } as React.CSSProperties}>
      <div className="arch-bars" aria-hidden="true">
        {BARS.map((b, i) => (
          <i key={i} className={`v-${b.voice}`} style={{ "--h": b.h.toFixed(2), "--i": i } as React.CSSProperties} />
        ))}
      </div>
      <ol className="arch-said">
        {SAID.map((l, i) => (
          <li key={l.who} className={`v-${l.voice}`} style={{ "--n": i } as React.CSSProperties}>
            <b>{l.who}</b><span>{l.text}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

export function Architecture() {
  return (
    <figure className="arch" aria-label="How a call travels through Recordem">
      <div className="arch-mac">
        <header className="arch-head">
          <span className="arch-chip"><span className="dot" aria-hidden="true" />Your Mac</span>
          <span className="arch-sub">recordem-engine, in Rust</span>
        </header>

        <div className="arch-row arch-in">
          <Node step={0} title="Microphone" note="You, with echo cancelled" />
          <Node step={0} title="Call audio" note="Everyone else, from the system" />
          <Node step={0} title="Call window" note="The names of who is in it" />
        </div>

        <Flow step={1} className="v" />

        <Voice />
      </div>

      <Flow step={3} label="Transcript, then audio" className="h" />

      <Node step={4} className="arch-server" title="Your Recordem server" note="Meetings, transcripts, notes and search. Ours or yours." />

      <Flow step={5} className="h" />

      <div className="arch-out">
        <Node step={6} title="Web app" note="Read and search" />
        <Node step={6} title="CLI and MCP" note="Your agents" />
        <Node step={6} title="Webhooks" note="Your backend" />
      </div>
    </figure>
  );
}
