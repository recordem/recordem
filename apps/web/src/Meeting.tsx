// A meeting: its recording's transcript with names on the voices, a player where a line's time seeks to it, and,
// while the call goes on, the transcript growing live over the event stream.
import type { MeetingDetail, Recording, Segment, Transcript } from "@recordem/sdk";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import { callApp, clock, day, duration, speakerName, time } from "./format";
import { link, navigate } from "./router";

export function MeetingPage({ id }: { id: string }) {
  const [meeting, setMeeting] = useState<MeetingDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pick, setPick] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);

  const load = useCallback(async () => {
    const { data, error } = await api.GET("/v1/meetings/{id}", { params: { path: { id } } });
    if (error) setError(error.message); else setMeeting(data);
  }, [id]);
  useEffect(() => { void load(); }, [load]);

  // A recording linked to the wrong meeting becomes a meeting of its own; the page follows it.
  const split = async (recordingId: string) => {
    const { data, error } = await api.POST("/v1/recordings/{id}/move", { params: { path: { id: recordingId } }, body: { meeting_id: null } });
    if (error) setError(error.message); else navigate(`/m/${data.meeting_id}`);
  };

  const removeRecording = async (recordingId: string) => {
    if (!confirm("Delete this recording, its transcript and audio? This can't be undone.")) return;
    const { error } = await api.DELETE("/v1/recordings/{id}", { params: { path: { id: recordingId } } });
    if (error) setError(error.message); else { setPick(null); void load(); }
  };
  const removeMeeting = async () => {
    const n = meeting?.recordings.length ?? 0;
    if (!confirm(`Delete this meeting${n ? ` and its ${n > 1 ? `${n} recordings` : "recording"} (transcripts and audio)` : ""}? This can't be undone.`)) return;
    const { error } = await api.DELETE("/v1/meetings/{id}", { params: { path: { id } } });
    if (error) setError(error.message); else navigate("/");
  };

  if (error) return <Frame><p className="error">{error}</p></Frame>;
  if (!meeting) return <Frame><p className="muted">Loading…</p></Frame>;
  const rec = meeting.recordings.find((r) => r.id === pick) ?? meeting.recordings.at(-1);

  return (
    <Frame>
      <div className="meeting-head">
        <div className="title-row">
          <Title meeting={meeting} editing={renaming} setEditing={setRenaming} onSaved={(m) => setMeeting({ ...meeting, ...m })} />
          <Menu label="Meeting actions">
            {(close) => (
              <>
                <button role="menuitem" onClick={() => { close(); setRenaming(true); }}>Rename</button>
                {meeting.recordings.length > 1 && rec && (
                  <>
                    <hr />
                    <p className="menu-label">Recording at {time(rec.started_at)}</p>
                    <button role="menuitem" onClick={() => { close(); void split(rec.id); }}>Move to its own meeting</button>
                    <button role="menuitem" className="danger" onClick={() => { close(); void removeRecording(rec.id); }}>Delete this recording…</button>
                  </>
                )}
                <hr />
                <button role="menuitem" className="danger" onClick={() => { close(); void removeMeeting(); }}>Delete meeting…</button>
              </>
            )}
          </Menu>
        </div>
        <p className="muted">
          {[day(new Date(meeting.starts_at)), time(meeting.starts_at), duration(meeting.starts_at, meeting.ends_at), callApp(meeting.call_key)].filter(Boolean).join(" · ")}
        </p>
        {meeting.recordings.length > 1 && rec && (
          <div className="segmented" role="tablist" aria-label="Recordings">
            {meeting.recordings.map((r) => (
              <button key={r.id} role="tab" aria-selected={r.id === rec.id} onClick={() => setPick(r.id)}>{time(r.started_at)}</button>
            ))}
          </div>
        )}
      </div>
      {rec ? <RecordingView key={rec.id} rec={rec} onChange={load} /> : <p className="muted">No recording for this meeting yet.</p>}
    </Frame>
  );
}

/** A "⋯" button and the menu it opens; closes on a choice, a click outside or Escape. */
function Menu({ label, children }: { label: string; children: (close: () => void) => React.ReactNode }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!box.current?.contains(e.target as Node)) setOpen(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", esc); };
  }, [open]);
  return (
    <div className="menu" ref={box}>
      <button className="ghost icon" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>⋯</button>
      {open && <div className="menu-pop" role="menu">{children(() => setOpen(false))}</div>}
    </div>
  );
}

/** The title, edited in place: click it (or "Rename"), type, Enter (or leave the field) saves, Escape cancels. */
function Title({ meeting, editing, setEditing, onSaved }: {
  meeting: MeetingDetail; editing: boolean; setEditing: (on: boolean) => void; onSaved: (m: Partial<MeetingDetail>) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const save = async (value: string) => {
    setEditing(false);
    const title = value.trim() || null;
    if (title === meeting.title) return;
    const { data, error } = await api.PATCH("/v1/meetings/{id}", { params: { path: { id: meeting.id } }, body: { title } });
    if (error) setError(error.message); else { setError(null); onSaved({ title: data.title }); }
  };
  if (editing) {
    return (
      <input className="title-input" autoFocus defaultValue={meeting.title ?? ""} placeholder="Untitled meeting" maxLength={200}
        onBlur={(e) => save(e.currentTarget.value)}
        onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); if (e.key === "Escape") setEditing(false); }} />
    );
  }
  return (
    <>
      <h1 className="title" title="Rename" onClick={() => setEditing(true)}>{meeting.title || <span className="muted">Untitled meeting</span>}</h1>
      {error && <p className="error small">{error}</p>}
    </>
  );
}

