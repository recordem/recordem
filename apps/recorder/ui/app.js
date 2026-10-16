// The recorder's window. All state lives in Rust (src-tauri/src/lib.rs); this polls `status` and renders the phase.
const { invoke } = window.__TAURI__.core;
const $view = document.getElementById("view");
const $foot = document.getElementById("foot");
let showSettings = false;
let showPerms = false;   // Settings → Permissions
let permsKey = "";       // what the permissions page shows, so the refresh only redraws on a change
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

/** The window's header: the app and its settings button, or, on a sub-page, the way back and the page's name. */
function header(page) {
  const $top = document.getElementById("top");
  const key = page ? `${page.title}|${page.back}` : "main";
  if ($top.dataset.key === key) return;
  $top.dataset.key = key;
  $top.className = page ? "sub" : "";
  $top.innerHTML = page
    ? `<button class="back" id="back">‹ ${esc(page.back)}</button><strong>${esc(page.title)}</strong><span></span>`
    : `<span class="logo" aria-hidden="true"></span><strong>Recordem</strong>${document.documentElement.dataset.flavor === "dev" ? `<span class="tag">DEV</span>` : ""}
       <button id="gear" class="icon" title="Settings" aria-label="Settings">⚙</button>`;
  if (page) document.getElementById("back").onclick = page.onBack;
  else document.getElementById("gear").onclick = () => { showSettings = true; showPerms = false; tick(true); };
}
const toMain = () => { showSettings = false; showPerms = false; permsKey = ""; tick(true); };
const toSettings = () => { showPerms = false; showSettings = true; permsKey = ""; tick(true); };

const toggle = (id, on) => `<label class="switch"><input type="checkbox" id="${id}" ${on ? "checked" : ""}><i></i></label>`;
const LANGS = [["en", "English"], ["pt", "Portuguese"], ["es", "Spanish"], ["fr", "French"], ["de", "German"]];

/** The server's addresses: shown open before signing in, folded away after. */
function serverGroup(c, open) {
  return `
    <details ${open ? "open" : ""}><summary class="group-title">Server ›</summary>
      <div class="group">
        <div class="item stack"><div class="label"><b>Portal</b></div><input id="web" type="text" value="${esc(c.web_url)}"></div>
        <div class="item stack"><div class="label"><b>API</b></div><input id="api" type="text" value="${esc(c.api_url)}"></div>
        <div class="item stack"><div class="label"><b>Sign-in</b></div><input id="auth" type="text" value="${esc(c.auth_url)}"></div>
      </div>
      ${open ? "" : `<p class="group-note">Changing the server signs you out.</p>`}
    </details>`;
}

