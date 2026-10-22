// The timeline: a month calendar (days with meetings marked) next to the meetings of the month or of the picked day.
import type { Meeting } from "@recordem/sdk";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "./api";
import { signOut } from "./auth";
import { callApp, day, dayKey, duration, monthTitle, time } from "./format";
import { link } from "./router";

async function monthMeetings(month: Date): Promise<Meeting[]> {
  const from = new Date(month.getFullYear(), month.getMonth(), 1);
  const to = new Date(month.getFullYear(), month.getMonth() + 1, 1);
  const out: Meeting[] = [];
  let cursor: string | undefined;
  do {
    const data = await api.meetings.list({ from: from.toISOString(), to: to.toISOString(), limit: 200, cursor });
    out.push(...data.items);
    cursor = data.next ?? undefined;
  } while (cursor);
  return out;
}

export function Home() {
  const [month, setMonth] = useState(() => { const d = new Date(); return new Date(d.getFullYear(), d.getMonth(), 1); });
  const [picked, setPicked] = useState<string | null>(null);
  const [items, setItems] = useState<Meeting[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState<Set<string>>(new Set());

  const load = useCallback(() => { monthMeetings(month).then(setItems, (e: Error) => setError(e.message)); }, [month]);
  useEffect(() => { setItems(null); load(); }, [load]);

  // New recordings appear, and meetings being recorded show as live, without reloading.
  useEffect(() => {
    const ctl = new AbortController();
    void api.stream({
      signal: ctl.signal,
      onOpen: load,
      onEvent: (e) => {
        if (e.type === "transcript.live") setLive((s) => (s.has(e.meeting_id) ? s : new Set(s).add(e.meeting_id)));
        if (e.type === "recording.ended" || e.type === "transcript.final") setLive((s) => { const n = new Set(s); n.delete(e.meeting_id); return n; });
        if (e.type === "recording.created" || e.type === "transcript.final" || e.type === "recording.deleted" || e.type === "meeting.deleted") load();
      },
    });
    return () => ctl.abort();
  }, [load]);

  const byDay = useMemo(() => {
    const m = new Map<string, Meeting[]>();
    for (const x of items ?? []) {
      const k = dayKey(new Date(x.starts_at));
      m.set(k, [...(m.get(k) ?? []), x]);
    }
    return m;
  }, [items]);

  const days = [...byDay.keys()].filter((k) => !picked || k === picked);

  return (
    <div className="shell">
      <header className="top">
        <a className="brand" {...link("/")}><span className="dot" /> Recordem</a>
        <nav className="top-actions">
          <a className="muted small" {...link("/settings")}>Settings</a>
          <button className="ghost small" onClick={signOut}>Sign out</button>
        </nav>
      </header>
      <div className="home">
        <aside>
          <Calendar month={month} counts={byDay} picked={picked}
            onMonth={(d) => { setMonth(d); setPicked(null); }}
            onPick={(k) => setPicked((p) => (p === k ? null : k))} />
        </aside>
        <main>
          {error && <p className="error">{error}</p>}
          {!items && !error && <p className="muted">Loading…</p>}
          {items && !days.length && (
            <div className="empty">
              <p>No meetings {picked ? "on this day" : `in ${monthTitle(month)}`}.</p>
              <p className="muted">Recordings from the desktop recorder show up here as soon as a call starts.</p>
            </div>
          )}
          {days.map((k) => (
            <section key={k} className="day">
              <h2>{day(new Date(`${k}T12:00:00`))}</h2>
              <ul className="meetings">
                {byDay.get(k)!.map((m) => (
                  <li key={m.id}>
                    <a {...link(`/m/${m.id}`)}>
                      <span className="when">{time(m.starts_at)}</span>
                      <span className="what">
                        <b>{m.title || "Untitled meeting"}</b>
                        <span className="muted">{[callApp(m.call_key), duration(m.starts_at, m.ends_at)].filter(Boolean).join(" · ")}</span>
                      </span>
                      {live.has(m.id) && <span className="badge live">● Live</span>}
                    </a>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </main>
      </div>
    </div>
  );
}

function Calendar(p: { month: Date; counts: Map<string, Meeting[]>; picked: string | null; onMonth: (d: Date) => void; onPick: (k: string) => void }) {
  const first = p.month;
  const lead = (first.getDay() + 6) % 7;                 // weeks start on Monday
  const total = new Date(first.getFullYear(), first.getMonth() + 1, 0).getDate();
  const today = dayKey(new Date());
  const cells = [...Array(lead).fill(null), ...Array.from({ length: total }, (_, i) => new Date(first.getFullYear(), first.getMonth(), i + 1))];
  const shift = (n: number) => p.onMonth(new Date(first.getFullYear(), first.getMonth() + n, 1));
  return (
    <div className="calendar">
      <div className="cal-head">
        <button className="ghost small" onClick={() => shift(-1)} aria-label="Previous month">‹</button>
        <b>{monthTitle(first)}</b>
        <button className="ghost small" onClick={() => shift(1)} aria-label="Next month">›</button>
      </div>
      <div className="grid">
        {["M", "T", "W", "T", "F", "S", "S"].map((d, i) => <span key={i} className="dow">{d}</span>)}
        {cells.map((d, i) => {
          if (!d) return <span key={i} />;
          const k = dayKey(d), n = p.counts.get(k)?.length ?? 0;
          return (
            <button key={i} className={`cell${n ? " has" : ""}${k === today ? " today" : ""}${k === p.picked ? " picked" : ""}`}
              disabled={!n} onClick={() => p.onPick(k)} title={n ? `${n} meeting${n > 1 ? "s" : ""}` : undefined}>
              {d.getDate()}
              {n > 0 && <i />}
            </button>
          );
        })}
      </div>
    </div>
  );
}
