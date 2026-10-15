// A meeting: its recording's transcript with names on the voices, a player where a line's time seeks to it, and,
// while the call goes on, the transcript growing live over the event stream.
import type { MeetingDetail, Recording, Segment, Transcript } from "@recordem/sdk";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import { callApp, clock, day, duration, speakerName, time } from "./format";
import { link } from "./router";

export function MeetingPage({ id }: { id: string }) {
  const [meeting, setMeeting] = useState<MeetingDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pick, setPick] = useState<string | null>(null);

  const load = useCallback(async () => {
    const { data, error } = await api.GET("/v1/meetings/{id}", { params: { path: { id } } });
    if (error) setError(error.message); else setMeeting(data);
  }, [id]);
  useEffect(() => { void load(); }, [load]);

  if (error) return <Frame><p className="error">{error}</p></Frame>;
  if (!meeting) return <Frame><p className="muted">Loading…</p></Frame>;
  const rec = meeting.recordings.find((r) => r.id === pick) ?? meeting.recordings.at(-1);

  return (
    <Frame>
      <div className="meeting-head">
        <h1>{meeting.title || "Untitled meeting"}</h1>
        <p className="muted">
          {[day(new Date(meeting.starts_at)), time(meeting.starts_at), duration(meeting.starts_at, meeting.ends_at), callApp(meeting.call_key)].filter(Boolean).join(" · ")}
        </p>
        {meeting.recordings.length > 1 && (
          <div className="tabs">
            {meeting.recordings.map((r, i) => (
              <button key={r.id} className={r.id === rec?.id ? "" : "ghost"} onClick={() => setPick(r.id)}>Recording {i + 1}</button>
            ))}
          </div>
        )}
      </div>
      {rec ? <RecordingView key={rec.id} rec={rec} onChange={load} /> : <p className="muted">No recording for this meeting yet.</p>}
    </Frame>
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
