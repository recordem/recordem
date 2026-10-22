// The same call, with a meeting bot and with Recordem. It alternates on its own while on screen, until someone picks
// a side; with reduced motion it waits for a click. The server renders the bot side.
import { useEffect, useRef, useState } from "react";

const POINTS = {
  bot: [
    "A stranger called “Notetaker” joins, and everyone sees it.",
    "Hosts have to let it in. Locked meetings turn it away.",
    "Your audio is shipped to someone else to be transcribed.",
  ],
  ours: [
    "Nothing joins the call. The recorder notices it on your Mac and starts.",
    "No one has to let it in. Works with Meet, Zoom and Webex, in the browser or their apps.",
    "Your audio stays on your Mac. Only the transcript goes to your server.",
  ],
};

const Mark = ({ bot }: { bot: boolean }) => bot
  ? <span className="mark dash" aria-hidden="true" />
  : (
    <svg className="mark" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );

export function NoBot() {
  const [bot, setBot] = useState(true);
  const picked = useRef(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let visible = false;
    const io = new IntersectionObserver(([e]) => { visible = e.isIntersecting; }, { threshold: 0.4 });
    if (ref.current) io.observe(ref.current);
    const id = setInterval(() => { if (visible && !picked.current) setBot((b) => !b); }, 3600);
    return () => { clearInterval(id); io.disconnect(); };
  }, []);

  const pick = (b: boolean) => { picked.current = true; setBot(b); };

  return (
    <div className={bot ? "nobot-demo is-bot" : "nobot-demo"} ref={ref}>
      <div className="call-window" aria-hidden="true">
        <div className="cw-bar">
          <b>Design review</b>
          <span className={bot ? "cw-rec bot" : "cw-rec"}><i />{bot ? "Notetaker is recording" : "Recording on your Mac"}</span>
        </div>
        <div className="cw-tiles">
          <div className="cw-tile v-ana"><span>AS</span><em>Ana Souza</em></div>
          <div className="cw-tile v-marcus"><span>ML</span><em>Marcus Lee</em></div>
          <div className="cw-tile v-you"><span>You</span><em>You</em></div>
          <div className="cw-tile bot"><span>N</span><em>Notetaker</em></div>
        </div>
      </div>

      <div className="nobot-side">
        <div className="seg" role="tablist" aria-label="Compare">
          <button role="tab" aria-selected={bot} onClick={() => pick(true)}>With a bot</button>
          <button role="tab" aria-selected={!bot} onClick={() => pick(false)}>With Recordem</button>
        </div>
        {/* Both lists sit in the same cell, so the block keeps the height of the longer one. */}
        <div className="points">
          {([true, false] as const).map((side) => (
            <ul key={String(side)} className={side ? "bot" : "ours"} aria-hidden={side !== bot} data-on={side === bot}>
              {POINTS[side ? "bot" : "ours"].map((p) => <li key={p}><Mark bot={side} />{p}</li>)}
            </ul>
          ))}
        </div>
      </div>
    </div>
  );
}
