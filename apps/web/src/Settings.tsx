// Settings: keys for the workspace's own apps, e.g. a backend that records calls on its users' behalf (the Desktop SDK).
// Keys (API keys) are long-lived Bearer tokens; the advanced kind (machine clients) trades an id and secret for 1 h
// tokens (client_credentials). Either is made in a dialog and shown once. Only the workspace's owners and admins see
// this page work.
import type { components } from "@recordem/sdk";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import { signIn } from "./auth";
import { link } from "./router";

type ApiKey = components["schemas"]["ApiKey"];
type MachineClient = components["schemas"]["MachineClient"];
type Scope = ApiKey["scopes"][number];
type Make = (name: string, scopes: Scope[]) => Promise<[string, string][]>;

const SCOPES: { scope: Scope; label: string; hint: string; on: boolean }[] = [
  { scope: "meetings:write", label: "Record calls", hint: "Start recordings, also from your desktop app", on: true },
  { scope: "meetings:read", label: "See meetings", hint: "Titles, times, people and recordings", on: true },
  { scope: "transcripts:read", label: "Read transcripts and notes", hint: "What was said, and the summary", on: true },
  { scope: "webhooks:manage", label: "Get notified", hint: "Your server hears when a meeting is ready", on: true },
  { scope: "audio:read", label: "Download audio", hint: "The recording's sound files", on: false },
  { scope: "calendars:read", label: "See calendars", hint: "Upcoming events from connected calendars", on: false },
  { scope: "claims:write", label: "Link meetings to its records", hint: "E.g. attach a meeting to a candidate", on: false },
];
const label = (s: Scope) => SCOPES.find((x) => x.scope === s)?.label ?? s;

const date = (iso: string) => {
  const d = new Date(iso);
  return d.toLocaleDateString([], { day: "numeric", month: "short", ...(d.getFullYear() !== new Date().getFullYear() && { year: "numeric" }) });
};

