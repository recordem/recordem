// The call prompt (window "prompt", top-right of the screen): when a call starts, the meeting it matches and one
// button to record it, like a notification. It never takes the focus from the call. Other choices (another meeting,
// a new one) are in the main window. Closed by the app once the call is answered or gone.
// It says which recorder asks (the release and a dev build can both be running) and shows the call app's icon.
const { invoke } = window.__TAURI__.core;
const $pill = document.getElementById("pill");
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const PLATFORMS = { google_meet: "Google Meet", zoom: "Zoom", microsoft_teams: "Teams", webex: "Webex" };
let last = "";
let flavor = { app: "Recordem", dev: false };
const icons = {};   // bundle id → data URL, or null when the app has none we can read

async function icon(id) {
  if (!id) return null;
  if (!(id in icons)) icons[id] = await invoke("app_icon", { bundleId: id }).catch(() => null);
  return icons[id];
}

async function tick() {
  const s = await invoke("status");
  if (s.phase !== "detected") return;
  const img = await icon(s.call.app_id);
  const key = JSON.stringify([s.matches, s.error, s.call, img]);
  if (key === last) return;
  last = key;
  const m = s.matches;
  const best = m?.[0];
  const platform = PLATFORMS[s.call.app];
  const browser = s.call.app_name?.replace(/^Google /, "");
  const call = platform && browser && platform !== s.call.app_name ? `${platform} · ${browser}` : platform || s.call.app_name || "a call";
  const who = `<strong>${esc(flavor.app.replace(/ Dev$/, ""))}</strong>${flavor.dev ? `<span class="tag">DEV</span>` : ""}`;
  const where = `${who} · ${esc(call)}`;
  const title = m === null ? "Looking it up…" : best ? esc(best.title || "Untitled meeting") : esc(s.call.title || "A new meeting");
  const other = m && m.length !== 1 ? "choose another" : "other";
  $pill.innerHTML = `
    <div class="ic">${img ? `<img src="${img}" alt=""><b></b>` : "<i></i>"}</div>
    <div class="tx">
      <div class="t1">${where}</div>
      <div class="t2" title="${title}">${title}</div>
      <div class="t3"><a id="other">${other}</a></div>
    </div>
    <button class="go" id="go" ${m === null ? "disabled" : ""}>Record</button>
    <button class="x" id="x" title="Not now" aria-label="Not now">×</button>`;
  document.getElementById("go").onclick = async () => {
    try { await invoke("start", { meetingId: best?.meeting_id ?? null }); } catch (e) { alert(e); }
  };
  document.getElementById("x").onclick = () => invoke("dismiss");
  document.getElementById("other").onclick = () => invoke("show_main");
}

invoke("flavor").then((f) => { flavor = f; document.documentElement.dataset.flavor = f.dev ? "dev" : ""; last = ""; tick(); });
tick();
setInterval(tick, 500);
