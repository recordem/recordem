// The recorder's window. All state lives in Rust (src-tauri/src/lib.rs); this polls `status` and renders the phase.
const { invoke } = window.__TAURI__.core;
const $view = document.getElementById("view");
const $foot = document.getElementById("foot");
let showSettings = false;
let last = "";       // the last rendered phase, so typing in a form isn't wiped by the poll
let selected = null; // the meeting id picked in the "detected" list ("" = a new meeting)

const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const time = (iso) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const mb = (b) => `${Math.round(b / 1e6)} MB`;
const clock = (from) => {
  const s = Math.max(0, Math.floor((Date.now() - new Date(from).getTime()) / 1000));
  return [Math.floor(s / 3600), Math.floor(s / 60) % 60, s % 60].map((n) => String(n).padStart(2, "0")).join(":");
};
const run = async (cmd, args) => {
  try { await invoke(cmd, args); } catch (e) { alert(e); }
  tick(true);
};

document.getElementById("gear").onclick = () => { showSettings = !showSettings; tick(true); };

async function settings(first) {
  const c = await invoke("get_config");
  const addresses = `
    <details class="small" ${c.api_url ? "" : "open"}><summary class="muted">Server addresses</summary>
      <label class="small">Portal<input id="web" type="text" value="${esc(c.web_url)}"></label>
      <label class="small">API<input id="api" type="text" value="${esc(c.api_url)}"></label>
      <label class="small">Sign-in<input id="auth" type="text" value="${esc(c.auth_url)}"></label>
    </details>`;
  const val = (id) => document.getElementById(id).value;
  if (!c.signed_in) {
    $view.innerHTML = `
      <h2>Sign in to Recordem</h2>
      <p class="muted">Your browser opens the sign-in and comes back here.</p>
      <button class="btn" id="login">Sign in</button>
      <p id="err" class="error" style="margin-top:10px"></p>
      ${addresses}`;
    document.getElementById("login").onclick = async () => {
      const err = document.getElementById("err");
      err.textContent = "";
      try {
        await invoke("login", { apiUrl: val("api"), authUrl: val("auth"), webUrl: val("web") });
        err.style.color = "var(--muted)";
        err.textContent = "Finish signing in in your browser…";
      } catch (e) { err.style.color = ""; err.textContent = e; }
    };
    return;
  }
  $view.innerHTML = `
    <h2>Settings</h2>
    <p class="small">Account: <b>${esc(c.account || "connected")}</b> · <a href="#" id="logout">Sign out</a></p>
    <div class="small">Languages of your calls <span class="muted">(each turn is transcribed in the one it sounds like)</span>
      <div class="row" style="justify-content:flex-start;gap:14px;margin:4px 0 10px">${[["en", "English"], ["pt", "Portuguese"], ["es", "Spanish"]]
        .map(([v, l]) => `<label class="opt"><input type="checkbox" name="lang" value="${v}" ${c.languages.includes(v) ? "checked" : ""}><span>${l}</span></label>`).join("")}</div></div>
    <label class="small">When a call starts
      <select id="oncall">${[["ask", "Ask whether to record"], ["auto", "Record by itself when it matches a meeting"], ["off", "Do nothing"]]
        .map(([v, l]) => `<option value="${v}" ${c.on_call === v ? "selected" : ""}>${l}</option>`).join("")}</select></label>
    <label class="small">Microphone
      <select id="mic"><option value="">System default</option>${c.mics
        .map((m) => `<option value="${esc(m)}" ${c.mic_device === m ? "selected" : ""}>${esc(m)}</option>`).join("")}</select></label>
    <label class="small">Extra vocabulary <span class="muted">(one term per line; stays on this Mac)</span>
      <textarea id="vocab" rows="3" placeholder="Kubernetes&#10;OKR&#10;due diligence">${esc(c.vocabulary)}</textarea></label>
    <label class="opt small"><input id="live" type="checkbox" ${c.live ? "checked" : ""}>
      <span>Transcribe during the call too (live, in the portal)</span></label>
    <label class="opt small"><input id="hq" type="checkbox" ${c.voice_hq ? "checked" : ""}>
      <span>Also keep my voice in high quality (48 kHz), to listen back to</span></label>
    <label class="opt small"><input id="atlogin" type="checkbox" ${c.launch_at_login ? "checked" : ""}>
      <span>Open when I log in to this Mac</span></label>
    <label class="opt small" style="margin:6px 0 12px"><input id="keep" type="checkbox" ${c.keep_audio ? "checked" : ""}>
      <span>Keep a copy of the audio on this Mac too</span></label>
    ${addresses}
    <p id="err" class="error"></p>
    <div class="row"><button class="btn ghost" id="cancel">Back</button><button class="btn" id="save">Save</button></div>`;
  document.getElementById("logout").onclick = async (e) => {
    e.preventDefault();
    if (confirm("Sign out on this Mac?")) { await invoke("logout"); showSettings = false; tick(true); }
  };
  document.getElementById("save").onclick = async () => {
    const languages = [...document.querySelectorAll('input[name="lang"]:checked')].map((i) => i.value);
    if (!languages.length) { document.getElementById("err").textContent = "Pick at least one language."; return; }
    const on = (id) => document.getElementById(id).checked;
    try {
      await invoke("save_config", { input: {
        api_url: val("api"), auth_url: val("auth"), web_url: val("web"), languages, keep_audio: on("keep"), voice_hq: on("hq"),
        launch_at_login: on("atlogin"), on_call: val("oncall"), mic_device: val("mic") || null, vocabulary: val("vocab"), live: on("live") } });
      showSettings = false;
      tick(true);
    } catch (e) { document.getElementById("err").textContent = e; }
  };
  document.getElementById("cancel").onclick = () => { showSettings = false; tick(true); };
}