export function Settings() {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [clients, setClients] = useState<MachineClient[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [needsScope, setNeedsScope] = useState(false);
  const [making, setMaking] = useState<"key" | "credential" | null>(null);
  const [showRevoked, setShowRevoked] = useState(false);

  const load = useCallback(async () => {
    const [k, c] = await Promise.all([api.GET("/v1/api-keys"), api.GET("/v1/machine-clients")]);
    const e = k.error ?? c.error;
    if (e) { setNeedsScope(e.error === "insufficient_scope"); setError(e.message); return; }
    setKeys(k.data!.items); setClients(c.data!.items); setError(null);
  }, []);
  useEffect(() => { void load(); }, [load]);

  const makeKey: Make = async (name, scopes) => {
    const r = await api.POST("/v1/api-keys", { body: { name, scopes } });
    if (r.error) throw new Error(r.error.message);
    await load(); return [["Key", r.data.key]];
  };
  const makeCredential: Make = async (name, scopes) => {
    const r = await api.POST("/v1/machine-clients", { body: { name, scopes } });
    if (r.error) throw new Error(r.error.message);
    await load(); return [["ID", r.data.client_id], ["Secret", r.data.client_secret], ["Token URL", r.data.token_endpoint]];
  };
  const active = keys?.filter((k) => !k.revoked_at) ?? [];
  const revoked = keys?.filter((k) => k.revoked_at) ?? [];

  return (
    <div className="shell">
      <header className="top">
        <a className="brand" {...link("/")}><span className="dot" /> Recordem</a>
        <a className="muted small" {...link("/")}>← All meetings</a>
      </header>
      <main className="settings">
        <h1>Settings</h1>
        {error && (needsScope
          ? <p className="muted">To manage keys, sign in again. <button className="ghost small" onClick={() => signIn("/settings")}>Sign in</button></p>
          : <p className="error">{error}</p>)}
        {!keys && !error && <p className="muted">Loading…</p>}
        {keys && clients && (
          <>
            <section className="card">
              <header>
                <div>
                  <h2>Keys</h2>
                  <p className="muted">Let your own apps record calls and read meetings in this workspace.</p>
                </div>
                {active.length > 0 && <button onClick={() => setMaking("key")}>New key</button>}
              </header>
              {active.length > 0 ? (
                <ul className="creds">
                  {active.map((k) => (
                    <Item key={k.id} name={k.name} id={`${k.prefix}…`} scopes={k.scopes}
                      when={k.last_used_at ? `Last used ${date(k.last_used_at)}` : `Created ${date(k.created_at)}, not used yet`}
                      action={["Revoke", async () => {
                        if (!confirm(`Revoke "${k.name}"? The app using it stops working within a minute.`)) return;
                        await api.DELETE("/v1/api-keys/{id}", { params: { path: { id: k.id } } }); await load();
                      }]} />
                  ))}
                </ul>
              ) : (
                <div className="empty">
                  <span className="empty-icon" aria-hidden>
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                      <circle cx="8" cy="15" r="4" /><path d="M10.8 12.2 20 3M16 7l3 3M14 9l2 2" /></svg>
                  </span>
                  <b>No keys yet</b>
                  <p className="muted">Create one for each app, so you can turn one off without touching the others.</p>
                  <button onClick={() => setMaking("key")}>Create a key</button>
                </div>
              )}
              {revoked.length > 0 && (
                <footer>
                  <button className="link" onClick={() => setShowRevoked((x) => !x)}>
                    {showRevoked ? "Hide" : "Show"} {revoked.length} revoked {revoked.length === 1 ? "key" : "keys"}
                  </button>
                  {showRevoked && <ul className="creds off">
                    {revoked.map((k) => <Item key={k.id} name={k.name} id={`${k.prefix}…`} scopes={k.scopes} when={`Revoked ${date(k.revoked_at!)}`} />)}
                  </ul>}
                </footer>
              )}
            </section>

            <details className="card advanced" open={clients.length > 0}>
              <summary>
                <div>
                  <h2>Expiring credentials</h2>
                  <p className="muted">Advanced. For security policies that don't allow keys that last forever.</p>
                </div>
              </summary>
              <p className="muted small explain">The app trades an ID and a secret for an access token that expires after an hour (OAuth client credentials).</p>
              {clients.length > 0 && (
                <ul className="creds">
                  {clients.map((c) => (
                    <Item key={c.client_id} name={c.name} id={c.client_id} scopes={c.scopes} when={`Created ${date(c.created_at)}`}
                      action={["Delete", async () => {
                        if (!confirm(`Delete "${c.name}"? It gets no new tokens; the ones it has work for up to an hour.`)) return;
                        await api.DELETE("/v1/machine-clients/{id}", { params: { path: { id: c.client_id } } }); await load();
                      }]} />
                  ))}
                </ul>
              )}
              <div className="explain"><button className="ghost" onClick={() => setMaking("credential")}>New credential</button></div>
            </details>
          </>
        )}
        {making && <MakeDialog noun={making} make={making === "key" ? makeKey : makeCredential} onClose={() => setMaking(null)} />}
      </main>
    </div>
  );
}

function Item({ name, id, scopes, when, action }: {
  name: string; id: string; scopes: Scope[]; when: string; action?: [string, () => Promise<void>];
}) {
  return (
    <li>
      <div className="cred-main">
        <div><b>{name}</b> <code>{id}</code></div>
        <div className="perms">{scopes.map((s) => <span key={s}>{label(s)}</span>)}</div>
      </div>
      <div className="cred-side">
        <span className="muted small">{when}</span>
        {action && <button className="ghost small" onClick={action[1]}>{action[0]}</button>}
      </div>
    </li>
  );
}

/** Name it and pick what it can do; then what was made, shown once, to copy. */
function MakeDialog({ noun, make, onClose }: { noun: string; make: Make; onClose: () => void }) {
  const box = useRef<HTMLDialogElement>(null);
  useEffect(() => { box.current?.showModal(); }, []);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<Scope[]>(SCOPES.filter((s) => s.on).map((s) => s.scope));
  const [made, setMade] = useState<[string, string][] | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setError(null); setBusy(true);
    try { setMade(await make(name.trim(), scopes)); } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
  };
  const copy = (k: string, v: string) => { void navigator.clipboard?.writeText(v); setCopied(k); };
  return (
    <dialog ref={box} className="details make" onClose={onClose}>
      {made ? (
        <>
          <h3>Your new {noun}</h3>
          <p className="muted">Copy it now and keep it somewhere safe. It won't be shown again.</p>
          <div className="secret">
            {made.map(([k, v]) => <div key={k}><span className="muted small">{k}</span><code>{v}</code>
              <button className="ghost small" onClick={() => copy(k, v)}>{copied === k ? "Copied" : "Copy"}</button></div>)}
          </div>
          <div className="actions"><button onClick={() => box.current?.close()}>Done</button></div>
        </>
      ) : (
        <form onSubmit={(e) => { e.preventDefault(); void submit(); }}>
          <h3>New {noun}</h3>
          <label className="field">Name
            <input placeholder="The app that will use it, e.g. Hiring app" value={name} onChange={(e) => setName(e.target.value)} maxLength={100} required autoFocus />
          </label>
          <fieldset>
            <legend>What it can do</legend>
            {SCOPES.map((s) => (
              <label key={s.scope} className="perm"><input type="checkbox" checked={scopes.includes(s.scope)}
                onChange={(e) => setScopes((x) => (e.target.checked ? [...x, s.scope] : x.filter((y) => y !== s.scope)))} />
                <span>{s.label}<span className="muted small">{s.hint}</span></span></label>
            ))}
          </fieldset>
          {error && <p className="error small">{error}</p>}
          <div className="actions">
            <button type="button" className="ghost" onClick={() => box.current?.close()}>Cancel</button>
            <button disabled={busy || !name.trim() || !scopes.length}>Create {noun}</button>
          </div>
        </form>
      )}
    </dialog>
  );
}