function Frame({ children }: { children: React.ReactNode }) {
  return (
    <div className="shell">
      <header className="top">
        <a className="brand" {...link("/")}><span className="dot" /> Recordem</a>
        <a className="muted small" {...link("/")}>← All meetings</a>
      </header>
      <main className="meeting">{children}</main>
    </div>
  );
}

function RecordingView({ rec, onChange }: { rec: Recording; onChange: () => void }) {
  const [t, setT] = useState<Transcript | null>(null);
  const [missing, setMissing] = useState(false);
  // The whole call by default; tracks can arrive after the page opened (the upload follows the transcript).
  const [chosen, setTrack] = useState<"mix" | "mic_hq" | null>(null);
  const track = chosen ?? (rec.tracks.some((x) => x.kind === "mix") ? "mix" : rec.tracks[0]?.kind ?? null);
  const [src, setSrc] = useState<string | null>(null);
  const [now, setNow] = useState(-1);
  const [stopping, setStopping] = useState(false);
  const audio = useRef<HTMLAudioElement>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const live = !rec.ended_at || rec.transcript === "live";

  const loadTranscript = useCallback(async () => {
    const { data, response } = await api.GET("/v1/recordings/{id}/transcript", { params: { path: { id: rec.id } } });
    if (data) { setT(data); setMissing(false); } else if (response.status === 404) setMissing(true);
  }, [rec.id]);
  useEffect(() => { void loadTranscript(); }, [loadTranscript]);

  useEffect(() => {
    if (!track) return;
    void api.GET("/v1/recordings/{id}/tracks/{kind}/url", { params: { path: { id: rec.id, kind: track } } }).then(({ data }) => data && setSrc(data.url));
  }, [rec.id, track]);

  // Live: segments arrive in batches from index `from`; the final transcript and the audio replace the draft.
  useEffect(() => {
    if (!live) return;
    const ctl = new AbortController();
    void api.stream({
      recordingId: rec.id, signal: ctl.signal, onOpen: loadTranscript,
      onEvent: (e) => {
        if (e.type === "transcript.live") {
          const { from, segments } = e.data as { from: number; segments: Segment[] };
          setT((cur) => ({ recording_id: rec.id, status: "live", language: cur?.language ?? null, model: cur?.model ?? null,
            segments: [...(cur?.segments ?? []).slice(0, from), ...segments] }));
          setMissing(false);
        }
        if (e.type === "transcript.final" || e.type === "recording.ended" || e.type === "track.ready") { void loadTranscript(); onChange(); }
      },
    });
    return () => ctl.abort();
  }, [live, rec.id, loadTranscript, onChange]);

  useEffect(() => { if (live) bottom.current?.scrollIntoView({ block: "end", behavior: "smooth" }); }, [live, t?.segments.length]);

  const segs = t?.segments ?? [];
  const seek = (s: Segment) => { const a = audio.current; if (!a) return; a.currentTime = s.start_ms / 1000; void a.play(); };
  const onTime = () => {
    const ms = (audio.current?.currentTime ?? 0) * 1000;
    setNow(segs.findLastIndex((s) => s.start_ms <= ms));
  };
  const stop = async () => { setStopping(true); await api.POST("/v1/recordings/{id}/stop", { params: { path: { id: rec.id } } }); };

  return (
    <>
      {live && (
        <div className="live-bar">
          <span className="badge live">● Recording</span>
          <span className="muted">The transcript is a draft until the call ends.</span>
          {!rec.ended_at && <button className="ghost small" disabled={stopping} onClick={stop}>{stopping ? "Asked to stop…" : "Stop recording"}</button>}
        </div>
      )}
      {src && (
        <div className="player">
          <audio ref={audio} src={src} controls preload="metadata" onTimeUpdate={onTime} />
          {rec.tracks.length > 1 && (
            <select value={track ?? ""} onChange={(e) => setTrack(e.target.value as "mix" | "mic_hq")} aria-label="Audio track">
              {rec.tracks.map((x) => <option key={x.kind} value={x.kind}>{x.kind === "mix" ? "Whole call" : "My voice (high quality)"}</option>)}
            </select>
          )}
        </div>
      )}
      {missing && !live && <p className="muted">No transcript for this recording.</p>}
      <ol className="transcript">
        {segs.map((s, i) => {
          const who = speakerName(s, segs);
          const same = i > 0 && speakerName(segs[i - 1], segs) === who;
          return (
            <li key={i} className={`${s.channel}${i === now ? " now" : ""}${same ? " cont" : ""}`}>
              {!same && <span className="who">{who}</span>}
              <button className="ts" onClick={() => seek(s)} disabled={!src} title="Play from here">{clock(s.start_ms)}</button>
              <p>{s.text}</p>
            </li>
          );
        })}
      </ol>
      <div ref={bottom} />
    </>
  );
}