function render(s) {
  switch (s.phase) {
    case "models": {
      const pct = s.total ? Math.round((s.done / s.total) * 100) : 0;
      const busy = s.done > 0 && !s.error;
      return `
        <h2>Transcription model</h2>
        <p class="muted">Transcription runs on this Mac. First I download Whisper (${mb(s.total)}), once.</p>
        ${busy ? `<div class="bar"><i style="width:${pct}%"></i></div><p class="small muted">${mb(s.done)} of ${mb(s.total)}</p>` : ""}
        ${s.error ? `<p class="error">${esc(s.error)}</p>` : ""}
        ${busy ? "" : `<button class="btn" data-cmd="download_models">Download</button>`}`;
    }
    case "idle":
      return `
        <h2>Waiting for a call</h2>
        <p class="muted">When Meet, Zoom or Teams takes the microphone, I'll ask whether to record.</p>
        <button class="btn ghost" data-cmd="lookup">Record now</button>`;
    case "detected": {
      const m = s.matches;
      if (selected === null && m?.length) selected = m[0].meeting_id;
      const list = m === null ? `<p class="muted">Looking this call up…</p>` : [...m.map((x) => `
        <label class="opt card"><input type="radio" name="meeting" value="${esc(x.meeting_id)}" ${selected === x.meeting_id ? "checked" : ""}>
          <span><strong>${esc(x.title || "Untitled meeting")}</strong>
          <br><span class="small muted">${time(x.starts_at)}</span>
          ${x.matched_by === "link" ? `<span class="chip">same link</span>` : ""}</span></label>`),
        `<label class="opt card"><input type="radio" name="meeting" value="" ${selected === "" || !m.length ? "checked" : ""}>
          <span><strong>A new meeting</strong><br><span class="small muted">${esc(s.call.title || "Named after the call window")}</span></span></label>`].join("");
      return `
        <h2>${s.call.app_name ? `Call in ${esc(s.call.app_name)}` : "New recording"}</h2>
        ${s.error ? `<p class="error">${esc(s.error)}</p>` : `<p class="muted small">Which meeting is this?</p>`}
        ${list}
        <div class="notice">Let the others know the call will be recorded and transcribed.</div>
        <div class="row"><button class="btn ghost" data-cmd="dismiss">Not now</button><button class="btn rec" id="go" ${m === null ? "disabled" : ""}>Record</button></div>`;
    }
    case "recording":
      return `
        <h2><span class="dot"></span>Recording</h2>
        <p class="muted">${esc(s.title ?? "Untitled meeting")}${s.app_name ? ` · ${esc(s.app_name)}` : ""}</p>
        <div class="timer" data-since="${esc(s.started_at)}">${clock(s.started_at)}</div>
        ${s.muted ? `<div class="notice">Microphone muted: your voice isn't being recorded or transcribed.</div>` : `<p class="small muted">Stops by itself when the call ends.</p>`}
        <div class="row">${s.live ? `<button class="btn ghost" data-cmd="open_live">Open live</button>` : ""}<button class="btn ghost" data-cmd="toggle_mute">${s.muted ? "Unmute" : "Mute"}</button><button class="btn" data-cmd="stop">Stop</button></div>`;
    case "processing":
      return `
        <h2>${s.step === "sending" ? "Sending" : "Transcribing"}</h2>
        <p class="muted">${esc(s.title ?? "Untitled meeting")}</p>
        <div class="bar"><i style="width:${Math.round(s.progress / 10)}%"></i></div>
        <p class="small muted">${s.step === "sending" ? "Text and audio to your Recordem server." : "On this Mac."}</p>`;
    case "done":
      return `
        <h2>Transcript sent ✓</h2>
        <p class="muted">${esc(s.title ?? "Untitled meeting")}</p>
        ${s.warning ? `<div class="notice">${esc(s.warning)}</div>` : ""}
        <div class="row">${s.meeting_id ? `<button class="btn ghost" id="openit">Open in Recordem</button>` : ""}<button class="btn" data-cmd="dismiss">OK</button></div>`;
    case "failed":
      return `<h2>Something went wrong</h2><p class="error">${esc(s.message)}</p><button class="btn" data-cmd="dismiss">OK</button>`;
    default:
      return "";
  }
}

async function tick(force) {
  if (showSettings) { if (force) settings(false); return; }
  const s = await invoke("status");
  if (s.phase === "setup") { if (last !== "setup" || force) { last = "setup"; settings(true); } return; }
  $foot.textContent = s.pending ? `${s.pending} recording${s.pending > 1 ? "s" : ""} waiting to be sent` : $foot.textContent;
  const key = JSON.stringify(s);
  if (!force && key === last) {
    const t = document.querySelector(".timer");
    if (t) t.textContent = clock(t.dataset.since);
    return;
  }
  if (s.phase !== "detected") selected = null;
  last = key;
  $view.innerHTML = render(s);
  for (const b of $view.querySelectorAll("[data-cmd]")) b.onclick = () => run(b.dataset.cmd, {});
  for (const r of $view.querySelectorAll("input[name=meeting]")) r.onchange = () => { selected = r.value; };
  const go = document.getElementById("go");
  if (go) go.onclick = () => {
    const pick = (s.matches ?? []).find((x) => x.meeting_id === selected);
    run("start", { meetingId: pick?.meeting_id ?? null });
  };
  const open = document.getElementById("openit");
  if (open) open.onclick = () => run("open", { meetingId: s.meeting_id });
}

// Version in the footer.
async function version() {
  try {
    const v = await invoke("version");
    const el = document.getElementById("ver") ?? Object.assign(document.createElement("p"), { id: "ver", className: "small muted" });
    el.textContent = `Version ${v.current}`;
    document.body.appendChild(el);
  } catch { /* no footer */ }
}

tick(true);
setInterval(() => tick(false), 700);
version();
