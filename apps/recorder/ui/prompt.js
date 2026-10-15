// The call prompt (window "prompt", top-right of the screen): when a call starts, the meeting it matches and one
// button to record it, like a notification. It never takes the focus from the call. Other choices (another meeting,
// a new one) are in the main window. Closed by the app once the call is answered or gone.
const { invoke } = window.__TAURI__.core;
const $pill = document.getElementById("pill");
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
let last = "";

async function tick() {
  const s = await invoke("status");
  if (s.phase !== "detected") return;
  const key = JSON.stringify([s.matches, s.error]);
  if (key === last) return;
  last = key;
  const m = s.matches;
  const best = m?.[0];
  const where = s.call.app_name ? `Call in ${esc(s.call.app_name)}` : "Call detected";
  const title = m === null ? "Looking it up…" : best ? esc(best.title || "Untitled meeting") : esc(s.call.title || "A new meeting");
  const other = m && m.length !== 1 ? "choose another" : "other";
  $pill.innerHTML = `
    <div class="ic"><i></i></div>
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

tick();
setInterval(tick, 500);