async function settings() {
  const c = await invoke("get_config");
  const val = (id) => document.getElementById(id)?.value ?? "";
  if (!c.signed_in) {
    $view.innerHTML = `
      <h2>Sign in to Recordem</h2>
      <p class="muted">Your browser opens the sign-in and comes back here.</p>
      <button class="btn" id="login">Sign in</button>
      <p id="err" class="error" style="margin-top:10px"></p>
      ${serverGroup(c, !c.api_url)}`;
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
  const items = await invoke("permission_list").catch(() => []);
  const missing = items.filter((i) => i.status !== "granted").length;
  const initial = (c.account || "?").trim()[0]?.toUpperCase() ?? "?";
  let langs = [...c.languages];
  $view.innerHTML = `

    <div class="group-title">Account</div>
    <div class="group">
      <div class="item"><span class="avatar">${esc(initial)}</span>
        <div class="label"><b>${esc(c.account || "Signed in")}</b><span>${esc(new URL(c.web_url || c.api_url).host)}</span></div>
        <button class="btn ghost sm" id="logout">Sign out</button></div>
    </div>

    <div class="group-title">Recording</div>
    <div class="group">
      <div class="item"><div class="label"><b>When a call starts</b></div>
        <select id="oncall">${[["ask", "Ask"], ["auto", "Record if it matches"], ["off", "Do nothing"]]
          .map(([v, l]) => `<option value="${v}" ${c.on_call === v ? "selected" : ""}>${l}</option>`).join("")}</select></div>
      <div class="item"><div class="label"><b>Microphone</b></div>
        <select id="mic"><option value="">System default</option>${c.mics
          .map((m) => `<option value="${esc(m)}" ${c.mic_device === m ? "selected" : ""}>${esc(m)}</option>`).join("")}</select></div>
      <div class="item"><div class="label"><b>My voice in high quality</b><span>48 kHz, to listen back to how you speak</span></div>${toggle("hq", c.voice_hq)}</div>
      <div class="item"><div class="label"><b>Keep audio on this Mac</b><span>Besides the server's copy</span></div>${toggle("keep", c.keep_audio)}</div>
      <div class="item"><div class="label"><b>Help fix call readers</b><span>When reading Meet or Zoom breaks, send what its call area looked like (the tiles, not the chat)</span></div>${toggle("dumps", c.share_reader_dumps)}</div>
    </div>

    <div class="group-title">Transcription</div>
    <div class="group">
      <div class="item stack"><div class="label"><b>Languages</b><span>Each turn is transcribed in the one it sounds like</span></div>
        <div class="chips" id="langs">${LANGS.map(([v, l]) => `<button data-lang="${v}" aria-pressed="${langs.includes(v)}">${l}</button>`).join("")}</div></div>
      <div class="item"><div class="label"><b>Live transcript</b><span>While the call goes on, in the portal</span></div>${toggle("live", c.live)}</div>
      <div class="item stack"><div class="label"><b>Vocabulary</b><span>Names and terms Whisper should expect, one per line</span></div>
        <textarea id="vocab" rows="3" placeholder="Kubernetes&#10;OKR">${esc(c.vocabulary)}</textarea></div>
    </div>

    <div class="group-title">General</div>
    <div class="group">
      <div class="item link" id="perms"><div class="label"><b>Permissions</b></div>
        <span class="badge ${missing ? "warn" : "ok"}">${missing ? `${missing} to review` : "All allowed"}</span><span class="chev">›</span></div>
      <div class="item"><div class="label"><b>Open at login</b></div>${toggle("atlogin", c.launch_at_login)}</div>
    </div>

    ${serverGroup(c, false)}
    <p id="err" class="error" style="margin-top:8px"></p>`;

  // Every change is saved as it's made; text fields when they lose focus.
  const save = async () => {
    const on = (id) => document.getElementById(id).checked;
    try {
      await invoke("save_config", { input: {
        api_url: val("api"), auth_url: val("auth"), web_url: val("web"), languages: langs, keep_audio: on("keep"), share_reader_dumps: on("dumps"), voice_hq: on("hq"),
        launch_at_login: on("atlogin"), on_call: val("oncall"), mic_device: val("mic") || null, vocabulary: val("vocab"), live: on("live") } });
      document.getElementById("err").textContent = "";
    } catch (e) { document.getElementById("err").textContent = e; }
  };
  for (const id of ["oncall", "mic", "hq", "keep", "live", "atlogin"]) document.getElementById(id).onchange = save;
  for (const id of ["vocab", "web", "api", "auth"]) document.getElementById(id).onchange = save;
  for (const b of document.querySelectorAll("[data-lang]")) b.onclick = () => {
    const v = b.dataset.lang, on = langs.includes(v);
    if (on && langs.length === 1) return;          // at least one language
    langs = on ? langs.filter((x) => x !== v) : [...langs, v];
    b.setAttribute("aria-pressed", String(!on));
    save();
  };
  document.getElementById("logout").onclick = async () => {
    if (confirm("Sign out on this Mac?")) { await invoke("logout"); showSettings = false; tick(true); }
  };
  document.getElementById("perms").onclick = () => { showSettings = false; showPerms = true; permsKey = ""; tick(true); };
}

const PERMS = {
  microphone: ["Microphone", "Your voice"],
  system_audio: ["System audio", "The other participants"],
  accessibility: ["Accessibility", "The meeting's name and code, from its window"],
  automation: ["Browser", "The meeting's link, from its tab"],
};

/** One row per permission: what it's for, and its state or the one action that moves it forward. */
function permRows(items) {
  return items.map((i) => {
    const [name, why] = PERMS[i.id];
    const action =
      i.status === "granted" ? `<span class="perm-state">✓ Allowed</span>`
      : i.status === "denied" ? `<button class="btn ghost sm perm-btn" data-open="${i.id}">Open Settings</button>`
      : i.status === "unknown" ? `<span class="perm-state wait">Open a browser</span>`
      : `<button class="btn sm perm-btn" data-perm="${i.id}">Allow</button>`;
    const hint = i.id === "accessibility" && i.status === "denied" ? " · if it's on there, turn it off and on" : "";
    return `<div class="item"><div class="label"><b>${name}</b><span>${why}${hint}</span></div>${action}</div>`;
  }).join("");
}

/** Settings → Permissions: every permission, refreshed while open (they change in System Settings). */
async function permissionsPage() {
  const items = await invoke("permission_list");
  const key = JSON.stringify(items);
  if (key === permsKey) return;
  permsKey = key;
  $view.innerHTML = `
    <div class="group-title">Needed to record</div>
    <div class="group">${permRows(items.filter((i) => i.required))}</div>
    <div class="group-title">Recommended</div>
    <div class="group">${permRows(items.filter((i) => !i.required))}</div>
    <p class="group-note">These link each recording to the right meeting. Without them, recordings are matched by time.</p>`;
  bindPerms();
}

function bindPerms() {
  for (const b of $view.querySelectorAll("[data-perm]")) b.onclick = async () => {
    b.disabled = true;
    try { await invoke("request_permission", { id: b.dataset.perm }); } catch (e) { alert(e); }
    permsKey = ""; tick(true);
  };
  for (const b of $view.querySelectorAll("[data-open]")) b.onclick = () => invoke("open_permission", { id: b.dataset.open });
}

/** The meeting's name inside a browser window's title ("Meet - Weekly sync - Camera and microphone recording - Google
 *  Chrome - Profile" → "Weekly sync"); the recorder cleans it the same way before sending it. */
const callName = (t) => (t ?? "").split(/ - (?:Camera and microphone recording|Microphone recording|Camera recording|Google Chrome|Microsoft Edge|Brave|Arc)\b/)[0]
  .replace(/^Meet [-–] /, "").replace(/^[a-z]{3}-[a-z]{4}-[a-z]{3}$/, "").trim();

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
    case "permissions": {
      const ready = s.items.every((i) => i.status === "granted");
      return `
        <h2>Allow recording</h2>
        <p class="muted">macOS asks for these once. Both are needed to record a call.</p>
        <div class="group">${permRows(s.items)}</div>
        <p class="group-note">More, recommended, in Settings → Permissions.</p>
        <button class="btn" id="perm-done" style="margin-top:14px" ${ready ? "" : "disabled"}>Continue</button>`;
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
          <span><strong>A new meeting</strong><br><span class="small muted">${esc(callName(s.call.title) || "Named after the call window")}</span></span></label>`].join("");
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
  if (showPerms) { header({ title: "Permissions", back: "Settings", onBack: toSettings }); await permissionsPage(); return; }
  if (showSettings) { header({ title: "Settings", back: "Back", onBack: toMain }); if (force) settings(); return; }
  header(null);
  const s = await invoke("status");
  if (s.phase === "setup") { if (last !== "setup" || force) { last = "setup"; settings(); } return; }
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
  bindPerms();
  const done = document.getElementById("perm-done");
  if (done) done.onclick = () => run("finish_onboarding", {});
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

// A dev build shows it (violet, a DEV tag) so it never passes for the release.
invoke("flavor").then((f) => {
  document.documentElement.dataset.flavor = f.dev ? "dev" : "";
  document.getElementById("top").dataset.key = "";
  tick(true);
});
tick(true);
setInterval(() => tick(false), 700);
version();
